//! Cubie call engine.
//!
//! Owns everything about being in a call: the LiveKit room, platform audio,
//! published tracks and remote video plumbing. It has no UI dependency — a UI
//! drives it through [`Engine`] methods and consumes [`Event`]s.
//!
//! Each engine owns its runtime, including LiveKit internals. Application HTTP
//! uses a separate lazy runtime that survives call teardown.

// Without `transcribe` nothing starts a transcript here, but the call still
// follows everyone else's: the rest of that machinery stays, part of it idle.
#![cfg_attr(not(feature = "transcribe"), allow(dead_code))]

pub mod audio_tap;
mod camera;
#[cfg(feature = "denoise")]
pub mod denoise;
pub mod http;
#[cfg(feature = "transcribe")]
pub mod models;
pub mod permissions;
#[cfg(target_vendor = "apple")]
pub mod pixel_buffer;
#[cfg_attr(target_os = "macos", path = "player.rs")]
#[cfg_attr(not(target_os = "macos"), path = "player_none.rs")]
pub mod player;
pub mod record;
#[cfg(target_os = "macos")]
mod record_mac;
mod screen;
#[cfg(target_os = "macos")]
mod screen_mac;
mod tasks;
mod transcribe;
#[cfg(feature = "transcribe")]
pub mod trial;
pub mod video;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use cubie_protocol::JoinInfo;
use futures_util::{FutureExt as _, StreamExt as _};
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::{AudioProcessingOptions, PlatformAudio};
use parking_lot::Mutex;

pub use audio_tap::{Listener, Speaker, VoiceFrame};
pub use cubie_protocol as protocol;
pub use cubie_protocol::TranscriptLine;
use cubie_protocol::{TRANSCRIPT_TOPIC, TranscriptMessage};
pub use livekit;
pub use livekit::track::VideoQuality;
#[cfg(feature = "transcribe")]
pub use models::{ModelInfo, catalog};
pub use permissions::{Access, MediaDevice};
pub use record::Saved;
pub use screen::{ContentMode, ScreenSharePreset, SourceId};
#[cfg(target_os = "macos")]
pub use screen_mac::{NoPermission, ShareSource, ShareSourceUpdate, Thumbnail, own_window};
pub use video::{FrameSink, VideoSubscription};

// These deployed names allow old and new clients to share the same call.
/// The participant attribute that says someone is recording the call:
/// "audio", or "screen" (audio and their screen). Everyone's app shows it.
const RECORDING: &str = "quipcall.recording";
/// The participant attribute that says someone's Mac transcribes the call:
/// "<model id>@<unix ms it started>". If two Macs ever do, the one that
/// started last carries on (a handover), and the other stops.
const TRANSCRIBING: &str = "quipcall.transcribing";
/// The participant attribute that says a Mac could transcribe the call: the
/// id of the model it has ready.
const CAN_TRANSCRIBE: &str = "quipcall.can_transcribe";
/// How long after a Mac stops transcribing its last lines are still taken:
/// its claim goes first, then what was still being said is transcribed and
/// sent. The web app takes them as long.
const LAST_WORDS: Duration = Duration::from_millis(cubie_protocol::LAST_WORDS_MS);

/// Stable identifier of a remote track inside the UI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackKey {
    pub participant: String,
    pub sid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoKind {
    Camera,
    Screen,
}

#[derive(Debug)]
pub enum Event {
    Connected {
        room: String,
        identity: String,
        /// Everyone else in the room as you got in. Each also comes as a
        /// [`Event::ParticipantJoined`] right after.
        others: Vec<String>,
    },
    /// Getting into the room failed. `retry`: it may work in a moment (the
    /// network isn't up yet); otherwise it won't by itself.
    ConnectFailed {
        message: String,
        retry: bool,
    },
    /// The call is over for this connection. `lost`: nobody ended it, the
    /// connection did (a Mac that slept past LiveKit's own reconnecting,
    /// a network that stayed away); joining again can carry on.
    Disconnected {
        reason: String,
        lost: bool,
    },
    ParticipantJoined {
        identity: String,
        name: String,
    },
    ParticipantLeft {
        identity: String,
    },
    VideoTrackAdded {
        key: TrackKey,
        kind: VideoKind,
        track: RemoteVideoTrack,
    },
    VideoTrackRemoved {
        key: TrackKey,
    },
    ActiveSpeakers(Vec<String>),
    /// A remote participant's microphone was muted, unmuted, or removed.
    ParticipantMic {
        identity: String,
        muted: bool,
    },
    /// The OS is showing a permission prompt; the call waits for the answer.
    WaitingForPermission(MediaDevice),
    MicChanged {
        enabled: bool,
    },
    CameraChanged {
        enabled: bool,
    },
    ScreenShareChanged {
        active: bool,
        preset: Option<ScreenSharePreset>,
    },
    /// You're recording into `path`, with your screen if `screen`; everyone
    /// has been told.
    RecordingStarted {
        path: PathBuf,
        screen: bool,
    },
    /// Your recording stopped and its file is complete.
    RecordingSaved(Saved),
    /// Your recording couldn't start, or failed on the way. `path`: the
    /// recording's file, if it had started.
    RecordingFailed {
        message: String,
        path: Option<PathBuf>,
    },
    /// Someone else started or stopped recording the call (with their screen
    /// if `screen`).
    ParticipantRecording {
        identity: String,
        recording: bool,
        screen: bool,
    },
    /// Your Mac transcribes the call; everyone has been told.
    TranscriptionStarted,
    /// What someone said, or a draft of it while they talk (`done` false),
    /// as transcribed on `by`'s Mac (an identity; yours, or a transcriber's,
    /// now or before a handover). A line replaces earlier ones with its id
    /// from the same transcriber. `relayed`: it came from another Mac (the
    /// next transcriber's history), so it stands only until `by`'s own final.
    TranscriptLine {
        by: String,
        line: TranscriptLine,
        relayed: bool,
    },
    /// Your transcription stopped, and every line is out. `by` asked for it,
    /// if it wasn't you; `handed_over`: their Mac carries on.
    TranscriptionStopped {
        by: Option<String>,
        handed_over: bool,
    },
    /// Your transcription couldn't start.
    TranscriptionFailed(String),
    /// Someone else's Mac started or stopped transcribing the call, with the
    /// model `model` (a catalog id).
    ParticipantTranscribing {
        identity: String,
        model: Option<String>,
    },
    /// Someone's Mac can transcribe the call, with `model`, or can't any more.
    ParticipantCanTranscribe {
        identity: String,
        model: Option<String>,
    },
    /// `from` asks your Mac to transcribe the call; `handover`: their Mac
    /// does now, and stops once yours has started. `claim`: theirs when they
    /// asked, for [`Handover`].
    TranscriptAsked {
        from: String,
        handover: bool,
        claim: Option<u64>,
    },
    /// `from` answered your ask.
    TranscriptAnswered {
        from: String,
        accepted: bool,
    },
    Error(String),
}

/// What to transcribe with.
#[cfg(feature = "transcribe")]
#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    pub model: &'static ModelInfo,
    /// Where the model and [`catalog::VAD`] are.
    pub dir: PathBuf,
    /// ISO 639-1, or `None` to let the model tell.
    pub language: Option<String>,
    /// Take over from the Mac that asked: start although it transcribes.
    pub handover: Option<Handover>,
}

/// Taking over the transcript from the Mac that asked. Only while its claim
/// is the one it asked with, and nobody else started since, once the model
/// has loaded: a Stop or a restart in between calls it off.
#[derive(Debug, Clone)]
pub struct Handover {
    /// Its identity.
    pub from: String,
    /// Its claim when it asked ([`Event::TranscriptAsked`]); older apps
    /// don't say, and then it's whatever it is when you start.
    pub claim: Option<u64>,
}

pub struct Engine {
    runtime: Option<tokio::runtime::Runtime>,
    tasks: tasks::Tasks,
    state: Arc<Mutex<State>>,
    events: async_channel::Sender<Event>,
}

/// How long leaving may take once the engine is dropped.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Default)]
struct State {
    /// The call. Leaving, or losing it, clears it: whatever was starting
    /// for it (a recording, the transcript) sees that and stops.
    room: Option<Arc<Room>>,
    /// Its voices, to whoever listens. A new one for every call: a
    /// recording still being finished hears nothing of the next.
    hub: Arc<audio_tap::Hub>,
    /// Which connection the state is for, counted: every connect and
    /// leave moves it on, so a room still closing can't touch the next
    /// one's state with its last events.
    connection: u64,
    /// Keeps the platform audio device module alive for the duration of a call.
    /// Without it remote audio has nowhere to play.
    audio: Option<PlatformAudio>,
    mic: Option<LocalAudioTrack>,
    /// The microphone, when it opens, opens muted (asked for by a phone,
    /// where mute can be pressed while the call is still joining).
    mic_opens_muted: bool,
    /// This engine has been in its call before: connecting again (the
    /// connection was lost) tells everything anew, what stopped meanwhile too.
    joined_before: bool,
    /// Who you've been in this call: this connection and the ones before
    /// it. One from before may still show in the room for a while.
    mine: HashSet<String>,
    camera: Option<camera::Camera>,
    /// The camera start under way, to ignore repeated clicks. Forgotten
    /// with the call: one for a connection that's gone stops by itself.
    camera_starting: Option<u64>,
    camera_starts: u64,
    screen: Option<screen::ScreenShare>,
    /// Remote video publications, for per-tile quality control.
    videos: HashMap<TrackKey, RemoteTrackPublication>,
    /// Remote microphones, so someone listening (a recording) can hear them.
    mics: HashMap<TrackKey, RemoteAudioTrack>,
    /// Their decoded audio, while something listens.
    mic_streams: HashMap<TrackKey, tokio::task::JoinHandle<()>>,
    /// Your recording, while you record.
    recorder: Option<record::Recorder>,
    /// Your transcription, while your Mac transcribes.
    transcribing: Option<transcribe::Transcribing>,
    /// Your transcription on its way: the model loading, the claim going
    /// out. Stop can come meanwhile, and wins; leaving drops it.
    start: Start,
    /// Starts so far, counted: each knows which is its own.
    starts: u64,
    /// The Macs you asked to take over your transcript, with the claim you
    /// have now: any of them may. A Stop that comes to you goes on to each,
    /// so none carries on a transcript that was stopped: even after one took
    /// over, as another's start may still be on its way and pass it by.
    /// Until the Stop, or you start again, or leave.
    handover_asked: Vec<String>,
    /// The Mac that took over, and when: a Stop just after goes on to it.
    handed_to: Option<(String, Instant)>,
    /// The call's final lines, from your Mac or whoever transcribed before
    /// (a handover keeps them), for whoever joins while yours transcribes.
    transcribed: Vec<Kept>,
    /// Macs that stopped transcribing the call, and when: their last lines
    /// may still be on the way. Anyone else's lines are dropped.
    stopped_transcribing: HashMap<String, Instant>,
    /// When your transcription started, unix ms (see [`TRANSCRIBING`]).
    transcribing_since: u64,
    /// The model your Mac could transcribe with, told to everyone.
    can_transcribe: Option<String>,
    /// `disconnect` saying goodbye to the server; see [`Engine::closed`].
    closing: Vec<Closing>,
    /// Delivery tasks for this connection, including a transcript already stopping.
    transcript_sends: Vec<Closing>,
}

type Closing = futures_util::future::Shared<futures_util::future::BoxFuture<'static, ()>>;

/// A start of your transcription, by its number.
#[derive(Debug, Default, PartialEq)]
enum Start {
    #[default]
    No,
    Going(u64),
    /// Stop came, from them (or you), before it listened: it won't.
    Stopped(u64, Option<String>),
}

/// What was starting (the camera, a screen share) finished after its
/// connection was gone: it stopped, and has nothing to tell.
#[derive(Debug)]
struct Gone;

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the connection it started on is gone")
    }
}

impl std::error::Error for Gone {}

/// A transcription stopped before it listened to anyone.
#[derive(Debug)]
struct StoppedStarting(Option<String>);

impl std::fmt::Display for StoppedStarting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the transcript was stopped while your Mac got ready")
    }
}

