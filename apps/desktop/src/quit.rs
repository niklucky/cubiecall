//! Cmd+Q needs a deliberate hold or a second press, in every app window.

use std::cell::Cell;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use block2::RcBlock;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, FontWeight, Task, Window, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, div, px, size,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplicationDidResignActiveNotification, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType,
};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use crate::{platform, theme};

const HOLD: Duration = Duration::from_millis(700);
const CONFIRM: Duration = Duration::from_secs(2);
const MESSAGE: &str = "Hold ⌘Q or press twice to quit";
const RADIUS: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    Press,
    Release,
    Cancel,
}

fn extra_shortcut_modifiers(flags: NSEventModifierFlags) -> bool {
    flags.intersects(
        NSEventModifierFlags::Control
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Shift
            | NSEventModifierFlags::Function,
    )
}

fn modifier_input(key_code: u16, flags: NSEventModifierFlags) -> Option<Input> {
    // AppKit's device-independent key codes for right and left Command.
    // Caps Lock may already be enabled when the shortcut starts; cancel on
    // its key transition, rather than on its persistent modifier flag.
    if !matches!(key_code, 54 | 55) || extra_shortcut_modifiers(flags) {
        Some(Input::Cancel)
    } else if !flags.contains(NSEventModifierFlags::Command) {
        Some(Input::Release)
    } else {
        None
    }
}

#[derive(Default)]
struct Confirmation {
    first_press: Option<Instant>,
    down: bool,
}

impl Confirmation {
    /// Auto-repeat is filtered at the native event boundary.
    fn press(&mut self, now: Instant) -> bool {
        if self
            .first_press
            .is_some_and(|first| now.duration_since(first) < CONFIRM)
        {
            return true;
        }
        self.first_press = Some(now);
        self.down = true;
        false
    }

    fn held(&self, now: Instant) -> bool {
        self.down
            && self
                .first_press
                .is_some_and(|first| now.duration_since(first) >= HOLD)
    }
}

pub fn start(cx: &mut App) {
    let (sender, events) = async_channel::unbounded();
    let monitor = NativeMonitor::new(sender);
    let guard = cx.new(|_| QuitGuard {
        confirmation: Confirmation::default(),
        overlay: None,
        hold: None,
        dismiss: None,
    });
    // Native callbacks only queue input. Updating GPUI from inside AppKit's
    // event monitor would re-enter its application borrow.
    cx.spawn(async move |cx| {
        let _monitor = monitor;
        while let Ok(input) = events.recv().await {
            guard.update(cx, |guard, cx| guard.input(input, cx));
        }
    })
    .detach();
}

struct QuitGuard {
    confirmation: Confirmation,
    overlay: Option<WindowHandle<QuitOverlay>>,
    hold: Option<Task<()>>,
    dismiss: Option<Task<()>>,
}

impl QuitGuard {
    fn input(&mut self, input: Input, cx: &mut Context<Self>) {
        match input {
            Input::Release => {
                self.confirmation.down = false;
                self.hold = None;
            }
            Input::Cancel => self.cancel(cx),
            Input::Press => {
                if self.confirmation.press(Instant::now()) {
                    cx.quit();
                    return;
                }
                self.show(cx);
                self.hold = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOLD).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.confirmation.held(Instant::now()) {
                            cx.quit();
                        }
                    });
                }));
                self.dismiss = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(CONFIRM).await;
                    let _ = this.update(cx, |this, cx| this.cancel(cx));
                }));
            }
        }
    }

    fn show(&mut self, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            return;
        }
        self.overlay = open_overlay(cx);
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.confirmation = Confirmation::default();
        self.hold = None;
        self.dismiss = None;
        if let Some(overlay) = self.overlay.take() {
            let _ = overlay.update(cx, |_, window, _| window.remove_window());
        }
    }
}

fn open_overlay(cx: &mut App) -> Option<WindowHandle<QuitOverlay>> {
    let bounds = Bounds::centered(None, size(px(452.0), px(72.0)), cx);
    let mut options = platform::floating_options(WindowBounds::Windowed(bounds));
    options.is_movable = false;
    options.window_background = WindowBackgroundAppearance::Blurred;
    cx.open_window(options, |window, cx| {
        platform::make_shape(window, platform::Mask::Rounded(RADIUS.into()), cx);
        platform::center_on_active_screen(window, cx);
        window
            .observe_window_appearance(|window, _| window.refresh())
            .detach();
        cx.new(|_| QuitOverlay)
    })
    .inspect_err(|e| tracing::warn!("open quit overlay: {e:#}"))
    .ok()
}

/// Keep the production overlay visible for the dev screenshot script.
#[cfg(feature = "snapshot")]
pub fn preview(cx: &mut App) {
    let _ = open_overlay(cx);
}

struct QuitOverlay;

impl Render for QuitOverlay {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The window's effective appearance follows macOS and honors an
        // app-level appearance override, including changes while visible.
        let (background, foreground) = match window.appearance() {
            WindowAppearance::Light | WindowAppearance::VibrantLight => {
                (theme::white(0.92), theme::black(1.0))
            }
            WindowAppearance::Dark | WindowAppearance::VibrantDark => {
                (theme::glass_tint(), theme::white(1.0))
            }
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(background)
            .rounded(px(RADIUS))
            .text_color(foreground)
            .text_size(px(26.0))
            .font_weight(FontWeight::BOLD)
            .child(MESSAGE)
    }
}

/// A local monitor works in text fields and when only non-key call panels
/// remain. It never observes shortcuts sent to another app. Key-up and
/// modifier events are needed so a quick tap cannot become a timed hold.
struct NativeMonitor {
    events: Option<Retained<AnyObject>>,
    deactivation: Retained<AnyObject>,
}

