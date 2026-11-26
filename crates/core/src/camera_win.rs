//! Camera capture on Windows: Media Foundation's source reader on a thread of
//! its own, NV12 copied into I420 buffers for WebRTC and the self-view.
//!
//! The reader runs synchronously: one blocking `ReadSample` per frame, so the
//! thread sees the stop flag at least once a frame. We pick the camera's own
//! mode closest to 720p30 and ask for NV12 at that size; the reader's advanced
//! video processing decodes MJPG and converts YUY2 when the camera has no
//! NV12 mode, without scaling.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::{BoxVideoBuffer, I420Buffer, VideoFrame, VideoRotation};
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer2, IMFActivate, IMFMediaSource, IMFMediaType, IMFSample, IMFSourceReader,
    MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, MF_E_HW_MFT_FAILED_START_STREAMING,
    MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED, MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED, MF_LOW_LATENCY,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_VERSION, MF2DBuffer_LockFlags_Read,
    MFCreateAttributes, MFCreateMediaType, MFCreateSourceReaderFromMediaSource,
    MFEnumDeviceSources, MFMediaType_Video, MFSTARTUP_LITE, MFShutdown, MFStartup,
    MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_YUY2,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::{GUID, Interface as _, PWSTR};

use super::*;
use crate::permissions::{MediaDevice, where_to_allow};

/// The reader's first video stream; the only one we select.
const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// How long [`Capture::start`] waits for the first frame to reach WebRTC.
/// Cameras take a second or two to wake; one that sends nothing usable in
/// this long counts as failed, rather than a track that never shows.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(10);

/// How long stopping waits for the capture thread. A driver can stall
/// inside `ReadSample`; the call takes the track away regardless, and the
/// thread lets go of the device whenever the read returns.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// Keeps capture running; dropping it stops the camera and waits until it
/// has let go of the device.
pub(crate) struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
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
            frames: 0,
        };
        let stop = Arc::new(AtomicBool::new(false));
        // The device is opened on the capture thread (COM objects stay on the
        // thread that made them), which reports back once frames flow.
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("camera".into())
            .spawn({
                let stop = stop.clone();
                move || {
                    if synthetic() {
                        tracing::info!("using the synthetic camera (CUBIE_FAKE_CAMERA)");
                        let _ = ready_tx.send(Ok(()));
                        run_synthetic(handler, &stop);
                    } else {
                        run_device(handler, &stop, ready_tx);
                    }
                }
            })
            .context("camera thread")?;
        // From here on, an early return stops and joins the thread.
        let capture = Self {
            stop,
            thread: Some(thread),
        };
        match ready_rx.recv_timeout(FIRST_FRAME_WAIT) {
            Ok(Ok(())) => Ok(capture),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => {
                anyhow::bail!(
                    "the camera sent no picture in {}s",
                    FIRST_FRAME_WAIT.as_secs()
                )
            }
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("the camera thread ended"),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + STOP_WAIT;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            // Left to finish by itself: it holds the device until then.
            tracing::warn!("camera: still reading after {STOP_WAIT:?}; not waiting");
        }
    }
}

/// Where every frame goes, on the capture thread.
struct Handler {
    source: NativeVideoSource,
    preview: Option<Arc<dyn FrameSink>>,
    started: Instant,
    frames: u64,
}

impl Handler {
    fn push(&mut self, buffer: I420Buffer, width: u32, height: u32) {
        if self.frames == 0 {
            tracing::info!(width, height, "first camera frame");
        }
        self.frames += 1;
        let frame = VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: self.started.elapsed().as_micros() as i64,
            frame_metadata: None,
            buffer: Box::new(buffer) as BoxVideoBuffer,
        };
        self.source.capture_frame(&frame);
        if let Some(preview) = &self.preview {
            preview.on_frame(&frame);
        }
    }
}

