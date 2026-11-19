//! AppKit delivers the recovery redraw on the main thread.

use gpui::{Bounds, Empty, Platform, WindowHandle, WindowKind, WindowParams, px, size};
use gpui_macos::MacPlatform;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSView};
use objc2_foundation::NSSize;
use raw_window_handle::RawWindowHandle;
use std::{cell::Cell, rc::Rc};

fn main() {
    let marker = MainThreadMarker::new().expect("AppKit tests run on the main thread");
    let _app = NSApplication::sharedApplication(marker);
    let platform = MacPlatform::new(false);
    let window = platform
        .open_window(
            WindowHandle::<Empty>::new(1.into()).into(),
            WindowParams {
                bounds: Bounds::new(Default::default(), size(px(240.0), px(90.0))),
                kind: WindowKind::PopUp,
                titlebar: None,
                is_movable: false,
                app_owns_titlebar_drag: false,
                is_resizable: false,
                is_minimizable: false,
                focus: false,
                show: false,
                icon: None,
                display_id: None,
                app_id: None,
                window_min_size: None,
                tabbing_identifier: None,
                use_display_p3: false,
            },
        )
        .unwrap();
    let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
        panic!("expected an AppKit view");
    };
    // SAFETY: the window owns this view and both live on the main thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
        .unwrap()
        .setContentSize(NSSize::new(240.0, 52.0));
    view.displayIfNeeded();

    let required_frames = Rc::new(Cell::new(0));
    window.on_request_frame(Box::new({
        let required_frames = required_frames.clone();
        move |frame| {
            if frame.require_presentation {
                required_frames.set(required_frames.get() + 1);
            }
        }
    }));

    view.setNeedsDisplay(true);
    view.displayIfNeeded();
    assert_eq!(
        required_frames.get(),
        1,
        "AppKit should request a fresh presentation"
    );
    println!("AppKit requested presentation after a view was marked for redraw");
}
