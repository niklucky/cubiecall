//! Minimal mode: the call in one small floating widget.
//!
//! A solid pill, exactly as long as what's in it: up to three people who
//! were active last (spoke, unmuted or joined; a newcomer takes the place of
//! whoever has been quiet longest), then you (muted or not, so muting moves
//! nothing), then a stack with everyone muted or quiet for two minutes, then
//! a red record button while anyone records, a captions button while anyone
//! transcribes, then ⋯, Share, Mic and Leave. In a call of two or three
//! nobody goes to the stack. Alone, a dashed seat copies the link.
//!
//! The pill lies down, or stands as a column once it's dropped at the left
//! or right side of the screen (and lies down again at the top or bottom).
//! Pushed right into an edge it tucks away as a tab with a dot per person.
//! Pausing over an edge while dragging shows the outline a drop would make.
//!
//! Everything else floats by the pill and never changes it: cameras and
//! your share's preview as bubbles (windows of their own, see
//! [`crate::bubble`]), the sentence being said while the call is
//! transcribed, and a card: the ⋯ menu, the stack's people, a name under the
//! cursor, a notice, a question. Cards open on a click, names on hover. The
//! pill never moves: the window grows around it, and it gets longer or
//! shorter at the end away from its buttons, so they stay under the cursor.
//!
//! A face is that person's video switch: yours turns your camera on and
//! off, anyone else's hides and shows their bubble. In a call of four or
//! more, one bubble follows the speaker instead (Settings can give everyone
//! a bubble).

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EntityId, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, Size, Stateful, Subscription, Task, Window,
    WindowBounds, WindowHandle, div, point, px, rgb, size,
};

use crate::Shell;
use crate::assets::Icon;
use crate::bubble::{self, Bubble, Who};
use crate::call::{Call, Camera, Member, Mode, NoticeAction, Preparing, PromptKind};
use crate::platform::{self, Rect};
use crate::theme::{
    ACCENT, CARD, DANGER, PILL, RECORDING, SPEAKING, TEXT, TEXT_DIM, first_name, white,
};
use crate::ui::{Kind, Tone, avatar, button, icon, icon_button, ring, round_button};

/// People in the row besides you.
const ROW_MAX: usize = 3;
/// With this many others or fewer nobody goes to the stack, and everyone
/// with a camera gets a bubble: a call of two or three never reshuffles.
const SMALL: usize = 2;
/// Quiet this long and you go into the stack.
const QUIET: Duration = Duration::from_secs(120);
const FACE: f32 = 40.0;
const PAD: f32 = 6.0;
const GAP: f32 = 6.0;
const BUTTON: f32 = 38.0;
/// The pill's height, lying down; its width as a column.
const MAIN: f32 = 52.0;
const RADIUS: f32 = MAIN / 2.0;
/// Between the pill and what floats by it: bubbles, the caption, a card.
const FLOAT_GAP: f32 = 8.0;
/// Green while someone speaks.
const RING: f32 = 2.5;
const EDGE: u32 = 0x1d1f26;
/// The stack: avatars behind the front one, and the "+N" after it.
const GHOST: f32 = 9.0;
const COUNT: f32 = 26.0;
/// A person in the stack's card: avatar and first name. Four a line.
const PERSON: f32 = 54.0;
const PEOPLE_W: f32 = 4.0 * PERSON + 3.0 * 4.0 + 24.0;
const CARD_RADIUS: f32 = 16.0;
/// The ⋯ menu: its width, its line with the room's name, a line of it.
const MENU_W: f32 = 216.0;
const MENU_HEAD: f32 = 30.0;
const MENU_ITEM: f32 = 34.0;
/// A card of one line: a name, a notice.
const LINE_H: f32 = 28.0;
/// A line with a button in it (Stop, Show in Finder).
const BUTTON_LINE_H: f32 = 36.0;
/// A line of text over a line of buttons (what to record).
const CHOICE_H: f32 = 10.0 + 18.0 + 6.0 + 24.0 + 10.0;
/// The sentence being said: its height, and how wide it gets by a pill
/// lying down. By a column, it and the cards without a width of their own
/// get [`SIDE_W`].
const CAPTION_H: f32 = 30.0;
const CAPTION_MAX: f32 = 420.0;
const SIDE_W: f32 = 300.0;
/// The tab the widget tucks into: how thick, and how long per person.
const TAB: f32 = 18.0;
const TAB_DOT: f32 = 9.0;
const TAB_STEP: f32 = 15.0;
/// How deep the bands along a screen's edges are, as parts of its width (at
/// the sides) or height (top and bottom). Dropped with the cursor in the
/// outer one, the pill goes to stand by that edge: a column at a side, lying
/// down at the top or bottom. In the inner one it tucks into the edge.
const TURN: f64 = 0.10;
const TUCK: f64 = 0.05;
/// A drop is for an edge only after the drag came this far towards it: a
/// pill standing by an edge slides along it, or comes away from it, as it is.
const PUSH: f64 = 20.0;
/// What the pill keeps from the screen's edges when a drop places it.
const MARGIN: f64 = 8.0;
/// The outline of what a drop would make shows after the cursor has been
/// still this long.
const PAUSE: Duration = Duration::from_millis(240);
/// While something is dragged, where it is gets checked this often: macOS
/// moves the window by itself and says nothing when the button goes up.
const DRAG_POLL: Duration = Duration::from_millis(40);
/// A bubble dropped this near the pill rides on it again; dropped this near
/// the screen's edge, it lines up with it.
const MAGNET: f64 = 30.0;
const SNAP: f64 = 16.0;
/// The bubble that follows the speaker stays on someone at least this long.
const STAGE_HOLD: Duration = Duration::from_secs(3);
/// Holding the mic button this long is holding to talk, not a click.
const HOLD_AFTER: Duration = Duration::from_millis(280);
/// A name opens and closes after the cursor settles this long.
const HOVER_DELAY: Duration = Duration::from_millis(250);
/// A card opened by a click closes once the cursor has been away this long
/// (or on a click anywhere else): the widget never has focus to lose.
const AWAY: Duration = Duration::from_secs(5);
/// While a card is open, where the cursor is gets checked this often: GPUI
/// reports hovering only while the mouse moves inside the window, so
/// nothing says the cursor has left, or that a click landed elsewhere.
const CURSOR_POLL: Duration = Duration::from_millis(60);

pub fn open(call: Entity<Call>, cx: &mut App) -> Option<WindowHandle<Capsule>> {
    let area = cx
        .primary_display()
        .map(|d| d.visible_bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.0), px(0.0)), size(px(1440.0), px(900.0))));
    // A first size; the widget fits itself as soon as it opens.
    let s = size(240.0, MAIN);
    let bounds = Bounds::new(
        point(
            area.origin.x + (area.size.width - px(s.width)) / 2.0,
            area.origin.y + area.size.height - px(s.height) - px(28.0),
        ),
        size(px(s.width), px(s.height)),
    );
    // Transparent: the pill and the cards each draw themselves.
    let options = platform::floating_options(WindowBounds::Windowed(bounds));
    cx.open_window(options, |window, cx| {
        cx.new(|cx| Capsule::new(call, s, window, cx))
    })
    .inspect_err(|e| tracing::warn!("open capsule: {e:#}"))
    .ok()
}

/// The card floating by the pill.
#[derive(Clone, PartialEq)]
enum Drawer {
    /// The ⋯ menu: camera, invite, call window, record, transcribe.
    Menu,
    /// Everyone in the stack.
    People,
    /// The name of the face under the cursor (an identity).
    Name(String),
    /// Who records the call, for how long; Stop if it's you.
    Recording,
    /// What to record: audio, or audio and a screen.
    RecordChoice,
    /// Transcribe on this Mac (with a download first, maybe).
    TranscribeChoice,
    /// Whose Mac transcribes, with which model; Stop if it's yours.
    Transcript,
    /// A question until you answer it: transcribe on this Mac?
    Prompt,
    /// "Joining…", a permission prompt or an error.
    Notice,
}

/// An edge of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// The left or the right: where the pill stands as a column.
    fn side(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }
}

/// How a drag of the pill went: where the cursor was when it began, and how
/// far it has been each way since.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    start: (f64, f64),
    min: (f64, f64),
    max: (f64, f64),
}

impl Drag {
    fn new(start: (f64, f64)) -> Self {
        Self {
            start,
            min: start,
            max: start,
        }
    }

    fn through(mut self, p: (f64, f64)) -> Self {
        self.min = (self.min.0.min(p.0), self.min.1.min(p.1));
        self.max = (self.max.0.max(p.0), self.max.1.max(p.1));
        self
    }
}

/// What dropping the pill here makes of it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Zone {
    edge: Edge,
    /// Tucked into the edge as a tab; otherwise only turned to fit it.
    tuck: bool,
    /// A column (left and right), not lying down (top and bottom).
    column: bool,
    /// Where the pill's bottom-right corner goes, on screen.
    corner: (f64, f64),
    /// The pill's outline there.
    pill: Rect,
}

/// Who goes where, for one frame.
struct Plan {
    /// Everyone in the row. You last.
    faces: Vec<Entity<Member>>,
    /// Nobody else is here: a seat that copies the link.
    seat: bool,
    /// Others muted or quiet, the most recently active first.
    stack: Vec<Entity<Member>>,
    /// Identities of people sharing their screen.
    sharers: Vec<String>,
    /// The shape of what you share, while you share.
    preview: Option<f32>,
    /// The bubbles to show, in the order they ride on the pill.
    bubbles: Vec<Who>,
    /// The call is transcribed: who is speaking and what they say, or
    /// nothing yet.
    caption: Option<Option<(String, String)>>,
    /// Someone records the call: the record button shows.
    recording: bool,
    /// Someone transcribes the call, or your Mac gets ready to: the
    /// captions button shows.
    captions: bool,
    /// Record and Transcribe in the menu, while the call has neither.
    menu_extras: usize,
    /// The notice has a button (Show in Finder, Settings).
    notice_button: bool,
}

/// A bubble's window.
struct Slot {
    handle: WindowHandle<Bubble>,
    native: platform::Native,
}

/// A bubble's height, and whether it rides on the pill.
#[derive(Clone, Copy)]
struct Geo {
    size: f32,
    docked: bool,
}

/// What of a person shows in the pill: it's drawn again only when this
/// changes, not with every frame of their video.
#[derive(Clone, PartialEq)]
struct Sig {
    name: String,
    speaking: bool,
    muted: bool,
    video: bool,
    /// The shape of what they share, in hundredths.
    screen: Option<i32>,
}

impl Sig {
    fn of(m: &Member) -> Self {
        Self {
            name: m.name.clone(),
            speaking: m.speaking,
            muted: m.mic_muted,
            video: m.video.is_some(),
            screen: m
                .screen
                .as_ref()
                .map(|s| s.sink.aspect().map_or(0, |a| (a * 100.0) as i32)),
        }
    }
}

pub struct Capsule {
    call: Entity<Call>,
    native: Option<platform::Native>,
    /// The people in the row besides you; positions stay put.
    slots: Vec<Entity<Member>>,
    /// The card opened by a click, or the name under the cursor.
    open: Option<Drawer>,
    /// Standing at the screen's side, not lying down.
    column: bool,
    /// Tucked into this edge as a tab.
    tucked: Option<Edge>,
    /// While tucked: where the pill's corner goes when it comes back.
    home: (f64, f64),
    size: Size<f32>,
    /// Where the pill's bottom-right corner is in the window, from the
    /// window's top-left.
    anchor: (f32, f32),
    /// [`Self::anchor`] for the window's frame as it is now: a resize lands
    /// a turn later (set right before it), and until then the pill is drawn
    /// here, so it stays put.
    shown: (f32, f32),
    /// The pill's corner must go here on screen: it was dropped at an edge,
    /// or comes back from one.
    jump: Option<(f64, f64)>,
    /// A jump on its way: where the pill will be.
    moving: Option<(f64, f64)>,
    /// The card is on the pill's first side (over it, lying down).
    card_first: bool,
    /// Bubbles and the caption ride over the pill, not under it.
    dock_up: bool,
    /// By a column, things open to its right.
    side_right: bool,
    bubbles: HashMap<Who, Slot>,
    opening: HashSet<Who>,
    geo: HashMap<Who, Geo>,
    /// A bubble under the mouse button: left where it is meanwhile.
    dragging: Option<Who>,
    /// People whose video you've hidden (identities).
    hidden: HashSet<String>,
    /// Who the bubble that follows the speaker shows, and since when.
    stage: Option<String>,
    stage_at: Instant,
    /// A dragged bubble is near enough to ride again.
    magnet: bool,
    /// A face is pressed, here: a click if the mouse goes up, a drag if it
    /// moves.
    press: Option<Point<Pixels>>,
    /// The mic button was held to talk: letting go isn't a click.
    held_click: bool,
    /// The mic button is held to talk right now: the widget owes the call a
    /// mute when it's let go, or when the widget goes first.
    mic_held: bool,
    ghost: Option<(WindowHandle<Ghost>, platform::Native)>,
    ghost_opening: bool,
    /// Where the outline should be, and where it is.
    ghost_want: Option<Rect>,
    ghost_at: Option<Rect>,
    hover_task: Option<Task<()>>,
    /// Closes the open card once the cursor has left, or a click lands
    /// elsewhere.
    cursor_task: Option<Task<()>>,
    /// Follows the pill or a bubble while it's dragged.
    drag_task: Option<Task<()>>,
    hold_task: Option<Task<()>>,
    menu_task: Option<Task<()>>,
    /// Everyone in the call, as last drawn.
    watched: HashMap<EntityId, (Sig, Subscription)>,
    _subscriptions: Vec<Subscription>,
    _tick: Task<()>,
}

