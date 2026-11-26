//! Screen sharing: presets, codecs and publishing.
//!
//! Capture is per platform. macOS uses ScreenCaptureKit directly
//! (`screen_mac`): our own picker with previews, frames straight to
//! VideoToolbox. Elsewhere, libwebrtc's `DesktopCapturer` (the `generic`
//! module below), driven on our own clock for a selectable frame rate.

use anyhow::Context as _;
use livekit::options::{DegradationPreference, TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::prelude::{RtcVideoSource, VideoResolution};
use livekit::webrtc::video_source::native::NativeVideoSource;

use std::sync::Arc;

use crate::{Event, FrameSink};

/// A screen or window you can share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceId {
    Display(u32),
    Window(u32),
}

/// What the shared content looks like; drives codec and degradation choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentMode {
    /// Documents, code, slides: keep resolution sharp, drop frames under pressure.
    Text,
    /// Video, animations, scrolling demos: keep frame rate, drop resolution.
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenSharePreset {
    pub fps: u32,
    pub mode: ContentMode,
}

impl ScreenSharePreset {
    pub const PRESETS: [ScreenSharePreset; 4] = [
        ScreenSharePreset {
            fps: 5,
            mode: ContentMode::Text,
        },
        ScreenSharePreset {
            fps: 15,
            mode: ContentMode::Text,
        },
        ScreenSharePreset {
            fps: 30,
            mode: ContentMode::Motion,
        },
        ScreenSharePreset {
            fps: 60,
            mode: ContentMode::Motion,
        },
    ];

    pub fn label(&self) -> String {
        let mode = match self.mode {
            ContentMode::Text => "text",
            ContentMode::Motion => "motion",
        };
        format!("{} fps · {mode}", self.fps)
    }

    /// Longest edge we encode. Retina captures are 3000+ px wide; encoding
    /// that is wasted CPU and bandwidth for a call.
    pub(crate) fn max_width(&self) -> u32 {
        match self.mode {
            ContentMode::Text => 2560,
            ContentMode::Motion => 1920,
        }
    }

    /// Average bits per second when your screen is recorded: a little under
    /// what a share sends at most, since a file can take its time.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn recording_bitrate(&self) -> u32 {
        match (self.mode, self.fps) {
            (ContentMode::Text, f) if f <= 5 => 1_000_000,
            (ContentMode::Text, _) => 2_000_000,
            (ContentMode::Motion, f) if f <= 30 => 4_000_000,
            (ContentMode::Motion, _) => 6_000_000,
        }
    }

    fn encoding(&self) -> VideoEncoding {
        let max_bitrate = match (self.mode, self.fps) {
            (ContentMode::Text, f) if f <= 5 => 1_500_000,
            (ContentMode::Text, _) => 3_000_000,
            (ContentMode::Motion, f) if f <= 30 => 4_000_000,
            (ContentMode::Motion, _) => 6_000_000,
        };
        VideoEncoding {
            max_bitrate,
            max_framerate: self.fps as f64,
        }
    }

    fn publish_options(&self) -> TrackPublishOptions {
        // Both modes use H.264 on VideoToolbox. VP9's screen-content tools look
        // a little better per bit, but software VP9 at 2560 px cost ~460 MB
        // and ~24% CPU in our measurements, against ~190 MB and ~13% here.
        let codec = VideoCodec::H264;
        let degradation = match self.mode {
            // Keep text sharp; drop frames under pressure.
            ContentMode::Text => DegradationPreference::MaintainResolution,
            ContentMode::Motion => DegradationPreference::MaintainFramerate,
        };
        TrackPublishOptions {
            source: TrackSource::Screenshare,
            video_codec: codec,
            video_encoding: Some(self.encoding()),
            simulcast: false,
            degradation_preference: Some(degradation),
            ..Default::default()
        }
    }
}

pub(crate) struct ScreenShare {
    /// Stops capturing when dropped.
    _capture: Box<dyn Send>,
    sid: TrackSid,
}