impl std::error::Error for StoppedStarting {}

/// Account/room HTTP runtime, initialized lazily per process.
fn runtime() -> anyhow::Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: std::sync::OnceLock<Result<tokio::runtime::Runtime, std::io::Error>> =
        std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("cubie-http")
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{e}"))
}
async fn after_permission<T>(
    permission: impl Future<Output = anyhow::Result<()>>,
    request: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    permission.await?;
    request.await
}

impl Engine {
    pub fn new() -> anyhow::Result<(Self, async_channel::Receiver<Event>)> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("cubie-engine")
            .enable_all()
            .build()?;
        let tasks = tasks::Tasks::new(runtime.handle().clone());
        let (events, rx) = async_channel::unbounded();
        Ok((
            Self {
                runtime: Some(runtime),
                tasks,
                state: Default::default(),
                events,
            },
            rx,
        ))
    }

    /// Fetch a token for `room` from a Cubie server.
    pub fn join(
        &self,
        server: &str,
        room: &str,
        session: &str,
    ) -> impl Future<Output = anyhow::Result<JoinInfo>> + Send + 'static + use<> {
        self.join_at(http::Endpoint::Direct(server), room, session)
    }

    pub fn join_at(
        &self,
        endpoint: http::Endpoint<&str>,
        room: &str,
        session: &str,
    ) -> impl Future<Output = anyhow::Result<JoinInfo>> + Send + 'static + use<> {
        let events = self.events.clone();
        let request = endpoint.request_with_retry(
            Some(session),
            "/join",
            Some(serde_json::json!({"room":room})),
            "POST",
            true,
        );
        async move {
            // A macOS prompt can remain open indefinitely. Do not mint the
            // short-lived admission token until the user has answered it.
            let permission = async move {
                microphone_permission(&events).await?;
                anyhow::Ok(())
            };
            let value = after_permission(permission, request).await?;
            Ok(serde_json::from_value(value)?)
        }
    }

    pub fn connect(&self, join: JoinInfo) {
        // Its number now: leaving, or connecting again, before this one is
        // through makes it stale, and it closes.
        let connection = {
            let mut s = self.state.lock();
            s.connection += 1;
            s.connection
        };
        let state = self.state.clone();
        let events = self.events.clone();
        self.tasks.spawn(async move {
            if let Err(e) = connect(state, events.clone(), join, connection).await {
                let retry = e.downcast_ref::<RoomError>().is_some_and(may_connect_later);
                let _ = events
                    .send(Event::ConnectFailed {
                        message: format!("connect failed: {e:#}"),
                        retry,
                    })
                    .await;
            }
        });
    }

    pub fn disconnect(&self) {
        // Everything of this call leaves the state now, the audio device too:
        // a call connected before the goodbye below finishes has its own.
        let (room, audio, screen, camera, recorder, transcribing, outgoing) = {
            let mut s = self.state.lock();
            forget_call(&mut s);
            // Forget goodbyes already said. `peek` would only see ones
            // something awaited, so poll them.
            s.closing.retain(|c| c.clone().now_or_never().is_none());
            (
                s.room.take(),
                s.audio.take(),
                s.screen.take(),
                s.camera.take(),
                s.recorder.take(),
                s.transcribing.take(),
                std::mem::take(&mut s.transcript_sends),
            )
        };
        stop_taking_in(transcribing.as_ref(), recorder.as_ref());
        let events = self.events.clone();
        let closing = self.tasks.spawn(async move {
            let started = Instant::now();
            // Your screen goes first, without waiting for the goodbye: if
            // the app quits before that's out, the server only notices you
            // went 20 s later, and everyone sees your screen until then.
            if let Some(screen) = screen {
                match &room {
                    Some(room) => screen.unpublish(room).await,
                    None => drop(screen),
                }
            }
            finish_call(
                async {
                    finish_words(transcribing, &events).await;
                    futures_util::future::join_all(outgoing).await;
                },
                finish_recording(recorder, &events, false),
                async {
                    if let Some(room) = room {
                        let _ = room.close().await;
                    }
                },
                Duration::from_secs(2),
            )
            .await;
            tracing::info!(took = ?started.elapsed(), "left the room");
            // Stopping the camera blocks briefly; keep it off the UI thread.
            if let Some(camera) = camera {
                let _ = tokio::task::spawn_blocking(move || drop(camera)).await;
            }
            // Release the audio device only after the room is gone.
            drop(audio);
        });
        let closing = closing.map(|_| ()).boxed().shared();
        self.state.lock().closing.push(closing);
    }

    /// `identity` is you, now or as you were before joining again: the
    /// room may still show that one for a while.
    pub fn was_me(&self, identity: &str) -> bool {
        self.state.lock().mine.contains(identity)
    }

    /// Resolves once every `disconnect` so far has finished: the server knows
    /// you left. Works from any executor, e.g. while the app quits.
    pub fn closed(&self) -> impl std::future::Future<Output = ()> + Send + use<> {
        let closing = self.state.lock().closing.clone();
        futures_util::future::join_all(closing).map(|_| ())
    }

    fn rt(&self) -> &tokio::runtime::Runtime {
        self.runtime.as_ref().expect("engine runtime initialized")
    }

    /// Before the microphone is open: whether it opens muted, so nothing of
    /// you is sent until you unmute. Once it's open, [`Engine::set_mic_enabled`].
    /// Lasts for one call.
    pub fn set_mic_opens_muted(&self, muted: bool) {
        self.state.lock().mic_opens_muted = muted;
    }

    pub fn set_mic_enabled(&self, enabled: bool) {
        // LiveKit spawns tokio tasks internally; UI threads have no runtime.
        let _rt = self.rt().enter();
        tracing::info!(enabled, "set mic");
        let mut s = self.state.lock();
        if let Some(mic) = &s.mic {
            if enabled {
                mic.unmute()
            } else {
                mic.mute()
            }
            audio_tap::set_mic_live(&s.hub, enabled);
            let _ = self.events.try_send(Event::MicChanged { enabled });
        } else {
            // Not published yet (still joining): it opens as asked.
            s.mic_opens_muted = !enabled;
        }
    }

    /// Open the camera and publish it. Frames also go to `preview` (self-view).
    pub fn start_camera(&self, preview: Option<Arc<dyn FrameSink>>) {
        let start = {
            let mut s = self.state.lock();
            if s.camera.is_some() || s.camera_starting.is_some() {
                return;
            }
            s.camera_starts += 1;
            s.camera_starting = Some(s.camera_starts);
            s.camera_starts
        };
        let state = self.state.clone();
        let events = self.events.clone();
        self.tasks.spawn(async move {
            let result = async {
                if !camera::synthetic() {
                    if permissions::status(MediaDevice::Camera) == Access::Undecided {
                        let _ = events
                            .send(Event::WaitingForPermission(MediaDevice::Camera))
                            .await;
                    }
                    anyhow::ensure!(
                        permissions::request(MediaDevice::Camera).await,
                        "camera access is off. Allow it in {}",
                        permissions::where_to_allow(MediaDevice::Camera)
                    );
                }
                // The room now: it may have connected while you were asked.
                let (room, connection) = current_room(&state).context("not in a call")?;
                let camera = camera::Camera::start(&room, preview).await?;
                // The call may have ended while the camera was starting.
                let stale = {
                    let mut s = state.lock();
                    if s.connection == connection {
                        s.camera = Some(camera);
                        None
                    } else {
                        Some(camera)
                    }
                };
                if let Some(camera) = stale {
                    camera.stop(&room).await;
                    return Err(Gone.into());
                }
                Ok(())
            }
            .await;
            state.lock().camera_starting.take_if(|id| *id == start);
            // The next connection hears nothing of it.
            if result.as_ref().is_err_and(|e| e.is::<Gone>()) {
                tracing::info!("a camera start for a connection that's gone");
                return;
            }
            let event = match result {
                Ok(()) => Event::CameraChanged { enabled: true },
                Err(e) => Event::Error(format!("camera: {e:#}")),
            };
            let _ = events.send(event).await;
        });
    }

    /// A phone's front or back camera, for the next [`Engine::start_camera`].
    #[cfg(target_os = "ios")]
    pub fn set_camera_front(&self, front: bool) {
        camera::set_front(front);
    }

    pub fn stop_camera(&self) {
        let (room, camera) = {
            let mut s = self.state.lock();
            (s.room.clone(), s.camera.take())
        };
        let events = self.events.clone();
        self.tasks.spawn(async move {
            if let Some(camera) = camera {
                match room {
                    Some(room) => camera.stop(&room).await,
                    None => drop(camera),
                }
            }
            let _ = events.send(Event::CameraChanged { enabled: false }).await;
        });
    }

    /// Lists share choices first, then delivers previews as they arrive. Native
    /// screen APIs may block even before calling their completion handlers, so
    /// keep the entire operation off both the UI and media executor threads.
    /// Dropping the receiver stops queuing and waiting for previews. In-flight
    /// native requests retain their shared budget until completion, including
    /// when another picker opens with its own stream.
    #[cfg(target_os = "macos")]
    pub fn screen_sources(
        &self,
        thumb_width: u32,
    ) -> async_channel::Receiver<anyhow::Result<ShareSourceUpdate>> {
        let (tx, rx) = async_channel::bounded(8);
        let runtime = self.rt().handle().clone();
        let tasks = self.tasks.clone();
        self.rt().spawn_blocking(move || {
            crate::autorelease_pool(|| {
                runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = tx.closed() => {},
                        _ = tasks.cancelled() => {},
                        result = screen_mac::list_sources(thumb_width, &tx) => {
                            if let Err(error) = result {
                                let _ = tx.send(Err(error)).await;
                            }
                        }
                    }
                });
            });
        });
        rx
    }

    /// Share `source` (required on macOS, where our own picker chooses it).
    /// Captured frames also go to `preview`, so you can see what you share.
    pub fn start_screen_share(
        &self,
        source: Option<SourceId>,
        preset: ScreenSharePreset,
        preview: Option<Arc<dyn FrameSink>>,
    ) {
        let state = self.state.clone();
        let events = self.events.clone();
        let handle = self.tasks.clone();
        self.tasks.spawn(async move {
            let result = async {
                let (room, connection, previous) = {
                    let mut s = state.lock();
                    let room = s.room.clone().context("not in a call")?;
                    (room, s.connection, s.screen.take())
                };
                if let Some(previous) = previous {
                    previous.unpublish(&room).await;
                }
                // Failing later stops this share, not one of a later connection.
                let on_fail = {
                    let (state, events) = (state.clone(), events.clone());
                    move || {
                        handle.spawn(stop_screen_share(state, events, Some(connection)));
                    }
                };
                let share = screen::ScreenShare::start(
                    &room,
                    source,
                    preset,
                    preview,
                    events.clone(),
                    on_fail,
                )
                .await?;
                // The connection may have gone while it started.
                let stale = {
                    let mut s = state.lock();
                    if s.connection == connection {
                        s.screen = Some(share);
                        None
                    } else {
                        Some(share)
                    }
                };
                if let Some(share) = stale {
                    share.unpublish(&room).await;
                    return Err(Gone.into());
                }
                anyhow::Ok(())
            }
            .await;
            if result.as_ref().is_err_and(|e| e.is::<Gone>()) {
                tracing::info!("a screen share for a connection that's gone");
                return;
            }
            let event = match result {
                Ok(()) => Event::ScreenShareChanged {
                    active: true,
                    preset: Some(preset),
                },
                Err(e) => Event::Error(format!("screen share: {e:#}")),
            };
            let _ = events.send(event).await;
        });
    }

    pub fn stop_screen_share(&self) {
        self.tasks.spawn(stop_screen_share(
            self.state.clone(),
            self.events.clone(),
            None,
        ));
    }

    /// Pick the simulcast layer for a remote video. The UI calls this from the
    /// tile's on-screen size; LiveKit's Rust SDK doesn't do adaptive sizing itself.
    pub fn set_video_quality(&self, key: &TrackKey, quality: VideoQuality) {
        let _rt = self.rt().enter();
        if let Some(publication) = self.state.lock().videos.get(key)
            && publication.simulcasted()
        {
            publication.set_video_quality(quality);
        }
    }

    /// Start pumping decoded frames of `track` into `sink`. Frames stop when the
    /// returned subscription is dropped.
    pub fn attach_video(
        &self,
        track: &RemoteVideoTrack,
        sink: Arc<dyn FrameSink>,
    ) -> VideoSubscription {
        video::attach(&self.tasks, track, sink)
    }

    /// Everyone's voice, 10 ms at a time, until the listener is dropped.
    pub fn listen(&self) -> Listener {
        listen(&self.state, &self.tasks)
    }

    /// Record the call into `path`: everyone's voice, mixed (.m4a), and with
    /// `screen`, that screen or window too (.mp4). Everyone is told first (a
    /// participant attribute), so nobody is recorded without seeing it. One
    /// recording per call: refused while someone else records. With
    /// `each_voice`, every voice also goes into a file of its own, in a
    /// folder named after the recording.
    pub fn start_recording(
        &self,
        path: PathBuf,
        screen: Option<(SourceId, ScreenSharePreset)>,
        each_voice: bool,
    ) {
        let state = self.state.clone();
        let events = self.events.clone();
        let handle = self.tasks.clone();
        let with_screen = screen.is_some();
        self.tasks.spawn(async move {
            let event =
                match start_recording(&state, &handle, &events, path, screen, each_voice).await {
                    Ok(path) => Event::RecordingStarted {
                        path,
                        screen: with_screen,
                    },
                    Err(e) => Event::RecordingFailed {
                        message: format!("{e:#}"),
                        path: None,
                    },
                };
            let _ = events.send(event).await;
        });
    }

    /// Finish your recording; [`Event::RecordingSaved`] follows.
    pub fn stop_recording(&self) {
        let (room, recorder) = {
            let mut s = self.state.lock();
            (s.room.clone(), s.recorder.take())
        };
        let Some(recorder) = recorder else { return };
        let events = self.events.clone();
        self.tasks.spawn(async move {
            let event = stop_recorder(recorder).await;
            if let Some(room) = room
                && let Err(e) = set_attribute(&room, RECORDING, "").await
            {
                tracing::warn!("recording attribute: {e}");
            }
            let _ = events.send(event).await;
        });
    }

    /// Transcribe the call on this Mac: everyone's voice, each on its own.
    /// Loading the model takes a few seconds. Everyone is told before any
    /// voice is heard; one transcriber per call.
    #[cfg(feature = "transcribe")]
    pub fn start_transcribing(&self, options: TranscribeOptions) {
        // For this call, as it is now: leaving or joining another before it's
        // through calls it off.
        let reserved = {
            let mut s = self.state.lock();
            match s.room.clone() {
                None => Err("not in a call"),
                Some(room) => reserve_start(&mut s)
                    .map(|id| (room, s.connection, id))
                    .ok_or("you're already transcribing"),
            }
        };
        let (room, connection, id) = match reserved {
            Ok(reserved) => reserved,
            Err(why) => {
                let _ = self.events.try_send(Event::TranscriptionFailed(why.into()));
                return;
            }
        };
        let state = self.state.clone();
        let events = self.events.clone();
        let handle = self.tasks.clone();
        self.tasks.spawn(async move {
            let started =
                start_transcribing(&state, &handle, &events, options, room, connection, id).await;
            let current = {
                let mut s = state.lock();
                finish_start(&mut s, id);
                s.connection == connection
            };
            // A start for a call you've left tells the next one nothing.
            if !current {
                if let Err(e) = &started {
                    tracing::info!("a transcript start for a call you left: {e:#}");
                }
                return;
            }
            let event = match &started {
                Ok(()) => Event::TranscriptionStarted,
                Err(e) => match e.downcast_ref::<StoppedStarting>() {
                    Some(StoppedStarting(by)) => Event::TranscriptionStopped {
                        by: by.clone(),
                        handed_over: false,
                    },
                    None => Event::TranscriptionFailed(format!("{e:#}")),
                },
            };
            let _ = events.send(event).await;
            if started.is_ok() {
                settle_transcriber(&state, &events).await;
            }
        });
    }

    /// Stop transcribing; the last lines and [`Event::TranscriptionStopped`]
    /// follow.
    pub fn stop_transcribing(&self) {
        let state = self.state.clone();
        let events = self.events.clone();
        self.tasks
            .spawn(async move { stop_transcribing(&state, &events, None).await });
    }

    /// Ask `identity`'s Mac to stop transcribing the call: anyone may.
    pub fn ask_to_stop_transcript(&self, identity: &str) {
        self.send_transcript_to(identity, TranscriptMessage::Stop);
    }

    /// Ask `identity`'s Mac to transcribe the call; `handover` if yours does
    /// now. [`Event::TranscriptAnswered`] follows, unless they just start.
    pub fn ask_to_transcribe(&self, identity: &str, handover: bool) {
        let claim = {
            let mut s = self.state.lock();
            let claim = (handover && s.transcribing.is_some()).then_some(s.transcribing_since);
            if claim.is_some() && !s.handover_asked.iter().any(|a| a == identity) {
                s.handover_asked.push(identity.to_owned());
            }
            claim
        };
        self.send_transcript_to(identity, TranscriptMessage::Ask { handover, claim });
    }

    /// Answer `identity`'s ask.
    pub fn answer_transcript_ask(&self, identity: &str, accepted: bool) {
        self.send_transcript_to(identity, TranscriptMessage::Answer { accepted });
    }

    /// Tell everyone your Mac could transcribe the call with `model` (it's
    /// downloaded), or that it can't. Kept for the next call too.
    #[cfg(feature = "transcribe")]
    pub fn set_can_transcribe(&self, model: Option<&'static ModelInfo>) {
        let value = model.map(|m| m.id.to_owned());
        let room = {
            let mut s = self.state.lock();
            if s.can_transcribe == value {
                return;
            }
            s.can_transcribe = value.clone();
            s.room.clone()
        };
        if let Some(room) = room {
            self.tasks.spawn(async move {
                let value = value.as_deref().unwrap_or("");
                if let Err(e) = set_attribute(&room, CAN_TRANSCRIBE, value).await {
                    tracing::warn!("can-transcribe attribute: {e}");
                }
            });
        }
    }

    fn send_transcript_to(&self, identity: &str, message: TranscriptMessage) {
        let Some(room) = self.state.lock().room.clone() else {
            return;
        };
        let to = identity.to_owned();
        self.tasks.spawn(async move {
            if let Err(e) = send_transcript(&room, &message, Some(to)).await {
                tracing::warn!("transcript message {message:?}: {e}");
            }
        });
    }
}