impl Capsule {
    fn new(
        call: Entity<Call>,
        size: Size<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Square and shadowless: the pieces round their own corners.
        platform::make_shape(window, platform::Mask::Rounded(0.0), cx);
        let transcript = call.read(cx).transcript.clone();
        let subscriptions = vec![
            cx.observe(&call, |this, _, cx| {
                this.watch(cx);
                cx.notify();
            }),
            // The caption follows what's said.
            cx.observe(&transcript, |this, _, cx| {
                if this.call.read(cx).transcription.is_some() {
                    cx.notify();
                }
            }),
            cx.observe(&Shell::global(cx), |_, _, cx| cx.notify()),
            // Dragged somewhere else: what rides on the pill may change sides.
            cx.observe_window_bounds(window, |_, _, cx| cx.notify()),
        ];
        // People go quiet without any event: look again every second.
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    let ticking = matches!(
                        this.open,
                        Some(Drawer::Name(_) | Drawer::Recording | Drawer::Menu)
                    );
                    if this.update_slots(cx) || ticking {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            call,
            native: platform::Native::of(window),
            slots: Vec::new(),
            open: None,
            column: false,
            tucked: None,
            home: (0.0, 0.0),
            size,
            anchor: (size.width, size.height),
            shown: (size.width, size.height),
            jump: None,
            moving: None,
            card_first: false,
            dock_up: true,
            side_right: true,
            bubbles: HashMap::new(),
            opening: HashSet::new(),
            geo: HashMap::new(),
            dragging: None,
            hidden: HashSet::new(),
            stage: None,
            stage_at: Instant::now(),
            magnet: false,
            press: None,
            held_click: false,
            mic_held: false,
            ghost: None,
            ghost_opening: false,
            ghost_want: None,
            ghost_at: None,
            hover_task: None,
            cursor_task: None,
            drag_task: None,
            hold_task: None,
            menu_task: None,
            watched: HashMap::new(),
            _subscriptions: subscriptions,
            _tick: tick,
        };
        this.watch(cx);
        // The bubbles and the outline go with the widget. So does a mic held
        // to talk: the task that would mute it on release dies with the
        // widget, and nothing else would.
        cx.on_release(|this, cx| {
            if this.mic_held {
                this.call.update(cx, |c, cx| c.hold_to_talk(false, cx));
            }
            let ghost = this.ghost.take().map(|(handle, _)| handle);
            let bubbles: Vec<Slot> = this.bubbles.drain().map(|(_, slot)| slot).collect();
            cx.spawn(async move |cx| {
                for slot in bubbles {
                    platform::retire(&slot.native);
                    let _ = slot
                        .handle
                        .update(cx, |_, window, _| window.remove_window());
                }
                if let Some(ghost) = ghost {
                    let _ = ghost.update(cx, |_, window, _| window.remove_window());
                }
            })
            .detach();
        })
        .detach();
        this
    }

