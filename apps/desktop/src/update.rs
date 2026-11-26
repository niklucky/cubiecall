//! Sparkle is embedded in packaged Mac builds and manages its own update UI.

use std::cell::OnceCell;

use anyhow::{Context as _, Result};
use gpui::App;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{NSBundle, NSMutableURLRequest, NSObject, NSString, NSURL};

use crate::auth::{CUBIE, PRIMARY};

define_class!(
    // SAFETY: NSObject has no subclassing requirements. Sparkle calls its
    // updater delegate on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CubieUpdateDelegate"]
    #[ivars = String]
    struct UpdateDelegate;

    impl UpdateDelegate {
        #[unsafe(method_id(feedURLStringForUpdater:))]
        fn feed_url(&self, _updater: &AnyObject) -> Retained<NSString> {
            let url = update_url(self.ivars(), CUBIE.origin());
            NSString::from_str(url.as_deref().unwrap_or(self.ivars()))
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
            if let Some(url) = update_url(&current.to_string(), CUBIE.origin())
                && let Some(url) = NSURL::URLWithString(&NSString::from_str(&url))
            {
                request.setURL(Some(&url));
            }
        }
    }
);

fn update_url(url: &str, origin: &str) -> Option<String> {
    let path = url.strip_prefix(PRIMARY)?.strip_prefix("/download/")?;
    Some(format!("{origin}/download/{path}"))
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
        CUBIE.probe().await;
        cx.update(|_| {
            if let Err(error) = start_updater(&NSBundle::mainBundle(), feed) {
                tracing::warn!("starting software updates: {error:#}");
            }
        });
    })
    .detach();
}

fn start_updater(bundle: &NSBundle, feed: String) -> Result<()> {
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
    let delegate = UpdateDelegate::alloc(mtm).set_ivars(feed);
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
        let (controller, _) = updater.get_or_init(|| (controller, delegate));
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str("Check for Updates…"));
        // SAFETY: Sparkle's controller implements this action and stays retained in UPDATER.
        unsafe {
            item.setTarget(Some(controller));
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
        for path in [
            "current/appcast-apple-silicon.xml",
            "releases/120/Cubie-apple-silicon.zip",
        ] {
            assert_eq!(
                update_url(
                    &format!("{PRIMARY}/download/{path}"),
                    "https://cubiecall.ru"
                ),
                Some(format!("https://cubiecall.ru/download/{path}"))
            );
        }
        assert_eq!(
            update_url(
                "https://cubiecall.com.example/download/update.zip",
                "https://cubiecall.ru"
            ),
            None
        );
    }
}
