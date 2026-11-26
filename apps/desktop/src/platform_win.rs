//! Win32 touches GPUI doesn't expose, for the same jobs as `platform_mac.rs`:
//! shaped borderless windows, windows that ride on another, the cursor and
//! the keyboard's state, a native menu.
//!
//! Positions are AppKit's (`geometry.rs`): points, `y` up from the bottom of
//! the main display, so the call widget lays itself out the same on both
//! systems. Win32 has physical pixels with `y` down from the main display's
//! top; each conversion uses the scale of the display concerned, which is
//! exact with one display, or several at the same scale.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Size, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow, SetWindowRgn,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_MBUTTON, VK_MENU, VK_RBUTTON, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CHILDID_SELF, CreatePopupMenu, DestroyMenu, EVENT_OBJECT_LOCATIONCHANGE,
    EnumThreadWindows, GetCursorPos, GetSystemMetrics, GetWindowRect, IsIconic, IsWindowVisible,
    KillTimer, MB_OK, MF_CHECKED, MF_SEPARATOR, MF_STRING, OBJID_WINDOW, PostMessageW, SM_CYSCREEN,
    SM_SWAPBUTTON, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WINEVENT_OUTOFCONTEXT, WM_NULL,
};
use windows::core::{BOOL, PCWSTR};

pub use crate::geometry::{Anchor, Mask, Rect};

/// Hold to talk: the right Ctrl key. Under the hand, it types nothing by
/// itself, and unlike Alt, letting go of it doesn't open the menu of the app
/// in front.
pub const HOLD_KEY: u16 = VK_RCONTROL.0;

/// A window of ours, to move and size from another window's view: the call
/// widget places the bubbles that ride on it.
#[derive(Clone, PartialEq)]
pub struct Native(HWND);

impl Native {
    pub fn of(window: &Window) -> Option<Self> {
        hwnd(window).map(Self)
    }

    pub fn frame(&self) -> Rect {
        window_rect(self.0).map_or_else(Rect::default, |r| to_points(r, window_scale(self.0)))
    }

    /// The usable part of the screen the window is on: without the taskbar.
    pub fn visible(&self) -> Option<Rect> {
        let (work, _, scale) = monitor_of(self.0)?;
        Some(to_points(work, scale))
    }

    /// All of the screen the window is on.
    pub fn screen(&self) -> Option<Rect> {
        let (_, whole, scale) = monitor_of(self.0)?;
        Some(to_points(whole, scale))
    }

    pub fn shown(&self) -> bool {
        // SAFETY: a plain query; a stale handle answers false.
        unsafe { IsWindowVisible(self.0) }.as_bool()
    }

    /// Whether the window rides on `parent`, moving when it moves.
    pub fn rides_on(&self, parent: &Native) -> bool {
        RIDERS.with(|r| {
            r.borrow()
                .iter()
                .any(|rider| rider.child == self.0 && rider.parent == parent.0)
        })
    }
}

/// Where a window goes: its frame and corner radius, and whether it rides
/// on another window (moving with it) or floats by itself.
pub struct Placement {
    pub window: Native,
    pub frame: Rect,
    /// `None` to keep the shape it has.
    pub radius: Option<f64>,
    pub parent: Option<Native>,
    /// Out of sight for now (its widget is tucked away).
    pub hidden: bool,
}

/// A window that moves with another. macOS does this for child windows;
/// Win32 only keeps owned windows above their owner, so we follow the
/// parent's moves ourselves ([`follow`]).
struct Rider {
    child: HWND,
    parent: HWND,
    /// From the parent's top-left corner to the child's, in pixels.
    offset: (i32, i32),
}

thread_local! {
    static RIDERS: RefCell<Vec<Rider>> = const { RefCell::new(Vec::new()) };
    static HOOK: Cell<Option<HWINEVENTHOOK>> = const { Cell::new(None) };
}