/// Capture until `stop`. Errors before the first frame go back to
/// [`Capture::start`]; later ones (the camera unplugged) end up in the log.
fn run_device(mut handler: Handler, stop: &AtomicBool, ready: SyncSender<anyhow::Result<()>>) {
    let mut ready = Some(ready);
    if let Err(e) = capture_device(&mut handler, stop, &mut ready) {
        // Before the first frame, `Capture::start` reports it, unless it
        // already gave up waiting.
        let unheard = match ready.take() {
            Some(ready) => match ready.send(Err(e)) {
                Ok(()) => None,
                Err(mpsc::SendError(sent)) => sent.err(),
            },
            None => Some(e),
        };
        if let Some(e) = unheard {
            tracing::warn!("camera stopped: {e:#}");
        }
    }
}

fn capture_device(
    handler: &mut Handler,
    stop: &AtomicBool,
    ready: &mut Option<SyncSender<anyhow::Result<()>>>,
) -> anyhow::Result<()> {
    // Dropped in reverse: the device first, then Media Foundation, then COM.
    let _com = Com::init()?;
    let _mf = MediaFoundation::start()?;
    let device = Device::open()?;
    let mut format = device.format()?;
    let mut unusable = false;
    while !stop.load(Ordering::Relaxed) {
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        // SAFETY: a live reader; the out-pointers are locals that outlive the
        // call.
        unsafe {
            device
                .reader
                .ReadSample(STREAM, 0, None, Some(&mut flags), None, Some(&mut sample))
        }
        .map_err(|e| explain(e, "read from the camera"))?;
        let has = |flag: i32| flags & flag as u32 != 0;
        anyhow::ensure!(
            !has(MF_SOURCE_READERF_ERROR.0),
            "the camera stopped delivering frames"
        );
        anyhow::ensure!(
            !has(MF_SOURCE_READERF_ENDOFSTREAM.0),
            "the camera's stream ended"
        );
        if has(MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0) {
            format = device.format()?;
        }
        // No sample: a gap in the stream (a "stream tick").
        let Some(sample) = sample else { continue };
        match to_i420(&sample, &format)? {
            Some(buffer) => {
                handler.push(buffer, format.width, format.height);
                // Ready once a frame has reached WebRTC, not before.
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(()));
                }
            }
            None if !unusable => {
                unusable = true;
                tracing::warn!(
                    ?format,
                    "camera frames don't fit their format; dropping them"
                );
            }
            None => {}
        }
    }
    Ok(())
}

/// COM on this thread, for as long as it's held.
struct Com;

impl Com {
    fn init() -> anyhow::Result<Self> {
        // SAFETY: called on our own capture thread; paired with
        // CoUninitialize in Drop. S_FALSE (already initialised) is a success
        // and is paired the same way.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .context("start COM")?;
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: pairs the successful CoInitializeEx in `init`, on the same
        // thread (the guard never leaves `capture_device`).
        unsafe { CoUninitialize() };
    }
}

/// Media Foundation started, for as long as it's held.
struct MediaFoundation;

impl MediaFoundation {
    fn start() -> anyhow::Result<Self> {
        // SAFETY: COM is initialised on this thread; paired with MFShutdown.
        // LITE: capture needs no network sources.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.context("start Media Foundation")?;
        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: pairs the successful MFStartup in `start`.
        let _ = unsafe { MFShutdown() };
    }
}

/// The open camera: its reader, then the activation object that shuts the
/// device down once the reader has let go (fields drop in this order).
struct Device {
    reader: IMFSourceReader,
    _source: SourceShutdown,
}

struct SourceShutdown(IMFActivate);

impl Drop for SourceShutdown {
    fn drop(&mut self) {
        // SAFETY: the object `ActivateObject` created; shutting it down twice
        // (the reader also does on release) only returns an error.
        let _ = unsafe { self.0.ShutdownObject() };
    }
}

/// What the reader delivers now.
#[derive(Debug)]
struct Format {
    width: u32,
    height: u32,
    /// Bytes per row when a buffer doesn't say: the media type's default
    /// stride, else the width.
    stride: usize,
}

/// One of the camera's own modes.
struct Mode {
    media_type: IMFMediaType,
    width: u32,
    height: u32,
    fps: f64,
    subtype: GUID,
}

