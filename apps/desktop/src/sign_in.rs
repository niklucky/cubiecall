//! Signing in: your email, then the six digits sent to it. Where a Mac
//! without an account starts, and where an expired session comes back to.
//! No browser and no password. A new account goes on to `onboarding.rs` for
//! its name.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Div, Entity, FontWeight, SharedString, Subscription, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, img, px, rgb, size,
};

use crate::Shell;
use crate::assets::{Icon, QUBY};
use crate::auth;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{ACCENT, BG, DANGER, FIELD, RAISED, TEXT, TEXT_DIM, TEXT_FAINT, white};
use crate::ui::{Kind, button, icon};

/// How soon another code can be asked for: the server sends only a few to
/// one address in a quarter of an hour.
const RESEND_AFTER: Duration = Duration::from_secs(30);
const CODE_LEN: usize = 6;

/// Open the window, or bring it forward. Deferred by [`Shell::sign_in`].
pub fn open(cx: &mut App) {
    for handle in cx.windows() {
        if let Some(existing) = handle.downcast::<SignInWindow>() {
            let _ = existing.update(cx, |_, window, _| window.activate_window());
            return;
        }
    }
    // Cancelled before this ran.
    if !Shell::global(cx).read(cx).auth_attempt.pending() {
        return;
    }
    let bounds = Bounds::centered(None, size(px(400.0), px(500.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Sign in to Cubie".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        is_resizable: false,
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| SignInWindow::new(window, cx))
    });
    if let Err(e) = opened {
        tracing::warn!("open sign-in: {e:#}");
        Shell::global(cx).update(cx, |s, cx| s.cancel_sign_in(cx));
    }
}

/// Test scripts type for the person: an address, then Continue.
#[cfg(feature = "snapshot")]
pub fn script_email(email: &str, cx: &mut App) {
    with_window(cx, |this, window, cx| {
        this.email
            .update(cx, |input, cx| input.set_text(email.to_owned(), cx));
        this.send(window, cx);
    });
}

/// Test scripts type for the person: the code from the email.
#[cfg(feature = "snapshot")]
pub fn script_code(code: &str, cx: &mut App) {
    with_window(cx, |this, window, cx| {
        this.code
            .update(cx, |input, cx| input.set_text(code.to_owned(), cx));
        this.typed(window, cx);
    });
}

#[cfg(feature = "snapshot")]
fn with_window(
    cx: &mut App,
    act: impl FnOnce(&mut SignInWindow, &mut Window, &mut Context<SignInWindow>),
) {
    let window = cx
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<SignInWindow>());
    match window {
        Some(window) => drop(window.update(cx, act)),
        None => tracing::warn!("script: the sign-in window isn't open"),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Email,
    Code,
}

pub struct SignInWindow {
    email: Entity<TextInput>,
    code: Entity<TextInput>,
    step: Step,
    /// The address the code went to, as typed.
    sent_to: String,
    /// What the server gave for that request; the code only works with it.
    challenge: String,
    sent_at: Option<Instant>,
    error: SharedString,
    /// A request on its way. Dropping it (a new one, the window closing)
    /// cancels what would follow its answer.
    working: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

/// Enough of an address to be worth sending: the server has the last word.
fn plausible(email: &str) -> bool {
    let email = email.trim();
    !email.contains(char::is_whitespace)
        && email.split_once('@').is_some_and(|(name, host)| {
            !name.is_empty() && host.contains('.') && !host.ends_with('.')
        })
}

/// The digits in what was typed or pasted ("482 913" as the email shows it).
fn digits(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_digit)
        .take(CODE_LEN)
        .collect()
}

