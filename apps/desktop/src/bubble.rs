//! A camera floating by the call widget, or the preview of what you share.
//!
//! Each is a window of its own, so the compositor cuts it to its shape for
//! free: a circle up to 180 px, then a tile that widens to 4:3. A bubble
//! rides on the widget until it's dragged away; dropped by the widget, it
//! rides again. Scroll or pinch over one to zoom it, double-click for big
//! or small. The widget ([`Capsule`]) decides which bubbles there are and
//! where the riding ones go; a bubble only draws and passes on what's done
//! to it.

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Div, Entity, EntityId, FontWeight, MouseButton, MouseDownEvent,
    ObjectFit, PinchEvent, ScrollEvent, Stateful, Subscription, WeakEntity, Window, WindowBounds,
    WindowHandle, div, point, px, rgb, size,
};

use crate::assets::Icon;
use crate::call::{Call, Member};
use crate::capsule::Capsule;
use crate::mac;
use crate::theme::{ACCENT, DANGER, PILL, SPEAKING, avatar_background, black, first_name, white};
use crate::ui::{avatar, icon, ring};

/// A bubble's height.
pub const MIN: f32 = 56.0;
pub const MAX: f32 = 340.0;
/// A circle up to this; then it widens into a tile.
const ROUND: f32 = 180.0;
/// How much more it takes to be a whole 4:3 tile.
const MORPH: f32 = 70.0;
const TILE_RADIUS: f32 = 26.0;
const SHARE_RADIUS: f32 = 14.0;
const SHARE_MIN: f32 = 70.0;
const SHARE_MAX: f32 = 240.0;
/// Double-click: big if it's smaller than this, small otherwise.
const MIDDLE: f32 = 150.0;
const RING: f32 = 2.5;
/// The button under the cursor, and what it keeps from the shape's edge.
const DISMISS: f32 = 24.0;
const DISMISS_INSET: f32 = 5.0;
/// Too small for a name.
const TINY: f32 = 76.0;

/// Whose bubble.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Who {
    /// Someone else's camera (an identity).
    Member(String),
    /// The one bubble that follows the speaker.
    Stage,
    /// Your camera.
    Me,
    /// What you share.
    Share,
}

impl Who {
    /// The height a new bubble starts at.
    pub fn first_size(&self) -> f32 {
        match self {
            Who::Me => 84.0,
            Who::Share => 124.0,
            Who::Member(_) | Who::Stage => 112.0,
        }
    }

    pub fn clamp(&self, size: f32) -> f32 {
        match self {
            Who::Share => size.clamp(SHARE_MIN, SHARE_MAX),
            _ => size.clamp(MIN, MAX),
        }
    }

    /// The height a double-click goes to from `size`.
    pub fn other_size(&self, size: f32) -> f32 {
        match (self, size < MIDDLE) {
            (Who::Share, true) => 200.0,
            (Who::Share, false) => 124.0,
            (_, true) => 230.0,
            (_, false) => 96.0,
        }
    }
}

/// A bubble `height` tall: its width and corner radius. `aspect` is the
/// shape of what you share (for [`Who::Share`]).
pub fn shape(who: &Who, height: f32, aspect: f32) -> (f32, f32) {
    let h = height.round();
    if *who == Who::Share {
        return ((h * aspect).round(), SHARE_RADIUS);
    }
    if h <= ROUND {
        return (h, h / 2.0);
    }
    let t = ((h - ROUND) / MORPH).min(1.0);
    (
        (h * (1.0 + t / 3.0)).round(),
        (h / 2.0 + (TILE_RADIUS - h / 2.0) * t).round(),
    )
}

/// The corner radius of a bubble `w` by `h`, as [`shape`] made it.
fn radius(who: &Who, w: f32, h: f32) -> f32 {
    if *who == Who::Share {
        SHARE_RADIUS
    } else if w <= h + 0.5 {
        h / 2.0
    } else {
        shape(who, h, 1.0).1
    }
}

/// Where the button goes in a bubble `w` wide with corners of radius `r`
/// (its top-left corner): on the diagonal of the top right corner, wholly
/// inside the shape. The window is cut to the shape, so anything on the rim
/// would be cut with it.
fn dismiss_at(w: f32, r: f32) -> (f32, f32) {
    let half = DISMISS / 2.0;
    let along = (r - half - DISMISS_INSET).max(0.0) * std::f32::consts::FRAC_1_SQRT_2;
    (w - r + along - half, r - along - half)
}

