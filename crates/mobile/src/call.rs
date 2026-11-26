//! One call, as the screens see it.
//!
//! A [`Call`] wraps the desktop's engine. What you do is a method; what
//! happens comes back as a [`CallEvent`] on the call's own thread (the app
//! hops to its main thread). The app keeps the picture of the call (who's in
//! it, who speaks) from those events; nothing is polled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use cubie_core::livekit::prelude::RemoteVideoTrack;
use cubie_core::permissions::{self, Access};
use cubie_core::{
    Engine, Event, MediaDevice, TrackKey, VideoKind, VideoQuality, VideoSubscription,
};
use parking_lot::Mutex;

use crate::account::Client;
use crate::log::Log;
use crate::store::Store;

/// What happens in a call.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum CallEvent {
    /// The system is asking for the microphone (or the `camera`); the call
    /// waits for the answer.
    WaitingForPermission {
        camera: bool,
    },
    /// You're in. `you` is your identity in this call; `room_name` is what
    /// the room is called, if the server said.
    Connected {
        you: String,
        room_name: Option<String>,
    },
    Joined {
        id: String,
        name: String,
    },
    Left {
        id: String,
    },
    /// Everyone speaking right now (you included), loudest first.
    Speaking {
        ids: Vec<String>,
    },
    /// Someone else's microphone.
    Mic {
        id: String,
        muted: bool,
    },
    /// Someone's camera or screen is there to show: [`Call::attach_video`].
    VideoAdded {
        track: String,
        id: String,
        screen: bool,
    },
    VideoRemoved {
        track: String,
    },
    YourMic {
        on: bool,
    },
    YourCamera {
        on: bool,
    },
    /// Someone records the call (with their screen, if `screen`), or stopped.
    Recording {
        id: String,
        on: bool,
        screen: bool,
    },
    /// Someone's Mac transcribes the call, or stopped.
    Transcribing {
        id: String,
        on: bool,
    },
    /// What someone said, as transcribed on someone's Mac. A later line with
    /// the same `key` replaces this one; `done` false is a draft.
    Line {
        key: String,
        speaker: String,
        at_ms: u64,
        text: String,
        done: bool,
    },
    /// Something went wrong that the call survives (the camera wouldn't
    /// open, say): show it and carry on.
    Notice {
        message: String,
    },
    /// You never got in. `signed_out`: the session has ended, sign in again.
    Failed {
        message: String,
        signed_out: bool,
    },
    /// The call is over without you leaving it: the room closed, you were
    /// removed, or the connection couldn't be had back.
    Ended {
        reason: String,
    },
    /// The connection was lost (the phone slept, the network went): the
    /// call joins again by itself, and says [`CallEvent::Connected`] once
    /// it's back. Everyone else comes again with [`CallEvent::Joined`].
    Reconnecting,
}

/// Seconds between tries to join again after the connection was lost.
const REJOIN_BACKOFF: [u64; 7] = [0, 1, 2, 4, 8, 15, 30];

/// The app's end of a call. Called on the call's thread, never the main one.
#[uniffi::export(with_foreign)]
pub trait CallListener: Send + Sync {
    fn on_event(&self, event: CallEvent);
}

/// How much of a video to receive: pick it from the size it's shown at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Quality {
    Low,
    Medium,
    High,
}

struct Video {
    key: TrackKey,
    track: RemoteVideoTrack,
    /// Frames flow into a view while this is here.
    showing: Option<VideoSubscription>,
}