    /// Repaint when what the pill shows of anyone changes: who speaks, who's
    /// muted, whose camera is on. Not with every frame of video: a bubble
    /// draws those itself.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let members: Vec<Entity<Member>> = self.call.read(cx).members().cloned().collect();
        let live: Vec<EntityId> = members.iter().map(|m| m.entity_id()).collect();
        for member in members {
            let id = member.entity_id();
            if self.watched.contains_key(&id) {
                continue;
            }
            let sig = Sig::of(member.read(cx));
            let subscription = cx.observe(&member, |this, member, cx| {
                let sig = Sig::of(member.read(cx));
                if let Some((old, _)) = this.watched.get_mut(&member.entity_id())
                    && *old != sig
                {
                    *old = sig;
                    cx.notify();
                }
            });
            self.watched.insert(id, (sig, subscription));
        }
        self.watched.retain(|id, _| live.contains(id));
    }

    /// One bubble follows the speaker, instead of a bubble each: in a call
    /// of four or more, unless Settings says otherwise.
    fn follows(&self, cx: &App) -> bool {
        self.call.read(cx).others.len() > SMALL
            && !Shell::global(cx).read(cx).store.call.bubble_each
    }

    /// Keep the row to the people active last, and the bubble that follows
    /// the speaker on the speaker. Returns whether anything changed.
    fn update_slots(&mut self, cx: &App) -> bool {
        let call = self.call.read(cx);
        let sharers = call.sharers(cx);
        let others = call.others.clone();
        let before = self.slots.clone();
        let sharing = |m: &Member| sharers.contains(&m.identity);
        let small = others.len() <= SMALL;
        let active = |m: &Member| small || sharing(m) || m.is_active(QUIET);
        self.slots
            .retain(|e| others.contains(e) && active(e.read(cx)));
        let mut candidates: Vec<Entity<Member>> = others
            .iter()
            .filter(|e| !self.slots.contains(e) && active(e.read(cx)))
            .cloned()
            .collect();
        // Sharers first, then whoever was active most recently.
        candidates.sort_by_key(|e| {
            let m = e.read(cx);
            (!sharing(m), !m.speaking, Reverse(m.last_active))
        });
        for candidate in candidates {
            self.take(candidate, &sharers, cx);
        }
        let stage = self.update_stage(&others, cx);
        self.slots != before || stage
    }

    /// Put `member` in the row: in a free place, or in place of whoever has
    /// been quiet longest if `member` was active since.
    fn take(&mut self, member: Entity<Member>, sharers: &[String], cx: &App) {
        if self.slots.len() < ROW_MAX {
            self.slots.push(member);
            return;
        }
        let m = member.read(cx);
        let sharer = sharers.contains(&m.identity);
        let oldest = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| !sharers.contains(&s.read(cx).identity))
            .min_by_key(|(_, s)| {
                let s = s.read(cx);
                (s.speaking, s.last_active)
            })
            .map(|(ix, _)| ix);
        if let Some(ix) = oldest {
            let o = self.slots[ix].read(cx);
            let newer =
                (m.speaking && !o.speaking) || (!o.speaking && m.last_active > o.last_active);
            if sharer || newer {
                self.slots[ix] = member;
            }
        }
    }

    /// Who the one bubble shows: whoever speaks with their camera on, once
    /// the last one has had [`STAGE_HOLD`]; they stay until someone else
    /// does. Returns whether it changed.
    fn update_stage(&mut self, others: &[Entity<Member>], cx: &App) -> bool {
        let before = self.stage.clone();
        if !self.follows(cx) {
            self.stage = None;
            return before.is_some();
        }
        let shown = |m: &Member| m.video.is_some() && !self.hidden.contains(&m.identity);
        let people: Vec<&Member> = others
            .iter()
            .map(|e| e.read(cx))
            .filter(|m| shown(m))
            .collect();
        let current = before
            .as_ref()
            .is_some_and(|id| people.iter().any(|m| m.identity == *id));
        let speaker = people
            .iter()
            .filter(|m| m.speaking && !m.mic_muted)
            .max_by_key(|m| m.last_active)
            .map(|m| m.identity.clone());
        let next = match speaker {
            Some(speaker) if !current || self.stage_at.elapsed() >= STAGE_HOLD => Some(speaker),
            _ if current => before.clone(),
            _ => people
                .iter()
                .max_by_key(|m| m.last_active)
                .map(|m| m.identity.clone()),
        };
        if next != before {
            self.stage = next;
            self.stage_at = Instant::now();
            return true;
        }
        false
    }

    fn plan(&self, cx: &App) -> Plan {
        let call = self.call.read(cx);
        let me = call.me.clone();
        let mut faces = self.slots.clone();
        faces.push(me.clone());
        let mut stack: Vec<Entity<Member>> = call
            .others
            .iter()
            .filter(|e| !self.slots.contains(e))
            .cloned()
            .collect();
        stack.sort_by_key(|e| Reverse(e.read(cx).last_active));
        let preview = preview_aspect(me.read(cx));
        let mut bubbles = Vec::new();
        if preview.is_some() {
            bubbles.push(Who::Share);
        }
        if self.follows(cx) {
            if self.stage.is_some() {
                bubbles.push(Who::Stage);
            }
        } else {
            for slot in &self.slots {
                let m = slot.read(cx);
                if m.video.is_some() && !self.hidden.contains(&m.identity) {
                    bubbles.push(Who::Member(m.identity.clone()));
                }
            }
        }
        if me.read(cx).video.is_some() {
            bubbles.push(Who::Me);
        }
        let settings = &Shell::global(cx).read(cx).store.call;
        let caption = (call.transcription.is_some() && settings.caption_line).then(|| {
            call.transcript.read(cx).lines().last().map(|said| {
                let who = if said.mine {
                    "You".to_owned()
                } else {
                    first_name(&said.name).to_owned()
                };
                (who, said.text.clone())
            })
        });
        Plan {
            faces,
            seat: call.others.is_empty(),
            stack,
            sharers: call.sharers(cx),
            preview,
            bubbles,
            caption,
            recording: call.recording.is_some(),
            captions: call.transcription.is_some() || call.preparing.is_some(),
            // The same items `menu` shows: Record only where recording is.
            menu_extras: usize::from(crate::call::RECORDS_HERE && call.recording.is_none())
                + usize::from(call.can_transcribe()),
            notice_button: call.notice().is_some_and(|n| n.action.is_some()),
        }
    }

    /// The card showing: the one opened by a click or by hovering a face,
    /// else a question, else a notice. None while tucked away.
    fn drawer(&self, cx: &App) -> Option<Drawer> {
        if self.tucked.is_some() {
            return None;
        }
        let c = self.call.read(cx);
        self.open
            .clone()
            .or_else(|| c.prompt.is_some().then_some(Drawer::Prompt))
            .or_else(|| c.notice().is_some().then_some(Drawer::Notice))
    }

    /// Whose camera a bubble shows.
    pub fn bubble_member(&self, who: &Who, cx: &App) -> Option<Entity<Member>> {
        let call = self.call.read(cx);
        let identity = match who {
            Who::Me => return Some(call.me.clone()),
            Who::Share => return None,
            Who::Member(identity) => identity,
            Who::Stage => self.stage.as_ref()?,
        };
        call.others
            .iter()
            .find(|e| e.read(cx).identity == *identity)
            .cloned()
    }

    /// The pill's bottom-right corner on screen, and the usable part of its
    /// screen.
    fn corner(&self) -> Option<((f64, f64), Rect)> {
        let native = self.native.as_ref()?;
        let frame = native.frame();
        let here = (
            frame.x + self.shown.0 as f64,
            frame.top() - self.shown.1 as f64,
        );
        Some((
            self.moving.unwrap_or(here),
            native.visible().unwrap_or(frame),
        ))
    }

    /// The pill's outline on screen.
    fn pill_rect(&self, plan: &Plan, corner: (f64, f64)) -> Rect {
        let len = pill_len(plan, self.column) as f64;
        let (w, h) = if self.column {
            (MAIN as f64, len)
        } else {
            (len, MAIN as f64)
        };
        Rect {
            x: corner.0 - w,
            y: corner.1,
            w,
            h,
        }
    }

    /// Size the window for what's on show, keeping the pill in place, and
    /// put the bubbles where they go.
    fn fit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Plan {
        let plan = self.plan(cx);
        self.sync_bubbles(&plan, window, cx);
        if let Some(jump) = self.jump.take() {
            self.moving = Some(jump);
        }
        let jumping = self.moving.is_some();
        let Some((corner, visible)) = self.corner() else {
            return plan;
        };
        let len = pill_len(&plan, self.column);
        let (wanted, anchor, target) = if let Some(edge) = self.tucked {
            // A tab at the edge, by where the pill was.
            let tab = tab_rect(edge, self.home, len as f64, plan.faces.len(), visible);
            let s = (tab.w as f32, tab.h as f32);
            (size(s.0, s.1), s, Some((tab.right(), tab.y)))
        } else if self.column {
            let (wanted, anchor) = self.fit_column(&plan, len, corner, visible, cx);
            (wanted, anchor, jumping.then_some(corner))
        } else {
            let (wanted, anchor) = self.fit_lying(&plan, len, corner, visible, cx);
            (wanted, anchor, jumping.then_some(corner))
        };
        let after = |this: &mut Self, cx: &mut Context<Self>| {
            let plan = this.plan(cx);
            this.placements(&plan)
        };
        if let Some(target) = target {
            let here = self.native.as_ref().map(|n| n.frame());
            let there = Rect {
                x: target.0 - anchor.0 as f64,
                y: target.1 + anchor.1 as f64 - wanted.height as f64,
                w: wanted.width as f64,
                h: wanted.height as f64,
            };
            let arrived = here.is_some_and(|h| {
                (h.x - there.x).abs() < 1.0
                    && (h.y - there.y).abs() < 1.0
                    && (h.w - there.w).abs() < 1.0
                    && (h.h - there.h).abs() < 1.0
            });
            if !arrived || jumping {
                platform::move_with(
                    window,
                    wanted,
                    anchor,
                    target,
                    cx,
                    move |this, cx| {
                        this.shown = anchor;
                        this.moving = None;
                        cx.notify();
                    },
                    after,
                );
                self.size = wanted;
                self.anchor = anchor;
            }
        } else if wanted != self.size || anchor != self.anchor {
            platform::resize_with(
                window,
                wanted,
                platform::Mask::Rounded(0.0),
                platform::Anchor::Point {
                    before: self.anchor,
                    after: anchor,
                },
                cx,
                move |this, cx| {
                    this.shown = anchor;
                    cx.notify();
                },
                after,
            );
            self.size = wanted;
            self.anchor = anchor;
        }
        platform::place(self.placements(&plan), cx);
        plan
    }

    /// The window around a pill lying down: the caption on the side the
    /// bubbles ride on (over it, if the screen has room), and a card on the
    /// other side, so nothing has to move for it. A card by a pill alone
    /// opens over it. With no room on the free side, the card opens over
    /// the bubbles.
    fn fit_lying(
        &mut self,
        plan: &Plan,
        len: f32,
        corner: (f64, f64),
        visible: Rect,
        cx: &App,
    ) -> (Size<f32>, (f32, f32)) {
        let riding = self.riding(plan).map(|(.., h, _)| h).fold(0.0f32, f32::max);
        let caption = if plan.caption.is_some() {
            CAPTION_H + FLOAT_GAP
        } else {
            0.0
        };
        let card = self
            .drawer(cx)
            .map_or(0.0, |d| card_height(&d, plan) + FLOAT_GAP);
        let room_up = (visible.top() - corner.1) as f32 - MAIN;
        let room_down = (corner.1 - visible.y) as f32;
        let floats = caption
            + if riding > 0.0 {
                riding + FLOAT_GAP
            } else {
                0.0
            };
        self.dock_up = room_up >= floats || room_down < floats;
        self.card_first = card > 0.0
            && if floats == 0.0 {
                room_up >= card + 4.0
            } else if self.dock_up {
                room_down < card + 12.0
            } else {
                room_up >= card + 4.0
            };
        let (mut above, mut below) = (0.0, 0.0);
        *if self.dock_up { &mut above } else { &mut below } += caption;
        *if self.card_first {
            &mut above
        } else {
            &mut below
        } += card;
        (size(len, above + MAIN + below), (len, above + MAIN))
    }

    /// The window around a column: the caption and a card beside it, at its
    /// foot, on the side with the screen.
    fn fit_column(
        &mut self,
        plan: &Plan,
        len: f32,
        corner: (f64, f64),
        visible: Rect,
        cx: &App,
    ) -> (Size<f32>, (f32, f32)) {
        self.side_right = corner.0 - (MAIN as f64) / 2.0 < visible.x + visible.w / 2.0;
        self.dock_up = true;
        self.card_first = true;
        let drawer = self.drawer(cx);
        let caption = plan.caption.is_some();
        let mut side_h = drawer.as_ref().map_or(0.0, |d| card_height(d, plan));
        if caption {
            side_h += CAPTION_H + if side_h > 0.0 { FLOAT_GAP } else { 0.0 };
        }
        let mut side_w = drawer
            .as_ref()
            .map_or(0.0, |d| card_width(d).unwrap_or(SIDE_W));
        if caption {
            side_w = side_w.max(SIDE_W);
        }
        let w = MAIN
            + if side_w > 0.0 {
                FLOAT_GAP + side_w
            } else {
                0.0
            };
        let h = len.max(side_h);
        let anchor = if self.side_right { (MAIN, h) } else { (w, h) };
        (size(w, h), anchor)
    }

    /// The bubbles riding on the pill, in order: whose, its window, its
    /// width, height and corner radius.
    fn riding<'a>(
        &'a self,
        plan: &'a Plan,
    ) -> impl Iterator<Item = (&'a Who, &'a Slot, f32, f32, f32)> + 'a {
        let aspect = plan.preview.unwrap_or(1.6);
        plan.bubbles.iter().filter_map(move |who| {
            let slot = self.bubbles.get(who)?;
            let geo = self.geo.get(who)?;
            let (w, r) = bubble::shape(who, geo.size, aspect);
            // Fit the attached preview to the horizontal pill, including
            // changes to its people and controls. Keep column bubbles at
            // their usual size rather than squeezing a share to MAIN wide.
            let (w, h) = if *who == Who::Share && !self.column {
                let w = pill_len(plan, false);
                (w, (w / aspect).round())
            } else {
                (w, geo.size.round())
            };
            geo.docked.then_some((who, slot, w, h, r))
        })
    }

    /// Where every bubble goes, for those not there already. The ones riding
    /// on the pill: in a row over it (or under it) ending at its right end,
    /// past the caption; by a column, in a column beside it, from its foot
    /// up. The ones floating by themselves stay where they were put, at
    /// their size.
    fn placements(&self, plan: &Plan) -> Vec<platform::Placement> {
        let Some((corner, visible)) = self.corner() else {
            return Vec::new();
        };
        let parent = self.native.clone();
        let mut out = Vec::new();
        let mut put = |slot: &Slot, frame: Rect, radius: f32, rides: bool, hidden: bool| {
            let native = &slot.native;
            // Whole points: a display may round a frame, and a window that's
            // never quite where it was put would be put there every time.
            let frame = Rect {
                x: frame.x.round(),
                y: frame.y.round(),
                ..frame
            };
            let there = if hidden {
                !native.shown()
            } else {
                native.shown()
                    && native.frame() == frame
                    && parent.as_ref().is_some_and(|p| native.rides_on(p)) == rides
            };
            if !there {
                out.push(platform::Placement {
                    window: native.clone(),
                    frame,
                    radius: Some(radius as f64),
                    parent: parent.clone().filter(|_| rides),
                    hidden,
                });
            }
        };
        let caption = if plan.caption.is_some() {
            (CAPTION_H + FLOAT_GAP) as f64
        } else {
            0.0
        };
        let (gap, float_gap, main) = (GAP as f64, FLOAT_GAP as f64, MAIN as f64);
        // A bubble under the mouse button keeps its place in the row until
        // it has moved away: the others don't slide under it on a press.
        let riding: Vec<_> = self.riding(plan).collect();
        let held = |who: &Who| self.dragging.as_ref() == Some(who);
        if self.tucked.is_some() {
            // What rode on the pill goes with it.
            for (_, slot, ..) in &riding {
                put(slot, Rect::default(), 0.0, false, true);
            }
        } else if self.column {
            let total: f64 = riding.iter().map(|b| b.3 as f64).sum::<f64>()
                + gap * riding.len().saturating_sub(1) as f64;
            let mut y = (corner.1 + caption)
                .min(visible.top() - total)
                .max(visible.y);
            for (who, slot, w, h, r) in riding.iter().rev() {
                let (w, h) = (*w as f64, *h as f64);
                let x = if self.side_right {
                    corner.0 + float_gap
                } else {
                    corner.0 - main - float_gap - w
                };
                if !held(who) {
                    put(slot, Rect { x, y, w, h }, *r, true, false);
                }
                y += h + gap;
            }
        } else {
            let total: f64 = riding.iter().map(|b| b.2 as f64).sum::<f64>()
                + gap * riding.len().saturating_sub(1) as f64;
            let most = (visible.right() - MARGIN - total).max(visible.x + MARGIN);
            let mut x = (corner.0 - total).clamp(visible.x + MARGIN, most);
            for (who, slot, w, h, r) in &riding {
                let (w, h) = (*w as f64, *h as f64);
                let y = if self.dock_up {
                    corner.1 + main + float_gap + caption
                } else {
                    corner.1 - float_gap - caption - h
                };
                if !held(who) {
                    put(slot, Rect { x, y, w, h }, *r, true, false);
                }
                x += w + gap;
            }
        }
        // By themselves: where they are, at their size, around their middle,
        // and on their screen.
        let aspect = plan.preview.unwrap_or(1.6);
        for who in &plan.bubbles {
            let (Some(slot), Some(geo)) = (self.bubbles.get(who), self.geo.get(who)) else {
                continue;
            };
            if geo.docked || self.dragging.as_ref() == Some(who) {
                continue;
            }
            let (w, r) = bubble::shape(who, geo.size, aspect);
            let (w, h) = (w as f64, geo.size.round() as f64);
            let old = slot.native.frame();
            // On the screen it's on, which needn't be the widget's.
            let v = slot.native.visible().unwrap_or(visible);
            let frame = Rect {
                x: (old.x + (old.w - w) / 2.0)
                    .clamp(v.x + 4.0, (v.right() - 4.0 - w).max(v.x + 4.0)),
                y: (old.y + (old.h - h) / 2.0).clamp(v.y + 4.0, (v.top() - 4.0 - h).max(v.y + 4.0)),
                w,
                h,
            };
            put(slot, frame, r, false, false);
        }
        out
    }

    /// Open a window for every bubble wanted, close the rest, and ask for
    /// the video layer that fits each.
    fn sync_bubbles(&mut self, plan: &Plan, window: &mut Window, cx: &mut Context<Self>) {
        let gone: Vec<Who> = self
            .bubbles
            .keys()
            .filter(|who| !plan.bubbles.contains(who))
            .cloned()
            .collect();
        for who in gone {
            // Back on the pill when it returns: its new window has no place
            // of its own to float at.
            if let Some(geo) = self.geo.get_mut(&who) {
                geo.docked = true;
            }
            if let Some(slot) = self.bubbles.remove(&who) {
                // Not from inside this update: AppKit reorders windows.
                cx.spawn(async move |_, cx| {
                    platform::retire(&slot.native);
                    let _ = slot
                        .handle
                        .update(cx, |_, window, _| window.remove_window());
                })
                .detach();
            }
        }
        for who in &plan.bubbles {
            self.geo.entry(who.clone()).or_insert(Geo {
                size: who.first_size(),
                docked: true,
            });
            if self.bubbles.contains_key(who) || !self.opening.insert(who.clone()) {
                continue;
            }
            let (call, this, who) = (self.call.clone(), cx.weak_entity(), who.clone());
            cx.defer(move |cx| {
                let opened = bubble::open(call, this.clone(), who.clone(), cx);
                let handle = opened.as_ref().map(|(handle, _)| *handle);
                let kept = this.update(cx, |this, cx| {
                    this.opening.remove(&who);
                    if let Some((handle, native)) = opened {
                        this.bubbles.insert(who, Slot { handle, native });
                    }
                    cx.notify();
                });
                // The widget went while its bubble was opening: nobody
                // else would close it.
                if kept.is_err()
                    && let Some(handle) = handle
                {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            });
        }
        // The layer that fits: small bubbles don't decode 720p, and video
        // nobody sees comes in at its lowest.
        let scale = window.scale_factor();
        let call = self.call.clone();
        let members: Vec<Entity<Member>> = call.read(cx).members().cloned().collect();
        let mut heights: HashMap<EntityId, f32> = HashMap::new();
        for who in &plan.bubbles {
            if let (Some(member), Some(geo)) = (self.bubble_member(who, cx), self.geo.get(who)) {
                heights.insert(member.entity_id(), geo.size * scale);
            }
        }
        call.update(cx, |call, cx| {
            for member in &members {
                let height = heights.get(&member.entity_id()).copied().unwrap_or(0.0);
                call.fit_video(member, height, cx);
            }
        });
    }

    // ---- bubbles: what's done to them --------------------------------

    /// A bubble went under the mouse button: follow it until it's let go.
    /// Moved away, it floats by itself; dropped by the pill, it rides
    /// again; dropped by the screen's edge, it lines up with it.
    pub fn bubble_pressed(&mut self, who: &Who, cx: &mut Context<Self>) {
        let Some(native) = self.bubbles.get(who).map(|s| s.native.clone()) else {
            return;
        };
        let start = native.frame();
        let who = who.clone();
        self.end_drag(cx);
        self.dragging = Some(who.clone());
        self.drag_task = Some(cx.spawn(async move |this, cx| {
            let mut moved = false;
            loop {
                cx.background_executor().timer(DRAG_POLL).await;
                let done = this.update(cx, |this, cx| {
                    // Closed meanwhile (their camera went off): nothing left
                    // to follow, and nothing to put anywhere.
                    if this.bubbles.get(&who).is_none_or(|s| s.native != native) {
                        this.dragging = None;
                        this.magnet = false;
                        cx.notify();
                        return true;
                    }
                    let frame = native.frame();
                    moved |= (frame.x - start.x).abs() > 3.0 || (frame.y - start.y).abs() > 3.0;
                    let near = moved && this.tucked.is_none() && this.near_pill(&frame, cx);
                    if platform::mouse_down() {
                        if this.magnet != near {
                            this.magnet = near;
                            cx.notify();
                        }
                        return false;
                    }
                    this.dragging = None;
                    this.magnet = false;
                    if moved {
                        if let Some(geo) = this.geo.get_mut(&who) {
                            if who == Who::Share && geo.docked && !near {
                                // Detach at the size it had on the pill.
                                geo.size = frame.h as f32;
                            }
                            geo.docked = near;
                        }
                        if !near {
                            this.snap(&native, frame, cx);
                        }
                    }
                    cx.notify();
                    true
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
        }));
    }

    /// Whether a bubble at `frame` is near enough to the pill to ride on it.
    fn near_pill(&self, frame: &Rect, cx: &App) -> bool {
        let Some((corner, _)) = self.corner() else {
            return false;
        };
        let plan = self.plan(cx);
        let caption = if plan.caption.is_some() {
            (CAPTION_H + FLOAT_GAP) as f64
        } else {
            0.0
        };
        frame.distance(&self.pill_rect(&plan, corner)) < MAGNET + caption
    }

    /// A bubble dropped by the screen's edge lines up with it.
    fn snap(&self, native: &platform::Native, frame: Rect, cx: &App) {
        let Some(visible) = native.visible() else {
            return;
        };
        let m = MARGIN + 2.0;
        let mut to = frame;
        if frame.x < visible.x + m + SNAP {
            to.x = visible.x + m;
        } else if frame.right() > visible.right() - m - SNAP {
            to.x = visible.right() - m - frame.w;
        }
        if frame.y < visible.y + m + SNAP {
            to.y = visible.y + m;
        } else if frame.top() > visible.top() - m - SNAP {
            to.y = visible.top() - m - frame.h;
        }
        if to != frame {
            platform::place(
                vec![platform::Placement {
                    window: native.clone(),
                    frame: to,
                    radius: None,
                    parent: None,
                    hidden: false,
                }],
                cx,
            );
        }
    }

    /// Scrolled or pinched over: `by` times as big.
    pub fn bubble_zoomed(&mut self, who: &Who, by: f32, cx: &mut Context<Self>) {
        if let Some(geo) = self.geo.get_mut(who) {
            let size = who.clamp(geo.size * by);
            if size != geo.size {
                geo.size = size;
                cx.notify();
            }
        }
    }

    /// Double-clicked: big, or small again.
    pub fn bubble_flipped(&mut self, who: &Who, cx: &mut Context<Self>) {
        if let Some(geo) = self.geo.get_mut(who) {
            geo.size = who.other_size(geo.size);
            cx.notify();
        }
    }

    /// Its button was pressed: your camera off, or their video out of sight
    /// until you click their face.
    pub fn bubble_dismissed(&mut self, who: &Who, cx: &mut Context<Self>) {
        match who {
            Who::Me => self.call.update(cx, |c, cx| c.toggle_camera(cx)),
            Who::Share => self.call.update(cx, |c, cx| c.toggle_share(cx)),
            Who::Member(identity) => {
                self.hidden.insert(identity.clone());
            }
            Who::Stage => {
                if let Some(identity) = self.stage.take() {
                    self.hidden.insert(identity);
                }
            }
        }
        self.update_slots(cx);
        cx.notify();
    }

    /// A face was clicked: that person's video switch.
    fn face_clicked(&mut self, member: &Entity<Member>, cx: &mut Context<Self>) {
        let m = member.read(cx);
        if m.is_local {
            self.call.update(cx, |c, cx| c.toggle_camera(cx));
            return;
        }
        if m.video.is_none() {
            return;
        }
        let identity = m.identity.clone();
        let on_stage = self.stage.as_ref() == Some(&identity);
        if self.hidden.remove(&identity) || (self.follows(cx) && !on_stage) {
            // Back in sight; with one bubble, it shows them now.
            if self.follows(cx) {
                self.stage = Some(identity);
                self.stage_at = Instant::now();
            }
        } else {
            self.hidden.insert(identity);
        }
        self.update_slots(cx);
        cx.notify();
    }

    // ---- the pill: dragging, turning, tucking -------------------------

    /// The pill went under the mouse button: macOS moves the window; follow
    /// the cursor until the button goes up. Still over an edge for
    /// [`PAUSE`], the outline of what a drop makes shows; dropped there,
    /// the pill turns or tucks away.
    fn begin_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::platform::start_window_move(window);
        self.end_drag(cx);
        let start = platform::cursor();
        self.drag_task = Some(cx.spawn(async move |this, cx| {
            let (mut moved, mut last, mut still) = (false, start, Duration::ZERO);
            let mut drag = Drag::new(start);
            loop {
                cx.background_executor().timer(DRAG_POLL).await;
                let done = this.update(cx, |this, cx| {
                    let p = platform::cursor();
                    drag = drag.through(p);
                    moved |= (p.0 - start.0).hypot(p.1 - start.1) > 4.0;
                    let zone = moved.then(|| this.zone_at(p, Some(&drag), cx)).flatten();
                    if !platform::mouse_down() {
                        this.show_ghost(None, cx);
                        if let Some(zone) = zone {
                            this.drop_in(zone, cx);
                        }
                        cx.notify();
                        return true;
                    }
                    if p != last {
                        last = p;
                        still = Duration::ZERO;
                        this.show_ghost(None, cx);
                    } else {
                        still += DRAG_POLL;
                        if still >= PAUSE {
                            this.show_ghost(zone, cx);
                        }
                    }
                    false
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
        }));
    }

    /// A new drag takes the place of one whose button came up less than a
    /// look ago: leave nothing of the old one behind.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.drag_task = None;
        if self.dragging.take().is_some() || std::mem::take(&mut self.magnet) {
            cx.notify();
        }
        self.show_ghost(None, cx);
    }

    /// What a drop with the cursor at `p` makes of the pill, if anything.
    /// `drag` is how the cursor got there; a script drops with none.
    fn zone_at(&self, p: (f64, f64), drag: Option<&Drag>, cx: &App) -> Option<Zone> {
        // The screen under the cursor, not the one most of the window is
        // still on: a pill on its way to another display belongs there.
        let (v, whole) = platform::screen_at(p).or_else(|| {
            let native = self.native.as_ref()?;
            Some((native.visible()?, native.screen()?))
        })?;
        // Where two displays meet is no edge: the cursor is only crossing.
        let open = |edge: Edge| {
            let past = match edge {
                Edge::Left => (whole.x - 20.0, p.1),
                Edge::Right => (whole.right() + 20.0, p.1),
                Edge::Bottom => (p.0, whole.y - 20.0),
                Edge::Top => (p.0, whole.top() + 20.0),
            };
            platform::screen_at(past).is_none()
        };
        let plan = self.plan(cx);
        let now = self.pill_rect(&plan, self.corner()?.0);
        let (edge, tuck) = edge_for(p, drag, &now, &v, &whole, open)?;
        let column = edge.side();
        let (main, len) = (MAIN as f64, pill_len(&plan, column) as f64);
        // A pill that already lies this way keeps its place along the edge;
        // one that turns comes out under the cursor.
        let here = (self.column == column && self.tucked.is_none())
            .then(|| self.corner())
            .flatten()
            .map(|(corner, _)| corner);
        let (corner, pill) = if column {
            let x = if edge == Edge::Left {
                v.x + MARGIN + main
            } else {
                v.right() - MARGIN
            };
            let highest = (v.top() - len - 2.0).max(v.y + MARGIN);
            let y = here
                .map_or(p.1 - len / 2.0, |c| c.1)
                .clamp(v.y + MARGIN, highest)
                .round();
            let pill = Rect {
                x: x - main,
                y,
                w: main,
                h: len,
            };
            ((x, y), pill)
        } else {
            let rightmost = (v.right() - MARGIN).max(v.x + len + MARGIN);
            let x = here
                .map_or(p.0 + len / 2.0, |c| c.0)
                .clamp(v.x + len + MARGIN, rightmost)
                .round();
            let y = if edge == Edge::Top {
                v.top() - 4.0 - main
            } else {
                v.y + 12.0
            };
            let pill = Rect {
                x: x - len,
                y,
                w: len,
                h: main,
            };
            ((x, y), pill)
        };
        if tuck {
            return Some(Zone {
                edge,
                tuck: true,
                column,
                corner,
                pill: tab_rect(edge, corner, len, plan.faces.len(), v),
            });
        }
        // Standing there already, or as good as.
        if here.is_some_and(|c| (c.0 - corner.0).abs() < 4.0 && (c.1 - corner.1).abs() < 4.0) {
            return None;
        }
        Some(Zone {
            edge,
            tuck: false,
            column,
            corner,
            pill,
        })
    }

    /// The pill was dropped in `zone`: turn it, or tuck it away.
    fn drop_in(&mut self, zone: Zone, cx: &mut Context<Self>) {
        tracing::debug!(?zone, "widget dropped at an edge");
        self.open = None;
        self.column = zone.column;
        self.tucked = zone.tuck.then_some(zone.edge);
        self.home = zone.corner;
        self.jump = Some(zone.corner);
        cx.notify();
    }

    /// The tab was clicked: the call comes back, by the edge it was in.
    fn untuck(&mut self, cx: &mut Context<Self>) {
        if self.tucked.take().is_some() {
            // The display it was tucked on may be gone: come back on the
            // one the tab is on now.
            let mut home = self.home;
            if let Some(v) = self.native.as_ref().and_then(|n| n.visible()) {
                let len = pill_len(&self.plan(cx), self.column) as f64;
                let (w, h) = if self.column {
                    (MAIN as f64, len)
                } else {
                    (len, MAIN as f64)
                };
                home.0 = home.0.clamp(v.x + w, v.right().max(v.x + w));
                home.1 = home.1.clamp(v.y, (v.top() - h).max(v.y));
            }
            self.jump = Some(home);
            cx.notify();
        }
    }

    /// Show the outline of what a drop would make, or take it away.
    fn show_ghost(&mut self, zone: Option<Zone>, cx: &mut Context<Self>) {
        self.ghost_want = zone.map(|z| z.pill);
        self.sync_ghost(cx);
    }

    fn sync_ghost(&mut self, cx: &mut Context<Self>) {
        let at = self.ghost_want;
        if at == self.ghost_at {
            return;
        }
        let Some((_, native)) = &self.ghost else {
            // Opened the first time it's needed.
            if at.is_some() && !std::mem::replace(&mut self.ghost_opening, true) {
                let this = cx.weak_entity();
                cx.defer(move |cx| {
                    let ghost = open_ghost(cx);
                    let handle = ghost.as_ref().map(|(handle, _)| *handle);
                    let kept = this.update(cx, |this, cx| {
                        this.ghost_opening = false;
                        this.ghost = ghost;
                        this.sync_ghost(cx);
                    });
                    if kept.is_err()
                        && let Some(handle) = handle
                    {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                });
            }
            return;
        };
        self.ghost_at = at;
        let frame = at.unwrap_or_default();
        platform::place(
            vec![platform::Placement {
                window: native.clone(),
                frame,
                radius: Some(ghost_radius(&frame)),
                parent: None,
                hidden: at.is_none(),
            }],
            cx,
        );
    }

    // ---- cards ---------------------------------------------------------

    fn set_open(&mut self, open: Option<Drawer>, window: &mut Window, cx: &mut Context<Self>) {
        self.hover_task = None;
        if self.open != open {
            self.open = open;
            cx.notify();
        }
        if self.open.is_some() && self.cursor_task.is_none() {
            self.watch_cursor(window, cx);
        }
    }

    /// Open `drawer`, or close it if it's the one open.
    fn toggle(&mut self, drawer: Drawer, window: &mut Window, cx: &mut Context<Self>) {
        let open = (self.open.as_ref() != Some(&drawer)).then_some(drawer);
        self.set_open(open, window, cx);
    }

    /// Open a name once the cursor has settled.
    fn open_after(&mut self, open: Drawer, window: &mut Window, cx: &mut Context<Self>) {
        self.hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update_in(cx, |this, window, cx| this.set_open(Some(open), window, cx));
        }));
    }

    /// Close what's open by itself: a name once the cursor has been outside
    /// the window for [`HOVER_DELAY`]; a card opened by a click on a click
    /// anywhere else, or after [`AWAY`] with the cursor elsewhere.
    fn watch_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut outside = Duration::ZERO;
            loop {
                cx.background_executor().timer(CURSOR_POLL).await;
                let done = this.update_in(cx, |this, window, cx| {
                    let Some(open) = &this.open else {
                        return true;
                    };
                    if platform::cursor_inside(window) == Some(false) {
                        outside += CURSOR_POLL;
                    } else {
                        outside = Duration::ZERO;
                    }
                    let close = if matches!(open, Drawer::Name(_)) {
                        outside >= HOVER_DELAY
                    } else {
                        outside >= AWAY || (outside > Duration::ZERO && platform::mouse_down())
                    };
                    if close {
                        this.hover_task = None;
                        this.open = None;
                        cx.notify();
                    }
                    close
                });
                if done.unwrap_or(true) {
                    let _ = this.update(cx, |this, _| this.cursor_task = None);
                    break;
                }
            }
        }));
    }

    // ---- the mic button: a click, or held to talk ----------------------

    /// The mic button went down while you're muted: held past
    /// [`HOLD_AFTER`], you talk until it's let go.
    fn mic_pressed(&mut self, cx: &mut Context<Self>) {
        // A new press starts clean, whatever the last one left behind.
        self.held_click = false;
        let hold = Shell::global(cx).read(cx).store.call.hold_to_talk;
        if !hold || self.call.read(cx).mic_on {
            return;
        }
        self.hold_task = Some(cx.spawn(async move |this, cx| {
            // The same press all the way: a button that came up meanwhile,
            // even to go down again somewhere else, was a click.
            let mut waited = Duration::ZERO;
            while waited < HOLD_AFTER {
                cx.background_executor().timer(DRAG_POLL).await;
                waited += DRAG_POLL;
                if !platform::left_mouse_down() {
                    return;
                }
            }
            let held = this.update(cx, |this, cx| {
                // Already talking (the key holds it): the button adds
                // nothing, and its release mustn't mute the key's hold.
                if this.call.read(cx).held {
                    return false;
                }
                this.held_click = true;
                this.mic_held = true;
                this.call.update(cx, |c, cx| c.hold_to_talk(true, cx));
                true
            });
            if !held.unwrap_or(false) {
                return;
            }
            // Until the button comes up, or Settings turns hold to talk off.
            while platform::left_mouse_down()
                && this
                    .update(cx, |_, cx| {
                        Shell::global(cx).read(cx).store.call.hold_to_talk
                    })
                    .unwrap_or(false)
            {
                cx.background_executor().timer(DRAG_POLL).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.mic_held = false;
                this.call.update(cx, |c, cx| c.hold_to_talk(false, cx));
            });
            // The click that ends the hold has come by now, if it will.
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |this, _| this.held_click = false);
        }));
    }

    fn mic_clicked(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.held_click) {
            return;
        }
        self.hold_task = None;
        self.call.update(cx, |c, cx| c.toggle_mic(cx));
    }

    // ---- right-click: the menu as macOS draws it -----------------------

    fn context_menu(&mut self, cx: &mut Context<Self>) {
        let c = self.call.read(cx);
        let items = vec![
            Some((if c.mic_on { "Mute" } else { "Unmute" }.to_owned(), false)),
            Some(("Camera".to_owned(), c.camera != Camera::Off)),
            Some(("Share Screen".to_owned(), c.sharing.is_some())),
            None,
            Some(("Copy Invite Link".to_owned(), false)),
            Some(("Open Call Window".to_owned(), false)),
            None,
            Some(("Leave Call".to_owned(), false)),
        ];
        let (tx, rx) = async_channel::bounded(1);
        platform::popup_menu(items, tx);
        self.menu_task = Some(cx.spawn(async move |this, cx| {
            let Ok(picked) = rx.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.call.update(cx, |c, cx| match picked {
                    0 => c.toggle_mic(cx),
                    1 => c.toggle_camera(cx),
                    2 => c.toggle_share(cx),
                    4 => c.copy_link(cx),
                    5 => c.set_mode(Mode::Regular, cx),
                    7 => c.leave(cx),
                    _ => {}
                })
            });
        }));
    }

    /// Reach a state as if by hand (snapshot tests): a card (`controls`,
    /// `people`…, `none` to close it), the pill at an edge (`column-left`,
    /// `column-right`, `lying`, `tuck-left`, `tuck-bottom`, `untuck`), or a
    /// bubble (`hide` and `show` the first camera, `big` for yours, `float`
    /// to take yours off the pill).
    #[cfg(feature = "snapshot")]
    pub fn show(&mut self, which: &str, cx: &mut Context<Self>) {
        // A point this far in from an edge, as a part of the screen's size.
        let edge_point = |this: &Self, edge: Edge, part: f64| {
            let v = this.native.as_ref()?.visible()?;
            Some(match edge {
                Edge::Left => (v.x + v.w * part, v.y + v.h / 2.0),
                Edge::Right => (v.right() - v.w * part, v.y + v.h / 2.0),
                Edge::Top => (v.x + v.w / 2.0, v.top() - v.h * part),
                Edge::Bottom => (v.x + v.w / 2.0, v.y + v.h * part),
            })
        };
        let drop_at = |this: &mut Self, edge: Edge, part: f64, cx: &mut Context<Self>| {
            let zone = edge_point(this, edge, part).and_then(|p| this.zone_at(p, None, cx));
            if let Some(zone) = zone {
                this.drop_in(zone, cx);
            } else {
                tracing::warn!(?edge, "script: nothing to do at that edge");
            }
        };
        let (turn, tuck) = (TURN * 0.75, TUCK * 0.2);
        let first_camera = |this: &Self, cx: &App| {
            let call = this.call.read(cx);
            call.others
                .iter()
                .find(|e| e.read(cx).video.is_some())
                .cloned()
        };
        // Not through set_open: the cursor is elsewhere in tests.
        match which {
            "column-left" => return drop_at(self, Edge::Left, turn, cx),
            "column-right" => return drop_at(self, Edge::Right, turn, cx),
            "lying" => return drop_at(self, Edge::Bottom, turn, cx),
            "lying-top" => return drop_at(self, Edge::Top, turn, cx),
            "tuck-left" => return drop_at(self, Edge::Left, tuck, cx),
            "tuck-bottom" => return drop_at(self, Edge::Bottom, tuck, cx),
            "untuck" => return self.untuck(cx),
            "hide" | "unhide" => {
                if let Some(member) = first_camera(self, cx) {
                    self.face_clicked(&member, cx);
                }
                return;
            }
            "big" => return self.bubble_flipped(&Who::Me, cx),
            "float" => {
                if let Some(geo) = self.geo.get_mut(&Who::Me) {
                    geo.docked = !geo.docked;
                }
                if let Some(slot) = self.bubbles.get(&Who::Me) {
                    let mut frame = slot.native.frame();
                    frame.x -= 200.0;
                    frame.y += 120.0;
                    platform::place(
                        vec![platform::Placement {
                            window: slot.native.clone(),
                            frame,
                            radius: None,
                            parent: None,
                            hidden: false,
                        }],
                        cx,
                    );
                }
                cx.notify();
                return;
            }
            "ghost" => {
                let zone =
                    edge_point(self, Edge::Left, turn).and_then(|p| self.zone_at(p, None, cx));
                self.show_ghost(zone, cx);
                return;
            }
            "unghost" => return self.show_ghost(None, cx),
            "context" => return self.context_menu(cx),
            "uncontext" => return platform::cancel_menu(),
            _ => {}
        }
        self.open = match which {
            "hover-name" => Some(Drawer::Name(
                self.call.read(cx).me.read(cx).identity.clone(),
            )),
            "people" => Some(Drawer::People),
            "recording" => Some(Drawer::Recording),
            "record-choice" => Some(Drawer::RecordChoice),
            "transcribe-choice" => Some(Drawer::TranscribeChoice),
            "transcript" => Some(Drawer::Transcript),
            "prompt" => Some(Drawer::Prompt),
            "none" => None,
            _ => Some(Drawer::Menu),
        };
        cx.notify();
    }
}

