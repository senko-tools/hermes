//! Avatars and attachment images, loaded from the server once and decoded off the UI thread.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, RenderImage, SharedString, Window, div, img};
use hermes_client_core::InstanceId;
use hermes_client_core::picture;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{Attachment, AttachmentRef};
use hermes_client_core::proto::models::{Status, User};
use image::Frame;

use crate::shell::Shell;
use crate::theme::{self, px};

/// The largest size an avatar is drawn at, in pixels on a 2x display.
pub(crate) const AVATAR_PIXELS: u32 = 192;
/// The same for a profile banner (300 by 120 on the card).
pub(crate) const BANNER_PIXELS: u32 = 640;

#[derive(Clone)]
pub enum Img {
    Loading,
    Ready(Picture),
    Failed,
}

mod stream;
mod svg_anim;
pub use stream::advance_all as advance_streams;

/// How an animated picture moves.
#[derive(Clone)]
pub enum Moving {
    /// All its frames, decoded.
    Frames(Arc<Held>),
    /// Too big to hold decoded: played from its file a few frames ahead.
    Stream(Arc<stream::AnimStream>),
}

/// An animation with all its frames decoded, each its own image. It picks the frame due itself
/// instead of handing gpui one image of many frames: gpui would draw the whole window again on
/// every screen refresh for that, however slowly the animation moves (#60).
pub struct Held {
    frames: Vec<Arc<RenderImage>>,
    /// When each frame ends, from the start of a loop.
    ends: Vec<Duration>,
    start: Instant,
}

impl Held {
    /// `frames` in BGRA, each with how long it shows.
    fn new(frames: Vec<Frame>) -> Self {
        let mut ends = Vec::with_capacity(frames.len());
        let mut end = Duration::ZERO;
        let frames = frames
            .into_iter()
            .map(|f| {
                end += frame_delay(f.delay());
                ends.push(end);
                Arc::new(RenderImage::new(vec![Frame::new(f.into_buffer())]))
            })
            .collect();
        Held {
            frames,
            ends,
            start: Instant::now(),
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    #[cfg(test)]
    pub fn frame(&self, n: usize) -> &Arc<RenderImage> {
        &self.frames[n]
    }

    /// The frame shown at `now`, and when the next one is due.
    fn at(&self, now: Instant) -> (usize, Instant) {
        let total = self.ends.last().copied().unwrap_or_default();
        if total.is_zero() {
            return (0, now + Duration::from_secs(3600));
        }
        let into = now.duration_since(self.start);
        let t = Duration::from_nanos((into.as_nanos() % total.as_nanos()) as u64);
        let n = self
            .ends
            .partition_point(|end| *end <= t)
            .min(self.ends.len() - 1);
        (n, now + (self.ends[n] - t))
    }
}

/// How long a frame shows: like browsers, 10 ms or less counts as 100 ms.
fn frame_delay(delay: image::Delay) -> Duration {
    let (numer, denom) = delay.numer_denom_ms();
    let ms = numer / denom.max(1);
    Duration::from_millis(if ms <= 10 { 100 } else { u64::from(ms) })
}

thread_local! {
    /// The earliest moment a picture drawn in this frame changes (see [`wake_at`]).
    static WAKE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// A picture being drawn changes at `at`: the window is drawn again then.
pub(crate) fn wake_at(at: Instant) {
    WAKE.with(|w| w.set(Some(w.get().map_or(at, |old| old.min(at)))));
}

/// When the pictures drawn since the last call change next, if any moves.
pub(crate) fn take_wake() -> Option<Instant> {
    WAKE.with(std::cell::Cell::take)
}

/// A decoded picture: its first frame, and how it moves when it is animated.
#[derive(Clone)]
pub struct Picture {
    pub still: Arc<RenderImage>,
    pub moving: Option<Moving>,
}

impl Picture {
    /// What to draw: the animation only while `animate` (the window has the focus).
    pub fn shown(&self, animate: bool) -> Arc<RenderImage> {
        match (&self.moving, animate) {
            (Some(Moving::Frames(held)), true) => {
                let (n, next) = held.at(Instant::now());
                wake_at(next);
                held.frames[n].clone()
            }
            (Some(Moving::Stream(stream)), true) => {
                stream.frame().unwrap_or_else(|| self.still.clone())
            }
            _ => self.still.clone(),
        }
    }

    /// Everything of it that may be on the GPU.
    fn gpu_images(&self) -> Vec<Arc<RenderImage>> {
        let mut out = vec![self.still.clone()];
        match &self.moving {
            // The still is its first frame.
            Some(Moving::Frames(held)) => out.extend(held.frames.iter().skip(1).cloned()),
            Some(Moving::Stream(stream)) => out.extend(stream.current()),
            None => {}
        }
        out
    }
}

/// How many bytes of decoded frames one animation may hold; a longer one is made smaller.
const ANIMATION_BUDGET: usize = 96 << 20;
/// The same for one in the chat, an avatar, icon or banner: many of these stay loaded at once.
const PREVIEW_ANIMATION_BUDGET: usize = 24 << 20;
/// The long side an animated picture in the chat is decoded at (it is shown at most 400 wide).
const ANIMATED_PREVIEW: u32 = 480;
/// Below this many pixels on its long side an animation stays still instead.
const ANIMATION_MIN_SIDE: u32 = 48;

/// Whether an attachment is an SVG picture: it has no thumbnail, so its original is drawn.
pub fn is_svg(content_type: &str, filename: &str) -> bool {
    content_type == "image/svg+xml" || filename.to_ascii_lowercase().ends_with(".svg")
}

/// Whether `bytes` look like an SVG document rather than a bitmap.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let start = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(start);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
}

/// Draws an SVG into a PNG whose long side is `side` pixels. Fonts come from the system.
pub fn svg_to_png(bytes: &[u8], side: f32) -> Result<Vec<u8>, String> {
    svg_pixmap(bytes, side, &svg_options())?
        .encode_png()
        .map_err(|e| e.to_string())
}

fn svg_options() -> resvg::usvg::Options<'static> {
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    options
}

/// An SVG drawn with its long side `side` pixels, as straight (not premultiplied) RGBA.
fn svg_pixels(
    bytes: &[u8],
    side: f32,
    options: &resvg::usvg::Options,
) -> Result<image::RgbaImage, String> {
    let pixmap = svg_pixmap(bytes, side, options)?;
    let mut out = image::RgbaImage::new(pixmap.width(), pixmap.height());
    for (to, from) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = from.demultiply();
        *to = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(out)
}

fn svg_pixmap(
    bytes: &[u8],
    side: f32,
    options: &resvg::usvg::Options,
) -> Result<resvg::tiny_skia::Pixmap, String> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(bytes, options).map_err(|e| format!("not an SVG: {e}"))?;
    let size = tree.size();
    let scale = side / size.width().max(size.height());
    let (w, h) = (
        ((size.width() * scale).round() as u32).max(1),
        ((size.height() * scale).round() as u32).max(1),
    );
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("the SVG has no size")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// An SVG animated by CSS keyframes that switch its parts (see [`svg_anim`]), drawn moment by
/// moment with its long side at most `side` and all frames within `budget`; `None` if it does not
/// move.
fn svg_animation(bytes: &[u8], side: f32, budget: usize) -> Result<Option<Picture>, String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(None);
    };
    let svg_anim::Plan::Frames(moments) = svg_anim::plan(text) else {
        return Ok(None);
    };
    let options = svg_options();
    let first = svg_pixels(moments[0].0.as_bytes(), side, &options)?;
    // All frames within the budget: drawn smaller if need be.
    let per_frame = first.as_raw().len() * moments.len();
    let side = if per_frame > budget {
        let shrink = (budget as f64 / per_frame as f64).sqrt() as f32;
        (side * shrink).max(ANIMATION_MIN_SIDE as f32)
    } else {
        side
    };
    if moments.len() == 1 {
        let mut pixels = svg_pixels(moments[0].0.as_bytes(), side, &options)?;
        to_bgra(&mut pixels);
        return Ok(Some(Picture {
            still: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
            moving: None,
        }));
    }
    let mut frames = Vec::with_capacity(moments.len());
    for (text, ms) in &moments {
        let mut pixels = svg_pixels(text.as_bytes(), side, &options)?;
        to_bgra(&mut pixels);
        frames.push(Frame::from_parts(
            pixels,
            0,
            0,
            image::Delay::from_numer_denom_ms(*ms, 1),
        ));
    }
    Ok(Some(animated(frames)))
}