async fn stop_transcribing(
    state: &Arc<Mutex<State>>,
    events: &async_channel::Sender<Event>,
    by: Option<String>,
) {
    stop_transcribing_for(state, events, by, false).await
}

async fn stop_transcribing_for(
    state: &Arc<Mutex<State>>,
    events: &async_channel::Sender<Event>,
    by: Option<String>,
    handed_over: bool,
) {
    let (room, transcribing, pass_on) = {
        let mut s = state.lock();
        let transcribing = s.transcribing.take();
        // Stopped means stopped: nothing more is heard, from now, before
        // anything goes over the network.
        if let Some(transcribing) = &transcribing {
            transcribing.stop_listening();
        }
        let pass_on = if handed_over {
            took_over(&mut s, by.as_deref());
            Vec::new()
        } else {
            // Stop wins over a start on its way: it won't listen (see
            // `install_transcribing`).
            if transcribing.is_none()
                && let Start::Going(id) = s.start
            {
                s.start = Start::Stopped(id, by.clone());
            }
            // And over any Mac taking over from yours.
            stop_passes_on(&mut s, by.as_deref())
        };
        (s.room.clone(), transcribing, pass_on)
    };
    // Sending can wait on the connection: it holds nothing up.
    if let Some(room) = room.clone().filter(|_| !pass_on.is_empty()) {
        tasks::spawn(async move {
            for to in pass_on {
                tracing::info!(%to, "the transcript stopped: so does its handover");
                if let Err(e) = send_transcript(&room, &TranscriptMessage::Stop, Some(to)).await {
                    tracing::warn!("passing on Stop: {e}");
                }
            }
        });
    }
    let Some(transcribing) = transcribing else {
        return;
    };
    // The claim goes before the last words are out, so no Mac takes over
    // meanwhile. The last words still count (see `LAST_WORDS`).
    if let Some(room) = &room
        && let Err(e) = set_attribute(room, TRANSCRIBING, "").await
    {
        tracing::warn!("transcribing attribute: {e}");
    }
    let _ = tokio::task::spawn_blocking(move || transcribing.stop()).await;
    // They may have stopped while your last words went out: then nobody
    // carries on.
    let handed_over = handed_over
        && room.as_ref().zip(by.as_deref()).is_some_and(|(room, by)| {
            room.remote_participants()
                .get(&ParticipantIdentity(by.to_owned()))
                .is_some_and(transcribes)
        });
    let _ = events
        .send(Event::TranscriptionStopped { by, handed_over })
        .await;
}

/// Someone else claimed the transcript while yours was starting (two people
/// pressing Transcribe at once): their claim came before yours was in place,
/// so the room's events let it by. Settle it the way both Macs agree on.
async fn settle_transcriber(state: &Arc<Mutex<State>>, events: &async_channel::Sender<Event>) {
    let (room, since) = {
        let s = state.lock();
        match (&s.room, &s.transcribing) {
            (Some(room), Some(_)) => (room.clone(), s.transcribing_since),
            _ => return,
        }
    };
    let you = room.local_participant().identity().to_string();
    let newest = newest_claim(&room, &state.lock().mine);
    if let Some((theirs, identity)) = newest
        && yields_to((since, &you), (theirs, &identity))
    {
        tracing::info!(%identity, "their Mac started at the same time and carries on");
        stop_transcribing_for(state, events, Some(identity), true).await;
    }
}

/// The most a transcript message may take: LiveKit's limit for a reliable
/// data packet is 15 KiB, and the packet's own fields need some of it.
const MESSAGE_BYTES: usize = 14_000;

/// A transcript message to everyone, or to one person.
async fn send_transcript(
    room: &Room,
    message: &TranscriptMessage,
    to: Option<String>,
) -> anyhow::Result<()> {
    send_encoded(room, serde_json::to_vec(message)?, to).await
}

async fn send_encoded(room: &Room, payload: Vec<u8>, to: Option<String>) -> anyhow::Result<()> {
    room.local_participant()
        .publish_data(DataPacket {
            payload,
            topic: Some(TRANSCRIPT_TOPIC.to_owned()),
            reliable: true,
            destination_identities: to.into_iter().map(ParticipantIdentity).collect(),
        })
        .await?;
    Ok(())
}

/// The lines so far, to someone who just joined. One message that doesn't
/// go through doesn't hold back the rest.
async fn send_history(room: &Room, lines: Vec<TranscriptLine>, to: String) {
    for payload in history_messages(&lines) {
        if let Err(e) = send_encoded(room, payload, Some(to.clone())).await {
            tracing::warn!("transcript history to {to}: {e}");
        }
    }
}

/// `lines` as encoded messages of as many lines as fit in
/// [`MESSAGE_BYTES`], measured as sent: a line of Cyrillic or Chinese takes
/// two or three times its length in characters.
fn history_messages(lines: &[TranscriptLine]) -> Vec<Vec<u8>> {
    let encode = |lines: &[TranscriptLine]| {
        let message = TranscriptMessage::Lines {
            lines: lines.to_vec(),
        };
        serde_json::to_vec(&message).unwrap_or_default()
    };
    let empty = encode(&[]).len();
    let mut messages = Vec::new();
    let (mut first, mut bytes) = (0, empty);
    for (i, line) in lines.iter().enumerate() {
        // Each line, and a comma before it.
        let size = serde_json::to_vec(line).map_or(0, |l| l.len()) + 1;
        if i > first && bytes + size > MESSAGE_BYTES {
            messages.push(encode(&lines[first..i]));
            (first, bytes) = (i, empty);
        }
        bytes += size;
    }
    if first < lines.len() {
        messages.push(encode(&lines[first..]));
    }
    messages
}