/// The outline of what a drop would make of the pill: a window of its own,
/// so it can show anywhere on screen.
pub struct Ghost;

impl Render for Ghost {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let s = window.bounds().size;
        let radius = ghost_radius(&Rect {
            w: (s.width / px(1.0)) as f64,
            h: (s.height / px(1.0)) as f64,
            ..Rect::default()
        });
        div()
            .size_full()
            .rounded(px(radius as f32))
            .bg(gpui::rgba(0x4f8cff2e))
            .border_2()
            .border_dashed()
            .border_color(rgb(ACCENT))
    }
}

fn open_ghost(cx: &mut App) -> Option<(WindowHandle<Ghost>, platform::Native)> {
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(MAIN), px(MAIN)));
    let mut native = None;
    let handle = cx
        .open_window(
            platform::hidden_floating_options(WindowBounds::Windowed(bounds)),
            |window, cx| {
                native = platform::Native::of(window);
                cx.new(|_| Ghost)
            },
        )
        .inspect_err(|e| tracing::warn!("open outline: {e:#}"))
        .ok()?;
    Some((handle, native?))
}

/// The outline's corners: a tab's for something thin, the pill's otherwise.
fn ghost_radius(frame: &Rect) -> f64 {
    if frame.w.min(frame.h) <= TAB as f64 + 0.5 {
        8.0
    } else {
        RADIUS as f64
    }
}