/// How large an SVG is drawn when shown: big enough for the full view.
const SVG_SIDE: f32 = 2048.;

/// Whether an attachment may be animated: its original is loaded instead of the still thumbnail.
pub fn maybe_animated(content_type: &str, filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    matches!(content_type, "image/gif" | "image/webp" | "image/apng")
        || [".gif", ".webp", ".apng"]
            .iter()
            .any(|e| lower.ends_with(e))
}

type ImageKey = (InstanceId, Id);

#[derive(Default)]
struct DrawnMedia {
    pictures: HashMap<ImageKey, Attachment>,
    profiles: HashMap<ImageKey, (String, u32)>,
}

#[derive(Default)]
pub struct Images {
    pub thumbs: HashMap<(InstanceId, Id), Img>,
    /// When each thumbnail arrived: one that just did fades in, one shown again does not.
    pub thumb_arrived: HashMap<(InstanceId, Id), std::time::Instant>,
    pub avatars: HashMap<(InstanceId, Id), Img>,
    /// The thumbnails in the order they were asked for: the oldest go first.
    thumb_order: std::collections::VecDeque<(InstanceId, Id)>,
    /// Pictures no longer used, to be freed the next time the window draws.
    pub retired: Vec<Arc<RenderImage>>,
    /// The viewer's picture as last drawn, freed from the GPU once the viewer lets go of it.
    viewer_drawn: Option<Picture>,
    drawn: Rc<RefCell<DrawnMedia>>,
    visible: HashMap<ImageKey, Attachment>,
    originals: VecDeque<(ImageKey, Arc<Vec<u8>>)>,
    banner_saved: RefCell<Option<(ImageKey, std::time::Instant)>>,
    active: HashSet<ImageKey>,
    failed: HashSet<ImageKey>,
    /// Originals fetched ahead of a click once already: not fetched ahead again once evicted.
    warmed: HashSet<ImageKey>,
    accounts: HashMap<InstanceId, String>,
}

/// Originals kept in memory; ones on screen or open stay even past it.
const ORIGINALS_BYTES: usize = 192 << 20;
/// Larger originals are only downloaded when opened.
const WARM_MAX_BYTES: u64 = 16 << 20;

fn original_preview(a: &Attachment) -> bool {
    maybe_animated(&a.content_type, &a.filename)
        || is_svg(&a.content_type, &a.filename)
        || a.sealed
            .as_ref()
            .map_or(!a.has_thumb, |s| s.thumb.is_none())
}

