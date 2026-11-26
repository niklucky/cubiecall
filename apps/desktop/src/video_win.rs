//! Video frames → GPUI image sprites on Windows.
//!
//! GPUI's DirectX renderer has no video surface (on the Mac, `video_mac.rs`
//! draws decoder buffers as they are). Each frame is converted to BGRA once,
//! on the engine thread that decoded it, and drawn as an image sprite. A
//! sprite takes rounded corners, so circles are cut by the GPU without a
//! window mask or the Mac's bands.
//!
//! Every frame is a new image in each window's sprite atlas. A window keeps
//! only the newest frame of each video: the one before is dropped when the
//! next is painted, and a video that's gone has its last frame dropped the
//! next time anything in that window paints video ([`Painted`]).

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use cubie_core::FrameSink;
use cubie_core::livekit::webrtc::prelude::VideoFormatType;
use cubie_core::livekit::webrtc::video_frame::native::VideoFrameBufferExt as _;
use cubie_core::livekit::webrtc::video_frame::{BoxVideoBuffer, VideoBuffer, VideoFrame};
use gpui::{
    App, Bounds, Corners, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, ObjectFit, Pixels, Refineable as _, RenderImage, SharedString, Style,
    StyleRefinement, Styled, Window, WindowId, px,
};
use parking_lot::Mutex;

/// A frame ready to draw: BGRA pixels in an image GPUI can put in its atlas.
#[derive(Clone)]
pub struct Frame {
    image: Arc<RenderImage>,
    /// Which video it's from, and whether that video is still around.
    video: u64,
    alive: Weak<()>,
}

impl Frame {
    fn size(&self) -> gpui::Size<gpui::DevicePixels> {
        self.image.size(0)
    }
}

pub struct SurfaceSink {
    label: SharedString,
    id: u64,
    alive: Arc<()>,
    latest: Mutex<Option<Frame>>,
    /// Coalescing "please repaint" signal (bounded(1); extra sends are dropped).
    redraw: async_channel::Sender<()>,
    /// Convert and repaint at most this often (zero: every frame).
    min_interval: Duration,
    created: Instant,
    /// When the last frame was taken, in µs since `created`.
    last_frame: AtomicU64,
    frames: AtomicU64,
    /// Last frame size as `width << 32 | height`.
    size: AtomicU64,
}

impl SurfaceSink {
    pub fn new(label: SharedString, redraw: async_channel::Sender<()>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            label,
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            alive: Arc::new(()),
            latest: Mutex::new(None),
            redraw,
            min_interval: Duration::ZERO,
            created: Instant::now(),
            last_frame: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            size: AtomicU64::new(0),
        }
    }

    /// Repaint at most `fps` times a second, however fast frames arrive.
    /// Frames in between aren't converted at all.
    pub fn with_max_fps(mut self, fps: u32) -> Self {
        self.min_interval = Duration::from_secs(1) / fps.max(1);
        self
    }

    pub fn latest(&self) -> Option<Frame> {
        self.latest.lock().clone()
    }

    /// Width / height of the last frame, once one has arrived.
    pub fn aspect(&self) -> Option<f32> {
        let packed = self.size.load(Ordering::Relaxed);
        let (width, height) = ((packed >> 32) as f32, (packed & 0xffff_ffff) as f32);
        (width > 0.0 && height > 0.0).then(|| width / height)
    }

    /// Whether this frame is too soon after the last one taken.
    fn too_soon(&self) -> bool {
        if self.min_interval.is_zero() {
            return false;
        }
        let now = self.created.elapsed().as_micros() as u64;
        let last = self.last_frame.load(Ordering::Relaxed);
        if last != 0 && now.saturating_sub(last) < self.min_interval.as_micros() as u64 {
            return true;
        }
        self.last_frame.store(now.max(1), Ordering::Relaxed);
        false
    }
}

