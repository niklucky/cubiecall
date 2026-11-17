//! Video frames → a view's `AVSampleBufferVideoRenderer` (iOS).
//!
//! A video view is an `AVSampleBufferDisplayLayer`; it hands us the address
//! of its renderer. Each frame is wrapped in a `CMSampleBuffer` and enqueued
//! on the engine's thread: the system composites it, so the app has no
//! renderer, no shaders and no textures of its own, and the UI thread never
//! sees a frame.
//!
//! - Frames that already are `CVPixelBuffer`s (VideoToolbox-decoded H.264,
//!   your own camera) go in as they are: no copy.
//! - Software-decoded frames (VP8, VP9, AV1) are I420: each is packed into a
//!   pooled video-range NV12 buffer first (libyuv, one pass).

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, Ordering};

use cubie_core::FrameSink;
use cubie_core::livekit::webrtc::native::yuv_helper;
use cubie_core::livekit::webrtc::video_frame::{BoxVideoBuffer, VideoBuffer, VideoFrame};
use cubie_core::pixel_buffer::PixelBufferPool;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMSampleBuffer, CMTime, CMVideoFormatDescription, kCMTimeInvalid};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
};
use parking_lot::Mutex;

#[repr(C)]
struct Timing {
    duration: CMTime,
    presentation: CMTime,
    decode: CMTime,
}

// Core Media and Core Foundation, by their C names: the frameworks are linked
// by the engine already.
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateForImageBuffer(
        allocator: *const c_void,
        image: *const CVPixelBuffer,
        out: *mut *const CMVideoFormatDescription,
    ) -> i32;
    fn CMVideoFormatDescriptionMatchesImageBuffer(
        description: *const CMVideoFormatDescription,
        image: *const CVPixelBuffer,
    ) -> u8;
    fn CMSampleBufferCreateReadyWithImageBuffer(
        allocator: *const c_void,
        image: *const CVPixelBuffer,
        description: *const CMVideoFormatDescription,
        timing: *const Timing,
        out: *mut *mut CMSampleBuffer,
    ) -> i32;
    fn CMSampleBufferGetSampleAttachmentsArray(
        sample: *const CMSampleBuffer,
        create: u8,
    ) -> *const c_void;
    static kCMSampleAttachmentKey_DisplayImmediately: *const c_void;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFDictionarySetValue(dictionary: *const c_void, key: *const c_void, value: *const c_void);
    static kCFBooleanTrue: *const c_void;
}

/// The view's renderer. Let go on the main thread only: ours may be the last
/// reference, and what belongs to a view is torn down there.
struct Renderer(Option<Retained<AnyObject>>);

// SAFETY: `enqueueSampleBuffer:`, `flush` and `status` may be called from any
// thread; the reference itself is only dropped on the main thread (`Drop`).
unsafe impl Send for Renderer {}

impl Drop for Renderer {
    fn drop(&mut self) {
        if let Some(renderer) = self.0.take() {
            let renderer = Renderer(Some(renderer));
            dispatch2::DispatchQueue::main().exec_async(move || {
                let mut renderer = renderer;
                // Now on the main thread: let go for real.
                drop(renderer.0.take());
            });
        }
    }
}

struct Format(CFRetained<CMVideoFormatDescription>);

// SAFETY: an immutable, reference-counted Core Media object.
unsafe impl Send for Format {}

/// Draws one video into one view. The view can change, or go, at any time
/// (a tile closed, your preview hidden): frames without one are dropped
/// before any work is done.
#[derive(Default)]
pub(crate) struct RendererSink {
    renderer: Mutex<Option<Renderer>>,
    format: Mutex<Option<Format>>,
    pool: Mutex<Option<PixelBufferPool>>,
    frames: AtomicU64,
}

impl RendererSink {
    /// `renderer`: the address of an `AVSampleBufferVideoRenderer`, alive for
    /// the duration of this call (we take our own reference).
    pub(crate) fn new(renderer: u64) -> Self {
        let sink = Self::default();
        sink.set_renderer(renderer);
        sink
    }

    /// Draw into `renderer` from now on (0: nowhere).
    pub(crate) fn set_renderer(&self, renderer: u64) {
        let new = NonNull::new(renderer as *mut AnyObject).map(|ptr| {
            // SAFETY: the caller passes a live Objective-C object; we take
            // our own reference.
            Renderer(Some(
                unsafe { Retained::retain(ptr.as_ptr()) }.expect("non-null"),
            ))
        });
        // The old one goes outside the lock (its release hops to the main
        // thread anyway).
        let old = std::mem::replace(&mut *self.renderer.lock(), new);
        drop(old);
    }
}

impl FrameSink for RendererSink {
    fn on_frame(&self, frame: &VideoFrame<BoxVideoBuffer>) {
        // Held while the frame goes in, so `set_renderer` never swaps the
        // view under a frame on its way.
        let renderer = self.renderer.lock();
        let Some(Renderer(Some(renderer))) = renderer.as_ref() else {
            return;
        };
        let Some(pixel_buffer) = native(frame).or_else(|| self.convert(frame)) else {
            return;
        };
        if self.frames.fetch_add(1, Ordering::Relaxed) == 0 {
            tracing::info!(
                width = CVPixelBufferGetWidth(&pixel_buffer),
                height = CVPixelBufferGetHeight(&pixel_buffer),
                zero_copy = frame.buffer.as_native().is_some(),
                "first video frame on screen"
            );
        }
        let Some(sample) = self.sample(&pixel_buffer) else {
            return;
        };
        // SAFETY: a live AVSampleBufferVideoRenderer and a valid sample
        // buffer; these selectors are thread-safe.
        unsafe {
            // A renderer that failed (the app was in the background, say)
            // takes frames again after a flush.
            let status: isize = msg_send![&**renderer, status];
            if status == FAILED {
                let _: () = msg_send![&**renderer, flush];
            }
            let _: () = msg_send![&**renderer, enqueueSampleBuffer: &*sample];
        }
    }
}