fn when_visible(work: impl FnOnce() + 'static) -> AnyElement {
    gpui_kit::canvas(
        move |bounds, window, _| {
            if bounds.intersects(&window.content_mask().bounds) {
                work();
            }
        },
        |_, (), _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Thumbnails kept in memory.
const MAX_THUMBNAILS: usize = 400;

impl Images {
    pub fn account(&self, host: &str) -> Option<String> {
        self.accounts.get(host).cloned()
    }

    pub fn track_banner(&self, key: ImageKey, started: std::time::Instant) {
        self.banner_saved.replace(Some((key, started)));
    }

    fn original(&self, key: &ImageKey) -> Option<Arc<Vec<u8>>> {
        self.originals
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, b)| b.clone())
    }

    fn originals_bytes(&self) -> usize {
        self.originals.iter().map(|(_, b)| b.len()).sum()
    }

    fn trim_originals(&mut self, viewer: Option<&ImageKey>, budget: usize) {
        let mut bytes = self.originals_bytes();
        while bytes > budget {
            let Some(i) = self
                .originals
                .iter()
                .position(|(k, _)| !self.visible.contains_key(k) && Some(k) != viewer)
            else {
                break;
            };
            bytes -= self.originals.remove(i).unwrap().1.len();
        }
    }

    pub fn attachment_marker(&self, inst: &str, attachment: &Attachment) -> AnyElement {
        let drawn = self.drawn.clone();
        let key = (inst.to_owned(), attachment.id);
        let attachment = attachment.clone();
        when_visible(move || {
            drawn.borrow_mut().pictures.insert(key, attachment);
        })
    }

    pub fn profile_marker(&self, inst: &str, file: &AttachmentRef, max: u32) -> Option<AnyElement> {
        let key = (inst.to_owned(), file.id);
        if self.avatars.contains_key(&key) {
            return None;
        }
        let drawn = self.drawn.clone();
        let filename = file.filename.clone();
        Some(when_visible(move || {
            let mut drawn = drawn.borrow_mut();
            let entry = drawn.profiles.entry(key).or_insert((filename, max));
            entry.1 = entry.1.max(max);
        }))
    }

    /// At the start of each draw: frees the GPU copies of pictures nothing draws any more
    /// (dropped or replaced ones, and the full-size picture of a closed viewer; gpui keeps every
    /// drawn image until told).
    pub fn free_unused(&mut self, viewer: Option<&Img>, window: &mut Window) {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        let shown = match viewer {
            Some(Img::Ready(picture)) => Some(picture),
            _ => None,
        };
        if let Some(old) = self.viewer_drawn.take() {
            let keep = shown.map(Picture::gpu_images).unwrap_or_default();
            for image in old.gpu_images() {
                if !keep.iter().any(|k| Arc::ptr_eq(k, &image)) {
                    let _ = window.drop_image(image);
                }
            }
        }
        self.viewer_drawn = shown.cloned();
    }

    fn retire(&mut self, picture: Img) {
        if let Img::Ready(picture) = picture {
            self.retired.extend(picture.gpu_images());
        }
    }

    /// Puts a picture made here in place of a thumbnail (a video's sharper poster, #76); it
    /// fades in as one that just arrived.
    pub fn put_thumbnail(&mut self, key: ImageKey, picture: Picture) {
        match self.thumbs.remove(&key) {
            Some(old) => self.retire(old),
            None => {
                self.evict_thumbnails();
                self.thumb_order.push_back(key.clone());
            }
        }
        self.thumb_arrived
            .insert(key.clone(), std::time::Instant::now());
        self.thumbs.insert(key, Img::Ready(picture));
    }

    /// Makes room for one more thumbnail.
    fn evict_thumbnails(&mut self) {
        while self.thumb_order.len() >= MAX_THUMBNAILS {
            let Some(oldest) = self.thumb_order.pop_front() else {
                break;
            };
            self.thumb_arrived.remove(&oldest);
            if let Some(picture) = self.thumbs.remove(&oldest) {
                self.retire(picture);
            }
        }
    }
}

/// Decodes image bytes for drawing. gpui wants BGRA.
/// The size encrypted pictures are made into previews at, here on the device.
const SEALED_PREVIEW: u32 = 800;

pub fn decode(bytes: &[u8], max: Option<u32>) -> Result<Picture, String> {
    decode_within(bytes, max, ANIMATION_BUDGET)
}

/// [`decode`] for a picture that stays loaded: an animation gets a smaller budget.
pub(crate) fn decode_preview(bytes: &[u8], max: Option<u32>) -> Result<Picture, String> {
    decode_within(bytes, max, PREVIEW_ANIMATION_BUDGET)
}

/// [`decode`] for the viewer (`max` `None`) or a chat preview: an animation too big to hold
/// decoded at `max` is played from `bytes` instead of being made smaller.
pub(crate) fn decode_streamed(bytes: Arc<Vec<u8>>, max: Option<u32>) -> Result<Picture, String> {
    let budget = if max.is_none() {
        ANIMATION_BUDGET
    } else {
        PREVIEW_ANIMATION_BUDGET
    };
    if looks_like_svg(&bytes) {
        return decode_within(&bytes, max, budget);
    }
    // A WebP says its size and frame count up front: one too big is not decoded twice.
    let too_big = crate::webp_anim::size_and_count(&bytes).is_some_and(|((w, h), n)| {
        let (w, h) = fitted((w, h), max);
        w as usize * h as usize * 4 * n > budget
    });
    if !too_big {
        match frames_fitting(&bytes, max, budget, false) {
            Ok(Fit::Frames(frames)) => return Ok(animated(frames)),
            Ok(Fit::TooBig) => {}
            Ok(Fit::Still) => return decode_still(&bytes, max),
            Err(err) => {
                tracing::debug!("not animated: {err}");
                return decode_still(&bytes, max);
            }
        }
    }
    let (mut pixels, _) = BgraFrames::new(&bytes)?
        .and_then(|mut frames| frames.next_frame())
        .ok_or("no frames")??;
    if let Some(max) = max {
        pixels = fit(pixels, max);
    }
    Ok(Picture {
        still: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
        moving: Some(Moving::Stream(stream::AnimStream::new(bytes, max))),
    })
}

/// The size `size` is made to fit `max` at, as [`fit`] makes it.
fn fitted((w, h): (u32, u32), max: Option<u32>) -> (u32, u32) {
    match max {
        Some(side) if w > side || h > side => {
            let scale = f64::from(side) / f64::from(w.max(h));
            (
                ((f64::from(w) * scale).round() as u32).max(1),
                ((f64::from(h) * scale).round() as u32).max(1),
            )
        }
        _ => (w, h),
    }
}

/// A picture of `frames` (BGRA), all decoded.
fn animated(frames: Vec<Frame>) -> Picture {
    let held = Held::new(frames);
    Picture {
        still: held.frames[0].clone(),
        moving: Some(Moving::Frames(Arc::new(held))),
    }
}

fn decode_within(bytes: &[u8], max: Option<u32>, budget: usize) -> Result<Picture, String> {
    // An SVG is drawn first (#5), then read like any picture.
    if looks_like_svg(bytes) {
        let side = max.map_or(SVG_SIDE, |m| m as f32);
        if let Some(picture) = svg_animation(bytes, side, budget)? {
            return Ok(picture);
        }
        let png = svg_to_png(bytes, side)?;
        return decode_within(&png, max, budget);
    }
    let moving = match frames_within(bytes, max, budget) {
        Ok(Some(frames)) => Some(frames),
        Ok(None) => None,
        Err(err) => {
            tracing::debug!("not animated: {err}");
            None
        }
    };
    if let Some(frames) = moving {
        return Ok(animated(frames));
    }
    decode_still(bytes, max)
}

/// A picture as one still frame, at most `max` on a side.
fn decode_still(bytes: &[u8], max: Option<u32>) -> Result<Picture, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let mut pixels = image.to_rgba8();
    if let Some(max) = max {
        pixels = picture::fit(pixels, max, image::imageops::FilterType::Lanczos3);
    }
    to_bgra(&mut pixels);
    Ok(Picture {
        still: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
        moving: None,
    })
}