/// A transcript message from someone in the call.
async fn on_transcript(
    state: &Arc<Mutex<State>>,
    events: &async_channel::Sender<Event>,
    from: &RemoteParticipant,
    payload: &[u8],
) {
    let by = from.identity().to_string();
    match serde_json::from_slice::<TranscriptMessage>(payload) {
        Ok(TranscriptMessage::Lines { lines }) => {
            // Only a Mac that transcribes the call speaks for it: anyone can
            // send on the topic, and put words in anyone's mouth. Which Mac
            // that is, everyone sees. It's an attribute anyone may set, so
            // a lease from the server will make it exclusive.
            let trusted = transcribes(from)
                || state
                    .lock()
                    .stopped_transcribing
                    .get(&by)
                    .is_some_and(|at| at.elapsed() < LAST_WORDS);
            if !trusted {
                tracing::warn!(%by, "transcript lines from a Mac that doesn't transcribe: dropped");
                return;
            }
            let lines = take_lines(&mut state.lock().transcribed, &by, lines);
            for (line, relayed) in lines {
                let by = line.by.clone().unwrap_or_default();
                let _ = events
                    .send(Event::TranscriptLine { by, line, relayed })
                    .await;
            }
        }
        Ok(TranscriptMessage::Stop) => {
            tracing::info!(%by, "asked to stop the transcript");
            // The last words take a moment; room events go on meanwhile.
            let (state, events) = (state.clone(), events.clone());
            tasks::spawn(async move { stop_transcribing(&state, &events, Some(by)).await });
        }
        Ok(TranscriptMessage::Ask { handover, claim }) => {
            // Older apps don't say which claim they hand over: the one they
            // have as the ask comes, so a restart since still calls it off.
            let claim = claim.or_else(|| {
                handover
                    .then(|| claim_of(from).map(|(since, _)| since))
                    .flatten()
            });
            tracing::info!(%by, handover, ?claim, "asked to transcribe");
            let _ = events
                .send(Event::TranscriptAsked {
                    from: by,
                    handover,
                    claim,
                })
                .await;
        }
        Ok(TranscriptMessage::Answer { accepted }) => {
            let _ = events
                .send(Event::TranscriptAnswered { from: by, accepted })
                .await;
        }
        Err(e) => tracing::warn!(%by, "transcript message: {e}"),
    }
}

/// Start `id` ([`reserve_start`]) in `room`, the call `connection`.
#[cfg(feature = "transcribe")]
async fn start_transcribing(
    state: &Arc<Mutex<State>>,
    handle: &tasks::Tasks,
    events: &async_channel::Sender<Event>,
    options: TranscribeOptions,
    room: Arc<Room>,
    connection: u64,
    id: u64,
) -> anyhow::Result<()> {
    // Taking over: their claim, the one they asked with, which must still
    // stand once the model has loaded. They may stop meanwhile, or someone
    // else start.
    let taking_over = match &options.handover {
        Some(Handover { from, claim }) => {
            let theirs = room
                .remote_participants()
                .get(&ParticipantIdentity(from.clone()))
                .and_then(claim_of)
                .filter(|(since, _)| *claim == Some(*since))
                .context("they stopped transcribing since they asked")?;
            Some(theirs)
        }
        None => {
            nobody_transcribes(&room, &state.lock().mine)?;
            None
        }
    };
    let model = options.model;
    let stt = cubie_stt::Options {
        model: models::path(model, &options.dir),
        vad: models::path(&catalog::VAD, &options.dir),
        language: options.language,
    };
    let you = room.local_participant().identity().to_string();
    // Each line shows here and goes to everyone, in order.
    let (outgoing, mut to_send) = tokio::sync::mpsc::unbounded_channel::<TranscriptLine>();
    let lines = events.clone();
    let by = you.clone();
    let on_line = move |line: cubie_stt::Line| {
        let line = TranscriptLine {
            id: line.id,
            speaker: line.speaker,
            at_ms: line
                .at
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            text: line.text,
            done: line.done,
            by: Some(by.clone()),
        };
        let _ = outgoing.send(line.clone());
        let _ = lines.try_send(Event::TranscriptLine {
            by: by.clone(),
            line,
            relayed: false,
        });
    };
    {
        let state_for_delivery = state.clone();
        let (room, state) = (room.clone(), state.clone());
        let delivery = handle.spawn(async move {
            while let Some(line) = to_send.recv().await {
                // The last words after you left still go to that call, but
                // into no other's history.
                {
                    let mut s = state.lock();
                    if s.connection == connection {
                        remember(&mut s.transcribed, &line, false);
                    }
                }
                let message = TranscriptMessage::Lines { lines: vec![line] };
                if let Err(e) = send_transcript(&room, &message, None).await {
                    tracing::warn!("sending a transcript line: {e}");
                }
            }
        });
        let mut s = state_for_delivery.lock();
        if s.connection == connection {
            s.transcript_sends
                .retain(|task| task.clone().now_or_never().is_none());
            s.transcript_sends
                .push(delivery.map(|_| ()).boxed().shared());
        }
    }
    let transcriber =
        tokio::task::spawn_blocking(move || cubie_stt::Transcriber::start(stt, on_line)).await??;
    // Loading takes seconds: is it still yours to start?
    if let Err(e) = may_start(state, &room, connection, id, taking_over.as_ref()) {
        let _ = tokio::task::spawn_blocking(move || transcriber.stop()).await;
        return Err(e);
    }
    // Everyone sees it before a single word is heard. Later than every
    // claim you know of, so taking over wins even if your clock is behind
    // theirs.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let since = now.max(newest_claim(&room, &state.lock().mine).map_or(0, |(since, _)| since + 1));
    state.lock().transcribing_since = since;
    if let Err(e) = set_attribute(&room, TRANSCRIBING, &format!("{}@{since}", model.id)).await {
        let _ = tokio::task::spawn_blocking(move || transcriber.stop()).await;
        return Err(e.context("couldn't let the others know, so nothing is transcribed"));
    }
    // Only now does it listen: unless the call ended, or Stop came, while
    // everyone was told. Then nothing was heard, and the claim goes again.
    let mut transcriber = Some(transcriber);
    let installed = install_transcribing(state, connection, id, handle, |listener| {
        transcribe::Transcribing::start(listener, transcriber.take().expect("once"), you)
    });
    if let Err(e) = installed {
        if let Err(e) = set_attribute(&room, TRANSCRIBING, "").await {
            tracing::warn!("transcribing attribute: {e}");
        }
        if let Some(transcriber) = transcriber {
            let _ = tokio::task::spawn_blocking(move || transcriber.stop()).await;
        }
        return Err(e);
    }
    Ok(())
}

/// A start of your transcription: its number, unless one is on its way or
/// yours already runs.
fn reserve_start(s: &mut State) -> Option<u64> {
    if s.transcribing.is_some() || s.start != Start::No {
        return None;
    }
    s.starts += 1;
    s.start = Start::Going(s.starts);
    s.handover_asked.clear();
    s.handed_to = None;
    Some(s.starts)
}

/// Whether start `id` may go on: its call (`connection`) goes on, and Stop
/// hasn't come.
fn start_goes_on(s: &State, connection: u64, id: u64) -> anyhow::Result<()> {
    anyhow::ensure!(s.connection == connection, "call ended");
    match &s.start {
        Start::Going(own) if *own == id => Ok(()),
        Start::Stopped(own, by) if *own == id => Err(StoppedStarting(by.clone()).into()),
        _ => anyhow::bail!("call ended"),
    }
}

/// Start `id` is over, one way or another; another start is left alone.
fn finish_start(s: &mut State, id: u64) {
    if matches!(s.start, Start::Going(own) | Start::Stopped(own, _) if own == id) {
        s.start = Start::No;
    }
}

/// `by` took over your transcript: a Stop just after goes on to it. The
/// others you asked still get it too (see [`State::handover_asked`]).
fn took_over(s: &mut State, by: Option<&str>) {
    if let Some(by) = by {
        s.handed_to = Some((by.to_owned(), Instant::now()));
    }
}

/// Who a Stop (from `by`) goes on to: every Mac you asked to take over, and
/// the one that just did.
fn stop_passes_on(s: &mut State, by: Option<&str>) -> Vec<String> {
    let mut to = std::mem::take(&mut s.handover_asked);
    if let Some((handed, at)) = s.handed_to.take()
        && at.elapsed() < LAST_WORDS
        && !to.contains(&handed)
    {
        to.push(handed);
    }
    to.retain(|t| Some(t.as_str()) != by);
    to
}

/// Your transcription starts listening: everyone's voice goes to `make`'s
/// transcriber from now. Not if the call ended, or Stop came while it got
/// ready; then `make` isn't called, and nothing is heard. Under one lock
/// with Stop, so it's one or the other.
#[cfg_attr(not(feature = "transcribe"), allow(unreachable_code))]
fn install_transcribing(
    state: &Arc<Mutex<State>>,
    connection: u64,
    id: u64,
    handle: &tasks::Tasks,
    make: impl FnOnce(Listener) -> transcribe::Transcribing,
) -> anyhow::Result<()> {
    let weak = Arc::downgrade(state);
    let mut s = state.lock();
    start_goes_on(&s, connection, id)?;
    let listener = listen_to(&mut s, weak, handle);
    s.transcribing = Some(make(listener));
    Ok(())
}

/// Whether your Mac may still start transcribing, now its model has loaded:
/// the call goes on, and whoever you take over from (`taking_over`, their
/// claim) still transcribes, with nobody newer; or nobody does.
fn may_start(
    state: &Mutex<State>,
    room: &Room,
    connection: u64,
    id: u64,
    taking_over: Option<&(u64, String)>,
) -> anyhow::Result<()> {
    start_goes_on(&state.lock(), connection, id)?;
    let mine = state.lock().mine.clone();
    match taking_over {
        Some(theirs) => anyhow::ensure!(
            newest_claim(room, &mine).as_ref() == Some(theirs),
            "the transcript stopped or moved on while your Mac got ready"
        ),
        None => nobody_transcribes(room, &mine)?,
    }
    Ok(())
}

/// A listener to this call; remote microphones start decoding for it.
fn listen(state: &Arc<Mutex<State>>, handle: &tasks::Tasks) -> Listener {
    let weak = Arc::downgrade(state);
    listen_to(&mut state.lock(), weak, handle)
}

fn listen_to(
    s: &mut State,
    weak: std::sync::Weak<Mutex<State>>,
    handle: &tasks::Tasks,
) -> Listener {
    let listener = s.hub.listen(move || {
        if let Some(state) = weak.upgrade() {
            let mut s = state.lock();
            // Someone may have started listening again meanwhile.
            if !s.hub.listening() {
                for (_, task) in s.mic_streams.drain() {
                    task.abort();
                }
            }
        }
    });
    for (key, track) in &s.mics {
        if !s.mic_streams.contains_key(key) {
            let pump = pump_mic(key.participant.clone(), track.clone(), s.hub.clone());
            s.mic_streams.insert(key.clone(), handle.spawn(pump));
        }
    }
    listener
}

/// One remote microphone, decoded at 48 kHz mono, to everyone listening in
/// its call.
async fn pump_mic(identity: String, track: RemoteAudioTrack, hub: Arc<audio_tap::Hub>) {
    use livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
    // Two seconds of queue: the decoder never waits for us.
    let mut stream = NativeAudioStream::with_options(
        track.rtc_track(),
        audio_tap::SAMPLE_RATE as i32,
        1,
        NativeAudioStreamOptions {
            queue_size_frames: Some(200),
        },
    );
    let speaker = Speaker::Remote(identity);
    while let Some(frame) = stream.next().await {
        let samples = frame.data.iter().map(|&s| s as f32 / 32768.0).collect();
        hub.send(
            VoiceFrame {
                speaker: speaker.clone(),
                samples,
            },
            true,
        );
    }
}

