//! Camera capture and publishing.
//!
//! macOS: AVFoundation delivers full-range NV12 `CVPixelBuffer`s, which go to
//! WebRTC as native buffers (VideoToolbox encodes them without a CPU copy) and,
//! unchanged, to the self-view sink (GPUI draws them directly).
//!
//! iOS: the same path, from the front or the back camera, turned upright by
//! the capture connection (so nothing rotates frames on the CPU) and held to
//! the phone's limits below.
//!
//! `QUIPCALL_FAKE_CAMERA=1` swaps the device for a synthetic 720p30 pattern
//! that takes the same path, for headless tests and measurements.

use std::sync::Arc;

use anyhow::Context as _;
#[cfg(target_os = "ios")]
use livekit::options::VideoPreset;
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::prelude::{RtcVideoSource, VideoResolution};
use livekit::webrtc::video_source::native::NativeVideoSource;

use crate::FrameSink;

#[cfg(not(target_os = "ios"))]
const WIDTH: u32 = 1280;
#[cfg(not(target_os = "ios"))]
const HEIGHT: u32 = 720;

// A phone sends less: 540p upright at 24 fps, with one small layer under it.
// The floor we start from; raise it when a call shows it's too little
// (docs/PLAN.md, the mobile budget).
#[cfg(target_os = "ios")]
const WIDTH: u32 = 540;
#[cfg(target_os = "ios")]
const HEIGHT: u32 = 960;
#[cfg(target_os = "ios")]
const FPS: i32 = 24;

/// Which camera a phone opens: the front one, until told otherwise.
#[cfg(target_os = "ios")]
static FRONT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Choose the front or the back camera for the next [`Camera::start`].
#[cfg(target_os = "ios")]
pub(crate) fn set_front(front: bool) {
    FRONT.store(front, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(target_os = "ios")]
pub(crate) fn front() -> bool {
    FRONT.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(target_os = "ios"))]
fn publish_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::Camera,
        // VideoToolbox takes the CVPixelBuffers as they are; simulcast
        // gives 720p / 360p / 180p layers for the per-tile layer picker.
        video_codec: VideoCodec::H264,
        simulcast: true,
        video_encoding: Some(VideoEncoding {
            max_bitrate: 1_700_000,
            max_framerate: 30.0,
        }),
        ..Default::default()
    }
}

#[cfg(target_os = "ios")]
fn publish_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::Camera,
        video_codec: VideoCodec::H264,
        simulcast: true,
        video_encoding: Some(VideoEncoding {
            max_bitrate: 800_000,
            max_framerate: FPS as f64,
        }),
        // One layer under the full one, for small tiles and thin connections:
        // a third of the size, 180 × 320.
        simulcast_layers: Some(vec![VideoPreset::new(WIDTH / 3, HEIGHT / 3, 150_000, 15.0)]),
        ..Default::default()
    }
}

/// Whether the synthetic camera is in use (no device, no permission prompt).
pub(crate) fn synthetic() -> bool {
    std::env::var_os("QUIPCALL_FAKE_CAMERA").is_some()
}

pub(crate) struct Camera {
    _capture: platform::Capture,
    sid: TrackSid,
}

impl Camera {
    pub(crate) async fn start(
        room: &Room,
        preview: Option<Arc<dyn FrameSink>>,
    ) -> anyhow::Result<Self> {
        let source = NativeVideoSource::new(
            VideoResolution {
                width: WIDTH,
                height: HEIGHT,
            },
            false,
        );
        let track =
            LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source.clone()));

        // `startRunning` blocks until the device is up (a few hundred ms).
        let capture =
            tokio::task::spawn_blocking(move || platform::Capture::start(source, preview))
                .await
                .context("camera thread")??;

        let publication = room
            .local_participant()
            .publish_track(LocalTrack::Video(track), publish_options())
            .await
            .context("publish camera track")?;
        Ok(Self {
            _capture: capture,
            sid: publication.sid(),
        })
    }

    /// Stops the device and removes the track from the room.
    pub(crate) async fn stop(self, room: &Room) {
        let Self { _capture, sid } = self;
        // Stopping AVFoundation blocks until capture has wound down.
        let _ = tokio::task::spawn_blocking(move || drop(_capture)).await;
        if let Err(e) = room.local_participant().unpublish_track(&sid).await {
            tracing::warn!("unpublish camera track: {e}");
        }
    }
}