impl NativeMonitor {
    fn new(sender: async_channel::Sender<Input>) -> Self {
        let deactivation_sender = sender.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            let _ = deactivation_sender.try_send(Input::Cancel);
        });
        // SAFETY: this immutable notification name and the callback's queued
        // message are valid for the registration's lifetime.
        let deactivation = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidResignActiveNotification),
                None,
                None,
                &block,
            )
        };
        let key = Cell::new(None);
        let block = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: AppKit supplies a live event for this callback.
            let native = unsafe { event.as_ref() };
            let flags = native.modifierFlags();
            let command = flags.contains(NSEventModifierFlags::Command);
            let extra = extra_shortcut_modifiers(flags);
            match native.r#type() {
                NSEventType::KeyDown
                    if command
                        && !extra
                        && native
                            .charactersByApplyingModifiers(NSEventModifierFlags::Command)
                            .is_some_and(|s| s.to_string().eq_ignore_ascii_case("q")) =>
                {
                    key.set(Some(native.keyCode()));
                    if !native.isARepeat() {
                        let _ = sender.try_send(Input::Press);
                    }
                    return std::ptr::null_mut();
                }
                NSEventType::KeyUp if key.get() == Some(native.keyCode()) => {
                    key.set(None);
                    let _ = sender.try_send(Input::Release);
                }
                NSEventType::FlagsChanged => {
                    if let Some(input) = modifier_input(native.keyCode(), flags) {
                        key.set(None);
                        let _ = sender.try_send(input);
                    }
                }
                NSEventType::KeyDown
                | NSEventType::LeftMouseDown
                | NSEventType::RightMouseDown
                | NSEventType::OtherMouseDown
                | NSEventType::ScrollWheel
                | NSEventType::BeginGesture
                | NSEventType::Gesture
                | NSEventType::Magnify
                | NSEventType::SmartMagnify
                | NSEventType::Swipe
                | NSEventType::Rotate => {
                    key.set(None);
                    let _ = sender.try_send(Input::Cancel);
                }
                _ => {}
            }
            event.as_ptr()
        });
        // SAFETY: the callback returns either the original live event or null
        // to consume Cmd+Q. AppKit copies the block; Drop removes the monitor.
        let events = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown
                    | NSEventMask::KeyUp
                    | NSEventMask::FlagsChanged
                    | NSEventMask::LeftMouseDown
                    | NSEventMask::RightMouseDown
                    | NSEventMask::OtherMouseDown
                    | NSEventMask::ScrollWheel
                    | NSEventMask::BeginGesture
                    | NSEventMask::Gesture
                    | NSEventMask::Magnify
                    | NSEventMask::SmartMagnify
                    | NSEventMask::Swipe
                    | NSEventMask::Rotate,
                &block,
            )
        };
        Self {
            events,
            deactivation: deactivation.into(),
        }
    }
}

impl Drop for NativeMonitor {
    fn drop(&mut self) {
        // SAFETY: these are the registration tokens returned by AppKit above.
        unsafe {
            if let Some(events) = &self.events {
                NSEvent::removeMonitor(events);
            }
            NSNotificationCenter::defaultCenter().removeObserver(&self.deactivation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_transitions_preserve_double_press_with_caps_lock_already_on() {
        for command_key in [54, 55] {
            for caps_lock in [
                NSEventModifierFlags::empty(),
                NSEventModifierFlags::CapsLock,
            ] {
                assert_eq!(
                    modifier_input(command_key, caps_lock | NSEventModifierFlags::Command),
                    None,
                );
                assert_eq!(modifier_input(command_key, caps_lock), Some(Input::Release));
            }
        }
    }

    #[test]
    fn caps_lock_and_function_changes_cancel_confirmation_in_both_directions() {
        for (key_code, modifier) in [
            (57, NSEventModifierFlags::CapsLock),
            (63, NSEventModifierFlags::Function),
        ] {
            assert_eq!(
                modifier_input(key_code, NSEventModifierFlags::Command | modifier),
                Some(Input::Cancel),
            );
            assert_eq!(
                modifier_input(key_code, NSEventModifierFlags::Command),
                Some(Input::Cancel),
            );
        }
        assert!(extra_shortcut_modifiers(
            NSEventModifierFlags::Command | NSEventModifierFlags::Function
        ));
        assert!(!extra_shortcut_modifiers(
            NSEventModifierFlags::Command | NSEventModifierFlags::CapsLock
        ));
    }

    #[test]
    fn two_presses_confirm_only_within_the_deadline() {
        let now = Instant::now();
        let mut confirmation = Confirmation::default();
        assert!(!confirmation.press(now));
        confirmation.down = false;
        assert!(confirmation.press(now + Duration::from_millis(500)));

        let mut confirmation = Confirmation::default();
        assert!(!confirmation.press(now));
        assert!(!confirmation.press(now + CONFIRM));
        assert!(!confirmation.held(now + CONFIRM));
    }

    #[test]
    fn a_hold_requires_the_keys_to_stay_down() {
        let now = Instant::now();
        let mut confirmation = Confirmation::default();
        confirmation.press(now);
        assert!(!confirmation.held(now + HOLD - Duration::from_millis(1)));
        assert!(confirmation.held(now + HOLD));
        confirmation.down = false;
        assert!(!confirmation.held(now + HOLD));
        confirmation = Confirmation::default();
        assert!(!confirmation.held(now + CONFIRM));
        assert!(!confirmation.press(now + CONFIRM));
    }
}