/// The edge a drop with the cursor at `p` is for, and whether it's deep
/// enough to tuck there. `visible` is the screen without its menu bar and
/// Dock, `whole` all of it; `pill` is the pill's outline where it is now.
///
/// A pill dragged from beside an edge it lies along (its home) is still
/// inside that edge's band, and by a corner inside the next one's too. So it
/// stays with its home: pushed [`PUSH`] into it, the drop is for that edge;
/// slid along it, for none, unless the pill itself was pushed out over the
/// edge at the end of the slide. Come away from its home, the pill is any
/// other edge's whose band the cursor is in, the one it's deeper into (as a
/// share of the band) if there are two, once the cursor has come [`PUSH`]
/// back towards that edge from the furthest it was from it.
fn edge_for(
    p: (f64, f64),
    drag: Option<&Drag>,
    pill: &Rect,
    visible: &Rect,
    whole: &Rect,
    open: impl Fn(Edge) -> bool,
) -> Option<(Edge, bool)> {
    let v = visible;
    let column = pill.h > pill.w;
    // How far a point is from an edge, and that as a share of the edge's
    // band: 0 at the edge, 1 where the band ends.
    let from = |q: (f64, f64), edge: Edge| match edge {
        Edge::Left => q.0 - v.x,
        Edge::Right => v.right() - q.0,
        Edge::Bottom => q.1 - v.y,
        Edge::Top => v.top() - q.1,
    };
    let depth =
        |q: (f64, f64), edge: Edge| from(q, edge) / (TURN * if edge.side() { v.w } else { v.h });
    // The cursor came `PUSH` towards the edge from `far` away; or any way at
    // all, where the screen ends and it can go no further.
    let pushed = |edge: Edge, far: f64| {
        let by = far - from(p, edge);
        let stopped = match edge {
            Edge::Left => p.0 - whole.x,
            Edge::Right => whole.right() - p.0,
            Edge::Bottom => p.1 - whole.y,
            Edge::Top => whole.top() - p.1,
        } <= 2.0;
        by >= PUSH || (stopped && by > 0.0)
    };
    let found = |edge: Edge| Some((edge, depth(p, edge) < TUCK / TURN));
    let edges = [Edge::Left, Edge::Right, Edge::Bottom, Edge::Top];
    let home = drag.and_then(|drag| {
        edges
            .into_iter()
            .find(|edge| edge.side() == column && depth(drag.start, *edge) < 1.0 && open(*edge))
            .map(|edge| (edge, from(drag.start, edge)))
    });
    if let Some((home, was)) = home {
        if pushed(home, was) {
            return found(home);
        }
        if from(p, home) < was + PUSH {
            let over = |edge: Edge| match edge {
                Edge::Left => v.x - pill.x,
                Edge::Right => pill.right() - v.right(),
                Edge::Bottom => v.y - pill.y,
                Edge::Top => pill.top() - v.top(),
            };
            return edges
                .into_iter()
                .filter(|edge| *edge != home && depth(p, *edge) < 1.0 && open(*edge))
                .filter(|edge| over(*edge) >= PUSH)
                .max_by(|a, b| over(*a).total_cmp(&over(*b)))
                .and_then(found);
        }
    }
    let edge = edges
        .into_iter()
        .filter(|edge| home.is_none_or(|(home, _)| home != *edge))
        .filter(|edge| depth(p, *edge) < 1.0 && open(*edge))
        .min_by(|a, b| depth(p, *a).total_cmp(&depth(p, *b)))?;
    if let Some(drag) = drag {
        let furthest = match edge {
            Edge::Left | Edge::Bottom => drag.max,
            Edge::Right | Edge::Top => drag.min,
        };
        if !pushed(edge, from(furthest, edge)) {
            return None;
        }
    }
    found(edge)
}

/// The tab for a pill tucked into `edge`, by where the pill (its
/// bottom-right `corner`, `len` long) was: a dot per person.
fn tab_rect(edge: Edge, corner: (f64, f64), len: f64, people: usize, visible: Rect) -> Rect {
    let long = (people as f32 * TAB_STEP + 14.0) as f64;
    let thick = TAB as f64;
    match edge {
        Edge::Left | Edge::Right => {
            let middle = corner.1 + len / 2.0;
            let highest = (visible.top() - long).max(visible.y);
            Rect {
                x: if edge == Edge::Left {
                    visible.x
                } else {
                    visible.right() - thick
                },
                y: (middle - long / 2.0).clamp(visible.y, highest).round(),
                w: thick,
                h: long,
            }
        }
        Edge::Top | Edge::Bottom => {
            let middle = corner.0 - len / 2.0;
            let rightmost = (visible.right() - long).max(visible.x);
            Rect {
                x: (middle - long / 2.0).clamp(visible.x, rightmost).round(),
                y: if edge == Edge::Top {
                    visible.top() - thick
                } else {
                    visible.y
                },
                w: long,
                h: thick,
            }
        }
    }
}

/// While you share: the shape of what you share.
/// Shown once the first frame arrives, so the bubble opens once, in the
/// right shape.
fn preview_aspect(me: &Member) -> Option<f32> {
    Some(me.screen.as_ref()?.sink.aspect()?.clamp(1.0, 2.5))
}

/// The stack lying down: its front face, up to two faces peeking out behind
/// it, "+N". Standing, the same from top to bottom.
fn stack_len(n: usize, column: bool) -> f32 {
    let behind = n.saturating_sub(1).min(2) as f32;
    if column {
        let count = if n > 1 { 4.0 + 16.0 } else { 0.0 };
        2.0 + FACE + behind * GHOST + count + 8.0
    } else {
        let count = if n > 1 { GAP + 2.0 + COUNT } else { 0.0 };
        2.0 + FACE + behind * GHOST + count + 9.0
    }
}

/// How long the pill is: the seat or the faces, the stack, the buttons.
fn pill_len(plan: &Plan, column: bool) -> f32 {
    let mut items: Vec<f32> = plan.faces.iter().map(|_| FACE).collect();
    if plan.seat {
        items.push(FACE);
    }
    let mut extra = 0.0;
    if !plan.stack.is_empty() {
        items.push(stack_len(plan.stack.len(), column));
        extra += GAP;
    }
    items.push(5.0); // the separator
    if plan.recording {
        items.push(BUTTON);
    }
    if plan.captions {
        items.push(BUTTON);
    }
    items.push(4.0 * BUTTON + 3.0 * GAP);
    // 2 for the pill's hairline.
    2.0 + 2.0 * PAD + items.iter().sum::<f32>() + GAP * (items.len() - 1) as f32 + extra
}

/// How tall a card is, its hairline included.
fn card_height(drawer: &Drawer, plan: &Plan) -> f32 {
    2.0 + match drawer {
        Drawer::Menu => {
            let rule = if plan.menu_extras > 0 { 9.0 } else { 0.0 };
            12.0 + MENU_HEAD + (3 + plan.menu_extras) as f32 * MENU_ITEM + rule
        }
        Drawer::Recording | Drawer::Transcript => BUTTON_LINE_H,
        Drawer::RecordChoice | Drawer::TranscribeChoice | Drawer::Prompt => CHOICE_H,
        Drawer::Notice if plan.notice_button => BUTTON_LINE_H,
        Drawer::Name(_) | Drawer::Notice => LINE_H,
        Drawer::People => {
            let lines = plan.stack.len().div_ceil(4).max(1) as f32;
            10.0 + 18.0 + lines * 52.0 + (lines - 1.0) * 8.0 + 10.0
        }
    }
}

/// The menu and the people have a width; the rest is as wide as its words.
fn card_width(drawer: &Drawer) -> Option<f32> {
    match drawer {
        Drawer::Menu => Some(MENU_W),
        Drawer::People => Some(PEOPLE_W),
        _ => None,
    }
}

/// The end of `text` that fits in about `width` points at 12.5 px, from a
/// word's start, with "…" before it: the caption shows the newest words.
fn tail(text: &str, width: f32) -> String {
    let budget = (width / 6.6).max(8.0) as usize;
    let count = text.chars().count();
    if count <= budget {
        return text.to_owned();
    }
    let cut: String = text.chars().skip(count - budget + 1).collect();
    let from = cut.find(' ').map_or(0, |ix| ix + 1);
    format!("…{}", &cut[from..])
}

/// "speaking", "active 40 s ago", "muted"…
fn status(m: &Member, sharing: bool) -> String {
    if m.is_local {
        return if m.mic_muted { "muted" } else { "unmuted" }.into();
    }
    if sharing {
        return "sharing their screen".into();
    }
    if m.mic_muted {
        return "muted".into();
    }
    if m.speaking {
        return "speaking".into();
    }
    match m.last_active.map(|t| t.elapsed().as_secs()) {
        Some(s) if s < 60 => format!("active {s} s ago"),
        Some(s) => format!("active {} min ago", s / 60),
        None => "quiet".into(),
    }
}