async fn start_recording(
    state: &Arc<Mutex<State>>,
    handle: &tasks::Tasks,
    events: &async_channel::Sender<Event>,
    path: PathBuf,
    screen: Option<(SourceId, ScreenSharePreset)>,
    each_voice: bool,
) -> anyhow::Result<PathBuf> {
    let room = state.lock().room.clone().context("not in a call")?;
    anyhow::ensure!(state.lock().recorder.is_none(), "you're already recording");
    // "Design review 2026-11-10 10.02 voices/Anna Petrova.m4a".
    let voices = each_voice.then(|| {
        let stem = path.file_stem().map_or_else(
            || "Recording".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let weak = Arc::downgrade(&room);
        let name_of: record::NameOf = Arc::new(move |speaker: &Speaker| {
            let Some(room) = weak.upgrade() else {
                return "Someone".to_owned();
            };
            match speaker {
                Speaker::Local => room.local_participant().name(),
                Speaker::Remote(identity) => room
                    .remote_participants()
                    .get(&ParticipantIdentity(identity.clone()))
                    .map(|p| p.name())
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| identity.clone()),
            }
        });
        record::Voices {
            dir: path.with_file_name(format!("{stem} voices")),
            name_of,
        }
    });
    let mine = state.lock().mine.clone();
    if let Some(p) = room.remote_participants().values().find(|p| {
        p.attributes().get(RECORDING).is_some_and(|v| !v.is_empty())
            && !mine.contains(p.identity().as_str())
    }) {
        anyhow::bail!("{} is already recording this call", p.name());
    }
    // Everyone sees it before a single frame is kept.
    let kind = if screen.is_some() { "screen" } else { "audio" };
    set_attribute(&room, RECORDING, kind)
        .await
        .context("couldn't let the others know, so nothing is recorded")?;
    // Nothing is captured for a call that's over: nobody would see it.
    let still = || -> anyhow::Result<()> {
        anyhow::ensure!(is_current(&state.lock(), &room), "call ended");
        Ok(())
    };
    let screen = match async {
        still()?;
        let screen = capture_for_recording(screen).await?;
        still()?;
        Ok(screen)
    }
    .await
    {
        Ok(screen) => screen,
        Err(e) => {
            let _ = set_attribute(&room, RECORDING, "").await;
            return Err(e);
        }
    };
    let listener = listen(state, handle);
    // Writing can fail later (the encoder gave up): say so at once, and
    // stop telling everyone that you record.
    let on_failed = {
        let (state, events, handle) = (Arc::downgrade(state), events.clone(), handle.clone());
        let path = path.clone();
        move || {
            handle.spawn(async move {
                let Some(state) = state.upgrade() else { return };
                // This recording, if it's still the one: a call joined
                // again may have one of its own.
                let (room, recorder) = {
                    let mut s = state.lock();
                    let recorder = s.recorder.take_if(|r| r.path() == path);
                    (s.room.clone(), recorder)
                };
                let Some(recorder) = recorder else { return };
                let result = tokio::task::spawn_blocking(move || recorder.stop()).await;
                if let Some(room) = room {
                    let _ = set_attribute(&room, RECORDING, "").await;
                }
                let message = match result {
                    Ok(Err(e)) => format!("{e:#}"),
                    _ => "the recording stopped".into(),
                };
                let path = Some(path);
                let _ = events.send(Event::RecordingFailed { message, path }).await;
            });
        }
    };
    let started = tokio::task::spawn_blocking(move || {
        record::Recorder::start(listener, path, screen, voices, on_failed)
    })
    .await;
    let recorder = match started {
        Ok(Ok(recorder)) => recorder,
        Ok(Err(e)) => {
            let _ = set_attribute(&room, RECORDING, "").await;
            return Err(e);
        }
        Err(e) => {
            let _ = set_attribute(&room, RECORDING, "").await;
            return Err(e.into());
        }
    };
    let path = recorder.path().to_owned();
    // The call may have ended while the file was being created.
    let stale = {
        let mut s = state.lock();
        if is_current(&s, &room) {
            s.recorder = Some(recorder);
            None
        } else {
            Some(recorder)
        }
    };
    if let Some(recorder) = stale {
        let _ = tokio::task::spawn_blocking(move || recorder.stop()).await;
        anyhow::bail!("call ended");
    }
    Ok(path)
}

/// Tell everyone (or stop telling them) that you record or transcribe.
/// LiveKit deletes an attribute set to "".
async fn set_attribute(room: &Room, key: &str, value: &str) -> anyhow::Result<()> {
    room.local_participant()
        .set_attributes(HashMap::from([(key.to_owned(), value.to_owned())]))
        .await?;
    Ok(())
}

/// Whether `room` is still the call: not left, not lost.
fn is_current(s: &State, room: &Arc<Room>) -> bool {
    s.room.as_ref().is_some_and(|r| Arc::ptr_eq(r, room))
}

/// Whether `participant`'s Mac transcribes the call.
fn transcribes(participant: &RemoteParticipant) -> bool {
    participant
        .attributes()
        .get(TRANSCRIBING)
        .is_some_and(|v| !v.is_empty())
}

/// A [`TRANSCRIBING`] value: the model's id, and when it started.
fn parse_transcribing(value: &str) -> (&str, u64) {
    match value.split_once('@') {
        Some((model, since)) => (model, since.parse().unwrap_or(0)),
        None => (value, 0),
    }
}

/// `participant`'s [`TRANSCRIBING`] claim, if their Mac transcribes: when,
/// and whose.
fn claim_of(participant: &RemoteParticipant) -> Option<(u64, String)> {
    let attributes = participant.attributes();
    let value = attributes.get(TRANSCRIBING).filter(|v| !v.is_empty())?;
    Some((
        parse_transcribing(value).1,
        participant.identity().to_string(),
    ))
}

/// The newest [`TRANSCRIBING`] claim among the others (not you, as you
/// were before joining again: `mine`).
fn newest_claim(room: &Room, mine: &HashSet<String>) -> Option<(u64, String)> {
    room.remote_participants()
        .values()
        .filter(|p| !mine.contains(p.identity().as_str()))
        .filter_map(claim_of)
        .max()
}

/// One transcriber per call, unless it hands over.
fn nobody_transcribes(room: &Room, mine: &HashSet<String>) -> anyhow::Result<()> {
    match room
        .remote_participants()
        .values()
        .find(|p| transcribes(p) && !mine.contains(p.identity().as_str()))
    {
        Some(p) => anyhow::bail!("{} is already transcribing this call", p.name()),
        None => Ok(()),
    }
}

/// Lines `by` sent, as they're shown, and whether each was relayed: each
/// says which Mac transcribed it (older apps leave it out: then it's the
/// sender's). Another Mac's line, relayed (history after a handover), only
/// adds what you don't have: it never changes or takes back a line.
fn take_lines(
    kept: &mut Vec<Kept>,
    by: &str,
    lines: Vec<TranscriptLine>,
) -> Vec<(TranscriptLine, bool)> {
    lines
        .into_iter()
        .filter_map(|mut line| {
            let relayed = line.by.get_or_insert_with(|| by.to_owned()) != by;
            (remember(kept, &line, relayed) || !relayed).then_some((line, relayed))
        })
        .collect()
}

/// A final line, kept for whoever joins later.
#[derive(Clone, Debug)]
struct Kept {
    line: TranscriptLine,
    /// It came from another Mac than the one that transcribed it, which
    /// anyone claiming the transcript can be: it stands only until that
    /// Mac's own final.
    relayed: bool,
}

/// A final line, kept once however it came (from the Mac that transcribed
/// it, or `relayed` by the next one). That Mac's own final replaces a
/// relayed one, or takes it back; nothing else changes a kept line. Whether
/// anything changed.
fn remember(kept: &mut Vec<Kept>, line: &TranscriptLine, relayed: bool) -> bool {
    if !line.done {
        return false;
    }
    let same = kept
        .iter()
        .rposition(|k| k.line.by == line.by && k.line.id == line.id);
    match same {
        Some(at) if kept[at].relayed && !relayed => {
            if line.text.is_empty() {
                kept.remove(at);
            } else {
                kept[at] = Kept {
                    line: line.clone(),
                    relayed,
                };
            }
            true
        }
        Some(_) => false,
        None if line.text.is_empty() => false,
        None => {
            kept.push(Kept {
                line: line.clone(),
                relayed,
            });
            true
        }
    }
}

/// Someone else started transcribing while your Mac does. The one who
/// started last carries on: a handover, or two people pressing Transcribe at
/// once (both Macs agree on who stops).
fn yields_to(mine: (u64, &str), theirs: (u64, &str)) -> bool {
    theirs > mine
}

/// What a participant's attributes say they do, for everyone's UI. `changed`
/// limits it to those attributes (an update); `None` reports all (they
/// joined).
fn attribute_events(
    identity: &str,
    attributes: &HashMap<String, String>,
    changed: Option<&HashMap<String, String>>,
) -> Vec<Event> {
    let value = |key: &str| -> Option<&str> {
        match changed {
            Some(changed) => changed
                .contains_key(key)
                .then(|| changed.get(key).map_or("", String::as_str)),
            None => attributes
                .get(key)
                .map(String::as_str)
                .filter(|v| !v.is_empty()),
        }
    };
    let mut events = Vec::new();
    if let Some(v) = value(RECORDING) {
        events.push(Event::ParticipantRecording {
            identity: identity.to_owned(),
            recording: !v.is_empty(),
            screen: v == "screen",
        });
    }
    if let Some(v) = value(TRANSCRIBING) {
        events.push(Event::ParticipantTranscribing {
            identity: identity.to_owned(),
            model: (!v.is_empty()).then(|| parse_transcribing(v).0.to_owned()),
        });
    }
    if let Some(v) = value(CAN_TRANSCRIBE) {
        events.push(Event::ParticipantCanTranscribe {
            identity: identity.to_owned(),
            model: (!v.is_empty()).then(|| v.to_owned()),
        });
    }
    events
}

/// Your screen or a window for a recording, captured on its own.
#[cfg(target_os = "macos")]
async fn capture_for_recording(
    screen: Option<(SourceId, ScreenSharePreset)>,
) -> anyhow::Result<Option<record::ScreenInput>> {
    let Some((source, preset)) = screen else {
        return Ok(None);
    };
    let (tx, frames) = std::sync::mpsc::sync_channel(8);
    let sink = Arc::new(record::FrameQueue(tx));
    let on_stop = move || tracing::info!(?source, "the recorded screen or window went away");
    let (capture, (width, height)) = screen_mac::record(source, preset, sink, on_stop)
        .await
        .context("screen capture")?;
    Ok(Some(record::ScreenInput {
        frames,
        format: record::VideoFormat {
            width,
            height,
            fps: preset.fps,
            bitrate: preset.recording_bitrate(),
        },
        capture,
    }))
}

#[cfg(not(target_os = "macos"))]
async fn capture_for_recording(
    screen: Option<(SourceId, ScreenSharePreset)>,
) -> anyhow::Result<Option<record::ScreenInput>> {
    anyhow::ensure!(screen.is_none(), "recording the screen needs macOS for now");
    Ok(None)
}

/// Runs `f` inside an Objective-C autorelease pool on Apple platforms.
///
/// libwebrtc's Objective-C frame wrappers are autoreleased. Our own threads
/// (tokio workers, the synthetic camera) have no pool that ever drains, so
/// without this every video frame, and the IOSurface behind it, leaks.
pub(crate) fn autorelease_pool<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(target_vendor = "apple")]
    {
        objc2::rc::autoreleasepool(|_| f())
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        f()
    }
}