#[derive(uniffi::Object)]
pub struct Call {
    engine: Engine,
    /// The room's id and what it's called, for the list of calls.
    room: String,
    name: Mutex<String>,
    /// What you're called, for your own lines in the call's history.
    your_name: String,
    store: Arc<Store>,
    log: Mutex<Option<Log>>,
    videos: Mutex<HashMap<String, Video>>,
    /// Your camera, as you see it. It outlives the camera being on or off.
    #[cfg(target_vendor = "apple")]
    preview: Arc<crate::video::RendererSink>,
    /// What the mic button says, before and after the microphone is open.
    mic_wanted: AtomicBool,
    /// There is no microphone in this call: access to it is off.
    mic_missing: AtomicBool,
    /// The camera is restarting on its other side.
    flipping: AtomicBool,
    front: AtomicBool,
    /// Your camera is on (or restarting on its other side); joining
    /// again, it comes back, unless you turn it off meanwhile.
    camera_on: AtomicBool,
    camera_on_connect: AtomicBool,
    /// Joining again after the connection was lost: the tries so far. Mic
    /// and camera changes take this lock too, so none is lost to a rejoin.
    rejoin: Mutex<Option<usize>>,
    /// Closed when you leave (or the call goes): an admission still on its
    /// way is called off.
    cancel: async_channel::Sender<()>,
    cancelled: async_channel::Receiver<()>,
    /// You left: nothing joins again.
    left: AtomicBool,
    /// Who to ask to join again, and who hears of it.
    client: Arc<Client>,
    listener: Arc<dyn CallListener>,
}

#[uniffi::export]
impl Client {
    /// Join `room` (its id; `name` is what you know it as, if anything).
    /// Events go to `listener` until the call is dropped; drop it (after
    /// [`Call::leave`]) when the call is over.
    pub fn join(
        self: Arc<Self>,
        room: String,
        name: Option<String>,
        listener: Arc<dyn CallListener>,
    ) -> Result<Arc<Call>, crate::AppError> {
        let (engine, events) = Engine::new().map_err(crate::AppError::from)?;
        let (cancel, cancelled) = async_channel::bounded(1);
        // Every call opens on the front camera, whichever the last one ended
        // on (the engine's choice outlives a call).
        #[cfg(target_os = "ios")]
        engine.set_camera_front(true);
        let session = self.session();
        // Admission: asks for the microphone first (a token lasts two
        // minutes; an unanswered prompt can take longer), then the server.
        let admission = engine.join(&self.server, &room, session.as_deref().unwrap_or(""));
        let call = Arc::new(Call {
            engine,
            room,
            name: Mutex::new(name.unwrap_or_default()),
            your_name: self.account().map_or_else(|| "You".into(), |a| a.name),
            store: self.store.clone(),
            log: Mutex::default(),
            videos: Mutex::default(),
            #[cfg(target_vendor = "apple")]
            preview: Arc::default(),
            mic_wanted: AtomicBool::new(true),
            mic_missing: AtomicBool::new(false),
            flipping: AtomicBool::new(false),
            front: AtomicBool::new(true),
            camera_on: AtomicBool::new(false),
            camera_on_connect: AtomicBool::new(false),
            rejoin: Mutex::new(None),
            cancel,
            cancelled,
            left: AtomicBool::new(false),
            client: self.clone(),
            listener: listener.clone(),
        });
        let weak = Arc::downgrade(&call);
        let client = self;
        let cancelled = call.cancelled.clone();
        std::thread::Builder::new()
            .name("cubie-call".into())
            .spawn(move || {
                // Leaving before you're in calls it off.
                let Some(admission) = crate::block::on(until_cancelled(admission, cancelled))
                else {
                    return;
                };
                match admission {
                    Ok(info) => {
                        let Some(call) = weak.upgrade() else { return };
                        call.admitted(info);
                    }
                    Err(e) => {
                        let error = crate::AppError::from(e);
                        let signed_out = matches!(error, crate::AppError::SignedOut { .. });
                        // The session has ended: the app goes back to
                        // signing in, so it has to be gone here too.
                        if let (true, Some(session)) = (signed_out, &session) {
                            client.forget(session);
                        }
                        listener.on_event(CallEvent::Failed {
                            signed_out,
                            message: error.to_string(),
                        });
                        return;
                    }
                };
                drop(client);
                // Until the call is dropped: the engine holds the other end.
                while let Ok(event) = events.recv_blocking() {
                    for event in translate(&weak, event) {
                        listener.on_event(event);
                    }
                }
            })
            .map_err(|e| crate::AppError::Failed {
                message: format!("Couldn't start the call: {e}"),
            })?;
        Ok(call)
    }
}

