//! AppKit touches GPUI doesn't expose: shaped borderless windows.
//!
//! A floating circle is a square, transparent, borderless panel whose content
//! layer is masked to a circle. The compositor does the clipping, so video
//! inside costs nothing extra. GPUI can't round video corners itself.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use gpui::{
    App, Context, Size, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplicationDidBecomeActiveNotification, NSApplicationDidChangeScreenParametersNotification,
    NSApplicationDidHideNotification, NSApplicationDidResignActiveNotification,
    NSApplicationDidUnhideNotification, NSEvent, NSMenu, NSMenuItem, NSSound, NSView, NSWindow,
    NSWindowDidChangeOcclusionStateNotification, NSWindowDidChangeScreenNotification,
    NSWindowOcclusionState, NSWindowOrderingMode, NSWindowStyleMask, NSWindowWillCloseNotification,
    NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObject, NSPoint, NSRect, NSRunLoop,
    NSSize, NSString,
};
use objc2_quartz_core::CATransaction;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The visible shape of a floating window.
#[derive(Clone, Debug)]
pub enum Mask {
    /// Rounded rectangle with this corner radius.
    Rounded(f64),
}

/// Which point of the window stays put when it's resized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Center,
    /// Resize handles: the corner opposite the handle stays put.
    TopLeft,
    /// A point of the content stays put: `before` from the window's top-left
    /// corner now, `after` from it once resized. The call widget's pill, by
    /// the corner its buttons end at, with things growing around it.
    Point {
        before: (f32, f32),
        after: (f32, f32),
    },
}

/// A rectangle on screen, in AppKit's coordinates: `y` is the bottom edge
/// and grows upwards.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn top(&self) -> f64 {
        self.y + self.h
    }

    /// How far apart two rectangles are; 0 if they touch or overlap.
    pub fn distance(&self, other: &Rect) -> f64 {
        let dx = (other.x - self.right())
            .max(self.x - other.right())
            .max(0.0);
        let dy = (other.y - self.top()).max(self.y - other.top()).max(0.0);
        dx.hypot(dy)
    }

    fn ns(&self) -> NSRect {
        NSRect::new(NSPoint::new(self.x, self.y), NSSize::new(self.w, self.h))
    }
}

impl From<NSRect> for Rect {
    fn from(r: NSRect) -> Self {
        Self {
            x: r.origin.x,
            y: r.origin.y,
            w: r.size.width,
            h: r.size.height,
        }
    }
}

/// A window of ours, to move and size from another window's view: the call
/// widget places the bubbles that ride on it.
#[derive(Clone)]
pub struct Native(Retained<NSWindow>);

impl Native {
    pub fn of(window: &Window) -> Option<Self> {
        ns_window(window).map(Self)
    }

    pub fn frame(&self) -> Rect {
        self.0.frame().into()
    }

    /// The usable part of the screen the window is on.
    pub fn visible(&self) -> Option<Rect> {
        Some(self.0.screen()?.visibleFrame().into())
    }

    pub fn shown(&self) -> bool {
        self.0.isVisible()
    }

    /// Whether the window rides on `parent`, moving when it moves.
    pub fn rides_on(&self, parent: &Native) -> bool {
        self.0.parentWindow().is_some_and(|p| p == parent.0)
    }
}

/// Where a window goes: its frame and corner radius, and whether it rides
/// on another window (under it, moving with it) or floats by itself.
pub struct Placement {
    pub window: Native,
    pub frame: Rect,
    /// `None` to keep the shape it has.
    pub radius: Option<f64>,
    pub parent: Option<Native>,
    /// Out of sight for now (its widget is tucked away).
    pub hidden: bool,
}

/// Put windows where they go, on the next run-loop turn (see [`resize`]).
pub fn place(placements: Vec<Placement>, cx: &App) {
    if placements.is_empty() {
        return;
    }
    cx.foreground_executor()
        .spawn(async move { place_now(placements) })
        .detach();
}