/// Falling back after thread creation failure must not drop a Runtime inside
/// an async context (which would panic again during Engine::drop).
struct ShutdownRuntime {
    runtime: Option<tokio::runtime::Runtime>,
    tasks: tasks::Tasks,
}
impl ShutdownRuntime {
    fn take(&mut self) -> tokio::runtime::Runtime {
        self.runtime.take().expect("runtime owned")
    }
}
impl Drop for ShutdownRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            self.tasks.cancel();
            runtime.shutdown_background();
        }
    }
}
fn retire_runtime(
    runtime: tokio::runtime::Runtime,
    tasks: tasks::Tasks,
    finish: impl Future<Output = ()> + Send + 'static,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut owned = ShutdownRuntime {
        runtime: Some(runtime),
        tasks,
    };
    spawn(Box::new(move || {
        let runtime = owned.take();
        runtime.block_on(finish);
        runtime.shutdown_timeout(Duration::from_secs(1));
    }))
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.disconnect();
        let closed = self.closed();
        let tasks = self.tasks.clone();
        let runtime = self.runtime.take().expect("engine runtime initialized");
        let finish = async move {
            if tokio::time::timeout(CLOSE_TIMEOUT, closed).await.is_err() {
                tracing::warn!("leaving the room timed out");
            }
            tasks.shutdown().await;
        };
        if let Err(e) = retire_runtime(runtime, self.tasks.clone(), finish, |close| {
            std::thread::Builder::new()
                .name("cubie-close".into())
                .spawn(close)
                .map(|_| ())
        }) {
            tracing::warn!("could not start call teardown thread: {e}");
        }
    }
}

/// Stop your screen share; only if it's still `connection`'s, if given.
async fn stop_screen_share(
    state: Arc<Mutex<State>>,
    events: async_channel::Sender<Event>,
    connection: Option<u64>,
) {
    let (room, share) = {
        let mut s = state.lock();
        if connection.is_some_and(|c| c != s.connection) {
            return;
        }
        (s.room.clone(), s.screen.take())
    };
    if let (Some(room), Some(share)) = (room, share) {
        share.unpublish(&room).await;
    }
    let _ = events
        .send(Event::ScreenShareChanged {
            active: false,
            preset: None,
        })
        .await;
}

/// Skip the physical audio devices, including for test launches from before the rename.
pub fn no_mic() -> bool {
    std::env::var_os("CUBIE_NO_MIC").is_some() || std::env::var_os("QUIPCALL_NO_MIC").is_some()
}

/// Ask before admission or opening WebRTC's audio device. Calling again after
/// join only reads an answered permission; direct connect uses the same policy.
async fn microphone_permission(events: &async_channel::Sender<Event>) -> anyhow::Result<bool> {
    if no_mic() {
        return Ok(false);
    }
    if permissions::status(MediaDevice::Microphone) == Access::Undecided {
        events
            .send(Event::WaitingForPermission(MediaDevice::Microphone))
            .await?;
    }
    Ok(permissions::request(MediaDevice::Microphone).await)
}

async fn connect(
    state: Arc<Mutex<State>>,
    events: async_channel::Sender<Event>,
    join: JoinInfo,
    connection: u64,
) -> anyhow::Result<()> {
    // CUBIE_NO_MIC=1 skips the audio device entirely (headless tests, CI).
    let use_audio = !no_mic();

    // Ask before anything opens the mic: an unanswered prompt would block
    // WebRTC's audio thread. Without access we still play remote audio.
    let mic_allowed = microphone_permission(&events).await?;
    if use_audio && !mic_allowed {
        events
            .send(Event::Error(format!(
                "microphone access is off, so others can't hear you. Allow it in {}, then rejoin",
                permissions::where_to_allow(MediaDevice::Microphone)
            )))
            .await?;
    }

    // Load the noise model while the room connects; the microphone track
    // must wait for it, since it fixes WebRTC's own suppression on creation.
    #[cfg(feature = "denoise")]
    let denoise = mic_allowed.then(|| tokio::task::spawn_blocking(denoise::prepare));

    // The platform ADM has to exist before the peer connections are created.
    let audio = if use_audio {
        let audio = PlatformAudio::new().context("audio device")?;
        audio.configure_audio_processing(AudioProcessingOptions {
            echo_cancellation: true,
            noise_suppression: true,
            auto_gain_control: true,
            prefer_hardware_processing: true,
        })?;
        Some(audio)
    } else {
        tracing::warn!("CUBIE_NO_MIC set: no microphone, no speaker");
        None
    };

    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    // Adaptive stream in the Rust SDK only makes the server start at the lowest
    // layer; it never reports element sizes. We pick layers per tile instead.
    options.adaptive_stream = false;
    options.dynacast = true;
    let (room, mut room_events) = Room::connect(&join.url, &join.token, options).await?;
    let room = Arc::new(room);
    tracing::info!(
        room = %room.name(),
        identity = %room.local_participant().identity(),
        "connected"
    );

    // Left, or connected again, meanwhile: this one is no longer wanted.
    let can_transcribe = {
        let mut s = state.lock();
        if s.connection == connection {
            s.room = Some(room.clone());
            s.audio = audio.clone();
            s.mine
                .insert(room.local_participant().identity().to_string());
            Some((
                s.can_transcribe.clone(),
                std::mem::replace(&mut s.joined_before, true),
            ))
        } else {
            None
        }
    };
    let Some((can_transcribe, again)) = can_transcribe else {
        tracing::info!(room = %room.name(), "a connection overtaken by another: closing it");
        let _ = room.close().await;
        return Ok(());
    };
    // Others can ask this Mac to transcribe.
    if let Some(model) = can_transcribe {
        let room = room.clone();
        tasks::spawn(async move {
            if let Err(e) = set_attribute(&room, CAN_TRANSCRIBE, &model).await {
                tracing::warn!("can-transcribe attribute: {e}");
            }
        });
    }
    let identity = room.local_participant().identity().to_string();
    let others = room
        .remote_participants()
        .values()
        .map(|p| p.identity().to_string())
        .collect();
    events
        .send(Event::Connected {
            room: room.name(),
            identity,
            others,
        })
        .await?;
    for p in room.remote_participants().values() {
        events
            .send(Event::ParticipantJoined {
                identity: p.identity().to_string(),
                name: p.name(),
            })
            .await?;
        // Joining again, what isn't set any more counts too: they may have
        // stopped recording meanwhile.
        let attributes = p.attributes();
        let every = again.then(|| {
            [RECORDING, TRANSCRIBING, CAN_TRANSCRIBE]
                .into_iter()
                .map(|key| {
                    let value = attributes.get(key).cloned().unwrap_or_default();
                    (key.to_owned(), value)
                })
                .collect::<HashMap<_, _>>()
        });
        for event in attribute_events(&p.identity().to_string(), &attributes, every.as_ref()) {
            events.send(event).await?;
        }
    }

    // Publishing opens the mic and can take a moment; don't hold up room events.
    if let Some(audio) = audio.filter(|_| mic_allowed) {
        let (room, state, events) = (room.clone(), state.clone(), events.clone());
        tasks::spawn(async move {
            #[cfg(feature = "denoise")]
            if let Some(denoise) = denoise
                && let Err(e) = denoise.await
            {
                tracing::error!("noise suppression: {e}");
            }
            let mic = LocalAudioTrack::create_audio_track("microphone", audio.rtc_source());
            // Muted before it's published, if that was asked for: the server
            // hears of a muted track, and no sound goes out first.
            if state.lock().mic_opens_muted {
                mic.mute();
            }
            let options = TrackPublishOptions {
                source: TrackSource::Microphone,
                dtx: true,
                red: true,
                ..Default::default()
            };
            match room
                .local_participant()
                .publish_track(LocalTrack::Audio(mic.clone()), options)
                .await
            {
                Ok(_) => {
                    let enabled = {
                        let mut s = state.lock();
                        is_current(&s, &room).then(|| {
                            // Asked for while it was being published: now.
                            let enabled = !s.mic_opens_muted;
                            if enabled {
                                mic.unmute();
                            } else {
                                mic.mute();
                            }
                            s.mic = Some(mic);
                            audio_tap::set_mic_live(&s.hub, enabled);
                            enabled
                        })
                    };
                    if let Some(enabled) = enabled {
                        let _ = events.send(Event::MicChanged { enabled }).await;
                    }
                }
                Err(e) => {
                    let _ = events.send(Event::Error(format!("microphone: {e}"))).await;
                }
            }
        });
    }

    while let Some(event) = room_events.recv().await {
        // You left (or joined another call on this engine): the rest of
        // this room's events are no one's business.
        if state.lock().connection != connection {
            break;
        }
        if let RoomEvent::TrackMuted {
            participant,
            publication,
        }
        | RoomEvent::TrackUnmuted {
            participant,
            publication,
        } = &event
        {
            tracing::debug!(
                participant = %participant.identity(),
                track = %publication.sid(),
                muted = publication.is_muted(),
                "track mute changed"
            );
        }
        track_publications(&state, connection, &event);
        // Someone joining mid-recording, or starting or stopping.
        let attributes = match &event {
            RoomEvent::ParticipantConnected(p) => {
                attribute_events(&p.identity().to_string(), &p.attributes(), None)
            }
            RoomEvent::ParticipantAttributesChanged {
                participant: Participant::Remote(p),
                changed_attributes,
            } => attribute_events(
                &p.identity().to_string(),
                &p.attributes(),
                Some(changed_attributes),
            ),
            _ => Vec::new(),
        };
        match &event {
            RoomEvent::DataReceived {
                payload,
                topic: Some(topic),
                participant: Some(from),
                ..
            } if topic == TRANSCRIPT_TOPIC => {
                on_transcript(&state, &events, from, payload).await;
            }
            // Someone else started transcribing while you do: the newest
            // carries on.
            RoomEvent::ParticipantAttributesChanged {
                participant: Participant::Remote(p),
                changed_attributes,
            } if changed_attributes
                .get(TRANSCRIBING)
                .is_some_and(|v| !v.is_empty()) =>
            {
                let (_, since) = parse_transcribing(&changed_attributes[TRANSCRIBING]);
                let identity = p.identity().to_string();
                let you = room.local_participant().identity().to_string();
                let yields = {
                    let s = state.lock();
                    s.transcribing.is_some()
                        && yields_to((s.transcribing_since, &you), (since, &identity))
                };
                if yields {
                    tracing::info!(%identity, "their Mac takes over the transcript");
                    let (state, events) = (state.clone(), events.clone());
                    tasks::spawn(async move {
                        stop_transcribing_for(&state, &events, Some(identity), true).await
                    });
                }
            }
            // Their last lines may still come.
            RoomEvent::ParticipantAttributesChanged {
                participant: Participant::Remote(p),
                changed_attributes,
            } if changed_attributes
                .get(TRANSCRIBING)
                .is_some_and(String::is_empty) =>
            {
                let mut s = state.lock();
                s.stopped_transcribing
                    .insert(p.identity().to_string(), Instant::now());
            }
            // Someone joining while your Mac transcribes gets what was said.
            RoomEvent::ParticipantConnected(p) => {
                let history = {
                    let s = state.lock();
                    s.transcribing
                        .as_ref()
                        .map(|_| s.transcribed.iter().map(|k| k.line.clone()))
                        .map(Vec::from_iter)
                };
                if let Some(lines) = history.filter(|l| !l.is_empty()) {
                    let to = p.identity().to_string();
                    tasks::spawn({
                        let room = room.clone();
                        async move { send_history(&room, lines, to).await }
                    });
                }
            }
            _ => {}
        }
        if let Some(event) = translate(event) {
            let disconnected = matches!(event, Event::Disconnected { .. });
            if events.send(event).await.is_err() || disconnected {
                break;
            }
        }
        for event in attributes {
            let _ = events.send(event).await;
        }
    }
    // The call ended without you leaving (the server let you go, or the
    // connection gave up): nobody sees you record any more, so stop, and
    // keep what was recorded, as leaving would. Whatever was still starting
    // sees the call is gone, and stops too.
    let (recorder, transcribing) = {
        let mut s = state.lock();
        if is_current(&s, &room) {
            s.room = None;
            s.transcript_sends.clear();
            forget_call(&mut s);
            (s.recorder.take(), s.transcribing.take())
        } else {
            (None, None)
        }
    };
    stop_taking_in(transcribing.as_ref(), recorder.as_ref());
    wind_down(transcribing, recorder, &events, true).await;
    Ok(())
}

/// What the call that ends leaves behind, gone, however it ends: the next
/// call on this engine starts with nothing of it (its lines, a start on its
/// way, its microphones). Its own transcript and recording, once taken out,
/// still finish.
fn forget_call(s: &mut State) {
    audio_tap::set_mic_live(&s.hub, false);
    s.hub = Default::default();
    s.mic = None;
    s.mic_opens_muted = false;
    s.transcribed.clear();
    s.stopped_transcribing.clear();
    s.handover_asked.clear();
    s.handed_to = None;
    s.start = Start::No;
    s.camera_starting = None;
    s.videos.clear();
    s.mics.clear();
    for (_, task) in s.mic_streams.drain() {
        task.abort();
    }
    s.connection += 1;
}

