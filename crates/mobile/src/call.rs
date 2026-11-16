//! One call, as the screens see it.
//!
//! A [`Call`] wraps the desktop's engine. What you do is a method; what
//! happens comes back as a [`CallEvent`] on the call's own thread (the app
//! hops to its main thread). The app keeps the picture of the call (who's in
//! it, who speaks) from those events; nothing is polled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use quipcall_core::livekit::prelude::RemoteVideoTrack;
use quipcall_core::{
    Engine, Event, MediaDevice, TrackKey, VideoKind, VideoQuality, VideoSubscription,
};

use crate::account::Client;
use crate::store::{CallRecord, Store};

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
    /// removed, or the connection gave up.
    Ended {
        reason: String,
    },
}

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

/// The call's entry for the Calls tab, while it's going on.
struct Log {
    /// When you got in: on the wall clock (unix ms), and for measuring.
    began: u64,
    since: Instant,
    people: Vec<String>,
}

#[derive(uniffi::Object)]
pub struct Call {
    engine: Engine,
    /// The room's id and what it's called, for the list of calls.
    room: String,
    name: Mutex<String>,
    store: Arc<Store>,
    log: Mutex<Option<Log>>,
    videos: Mutex<HashMap<String, Video>>,
    /// Your camera, as you see it. It outlives the camera being on or off.
    #[cfg(target_vendor = "apple")]
    preview: Arc<crate::video::RendererSink>,
    /// What the mic button says, before and after the microphone is open.
    mic_wanted: AtomicBool,
    /// The camera is restarting on its other side.
    flipping: AtomicBool,
    front: AtomicBool,
}