impl Device {
    fn open() -> anyhow::Result<Self> {
        // SAFETY (whole block): Media Foundation calls on a thread where COM
        // and Media Foundation are started, with valid arguments; every
        // out-pointer is a local that outlives its call, and what the calls
        // allocate is freed or owned below.
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).context("device attributes")?;
            let attributes = attributes.context("device attributes")?;
            attributes.SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )?;
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFEnumDeviceSources(&attributes, &mut list, &mut count)
                .map_err(|e| explain(e, "list cameras"))?;
            let activate = take_devices(list, count)
                .into_iter()
                .next()
                .context("no camera found")?;
            tracing::info!(
                camera = friendly_name(&activate).as_deref().unwrap_or("?"),
                "opening camera"
            );

            let media: IMFMediaSource = activate
                .ActivateObject()
                .map_err(|e| explain(e, "open the camera"))?;
            // From here on the device is shut down however this ends.
            let source = SourceShutdown(activate);
            let mut settings = None;
            MFCreateAttributes(&mut settings, 2).context("reader attributes")?;
            let settings = settings.context("reader attributes")?;
            settings.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
            settings.SetUINT32(&MF_LOW_LATENCY, 1)?;
            let reader = MFCreateSourceReaderFromMediaSource(&media, &settings)
                .map_err(|e| explain(e, "start the camera"))?;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(STREAM, true)?;

            let mode = modes(&reader)
                .into_iter()
                .min_by_key(rank)
                .context("the camera offers no video mode")?;
            tracing::info!(
                width = mode.width,
                height = mode.height,
                fps = mode.fps,
                subtype = ?mode.subtype,
                "camera mode"
            );
            reader
                .SetCurrentMediaType(STREAM, None, &mode.media_type)
                .map_err(|e| explain(e, "set the camera's mode"))?;
            if mode.subtype != MFVideoFormat_NV12 {
                let nv12 = MFCreateMediaType()?;
                nv12.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                nv12.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                nv12.SetUINT64(&MF_MT_FRAME_SIZE, pack(mode.width, mode.height))?;
                reader
                    .SetCurrentMediaType(STREAM, None, &nv12)
                    .map_err(|e| explain(e, "convert the camera's frames to NV12"))?;
            }
            Ok(Self {
                reader,
                _source: source,
            })
        }
    }

    fn format(&self) -> anyhow::Result<Format> {
        // SAFETY: a live reader with its video stream selected.
        unsafe {
            let current = self.reader.GetCurrentMediaType(STREAM)?;
            let (width, height) = unpack(current.GetUINT64(&MF_MT_FRAME_SIZE)?);
            // A signed value stored as unsigned; negative means bottom-up rows,
            // which NV12 never has.
            let stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .ok()
                .map(|s| s as i32)
                .filter(|&s| s > 0)
                .map_or(width as usize, |s| s as usize);
            Ok(Format {
                width,
                height,
                stride,
            })
        }
    }
}

/// The devices `MFEnumDeviceSources` found, owned: it hands out an array of
/// references for us to release, in memory for us to free.
///
/// # Safety
/// `list` is null or holds `count` initialised entries allocated with
/// `CoTaskMemAlloc`.
unsafe fn take_devices(list: *mut Option<IMFActivate>, count: u32) -> Vec<IMFActivate> {
    if list.is_null() {
        return Vec::new();
    }
    // SAFETY: as the caller promises; taking each entry leaves `None`, which
    // needs no release.
    let devices = unsafe { std::slice::from_raw_parts_mut(list, count as usize) }
        .iter_mut()
        .filter_map(Option::take)
        .collect();
    // SAFETY: the array itself, allocated by Media Foundation with
    // CoTaskMemAlloc; nothing points into it any more.
    unsafe { CoTaskMemFree(Some(list as *const _)) };
    devices
}