impl SignInWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let email = cx.new(|cx| TextInput::new("", "Your email", cx).with_max_len(254));
        // Room for a pasted code with spaces in it; only its digits are kept.
        let code = cx.new(|cx| TextInput::new("", "", cx).with_max_len(16));
        email.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions = [
            cx.subscribe_in(&email, window, |this, _, event, window, cx| match event {
                InputEvent::Submit => this.send(window, cx),
                InputEvent::Changed => {
                    this.error = "".into();
                    cx.notify();
                }
                InputEvent::Cancel => {}
            }),
            cx.subscribe_in(&code, window, |this, _, event, window, cx| match event {
                InputEvent::Changed => this.typed(window, cx),
                InputEvent::Submit => this.check(window, cx),
                InputEvent::Cancel => this.back(window, cx),
            }),
            // Closed before it was done: nothing is waiting to sign in.
            cx.on_release(|_, cx| Shell::global(cx).update(cx, |s, cx| s.cancel_sign_in(cx))),
        ];
        Self {
            email,
            code,
            step: Step::Email,
            sent_to: String::new(),
            challenge: String::new(),
            sent_at: None,
            error: "".into(),
            working: None,
            _subscriptions: subscriptions,
        }
    }

    fn busy(&self) -> bool {
        self.working.is_some()
    }

    /// Email a code to the address in the field (again, from the code step).
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let email = self.email.read(cx).text().trim().to_owned();
        if self.busy() || !plausible(&email) {
            return;
        }
        self.error = "".into();
        let server = Shell::global(cx).read(cx).args.server.clone();
        self.working = Some(cx.spawn_in(window, async move |this, cx| {
            let sent = auth::request_code(&server, &email).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.working = None;
                match sent {
                    Ok(challenge) => {
                        this.challenge = challenge;
                        this.sent_to = email;
                        this.sent_at = Some(Instant::now());
                        this.step = Step::Code;
                        this.code.update(cx, |input, cx| {
                            input.set_text("", cx);
                            input.focus(window, cx);
                        });
                        // "Send a new code" wakes up without a keystroke.
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(RESEND_AFTER).await;
                            let _ = this.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                    Err(e) => this.error = e.to_string().into(),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Keep the digits; the sixth one sends them.
    fn typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.code.read(cx).text().to_owned();
        let code = digits(&typed);
        if code != typed {
            self.code
                .update(cx, |input, cx| input.set_text(code.clone(), cx));
        }
        self.error = "".into();
        if code.len() == CODE_LEN {
            self.check(window, cx);
        }
        cx.notify();
    }

    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = digits(self.code.read(cx).text());
        if self.busy() || code.len() != CODE_LEN {
            return;
        }
        let shell = Shell::global(cx);
        let (server, previous) = {
            let s = shell.read(cx);
            (s.args.server.clone(), s.args.session.clone())
        };
        let challenge = self.challenge.clone();
        self.working = Some(cx.spawn_in(window, async move |this, cx| {
            let verified = auth::verify_code(&server, previous.as_deref(), &challenge, &code).await;
            let installed = match verified {
                Ok((token, principal)) => {
                    let kept = shell.update(cx, |s, cx| s.signed_in(token.clone(), principal, cx));
                    if kept.is_err() {
                        auth::discard(&server, &token).await;
                    }
                    kept
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.working = None;
                match installed {
                    // What comes next (a name, home, a call) was asked to open
                    // first: a moment with no window at all would quit the app.
                    Ok(()) => cx.defer_in(window, |_, window, _| window.remove_window()),
                    Err(e) => {
                        this.error = e.into();
                        this.code.update(cx, |input, cx| {
                            input.set_text("", cx);
                            input.focus(window, cx);
                        });
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.working = None;
        self.step = Step::Email;
        self.error = "".into();
        self.email.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn can_resend(&self) -> bool {
        !self.busy() && self.sent_at.is_none_or(|at| at.elapsed() >= RESEND_AFTER)
    }

    fn email_step(&self, cx: &mut Context<Self>) -> Div {
        let ready = plausible(self.email.read(cx).text()) && !self.busy();
        // Why this window opened, when it wasn't asked for: an expired session.
        let why = Shell::global(cx).read(cx).auth_message.clone();
        column()
            .child(img(QUBY).size(px(84.0)))
            .child(title("Welcome to Cubie"))
            .child(sub(if why.is_empty() {
                "Sign in with your email. No password needed.".into()
            } else {
                why
            }))
            .child(
                div()
                    .mt(px(22.0))
                    .w_full()
                    .h(px(38.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .rounded(px(9.0))
                    .bg(rgb(FIELD))
                    .border_1()
                    .border_color(rgb(ACCENT))
                    .child(self.email.clone()),
            )
            .child(
                button(
                    "continue",
                    if self.busy() {
                        "Sending…"
                    } else {
                        "Continue"
                    },
                    None,
                    Kind::Primary,
                    false,
                )
                .mt(px(10.0))
                .w_full()
                .h(px(38.0))
                .justify_center()
                .when(!ready, |el| el.opacity(0.5))
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            )
            .children(self.problem())
            .child(div().flex_1())
            .child(foot("Cubie is invite-only for now."))
    }

    fn code_step(&self, cx: &mut Context<Self>) -> Div {
        let code = self.code.read(cx).text().to_owned();
        let failed = !self.error.is_empty();
        let boxes = (0..CODE_LEN).map(|i| {
            let current = i == code.len().min(CODE_LEN - 1);
            div()
                .w(px(40.0))
                .h(px(48.0))
                // A wider gap in the middle, as the email writes it: 482 913.
                .when(i == CODE_LEN / 2, |el| el.ml(px(8.0)))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(9.0))
                .bg(rgb(FIELD))
                .border_1()
                .border_color(if failed {
                    rgb(DANGER).into()
                } else if current {
                    rgb(ACCENT).into()
                } else {
                    white(0.12)
                })
                .text_size(px(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .children(code.get(i..=i).map(str::to_owned))
        });
        let resend = self.can_resend();
        column()
            .child(
                div()
                    .size(px(84.0))
                    .rounded_full()
                    .bg(rgb(RAISED))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(Icon::Mail, 34.0, rgb(TEXT_DIM))),
            )
            .child(title("Check your email"))
            .child(sub(format!(
                "Enter the 6-digit code we sent to {}.",
                self.sent_to
            )))
            .child(
                div()
                    .relative()
                    .mt(px(22.0))
                    .flex()
                    .gap(px(6.0))
                    .children(boxes)
                    // The real field lies over the boxes, unseen: typing and
                    // pasting go to it, and the boxes show what it holds.
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .opacity(0.0)
                            .child(self.code.clone()),
                    ),
            )
            .children(self.problem())
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .gap(px(6.0))
                    .text_color(rgb(TEXT_FAINT))
                    .child(
                        link("resend", "Send a new code", resend).when(resend, |el| {
                            el.on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                        }),
                    )
                    .child("·")
                    .child(
                        link("other-email", "Use a different email", true)
                            .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                    ),
            )
            .child(div().flex_1())
            .child(foot(if self.busy() {
                "Checking…"
            } else {
                "The code works for 15 minutes."
            }))
    }

    fn problem(&self) -> Option<Div> {
        (!self.error.is_empty()).then(|| {
            div()
                .mt(px(12.0))
                .w_full()
                .text_center()
                .text_size(px(12.0))
                .text_color(rgb(DANGER))
                .child(self.error.clone())
        })
    }
}

fn column() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .px(px(40.0))
        .pt(px(64.0))
        .pb(px(22.0))
        .bg(rgb(BG))
        .text_color(rgb(TEXT))
        .text_size(px(13.0))
}

fn title(text: &'static str) -> Div {
    div()
        .mt(px(18.0))
        .mb(px(6.0))
        .text_size(px(19.0))
        .font_weight(FontWeight::BOLD)
        .child(text)
}

fn sub(text: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .text_center()
        .text_color(rgb(TEXT_DIM))
        .child(text.into())
}

fn foot(text: &'static str) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(rgb(TEXT_FAINT))
        .child(text)
}

fn link(id: &'static str, text: &'static str, enabled: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .when(enabled, |el| {
            el.text_color(rgb(ACCENT))
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
        })
        .child(text)
}

impl Render for SignInWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.step {
            Step::Email => self.email_step(cx),
            Step::Code => self.code_step(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_code_keeps_its_six_digits() {
        assert_eq!(digits("482 913"), "482913");
        assert_eq!(digits(" 482913\n"), "482913");
        assert_eq!(digits("Your Cubie code: 4829139"), "482913");
        assert_eq!(digits("48a"), "48");
    }

    #[test]
    fn continue_waits_for_something_like_an_address() {
        assert!(plausible("mom@gmail.com"));
        assert!(plausible("  mom@mail.example.org "));
        for not_yet in [
            "",
            "mom",
            "mom@",
            "mom@gmail",
            "@gmail.com",
            "mom@gmail.",
            "mo m@gmail.com",
        ] {
            assert!(!plausible(not_yet), "{not_yet}");
        }
    }
}