/// Only a picture's first frame, at most `max` pixels on a side, decoded and shrunk quickly: what
/// is shown right after picking a file. See [`decode_animation`] for the rest of it.
#[cfg(test)]
pub fn decode_first_frame(bytes: &[u8], max: u32) -> Result<Picture, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    Ok(first_frame_pixels(&image.into_rgba8(), max))
}

pub fn first_frame_pixels(source: &image::RgbaImage, max: u32) -> Picture {
    let mut pixels = if source.width() > max || source.height() > max {
        picture::thumbnail(source, max, max)
    } else {
        source.clone()
    };
    to_bgra(&mut pixels);
    Picture {
        still: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
        moving: None,
    }
}

/// All frames of an animated picture at most `side` pixels on a side, for a picture looked at
/// closely for a while (the crop view): the larger budget of the viewer.
pub fn decode_animation_sized(bytes: &[u8], side: u32) -> Result<Option<Moving>, String> {
    let frames = frames_within(bytes, Some(side), ANIMATION_BUDGET)?;
    Ok(frames.map(|frames| Moving::Frames(Arc::new(Held::new(frames)))))
}

/// gpui wants BGRA.
fn to_bgra(pixels: &mut image::RgbaImage) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

/// The frames of a GIF, an animated WebP or an APNG, undecoded until asked for; `None` for any
/// other picture. A GIF of one frame counts too.
pub fn animation_decoder(bytes: &[u8]) -> Result<Option<image::Frames<'_>>, String> {
    use image::AnimationDecoder as _;
    use std::io::Cursor;
    let err = |e: image::ImageError| e.to_string();
    Ok(Some(match image::guess_format(bytes).map_err(err)? {
        image::ImageFormat::Gif => image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .map_err(err)?
            .into_frames(),
        // Put together here: `image-webp` leaves disposed frames behind (see `webp_anim`).
        image::ImageFormat::WebP => match crate::webp_anim::frames(bytes) {
            Some(frames) => frames,
            None => return Ok(None),
        },
        image::ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).map_err(err)?;
            if !decoder.is_apng().map_err(err)? {
                return Ok(None);
            }
            decoder.apng().map_err(err)?.into_frames()
        }
        _ => return Ok(None),
    }))
}

/// The frames of an animation in BGRA (what gpui draws), decoded as they are asked for.
pub(crate) enum BgraFrames<'a> {
    /// libwebp writes BGRA itself.
    WebP(crate::webp_anim::Decoder<'a>),
    Other(image::Frames<'a>),
}

impl<'a> BgraFrames<'a> {
    /// `None` for a picture that is not a GIF, an animated WebP or an APNG.
    pub fn new(bytes: &'a [u8]) -> Result<Option<Self>, String> {
        if let Some(decoder) = crate::webp_anim::Decoder::new(bytes, true) {
            return Ok(Some(BgraFrames::WebP(decoder)));
        }
        Ok(animation_decoder(bytes)?.map(BgraFrames::Other))
    }

    /// The next frame and how long it shows; `None` after the last one.
    pub fn next_frame(&mut self) -> Option<Result<(image::RgbaImage, image::Delay), String>> {
        match self {
            BgraFrames::WebP(decoder) => decoder.next_frame(),
            BgraFrames::Other(frames) => {
                Some(frames.next()?.map_err(|e| e.to_string()).map(|frame| {
                    let delay = frame.delay();
                    let mut pixels = frame.into_buffer();
                    to_bgra(&mut pixels);
                    (pixels, delay)
                }))
            }
        }
    }
}

/// The frames of an animated GIF, WebP or PNG, at most `max` pixels on a side and within
/// `budget` bytes, in BGRA. `None`: one frame only, or too long to keep moving.
fn frames_within(
    bytes: &[u8],
    max: Option<u32>,
    budget: usize,
) -> Result<Option<Vec<Frame>>, String> {
    Ok(match frames_fitting(bytes, max, budget, true)? {
        Fit::Frames(frames) => Some(frames),
        Fit::Still | Fit::TooBig => None,
    })
}

enum Fit {
    /// Not animated.
    Still,
    Frames(Vec<Frame>),
    /// Its frames do not fit the budget (at the asked size, or at any when shrinking).
    TooBig,
}

/// [`frames_within`]; without `shrink`, an animation over the budget is [`Fit::TooBig`] instead
/// of made smaller.
fn frames_fitting(
    bytes: &[u8],
    max: Option<u32>,
    budget: usize,
    shrink: bool,
) -> Result<Fit, String> {
    let Some(mut frames) = BgraFrames::new(bytes)? else {
        return Ok(Fit::Still);
    };
    let mut side = max.unwrap_or(u32::MAX);
    let mut out: Vec<Frame> = Vec::new();
    let mut bytes_used = 0usize;
    while let Some(frame) = frames.next_frame() {
        let (pixels, delay) = frame?;
        let mut pixels = fit(pixels, side);
        bytes_used += pixels.as_raw().len();
        if bytes_used > budget && !shrink {
            return Ok(Fit::TooBig);
        }
        // Over the budget: everything so far and everything after at half the size.
        while bytes_used > budget {
            side = pixels.width().max(pixels.height()) / 2;
            if side < ANIMATION_MIN_SIDE {
                return Ok(Fit::TooBig);
            }
            for f in &mut out {
                let smaller = fit(f.buffer().clone(), side);
                *f = Frame::from_parts(smaller, 0, 0, f.delay());
            }
            pixels = fit(pixels, side);
            bytes_used = out.iter().map(|f| f.buffer().as_raw().len()).sum::<usize>()
                + pixels.as_raw().len();
        }
        out.push(Frame::from_parts(pixels, 0, 0, delay));
    }
    Ok(if out.len() > 1 {
        Fit::Frames(out)
    } else {
        Fit::Still
    })
}

/// `pixels` made to fit `side` on its long side, keeping its shape. While it is still at least
/// twice too big it is halved by averaging each 2×2 block, which looks the same as the filter
/// over that span and costs a fraction of it (a streamed full-HD frame for the chat: #60).
fn fit(mut pixels: image::RgbaImage, side: u32) -> image::RgbaImage {
    while pixels.width().max(pixels.height()) >= side.saturating_mul(2)
        && pixels.width() >= 2
        && pixels.height() >= 2
    {
        pixels = halve(&pixels);
    }
    picture::fit(pixels, side, image::imageops::FilterType::Triangle)
}

/// `pixels` at half the size, each pixel the average of a 2×2 block (an odd last row or column
/// is left out).
fn halve(pixels: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = (pixels.width() / 2, pixels.height() / 2);
    let stride = pixels.width() as usize * 4;
    let source = pixels.as_raw();
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let top = &source[2 * y * stride..][..stride];
        let bottom = &source[(2 * y + 1) * stride..][..stride];
        for x in 0..w as usize {
            let i = x * 8;
            for c in 0..4 {
                let sum = u16::from(top[i + c])
                    + u16::from(top[i + 4 + c])
                    + u16::from(bottom[i + c])
                    + u16::from(bottom[i + 4 + c]);
                out.push(((sum + 2) / 4) as u8);
            }
        }
    }
    image::RgbaImage::from_raw(w, h, out).expect("the size matches")
}