impl Render for Capsule {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_slots(cx);
        let plan = self.fit(window, cx);
        // Until a resize lands, draw the pill where it is on screen now.
        let (left, top) = (self.shown.0 - self.anchor.0, self.shown.1 - self.anchor.1);
        let content = if let Some(edge) = self.tucked {
            self.tab_view(edge, &plan, cx).into_any_element()
        } else {
            let drawer = self.drawer(cx);
            let len = pill_len(&plan, self.column);
            let pill = self.main_view(&plan, cx);
            if self.column {
                let side_w = self.size.width - MAIN - FLOAT_GAP;
                let card =
                    drawer.map(|d| self.drawer_view(&d, &plan, card_height(&d, &plan), false, cx));
                let caption = self.caption_view(&plan, side_w);
                let right = self.side_right;
                let side = (card.is_some() || caption.is_some()).then(|| {
                    div()
                        .w(px(side_w.max(0.0)))
                        .flex()
                        .flex_col()
                        .gap(px(FLOAT_GAP))
                        .map(|el| {
                            if right {
                                el.items_start()
                            } else {
                                el.items_end()
                            }
                        })
                        .children(card)
                        .children(caption)
                });
                div()
                    .size_full()
                    .flex()
                    .items_end()
                    .gap(px(FLOAT_GAP))
                    .map(|row| {
                        if right {
                            row.child(pill).children(side)
                        } else {
                            row.justify_end().children(side).child(pill)
                        }
                    })
                    .into_any_element()
            } else {
                let card =
                    drawer.map(|d| self.drawer_view(&d, &plan, card_height(&d, &plan), true, cx));
                let caption = self.caption_view(&plan, len.min(CAPTION_MAX)).map(|c| {
                    div()
                        .flex_none()
                        .h(px(CAPTION_H + FLOAT_GAP))
                        .flex()
                        .map(|el| {
                            if self.dock_up {
                                el.items_start()
                            } else {
                                el.items_end()
                            }
                        })
                        .child(c)
                });
                let (card_over, card_under) = if self.card_first {
                    (card, None)
                } else {
                    (None, card)
                };
                let (caption_over, caption_under) = if self.dock_up {
                    (caption, None)
                } else {
                    (None, caption)
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_end()
                    .children(card_over)
                    .children(caption_over)
                    .child(pill)
                    .children(caption_under)
                    .children(card_under)
                    .into_any_element()
            }
        };

        div()
            .id("capsule")
            .size_full()
            .relative()
            .overflow_hidden()
            .text_color(rgb(TEXT))
            .text_size(px(13.0))
            // Any spot that isn't a button drags the widget; a press outside
            // the open card closes it; twice is the call window.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if this.open.is_some() {
                        this.set_open(None, window, cx);
                    }
                    if event.click_count >= 2 && this.tucked.is_none() {
                        this.call.update(cx, |c, cx| c.set_mode(Mode::Regular, cx));
                    } else {
                        this.begin_drag(window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    this.set_open(None, window, cx);
                    this.context_menu(cx);
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(self.size.width))
                    .h(px(self.size.height))
                    .child(content),
            )
    }
}