impl FrameSink for SurfaceSink {
    fn on_frame(&self, frame: &VideoFrame<BoxVideoBuffer>) {
        if self.too_soon() {
            return;
        }
        let i420 = frame.buffer.to_i420();
        let (width, height) = (i420.width(), i420.height());
        if width == 0 || height == 0 {
            return;
        }
        let packed = (width as u64) << 32 | height as u64;
        if self.size.swap(packed, Ordering::Relaxed) != packed {
            tracing::debug!(track = %self.label, width, height, "video size");
        }
        // libyuv's ARGB is B, G, R, A in memory: GPUI's BGRA.
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        i420.to_argb(
            VideoFormatType::ARGB,
            &mut pixels,
            width * 4,
            width as i32,
            height as i32,
        );
        let Some(buffer) = image::RgbaImage::from_raw(width, height, pixels) else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        *self.latest.lock() = Some(Frame {
            image,
            video: self.id,
            alive: Arc::downgrade(&self.alive),
        });
        if self.frames.fetch_add(1, Ordering::Relaxed) == 0 {
            tracing::info!(track = %self.label, width, height, "first frame");
        }
        let _ = self.redraw.try_send(());
    }
}

/// The newest frame of a video that a window has in its atlas.
struct Painted {
    window: WindowId,
    video: u64,
    alive: Weak<()>,
    image: Arc<RenderImage>,
}

thread_local! {
    static PAINTED: RefCell<Vec<Painted>> = const { RefCell::new(Vec::new()) };
}

/// Draw `frame` into `bounds`, placed by `fit`, its corners rounded, and
/// let go of the frames this window drew before.
fn paint(
    frame: &Frame,
    bounds: Bounds<Pixels>,
    fit: &ObjectFit,
    corners: Corners<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let window_id = window.window_handle().window_id();
    let open: Vec<WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
    let stale: Vec<Arc<RenderImage>> = PAINTED.with(|p| {
        let mut painted = p.borrow_mut();
        let mut stale = Vec::new();
        painted.retain(|entry| {
            let here = entry.window == window_id;
            let gone_window = !here && !open.contains(&entry.window);
            let replaced = here && entry.video == frame.video && entry.image.id != frame.image.id;
            let gone_video = here && entry.alive.strong_count() == 0;
            if here && (replaced || gone_video) {
                stale.push(entry.image.clone());
            }
            !(gone_window || replaced || gone_video)
        });
        if !painted
            .iter()
            .any(|entry| entry.window == window_id && entry.video == frame.video)
        {
            painted.push(Painted {
                window: window_id,
                video: frame.video,
                alive: frame.alive.clone(),
                image: frame.image.clone(),
            });
        }
        stale
    });
    for image in stale {
        let _ = window.drop_image(image);
    }
    let image_bounds = fit.get_bounds(bounds, frame.size());
    if let Err(e) = window.paint_image(bounds, image_bounds, corners, frame.image.clone(), 0, false)
    {
        tracing::debug!("paint video frame: {e:#}");
    }
}

/// The Windows counterpart of `gpui::surface`: a video frame filling its
/// layout box. Rounded corners in its style round the video too.
pub fn surface(frame: Frame) -> Surface {
    Surface {
        frame,
        object_fit: ObjectFit::Contain,
        style: StyleRefinement::default(),
    }
}

pub struct Surface {
    frame: Frame,
    object_fit: ObjectFit,
    style: StyleRefinement,
}

impl Surface {
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.object_fit = object_fit;
        self
    }
}

impl Styled for Surface {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for Surface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Surface {
    type RequestLayoutState = Style;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Style) {
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style.clone(), [], cx);
        (layout_id, style)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Style,
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        style: &mut Style,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let corners = style.corner_radii.to_pixels(window.rem_size());
        paint(&self.frame, bounds, &self.object_fit, corners, window, cx);
    }
}

/// Video cut to a rounded rectangle filling its parent; a circle when
/// `radius` is half the side. The sprite's own corners, anti-aliased by the
/// GPU: no bands, and nothing for a ring to hide.
pub fn rounded(frame: Frame, radius: f32, fit: ObjectFit) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let (w, h) = (bounds.size.width / px(1.0), bounds.size.height / px(1.0));
            let r = px(radius.min(w / 2.0).min(h / 2.0));
            paint(&frame, bounds, &fit, Corners::all(r), window, cx);
        },
    )
    .size_full()
}