/// Opens out of sight; the widget places it and shows it.
pub fn open(
    call: Entity<Call>,
    capsule: WeakEntity<Capsule>,
    who: Who,
    cx: &mut App,
) -> Option<(WindowHandle<Bubble>, mac::Native)> {
    let s = who.first_size();
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(s), px(s)));
    let mut native = None;
    let handle = cx
        .open_window(
            mac::hidden_floating_options(WindowBounds::Windowed(bounds)),
            |window, cx| {
                native = mac::Native::of(window);
                cx.new(|cx| Bubble::new(call, capsule, who, cx))
            },
        )
        .inspect_err(|e| tracing::warn!("open bubble: {e:#}"))
        .ok()?;
    Some((handle, native?))
}

pub struct Bubble {
    call: Entity<Call>,
    capsule: WeakEntity<Capsule>,
    who: Who,
    hovered: bool,
    /// The person shown, and the repaints their frames ask for.
    shown: Option<(EntityId, Subscription)>,
    _subscriptions: Vec<Subscription>,
}

impl Bubble {
    fn new(
        call: Entity<Call>,
        capsule: WeakEntity<Capsule>,
        who: Who,
        cx: &mut Context<Self>,
    ) -> Self {
        // The widget says who's on the stage; the call, what you share.
        let mut subscriptions = vec![cx.observe(&call, |_, _, cx| cx.notify())];
        if let Some(capsule) = capsule.upgrade() {
            subscriptions.push(cx.observe(&capsule, |_, _, cx| cx.notify()));
        }
        Self {
            call,
            capsule,
            who,
            hovered: false,
            shown: None,
            _subscriptions: subscriptions,
        }
    }

    fn to_capsule(&self, cx: &mut App, f: impl FnOnce(&mut Capsule, &Who, &mut Context<Capsule>)) {
        let who = self.who.clone();
        let _ = self.capsule.update(cx, |capsule, cx| f(capsule, &who, cx));
    }

    /// Repaint with the frames of whoever is shown.
    fn follow(&mut self, member: Option<&Entity<Member>>, cx: &mut Context<Self>) {
        let id = member.map(|m| m.entity_id());
        if self.shown.as_ref().map(|(id, _)| *id) == id {
            return;
        }
        self.shown = member.map(|m| (m.entity_id(), cx.observe(m, |_, _, cx| cx.notify())));
    }

    fn camera(&self, member: &Member, w: f32, h: f32, r: f32) -> Div {
        let speaking = member.speaking && !member.mic_muted;
        let content = match member.video.as_ref().and_then(|v| v.sink.latest()) {
            Some(frame) => gpui::surface(frame.0)
                .object_fit(ObjectFit::Cover)
                .size_full()
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(avatar(&member.name, (h * 0.5).round()))
                .into_any_element(),
        };
        let name = if member.is_local {
            "You".to_owned()
        } else {
            first_name(&member.name).to_owned()
        };
        let label = (h >= TINY).then(|| {
            div()
                .absolute()
                .left_0()
                .bottom(px((h * 0.08).round()))
                .w(px(w))
                .flex()
                .justify_center()
                .child(
                    div()
                        .h(px(18.0))
                        .px(px(7.0))
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .rounded(px(9.0))
                        .bg(black(0.62))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(member.mic_muted, |el| {
                            el.child(icon(Icon::MicOff, 9.0, rgb(0xff8a80)))
                        })
                        .child(name),
                )
        });
        div()
            .size_full()
            .relative()
            .bg(avatar_background(&member.name))
            .child(content)
            // No border of its own: only the green of someone speaking.
            .when(speaking, |el| el.child(ring(r, RING, rgb(SPEAKING))))
            .children(label)
    }

    /// The button that shows under the cursor: your camera off, or their
    /// video out of sight. In the top right corner, inside the shape.
    fn dismiss(&self, w: f32, r: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let (left, top) = dismiss_at(w, r);
        div()
            .id("dismiss")
            .absolute()
            .left(px(left))
            .top(px(top))
            .size(px(DISMISS))
            .rounded_full()
            .bg(rgb(PILL))
            .border_1()
            .border_color(white(0.16))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x2a2c35)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                this.to_capsule(cx, |capsule, who, cx| capsule.bubble_dismissed(who, cx))
            }))
            .child(icon(Icon::VideoOff, 12.0, rgb(0xffffff)))
    }

    fn share(&self, cx: &mut Context<Self>) -> Div {
        let c = self.call.read(cx);
        let frame = c.me.read(cx).screen.as_ref().and_then(|v| v.sink.latest());
        let preset = c.sharing.map(|p| p.label());
        let call = self.call.clone();
        div()
            .size_full()
            .relative()
            .bg(rgb(0x000000))
            .children(frame.map(|f| {
                gpui::surface(f.0)
                    .object_fit(ObjectFit::Contain)
                    .size_full()
            }))
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .top(px(8.0))
                    .h(px(22.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(8.0))
                    .bg(black(0.62))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().size(px(7.0)).rounded_full().bg(rgb(ACCENT)))
                    .child(match preset {
                        Some(preset) => format!("Sharing · {preset}"),
                        None => "Starting to share…".into(),
                    }),
            )
            .child(
                div()
                    .id("stop-share")
                    .absolute()
                    .right(px(8.0))
                    .top(px(8.0))
                    .h(px(22.0))
                    .px(px(9.0))
                    .flex()
                    .items_center()
                    .rounded(px(8.0))
                    .bg(rgb(DANGER))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.88))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| call.update(cx, |c, cx| c.toggle_share(cx)))
                    .child("Stop"),
            )
    }
}