impl Capsule {
    /// The screen badge on someone sharing: click for their share window.
    fn share_badge(&self, identity: &str, top: f32, right: f32) -> Stateful<Div> {
        let call = self.call.clone();
        let who = identity.to_owned();
        div()
            .id(gpui::SharedString::from(format!("share-{identity}")))
            .absolute()
            .top(px(top))
            .right(px(right))
            .size(px(16.0))
            .rounded_full()
            .bg(rgb(ACCENT))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                let call = call.clone();
                let who = who.clone();
                // Not from inside this window's event: AppKit reorders windows.
                cx.defer(move |cx| {
                    if let Some(handle) = call.read(cx).share_window_of(&who) {
                        let _ = handle
                            .update(cx, |_, window, _| crate::platform::bring_to_front(window));
                    }
                });
            })
            .child(icon(Icon::MonitorUp, 10.0, rgb(0xffffff)))
    }

    fn main_view(&self, plan: &Plan, cx: &mut Context<Self>) -> Div {
        let column = self.column;
        let faces: Vec<AnyElement> = plan
            .faces
            .iter()
            .enumerate()
            .map(|(ix, member)| self.face(ix, member, plan, cx).into_any_element())
            .collect();
        let stack = (!plan.stack.is_empty()).then(|| self.stack_view(plan, cx));
        let c = self.call.read(cx);
        // Alone: a seat where the first guest will sit. It copies the link.
        let seat = plan.seat.then(|| {
            let call = self.call.clone();
            let glyph = if c.copied_at.is_some() {
                Icon::Check
            } else {
                Icon::Plus
            };
            div()
                .id("seat")
                .flex_none()
                .size(px(FACE))
                .rounded_full()
                .border_1()
                .border_dashed()
                .border_color(white(0.3))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.border_color(white(0.6)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| call.update(cx, |c, cx| c.copy_link(cx)))
                .child(icon(glyph, 16.0, rgb(TEXT_DIM)))
        });
        let on = |f: fn(&mut Call, &mut Context<Call>)| {
            let call = self.call.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| call.update(cx, f)
        };
        let sharing = c.sharing.is_some();
        // Held to talk: green until it's let go. Talking while muted: a
        // ring, for as long as the notice says so.
        let mic = div()
            .relative()
            .flex_none()
            .child(
                round_button(
                    "mic",
                    if c.mic_on { Icon::Mic } else { Icon::MicOff },
                    BUTTON,
                    if c.held {
                        Tone::Live
                    } else if c.mic_on {
                        Tone::Plain
                    } else {
                        Tone::Off
                    },
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.mic_pressed(cx)),
                )
                .on_click(cx.listener(|this, _, _, cx| this.mic_clicked(cx))),
            )
            .when(c.nudging(), |el| {
                el.child(ring(BUTTON / 2.0, 2.0, rgb(ACCENT)))
            });
        let controls = div()
            .flex()
            .when(column, |el| el.flex_col())
            .gap(px(GAP))
            .child(
                round_button(
                    "more",
                    Icon::Ellipsis,
                    BUTTON,
                    if self.open == Some(Drawer::Menu) {
                        Tone::Selected
                    } else {
                        Tone::Plain
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle(Drawer::Menu, window, cx))),
            )
            .child(
                round_button(
                    "share",
                    if sharing {
                        Icon::MonitorOff
                    } else {
                        Icon::MonitorUp
                    },
                    BUTTON,
                    if sharing { Tone::Active } else { Tone::Plain },
                )
                .on_click(on(Call::toggle_share)),
            )
            .child(mic)
            .child(
                round_button("leave", Icon::PhoneDown, BUTTON, Tone::Danger)
                    .on_click(on(Call::leave)),
            );
        let separator = if column {
            div().w(px(24.0)).h(px(1.0)).my(px(2.0)).bg(white(0.14))
        } else {
            div().w(px(1.0)).h(px(24.0)).mx(px(2.0)).bg(white(0.14))
        };
        // Everyone sees that the call is recorded; a click says by whom.
        let record = plan.recording.then(|| {
            round_button("recording", Icon::CircleDot, BUTTON, Tone::Recording).on_click(
                cx.listener(|this, _, window, cx| this.toggle(Drawer::Recording, window, cx)),
            )
        });
        // Nobody is transcribed without seeing it either; a click says on
        // whose Mac, with the transcript and Stop.
        let captions = plan.captions.then(|| {
            let live = c.transcription.is_some();
            div()
                .relative()
                .flex_none()
                .child(
                    round_button(
                        "captions",
                        Icon::Captions,
                        BUTTON,
                        if self.open == Some(Drawer::Transcript) {
                            Tone::Selected
                        } else {
                            Tone::Plain
                        },
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle(Drawer::Transcript, window, cx)
                    })),
                )
                .when(live, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(1.0))
                            .right(px(1.0))
                            .size(px(9.0))
                            .rounded_full()
                            .bg(rgb(RECORDING))
                            .border_2()
                            .border_color(rgb(EDGE)),
                    )
                })
        });
        let len = pill_len(plan, column);
        div()
            .flex_none()
            .map(|el| {
                if column {
                    el.w(px(MAIN)).h(px(len)).py(px(PAD)).flex_col()
                } else {
                    el.h(px(MAIN)).w(px(len)).px(px(PAD))
                }
            })
            .flex()
            .items_center()
            .justify_end()
            .gap(px(GAP))
            .rounded(px(RADIUS))
            .bg(rgb(PILL))
            .border_1()
            // A dragged bubble is near enough to ride again.
            .border_color(if self.magnet {
                rgb(ACCENT).into()
            } else {
                white(0.09)
            })
            .children(seat)
            .children(faces)
            .children(stack)
            .child(separator)
            .children(record)
            .children(captions)
            .child(controls)
    }

    /// A face: who it is, and their video's switch. Yours turns your camera
    /// on and off; anyone else's with a camera hides and shows their
    /// bubble. Pressed and moved, it drags the widget like any other spot.
    fn face(
        &self,
        ix: usize,
        member: &Entity<Member>,
        plan: &Plan,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let m = member.read(cx);
        let speaking = m.speaking && !m.mic_muted;
        let sharing = !m.is_local && plan.sharers.contains(&m.identity);
        let muted = m.mic_muted;
        let identity = m.identity.clone();
        let name = m.name.clone();
        let switch = m.is_local || m.video.is_some();
        let unseen = !m.is_local && m.video.is_some() && self.hidden.contains(&m.identity);
        let member = member.clone();
        div()
            .id(("face", ix))
            .relative()
            .flex_none()
            .size(px(FACE))
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                let name = Drawer::Name(identity.clone());
                if !*hovered {
                    this.hover_task = None;
                    if this.open == Some(name) {
                        this.set_open(None, window, cx);
                    }
                } else if matches!(this.open, None | Some(Drawer::Name(_))) {
                    // Not over a card opened by a click.
                    this.open_after(name, window, cx);
                }
            }))
            .when(switch, |el| {
                el.cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.press = Some(event.position);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        let Some(from) = this.press else {
                            return;
                        };
                        if event.pressed_button != Some(MouseButton::Left) {
                            this.press = None;
                            return;
                        }
                        let moved = event.position - from;
                        if (moved.x / px(1.0)).hypot(moved.y / px(1.0)) > 4.0 {
                            this.press = None;
                            this.begin_drag(window, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.press.take().is_some() {
                            this.face_clicked(&member, cx);
                        }
                    }))
            })
            .child(avatar(&name, FACE))
            .when(speaking, |el| {
                el.child(ring(FACE / 2.0, RING, rgb(SPEAKING)))
            })
            .when(muted, |el| el.child(mute_badge()))
            .when(unseen, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(-2.0))
                        .bottom(px(-2.0))
                        .size(px(16.0))
                        .rounded_full()
                        .bg(rgb(0x2c2f38))
                        .border_2()
                        .border_color(rgb(EDGE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(Icon::VideoOff, 8.0, rgb(TEXT))),
                )
            })
            .when(sharing, |el| {
                el.child(self.share_badge(&m.identity, -4.0, -4.0))
            })
    }

    fn stack_view(&self, plan: &Plan, cx: &mut Context<Self>) -> Stateful<Div> {
        let column = self.column;
        let n = plan.stack.len();
        let behind = n.saturating_sub(1).min(2);
        let front = plan.stack[0].read(cx);
        let open = self.open == Some(Drawer::People);
        // The next people in the stack, peeking out behind the front one,
        // dimmer the further back they are.
        let peeking: Vec<AnyElement> = plan.stack[1..=behind]
            .iter()
            .enumerate()
            .rev()
            .map(|(ix, member)| {
                let name = member.read(cx).name.clone();
                let by = (ix + 1) as f32 * GHOST;
                div()
                    .absolute()
                    .map(|el| {
                        if column {
                            el.left_0().top(px(by))
                        } else {
                            el.top_0().left(px(by))
                        }
                    })
                    .size(px(FACE))
                    .rounded_full()
                    .bg(rgb(EDGE))
                    .child(avatar(&name, FACE).opacity(if ix == 0 { 0.45 } else { 0.3 }))
                    .into_any_element()
            })
            .collect();
        let pile = behind as f32 * GHOST;
        div()
            .id("stack")
            .flex_none()
            .map(|el| {
                if column {
                    el.w(px(44.0))
                        .pt(px(2.0))
                        .pb(px(8.0))
                        .mt(px(GAP))
                        .flex_col()
                        .gap(px(4.0))
                } else {
                    el.h(px(44.0))
                        .pl(px(2.0))
                        .pr(px(9.0))
                        .ml(px(GAP))
                        .gap(px(GAP))
                }
            })
            .flex()
            .items_center()
            .rounded(px(22.0))
            .bg(white(if open { 0.12 } else { 0.06 }))
            .cursor_pointer()
            .hover(|s| s.bg(white(0.12)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.toggle(Drawer::People, window, cx)))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .map(|el| {
                        if column {
                            el.w(px(FACE)).h(px(FACE + pile))
                        } else {
                            el.h(px(FACE)).w(px(FACE + pile))
                        }
                    })
                    .children(peeking)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .rounded_full()
                            .bg(rgb(EDGE))
                            .child(avatar(&front.name, FACE).opacity(0.7))
                            .when(front.mic_muted, |el| el.child(mute_badge())),
                    ),
            )
            .when(n > 1, |el| {
                el.child(
                    div()
                        .map(|el| {
                            if column {
                                el.h(px(16.0)).text_size(px(12.0))
                            } else {
                                el.w(px(COUNT)).ml(px(2.0)).text_size(px(13.0))
                            }
                        })
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(TEXT_DIM))
                        .child(format!("+{}", n - 1)),
                )
            })
    }

    /// The sentence being said, by the pill: who, and the newest words.
    fn caption_view(&self, plan: &Plan, width: f32) -> Option<Div> {
        let caption = plan.caption.as_ref()?;
        let line = div()
            .flex_none()
            .h(px(CAPTION_H))
            .w(px(width))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .overflow_hidden()
            .rounded(px(CAPTION_H / 2.0))
            .bg(rgb(CARD))
            .border_1()
            .border_color(white(0.09))
            .text_size(px(12.5))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        Some(match caption {
            Some((who, text)) => {
                let room = width - 26.0 - 6.0 - who.chars().count() as f32 * 7.4;
                line.child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(TEXT_DIM))
                        .child(who.clone()),
                )
                .child(div().min_w_0().truncate().child(tail(text, room)))
            }
            None => line.text_color(rgb(TEXT_DIM)).child("Listening…"),
        })
    }

    /// Tucked into an edge: a tab with a dot per person in the row, green
    /// while they speak, red for you muted. A click brings the call back.
    fn tab_view(&self, edge: Edge, plan: &Plan, cx: &mut Context<Self>) -> Stateful<Div> {
        let dots: Vec<Div> = plan
            .faces
            .iter()
            .map(|member| {
                let m = member.read(cx);
                let color = if m.speaking && !m.mic_muted {
                    rgb(SPEAKING).into()
                } else if m.is_local && m.mic_muted {
                    rgb(DANGER).into()
                } else {
                    white(0.32)
                };
                div().flex_none().size(px(TAB_DOT)).rounded_full().bg(color)
            })
            .collect();
        let r = px(9.0);
        div()
            .id("tab")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(TAB_STEP - TAB_DOT))
            .bg(rgb(PILL))
            .border_1()
            .border_color(white(0.14))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x22242b)))
            .map(|el| match edge {
                Edge::Left => el.flex_col().rounded_tr(r).rounded_br(r),
                Edge::Right => el.flex_col().rounded_tl(r).rounded_bl(r),
                Edge::Top => el.rounded_bl(r).rounded_br(r),
                Edge::Bottom => el.rounded_tl(r).rounded_tr(r),
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.untuck(cx)))
            .children(dots)
    }

    /// A card by the pill. Lying down (`lying`): with a gap between, at the
    /// pill's right end, or centred over it for a name. By a column: at the
    /// column's side.
    fn drawer_view(
        &self,
        drawer: &Drawer,
        plan: &Plan,
        height: f32,
        lying: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let body: AnyElement = match drawer {
            Drawer::Menu => self.menu(cx).into_any_element(),
            Drawer::People => self.people_row(plan, cx).into_any_element(),
            Drawer::Name(identity) => self.name_line(identity, plan, cx),
            Drawer::Recording => self.recording_line(cx),
            Drawer::RecordChoice => self.record_choice(cx).into_any_element(),
            Drawer::TranscribeChoice => self.transcribe_choice(cx).into_any_element(),
            Drawer::Transcript => self.transcript_line(cx),
            Drawer::Prompt => self.prompt_view(cx).into_any_element(),
            Drawer::Notice => self.notice_line(cx),
        };
        // The menu and the people have a width; the rest is as wide as its
        // words, up to the widget's width.
        let radius = match drawer {
            Drawer::Menu
            | Drawer::People
            | Drawer::RecordChoice
            | Drawer::TranscribeChoice
            | Drawer::Prompt => CARD_RADIUS,
            _ => height / 2.0,
        };
        let centred = lying && matches!(drawer, Drawer::Name(_));
        let card = div()
            .id("card")
            .flex_none()
            .h(px(height))
            .max_w_full()
            .overflow_hidden()
            .rounded(px(radius))
            .bg(rgb(CARD))
            .border_1()
            .border_color(white(0.09))
            .flex()
            .flex_col()
            .justify_center()
            .when_some(card_width(drawer), |el, width| el.w(px(width)))
            // A press in a card neither drags the widget nor closes the card.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(body);
        if !lying {
            return div().flex_none().max_w_full().child(card);
        }
        div()
            .flex_none()
            .w_full()
            .h(px(height + FLOAT_GAP))
            .flex()
            .map(|el| {
                if centred {
                    el.justify_center()
                } else {
                    el.justify_end()
                }
            })
            .map(|el| {
                if self.card_first {
                    el.items_start()
                } else {
                    el.items_end()
                }
            })
            .child(card)
    }

    fn people_row(&self, plan: &Plan, cx: &mut Context<Self>) -> Div {
        // With one bubble for the speaker, a person here with a camera on
        // is a switch too: a click puts them in it, or hides them again.
        let follows = self.follows(cx);
        let people: Vec<AnyElement> = plan
            .stack
            .iter()
            .enumerate()
            .map(|(ix, member)| {
                let m = member.read(cx);
                let switch = follows && m.video.is_some();
                let unseen = switch && self.hidden.contains(&m.identity);
                let member = member.clone();
                div()
                    .id(("person", ix))
                    .w(px(PERSON))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .when(switch, |el| {
                        el.cursor_pointer().on_click(
                            cx.listener(move |this, _, _, cx| this.face_clicked(&member, cx)),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .size(px(34.0))
                            .child(avatar(&m.name, 34.0).when(unseen, |el| el.opacity(0.5)))
                            .when(m.mic_muted, |el| el.child(mute_badge())),
                    )
                    .child(
                        div()
                            .max_w(px(PERSON))
                            .truncate()
                            .text_size(px(10.5))
                            .text_color(rgb(TEXT_DIM))
                            .child(if m.is_local {
                                "You".to_owned()
                            } else {
                                first_name(&m.name).to_owned()
                            }),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .px(px(12.0))
            .child(
                div()
                    .h(px(18.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT_DIM))
                    .child(format!("Muted or quiet · {}", plan.stack.len())),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(4.0))
                    .gap_y(px(8.0))
                    .children(people),
            )
    }

    fn name_line(&self, identity: &str, plan: &Plan, cx: &App) -> AnyElement {
        let c = self.call.read(cx);
        let Some(m) = c
            .members()
            .map(|m| m.read(cx))
            .find(|m| m.identity == identity)
        else {
            return div().into_any_element();
        };
        let name = if m.is_local {
            "You".to_owned()
        } else {
            m.name.clone()
        };
        // The widget is as narrow as its row: a long name gives way to the
        // status.
        div()
            .h(px(18.0))
            .px(px(12.0))
            .flex()
            .justify_center()
            .gap(px(4.0))
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(rgb(TEXT_DIM))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child(name),
            )
            .child(div().flex_none().child(format!(
                "· {}",
                status(m, plan.sharers.contains(&m.identity))
            )))
            .into_any_element()
    }

    fn notice_line(&self, cx: &App) -> AnyElement {
        let Some(notice) = self.call.read(cx).notice() else {
            return div().into_any_element();
        };
        let call = self.call.clone();
        let action = notice.action.clone();
        div()
            .h(px(if action.is_some() { 24.0 } else { 18.0 }))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .text_size(px(12.0))
            .text_color(if notice.error {
                rgb(0xff8a80)
            } else {
                rgb(TEXT_DIM)
            })
            .child(div().min_w_0().truncate().child(notice.text))
            .when_some(action, |el, action| {
                let button = match action {
                    NoticeAction::ShowInFinder(file) => {
                        button("show-file", "Show in Finder", None, Kind::Secondary, true)
                            .on_click(move |_, _, _| crate::home::show_in_finder(&file))
                    }
                    // A failed download: try it again there.
                    NoticeAction::Settings => button(
                        "notice-settings",
                        "Settings",
                        Some(Icon::Settings),
                        Kind::Secondary,
                        true,
                    )
                    .on_click(|_, _, cx| crate::settings::open(cx)),
                };
                el.child(button.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
            })
            .when(notice.error, |el| {
                el.child(
                    icon_button("dismiss", Icon::X, 20.0)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| call.update(cx, |c, cx| c.dismiss_error(cx))),
                )
            })
            .into_any_element()
    }

    /// "● Recording · 4:12 [Stop]", or who else records.
    fn recording_line(&self, cx: &App) -> AnyElement {
        let c = self.call.read(cx);
        let Some(r) = &c.recording else {
            return div().into_any_element();
        };
        let line = div()
            .h(px(24.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .text_size(px(12.0))
            .text_color(rgb(TEXT_DIM))
            .child(
                div()
                    .size(px(8.0))
                    .flex_none()
                    .rounded_full()
                    .bg(rgb(RECORDING)),
            );
        if r.is_mine() {
            let call = self.call.clone();
            line.child(
                div()
                    .min_w_0()
                    .truncate()
                    .flex()
                    .gap(px(4.0))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT))
                            .child("Recording"),
                    )
                    .child(format!(
                        "{} · {}",
                        if r.screen { "screen" } else { "audio" },
                        clock(r.since.elapsed())
                    )),
            )
            .child(
                button("stop-recording", "Stop", None, Kind::Secondary, true)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| call.update(cx, |c, cx| c.toggle_record(cx))),
            )
            .into_any_element()
        } else {
            let first = first_name(&r.name);
            line.child(div().min_w_0().truncate().child(if r.screen {
                format!("{first} is recording their screen. Only {first} can stop it.")
            } else {
                format!("{first} is recording. Only {first} can stop it.")
            }))
            .into_any_element()
        }
    }

    /// The ⋯ menu: the room and how long you've been in it, then Camera,
    /// Invite, Call window, and Record and Transcribe while the call has
    /// neither. Those two ask their question in the same card.
    fn menu(&self, cx: &mut Context<Self>) -> Div {
        type Action = Box<dyn Fn(&mut Capsule, &mut Window, &mut Context<Capsule>)>;
        let c = self.call.read(cx);
        let camera_off = c.camera == Camera::Off;
        let can_record = crate::call::RECORDS_HERE && c.recording.is_none();
        let can_transcribe = c.can_transcribe();
        let room = if c.room.name.is_empty() {
            "Call".to_owned()
        } else {
            c.room.name.clone()
        };
        let time = c.elapsed().map(clock).unwrap_or_default();
        let item = |id: &'static str,
                    glyph: Icon,
                    label: &'static str,
                    state: Option<&'static str>,
                    more: bool,
                    action: Action| {
            div()
                .id(id)
                .h(px(MENU_ITEM))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .hover(|s| s.bg(white(0.08)))
                .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
                .child(icon(glyph, 16.0, rgb(TEXT)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
                .when_some(state, |el, state| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(rgb(TEXT_DIM))
                            .child(state),
                    )
                })
                .when(more, |el| {
                    el.child(icon(Icon::ChevronRight, 14.0, rgb(TEXT_DIM)))
                })
        };
        div()
            .p(px(6.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(MENU_HEAD))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .text_size(px(12.0))
                    .text_color(rgb(TEXT_DIM))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT))
                            .child(room),
                    )
                    .child(div().flex_none().child(time)),
            )
            .child(item(
                "camera",
                if camera_off {
                    Icon::VideoOff
                } else {
                    Icon::Video
                },
                "Camera",
                Some(if camera_off { "Off" } else { "On" }),
                false,
                Box::new(|this, window, cx| {
                    this.set_open(None, window, cx);
                    this.call.update(cx, |c, cx| c.toggle_camera(cx));
                }),
            ))
            .child(item(
                "invite",
                Icon::UserPlus,
                "Invite",
                None,
                false,
                Box::new(|this, window, cx| {
                    this.set_open(None, window, cx);
                    this.call.update(cx, |c, cx| c.invite(cx));
                }),
            ))
            .child(item(
                "window",
                Icon::Maximize,
                "Call window",
                None,
                false,
                Box::new(|this, window, cx| {
                    this.set_open(None, window, cx);
                    this.call.update(cx, |c, cx| c.set_mode(Mode::Regular, cx));
                }),
            ))
            .when(can_record || can_transcribe, |el| {
                el.child(div().h(px(1.0)).my(px(4.0)).mx(px(10.0)).bg(white(0.07)))
            })
            // One recording per call: gone while anyone records.
            .when(can_record, |el| {
                el.child(item(
                    "record",
                    Icon::CircleDot,
                    "Record",
                    None,
                    true,
                    Box::new(|this, window, cx| {
                        this.set_open(Some(Drawer::RecordChoice), window, cx)
                    }),
                ))
            })
            // One transcript per call too.
            .when(can_transcribe, |el| {
                el.child(item(
                    "transcribe",
                    Icon::Captions,
                    "Transcribe",
                    None,
                    true,
                    Box::new(|this, window, cx| {
                        this.set_open(Some(Drawer::TranscribeChoice), window, cx)
                    }),
                ))
            })
    }

    /// "Parakeet v3 on this Mac. Everyone sees it." [Start], or [Download
    /// and start] when the model isn't here yet. With others whose Macs can
    /// transcribe: whose Mac? [This Mac] [Anna] …
    fn transcribe_choice(&self, cx: &mut Context<Self>) -> Div {
        let model = crate::Shell::global(cx)
            .read(cx)
            .store
            .transcription
            .model();
        let ready = crate::models::Models::global(cx).read(cx).is_ready(model);
        let others = self.call.read(cx).capable(cx);
        let start_label = match (ready, others.is_empty()) {
            (true, true) => "Start".to_owned(),
            (true, false) => crate::ui::THIS_COMPUTER_TITLE.to_owned(),
            (false, true) => format!(
                "Download {} and start",
                crate::store::file_size(model.bytes)
            ),
            (false, false) => format!(
                "{} · {}",
                crate::ui::THIS_COMPUTER_TITLE,
                crate::store::file_size(model.bytes)
            ),
        };
        let question = if others.is_empty() {
            format!(
                "{} on {}. Everyone sees it.",
                model.name,
                crate::ui::THIS_COMPUTER
            )
        } else {
            "Transcribe on whose Mac? Everyone sees it.".to_owned()
        };
        let start = button(
            "transcribe-start",
            start_label,
            Some(if ready {
                Icon::Captions
            } else {
                Icon::Download
            }),
            Kind::Primary,
            true,
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _, window, cx| {
            this.set_open(None, window, cx);
            this.call.update(cx, |c, cx| c.start_transcribing(cx));
        }));
        // Two at most: the widget is narrow. The call window lists everyone.
        let people: Vec<_> = others
            .into_iter()
            .take(2)
            .enumerate()
            .map(|(ix, (identity, name, _))| {
                button(
                    ("ask", ix),
                    first_name(&name).to_owned(),
                    None,
                    Kind::Secondary,
                    true,
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_open(None, window, cx);
                    this.call
                        .update(cx, |c, cx| c.ask_to_transcribe(&identity, cx));
                }))
            })
            .collect();
        let settings = if people.is_empty() {
            button(
                "transcribe-settings",
                "Settings",
                Some(Icon::Settings),
                Kind::Secondary,
                true,
            )
        } else {
            icon_button("transcribe-settings", Icon::Settings, 24.0)
        }
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, _, cx| crate::settings::open(cx));
        div()
            .px(px(12.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .h(px(18.0))
                    .max_w_full()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(rgb(TEXT_DIM))
                    .child(question),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(start)
                    .children(people)
                    .child(settings),
            )
    }

    /// "Anna asks your Mac to transcribe the call" [Accept] [Not now], or
    /// whether to carry on after the transcriber left.
    fn prompt_view(&self, cx: &mut Context<Self>) -> Div {
        let Some(prompt) = self.call.read(cx).prompt.clone() else {
            return div();
        };
        let first = first_name(&prompt.name).to_owned();
        let (question, yes) = match prompt.kind {
            PromptKind::Asked { handover: false } => (
                format!("{first} asks {} to transcribe", crate::ui::YOUR_COMPUTER),
                "Accept",
            ),
            PromptKind::Asked { handover: true } => (
                format!("{first} asks {} to take over", crate::ui::YOUR_COMPUTER),
                "Accept",
            ),
            PromptKind::TakeOver => (format!("{first} left. Transcribe here?"), "Transcribe"),
        };
        div()
            .px(px(12.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .h(px(18.0))
                    .max_w_full()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(rgb(TEXT))
                    .child(question),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        button("prompt-yes", yes, Some(Icon::Captions), Kind::Primary, true)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.call.update(cx, |c, cx| c.accept_prompt(cx))
                            })),
                    )
                    .child(
                        button("prompt-no", "Not now", None, Kind::Secondary, true)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.call.update(cx, |c, cx| c.decline_prompt(cx))
                            })),
                    ),
            )
    }

    /// "● Transcribing on your Mac · Parakeet v3 [Stop]", someone else's, or
    /// the download on its way.
    fn transcript_line(&self, cx: &App) -> AnyElement {
        let c = self.call.read(cx);
        let line = div()
            .h(px(24.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .text_size(px(12.0))
            .text_color(rgb(TEXT_DIM));
        let call = self.call.clone();
        let action = |id: &'static str, label: &'static str| {
            let call = call.clone();
            button(id, label, None, Kind::Secondary, true)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| call.update(cx, |c, cx| c.toggle_transcribe(cx)))
        };
        let dot = || {
            div()
                .size(px(8.0))
                .flex_none()
                .rounded_full()
                .bg(rgb(RECORDING))
        };
        // Opens or closes the transcript's window.
        let shown = c.transcript_open;
        let transcript = || {
            let call = call.clone();
            button(
                "transcript-window",
                if shown { "Hide" } else { "Transcript" },
                None,
                Kind::Secondary,
                true,
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                call.update(cx, |c, cx| {
                    let open = !c.transcript_open;
                    c.set_transcript_open(open, cx)
                })
            })
        };
        match (&c.transcription, c.preparing) {
            (Some(t), _) if t.mine => line
                .child(dot())
                .child(div().min_w_0().truncate().child(c.transcript_status(cx)))
                .child(transcript())
                .child(action("stop-transcript", "Stop"))
                .into_any_element(),
            (Some(t), _) => {
                let model = t.model.map_or(String::new(), |m| format!(" · {}", m.name));
                line.child(dot())
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(format!("On {}'s Mac{model}", first_name(&t.name))),
                    )
                    .child(transcript())
                    .child(action("stop-transcript", "Stop"))
                    .into_any_element()
            }
            (None, Some(Preparing::Download(model))) => {
                let percent =
                    match crate::models::Models::global_ref(cx).map(|m| m.read(cx).state(model)) {
                        Some(crate::models::State::Downloading { done, total }) if total > 0 => {
                            format!(" · {}%", done * 100 / total)
                        }
                        _ => String::new(),
                    };
                line.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(format!("Downloading {}{percent}", model.name)),
                )
                .child(action("cancel-download", "Cancel"))
                .into_any_element()
            }
            (None, _) => line
                .child(div().min_w_0().truncate().child("Starting the transcript…"))
                .into_any_element(),
        }
    }

    /// "Record on this Mac", then Audio or Audio and screen (the share
    /// picker chooses which screen).
    fn record_choice(&self, cx: &mut Context<Self>) -> Div {
        // What Settings says to offer first.
        let screen_first = crate::Shell::global(cx)
            .read(cx)
            .store
            .recording
            .screen_first;
        let choose = |id: &'static str, label: &'static str, glyph: Icon, screen: bool| {
            let kind = if screen == screen_first {
                Kind::Primary
            } else {
                Kind::Secondary
            };
            button(id, label, Some(glyph), kind, true)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_open(None, window, cx);
                    this.call.update(cx, |c, cx| {
                        if screen {
                            c.choose_screen_to_record(cx)
                        } else {
                            c.record_audio(cx)
                        }
                    });
                }))
        };
        div()
            .px(px(12.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .h(px(18.0))
                    .text_size(px(12.0))
                    .text_color(rgb(TEXT_DIM))
                    .child("Record on this Mac. Everyone sees it."),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(choose("record-audio", "Audio", Icon::Mic, false))
                    .child(choose(
                        "record-screen",
                        "Audio and screen",
                        Icon::Monitor,
                        true,
                    )),
            )
    }
}