#[uniffi::export]
impl Client {
    /// Join `room` (its id; `name` is what you know it as, if anything).
    /// Events go to `listener` until the call is dropped; drop it (after
    /// [`Call::leave`]) when the call is over.
    pub fn join(
        &self,
        room: String,
        name: Option<String>,
        listener: Arc<dyn CallListener>,
    ) -> Result<Arc<Call>, crate::AppError> {
        let (engine, events) = Engine::new().map_err(crate::AppError::from)?;
        let session = self.session();
        // Admission: asks for the microphone first (a token lasts two
        // minutes; an unanswered prompt can take longer), then the server.
        let admission = engine.join(&self.server, &room, session.as_deref().unwrap_or(""));
        let call = Arc::new(Call {
            engine,
            room,
            name: Mutex::new(name.unwrap_or_default()),
            store: self.store.clone(),
            log: Mutex::default(),
            videos: Mutex::default(),
            #[cfg(target_vendor = "apple")]
            preview: Arc::default(),
            mic_wanted: AtomicBool::new(true),
            flipping: AtomicBool::new(false),
            front: AtomicBool::new(true),
        });
        let weak = Arc::downgrade(&call);
        std::thread::Builder::new()
            .name("quipcall-call".into())
            .spawn(move || {
                let room_name = match crate::block::on(admission) {
                    Ok(info) => {
                        let Some(call) = weak.upgrade() else { return };
                        // The server's name for the room is the newest.
                        let name = info.room_name.clone();
                        if let Some(name) = name.as_ref().filter(|n| !n.is_empty()) {
                            call.name.lock().clone_from(name);
                        }
                        call.engine.connect(info);
                        name
                    }
                    Err(e) => {
                        let error = crate::AppError::from(e);
                        listener.on_event(CallEvent::Failed {
                            signed_out: matches!(error, crate::AppError::SignedOut { .. }),
                            message: error.to_string(),
                        });
                        return;
                    }
                };
                // Until the call is dropped: the engine holds the other end.
                while let Ok(event) = events.recv_blocking() {
                    for mut event in translate(&weak, event) {
                        if let CallEvent::Connected {
                            room_name: name, ..
                        } = &mut event
                        {
                            name.clone_from(&room_name);
                        }
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
    /// Mute or unmute. Before the microphone is open this is remembered, and
    /// applied the moment it is.
    pub fn set_mic(&self, on: bool) {
        self.mic_wanted.store(on, Ordering::Relaxed);
        self.engine.set_mic_enabled(on);
    }

    pub fn set_camera(&self, on: bool) {
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
        if running {
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
                // The old view's frames stop before the new one's start.
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
        let name = self.name.lock().clone();
        self.store.add_call(CallRecord {
            id: log.began,
            room: self.room.clone(),
            name: if name.is_empty() { "Call".into() } else { name },
            started: log.began / 1000,
            seconds: log.since.elapsed().as_secs(),
            people: log.people,
        });
    }

    fn start_camera(&self) {
        #[cfg(target_vendor = "apple")]
        self.engine.start_camera(Some(self.preview.clone()));
        #[cfg(not(target_vendor = "apple"))]
        self.engine.start_camera(None);
    }
}

/// An engine event as the app hears it, keeping the call's own books on the
/// way (which videos exist, what the mic should be).
fn translate(call: &Weak<Call>, event: Event) -> Vec<CallEvent> {
    let Some(call) = call.upgrade() else {
        return Vec::new();
    };
    let one = |event| vec![event];
    match event {
        Event::WaitingForPermission(device) => one(CallEvent::WaitingForPermission {
            camera: device == MediaDevice::Camera,
        }),
        Event::Connected { identity, .. } => {
            let began = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            *call.log.lock() = Some(Log {
                began,
                since: Instant::now(),
                people: Vec::new(),
            });
            one(CallEvent::Connected {
                you: identity,
                room_name: None,
            })
        }
        Event::Disconnected { reason } => {
            call.videos.lock().clear();
            call.keep_record();
            one(CallEvent::Ended { reason })
        }
        Event::ParticipantJoined { identity, name } => {
            if let Some(log) = call.log.lock().as_mut()
                && !name.is_empty()
                && !log.people.contains(&name)
            {
                log.people.push(name.clone());
            }
            one(CallEvent::Joined { id: identity, name })
        }
        Event::ParticipantLeft { identity } => one(CallEvent::Left { id: identity }),
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
            one(CallEvent::VideoRemoved { track: key.sid })
        }
        Event::MicChanged { enabled } => {
            // The microphone opens unmuted; muted before that means muted.
            if enabled && !call.mic_wanted.load(Ordering::Relaxed) {
                call.engine.set_mic_enabled(false);
                return Vec::new();
            }
            one(CallEvent::YourMic { on: enabled })
        }
        Event::CameraChanged { enabled } => {
            if !enabled && call.flipping.swap(false, Ordering::Relaxed) {
                call.start_camera();
                return Vec::new();
            }
            one(CallEvent::YourCamera { on: enabled })
        }
        Event::ParticipantRecording {
            identity,
            recording,
            screen,
        } => one(CallEvent::Recording {
            id: identity,
            on: recording,
            screen,
        }),
        Event::ParticipantTranscribing { identity, model } => one(CallEvent::Transcribing {
            id: identity,
            on: model.is_some(),
        }),
        Event::TranscriptLine { by, line, .. } => one(CallEvent::Line {
            key: format!("{by}:{}", line.id),
            speaker: line.speaker,
            at_ms: line.at_ms,
            text: line.text,
            done: line.done,
        }),
        Event::Error(message) => one(CallEvent::Notice { message }),
        // A phone neither shares its screen, records nor transcribes (yet),
        // and asks no Mac to.
        Event::ScreenShareChanged { .. }
        | Event::RecordingStarted { .. }
        | Event::RecordingSaved(_)
        | Event::RecordingFailed(_)
        | Event::TranscriptionStarted
        | Event::TranscriptionStopped { .. }
        | Event::TranscriptionFailed(_)
        | Event::ParticipantCanTranscribe { .. }
        | Event::TranscriptAsked { .. }
        | Event::TranscriptAnswered { .. } => Vec::new(),
    }
}
