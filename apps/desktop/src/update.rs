//! Sparkle is embedded in packaged Mac builds and manages its own update UI.

use std::cell::OnceCell;

use anyhow::{Context as _, Result};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{NSBundle, NSString};

thread_local! {
    // AppKit menu items hold their targets weakly.
    static UPDATER: OnceCell<Retained<AnyObject>> = const { OnceCell::new() };
}

pub fn start() {
    let bundle = NSBundle::mainBundle();
    if bundle
        .objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"))
        .is_none()
    {
        return;
    }

    if let Err(error) = start_updater(&bundle) {
        tracing::warn!("starting software updates: {error:#}");
    }
}

fn start_updater(bundle: &NSBundle) -> Result<()> {
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
    // SAFETY: These selectors and types match Sparkle's public Objective-C API.
    let controller: Option<Retained<AnyObject>> = unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![
            allocated,
            initWithStartingUpdater: Bool::YES,
            updaterDelegate: None::<&AnyObject>,
            userDriverDelegate: None::<&AnyObject>,
        ]
    };
    let controller = controller.context("could not initialize Sparkle updater controller")?;
    UPDATER.with(|updater| {
        let controller = updater.get_or_init(|| controller);
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
