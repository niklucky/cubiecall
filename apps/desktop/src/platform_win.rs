//! Win32 touches GPUI doesn't expose, for the same jobs as `platform_mac.rs`:
//! shaped borderless windows, windows that ride on another, the cursor and
//! the keyboard's state, a native menu.
//!
//! Positions are AppKit's (`geometry.rs`): points, `y` up from the bottom of
//! the main display, so the call widget lays itself out the same on both
//! systems. Win32 has physical pixels with `y` down from the main display's
//! top, and each display its own scale ([`Desktop`] converts).

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Size, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFO, SetWindowRgn,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, VK_CONTROL, VK_LBUTTON, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
    VK_LWIN, VK_MBUTTON, VK_MENU, VK_RBUTTON, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CHILDID_SELF, CreatePopupMenu, DestroyMenu, EVENT_OBJECT_LOCATIONCHANGE,
    EnumThreadWindows, GetCursorPos, GetSystemMetrics, GetWindowRect, HTCAPTION, IsIconic,
    IsWindowVisible, KillTimer, MB_OK, MF_CHECKED, MF_SEPARATOR, MF_STRING, OBJID_WINDOW,
    PostMessageW, SC_MOVE, SM_SWAPBUTTON, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WINEVENT_OUTOFCONTEXT, WM_NULL,
    WM_SYSCOMMAND,
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
        window_rect(self.0).map_or_else(Rect::default, |r| Desktop::now().to_points(r))
    }

    /// The usable part of the screen the window is on: without the taskbar.
    pub fn visible(&self) -> Option<Rect> {
        let desktop = Desktop::now();
        let display = desktop.display_of(window_rect(self.0)?)?;
        Some(desktop.work(display))
    }

    /// All of the screen the window is on.
    pub fn screen(&self) -> Option<Rect> {
        let desktop = Desktop::now();
        let display = desktop.display_of(window_rect(self.0)?)?;
        Some(desktop.whole(display))
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
        let target = Desktop::now().to_pixels(p.frame);
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

thread_local! {
    /// The window [`start_window_move`] moves on the event loop's next turn.
    static MOVING: Cell<Option<HWND>> = const { Cell::new(None) };
}

/// Let the user drag `window` with the button that's down, as dragging a
/// title bar does: GPUI's Windows backend leaves this out. Windows' own move
/// loop takes over until the button goes up. It runs a message loop of its
/// own, so it starts from the event loop's next turn (a thread timer), not
/// inside the GPUI event that asked for it.
pub fn start_window_move(window: &Window) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    MOVING.set(Some(hwnd));
    // SAFETY: a one-shot thread timer; `drag` kills it.
    unsafe { SetTimer(None, 0, 0, Some(drag)) };
}

unsafe extern "system" fn drag(_: HWND, _: u32, id: usize, _: u32) {
    // SAFETY: the thread timer `start_window_move` set.
    let _ = unsafe { KillTimer(None, id) };
    let Some(hwnd) = MOVING.take() else {
        return;
    };
    // Let go already: it was a click.
    if !left_mouse_down() {
        return;
    }
    // SAFETY: our own window, on its thread, outside any GPUI update. GPUI
    // took the mouse when the button went down; the move loop needs it.
    // SC_MOVE | HTCAPTION moves with the mouse from where it is now.
    unsafe {
        let _ = ReleaseCapture();
        SendMessageW(
            hwnd,
            WM_SYSCOMMAND,
            Some(WPARAM((SC_MOVE | HTCAPTION) as usize)),
            Some(LPARAM(0)),
        );
    }
}

/// Where the mouse cursor is on screen.
pub fn cursor() -> (f64, f64) {
    Desktop::now().point(cursor_px())
}

/// Whether the key with this virtual key code is held down right now, in any
/// app. A question about the keyboard's state, not a hook into its events.
pub fn key_down(code: u16) -> bool {
    pressed(code)
}