/// The camera's name as Windows shows it.
fn friendly_name(activate: &IMFActivate) -> Option<String> {
    let mut name = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: valid out-pointers; on success `name` is a NUL-terminated string
    // allocated with CoTaskMemAlloc, which we free after copying it.
    unsafe {
        activate
            .GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len)
            .ok()?;
        let text = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        text
    }
}

/// Every mode the camera offers on its first video stream.
fn modes(reader: &IMFSourceReader) -> Vec<Mode> {
    let mut modes = Vec::new();
    // Bounded: a driver listing modes forever shouldn't hang the call.
    for index in 0..1000 {
        // SAFETY: a live reader; MF_E_NO_MORE_TYPES ends the list.
        let Ok(media_type) = (unsafe { reader.GetNativeMediaType(STREAM, index) }) else {
            break;
        };
        // SAFETY: reading attributes of a live media type.
        let (subtype, size, rate) = unsafe {
            (
                media_type.GetGUID(&MF_MT_SUBTYPE),
                media_type.GetUINT64(&MF_MT_FRAME_SIZE),
                media_type.GetUINT64(&MF_MT_FRAME_RATE),
            )
        };
        let (Ok(subtype), Ok(size)) = (subtype, size) else {
            continue;
        };
        let (width, height) = unpack(size);
        let fps = rate.map_or(0.0, |rate| match unpack(rate) {
            (_, 0) => 0.0,
            (n, d) => n as f64 / d as f64,
        });
        modes.push(Mode {
            media_type,
            width,
            height,
            fps,
            subtype,
        });
    }
    modes
}

/// Lower is better: a smooth frame rate first, then the smallest size that
/// covers 720p (else the largest there is), then the cheapest format to turn
/// into NV12, then the rate closest to 30.
fn rank(mode: &Mode) -> (bool, (bool, u64), u8, u64) {
    let area = mode.width as u64 * mode.height as u64;
    let covers = mode.width >= WIDTH && mode.height >= HEIGHT;
    let size = if covers { area } else { u64::MAX - area };
    let format = match mode.subtype {
        s if s == MFVideoFormat_NV12 => 0,
        s if s == MFVideoFormat_YUY2 => 1,
        s if s == MFVideoFormat_MJPG => 2,
        _ => 3,
    };
    let off_30 = ((mode.fps - 30.0).abs() * 100.0) as u64;
    (mode.fps < 24.0, (!covers, size), format, off_30)
}