/// Put windows where they go, on the next turn of the event loop (see
/// [`resize`]).
pub fn place(placements: Vec<Placement>, cx: &App) {
    if placements.is_empty() {
        return;
    }
    cx.foreground_executor()
        .spawn(async move { place_now(placements) })
        .detach();
}

/// Only outside any GPUI update: Win32 calls straight back into GPUI's
/// window procedure.
fn place_now(placements: Vec<Placement>) {
    for p in placements {
        let hwnd = p.window.0;
        tracing::debug!(
            "place window {:?}: {:.0}x{:.0} at ({:.0}, {:.0}), radius {:.0}, {}",
            hwnd.0,
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
            unride(hwnd);
            // SAFETY: our own window, on its own thread.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            continue;
        }
        let scale = scale_at(p.frame.x + p.frame.w / 2.0, p.frame.y + p.frame.h / 2.0);
        let target = to_pixels(p.frame, scale);
        if window_rect(hwnd) != Some(target) {
            set_rect(hwnd, target);
        }
        if let Some(radius) = p.radius {
            apply_mask(hwnd, &Mask::Rounded(radius));
        }
        match &p.parent {
            Some(parent) => ride(hwnd, parent.0),
            None => unride(hwnd),
        }
        // SAFETY: as above.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }
}

/// Take a window off the one it rides on and out of sight, before it's
/// closed.
pub fn retire(window: &Native) {
    unride(window.0);
    // SAFETY: our own window, on its own thread.
    let _ = unsafe { ShowWindow(window.0, SW_HIDE) };
}

fn ride(child: HWND, parent: HWND) {
    let (Some(c), Some(p)) = (window_rect(child), window_rect(parent)) else {
        return;
    };
    let offset = (c.left - p.left, c.top - p.top);
    RIDERS.with(|r| {
        let mut riders = r.borrow_mut();
        match riders.iter_mut().find(|rider| rider.child == child) {
            Some(rider) => {
                rider.parent = parent;
                rider.offset = offset;
            }
            None => riders.push(Rider {
                child,
                parent,
                offset,
            }),
        }
    });
    watch_moves();
}

fn unride(child: HWND) {
    RIDERS.with(|r| {
        r.borrow_mut()
            .retain(|rider| rider.child != child && rider.parent != child)
    });
}

/// Follow every move of a window that others ride on. The hook's events
/// arrive through this thread's message queue, after whatever moved the
/// window has returned, so a rider [`place_now`] has just put in its new
/// place is already where it belongs and stays put.
fn watch_moves() {
    if HOOK.get().is_some() {
        return;
    }
    // SAFETY: an out-of-context hook for our own process; `follow` runs on
    // this thread, from its message loop.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(follow),
            std::process::id(),
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook.is_invalid() {
        tracing::warn!("can't follow window moves: bubbles won't move with the widget");
        return;
    }
    HOOK.set(Some(hook));
}

unsafe extern "system" fn follow(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 {
        return;
    }
    let moves: Vec<(HWND, i32, i32)> = RIDERS.with(|r| {
        let riders = r.borrow();
        if !riders.iter().any(|rider| rider.parent == hwnd) {
            return Vec::new();
        }
        let Some(parent) = window_rect(hwnd) else {
            return Vec::new();
        };
        riders
            .iter()
            .filter(|rider| rider.parent == hwnd)
            .filter_map(|rider| {
                let (x, y) = (parent.left + rider.offset.0, parent.top + rider.offset.1);
                let now = window_rect(rider.child)?;
                (now.left != x || now.top != y).then_some((rider.child, x, y))
            })
            .collect()
    });
    for (child, x, y) in moves {
        // SAFETY: our own windows, on their thread.
        let _ = unsafe {
            SetWindowPos(
                child,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }
}

/// Where the mouse cursor is on screen.
pub fn cursor() -> (f64, f64) {
    let p = cursor_px();
    let scale = monitor_scale(
        // SAFETY: a plain query.
        unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) },
    );
    (p.x as f64 / scale, (primary_height() - p.y as f64) / scale)
}

/// Whether the key with this virtual key code is held down right now, in any
/// app. A question about the keyboard's state, not a hook into its events.
pub fn key_down(code: u16) -> bool {
    pressed(code)
}

thread_local! {
    static LAST_PRESS: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// How long ago an ordinary key or a mouse button was last seen down, in any
/// app (modifiers and the hold key don't count). Windows keeps no time per
/// kind of input (`GetLastInputInfo` counts the held key's own auto-repeat),
/// so each call looks at every key: often enough while it matters, since
/// hold to talk asks every 50 ms.
pub fn since_press() -> Duration {
    const MODIFIERS: [u16; 14] = [
        VK_SHIFT.0,
        VK_CONTROL.0,
        VK_MENU.0,
        VK_LSHIFT.0,
        VK_RSHIFT.0,
        VK_LCONTROL.0,
        VK_RCONTROL.0,
        VK_LMENU.0,
        VK_RMENU.0,
        VK_LWIN.0,
        VK_RWIN.0,
        0x14, // Caps Lock
        0x90, // Num Lock
        0x91, // Scroll Lock
    ];
    let now = Instant::now();
    let down = (1u16..0xFF)
        .filter(|vk| !MODIFIERS.contains(vk) && *vk != HOLD_KEY)
        .any(pressed);
    if down {
        LAST_PRESS.set(Some(now));
    }
    LAST_PRESS.get().map_or(Duration::MAX, |t| now - t)
}

fn pressed(vk: u16) -> bool {
    // SAFETY: a plain query; the high bit says the key is down now.
    (unsafe { GetAsyncKeyState(vk as i32) }) < 0
}

/// The screen `p` is on: its usable part, and all of it. `None` between
/// screens.
pub fn screen_at(p: (f64, f64)) -> Option<(Rect, Rect)> {
    monitors().into_iter().find_map(|(work, whole, scale)| {
        let whole = to_points(whole, scale);
        let inside = p.0 >= whole.x && p.0 <= whole.right() && p.1 >= whole.y && p.1 <= whole.top();
        inside.then(|| (to_points(work, scale), whole))
    })
}

/// One soft system sound: you're talking while muted.
pub fn soft_beep() {
    // SAFETY: plays the "Default Beep" sound asynchronously.
    let _ = unsafe { MessageBeep(MB_OK) };
}

/// A menu's items (a label and whether it's ticked, or a rule) and where
/// the index picked goes.
type Menu = (Vec<Option<(String, bool)>>, async_channel::Sender<usize>);

thread_local! {
    /// The menu waiting for the event loop's next turn ([`popup_menu`]).
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
}

/// Close the menu [`popup_menu`] opened, as Escape does (snapshot tests).
#[cfg(feature = "snapshot")]
pub fn cancel_menu() {
    // SAFETY: ends this thread's active menu, if any.
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::EndMenu() };
}

/// A menu as Windows draws it, at the cursor. `items`: a label and whether
/// it's ticked, or `None` for a rule. The index picked comes through
/// `picked`.
///
/// The menu runs its own message loop until it closes, so it opens from the
/// event loop itself (a thread timer's callback): inside a GPUI event its
/// messages would come back into GPUI mid-update.
pub fn popup_menu(items: Vec<Option<(String, bool)>>, picked: async_channel::Sender<usize>) {
    MENU.with(|m| *m.borrow_mut() = Some((items, picked)));
    // SAFETY: a one-shot thread timer; `open_menu` kills it.
    unsafe { SetTimer(None, 0, 0, Some(open_menu)) };
}

unsafe extern "system" fn open_menu(_: HWND, _: u32, id: usize, _: u32) {
    // SAFETY: the thread timer `popup_menu` set.
    let _ = unsafe { KillTimer(None, id) };
    let Some((items, picked)) = MENU.with(|m| m.borrow_mut().take()) else {
        return;
    };
    let Some(owner) = any_window() else {
        return;
    };
    // SAFETY: a menu we create, track and destroy here; `owner` is one of our
    // windows on this thread, as TrackPopupMenuEx requires. Labels outlive
    // the AppendMenuW calls, which copy them.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        for (ix, item) in items.iter().enumerate() {
            match item {
                None => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                Some((label, ticked)) => {
                    let wide: Vec<u16> = label.encode_utf16().chain([0]).collect();
                    let flags = if *ticked {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    // Ids start at 1: 0 means nothing was picked.
                    let _ = AppendMenuW(menu, flags, ix + 1, PCWSTR(wide.as_ptr()));
                }
            }
        }
        let at = cursor_px();
        // The menu closes on a click elsewhere only if its owner is in front.
        let _ = SetForegroundWindow(owner);
        tracing::debug!("native menu: open");
        let id = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            at.x,
            at.y,
            owner,
            None,
        )
        .0;
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        tracing::debug!(picked = id, "native menu: closed");
        if id > 0 {
            let _ = picked.try_send(id as usize - 1);
        }
    }
}

