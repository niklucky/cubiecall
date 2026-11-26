//! Sparkle is embedded in packaged Mac builds and manages its own update UI.

use std::{
    cell::{Cell, OnceCell, RefCell},
    rc::Rc,
    time::Duration,
};

use anyhow::{Context as _, Result};
use gpui::{App, AsyncApp, Task};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{NSBundle, NSMutableURLRequest, NSObject, NSString, NSURL};

use crate::{SERVER, auth::FAILOVER};

async fn update_origin(previous: &'static str) -> &'static str {
    match FAILOVER
        .get()
        .filter(|route| route.primary() == SERVER.trim_end_matches('/'))
    {
        Some(route) => route.reachable_origin(previous).await,
        None => SERVER,
    }
}

struct UpdateState {
    feed: String,
    app: AsyncApp,
    origin: Rc<Cell<&'static str>>,
    scheduled_probe: RefCell<Option<Task<()>>>,
    checking: Rc<Cell<bool>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. Sparkle calls its
    // updater delegate on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CubieUpdateDelegate"]
    #[ivars = UpdateState]
    struct UpdateDelegate;

    impl UpdateDelegate {
        #[unsafe(method_id(feedURLStringForUpdater:))]
        fn feed_url(&self, _updater: &AnyObject) -> Retained<NSString> {
            let state = self.ivars();
            let url = update_url(&state.feed, SERVER, state.origin.get());
            NSString::from_str(url.as_deref().unwrap_or(&state.feed))
        }

        #[unsafe(method(updater:willDownloadUpdate:withRequest:))]
        fn prepare_download(
            &self,
            _updater: &AnyObject,
            _item: &AnyObject,
            request: &NSMutableURLRequest,
        ) {
            let Some(current) = request.URL().and_then(|url| url.absoluteString()) else {
                return;
            };
            if let Some(url) = update_url(&current.to_string(), SERVER, self.ivars().origin.get())
                && let Some(url) = NSURL::URLWithString(&NSString::from_str(&url))
            {
                request.setURL(Some(&url));
            }
        }

        #[unsafe(method(updater:willScheduleUpdateCheckAfterDelay:))]
        fn schedule_probe(&self, _updater: &AnyObject, delay: f64) {
            let state = self.ivars();
            let origin = state.origin.clone();
            // A probe can take up to six seconds. Give it time to finish
            // before Sparkle's timer fires, including when the Mac is signed out.
            let delay = Duration::try_from_secs_f64((delay - 8.0).max(0.0)).unwrap_or_default();
            state.scheduled_probe.replace(Some(state.app.spawn(async move |cx| {
                cx.background_executor().timer(delay).await;
                origin.set(update_origin(origin.get()).await);
            })));
        }

        #[unsafe(method(updaterWillNotScheduleUpdateCheck:))]
        fn cancel_probe(&self, _updater: &AnyObject) {
            self.ivars().scheduled_probe.replace(None);
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> Bool {
            if self.ivars().checking.get() {
                return Bool::NO;
            }
            UPDATER.with(|updater| {
                updater.get().map_or(Bool::NO, |(controller, _)| {
                    // SAFETY: Sparkle's controller validates this same menu action.
                    unsafe { msg_send![&**controller, validateMenuItem: item] }
                })
            })
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: &NSMenuItem) {
            let state = self.ivars();
            if state.checking.replace(true) {
                return;
            }
            let origin = state.origin.clone();
            let checking = state.checking.clone();
            state.app.spawn(async move |_| {
                origin.set(update_origin(origin.get()).await);
                checking.set(false);
                UPDATER.with(|updater| {
                    if let Some((controller, _)) = updater.get() {
                        // SAFETY: Invoke Sparkle's original menu action on the main thread.
                        let _: () = unsafe { msg_send![&**controller, checkForUpdates: None::<&AnyObject>] };
                    }
                });
            }).detach();
        }
    }
);

fn update_url(url: &str, primary: &str, origin: &str) -> Option<String> {
    let path = url
        .strip_prefix(primary.trim_end_matches('/'))?
        .strip_prefix("/download/")?;
    Some(format!("{}/download/{path}", origin.trim_end_matches('/')))
}

thread_local! {
    // The menu and Sparkle hold their target and delegate weakly.
    static UPDATER: OnceCell<(Retained<AnyObject>, Retained<UpdateDelegate>)> = const { OnceCell::new() };
}

pub fn start(cx: &mut App) {
    let bundle = NSBundle::mainBundle();
    let Some(feed) = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"))
        .and_then(|value| value.downcast::<NSString>().ok())
    else {
        return;
    };
    let feed = feed.to_string();

    cx.spawn(async move |cx| {
        // Even a signed-out Mac checks which host it can reach before
        // Sparkle makes its first feed request.
        let origin = update_origin(SERVER).await;
        if let Err(error) = start_updater(&NSBundle::mainBundle(), feed, origin, cx.clone()) {
            tracing::warn!("starting software updates: {error:#}");
        }
    })
    .detach();
}

fn start_updater(
    bundle: &NSBundle,
    feed: String,
    origin: &'static str,
    app: AsyncApp,
) -> Result<()> {
    let mtm = MainThreadMarker::new().context("Sparkle must run on the main thread")?;
    let frameworks = bundle
        .privateFrameworksPath()
        .context("missing app Frameworks path")?;
    let path = NSString::from_str(&format!("{frameworks}/Sparkle.framework"));
    let framework = NSBundle::bundleWithPath(&path).context("missing Sparkle.framework")?;
    // SAFETY: Load the Sparkle framework from the signed app bundle, on the main thread.
    unsafe { framework.loadAndReturnError() }
        .map_err(|error| anyhow::anyhow!("load Sparkle.framework: {error}"))?;
    let class = AnyClass::get(c"SPUStandardUpdaterController")
        .context("missing Sparkle updater controller class")?;
    let delegate = UpdateDelegate::alloc(mtm).set_ivars(UpdateState {
        feed,
        app,
        origin: Rc::new(Cell::new(origin)),
        scheduled_probe: RefCell::new(None),
        checking: Rc::new(Cell::new(false)),
    });
    // SAFETY: NSObject's plain init.
    let delegate: Retained<UpdateDelegate> = unsafe { msg_send![super(delegate), init] };
    // SAFETY: These selectors and types match Sparkle's public Objective-C API.
    let controller: Option<Retained<AnyObject>> = unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![
            allocated,
            initWithStartingUpdater: Bool::YES,
            updaterDelegate: &*delegate,
            userDriverDelegate: None::<&AnyObject>,
        ]
    };
    let controller = controller.context("could not initialize Sparkle updater controller")?;
    UPDATER.with(|updater| {
        let (_, delegate) = updater.get_or_init(|| (controller, delegate));
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str("Check for Updates…"));
        // SAFETY: The delegate implements the action and stays retained in UPDATER.
        unsafe {
            item.setTarget(Some(delegate));
            item.setAction(Some(sel!(checkForUpdates:)));
        }
        app_menu(mtm).insertItem_atIndex(&item, 0);
    });
    Ok(())
}