/// Media Foundation packs a size or a rate into one value: high, then low.
fn unpack(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

fn pack(high: u32, low: u32) -> u64 {
    (high as u64) << 32 | low as u64
}

/// One NV12 sample as a fresh I420 buffer (the encoder keeps a reference to
/// it). `None` if the buffer is too small for its format.
fn to_i420(sample: &IMFSample, format: &Format) -> anyhow::Result<Option<I420Buffer>> {
    // SAFETY: a live sample; with one buffer (a camera's case) this is that
    // buffer, not a copy.
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
    if let Ok(buffer) = buffer.cast::<IMF2DBuffer2>() {
        let mut scan0 = std::ptr::null_mut();
        let mut pitch = 0i32;
        let mut start = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: valid out-pointers. While locked, `start..start + len` is
        // the buffer's memory and `scan0` the first row inside it.
        unsafe {
            buffer.Lock2DSize(
                MF2DBuffer_LockFlags_Read,
                &mut scan0,
                &mut pitch,
                &mut start,
                &mut len,
            )
        }?;
        let i420 = (pitch > 0).then(|| {
            // SAFETY: locked above; the slice ends where the buffer does.
            let data = unsafe {
                let skip = scan0.offset_from(start).max(0) as usize;
                std::slice::from_raw_parts(scan0, (len as usize).saturating_sub(skip))
            };
            nv12_to_i420(data, pitch as usize, format)
        });
        // SAFETY: pairs the lock above; `data` is gone.
        unsafe { buffer.Unlock2D() }?;
        return Ok(i420.flatten());
    }
    let mut data = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: valid out-pointers; while locked, `data..data + len` is valid.
    unsafe { buffer.Lock(&mut data, None, Some(&mut len)) }?;
    // SAFETY: locked above.
    let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
    let i420 = nv12_to_i420(bytes, format.stride, format);
    // SAFETY: pairs the lock above; `bytes` is gone.
    unsafe { buffer.Unlock() }?;
    Ok(i420)
}

/// NV12 rows `stride` bytes apart, chroma right after the luma plane, into
/// I420. libyuv asserts on short planes, so lengths are checked first.
fn nv12_to_i420(data: &[u8], stride: usize, format: &Format) -> Option<I420Buffer> {
    let (width, height) = (format.width as usize, format.height as usize);
    if width < 2 || height < 2 || stride < width {
        return None;
    }
    let luma_rows = luma_rows(data.len(), stride, height);
    let (y, uv) = data.split_at_checked(stride * luma_rows)?;
    if uv.len() < stride * height.div_ceil(2) {
        return None;
    }
    let mut i420 = I420Buffer::new(format.width, format.height);
    let (stride_y, stride_u, stride_v) = i420.strides();
    let (dst_y, dst_u, dst_v) = i420.data_mut();
    yuv_helper::nv12_to_i420(
        y,
        stride as u32,
        uv,
        stride as u32,
        dst_y,
        stride_y,
        dst_u,
        stride_u,
        dst_v,
        stride_v,
        width as i32,
        height as i32,
    );
    Some(i420)
}

/// Rows in the luma plane: the picture's, unless the buffer is exactly a
/// slightly taller NV12 image (1088 rows for 1080p), whose chroma then starts
/// after the padding.
fn luma_rows(len: usize, stride: usize, height: usize) -> usize {
    let rows = len / stride;
    let padded = rows / 3 * 2;
    if len.is_multiple_of(stride)
        && rows.is_multiple_of(3)
        && padded > height
        && padded - height < 16
    {
        padded
    } else {
        height
    }
}

/// A Media Foundation error, in words where there is something to do about it.
fn explain(e: windows::core::Error, what: &'static str) -> anyhow::Error {
    let code = e.code();
    if code == E_ACCESSDENIED {
        anyhow::anyhow!(
            "camera access is off. Allow it in {}, including \"Let desktop apps access your camera\"",
            where_to_allow(MediaDevice::Camera)
        )
    } else if code == MF_E_HW_MFT_FAILED_START_STREAMING
        || code == MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED
    {
        anyhow::anyhow!("the camera is in use by another app")
    } else if code == MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED {
        anyhow::anyhow!("the camera was disconnected")
    } else {
        anyhow::Error::new(e).context(what)
    }
}

/// The synthetic camera: a moving 720p30 pattern until `stop`.
fn run_synthetic(mut handler: Handler, stop: &AtomicBool) {
    let interval = Duration::from_secs(1) / 30;
    let mut next = Instant::now();
    let mut n = 0;
    while !stop.load(Ordering::Relaxed) {
        handler.push(test_pattern(n), WIDTH, HEIGHT);
        n += 1;
        next += interval;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}

/// The Mac's test pattern (`pixel_buffer::paint_test_pattern`) in I420:
/// scrolling luma ramps under chroma bands that shift once a second.
fn test_pattern(n: usize) -> I420Buffer {
    let (width, height) = (WIDTH as usize, HEIGHT as usize);
    let mut i420 = I420Buffer::new(WIDTH, HEIGHT);
    let (stride_y, stride_u, stride_v) = i420.strides();
    let (stride_y, stride_u, stride_v) = (stride_y as usize, stride_u as usize, stride_v as usize);
    let (y, u, v) = i420.data_mut();
    for row in 0..height {
        let luma = ((row + n * 4) / 3) as u8;
        y[row * stride_y..][..width].fill(luma);
    }
    let [cb, cr] = [104u8, 176u8];
    for row in 0..height / 2 {
        let tint = ((row / 45 + n / 30) % 4) as u8 * 24;
        u[row * stride_u..][..width / 2].fill(cb.saturating_add(tint));
        v[row * stride_v..][..width / 2].fill(cr.saturating_sub(tint));
    }
    i420
}