/// Only outside any GPUI update: AppKit calls back into GPUI.
fn place_now(placements: Vec<Placement>) {
    for p in placements {
        let ns = &p.window.0;
        let old = ns.parentWindow();
        tracing::debug!(
            "place window {}: {:.0}x{:.0} at ({:.0}, {:.0}), radius {:.0}, {}",
            ns.windowNumber(),
            p.frame.w,
            p.frame.h,
            p.frame.x,
            p.frame.y,
            p.radius.unwrap_or(-1.0),
            match (p.hidden, &p.parent) {
                (true, _) => "hidden",
                (false, Some(_)) => "riding",
                (false, None) => "by itself",
            }
        );
        if p.hidden {
            if let Some(old) = &old {
                old.removeChildWindow(ns);
            }
            ns.orderOut(None);
            continue;
        }
        if ns.frame() != p.frame.ns() {
            ns.setFrame_display(p.frame.ns(), true);
        }
        if let Some(radius) = p.radius {
            apply_mask(ns, &Mask::Rounded(radius));
        }
        match (&p.parent, old) {
            (Some(parent), Some(old)) if parent.0 == old => {}
            (Some(parent), old) => {
                if let Some(old) = old {
                    old.removeChildWindow(ns);
                }
                ns.setHasShadow(false);
                // SAFETY: both windows are ours and alive; a child can't be
                // its parent's parent here (bubbles have no children).
                unsafe {
                    parent
                        .0
                        .addChildWindow_ordered(ns, NSWindowOrderingMode::Below)
                };
            }
            (None, Some(old)) => {
                old.removeChildWindow(ns);
                ns.orderFrontRegardless();
            }
            (None, None) => {
                if !ns.isVisible() {
                    ns.setHasShadow(false);
                    ns.orderFrontRegardless();
                }
            }
        }
    }
}

/// Take a window off the one it rides on and out of sight, before it's
/// closed. Only outside any GPUI update: AppKit calls back into GPUI.
pub fn retire(window: &Native) {
    if let Some(parent) = window.0.parentWindow() {
        parent.removeChildWindow(&window.0);
    }
    window.0.orderOut(None);
}

/// Where the mouse cursor is on screen, in AppKit's coordinates.
pub fn cursor() -> (f64, f64) {
    let p = NSEvent::mouseLocation();
    (p.x, p.y)
}

/// Whether the key with this virtual key code is held down right now, in any
/// app. A question about the keyboard's state, not a tap on its events: it
/// needs no permission.
pub fn key_down(code: u16) -> bool {
    CGEventSource::key_state(CGEventSourceStateID::HIDSystemState, code)
}

/// One soft system sound: you're talking while muted.
pub fn soft_beep() {
    if let Some(sound) = NSSound::soundNamed(&NSString::from_str("Tink")) {
        sound.setVolume(0.35);
        sound.play();
    }
}

struct MenuIvars {
    picked: async_channel::Sender<usize>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the target is only
    // used on the main thread and implements no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "QuipcallMenuTarget"]
    #[ivars = MenuIvars]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(pick:))]
        fn pick(&self, sender: &NSMenuItem) {
            let _ = self.ivars().picked.try_send(sender.tag() as usize);
        }
    }
);

thread_local! {
    /// A menu's items don't retain their target: the last one stays here,
    /// with its menu.
    static MENU: RefCell<Option<(Retained<MenuTarget>, Retained<NSMenu>)>> =
        const { RefCell::new(None) };
}

/// Close the menu [`popup_menu`] opened, as Escape does (snapshot tests).
#[cfg(feature = "snapshot")]
pub fn cancel_menu() {
    MENU.with(|m| {
        if let Some((_, menu)) = &*m.borrow() {
            menu.cancelTracking();
        }
    });
}