#[uniffi::export]
impl Call {
    /// Mute or unmute, and what the mic button should say now. Before the
    /// microphone is open this is remembered, and the microphone opens that
    /// way: muted, nothing of you is sent first. Without access to a
    /// microphone it stays off.
    pub fn set_mic(&self, on: bool) -> bool {
        if self.mic_missing.load(Ordering::Relaxed) {
            return false;
        }
        // In step with joining again, which sets how the next mic opens.
        let _rejoin = self.rejoin.lock();
        self.mic_wanted.store(on, Ordering::Relaxed);
        self.engine.set_mic_opens_muted(!on);
        self.engine.set_mic_enabled(on);
        on
    }

    pub fn set_camera(&self, on: bool) {
        // Joining again: it starts once you're back, or doesn't.
        {
            let rejoin = self.rejoin.lock();
            if rejoin.is_some() {
                self.camera_on_connect.store(on, Ordering::Relaxed);
                drop(rejoin);
                self.listener.on_event(CallEvent::YourCamera { on });
                return;
            }
        }
        if on {
            self.start_camera();
        } else {
            self.flipping.store(false, Ordering::Relaxed);
            self.engine.stop_camera();
        }
    }

    /// The other camera (front, back). A running camera restarts on it.
    pub fn flip_camera(&self, running: bool) {
        let front = !self.front.fetch_xor(true, Ordering::Relaxed);
        #[cfg(target_os = "ios")]
        self.engine.set_camera_front(front);
        let _ = front;
        // Joining again, the camera comes back on the side chosen.
        if running && self.rejoin.lock().is_none() {
            // One camera at a time: the new one opens once the old has
            // closed (see `translate`).
            self.flipping.store(true, Ordering::Relaxed);
            self.engine.stop_camera();
        }
    }

    /// Show your own camera in `renderer` (the address of a view's
    /// `AVSampleBufferVideoRenderer`), or nowhere (0).
    pub fn show_preview(&self, renderer: u64) {
        #[cfg(target_vendor = "apple")]
        self.preview.set_renderer(renderer);
        let _ = renderer;
    }

    /// Show `track` in `renderer` (the address of a view's
    /// `AVSampleBufferVideoRenderer`), instead of wherever it showed before.
    pub fn attach_video(&self, track: String, renderer: u64) {
        #[cfg(target_vendor = "apple")]
        {
            let mut videos = self.videos.lock();
            if let Some(video) = videos.get_mut(&track) {
                let sink = Arc::new(crate::video::RendererSink::new(renderer));
                // The old view stops being fed before the new one starts (a
                // frame already on its way to it may still land there).
                video.showing = None;
                video.showing = Some(self.engine.attach_video(&video.track, sink));
            }
        }
        let _ = (track, renderer);
    }

    /// Stop showing `track`: nothing is decoded into a view nobody sees.
    pub fn detach_video(&self, track: String) {
        if let Some(video) = self.videos.lock().get_mut(&track) {
            video.showing = None;
        }
    }

    /// Receive `track` at this quality (cameras come in layers).
    pub fn set_quality(&self, track: String, quality: Quality) {
        let key = self.videos.lock().get(&track).map(|v| v.key.clone());
        if let Some(key) = key {
            let quality = match quality {
                Quality::Low => VideoQuality::Low,
                Quality::Medium => VideoQuality::Medium,
                Quality::High => VideoQuality::High,
            };
            self.engine.set_video_quality(&key, quality);
        }
    }

    /// Leave. Everyone sees you go at once; then drop the call.
    pub fn leave(&self) {
        self.left.store(true, Ordering::Relaxed);
        self.rejoin.lock().take();
        self.cancel.close();
        self.videos.lock().clear();
        self.engine.disconnect();
        self.keep_record();
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.keep_record();
    }
}