thread_local! {
    static LAST_PRESS: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// How long ago an ordinary key or a mouse button was last seen going down,
/// in any app (modifiers and the hold key don't count). Windows keeps no time
/// per kind of input (`GetLastInputInfo` counts the held key's own
/// auto-repeat), so each call looks at every key: hold to talk asks every
/// 50 ms while it matters. A key tapped and let go between two looks counts
/// too, by the bit that says it went down since the last look. Windows
/// doesn't promise that bit, so it only ever adds presses: at worst a hold
/// is taken for a shortcut and stays muted.
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
    let mut seen = false;
    // Every key, not just up to the first one down: each look resets its
    // "went down since" bit.
    for vk in (1u16..0xFF).filter(|vk| !MODIFIERS.contains(vk) && *vk != HOLD_KEY) {
        // SAFETY: a plain query. High bit: down now; low bit: went down
        // since the last look.
        let state = unsafe { GetAsyncKeyState(vk as i32) };
        seen |= state < 0 || state & 1 != 0;
    }
    if seen {
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
    let desktop = Desktop::now();
    desktop.displays.iter().find_map(|display| {
        let whole = desktop.whole(display);
        let inside = p.0 >= whole.x && p.0 <= whole.right() && p.1 >= whole.y && p.1 <= whole.top();
        inside.then(|| (desktop.work(display), whole))
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
    // SAFETY: the system owns the region once it's set; until then, and if
    // setting it fails, it's ours to delete.
    unsafe {
        let region = CreateRoundRectRgn(0, 0, r.right - r.left + 1, r.bottom - r.top + 1, d, d);
        if region.is_invalid() {
            return;
        }
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(region.into());
        }
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
            let Some(frame) = window_rect(hwnd) else {
                return;
            };
            let Some(work) = Desktop::now().display_of(frame).map(|d| d.work) else {
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
            set_rect(hwnd, Desktop::now().to_pixels(target));
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
    let desktop = Desktop::now();
    let frame = desktop.to_points(current);
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
        && let Some(display) = desktop.display_of(current)
        && contains(&desktop.work(display), &frame)
    {
        let visible = desktop.work(display);
        target.x = target
            .x
            .clamp(visible.x, (visible.right() - w).max(visible.x));
        if matches!(anchor, Anchor::Center) {
            target.y = target
                .y
                .clamp(visible.y, (visible.top() - h).max(visible.y));
        }
    }
    set_rect(hwnd, desktop.to_pixels(target));
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

/// The displays as they are now, and how a position in pixels on one of
/// them reads in points. A display's top-left corner is its pixel position
/// over the main display's scale; inside it, distances are over its own
/// scale, which is what GPUI sizes its windows by. Displays touching the
/// main one meet it without gap or overlap, and every point belongs to one
/// display both ways.
struct Desktop {
    displays: Vec<Display>,
    /// The main display's height in points: AppKit's `y` counts up from its
    /// bottom edge.
    height: f64,
}

#[derive(Clone, Copy, Debug)]
struct Display {
    whole: RECT,
    work: RECT,
    scale: f64,
    /// Its top-left corner in points, `y` down.
    at: (f64, f64),
}

impl Desktop {
    fn now() -> Self {
        Self::new(monitors())
    }

    /// From each display's work area, whole and scale, in pixels.
    fn new(monitors: Vec<(RECT, RECT, f64)>) -> Self {
        let main = monitors
            .iter()
            .find(|(_, whole, _)| whole.left == 0 && whole.top == 0)
            .or(monitors.first())
            .copied();
        let (main_scale, height) = main.map_or((1.0, 0.0), |(_, whole, scale)| {
            (scale, (whole.bottom - whole.top) as f64 / scale)
        });
        let displays = monitors
            .into_iter()
            .map(|(work, whole, scale)| Display {
                whole,
                work,
                scale,
                at: (
                    whole.left as f64 / main_scale,
                    whole.top as f64 / main_scale,
                ),
            })
            .collect();
        Self { displays, height }
    }

    /// The display holding most of `r` (by its centre), else the nearest.
    fn display_of(&self, r: RECT) -> Option<&Display> {
        let (x, y) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        let distance = |d: &&Display| {
            let dx = (d.whole.left - x).max(x - d.whole.right + 1).max(0) as f64;
            let dy = (d.whole.top - y).max(y - d.whole.bottom + 1).max(0) as f64;
            dx.hypot(dy)
        };
        self.displays
            .iter()
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
    }

    /// The display a point in points is on, else the nearest.
    fn display_at(&self, x: f64, y: f64) -> Option<&Display> {
        let distance = |d: &&Display| {
            let r = self.whole(d);
            let dx = (r.x - x).max(x - r.right()).max(0.0);
            let dy = (r.y - y).max(y - r.top()).max(0.0);
            dx.hypot(dy)
        };
        self.displays
            .iter()
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
    }

    fn on(&self, d: &Display, r: RECT) -> Rect {
        let w = (r.right - r.left) as f64 / d.scale;
        let h = (r.bottom - r.top) as f64 / d.scale;
        let x = d.at.0 + (r.left - d.whole.left) as f64 / d.scale;
        let top = d.at.1 + (r.top - d.whole.top) as f64 / d.scale;
        Rect {
            x,
            y: self.height - top - h,
            w,
            h,
        }
    }

    fn whole(&self, d: &Display) -> Rect {
        self.on(d, d.whole)
    }

    fn work(&self, d: &Display) -> Rect {
        self.on(d, d.work)
    }

    fn to_points(&self, r: RECT) -> Rect {
        match self.display_of(r) {
            Some(d) => self.on(d, r),
            None => Rect::default(),
        }
    }

    fn to_pixels(&self, r: Rect) -> RECT {
        let Some(d) = self.display_at(r.x + r.w / 2.0, r.y + r.h / 2.0) else {
            return RECT::default();
        };
        let top = self.height - r.y - r.h;
        let left = d.whole.left + ((r.x - d.at.0) * d.scale).round() as i32;
        let top = d.whole.top + ((top - d.at.1) * d.scale).round() as i32;
        RECT {
            left,
            top,
            right: left + (r.w * d.scale).round() as i32,
            bottom: top + (r.h * d.scale).round() as i32,
        }
    }

    fn point(&self, p: POINT) -> (f64, f64) {
        let r = self.to_points(RECT {
            left: p.x,
            top: p.y,
            right: p.x,
            bottom: p.y,
        });
        (r.x, r.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, w: i32, h: i32) -> RECT {
        RECT {
            left,
            top,
            right: left + w,
            bottom: top + h,
        }
    }

    /// The main display at 100%, a 150% one to its right; then the other
    /// way round.
    fn desktops() -> [Desktop; 2] {
        [
            Desktop::new(vec![
                (rect(0, 0, 1920, 1040), rect(0, 0, 1920, 1080), 1.0),
                (rect(1920, 0, 2880, 1560), rect(1920, 0, 2880, 1620), 1.5),
            ]),
            Desktop::new(vec![
                (rect(0, 0, 2880, 1560), rect(0, 0, 2880, 1620), 1.5),
                (rect(2880, 0, 1920, 1040), rect(2880, 0, 1920, 1080), 1.0),
            ]),
        ]
    }

    #[test]
    fn displays_meet_without_overlapping() {
        for desktop in desktops() {
            let [a, b] = [&desktop.displays[0], &desktop.displays[1]];
            assert_eq!(desktop.whole(a).right(), desktop.whole(b).x);
            // A pixel either side of the seam stays on its own display.
            let seam = b.whole.left;
            let left = desktop.point(POINT { x: seam - 1, y: 10 });
            let right = desktop.point(POINT { x: seam, y: 10 });
            assert!(left.0 < desktop.whole(b).x && right.0 >= desktop.whole(b).x);
        }
    }

    #[test]
    fn windows_round_trip_on_every_display() {
        for desktop in desktops() {
            for d in &desktop.displays {
                let r = rect(d.whole.left + 300, d.whole.top + 200, 450, 78);
                let back = desktop.to_pixels(desktop.to_points(r));
                assert_eq!(back, r);
                // A window's size in points is its size over its display's scale.
                let pts = desktop.to_points(r);
                assert!((pts.w - 450.0 / d.scale).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn y_counts_up_from_the_main_display_bottom() {
        let [desktop, _] = desktops();
        let main = &desktop.displays[0];
        assert_eq!(desktop.whole(main).y, 0.0);
        let p = desktop.point(POINT { x: 10, y: 0 });
        assert_eq!(p.1, 1080.0);
    }
}