/// A menu as macOS draws it, at the cursor. `items`: a label and whether
/// it's ticked, or `None` for a rule. The index picked comes through
/// `picked`.
///
/// The menu runs its own event loop until it closes, so it opens from the
/// run loop itself: inside a GPUI event its events would come back into
/// GPUI, and inside a task GPUI's other tasks would wait for it.
pub fn popup_menu(items: Vec<Option<(String, bool)>>, picked: async_channel::Sender<usize>) {
    let block = RcBlock::new(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let target = MenuTarget::alloc(mtm).set_ivars(MenuIvars {
            picked: picked.clone(),
        });
        // SAFETY: NSObject's plain init.
        let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        for (ix, item) in items.iter().enumerate() {
            let Some((label, ticked)) = item else {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
                continue;
            };
            let entry = NSMenuItem::new(mtm);
            entry.setTitle(&NSString::from_str(label));
            entry.setTag(ix as isize);
            entry.setState(isize::from(*ticked));
            // SAFETY: the target outlives the menu (MENU) and implements
            // `pick:` with this signature.
            unsafe {
                let object: &AnyObject = &target;
                entry.setTarget(Some(object));
                entry.setAction(Some(sel!(pick:)));
            }
            menu.addItem(&entry);
        }
        MENU.with(|m| *m.borrow_mut() = Some((target, menu.clone())));
        tracing::debug!("native menu: open");
        let picked =
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSEvent::mouseLocation(), None);
        tracing::debug!(picked, "native menu: closed");
    });
    // SAFETY: `performBlock:` copies the block and runs it once, on the main
    // thread's next turn.
    unsafe {
        let _: () = msg_send![&*NSRunLoop::mainRunLoop(), performBlock: &*block];
    }
}

/// A borderless panel above all apps and Spaces that never takes focus.
pub fn floating_options(bounds: WindowBounds) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(bounds),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: true,
        app_owns_titlebar_drag: true,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// Like [`floating_options`], but out of sight until it's placed.
pub fn hidden_floating_options(bounds: WindowBounds) -> WindowOptions {
    WindowOptions {
        show: false,
        ..floating_options(bounds)
    }
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI hands out its live NSView; we're on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

fn apply_mask(window: &NSWindow, mask: &Mask) {
    let Some(content) = window.contentView() else {
        return;
    };
    content.setWantsLayer(true);
    let Some(layer) = content.layer() else {
        return;
    };
    // No implicit animation: the shape must follow the window instantly.
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let Mask::Rounded(radius) = mask;
    // SAFETY: clearing any mask; we're on the main thread.
    unsafe { layer.setMask(None) };
    layer.setCornerRadius(*radius);
    layer.setMasksToBounds(true);
    CATransaction::commit();
}

/// Drop the shadow and clip the content to `mask`. Screenshots and screen
/// recordings show floating windows; our own screen shares leave them out
/// through their ScreenCaptureKit filter (`crates/core/src/screen_mac.rs`).
///
/// The window must already be borderless: GPUI creates title-less popups that
/// way (patched, see vendor/README.md). Switching the style mask here instead
/// crashed Macs with a Touch Bar.
///
/// Deferred like [`resize`]: GPUI is still building the window when we're called.
pub fn make_shape(window: &Window, mask: Mask, cx: &App) {
    let Some(ns) = ns_window(window) else {
        return;
    };
    debug_assert!(
        !ns.styleMask().contains(NSWindowStyleMask::Titled),
        "floating windows must be created borderless"
    );
    tracing::debug!("floating window {}: {mask:?}", ns.windowNumber());
    cx.foreground_executor()
        .spawn(async move {
            ns.setHasShadow(false);
            apply_mask(&ns, &mask);
        })
        .detach();
}

/// Whether the mouse cursor is over `window` right now (`None` if the
/// window can't be found).
pub fn cursor_inside(window: &Window) -> Option<bool> {
    let ns = ns_window(window)?;
    let p = NSEvent::mouseLocation();
    let f = ns.frame();
    Some(
        p.x >= f.origin.x
            && p.x <= f.origin.x + f.size.width
            && p.y >= f.origin.y
            && p.y <= f.origin.y + f.size.height,
    )
}

/// Whether a mouse button is held down right now, anywhere on screen.
pub fn mouse_down() -> bool {
    NSEvent::pressedMouseButtons() != 0
}

/// Whether any of `window` is on screen now (`None` if the window can't be
/// found). For a fullscreen window: whether its Space is the one showing on
/// its display, the only time macOS will take it out of fullscreen.
pub fn on_screen(window: &Window) -> Option<bool> {
    Some(
        ns_window(window)?
            .occlusionState()
            .contains(NSWindowOcclusionState::Visible),
    )
}

/// Move `window` to the top of its screen's visible area (snapshot tests).
#[cfg(feature = "snapshot")]
pub fn move_to_top(window: &Window, cx: &App) {
    let Some(ns) = ns_window(window) else {
        return;
    };
    cx.foreground_executor()
        .spawn(async move {
            if let Some(visible) = ns.screen().map(|s| s.visibleFrame()) {
                let frame = ns.frame();
                let top = visible.origin.y + visible.size.height - frame.size.height;
                ns.setFrameOrigin(NSPoint::new(frame.origin.x, top));
            }
        })
        .detach();
}

/// Resize to `size`, keeping `anchor` where it is, and re-apply `mask`. A
/// window that was fully on its screen stays on it, except while a resize
/// handle is dragged (moving the corner under the cursor would feed back into
/// the drag), and except up and down for [`Anchor::Point`]: that point never
/// moves, and its window adds what it shows on the side with room.
///
/// Deferred to the next run-loop turn: AppKit calls straight back into GPUI
/// when a window's frame changes, which must not happen mid-event.
pub fn resize(window: &Window, size: Size<f32>, mask: Mask, anchor: Anchor, cx: &App) {
    let Some(ns) = ns_window(window) else {
        return;
    };
    cx.foreground_executor()
        .spawn(async move { set_frame(&ns, size, &mask, anchor) })
        .detach();
}

/// Like [`resize`], but first runs `before` on the view, in the same turn as
/// the frame change, so whatever the view draws next matches the new frame.
/// Several resizes may be on their way at once; each brings its own state.
/// `after` says where the windows riding on this one go now, in that same
/// turn too.
pub fn resize_with<V: 'static>(
    window: &Window,
    size: Size<f32>,
    mask: Mask,
    anchor: Anchor,
    cx: &mut Context<V>,
    before: impl FnOnce(&mut V, &mut Context<V>) + 'static,
    after: impl FnOnce(&mut V, &mut Context<V>) -> Vec<Placement> + 'static,
) {
    let Some(ns) = ns_window(window) else {
        return;
    };
    cx.spawn(async move |view, cx| {
        if view.update(cx, |v, cx| before(v, cx)).is_ok() {
            set_frame(&ns, size, &mask, anchor);
            if let Ok(placements) = view.update(cx, |v, cx| after(v, cx)) {
                place_now(placements);
            }
        }
    })
    .detach();
}