fn mute_badge() -> Div {
    div()
        .absolute()
        .right(px(-2.0))
        .bottom(px(-2.0))
        .size(px(14.0))
        .rounded_full()
        .bg(rgb(DANGER))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(Icon::MicOff, 8.0, rgb(0xffffff)))
}

/// "4:05", "1:02:09".
pub fn clock(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 60.0,
        w: 1440.0,
        h: 800.0,
    };

    #[test]
    fn the_caption_keeps_its_newest_words() {
        assert_eq!(tail("short enough", 300.0), "short enough");
        let long = "we should ship the redesign on Friday if the tests pass by then";
        let cut = tail(long, 200.0);
        assert!(cut.starts_with('…'), "{cut}");
        assert!(cut.ends_with("pass by then"), "{cut}");
        // From a word's start, never mid-word.
        let first = cut.trim_start_matches('…').split(' ').next().unwrap();
        assert!(long.split(' ').any(|word| word == first), "{cut}");
        assert!(
            cut.chars().count() <= (200.0f32 / 6.6) as usize + 1,
            "{cut}"
        );
    }

    #[test]
    fn a_tab_sits_in_its_edge_by_where_the_pill_was() {
        // A column 300 long with its foot at y 200: the tab's middle is the
        // column's.
        let tab = tab_rect(Edge::Left, (60.0, 200.0), 300.0, 3, SCREEN);
        assert_eq!((tab.x, tab.w, tab.h), (0.0, 18.0, 59.0));
        assert!((tab.y + tab.h / 2.0 - 350.0).abs() <= 0.5);
        assert_eq!(tab.y, tab.y.round());
        let tab = tab_rect(Edge::Right, (1432.0, 200.0), 300.0, 1, SCREEN);
        assert_eq!((tab.right(), tab.h), (1440.0, 29.0));
        // Lying at the bottom: along the edge, under the pill's middle.
        let tab = tab_rect(Edge::Bottom, (900.0, 72.0), 300.0, 2, SCREEN);
        assert_eq!((tab.y, tab.h, tab.w), (60.0, 18.0, 44.0));
        assert_eq!(tab.x + tab.w / 2.0, 750.0);
        // Never past the screen's corner.
        let tab = tab_rect(Edge::Top, (20.0, 800.0), 300.0, 4, SCREEN);
        assert_eq!((tab.x, tab.top()), (0.0, 860.0));
    }

    /// [`SCREEN`] with its menu bar and the Dock.
    const WHOLE: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1440.0,
        h: 900.0,
    };

    /// Where a drop is for, after a drag through these points of a pill
    /// that was at `pill` when it began. The bands are 144 deep at the sides
    /// and 80 at the top and bottom (from y 60 to 860), tucking in the inner
    /// half.
    fn dropped(pill: Rect, path: &[(f64, f64)]) -> Option<(Edge, bool)> {
        let (start, p) = (path[0], *path.last().unwrap());
        let drag = path.iter().fold(Drag::new(start), |d, p| d.through(*p));
        let pill = Rect {
            x: pill.x + p.0 - start.0,
            y: pill.y + p.1 - start.1,
            ..pill
        };
        edge_for(p, Some(&drag), &pill, &SCREEN, &WHOLE, |_| true)
    }

    /// A pill 280 long, lying with its left end at `x` and its foot at `y`,
    /// or standing as a column.
    fn lying(x: f64, y: f64) -> Rect {
        Rect {
            x,
            y,
            w: 280.0,
            h: 52.0,
        }
    }

    fn standing(x: f64, y: f64) -> Rect {
        Rect {
            x,
            y,
            w: 52.0,
            h: 280.0,
        }
    }

    #[test]
    fn a_drop_is_for_the_edge_the_drag_came_to() {
        // From the middle of the screen, held by its middle.
        let (pill, middle) = (lying(560.0, 374.0), (700.0, 400.0));
        assert_eq!(dropped(pill, &[middle, (700.0, 300.0)]), None);
        let to = |p| dropped(pill, &[middle, p]);
        assert_eq!(to((1300.0, 400.0)), Some((Edge::Right, false)));
        assert_eq!(to((1400.0, 400.0)), Some((Edge::Right, true)));
        assert_eq!(to((100.0, 400.0)), Some((Edge::Left, false)));
        assert_eq!(to((700.0, 130.0)), Some((Edge::Bottom, false)));
        assert_eq!(to((700.0, 90.0)), Some((Edge::Bottom, true)));
        assert_eq!(to((700.0, 800.0)), Some((Edge::Top, false)));
        // Over the Dock and the menu bar, past what's visible of the screen.
        assert_eq!(to((700.0, 20.0)), Some((Edge::Bottom, true)));
        assert_eq!(to((700.0, 880.0)), Some((Edge::Top, true)));
        // By a corner, the edge the cursor is deeper into its band.
        assert_eq!(to((60.0, 110.0)), Some((Edge::Left, true)));
        assert_eq!(to((120.0, 80.0)), Some((Edge::Bottom, true)));
        // Where another display begins is no edge.
        let no_right = |edge| edge != Edge::Right;
        let drag = Drag::new(middle);
        let at = |p| edge_for(p, Some(&drag.through(p)), &pill, &SCREEN, &WHOLE, no_right);
        assert_eq!(at((1400.0, 400.0)), None);
        assert_eq!(at((1400.0, 130.0)), Some((Edge::Bottom, false)));
        // A script drops with no drag behind it.
        assert_eq!(
            edge_for((1300.0, 400.0), None, &pill, &SCREEN, &WHOLE, |_| true),
            Some((Edge::Right, false))
        );
    }

    #[test]
    fn a_pill_by_an_edge_stays_with_it() {
        // Lying at the bottom, held by its middle 38 over the edge. It slides
        // along the edge and comes away without tucking; pushed into the
        // edge, it tucks.
        let (pill, held) = (lying(560.0, 72.0), (700.0, 98.0));
        assert_eq!(dropped(pill, &[held, (300.0, 92.0)]), None);
        assert_eq!(dropped(pill, &[held, (700.0, 125.0)]), None);
        assert_eq!(
            dropped(pill, &[held, (700.0, 76.0)]),
            Some((Edge::Bottom, true))
        );
        // Taken away and brought back to about where it was: as it is.
        assert_eq!(dropped(pill, &[held, (600.0, 400.0), (650.0, 90.0)]), None);
        // Taken from the bottom to a side, it's the side's.
        assert_eq!(
            dropped(pill, &[held, (1400.0, 400.0)]),
            Some((Edge::Right, true))
        );
        // Slid along the bottom into a corner, by the end that gets there
        // first: it lies in the corner, though the cursor is in the side's
        // band, and the inner half of it at that.
        assert_eq!(dropped(pill, &[(590.0, 98.0), (38.0, 98.0)]), None);
        assert_eq!(dropped(pill, &[(810.0, 98.0), (1402.0, 98.0)]), None);
        // Slid on until it hangs out over the side, it's the side's.
        assert_eq!(
            dropped(pill, &[held, (1372.0, 98.0)]),
            Some((Edge::Right, true))
        );
        assert_eq!(
            dropped(pill, &[held, (100.0, 98.0)]),
            Some((Edge::Left, false))
        );
    }

    #[test]
    fn a_pill_in_a_corner_is_not_the_other_edges() {
        // Lying in the bottom right corner, held by its right end.
        let (pill, held) = (lying(1152.0, 72.0), (1410.0, 98.0));
        // Lifted straight up along the side: it only moved.
        assert_eq!(dropped(pill, &[held, (1410.0, 300.0)]), None);
        // Out from the side and back to it: a column.
        assert_eq!(
            dropped(pill, &[held, (1250.0, 300.0), (1320.0, 300.0)]),
            Some((Edge::Right, false))
        );
        // A column at the right, held by its foot, slides down into the
        // corner as it is, and tucks pushed into its side.
        let (pill, held) = (standing(1380.0, 260.0), (1410.0, 290.0));
        assert_eq!(dropped(pill, &[held, (1410.0, 100.0)]), None);
        assert_eq!(
            dropped(pill, &[held, (1432.0, 290.0)]),
            Some((Edge::Right, true))
        );
    }

    #[test]
    fn a_push_is_as_far_as_the_screen_goes() {
        // A column at the left, held 12 from where the screen ends: there's
        // no 20 to push, so any push to the very edge counts.
        let (pill, held) = (standing(8.0, 260.0), (12.0, 400.0));
        assert_eq!(dropped(pill, &[held, (6.0, 400.0)]), None);
        assert_eq!(
            dropped(pill, &[held, (1.0, 400.0)]),
            Some((Edge::Left, true))
        );
        // The same for a pill lying on the screen's very bottom: the Dock is
        // elsewhere, so what's visible ends where the screen does.
        let whole = Rect { y: 60.0, ..WHOLE };
        let drag = Drag::new((700.0, 78.0)).through((700.0, 61.0));
        assert_eq!(
            edge_for(
                (700.0, 61.0),
                Some(&drag),
                &lying(560.0, 55.0),
                &SCREEN,
                &whole,
                |_| true
            ),
            Some((Edge::Bottom, true))
        );
    }

    #[test]
    fn the_stack_is_as_long_standing_as_it_looks() {
        // One person: only the face. More: faces behind it, and "+N".
        assert_eq!(stack_len(1, false), 2.0 + FACE + 9.0);
        assert_eq!(stack_len(1, true), 2.0 + FACE + 8.0);
        assert!(stack_len(5, false) > stack_len(2, false));
        assert_eq!(stack_len(5, true), stack_len(9, true));
    }
}