/// The call is over for your transcript and recording: neither takes in
/// anything more, from now (the screen capture stops too).
fn stop_taking_in(
    transcribing: Option<&transcribe::Transcribing>,
    recorder: Option<&record::Recorder>,
) {
    if let Some(transcribing) = transcribing {
        transcribing.stop_listening();
    }
    if let Some(recorder) = recorder {
        recorder.end();
    }
}

/// Then the last words are transcribed while the file is finished, each in
/// its own time. `say_saved`: tell how the recording went (leaving keeps it
/// in the call's history without).
async fn wind_down(
    transcribing: Option<transcribe::Transcribing>,
    recorder: Option<record::Recorder>,
    events: &async_channel::Sender<Event>,
    say_saved: bool,
) {
    futures_util::future::join(
        finish_words(transcribing, events),
        finish_recording(recorder, events, say_saved),
    )
    .await;
}
async fn finish_words(
    transcribing: Option<transcribe::Transcribing>,
    events: &async_channel::Sender<Event>,
) {
    if let Some(transcribing) = transcribing {
        let _ = tokio::task::spawn_blocking(move || transcribing.stop()).await;
        let _ = events
            .send(Event::TranscriptionStopped {
                by: None,
                handed_over: false,
            })
            .await;
    }
}
async fn finish_recording(
    recorder: Option<record::Recorder>,
    events: &async_channel::Sender<Event>,
    say_saved: bool,
) {
    let Some(recorder) = recorder else { return };
    match stop_recorder(recorder).await {
        event if say_saved => {
            let _ = events.send(event).await;
        }
        Event::RecordingFailed { message, .. } => tracing::warn!("recording: {message}"),
        _ => {}
    }
}
/// Stop `recorder`: saved, or failed, with its file.
async fn stop_recorder(recorder: record::Recorder) -> Event {
    let path = recorder.path().to_owned();
    let failed = |message| Event::RecordingFailed {
        message,
        path: Some(path.clone()),
    };
    match tokio::task::spawn_blocking(move || recorder.stop()).await {
        Ok(Ok(saved)) => Event::RecordingSaved(saved),
        Ok(Err(e)) => failed(format!("{e:#}")),
        Err(e) => failed(format!("{e}")),
    }
}
/// Finish sending final words before goodbye, with a bounded allowance so a
/// stalled transcriber/network cannot prevent disconnection. Recording is local
/// and runs independently. Engine drop retains its outer teardown deadline.
async fn finish_call(
    words: impl Future<Output = ()>,
    recording: impl Future<Output = ()>,
    close: impl Future<Output = ()>,
    words_timeout: Duration,
) {
    futures_util::future::join(
        async {
            if tokio::time::timeout(words_timeout, words).await.is_err() {
                tracing::warn!("final transcript delivery timed out; closing the call");
            }
            close.await;
        },
        recording,
    )
    .await;
}

/// Keeps track of remote videos and microphones, for `connection` only: a
/// room still closing after you left touches nothing.
fn track_publications(state: &Mutex<State>, connection: u64, event: &RoomEvent) {
    let mut s = state.lock();
    if s.connection != connection {
        return;
    }
    match event {
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Video(track),
            publication,
            participant,
        } => {
            let key = TrackKey {
                participant: participant.identity().to_string(),
                sid: track.sid().to_string(),
            };
            s.videos.insert(key, publication.clone());
        }
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Video(track),
            participant,
            ..
        } => {
            let key = TrackKey {
                participant: participant.identity().to_string(),
                sid: track.sid().to_string(),
            };
            s.videos.remove(&key);
        }
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(track),
            publication,
            participant,
        } if publication.source() == TrackSource::Microphone => {
            let key = TrackKey {
                participant: participant.identity().to_string(),
                sid: track.sid().to_string(),
            };
            if s.hub.listening() && !s.mic_streams.contains_key(&key) {
                let pump = pump_mic(key.participant.clone(), track.clone(), s.hub.clone());
                s.mic_streams.insert(key.clone(), tasks::spawn(pump));
            }
            s.mics.insert(key, track.clone());
        }
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Audio(track),
            participant,
            ..
        } => {
            let key = TrackKey {
                participant: participant.identity().to_string(),
                sid: track.sid().to_string(),
            };
            s.mics.remove(&key);
            if let Some(task) = s.mic_streams.remove(&key) {
                task.abort();
            }
        }
        RoomEvent::Disconnected { .. } => {
            s.videos.clear();
            s.mics.clear();
            for (_, task) in s.mic_streams.drain() {
                task.abort();
            }
        }
        _ => {}
    }
}

/// The room you're in, and its connection.
fn current_room(state: &Mutex<State>) -> Option<(Arc<Room>, u64)> {
    let s = state.lock();
    s.room.clone().map(|room| (room, s.connection))
}

/// A failed connect that may work in a moment: the network or the server
/// isn't reachable yet. One the server refused (a 4xx from its check of
/// the token) or a bad URL won't by itself.
fn may_connect_later(error: &RoomError) -> bool {
    use livekit::rtc_engine::EngineError;
    use livekit_signaling::SignalError;
    match error {
        RoomError::Engine(EngineError::Signal(SignalError::Client(status, _))) => {
            matches!(status.as_u16(), 408 | 429)
        }
        RoomError::Engine(EngineError::Signal(
            SignalError::UrlParse(_)
            | SignalError::TokenFormat
            | SignalError::TransportNotConfigured
            | SignalError::LeaveRequest { .. },
        )) => false,
        _ => true,
    }
}

/// The connection gave out, rather than someone ending the call: you left,
/// were removed, or the room is gone. After a sleep LiveKit's own
/// reconnecting fails with `JoinFailure`: its token has expired.
fn connection_lost(reason: livekit::DisconnectReason) -> bool {
    use livekit::DisconnectReason::*;
    match reason {
        UnknownReason | ServerShutdown | StateMismatch | JoinFailure | Migration | SignalClose
        | ConnectionTimeout | MediaFailure => true,
        ClientInitiated | DuplicateIdentity | ParticipantRemoved | RoomDeleted | RoomClosed
        | UserUnavailable | UserRejected | SipTrunkFailure | AgentError => false,
    }
}