/// Move and resize `window` so the point `at` of its content (from its
/// top-left corner) lands on `screen` (AppKit's coordinates), then run
/// `after` as [`resize_with`] does. The call widget turning into a column.
pub fn move_with<V: 'static>(
    window: &Window,
    size: Size<f32>,
    at: (f32, f32),
    screen: (f64, f64),
    cx: &mut Context<V>,
    before: impl FnOnce(&mut V, &mut Context<V>) + 'static,
    after: impl FnOnce(&mut V, &mut Context<V>) -> Vec<Placement> + 'static,
) {
    let Some(ns) = ns_window(window) else {
        return;
    };
    cx.spawn(async move |view, cx| {
        if view.update(cx, |v, cx| before(v, cx)).is_ok() {
            let (w, h) = (size.width as f64, size.height as f64);
            let target = NSRect::new(
                NSPoint::new(screen.0 - at.0 as f64, screen.1 + at.1 as f64 - h),
                NSSize::new(w, h),
            );
            ns.setFrame_display(target, true);
            if let Ok(placements) = view.update(cx, |v, cx| after(v, cx)) {
                place_now(placements);
            }
        }
    })
    .detach();
}

fn set_frame(ns: &NSWindow, size: Size<f32>, mask: &Mask, anchor: Anchor) {
    let frame = ns.frame();
    let (w, h) = (size.width as f64, size.height as f64);
    // AppKit's y axis points up: origin is the bottom-left corner.
    let (x, y) = match anchor {
        Anchor::Center => (
            frame.origin.x + (frame.size.width - w) / 2.0,
            frame.origin.y + (frame.size.height - h) / 2.0,
        ),
        Anchor::TopLeft => (frame.origin.x, frame.origin.y + frame.size.height - h),
        Anchor::Point { before, after } => (
            frame.origin.x + before.0 as f64 - after.0 as f64,
            frame.origin.y + frame.size.height - before.1 as f64 + after.1 as f64 - h,
        ),
    };
    let mut target = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    if !matches!(anchor, Anchor::TopLeft)
        && let Some(visible) = ns.screen().map(|s| s.visibleFrame())
        && contains(visible, frame)
    {
        target.origin.x = target.origin.x.clamp(
            visible.origin.x,
            (visible.origin.x + visible.size.width - w).max(visible.origin.x),
        );
        if matches!(anchor, Anchor::Center) {
            target.origin.y = target.origin.y.clamp(
                visible.origin.y,
                (visible.origin.y + visible.size.height - h).max(visible.origin.y),
            );
        }
    }
    ns.setFrame_display(target, true);
    apply_mask(ns, mask);
}