impl Shell {
    /// Resolve this frame's clipped attachment and profile elements after layout.
    pub fn prepare_media_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let accounts = self.core.connected_accounts();
        for (host, old) in &std::mem::take(&mut self.images.accounts) {
            if accounts.get(host) == Some(old) {
                continue;
            }
            let images = &mut self.images;
            images.originals.retain(|(k, _)| &k.0 != host);
            images.active.retain(|k| &k.0 != host);
            images.failed.retain(|k| &k.0 != host);
            images.warmed.retain(|k| &k.0 != host);
            images.thumb_order.retain(|k| &k.0 != host);
            images.thumb_arrived.retain(|k, _| &k.0 != host);
            let stale: Vec<_> = images
                .thumbs
                .extract_if(|k, _| &k.0 == host)
                .chain(images.avatars.extract_if(|k, _| &k.0 == host))
                .map(|(_, p)| p)
                .collect();
            for img in stale {
                images.retire(img);
            }
            if self.viewer.as_ref().is_some_and(|v| &v.inst == host) {
                self.viewer = None;
            }
        }
        self.images.accounts = accounts;
        self.images.drawn.replace(DrawnMedia::default());
        let shell = cx.entity().downgrade();
        window.defer(cx, move |_, cx| {
            let _ = shell.update(cx, |this, cx| this.load_drawn_media(cx));
        });
    }

    fn load_drawn_media(&mut self, cx: &mut Context<Self>) {
        let drawn = self.images.drawn.take();
        self.images.visible = drawn.pictures;
        self.images
            .failed
            .retain(|k| self.images.visible.contains_key(k));
        let viewer = self
            .viewer
            .as_ref()
            .map(|v| (v.inst.clone(), v.attachment.id));
        self.images.trim_originals(viewer.as_ref(), ORIGINALS_BYTES);
        let pictures: Vec<_> = self
            .images
            .visible
            .iter()
            .map(|(k, a)| (k.0.clone(), a.clone()))
            .collect();
        for (host, attachment) in &pictures {
            if !original_preview(attachment) {
                self.load_thumbnail(host, attachment, cx);
            }
        }
        self.start_video_fills(cx);
        // Originals that are themselves needed to draw a preview go first; the rest are fetched
        // ahead of a click once each, within the memory budget.
        for need_preview in [true, false] {
            for (host, a) in &pictures {
                let key = (host.clone(), a.id);
                let images = &self.images;
                if images.failed.contains(&key) || images.active.contains(&key) {
                    continue;
                }
                let preview = original_preview(a) && !images.thumbs.contains_key(&key);
                if need_preview != preview {
                    continue;
                }
                if !preview
                    && (images.warmed.contains(&key)
                        || a.size > WARM_MAX_BYTES
                        || !matches!(images.thumbs.get(&key), Some(Img::Ready(_)))
                        || images.original(&key).is_some()
                        || images.originals_bytes() + a.size as usize > ORIGINALS_BYTES)
                {
                    continue;
                }
                if images.active.iter().filter(|k| &k.0 == host).count() >= 2 {
                    continue;
                }
                self.images.warmed.insert(key.clone());
                self.request_original(key, a.clone(), false, cx);
            }
        }

        for (key, (filename, max)) in drawn.profiles {
            if self.images.avatars.contains_key(&key) {
                continue; // two users with the same avatar
            }
            self.images.avatars.insert(key.clone(), Img::Loading);
            let core = self.core.clone();
            let (inst, id) = key.clone();
            let account = self.images.accounts.get(&inst).cloned();
            self.run(
                cx,
                async move {
                    let rest = core.rest(&inst)?;
                    // The thumbnail of an animated picture is still: its original moves.
                    let bytes = if maybe_animated("", &filename) {
                        rest.download(id, &filename).await
                    } else {
                        rest.thumbnail(id).await
                    }?;
                    tokio::task::spawn_blocking(move || decode_preview(&bytes, Some(max)))
                        .await
                        .map_err(|e| e.to_string())?
                },
                move |this, result, _, _| {
                    let img = match result {
                        Ok(image) => Img::Ready(image),
                        Err(err) => {
                            tracing::warn!("avatar {}: {err}", key.1);
                            Img::Failed
                        }
                    };
                    if this.images.accounts.get(&key.0) == account.as_ref()
                        && matches!(this.images.avatars.get(&key), Some(Img::Loading))
                    {
                        this.images.avatars.insert(key, img);
                    }
                },
            );
        }
    }

    /// Someone's profile banner, once loaded.
    pub fn banner_picture(&self, inst: &str, user: Option<&User>) -> Option<Arc<RenderImage>> {
        let banner = user?.banner.as_ref()?;
        let key = (inst.to_owned(), banner.id);
        match self.images.avatars.get(&key) {
            Some(Img::Ready(picture)) => {
                let mut saved = self.images.banner_saved.borrow_mut();
                if saved.as_ref().is_some_and(|(k, _)| k == &key)
                    && let Some((_, at)) = saved.take()
                {
                    tracing::info!(
                        elapsed_ms = at.elapsed().as_millis(),
                        "new banner displayed"
                    );
                }
                Some(picture.shown(self.animating()))
            }
            _ => None,
        }
    }

    /// A server's icon, once loaded.
    pub fn space_picture(
        &self,
        inst: &str,
        space: Id,
    ) -> (Option<Arc<RenderImage>>, Option<AnyElement>) {
        let store = self.core.store();
        let Some(icon) = store
            .instance(inst)
            .and_then(|s| s.spaces.get(&space))
            .and_then(|s| s.space.icon.as_ref())
        else {
            return (None, None);
        };
        let picture = match self.images.avatars.get(&(inst.to_owned(), icon.id)) {
            Some(Img::Ready(picture)) => Some(picture.shown(self.animating())),
            _ => None,
        };
        (
            picture,
            self.images.profile_marker(inst, icon, AVATAR_PIXELS),
        )
    }

    /// Frees every thumbnail; those on screen load again.
    pub fn clear_pictures(&mut self) {
        let pictures: Vec<Img> = self.images.thumbs.drain().map(|(_, p)| p).collect();
        self.images.thumb_order.clear();
        for picture in pictures {
            self.images.retire(picture);
        }
    }

    fn begin_thumbnail(&mut self, key: &ImageKey) {
        self.images.evict_thumbnails();
        self.images.thumb_order.push_back(key.clone());
        self.images.thumbs.insert(key.clone(), Img::Loading);
    }

    fn finish_thumbnail(&mut self, key: ImageKey, result: Result<Picture, String>) {
        if !matches!(self.images.thumbs.get(&key), Some(Img::Loading)) {
            return;
        }
        let image = match result {
            Ok(picture) => {
                self.images
                    .thumb_arrived
                    .insert(key.clone(), std::time::Instant::now());
                Img::Ready(picture)
            }
            Err(err) => {
                tracing::warn!("preview {}: {err}", key.1);
                Img::Failed
            }
        };
        self.images.thumbs.insert(key, image);
    }

    pub fn load_thumbnail(&mut self, inst: &InstanceId, a: &Attachment, cx: &mut Context<Self>) {
        let key = (inst.clone(), a.id);
        if self.images.thumbs.contains_key(&key) {
            return;
        }
        self.begin_thumbnail(&key);
        let core = self.core.clone();
        let account = self.images.accounts.get(inst).cloned();
        let (inst, attachment) = (inst.clone(), a.clone());
        self.run(
            cx,
            async move {
                let bytes = core.thumbnail(&inst, &attachment).await?;
                tokio::task::spawn_blocking(move || decode_preview(&bytes, Some(SEALED_PREVIEW)))
                    .await
                    .map_err(|e| e.to_string())?
            },
            move |this, result, _, _| {
                if this.images.accounts.get(&key.0) == account.as_ref() {
                    this.finish_thumbnail(key, result);
                }
            },
        );
    }

    pub fn request_original(
        &mut self,
        key: ImageKey,
        a: Attachment,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(bytes) = self.images.original(&key) {
            self.decode_original(key, &a, bytes, open, cx);
            return;
        }
        if !self.images.active.insert(key.clone()) {
            return;
        }
        let core = self.core.clone();
        let host = key.0.clone();
        let account = self.images.accounts.get(&host).cloned();
        let file = a.clone();
        self.run(
            cx,
            async move { core.download(&host, &file).await },
            move |this, result, _, cx| {
                if this.images.accounts.get(&key.0) != account.as_ref() {
                    return;
                }
                this.images.active.remove(&key);
                let open = this
                    .viewer
                    .as_ref()
                    .is_some_and(|v| v.inst == key.0 && v.attachment.id == key.1);
                match result {
                    Ok(bytes) => {
                        let bytes = Arc::new(bytes);
                        this.images
                            .originals
                            .push_back((key.clone(), bytes.clone()));
                        this.decode_original(key, &a, bytes, open, cx);
                    }
                    Err(err) => {
                        tracing::warn!("original {}: {err}", key.1);
                        this.images.failed.insert(key.clone());
                        if open {
                            this.viewer.as_mut().unwrap().image = Img::Failed;
                        }
                        if original_preview(&a) && !this.images.thumbs.contains_key(&key) {
                            this.begin_thumbnail(&key);
                            this.finish_thumbnail(key, Err(err.to_string()));
                        }
                    }
                }
            },
        );
    }

    fn decode_original(
        &mut self,
        key: ImageKey,
        a: &Attachment,
        bytes: Arc<Vec<u8>>,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        let account = self.images.accounts.get(&key.0).cloned();
        if original_preview(a)
            && self.images.visible.contains_key(&key)
            && !matches!(
                self.images.thumbs.get(&key),
                Some(Img::Loading | Img::Ready(_))
            )
        {
            self.begin_thumbnail(&key);
            let bytes = bytes.clone();
            let preview_key = key.clone();
            let preview_account = account.clone();
            let max = if maybe_animated(&a.content_type, &a.filename)
                || is_svg(&a.content_type, &a.filename)
            {
                ANIMATED_PREVIEW
            } else {
                SEALED_PREVIEW
            };
            self.run(
                cx,
                async move {
                    tokio::task::spawn_blocking(move || decode_streamed(bytes, Some(max)))
                        .await
                        .map_err(|e| e.to_string())?
                },
                move |this, result, _, _| {
                    if this.images.accounts.get(&preview_key.0) == preview_account.as_ref() {
                        if result.is_err() {
                            this.images.failed.insert(preview_key.clone());
                        }
                        this.finish_thumbnail(preview_key, result);
                    }
                },
            );
        }
        if open {
            self.run(
                cx,
                async move {
                    tokio::task::spawn_blocking(move || decode_streamed(bytes, None))
                        .await
                        .map_err(|e| e.to_string())?
                },
                move |this, result, _, _| {
                    if this.images.accounts.get(&key.0) == account.as_ref()
                        && let Some(viewer) = &mut this.viewer
                        && viewer.inst == key.0
                        && viewer.attachment.id == key.1
                    {
                        viewer.image = result.map(Img::Ready).unwrap_or_else(|err| {
                            tracing::warn!("image {}: {err}", key.1);
                            Img::Failed
                        });
                    }
                },
            );
        }
    }

    /// An avatar in the chosen shape (round or a squircle): the user's picture, else a silhouette
    /// in the theme's colours (also while the picture loads).
    pub fn avatar(&self, inst: &str, user: Option<&User>, size: f32) -> AnyElement {
        let picture = user
            .and_then(|u| u.avatar.as_ref())
            .and_then(|a| self.images.avatars.get(&(inst.to_owned(), a.id)));
        if let Some(Img::Ready(picture)) = picture {
            return img(picture.shown(self.animating()))
                .flex_shrink_0()
                .size(px(size))
                .rounded(px(theme::avatar_radius(size)))
                .object_fit(gpui_kit::ObjectFit::Cover)
                .id(SharedString::from(format!(
                    "pic-{:p}",
                    Arc::as_ptr(&picture.still)
                )))
                .into_any_element();
        }
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(size))
            .rounded(px(theme::avatar_radius(size)))
            .overflow_hidden()
            .bg(theme::selected())
            .relative()
            .children(
                user.and_then(|u| u.avatar.as_ref())
                    .and_then(|file| self.images.profile_marker(inst, file, AVATAR_PIXELS)),
            )
            .child(crate::assets::icon(
                crate::assets::SILHOUETTE,
                size,
                theme::text_tertiary(),
            ))
            .into_any_element()
    }

    /// An avatar with the presence dot on its corner, on the sidebar's colour.
    pub fn avatar_with_status(
        &self,
        inst: &str,
        user: Option<&User>,
        size: f32,
        status: Status,
    ) -> AnyElement {
        self.avatar_with_status_on(inst, user, size, status, theme::sidebar())
    }

    /// The same on another surface: `ground` is the colour around the avatar, which the dot's
    /// ring and cut-outs take so they read as holes.
    pub fn avatar_with_status_on(
        &self,
        inst: &str,
        user: Option<&User>,
        size: f32,
        status: Status,
        ground: gpui_kit::Rgba,
    ) -> AnyElement {
        let dot = presence_dot_size(size);
        div()
            .relative()
            .flex_shrink_0()
            .size(px(size))
            .child(self.avatar(inst, user, size))
            .child(
                div()
                    .absolute()
                    .bottom(px(-1.))
                    .right(px(-1.))
                    .child(presence_dot(status, dot, ground)),
            )
            .into_any_element()
    }
}