/// `AVQueuedSampleBufferRenderingStatusFailed`.
const FAILED: isize = 2;

impl RendererSink {
    /// `pixel_buffer` as a sample to show at once.
    fn sample(&self, pixel_buffer: &CVPixelBuffer) -> Option<CFRetained<CMSampleBuffer>> {
        let mut format = self.format.lock();
        // SAFETY: valid Core Media calls on live objects; created objects are
        // owned (+1) and wrapped at once.
        unsafe {
            let matches = format.as_ref().is_some_and(|f| {
                CMVideoFormatDescriptionMatchesImageBuffer(&*f.0, pixel_buffer) != 0
            });
            if !matches {
                let mut out: *const CMVideoFormatDescription = ptr::null();
                let status = CMVideoFormatDescriptionCreateForImageBuffer(
                    ptr::null(),
                    pixel_buffer,
                    &mut out,
                );
                let out = NonNull::new(out.cast_mut()).filter(|_| status == 0)?;
                *format = Some(Format(CFRetained::from_raw(out)));
            }
            let description = &*format.as_ref()?.0;
            let timing = Timing {
                duration: kCMTimeInvalid,
                presentation: kCMTimeInvalid,
                decode: kCMTimeInvalid,
            };
            let mut out: *mut CMSampleBuffer = ptr::null_mut();
            let status = CMSampleBufferCreateReadyWithImageBuffer(
                ptr::null(),
                pixel_buffer,
                description,
                &timing,
                &mut out,
            );
            let sample = CFRetained::from_raw(NonNull::new(out).filter(|_| status == 0)?);
            // No clock: each frame shows as soon as it arrives.
            let attachments = CMSampleBufferGetSampleAttachmentsArray(&*sample, 1);
            if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
                CFDictionarySetValue(
                    CFArrayGetValueAtIndex(attachments, 0),
                    kCMSampleAttachmentKey_DisplayImmediately,
                    kCFBooleanTrue,
                );
            }
            Some(sample)
        }
    }

    /// A software-decoded frame, packed into a pooled NV12 buffer.
    fn convert(&self, frame: &VideoFrame<BoxVideoBuffer>) -> Option<CFRetained<CVPixelBuffer>> {
        let i420 = frame.buffer.to_i420();
        let (width, height) = (i420.width(), i420.height());
        if width == 0 || height == 0 {
            return None;
        }
        let pixel_buffer = {
            let mut pool = self.pool.lock();
            if pool
                .as_ref()
                .is_none_or(|p| p.width != width || p.height != height)
            {
                *pool = PixelBufferPool::with_format(
                    width,
                    height,
                    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                );
            }
            pool.as_ref()?.take()?
        };
        let (stride_y, stride_u, stride_v) = i420.strides();
        let (y, u, v) = i420.data();
        // SAFETY: the buffer is fresh from a pool made for `width` x `height`
        // NV12: plane 0 has `height` rows and plane 1 half as many (rounded
        // up), each `bytes per row` long.
        unsafe {
            if CVPixelBufferLockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags(0))
                != kCVReturnSuccess
            {
                return None;
            }
            let dst_stride_y = CVPixelBufferGetBytesPerRowOfPlane(&pixel_buffer, 0);
            let dst_stride_uv = CVPixelBufferGetBytesPerRowOfPlane(&pixel_buffer, 1);
            let dst_y = std::slice::from_raw_parts_mut(
                CVPixelBufferGetBaseAddressOfPlane(&pixel_buffer, 0).cast::<u8>(),
                dst_stride_y * height as usize,
            );
            let dst_uv = std::slice::from_raw_parts_mut(
                CVPixelBufferGetBaseAddressOfPlane(&pixel_buffer, 1).cast::<u8>(),
                dst_stride_uv * (height as usize).div_ceil(2),
            );
            yuv_helper::i420_to_nv12(
                y,
                stride_y,
                u,
                stride_u,
                v,
                stride_v,
                dst_y,
                dst_stride_y as u32,
                dst_uv,
                dst_stride_uv as u32,
                width as i32,
                height as i32,
            );
            CVPixelBufferUnlockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags(0));
        }
        Some(pixel_buffer)
    }
}

/// The frame's own `CVPixelBuffer`, if it has one.
fn native(frame: &VideoFrame<BoxVideoBuffer>) -> Option<CFRetained<CVPixelBuffer>> {
    let native = frame.buffer.as_native()?;
    let ptr = NonNull::new(native.get_cv_pixel_buffer().cast::<CVPixelBuffer>())?;
    // SAFETY: the frame holds a reference for as long as we borrow it; we take
    // our own.
    Some(unsafe { CFRetained::retain(ptr) })
}