fn contains(outer: NSRect, inner: NSRect) -> bool {
    inner.origin.x >= outer.origin.x
        && inner.origin.y >= outer.origin.y
        && inner.origin.x + inner.size.width <= outer.origin.x + outer.size.width
        && inner.origin.y + inner.size.height <= outer.origin.y + outer.size.height
}

/// Log what macOS does to our app and windows: deactivated, hidden, covered,
/// closed, moved to another display or Space. Some bugs only show on a
/// friend's Mac, and then the log is all we have.
pub fn log_window_events() {
    // SAFETY: AppKit's notification names are immutable statics.
    let events: [(&NSNotificationName, &'static str); 8] = unsafe {
        [
            (
                NSApplicationDidBecomeActiveNotification,
                "app became active",
            ),
            (
                NSApplicationDidResignActiveNotification,
                "app went to the background",
            ),
            (NSApplicationDidHideNotification, "app hidden"),
            (NSApplicationDidUnhideNotification, "app unhidden"),
            (
                NSApplicationDidChangeScreenParametersNotification,
                "displays changed",
            ),
            (
                NSWindowDidChangeOcclusionStateNotification,
                "window visibility changed",
            ),
            (NSWindowWillCloseNotification, "window closing"),
            (
                NSWindowDidChangeScreenNotification,
                "window moved to another display",
            ),
        ]
    };
    let center = NSNotificationCenter::defaultCenter();
    for (name, what) in events {
        observe(&center, name, what);
    }
    // SAFETY: as above.
    let space_changed = unsafe { NSWorkspaceActiveSpaceDidChangeNotification };
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    observe(&workspace, space_changed, "switched Space");
}

fn observe(center: &NSNotificationCenter, name: &NSNotificationName, what: &'static str) {
    let block = RcBlock::new(move |note: NonNull<NSNotification>| {
        // SAFETY: AppKit hands us a live notification.
        let note = unsafe { note.as_ref() };
        match note.object().and_then(|o| o.downcast::<NSWindow>().ok()) {
            Some(window) => {
                let frame = window.frame();
                tracing::debug!(
                    "{what}: window {} visible={} unobscured={} level={} at ({:.0}, {:.0}) {:.0}x{:.0}",
                    window.windowNumber(),
                    window.isVisible(),
                    window
                        .occlusionState()
                        .contains(NSWindowOcclusionState::Visible),
                    window.level(),
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height,
                );
            }
            None => tracing::info!("{what}"),
        }
    });
    // SAFETY: the block only logs. With no queue it runs on the posting
    // thread, the main thread for these. The center keeps the registration
    // alive; the returned token is only needed to unregister.
    let _ = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
    };
}

#[cfg(test)]
mod tests {
    use super::Rect;

    #[test]
    fn how_far_apart_two_rectangles_are() {
        let pill = Rect {
            x: 100.0,
            y: 100.0,
            w: 300.0,
            h: 52.0,
        };
        let at = |x: f64, y: f64| Rect {
            x,
            y,
            w: 80.0,
            h: 80.0,
        };
        // Over it, a gap of 8; touching; overlapping.
        assert_eq!(pill.distance(&at(200.0, 160.0)), 8.0);
        assert_eq!(pill.distance(&at(200.0, 152.0)), 0.0);
        assert_eq!(pill.distance(&at(200.0, 120.0)), 0.0);
        // Past a corner: straight-line distance.
        assert_eq!(pill.distance(&at(403.0, 156.0)), 5.0);
        assert_eq!(at(403.0, 156.0).distance(&pill), 5.0);
    }
}