impl ScreenShare {
    /// `on_fail` runs (on a capture thread) if capturing stops on its own:
    /// the window closed, the display went away, permission was revoked.
    /// `preview` gets the captured frames too (macOS only for now).
    pub(crate) async fn start(
        room: &Room,
        source: Option<SourceId>,
        preset: ScreenSharePreset,
        preview: Option<Arc<dyn FrameSink>>,
        events: async_channel::Sender<Event>,
        on_fail: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Self> {
        let video = NativeVideoSource::new(
            VideoResolution {
                width: 1920,
                height: 1080,
            },
            true,
        );
        let track =
            LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(video.clone()));

        #[cfg(target_os = "macos")]
        let capture: Box<dyn Send> = {
            let _ = events;
            let source = source.context("choose a screen or window to share")?;
            crate::screen_mac::start(source, preset, video, preview, on_fail).await?
        };
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
        let capture: Box<dyn Send> = {
            let _ = (source, preview);
            Box::new(generic::start(video, preset, events, on_fail)?)
        };
        // A phone shares its screen through the system (ReplayKit, a media
        // projection), not from inside the app: not built yet.
        #[cfg(any(target_os = "ios", target_os = "android"))]
        let capture: Box<dyn Send> = {
            let _ = (source, preview, video, events, on_fail);
            phone()?
        };

        let publication = room
            .local_participant()
            .publish_track(LocalTrack::Video(track), preset.publish_options())
            .await
            .context("publish screen track")?;
        Ok(Self {
            _capture: capture,
            sid: publication.sid(),
        })
    }