fn translate(event: RoomEvent) -> Option<Event> {
    Some(match event {
        RoomEvent::ParticipantConnected(p) => Event::ParticipantJoined {
            identity: p.identity().to_string(),
            name: p.name(),
        },
        RoomEvent::ParticipantDisconnected(p) => Event::ParticipantLeft {
            identity: p.identity().to_string(),
        },
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Video(track),
            publication,
            participant,
        } => {
            let kind = match publication.source() {
                TrackSource::Screenshare => VideoKind::Screen,
                _ => VideoKind::Camera,
            };
            let key = TrackKey {
                participant: participant.identity().to_string(),
                sid: track.sid().to_string(),
            };
            Event::VideoTrackAdded { key, kind, track }
        }
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Video(track),
            participant,
            ..
        } => {
            let key = TrackKey {
                participant: participant.identity().to_string(),
                sid: track.sid().to_string(),
            };
            Event::VideoTrackRemoved { key }
        }
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(_),
            publication,
            participant,
        } if publication.source() == TrackSource::Microphone => Event::ParticipantMic {
            identity: participant.identity().to_string(),
            muted: publication.is_muted(),
        },
        RoomEvent::TrackMuted {
            participant: Participant::Remote(participant),
            publication,
        } if publication.source() == TrackSource::Microphone => Event::ParticipantMic {
            identity: participant.identity().to_string(),
            muted: true,
        },
        RoomEvent::TrackUnmuted {
            participant: Participant::Remote(participant),
            publication,
        } if publication.source() == TrackSource::Microphone => Event::ParticipantMic {
            identity: participant.identity().to_string(),
            muted: false,
        },
        RoomEvent::TrackUnpublished {
            publication,
            participant,
        } if publication.source() == TrackSource::Microphone => Event::ParticipantMic {
            identity: participant.identity().to_string(),
            muted: true,
        },
        RoomEvent::ActiveSpeakersChanged { speakers } => {
            Event::ActiveSpeakers(speakers.iter().map(|p| p.identity().to_string()).collect())
        }
        RoomEvent::Disconnected { reason } => Event::Disconnected {
            reason: format!("{reason:?}"),
            lost: connection_lost(reason),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn goodbye_waits_for_final_delivery_but_not_local_recording() {
        let (send, mut lines) = tokio::sync::mpsc::unbounded_channel();
        let (arrived, received) = tokio::sync::oneshot::channel();
        let (release, network) = tokio::sync::oneshot::channel();
        let delivery = tokio::spawn(async move {
            assert_eq!(lines.recv().await, Some("last sentence"));
            arrived.send(()).unwrap();
            network.await.unwrap(); // A slow final send, after transcription stopped.
            assert_eq!(lines.recv().await, None);
        });
        let (closed, mut goodbye) = tokio::sync::oneshot::channel();
        let (recorded, recording) = tokio::sync::oneshot::channel();
        let finish = tokio::spawn(finish_call(
            async move {
                send.send("last sentence").unwrap();
                drop(send);
                delivery.await.unwrap();
            },
            async {
                recording.await.unwrap();
            },
            async {
                closed.send(()).unwrap();
            },
            Duration::from_secs(2),
        ));
        received.await.unwrap();
        assert!(matches!(
            goodbye.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), goodbye)
            .await
            .unwrap()
            .unwrap();
        assert!(!finish.is_finished(), "local recording is still finalizing");
        recorded.send(()).unwrap();
        finish.await.unwrap();
    }

    #[tokio::test]
    async fn stalled_final_transcript_still_allows_goodbye() {
        let (closed, goodbye) = tokio::sync::oneshot::channel();
        tokio::time::timeout(
            Duration::from_secs(1),
            finish_call(
                std::future::pending(),
                async {},
                async {
                    closed.send(()).unwrap();
                },
                Duration::from_millis(10),
            ),
        )
        .await
        .unwrap();
        goodbye.await.unwrap();
    }

    #[tokio::test]
    async fn teardown_thread_failure_shuts_runtime_without_panicking_in_async_context() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let tasks = tasks::Tasks::new(runtime.handle().clone());
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tasks.spawn(async {
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started.await.unwrap();
        let error = retire_runtime(runtime, tasks.clone(), async {}, |job| {
            drop(job); // Builder drops the closure when OS thread creation fails.
            Err(std::io::Error::other("thread limit"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "thread limit");
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            tasks
                .spawn(async { panic!("late work ran") })
                .await
                .unwrap_err()
                .is_cancelled()
        );
    }

    #[test]
    fn the_newest_transcriber_carries_on() {
        assert_eq!(parse_transcribing("parakeet@1700"), ("parakeet", 1700));
        assert_eq!(parse_transcribing("parakeet"), ("parakeet", 0));
        // A handover: they started after you.
        assert!(yields_to((1000, "you"), (2000, "anna")));
        assert!(!yields_to((2000, "you"), (1000, "anna")));
        // At the same moment, both Macs agree on one.
        assert!(
            yields_to((1000, "anna"), (1000, "boris"))
                != yields_to((1000, "boris"), (1000, "anna"))
        );
    }

    /// The history someone joining gets: final lines, each once, however
    /// they came (yours, a transcriber's before a handover, its history
    /// relayed by the next), and known by the Mac that transcribed them.
    #[test]
    fn history_keeps_each_final_line_once() {
        let line = |by: &str, id, text: &str, done| TranscriptLine {
            id,
            speaker: "anna".into(),
            at_ms: 1000 + id,
            text: text.into(),
            done,
            by: Some(by.into()),
        };
        let mut kept = Vec::new();
        remember(&mut kept, &line("a", 1, "Hel", false), false);
        remember(&mut kept, &line("a", 1, "Hello", true), false);
        // Again, in history from the Mac that took over.
        remember(&mut kept, &line("a", 1, "Hello", true), true);
        remember(&mut kept, &line("a", 2, "", true), false);
        // That Mac's own line, with the same number.
        remember(&mut kept, &line("b", 1, "Bye", true), false);
        let texts: Vec<&str> = kept.iter().map(|k| k.line.text.as_str()).collect();
        assert_eq!(texts, ["Hello", "Bye"]);
    }

    /// Stop comes while your transcription's claim is going out (taking
    /// over, say): it never starts listening, and says who stopped it.
    #[test]
    fn stop_while_starting_means_nothing_is_heard() {
        let (engine, _events) = Engine::new().unwrap();
        let state = &engine.state;
        let connection = state.lock().connection;
        let id = reserve_start(&mut state.lock()).unwrap();
        engine.rt().block_on(stop_transcribing(
            state,
            &engine.events,
            Some("anna".into()),
        ));
        assert_eq!(state.lock().start, Start::Stopped(id, Some("anna".into())));
        let installed = install_transcribing(state, connection, id, &engine.tasks, |_| {
            unreachable!("it listened")
        });
        let stopped = installed.expect_err("installed after Stop");
        assert!(matches!(
            stopped.downcast_ref::<StoppedStarting>(),
            Some(StoppedStarting(Some(by))) if by == "anna"
        ));
        assert!(state.lock().transcribing.is_none());
        assert!(!state.lock().hub.listening(), "it listened");
    }

    /// You asked Boris, then Carol, to take over; Boris did. A Stop that
    /// still comes to you goes on to both: Carol's start may still be on its
    /// way, and pass Boris by. Before anyone took over, to both too.
    #[test]
    fn stop_goes_on_to_every_mac_that_may_take_over() {
        let mut s = State {
            handover_asked: vec!["boris".into(), "carol".into()],
            ..State::default()
        };
        assert_eq!(stop_passes_on(&mut s, Some("anna")), ["boris", "carol"]);
        s.handover_asked = vec!["boris".into(), "carol".into()];
        took_over(&mut s, Some("boris"));
        assert_eq!(stop_passes_on(&mut s, Some("anna")), ["boris", "carol"]);
        // Once.
        assert!(stop_passes_on(&mut s, Some("anna")).is_empty());
        // Not back to whoever sent it.
        s.handover_asked = vec!["boris".into()];
        assert!(stop_passes_on(&mut s, Some("boris")).is_empty());
        // To a Mac that started at the same time as yours, and carries on.
        took_over(&mut s, Some("dmitri"));
        assert_eq!(stop_passes_on(&mut s, None), ["dmitri"]);
        // Starting again, yours is a new transcript.
        s.handover_asked = vec!["boris".into()];
        took_over(&mut s, Some("boris"));
        reserve_start(&mut s).unwrap();
        assert!(stop_passes_on(&mut s, None).is_empty());
    }

    /// However a call ends (you leave, or the connection gives up), the next
    /// one on this engine has nothing of it: no lines for its late joiners,
    /// no start in the way of its own, nobody to pass a Stop on to.
    #[test]
    fn an_ended_call_leaves_nothing_behind() {
        let mut s = State::default();
        let line = TranscriptLine {
            id: 1,
            speaker: "anna".into(),
            at_ms: 1000,
            text: "Hello".into(),
            done: true,
            by: Some("anna-mac".into()),
        };
        remember(&mut s.transcribed, &line, false);
        reserve_start(&mut s).unwrap();
        s.handover_asked.push("boris".into());
        took_over(&mut s, Some("carol"));
        s.stopped_transcribing
            .insert("dmitri".into(), Instant::now());
        let call = s.connection;
        forget_call(&mut s);
        assert!(s.transcribed.is_empty());
        assert!(s.stopped_transcribing.is_empty());
        assert!(stop_passes_on(&mut s, None).is_empty());
        assert_ne!(s.connection, call);
        assert!(
            reserve_start(&mut s).is_some(),
            "the old start is in the way"
        );
    }

    /// Leaving while your model loads drops that start: the next call can
    /// start its own, and the old one neither goes on nor clears it.
    #[test]
    fn a_start_is_for_its_own_call() {
        let (engine, _events) = Engine::new().unwrap();
        let state = &engine.state;
        let (old_call, old) = {
            let mut s = state.lock();
            (s.connection, reserve_start(&mut s).unwrap())
        };
        assert!(reserve_start(&mut state.lock()).is_none(), "two at once");
        engine.disconnect();
        let (call, new) = {
            let mut s = state.lock();
            let new = reserve_start(&mut s).expect("the next call can't start");
            (s.connection, new)
        };
        assert!(start_goes_on(&state.lock(), old_call, old).is_err());
        assert!(start_goes_on(&state.lock(), call, old).is_err());
        finish_start(&mut state.lock(), old);
        assert_eq!(state.lock().start, Start::Going(new));
        assert!(start_goes_on(&state.lock(), call, new).is_ok());
        finish_start(&mut state.lock(), new);
        assert_eq!(state.lock().start, Start::No);
    }

    /// History relayed by the Mac that took over adds the lines you missed,
    /// but can't rewrite or take back a line from the Mac that said it.
    #[test]
    fn relayed_lines_only_add() {
        let line = |by: Option<&str>, id, text: &str| TranscriptLine {
            id,
            speaker: "anna".into(),
            at_ms: 1000 + id,
            text: text.into(),
            done: true,
            by: by.map(Into::into),
        };
        let mut kept = Vec::new();
        let live = take_lines(
            &mut kept,
            "a",
            vec![line(None, 1, "Hello"), line(None, 2, "Bye")],
        );
        assert_eq!(live.len(), 2);
        assert!(
            live.iter()
                .all(|(l, relayed)| l.by.as_deref() == Some("a") && !relayed)
        );
        let relayed = take_lines(
            &mut kept,
            "b",
            vec![
                line(Some("a"), 1, "Forged"),
                line(Some("a"), 2, ""),
                line(Some("a"), 3, "Missed"),
                line(Some("b"), 1, "Theirs"),
            ],
        );
        let texts = |lines: &[(TranscriptLine, bool)]| -> Vec<(String, bool)> {
            lines.iter().map(|(l, r)| (l.text.clone(), *r)).collect()
        };
        assert_eq!(
            texts(&relayed),
            [("Missed".into(), true), ("Theirs".into(), false)]
        );
        let kept_texts =
            |kept: &[Kept]| -> Vec<String> { kept.iter().map(|k| k.line.text.clone()).collect() };
        assert_eq!(kept_texts(&kept), ["Hello", "Bye", "Missed", "Theirs"]);
        // A Mac's own line may still be taken back by it.
        let back = take_lines(&mut kept, "b", vec![line(Some("b"), 1, "")]);
        assert_eq!(back.len(), 1);
    }

    /// Anyone can claim the transcript, so a relayed final stands only until
    /// the Mac that said it sends its own: a final forged from a draft it
    /// saw doesn't keep the real one out.
    #[test]
    fn the_own_final_beats_a_relayed_one() {
        let line = |by: Option<&str>, id, text: &str| TranscriptLine {
            id,
            speaker: "anna".into(),
            at_ms: 1000 + id,
            text: text.into(),
            done: true,
            by: by.map(Into::into),
        };
        let kept_texts =
            |kept: &[Kept]| -> Vec<String> { kept.iter().map(|k| k.line.text.clone()).collect() };
        let mut kept = Vec::new();
        let forged = take_lines(&mut kept, "mallory", vec![line(Some("a"), 5, "Forged")]);
        assert_eq!(forged.len(), 1);
        assert!(forged[0].1, "relayed");
        let real = take_lines(&mut kept, "a", vec![line(None, 5, "Real")]);
        assert_eq!(real.len(), 1);
        assert!(!real[0].1);
        assert_eq!(kept_texts(&kept), ["Real"]);
        // Then it stays: relayed again, or changed.
        assert!(take_lines(&mut kept, "mallory", vec![line(Some("a"), 5, "Forged")]).is_empty());
        take_lines(&mut kept, "a", vec![line(None, 5, "Changed")]);
        assert_eq!(kept_texts(&kept), ["Real"]);
        // A relayed line the Mac takes back (it was nothing) goes.
        take_lines(&mut kept, "mallory", vec![line(Some("a"), 6, "Forged")]);
        take_lines(&mut kept, "a", vec![line(None, 6, "")]);
        assert_eq!(kept_texts(&kept), ["Real"]);
    }

    /// History to a late joiner fits LiveKit's packets however long the
    /// call, in any script: 40 lines of Russian are over 15 KiB.
    #[test]
    fn history_fits_in_packets() {
        let text = "Я посмотрел отчет, все выглядит хорошо, ".repeat(5);
        let lines: Vec<TranscriptLine> = (0..300)
            .map(|id| TranscriptLine {
                id: 1_794_313_778_560 + id,
                speaker: "anna-petrova-0cdd".into(),
                at_ms: 1_794_313_867_444 + id,
                text: text.clone(),
                done: true,
                by: Some("mac-tester-5368".into()),
            })
            .collect();
        let forty = serde_json::to_vec(&TranscriptMessage::Lines {
            lines: lines[..40].to_vec(),
        })
        .unwrap();
        assert!(forty.len() > 15 * 1024, "{} bytes", forty.len());
        let messages = history_messages(&lines);
        assert!(messages.len() > 1);
        let mut back = Vec::new();
        for m in &messages {
            assert!(m.len() <= MESSAGE_BYTES, "{} bytes", m.len());
            let Ok(TranscriptMessage::Lines { lines }) = serde_json::from_slice(m) else {
                panic!("lines");
            };
            back.extend(lines);
        }
        assert_eq!(back, lines);
        assert!(history_messages(&[]).is_empty());
    }

    /// Joining again on the same engine while the old room still closes:
    /// its last events (here, its Disconnected) leave the new call alone.
    #[test]
    fn a_closing_room_leaves_the_next_call_alone() {
        let (engine, _events) = Engine::new().unwrap();
        let (old, new) = {
            let mut s = engine.state.lock();
            s.connection += 2;
            (s.connection - 1, s.connection)
        };
        let key = TrackKey {
            participant: "boris".into(),
            sid: "TR_new".into(),
        };
        let pump = engine.rt().spawn(std::future::pending::<()>());
        engine.state.lock().mic_streams.insert(key, pump);
        let gone = RoomEvent::Disconnected {
            reason: livekit::DisconnectReason::ClientInitiated,
        };
        track_publications(&engine.state, old, &gone);
        assert_eq!(engine.state.lock().mic_streams.len(), 1);
        track_publications(&engine.state, new, &gone);
        assert!(engine.state.lock().mic_streams.is_empty());
    }

    /// You left a call whose recording is still being finished, and joined
    /// another: the old recording hears nothing of the new call, and the
    /// new call's voices aren't decoded for it.
    #[test]
    fn a_call_still_finishing_hears_nothing_of_the_next() {
        let _serial = audio_tap::tests::SERIAL.lock();
        let (old, _events) = Engine::new().unwrap();
        let (new, _events) = Engine::new().unwrap();
        let finishing = old.listen();
        assert!(!new.state.lock().hub.listening());
        audio_tap::set_mic_live(&new.state.lock().hub, true);
        let listening = new.listen();
        audio_tap::tests::speak();
        assert!(finishing.frames.try_recv().is_err());
        assert_eq!(listening.frames.try_recv().unwrap().speaker, Speaker::Local);
        // Joining again on the same engine starts afresh too.
        new.disconnect();
        assert!(!new.state.lock().hub.listening());
        audio_tap::tests::speak();
        assert!(listening.frames.try_recv().is_err());
    }

    /// Goodbyes already said don't pile up, even if nothing awaited them.
    #[test]
    fn finished_goodbyes_are_forgotten() {
        let (engine, _events) = Engine::new().unwrap();
        engine.disconnect();
        // Wait for the goodbye task itself; awaiting `closed()` would hide
        // the bug this is about.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while engine.closed().now_or_never().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "goodbye never finished"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        engine.disconnect();
        assert_eq!(engine.state.lock().closing.len(), 1);
    }
}