impl Render for Bubble {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.bounds().size;
        let (w, h) = (bounds.width / px(1.0), bounds.height / px(1.0));
        let r = radius(&self.who, w, h);
        let member = self
            .capsule
            .upgrade()
            .and_then(|c| c.read(cx).bubble_member(&self.who, cx));
        self.follow(member.as_ref(), cx);
        let body = match (&self.who, &member) {
            (Who::Share, _) => self.share(cx),
            (_, Some(member)) => self.camera(member.read(cx), w, h, r),
            // Between people: the widget is about to close this bubble.
            (_, None) => div().size_full().bg(rgb(0x0b0c0f)),
        };
        let dismiss = (self.hovered && self.who != Who::Share && member.is_some())
            .then(|| self.dismiss(w, r, cx));
        div()
            .id("bubble")
            .size_full()
            .relative()
            .text_color(rgb(0xffffff))
            .child(body)
            .children(dismiss)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if event.click_count >= 2 {
                        this.to_capsule(cx, |capsule, who, cx| capsule.bubble_flipped(who, cx));
                    } else {
                        window.start_window_move();
                        this.to_capsule(cx, |capsule, who, cx| capsule.bubble_pressed(who, cx));
                    }
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                let dy = event.delta.pixel_delta(px(16.0)).y / px(1.0);
                let by = (dy * 0.0025).exp();
                this.to_capsule(cx, |capsule, who, cx| capsule.bubble_zoomed(who, by, cx));
            }))
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                let by = 1.0 + event.delta;
                this.to_capsule(cx, |capsule, who, cx| capsule.bubble_zoomed(who, by, cx));
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_circle_until_180_then_a_tile() {
        let who = Who::Member("anna".into());
        assert_eq!(shape(&who, 112.0, 1.0), (112.0, 56.0));
        assert_eq!(shape(&who, 180.0, 1.0), (180.0, 90.0));
        // Halfway: a sixth wider, the corners halfway to a tile's.
        let (w, r) = shape(&who, 215.0, 1.0);
        assert_eq!(w, (215.0f32 * (1.0 + 0.5 / 3.0)).round());
        assert!(r < 215.0 / 2.0 && r > TILE_RADIUS);
        // A whole tile: 4:3.
        assert_eq!(shape(&who, 300.0, 1.0), (400.0, TILE_RADIUS));
        assert_eq!(radius(&who, 400.0, 300.0), TILE_RADIUS);
        assert_eq!(radius(&who, 112.0, 112.0), 56.0);
    }

    #[test]
    fn the_button_stays_inside_the_shape() {
        // Circles from the smallest to the biggest, then tiles.
        for h in [MIN, 84.0, 112.0, 180.0, 215.0, 300.0, MAX] {
            let who = Who::Me;
            let (w, r) = shape(&who, h, 1.0);
            let (left, top) = dismiss_at(w, r);
            let half = DISMISS / 2.0;
            // The button's middle, from the middle of the corner's arc.
            let (dx, dy) = (left + half - (w - r), r - (top + half));
            assert!(dx >= 0.0 && dy >= 0.0, "{h}: {left}, {top}");
            assert!(
                dx.hypot(dy) + half <= r - DISMISS_INSET + 0.01,
                "{h}: cut by the edge"
            );
            assert!(left + DISMISS <= w && top >= 0.0, "{h}");
        }
    }

    #[test]
    fn a_share_keeps_its_shape() {
        assert_eq!(shape(&Who::Share, 124.0, 1.6), (198.0, SHARE_RADIUS));
        assert_eq!(Who::Share.clamp(999.0), SHARE_MAX);
        assert_eq!(Who::Me.clamp(10.0), MIN);
    }

    #[test]
    fn double_click_goes_big_then_small() {
        assert_eq!(Who::Me.other_size(84.0), 230.0);
        assert_eq!(Who::Me.other_size(230.0), 96.0);
        assert_eq!(Who::Share.other_size(124.0), 200.0);
    }
}