fn app_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let app = NSApplication::sharedApplication(mtm);
    let menu_bar = app.mainMenu().unwrap_or_else(|| NSMenu::new(mtm));
    if menu_bar.numberOfItems() > 0
        && let Some(menu) = menu_bar.itemAtIndex(0).and_then(|item| item.submenu())
    {
        return menu;
    }

    let menu = NSMenu::new(mtm);
    let quit = NSMenuItem::new(mtm);
    quit.setTitle(&NSString::from_str("Quit Cubie"));
    quit.setKeyEquivalent(&NSString::from_str("q"));
    // SAFETY: AppKit routes the standard terminate: action to NSApplication.
    unsafe { quit.setAction(Some(sel!(terminate:))) };
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&quit);
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str("Cubie"));
    item.setSubmenu(Some(&menu));
    menu_bar.insertItem_atIndex(&item, 0);
    app.setMainMenu(Some(&menu_bar));
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserve_serves_the_feed_and_signed_archive_at_the_same_paths() {
        let primary = "https://call.example.test";
        let reserve = "https://backup.example.test";
        for path in [
            "current/appcast-apple-silicon.xml",
            "releases/120/Cubie-apple-silicon.zip",
        ] {
            assert_eq!(
                update_url(&format!("{primary}/download/{path}"), primary, reserve),
                Some(format!("{reserve}/download/{path}"))
            );
        }
        assert_eq!(
            update_url(
                "https://call.example.test.untrusted.test/download/update.zip",
                primary,
                reserve
            ),
            None
        );
    }
}