    pub(crate) async fn unpublish(self, room: &Room) {
        let Self { _capture, sid } = self;
        drop(_capture);
        if let Err(e) = room.local_participant().unpublish_track(&sid).await {
            tracing::warn!("unpublish screen track: {e}");
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
fn phone() -> anyhow::Result<Box<dyn Send>> {
    anyhow::bail!("sharing a phone's screen isn't built yet")
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
mod generic {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[cfg(windows)]
    use livekit::webrtc::desktop_capturer::CaptureSource;
    use livekit::webrtc::desktop_capturer::{
        CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
        DesktopFrame,
    };
    use livekit::webrtc::native::yuv_helper;
    use livekit::webrtc::prelude::{I420Buffer, VideoFrame, VideoRotation};

    use super::*;

    /// Stops the capture thread when dropped.
    pub(super) struct Stop(Arc<AtomicBool>);

    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn start(
        video: NativeVideoSource,
        preset: ScreenSharePreset,
        events: async_channel::Sender<Event>,
        on_fail: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Stop> {
        let stop = Arc::new(AtomicBool::new(false));
        spawn_capture_thread(video, preset, stop.clone(), events, on_fail)?;
        Ok(Stop(stop))
    }

    /// The display to share on Windows: the main one, and its size in
    /// pixels. Every Windows capturer (Windows.Graphics.Capture, DirectX,
    /// GDI) names screens by their `EnumDisplayDevices` index, so the main
    /// display is the active device flagged primary. With none chosen, or
    /// the wrong one, a share could show another display's contents: when
    /// it isn't in the list, nothing is shared.
    #[cfg(windows)]
    fn main_screen(capturer: &DesktopCapturer) -> Option<(CaptureSource, (i32, i32))> {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{
            DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW,
            EnumDisplayDevicesW, GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
            MonitorFromPoint,
        };
        let main = (0u32..64).find(|&index| {
            let mut device = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            // SAFETY: `cb` is set; writes into `device`.
            let found = unsafe { EnumDisplayDevicesW(None, index, &mut device, 0) }.as_bool();
            let flags = DISPLAY_DEVICE_ACTIVE | DISPLAY_DEVICE_PRIMARY_DEVICE;
            found && device.StateFlags & flags == flags
        })?;
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: plain queries; `cbSize` is set.
        let whole = unsafe {
            let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            GetMonitorInfoW(monitor, &mut info)
                .as_bool()
                .then_some(info.rcMonitor)
        }?;
        let size = (whole.right - whole.left, whole.bottom - whole.top);
        let sources = capturer.get_source_list();
        let pick = sources.iter().find(|s| s.id() == u64::from(main)).cloned();
        tracing::info!(
            screens = sources.len(),
            main,
            found = pick.is_some(),
            ?size,
            "screen share: the main display"
        );
        Some((pick?, size))
    }

    fn spawn_capture_thread(
        source: NativeVideoSource,
        preset: ScreenSharePreset,
        stop: Arc<AtomicBool>,
        events: async_channel::Sender<Event>,
        on_fail: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<()> {
        std::thread::Builder::new()
            .name("screen-capture".into())
            .spawn(move || {
                let kind = DesktopCaptureSourceType::Screen;

                let mut options = DesktopCapturerOptions::new(kind);
                options.set_include_cursor(true);
                let Some(mut capturer) = DesktopCapturer::new(options) else {
                    let _ = events.send_blocking(Event::Error("screen capture unavailable".into()));
                    on_fail();
                    return;
                };

                // Windows: one display, the main one (see `main_screen`).
                #[cfg(windows)]
                let (screen, only) = match main_screen(&capturer) {
                    Some((screen, size)) => (Some(screen), Some(size)),
                    None => {
                        let _ = events.send_blocking(Event::Error(
                            "can't find the main display to share".into(),
                        ));
                        on_fail();
                        return;
                    }
                };
                #[cfg(not(windows))]
                let (screen, only) = (None, None);

                let failed = Arc::new(AtomicBool::new(false));
                let mut frames = FramePump {
                    source,
                    max_width: preset.max_width(),
                    only,
                    started: Instant::now(),
                    sent: 0,
                };
                capturer.start_capture(screen, {
                    let failed = failed.clone();
                    move |result| match result {
                        Ok(frame) => {
                            if !frames.push(&frame) {
                                failed.store(true, Ordering::Relaxed);
                            }
                        }
                        Err(CaptureError::Temporary) => {} // e.g. picker still open
                        Err(CaptureError::Permanent) => failed.store(true, Ordering::Relaxed),
                    }
                });

                let interval = Duration::from_secs_f64(1.0 / preset.fps.max(1) as f64);
                let mut next = Instant::now();
                while !stop.load(Ordering::Relaxed) && !failed.load(Ordering::Relaxed) {
                    capturer.capture_frame();
                    next += interval;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now; // fell behind; don't try to catch up with a burst
                    }
                }

                if failed.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    tracing::info!("screen capture ended (picker cancelled or source closed)");
                    on_fail();
                }
            })?;
        Ok(())
    }

    struct FramePump {
        source: NativeVideoSource,
        max_width: u32,
        /// Windows: the main display's size. LiveKit doesn't say whether
        /// choosing the display worked, and a capturer that couldn't falls
        /// back to the whole desktop (every display), so a frame of any
        /// other size ends the share instead of going out.
        only: Option<(i32, i32)>,
        started: Instant,
        sent: u64,
    }

    impl FramePump {
        /// False if the frame isn't of the display being shared.
        fn push(&mut self, frame: &DesktopFrame) -> bool {
            if let Some(size) = self.only
                && (frame.width(), frame.height()) != size
            {
                tracing::warn!(
                    width = frame.width(),
                    height = frame.height(),
                    expected = ?size,
                    "screen share: a frame that isn't the main display; stopping"
                );
                return false;
            }
            let (width, height) = (frame.width() as u32, frame.height() as u32);
            if width < 2 || height < 2 {
                return true;
            }
            // A fresh buffer per frame: the encoder keeps a reference to it.
            let mut i420 = I420Buffer::new(width, height);
            let (stride_y, stride_u, stride_v) = i420.strides();
            let (y, u, v) = i420.data_mut();
            // macOS delivers BGRA in memory, which libyuv calls ARGB.
            yuv_helper::argb_to_i420(
                frame.data(),
                frame.stride(),
                y,
                stride_y,
                u,
                stride_u,
                v,
                stride_v,
                width as i32,
                height as i32,
            );

            if width > self.max_width {
                let new_w = self.max_width & !1;
                let new_h = ((height as u64 * new_w as u64 / width as u64) as u32) & !1;
                i420 = i420.scale(new_w as i32, new_h as i32);
            }

            self.source.capture_frame(&VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: self.started.elapsed().as_micros() as i64,
                frame_metadata: None,
                buffer: i420,
            });
            self.sent += 1;
            if self.sent == 1 {
                tracing::info!(width, height, "first screen frame captured");
            }
            true
        }
    }
}