#[cfg(not(target_vendor = "apple"))]
mod platform {
    use super::*;

    pub(crate) struct Capture;

    impl Capture {
        pub(crate) fn start(
            _source: NativeVideoSource,
            _preview: Option<Arc<dyn FrameSink>>,
        ) -> anyhow::Result<Self> {
            anyhow::bail!("camera capture is not implemented on this platform yet")
        }
    }
}

#[cfg(target_vendor = "apple")]
mod platform {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
    use livekit::webrtc::prelude::{BoxVideoBuffer, VideoFrame, VideoRotation};
    use livekit::webrtc::video_frame::native::NativeBuffer;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AnyThread, DefinedClass, define_class, msg_send};
    use objc2_av_foundation::{
        AVCaptureConnection, AVCaptureDeviceInput, AVCaptureOutput, AVCaptureSession,
        AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
    };
    #[cfg(not(target_os = "ios"))]
    use objc2_av_foundation::{AVCaptureDevice, AVCaptureSessionPreset1280x720};
    use objc2_core_foundation::{CFRetained, CFString};
    use objc2_core_media::CMSampleBuffer;
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
        kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString};

    use super::*;
    use crate::pixel_buffer::{PixelBufferPool, paint_test_pattern};

    /// Keeps capture running; dropping it stops the camera.
    pub(crate) struct Capture {
        _guard: Box<dyn Send>,
    }

    impl Capture {
        pub(crate) fn start(
            source: NativeVideoSource,
            preview: Option<Arc<dyn FrameSink>>,
        ) -> anyhow::Result<Self> {
            let handler = Handler {
                source,
                preview,
                started: Instant::now(),
                frames: AtomicU64::new(0),
            };
            let guard: Box<dyn Send> = if synthetic() {
                Box::new(SyntheticCapture::start(handler)?)
            } else {
                Box::new(DeviceCapture::start(handler)?)
            };
            Ok(Self { _guard: guard })
        }
    }

    /// Called on the capture queue for every frame.
    struct Handler {
        source: NativeVideoSource,
        preview: Option<Arc<dyn FrameSink>>,
        started: Instant,
        frames: AtomicU64,
    }

    impl Handler {
        fn on_pixel_buffer(&self, pixel_buffer: CFRetained<CVPixelBuffer>) {
            if self.frames.fetch_add(1, Ordering::Relaxed) == 0 {
                tracing::info!(
                    width = CVPixelBufferGetWidth(&pixel_buffer),
                    height = CVPixelBufferGetHeight(&pixel_buffer),
                    "first camera frame"
                );
            }
            // SAFETY: we pass an owned (+1) reference; WebRTC's wrapper retains
            // it and releases ours.
            let native = unsafe {
                NativeBuffer::from_cv_pixel_buffer(
                    CFRetained::into_raw(pixel_buffer).as_ptr().cast(),
                )
            };
            let frame = VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: self.started.elapsed().as_micros() as i64,
                frame_metadata: None,
                buffer: Box::new(native) as BoxVideoBuffer,
            };
            self.source.capture_frame(&frame);
            if let Some(preview) = &self.preview {
                preview.on_frame(&frame);
            }
        }
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; we don't implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "QuipcallCameraDelegate"]
        #[ivars = Handler]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
            #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
            fn did_output(
                &self,
                _output: &AVCaptureOutput,
                sample_buffer: &CMSampleBuffer,
                _connection: &AVCaptureConnection,
            ) {
                // SAFETY: a valid sample buffer from AVFoundation.
                if let Some(pixel_buffer) = unsafe { sample_buffer.image_buffer() } {
                    // GCD drains pools eventually; per frame keeps it tight.
                    crate::autorelease_pool(|| self.ivars().on_pixel_buffer(pixel_buffer));
                }
            }
        }
    );

    impl Delegate {
        fn new(handler: Handler) -> Retained<Self> {
            let this = Self::alloc().set_ivars(handler);
            // SAFETY: NSObject's designated initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    struct DeviceCapture {
        session: Retained<AVCaptureSession>,
        _delegate: Retained<Delegate>,
        _queue: DispatchRetained<DispatchQueue>,
    }

    // SAFETY: AVCaptureSession is documented as thread-safe for start/stop; we
    // only call `stopRunning` (on drop) besides setup.
    unsafe impl Send for DeviceCapture {}

    impl DeviceCapture {
        fn start(handler: Handler) -> anyhow::Result<Self> {
            // SAFETY: plain AVFoundation setup calls with valid arguments. The
            // delegate and queue are kept alive by `Capture`.
            unsafe {
                let media = AVMediaTypeVideo.context("AVMediaTypeVideo")?;
                #[cfg(not(target_os = "ios"))]
                let device = AVCaptureDevice::defaultDeviceWithMediaType(media)
                    .context("no camera found")?;
                #[cfg(target_os = "ios")]
                let device = ios::device(media).context("no camera found")?;
                tracing::info!(camera = %device.localizedName(), "opening camera");
                let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                    .map_err(|e| anyhow::anyhow!("open camera: {}", e.localizedDescription()))?;

                let output = AVCaptureVideoDataOutput::new();
                // Full-range NV12: what GPUI's surface wants and what WebRTC's
                // own macOS capturer uses.
                let key: &CFString = kCVPixelBufferPixelFormatTypeKey;
                let key: &NSString = &*(key as *const CFString as *const NSString);
                let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
                let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                    &[key],
                    &[format.as_ref() as &AnyObject],
                );
                output.setVideoSettings(Some(&settings));
                output.setAlwaysDiscardsLateVideoFrames(true);

                let queue = DispatchQueue::new("quipcall.camera", DispatchQueueAttr::SERIAL);
                let delegate = Delegate::new(handler);
                output.setSampleBufferDelegate_queue(
                    Some(ProtocolObject::from_ref(&*delegate)),
                    Some(&queue),
                );

                let session = AVCaptureSession::new();
                session.beginConfiguration();
                #[cfg(not(target_os = "ios"))]
                if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                    session.setSessionPreset(AVCaptureSessionPreset1280x720);
                }
                #[cfg(target_os = "ios")]
                ios::set_preset(&session);
                anyhow::ensure!(session.canAddInput(&input), "camera input rejected");
                session.addInput(&input);
                anyhow::ensure!(session.canAddOutput(&output), "camera output rejected");
                session.addOutput(&output);
                #[cfg(target_os = "ios")]
                ios::upright(&output, media);
                session.commitConfiguration();
                #[cfg(target_os = "ios")]
                ios::hold_frame_rate(&device);
                session.startRunning();
                anyhow::ensure!(session.isRunning(), "camera did not start");

                Ok(Self {
                    session,
                    _delegate: delegate,
                    _queue: queue,
                })
            }
        }
    }

    impl Drop for DeviceCapture {
        fn drop(&mut self) {
            // SAFETY: valid session. Blocks until capture has stopped, after
            // which the delegate gets no more callbacks.
            unsafe { self.session.stopRunning() };
        }
    }

    /// What a phone's camera needs that a Mac's doesn't.
    #[cfg(target_os = "ios")]
    mod ios {
        use objc2::msg_send;
        use objc2::rc::Retained;
        use objc2_av_foundation::{
            AVCaptureConnection, AVCaptureDevice, AVCaptureDevicePosition,
            AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureSession,
            AVCaptureSessionPreset1280x720, AVCaptureSessionPresetiFrame960x540,
            AVCaptureVideoDataOutput, AVMediaType,
        };
        use objc2_core_media::CMTime;

        /// The wide camera on the side asked for ([`super::super::set_front`]).
        pub(super) unsafe fn device(media: &AVMediaType) -> Option<Retained<AVCaptureDevice>> {
            let position = if super::super::front() {
                AVCaptureDevicePosition::Front
            } else {
                AVCaptureDevicePosition::Back
            };
            // SAFETY: AVFoundation constants and a plain class method.
            unsafe {
                AVCaptureDevice::defaultDeviceWithDeviceType_mediaType_position(
                    AVCaptureDeviceTypeBuiltInWideAngleCamera,
                    Some(media),
                    position,
                )
                .or_else(|| AVCaptureDevice::defaultDeviceWithMediaType(media))
            }
        }

        /// 960 × 540 from the sensor, or 720p where a camera has no such mode.
        pub(super) unsafe fn set_preset(session: &AVCaptureSession) {
            // SAFETY: a valid session, inside begin/commitConfiguration.
            unsafe {
                if session.canSetSessionPreset(AVCaptureSessionPresetiFrame960x540) {
                    session.setSessionPreset(AVCaptureSessionPresetiFrame960x540);
                } else if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                    session.setSessionPreset(AVCaptureSessionPreset1280x720);
                }
            }
        }

        /// The sensor is mounted sideways: have the capture pipeline turn
        /// frames upright (on the GPU), so buffers arrive as portrait and
        /// neither WebRTC nor the viewer has to rotate anything.
        pub(super) unsafe fn upright(output: &AVCaptureVideoDataOutput, media: &AVMediaType) {
            // SAFETY: valid objects; both selectors exist from iOS 17.
            unsafe {
                let connection: Option<Retained<AVCaptureConnection>> =
                    msg_send![output, connectionWithMediaType: media];
                let Some(connection) = connection else { return };
                let supported: bool =
                    msg_send![&connection, isVideoRotationAngleSupported: 90.0f64];
                if supported {
                    let _: () = msg_send![&connection, setVideoRotationAngle: 90.0f64];
                }
            }
        }

        /// The sensor at [`super::super::FPS`], not its default 30: fewer
        /// frames to capture, encode and send.
        pub(super) unsafe fn hold_frame_rate(device: &AVCaptureDevice) {
            // SAFETY: the device is open; configuration is locked around the change.
            unsafe {
                if device.lockForConfiguration().is_err() {
                    return;
                }
                let frame = CMTime::new(1, super::super::FPS);
                let _: () = msg_send![device, setActiveVideoMinFrameDuration: frame];
                let _: () = msg_send![device, setActiveVideoMaxFrameDuration: frame];
                device.unlockForConfiguration();
            }
        }
    }

    /// A moving 720p30 test pattern, painted into pooled IOSurface buffers so
    /// it is encoded exactly like camera frames.
    struct SyntheticCapture {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl SyntheticCapture {
        fn start(handler: Handler) -> anyhow::Result<Self> {
            tracing::info!("using the synthetic camera (QUIPCALL_FAKE_CAMERA)");
            let pool = PixelBufferPool::new(WIDTH, HEIGHT).context("pixel buffer pool")?;
            let stop = Arc::new(AtomicBool::new(false));
            let thread = std::thread::Builder::new()
                .name("fake-camera".into())
                .spawn({
                    let stop = stop.clone();
                    move || {
                        #[cfg(not(target_os = "ios"))]
                        let interval = Duration::from_secs(1) / 30;
                        #[cfg(target_os = "ios")]
                        let interval = Duration::from_secs(1) / FPS as u32;
                        let mut next = Instant::now();
                        let mut n = 0;
                        while !stop.load(Ordering::Relaxed) {
                            if let Some(pixel_buffer) = pool.take() {
                                paint_test_pattern(&pixel_buffer, n, [104, 176]);
                                crate::autorelease_pool(|| handler.on_pixel_buffer(pixel_buffer));
                            }
                            n += 1;
                            next += interval;
                            std::thread::sleep(next.saturating_duration_since(Instant::now()));
                        }
                    }
                })?;
            Ok(Self {
                stop,
                thread: Some(thread),
            })
        }
    }

    impl Drop for SyntheticCapture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}