/// How large the presence dot on an avatar `size` across is.
pub(crate) fn presence_dot_size(size: f32) -> f32 {
    (size / 3.).max(10.)
}

/// A presence dot `dot` across in a ring of `ground` (the colour around it), which its
/// cut-outs take too so they read as holes. A new status shows at once, without motion.
pub(crate) fn presence_dot(status: Status, dot: f32, ground: gpui_kit::Rgba) -> gpui_kit::Div {
    div()
        .flex_shrink_0()
        .size(px(dot))
        .rounded(px(dot / 2.))
        .border_2()
        .border_color(ground)
        .bg(theme::status(status))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .children(presence_shape(status, dot, ground))
}

/// What is cut out of a presence dot, so that a status never depends on its colour alone:
/// a bar for do-not-disturb, a crescent for idle, a ring for offline.
fn presence_shape(status: Status, dot: f32, ground: gpui_kit::Rgba) -> Option<gpui_kit::Div> {
    let cut = move || div().bg(ground);
    let inner = dot - 4.;
    match status {
        Status::Online => None,
        Status::Dnd => Some(
            cut()
                .w(px(inner * 0.7))
                .h(px((inner * 0.25).max(1.5)))
                .rounded(px(1.)),
        ),
        Status::Offline => Some(cut().size(px(inner * 0.5)).rounded(px(inner * 0.25))),
        Status::Idle => Some(
            cut()
                .absolute()
                .top(px(-inner * 0.1))
                .left(px(-inner * 0.1))
                .size(px(inner * 0.7))
                .rounded(px(inner * 0.35)),
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A GIF of `frames` frames, `side` pixels square, each a different grey.
    pub fn gif(frames: u8, side: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            let frames = (0..frames).map(|n| {
                let pixels = image::RgbaImage::from_pixel(
                    side,
                    side,
                    image::Rgba([n.wrapping_mul(20), 0, 0, 255]),
                );
                Frame::from_parts(pixels, 0, 0, image::Delay::from_numer_denom_ms(50, 1))
            });
            encoder.encode_frames(frames).unwrap();
        }
        bytes
    }

    #[test]
    fn originals_keep_the_last_few() {
        let mut images = Images::default();
        for n in 0..8 {
            images
                .originals
                .push_back((("h".into(), Id(n)), Arc::new(vec![0; 10])));
        }
        let open = ("h".into(), Id(0));
        images.trim_originals(Some(&open), 60);
        assert!(images.original(&open).is_some(), "the open one stays");
        assert!(images.original(&("h".into(), Id(1))).is_none());
        assert!(images.original(&("h".into(), Id(7))).is_some());
        assert_eq!(images.originals.len(), 6);
    }

    #[test]
    fn svg_pictures_are_drawn() {
        let svg = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#e8a33d"/></svg>"##;
        assert!(looks_like_svg(svg));
        assert!(!looks_like_svg(b"\x89PNG\r\n"));
        assert!(is_svg("application/octet-stream", "Logo.SVG"));
        let picture = decode(svg, Some(200)).expect("an SVG is drawn");
        let size = picture.still.size(0);
        assert_eq!(
            (size.width.0, size.height.0),
            (200, 100),
            "drawn at the asked size"
        );
    }

    #[test]
    fn a_flipbook_svg_moves() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><style>.f{visibility:hidden;animation:0.5s step-end infinite}@keyframes k0{0%{visibility:visible}50%{visibility:hidden}}.f0{animation-name:k0}@keyframes k1{0%{visibility:hidden}50%{visibility:visible}100%{visibility:hidden}}.f1{animation-name:k1}</style><rect class="f f0" width="2" height="2" fill="red"/><rect class="f f1" x="2" width="2" height="2" fill="blue"/></svg>"#;
        let picture = decode(svg, Some(4)).unwrap();
        let Some(Moving::Frames(frames)) = &picture.moving else {
            panic!("animated");
        };
        assert_eq!(frames.len(), 2);
        // BGRA: the red half shows first, then only the blue one.
        let first = frames.frame(0).as_bytes(0).unwrap();
        let second = frames.frame(1).as_bytes(0).unwrap();
        assert_eq!(&first[..4], &[0, 0, 255, 255]);
        assert_eq!(first[4 * 3 + 3], 0, "blue hidden");
        assert_eq!(second[3], 0, "red hidden");
        assert_eq!(&second[4 * 3..4 * 3 + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn an_animated_gif_keeps_its_frames_and_a_still_first_one() {
        let picture = decode(&gif(3, 16), None).unwrap();
        let Some(Moving::Frames(moving)) = &picture.moving else {
            panic!("all frames held");
        };
        assert_eq!(moving.len(), 3);
        assert_eq!(picture.still.frame_count(), 1);
        assert!(
            Arc::ptr_eq(&picture.still, moving.frame(0)),
            "the still is its first frame"
        );
    }

    #[test]
    fn a_held_animation_shows_the_frame_due_and_says_when_the_next_is() {
        let picture = decode(&gif(3, 4), None).unwrap();
        let Some(Moving::Frames(held)) = &picture.moving else {
            panic!("held");
        };
        // Three frames of 50 ms: 120 ms in is the third, the first again 30 ms later.
        let now = held.start + Duration::from_millis(120);
        let (n, next) = held.at(now);
        assert_eq!(n, 2);
        assert_eq!(next - now, Duration::from_millis(30));
        let (n, _) = held.at(held.start + Duration::from_millis(155));
        assert_eq!(n, 0, "and round again");
        let _ = take_wake();
        let shown = picture.shown(true);
        assert!(take_wake().is_some(), "drawing it asks for the next frame");
        assert!(held.frames.iter().any(|f| Arc::ptr_eq(f, &shown)));
        picture.shown(false);
        assert!(take_wake().is_none(), "a still one asks for nothing");
    }

    #[test]
    fn halving_averages_each_block() {
        let mut pixels = image::RgbaImage::new(5, 2);
        pixels.put_pixel(0, 0, image::Rgba([200, 0, 0, 255]));
        pixels.put_pixel(1, 1, image::Rgba([100, 40, 0, 255]));
        let half = halve(&pixels);
        assert_eq!(half.dimensions(), (2, 1));
        assert_eq!(half.get_pixel(0, 0).0, [75, 10, 0, 128]);
        let fitted = fit(image::RgbaImage::new(1920, 1080), 480);
        assert_eq!(fitted.dimensions(), (480, 270));
    }

    #[test]
    fn a_one_frame_gif_is_still() {
        assert!(decode(&gif(1, 16), None).unwrap().moving.is_none());
    }

    #[test]
    fn a_long_animation_is_made_smaller_to_fit_the_budget() {
        // 30 frames of 200 px: 4.8 MB at full size, against a 1 MB budget.
        let frames = frames_within(&gif(30, 200), None, 1 << 20)
            .unwrap()
            .unwrap();
        let total: usize = frames.iter().map(|f| f.buffer().as_raw().len()).sum();
        assert!(total <= 1 << 20, "{total}");
        assert_eq!(frames.len(), 30);
        assert!(
            frames
                .iter()
                .all(|f| f.buffer().width() == frames[0].buffer().width())
        );
    }

    #[test]
    fn the_first_frame_comes_alone_and_the_animation_after_it() {
        let moving = gif(3, 64);
        let first = decode_first_frame(&moving, 32).unwrap();
        assert!(first.moving.is_none());
        assert_eq!(first.still.size(0).width.0, 32);
        assert!(
            decode_animation_sized(&moving, ANIMATED_PREVIEW)
                .unwrap()
                .is_some()
        );
        assert!(
            decode_animation_sized(&gif(1, 64), ANIMATED_PREVIEW)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_big_animation_streams_at_full_size_and_a_small_one_is_held() {
        // 12 frames of 1200 px: 69 MB decoded, over the 24 MB of a preview.
        let big = Arc::new(gif(12, 1200));
        let picture = decode_streamed(big.clone(), None).unwrap();
        assert!(
            matches!(picture.moving, Some(Moving::Frames(_))),
            "fits 96 MB"
        );
        let picture = decode_streamed(big, Some(1000)).unwrap();
        assert!(matches!(picture.moving, Some(Moving::Stream(_))));
        assert_eq!(picture.still.size(0).width.0, 1000, "not made smaller");
        let small = decode_streamed(Arc::new(gif(3, 64)), Some(480)).unwrap();
        assert!(matches!(small.moving, Some(Moving::Frames(_))));
        let still = decode_streamed(Arc::new(gif(1, 64)), Some(480)).unwrap();
        assert!(still.moving.is_none());
    }

    /// Times decoding an animation from `HERMES_ANIM_SAMPLE` (run with `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn time_an_animation() {
        let Ok(path) = std::env::var("HERMES_ANIM_SAMPLE") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let start = std::time::Instant::now();
        let picture = decode_streamed(Arc::new(bytes.clone()), None).unwrap();
        let size = picture.still.size(0);
        let how = match &picture.moving {
            None => "still".to_owned(),
            Some(Moving::Frames(f)) => format!("{} frames held", f.len()),
            Some(Moving::Stream(_)) => "streamed".to_owned(),
        };
        eprintln!(
            "viewer: {}x{} {how} in {:?}",
            size.width.0,
            size.height.0,
            start.elapsed()
        );
        // HERMES_ANIM_OUT: the held frames as PNGs there, to look at.
        if let (Ok(out), Some(Moving::Frames(frames))) =
            (std::env::var("HERMES_ANIM_OUT"), &picture.moving)
        {
            for n in 0..frames.len() {
                let frame = frames.frame(n);
                let size = frame.size(0);
                let mut pixels = image::RgbaImage::from_raw(
                    size.width.0 as u32,
                    size.height.0 as u32,
                    frame.as_bytes(0).unwrap().to_vec(),
                )
                .unwrap();
                to_bgra(&mut pixels);
                pixels.save(format!("{out}/frame{n}.png")).unwrap();
            }
        }
        let start = std::time::Instant::now();
        let Ok(Some(frames)) = animation_decoder(&bytes) else {
            return;
        };
        let mut n = 0;
        for frame in frames {
            frame.unwrap();
            n += 1;
        }
        let total = start.elapsed();
        eprintln!("{n} frames in {total:?}: {:?} each", total / n.max(1));
    }

    #[test]
    fn gifs_and_webps_may_be_animated() {
        assert!(maybe_animated("image/gif", "a"));
        assert!(maybe_animated("application/octet-stream", "Party.GIF"));
        assert!(!maybe_animated("image/png", "a.png"));
    }
}