impl Call {
    /// The call is over, however it ended: into the list of calls it goes,
    /// once. A call you never got into leaves nothing.
    fn keep_record(&self) {
        let Some(log) = self.log.lock().take() else {
            return;
        };
        let (record, moments) = log.finish(&self.room, &self.name.lock());
        self.store.add_call(record, moments);
    }

    /// Write something into the call's history, while there's a call.
    fn write(&self, note: impl FnOnce(&mut Log)) {
        if let Some(log) = self.log.lock().as_mut() {
            note(log);
        }
    }

    /// The connection was lost: join again as you were (your mic as the
    /// button says, your camera if it was on).
    fn start_rejoin(self: &Arc<Self>) {
        tracing::warn!("connection lost: joining again");
        {
            // A mute or camera change waits for this, and then counts.
            let mut rejoin = self.rejoin.lock();
            self.camera_on_connect.store(
                self.camera_on.swap(false, Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.flipping.store(false, Ordering::Relaxed);
            // What's left of the old connection goes; the next opens its own.
            self.engine.disconnect();
            let mic = self.mic_wanted.load(Ordering::Relaxed)
                && !self.mic_missing.load(Ordering::Relaxed);
            self.engine.set_mic_opens_muted(!mic);
            *rejoin = Some(0);
        }
        self.rejoin_later();
    }

    /// Let in: the server's name for the room is the newest. Connects,
    /// unless you've left (`leave` takes the same lock).
    fn admitted(&self, info: cubie_core::JoinInfo) {
        if let Some(name) = info.room_name.as_ref().filter(|n| !n.is_empty()) {
            self.name.lock().clone_from(name);
        }
        let _rejoin = self.rejoin.lock();
        if !self.left.load(Ordering::Relaxed) {
            self.engine.connect(info);
        }
    }

    /// Try to join again, after a while that grows with every try, until
    /// it works, can't, or you leave.
    fn rejoin_later(self: &Arc<Self>) {
        let delay = {
            let mut rejoin = self.rejoin.lock();
            let Some(tries) = rejoin.as_mut() else { return };
            let delay = REJOIN_BACKOFF[(*tries).min(REJOIN_BACKOFF.len() - 1)];
            *tries += 1;
            Duration::from_secs(delay)
        };
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("cubie-rejoin".into())
            .spawn(move || {
                std::thread::sleep(delay);
                let Some(call) = weak.upgrade().filter(|c| c.rejoining()) else {
                    return;
                };
                let session = call.client.session();
                let admission = call.engine.join(
                    &call.client.server,
                    &call.room,
                    session.as_deref().unwrap_or(""),
                );
                // Leaving, or the call going, calls it off.
                let cancelled = call.cancelled.clone();
                drop(call);
                let Some(result) = crate::block::on(until_cancelled(admission, cancelled)) else {
                    return;
                };
                let Some(call) = weak.upgrade().filter(|c| c.rejoining()) else {
                    return;
                };
                match result {
                    Ok(info) => call.admitted(info),
                    // Not back on the network yet.
                    Err(e) if cubie_core::http::transient(&e) => {
                        tracing::info!("joining again: {e:#}");
                        call.rejoin_later();
                    }
                    Err(e) => {
                        let error = crate::AppError::from(e);
                        let signed_out = matches!(error, crate::AppError::SignedOut { .. });
                        if let (true, Some(session)) = (signed_out, &session) {
                            call.client.forget(session);
                        }
                        call.give_up();
                        call.listener.on_event(if signed_out {
                            CallEvent::Failed {
                                message: error.to_string(),
                                signed_out,
                            }
                        } else {
                            CallEvent::Ended {
                                reason: error.to_string(),
                            }
                        });
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::error!("joining again: {e}");
        }
    }

    fn rejoining(&self) -> bool {
        !self.left.load(Ordering::Relaxed) && self.rejoin.lock().is_some()
    }

    /// Joining again won't work: the call is over.
    fn give_up(&self) {
        self.rejoin.lock().take();
        self.videos.lock().clear();
        self.keep_record();
    }

    fn start_camera(&self) {
        #[cfg(target_vendor = "apple")]
        self.engine.start_camera(Some(self.preview.clone()));
        #[cfg(not(target_vendor = "apple"))]
        self.engine.start_camera(None);
    }
}

/// `future`'s output, or nothing once `cancelled` closes (dropping it).
async fn until_cancelled<T>(
    future: impl std::future::Future<Output = T>,
    cancelled: async_channel::Receiver<()>,
) -> Option<T> {
    let mut future = std::pin::pin!(future);
    let mut cancel = std::pin::pin!(cancelled.recv());
    std::future::poll_fn(|cx| {
        if cancel.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(None);
        }
        future.as_mut().poll(cx).map(Some)
    })
    .await
}

/// No microphone will open in this call: access is off, or (tests, the
/// Simulator) the engine was told to leave it alone.
fn mic_missing() -> bool {
    cubie_core::no_mic() || permissions::status(MediaDevice::Microphone) != Access::Granted
}

/// An engine event as the app hears it, keeping the call's own books on the
/// way (which videos exist, what the mic should be).
fn translate(call: &Weak<Call>, event: Event) -> Vec<CallEvent> {
    let Some(call) = call.upgrade() else {
        return Vec::new();
    };
    // You, from before joining again, still in the room for a while.
    if event
        .from_someone()
        .is_some_and(|id| call.engine.was_me(id))
    {
        return Vec::new();
    }
    let one = |event| vec![event];
    match event {
        Event::WaitingForPermission(device) => one(CallEvent::WaitingForPermission {
            camera: device == MediaDevice::Camera,
        }),
        Event::Connected {
            identity, others, ..
        } => {
            {
                let mut rejoin = call.rejoin.lock();
                // Left meanwhile: the call's history is kept already.
                if call.left.load(Ordering::Relaxed) {
                    return Vec::new();
                }
                // Back after the connection was lost: the call goes on, and
                // so does its history.
                if rejoin.take().is_some() {
                    call.write(|log| log.rejoined(identity.clone(), &others));
                    if call.camera_on_connect.swap(false, Ordering::Relaxed) {
                        call.start_camera();
                    }
                } else {
                    *call.log.lock() = Some(Log::begin(identity.clone(), call.your_name.clone()));
                }
            }
            let name = call.name.lock().clone();
            let connected = CallEvent::Connected {
                you: identity,
                room_name: (!name.is_empty()).then_some(name),
            };
            // The engine asked for the microphone before connecting. Without
            // it none will open: the mic button says so from the start.
            if mic_missing() {
                call.mic_missing.store(true, Ordering::Relaxed);
                return vec![connected, CallEvent::YourMic { on: false }];
            }
            one(connected)
        }
        Event::Disconnected { lost: true, .. }
            if call.log.lock().is_some() && !call.left.load(Ordering::Relaxed) =>
        {
            call.videos.lock().clear();
            call.start_rejoin();
            one(CallEvent::Reconnecting)
        }
        Event::Disconnected { reason, .. } => {
            call.videos.lock().clear();
            call.keep_record();
            one(CallEvent::Ended { reason })
        }
        Event::ParticipantJoined { identity, name } => {
            call.write(|log| log.joined(&identity, &name));
            one(CallEvent::Joined { id: identity, name })
        }
        Event::ParticipantLeft { identity } => {
            call.write(|log| log.left(&identity));
            one(CallEvent::Left { id: identity })
        }
        Event::ActiveSpeakers(ids) => one(CallEvent::Speaking { ids }),
        Event::ParticipantMic { identity, muted } => one(CallEvent::Mic {
            id: identity,
            muted,
        }),
        Event::VideoTrackAdded { key, kind, track } => {
            let event = CallEvent::VideoAdded {
                track: key.sid.clone(),
                id: key.participant.clone(),
                screen: kind == VideoKind::Screen,
            };
            if kind == VideoKind::Screen {
                call.write(|log| log.screen_started(&key.sid, &key.participant));
            }
            call.videos.lock().insert(
                key.sid.clone(),
                Video {
                    key,
                    track,
                    showing: None,
                },
            );
            one(event)
        }
        Event::VideoTrackRemoved { key } => {
            call.videos.lock().remove(&key.sid);
            call.write(|log| log.track_removed(&key.sid));
            one(CallEvent::VideoRemoved { track: key.sid })
        }
        Event::MicChanged { enabled } => {
            // It opens as it was asked to; should it come up live when mute
            // was pressed meanwhile, it's muted here.
            if enabled && !call.mic_wanted.load(Ordering::Relaxed) {
                call.engine.set_mic_enabled(false);
                return Vec::new();
            }
            one(CallEvent::YourMic { on: enabled })
        }
        Event::CameraChanged { enabled } => {
            // Restarting on its other side, it's still on.
            if !enabled && call.flipping.swap(false, Ordering::Relaxed) {
                call.start_camera();
                return Vec::new();
            }
            call.camera_on.store(enabled, Ordering::Relaxed);
            one(CallEvent::YourCamera { on: enabled })
        }
        Event::ParticipantRecording {
            identity,
            recording,
            screen,
        } => {
            call.write(|log| log.recording(&identity, recording));
            one(CallEvent::Recording {
                id: identity,
                on: recording,
                screen,
            })
        }
        Event::ParticipantTranscribing { identity, model } => {
            call.write(|log| log.transcribing(&identity, model.is_some()));
            one(CallEvent::Transcribing {
                id: identity,
                on: model.is_some(),
            })
        }
        Event::TranscriptLine { by, line, .. } => {
            let key = format!("{by}:{}", line.id);
            // Finished lines go into the call's history; drafts only pass by.
            if line.done {
                call.write(|log| log.said(&key, &line.speaker, line.at_ms, &line.text));
            }
            one(CallEvent::Line {
                key,
                speaker: line.speaker,
                at_ms: line.at_ms,
                text: line.text,
                done: line.done,
            })
        }
        // The engine never got into the room, or fell out of it for good:
        // that's the end of the call, not something to carry on past.
        // Joining again: the network may still be waking up.
        Event::ConnectFailed {
            message,
            retry: true,
        } if call.rejoining() => {
            tracing::info!("joining again: {message}");
            call.rejoin_later();
            Vec::new()
        }
        Event::ConnectFailed { message, .. } => {
            tracing::warn!("{message}");
            call.rejoin.lock().take();
            call.videos.lock().clear();
            let connected = call.log.lock().is_some();
            call.keep_record();
            let message = "Couldn't connect to the call. Check your connection and try again.";
            one(if connected {
                CallEvent::Ended {
                    reason: message.into(),
                }
            } else {
                CallEvent::Failed {
                    message: message.into(),
                    signed_out: false,
                }
            })
        }
        // A camera that didn't start (access off, say) isn't on, even if
        // the button said so while it was queued for joining again.
        Event::Error(message) if message.starts_with("camera") => {
            call.camera_on.store(false, Ordering::Relaxed);
            vec![
                CallEvent::Notice { message },
                CallEvent::YourCamera { on: false },
            ]
        }
        Event::Error(message) => one(CallEvent::Notice { message }),
        // A phone neither shares its screen, records nor transcribes (yet),
        // and asks no Mac to.
        Event::ScreenShareChanged { .. }
        | Event::RecordingStarted { .. }
        | Event::RecordingSaved(_)
        | Event::RecordingFailed { .. }
        | Event::TranscriptionStarted { .. }
        | Event::TranscriptionStopped { .. }
        | Event::TranscriptionFailed(_)
        | Event::ParticipantCanTranscribe { .. }
        | Event::TranscriptAsked { .. }
        | Event::TranscriptAnswered { .. } => Vec::new(),
    }
}