/// One of our windows that's showing, to own a menu.
fn any_window() -> Option<HWND> {
    unsafe extern "system" fn first(hwnd: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` points at the Option below for this call's length.
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                *(found.0 as *mut Option<HWND>) = Some(hwnd);
                return false.into();
            }
        }
        true.into()
    }
    let mut found: Option<HWND> = None;
    // SAFETY: enumerates this thread's windows synchronously.
    unsafe {
        let _ = EnumThreadWindows(
            windows::Win32::System::Threading::GetCurrentThreadId(),
            Some(first),
            LPARAM(&mut found as *mut _ as isize),
        );
    }
    found
}

/// A borderless panel above other apps that doesn't take focus when it
/// opens.
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

fn hwnd(window: &Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(HWND(handle.hwnd.get() as *mut _))
}

/// Cut the window to a rounded rectangle of its current size. What GPUI
/// draws already has rounded corners; the region makes the corners let
/// clicks through and clips anything square under them, as AppKit's layer
/// mask does.
fn apply_mask(hwnd: HWND, mask: &Mask) {
    let Some(r) = window_rect(hwnd) else {
        return;
    };
    let Mask::Rounded(radius) = mask;
    let d = (2.0 * radius * window_scale(hwnd)).round() as i32;
    // SAFETY: the system owns the region once it's set.
    unsafe {
        let region = CreateRoundRectRgn(0, 0, r.right - r.left + 1, r.bottom - r.top + 1, d, d);
        SetWindowRgn(hwnd, Some(region), true);
    }
}

/// Clip the window to `mask`. Win32 windows are created without shadows
/// for popups, so there's nothing else to take away.
///
/// Deferred like [`resize`]: GPUI is still building the window when we're
/// called.
pub fn make_shape(window: &Window, mask: Mask, cx: &App) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    tracing::debug!("floating window {:?}: {mask:?}", hwnd.0);
    cx.foreground_executor()
        .spawn(async move { apply_mask(hwnd, &mask) })
        .detach();
}

/// Whether the mouse cursor is over `window` right now (`None` if the
/// window can't be found).
pub fn cursor_inside(window: &Window) -> Option<bool> {
    let r = window_rect(hwnd(window)?)?;
    let p = cursor_px();
    Some(p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom)
}

/// Whether a mouse button is held down right now, anywhere on screen.
pub fn mouse_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON]
        .into_iter()
        .any(|vk| pressed(vk.0))
}

/// Whether the (primary) left mouse button is held down right now.
pub fn left_mouse_down() -> bool {
    // SAFETY: a plain query. The key state is of the physical buttons.
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    pressed(if swapped { VK_RBUTTON.0 } else { VK_LBUTTON.0 })
}

/// Whether `window` is showing (`None` if the window can't be found).
pub fn on_screen(window: &Window) -> Option<bool> {
    let hwnd = hwnd(window)?;
    // SAFETY: plain queries.
    Some(unsafe { IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() })
}

/// Move `window` to the top of its screen's usable area (snapshot tests).
#[cfg(feature = "snapshot")]
pub fn move_to_top(window: &Window, cx: &App) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    cx.foreground_executor()
        .spawn(async move {
            let (Some((work, _, _)), Some(frame)) = (monitor_of(hwnd), window_rect(hwnd)) else {
                return;
            };
            let h = frame.bottom - frame.top;
            set_rect(
                hwnd,
                RECT {
                    top: work.top,
                    bottom: work.top + h,
                    ..frame
                },
            );
        })
        .detach();
}

/// Resize to `size`, keeping `anchor` where it is, and re-apply `mask`. A
/// window that was fully on its screen stays on it, as on the Mac (see
/// `platform_mac::resize`).
///
/// Deferred to the event loop's next turn: Win32 calls straight back into
/// GPUI when a window's frame changes, which must not happen mid-event.
pub fn resize(window: &Window, size: Size<f32>, mask: Mask, anchor: Anchor, cx: &App) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    cx.foreground_executor()
        .spawn(async move { set_frame(hwnd, size, &mask, anchor) })
        .detach();
}

/// Like [`resize`], but first runs `before` on the view, in the same turn as
/// the frame change, so whatever the view draws next matches the new frame.
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
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    cx.spawn(async move |view, cx| {
        if view.update(cx, |v, cx| before(v, cx)).is_ok() {
            set_frame(hwnd, size, &mask, anchor);
            if let Ok(placements) = view.update(cx, |v, cx| after(v, cx)) {
                place_now(placements);
            }
        }
    })
    .detach();
}

/// Move and resize `window` so the point `at` of its content (from its
/// top-left corner) lands on `screen`, then run `after` as [`resize_with`]
/// does. The call widget turning into a column.
pub fn move_with<V: 'static>(
    window: &Window,
    size: Size<f32>,
    at: (f32, f32),
    screen: (f64, f64),
    cx: &mut Context<V>,
    before: impl FnOnce(&mut V, &mut Context<V>) + 'static,
    after: impl FnOnce(&mut V, &mut Context<V>) -> Vec<Placement> + 'static,
) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    cx.spawn(async move |view, cx| {
        if view.update(cx, |v, cx| before(v, cx)).is_ok() {
            let (w, h) = (size.width as f64, size.height as f64);
            let target = Rect {
                x: screen.0 - at.0 as f64,
                y: screen.1 + at.1 as f64 - h,
                w,
                h,
            };
            set_rect(hwnd, to_pixels(target, scale_at(screen.0, screen.1)));
            if let Ok(placements) = view.update(cx, |v, cx| after(v, cx)) {
                place_now(placements);
            }
        }
    })
    .detach();
}

fn set_frame(hwnd: HWND, size: Size<f32>, mask: &Mask, anchor: Anchor) {
    let Some(current) = window_rect(hwnd) else {
        return;
    };
    let scale = window_scale(hwnd);
    let frame = to_points(current, scale);
    let (w, h) = (size.width as f64, size.height as f64);
    let (x, y) = match anchor {
        Anchor::Center => (frame.x + (frame.w - w) / 2.0, frame.y + (frame.h - h) / 2.0),
        Anchor::TopLeft => (frame.x, frame.y + frame.h - h),
        Anchor::Point { before, after } => (
            frame.x + before.0 as f64 - after.0 as f64,
            frame.y + frame.h - before.1 as f64 + after.1 as f64 - h,
        ),
    };
    let mut target = Rect { x, y, w, h };
    if !matches!(anchor, Anchor::TopLeft)
        && let Some((work, _, _)) = monitor_of(hwnd)
        && contains(&to_points(work, scale), &frame)
    {
        let visible = to_points(work, scale);
        target.x = target
            .x
            .clamp(visible.x, (visible.right() - w).max(visible.x));
        if matches!(anchor, Anchor::Center) {
            target.y = target
                .y
                .clamp(visible.y, (visible.top() - h).max(visible.y));
        }
    }
    set_rect(hwnd, to_pixels(target, scale));
    apply_mask(hwnd, mask);
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.top() <= outer.top()
}

/// Nothing to log yet: macOS's app and Space notifications have no
/// counterpart here that our bugs have needed.
pub fn log_window_events() {}

fn set_rect(hwnd: HWND, r: RECT) {
    // SAFETY: our own window, on its own thread.
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: writes into `r`; fails for a window that's gone.
    unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    Some(r)
}

fn cursor_px() -> POINT {
    let mut p = POINT::default();
    // SAFETY: writes into `p`.
    let _ = unsafe { GetCursorPos(&mut p) };
    p
}

/// The main display's height in pixels: AppKit's `y` counts up from its
/// bottom edge.
fn primary_height() -> f64 {
    // SAFETY: a plain query (in physical pixels: the process is DPI-aware).
    unsafe { GetSystemMetrics(SM_CYSCREEN) as f64 }
}

fn window_scale(hwnd: HWND) -> f64 {
    // SAFETY: a plain query; 0 for a window that's gone.
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => 1.0,
        dpi => dpi as f64 / 96.0,
    }
}

fn monitor_scale(monitor: HMONITOR) -> f64 {
    let (mut x, mut y) = (0, 0);
    // SAFETY: writes into `x` and `y`.
    match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
        Ok(()) if x > 0 => x as f64 / 96.0,
        _ => 1.0,
    }
}

/// The scale of the display a point (in points) is on, or the main one's.
fn scale_at(x: f64, y: f64) -> f64 {
    monitors()
        .into_iter()
        .find(|(_, whole, scale)| {
            let r = to_points(*whole, *scale);
            x >= r.x && x < r.right() && y >= r.y && y < r.top()
        })
        .map_or_else(
            // SAFETY: a plain query.
            || {
                monitor_scale(unsafe {
                    MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY)
                })
            },
            |(_, _, scale)| scale,
        )
}

/// The work area and whole of a display, in pixels.
fn monitor_info(monitor: HMONITOR) -> Option<(RECT, RECT)> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `cbSize` is set; writes into `info`.
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then_some((info.rcWork, info.rcMonitor))
}

/// The display a window is on: work area, whole, scale.
fn monitor_of(hwnd: HWND) -> Option<(RECT, RECT, f64)> {
    // SAFETY: a plain query.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let (work, whole) = monitor_info(monitor)?;
    Some((work, whole, monitor_scale(monitor)))
}

/// Every display: work area, whole, scale.
fn monitors() -> Vec<(RECT, RECT, f64)> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
        // SAFETY: `out` points at the Vec below for this call's length.
        let out = unsafe { &mut *(out.0 as *mut Vec<(RECT, RECT, f64)>) };
        if let Some((work, whole)) = monitor_info(monitor) {
            out.push((work, whole, monitor_scale(monitor)));
        }
        true.into()
    }
    let mut out: Vec<(RECT, RECT, f64)> = Vec::new();
    // SAFETY: enumerates synchronously into `out`.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

fn to_points(r: RECT, scale: f64) -> Rect {
    Rect {
        x: r.left as f64 / scale,
        y: (primary_height() - r.bottom as f64) / scale,
        w: (r.right - r.left) as f64 / scale,
        h: (r.bottom - r.top) as f64 / scale,
    }
}

fn to_pixels(r: Rect, scale: f64) -> RECT {
    let left = (r.x * scale).round() as i32;
    let bottom = (primary_height() - r.y * scale).round() as i32;
    RECT {
        left,
        top: bottom - (r.h * scale).round() as i32,
        right: left + (r.w * scale).round() as i32,
        bottom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_and_pixels_round_trip() {
        let r = Rect {
            x: 100.0,
            y: 200.0,
            w: 300.0,
            h: 52.0,
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let back = to_points(to_pixels(r, scale), scale);
            assert!((back.x - r.x).abs() < 1.0 && (back.y - r.y).abs() < 1.0);
            assert!((back.w - r.w).abs() < 1.0 && (back.h - r.h).abs() < 1.0);
        }
    }
}
