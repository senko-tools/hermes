//! Avatars and attachment images, loaded from the server once and decoded off the UI thread.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, RenderImage, SharedString, Window, div, img};
use hermes_client_core::InstanceId;
use hermes_client_core::picture;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;
use hermes_client_core::proto::models::{Status, User};
use image::Frame;

use crate::shell::Shell;
use crate::theme::{self, px};

/// The largest size an avatar is drawn at, in pixels on a 2x display.
const AVATAR_PIXELS: u32 = 192;
/// The same for a profile banner (300 by 120 on the card).
const BANNER_PIXELS: u32 = 640;

#[derive(Clone)]
pub enum Img {
    Loading,
    Ready(Picture),
    Failed,
}

/// A decoded picture: its first frame, and all of them when it is animated.
#[derive(Clone)]
pub struct Picture {
    pub still: Arc<RenderImage>,
    pub moving: Option<Arc<RenderImage>>,
}

impl Picture {
    /// What to draw: the animation only while `animate` (the window has the focus).
    pub fn shown(&self, animate: bool) -> Arc<RenderImage> {
        match (&self.moving, animate) {
            (Some(moving), true) => moving.clone(),
            _ => self.still.clone(),
        }
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
    use resvg::{tiny_skia, usvg};
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|e| format!("not an SVG: {e}"))?;
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
    pixmap.encode_png().map_err(|e| e.to_string())
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
    /// Originals downloaded lately (for animated previews and SVGs): the full view opens from
    /// them instead of downloading the file again (#9). Shared with the background tasks.
    pub originals: Originals,
}

/// Recently downloaded originals by (instance, attachment), oldest first.
pub type Originals =
    Arc<std::sync::Mutex<std::collections::VecDeque<((InstanceId, Id), Arc<Vec<u8>>)>>>;

/// How many originals stay, and how many bytes at most.
const ORIGINALS_KEPT: usize = 6;
const ORIGINALS_BYTES: usize = 64 << 20;

/// Keeps `bytes` as the original of `key`, letting the oldest go past the limits.
pub fn keep_original(originals: &Originals, key: (InstanceId, Id), bytes: Arc<Vec<u8>>) {
    let mut kept = originals
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.retain(|(k, _)| *k != key);
    if bytes.len() > ORIGINALS_BYTES {
        return;
    }
    kept.push_back((key, bytes));
    while kept.len() > ORIGINALS_KEPT
        || kept.iter().map(|(_, b)| b.len()).sum::<usize>() > ORIGINALS_BYTES
    {
        kept.pop_front();
    }
}

/// The original of `key`, if it was downloaded lately.
pub fn kept_original(originals: &Originals, key: &(InstanceId, Id)) -> Option<Arc<Vec<u8>>> {
    originals
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, b)| b.clone())
}

/// Thumbnails kept in memory.
const MAX_THUMBNAILS: usize = 400;

impl Images {
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
            let same = |a: &Arc<RenderImage>, b: Option<&Arc<RenderImage>>| {
                b.is_some_and(|b| Arc::ptr_eq(a, b))
            };
            if !same(&old.still, shown.map(|s| &s.still)) {
                let _ = window.drop_image(old.still);
            }
            if let Some(moving) = old.moving
                && !same(&moving, shown.and_then(|s| s.moving.as_ref()))
            {
                let _ = window.drop_image(moving);
            }
        }
        self.viewer_drawn = shown.cloned();
    }

    fn retire(&mut self, picture: Img) {
        if let Img::Ready(picture) = picture {
            self.retired.push(picture.still);
            self.retired.extend(picture.moving);
        }
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
fn decode_preview(bytes: &[u8], max: Option<u32>) -> Result<Picture, String> {
    decode_within(bytes, max, PREVIEW_ANIMATION_BUDGET)
}

fn decode_within(bytes: &[u8], max: Option<u32>, budget: usize) -> Result<Picture, String> {
    // An SVG is drawn first (#5), then read like any picture.
    if looks_like_svg(bytes) {
        let side = max.map_or(SVG_SIDE, |m| m as f32);
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
        let still = Frame::from_parts(
            frames[0].buffer().clone(),
            0,
            0,
            image::Delay::from_numer_denom_ms(0, 1),
        );
        return Ok(Picture {
            still: Arc::new(RenderImage::new(vec![still])),
            moving: Some(Arc::new(RenderImage::new(frames))),
        });
    }
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
pub fn decode_first_frame(bytes: &[u8], max: u32) -> Result<Picture, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let mut pixels = image.to_rgba8();
    if pixels.width() > max || pixels.height() > max {
        pixels = picture::thumbnail(&pixels, max, max);
    }
    to_bgra(&mut pixels);
    Ok(Picture {
        still: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
        moving: None,
    })
}

/// All frames of an animated picture at most `side` pixels on a side, for a picture looked at
/// closely for a while (the crop view): the larger budget of the viewer.
pub fn decode_animation_sized(bytes: &[u8], side: u32) -> Result<Option<Arc<RenderImage>>, String> {
    let frames = frames_within(bytes, Some(side), ANIMATION_BUDGET)?;
    Ok(frames.map(|frames| Arc::new(RenderImage::new(frames))))
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

/// The frames of an animated GIF, WebP or PNG, at most `max` pixels on a side and within
/// `budget` bytes, in BGRA. `None`: one frame only, or too long to keep moving.
fn frames_within(
    bytes: &[u8],
    max: Option<u32>,
    budget: usize,
) -> Result<Option<Vec<Frame>>, String> {
    let err = |e: image::ImageError| e.to_string();
    let Some(frames) = animation_decoder(bytes)? else {
        return Ok(None);
    };
    let mut side = max.unwrap_or(u32::MAX);
    let mut out: Vec<Frame> = Vec::new();
    let mut bytes_used = 0usize;
    for frame in frames {
        let frame = frame.map_err(err)?;
        let delay = frame.delay();
        let mut pixels = fit(frame.into_buffer(), side);
        bytes_used += pixels.as_raw().len();
        // Over the budget: everything so far and everything after at half the size.
        while bytes_used > budget {
            side = pixels.width().max(pixels.height()) / 2;
            if side < ANIMATION_MIN_SIDE {
                return Ok(None);
            }
            for f in &mut out {
                let smaller = fit(f.buffer().clone(), side);
                *f = Frame::from_parts(smaller, 0, 0, f.delay());
            }
            pixels = fit(pixels, side);
            bytes_used = out.iter().map(|f| f.buffer().as_raw().len()).sum::<usize>()
                + pixels.as_raw().len();
        }
        to_bgra(&mut pixels);
        out.push(Frame::from_parts(pixels, 0, 0, delay));
    }
    Ok((out.len() > 1).then_some(out))
}

/// `pixels` made to fit `side` on its long side, keeping its shape.
fn fit(pixels: image::RgbaImage, side: u32) -> image::RgbaImage {
    picture::fit(pixels, side, image::imageops::FilterType::Triangle)
}

impl Shell {
    /// Starts loading the avatar of everyone the store knows whose avatar is not loaded (or
    /// loading) yet. A changed avatar is a new attachment, so each is fetched once.
    pub fn load_avatars(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(InstanceId, Id, String, u32)> = {
            let store = self.core.store();
            store
                .instances
                .iter()
                .flat_map(|(inst, s)| {
                    let users = || s.users.values().chain(s.me.as_ref().map(|me| &me.user));
                    users()
                        .filter_map(|u| u.avatar.as_ref())
                        // Server icons load the same way.
                        .chain(s.spaces.values().filter_map(|st| st.space.icon.as_ref()))
                        .map(|a| (a, AVATAR_PIXELS))
                        .chain(
                            users()
                                .filter_map(|u| u.banner.as_ref())
                                .map(|a| (a, BANNER_PIXELS)),
                        )
                        .map(move |(a, max)| (inst.clone(), a.id, a.filename.clone(), max))
                        .collect::<Vec<_>>()
                })
                .filter(|(inst, id, ..)| !self.images.avatars.contains_key(&(inst.clone(), *id)))
                .collect()
        };
        for (inst, id, filename, max) in wanted {
            let key = (inst, id);
            if self.images.avatars.contains_key(&key) {
                continue; // two users with the same avatar
            }
            self.images.avatars.insert(key.clone(), Img::Loading);
            let core = self.core.clone();
            let (inst, id) = key.clone();
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
                    this.images.avatars.insert(key, img);
                },
            );
        }
    }

    /// Someone's profile banner, once loaded.
    pub fn banner_picture(&self, inst: &str, user: Option<&User>) -> Option<Arc<RenderImage>> {
        let banner = user?.banner.as_ref()?;
        match self.images.avatars.get(&(inst.to_owned(), banner.id)) {
            Some(Img::Ready(picture)) => Some(picture.shown(self.animating())),
            _ => None,
        }
    }

    /// A server's icon, once loaded.
    pub fn space_picture(&self, inst: &str, space: Id) -> Option<Arc<RenderImage>> {
        let store = self.core.store();
        let icon = store
            .instance(inst)?
            .spaces
            .get(&space)?
            .space
            .icon
            .as_ref()?
            .id;
        match self.images.avatars.get(&(inst.to_owned(), icon)) {
            Some(Img::Ready(picture)) => Some(picture.shown(self.animating())),
            _ => None,
        }
    }

    /// Frees every thumbnail; those on screen load again.
    pub fn clear_pictures(&mut self) {
        let pictures: Vec<Img> = self.images.thumbs.drain().map(|(_, p)| p).collect();
        self.images.thumb_order.clear();
        for picture in pictures {
            self.images.retire(picture);
        }
    }

    /// Starts loading an attachment's thumbnail unless that was done already.
    pub fn load_thumbnail(
        &mut self,
        inst: &InstanceId,
        attachment: &Attachment,
        cx: &mut Context<Self>,
    ) {
        let id = attachment.id;
        let attachment = attachment.clone();
        let key = (inst.clone(), id);
        if self.images.thumbs.contains_key(&key) {
            return;
        }
        self.images.evict_thumbnails();
        self.images.thumb_order.push_back(key.clone());
        self.images.thumbs.insert(key.clone(), Img::Loading);
        let core = self.core.clone();
        let inst = inst.clone();
        let originals = self.images.originals.clone();
        self.run(
            cx,
            async move {
                // An encrypted picture may have a preview of its own; the server cannot make one.
                // The thumbnail of an animated one is still, so its original is loaded too.
                let animated = maybe_animated(&attachment.content_type, &attachment.filename)
                    || is_svg(&attachment.content_type, &attachment.filename);
                let no_preview = attachment
                    .sealed
                    .as_ref()
                    .is_some_and(|sealed| sealed.thumb.is_none());
                let (bytes, max) = if no_preview || animated {
                    let bytes = core.download(&inst, &attachment).await;
                    if let Ok(bytes) = &bytes {
                        keep_original(&originals, (inst.clone(), id), Arc::new(bytes.clone()));
                    }
                    let max = if animated {
                        ANIMATED_PREVIEW
                    } else {
                        SEALED_PREVIEW
                    };
                    (bytes?, Some(max))
                } else {
                    let bytes = core.thumbnail(&inst, &attachment).await;
                    (bytes?, None)
                };
                tokio::task::spawn_blocking(move || decode_preview(&bytes, max))
                    .await
                    .map_err(|e| e.to_string())?
            },
            move |this, result, _, _| {
                let img = match result {
                    Ok(image) => Img::Ready(image),
                    Err(err) => {
                        tracing::warn!("thumbnail {id}: {err}");
                        Img::Failed
                    }
                };
                if matches!(img, Img::Ready(_)) {
                    this.images
                        .thumb_arrived
                        .insert(key.clone(), std::time::Instant::now());
                }
                this.images.thumbs.insert(key, img);
            },
        );
    }

    /// A round avatar: the user's picture, else the first letter of their name.
    pub fn avatar(&self, inst: &str, user: Option<&User>, name: &str, size: f32) -> AnyElement {
        let picture = user
            .and_then(|u| u.avatar.as_ref())
            .and_then(|a| self.images.avatars.get(&(inst.to_owned(), a.id)));
        if let Some(Img::Ready(picture)) = picture {
            return img(picture.shown(self.animating()))
                .flex_shrink_0()
                .size(px(size))
                .rounded(px(size / 2.))
                .object_fit(gpui_kit::ObjectFit::Cover)
                // An id lets gpui step through an animation's frames.
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
            .rounded(px(size / 2.))
            .bg(theme::selected())
            .text_color(theme::text())
            .text_size(px((size * 0.45).max(12.)))
            .child(initial(name))
            .into_any_element()
    }

    /// An avatar with the presence dot on its corner, on the sidebar's colour.
    pub fn avatar_with_status(
        &self,
        inst: &str,
        user: Option<&User>,
        name: &str,
        size: f32,
        status: Status,
    ) -> AnyElement {
        self.avatar_with_status_on(inst, user, name, size, status, theme::sidebar())
    }

    /// The same on another surface: `ground` is the colour around the avatar, which the dot's
    /// ring and cut-outs take so they read as holes. A new status pops the dot.
    pub fn avatar_with_status_on(
        &self,
        inst: &str,
        user: Option<&User>,
        name: &str,
        size: f32,
        status: Status,
        ground: gpui_kit::Rgba,
    ) -> AnyElement {
        use gpui_kit::{AnimationExt as _, SpringAnimation, SpringConfig};
        let dot = (size / 3.).max(10.);
        let who = user.map_or_else(|| name.to_owned(), |u| u.id.to_string());
        div()
            .relative()
            .flex_shrink_0()
            .size(px(size))
            .child(self.avatar(inst, user, name, size))
            .child(
                // A box of the dot's size, so the dot grows from its middle.
                div()
                    .absolute()
                    .bottom(px(-1.))
                    .right(px(-1.))
                    .size(px(dot))
                    .flex()
                    .items_center()
                    .justify_center()
                    .with_spring(
                        SharedString::from(format!("presence-{inst}-{who}-{status:?}")),
                        SpringAnimation::new(crate::theme::paced(SpringConfig::new(520., 22., 1.)))
                            .to(true)
                            .from(false),
                        move |d, phase| {
                            let grown = dot * (0.55 + 0.45 * phase.0.max(0.));
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .size(px(grown))
                                    .rounded(px(grown / 2.))
                                    .border_2()
                                    .border_color(ground)
                                    .bg(theme::status(status))
                                    .overflow_hidden()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .children(presence_shape(status, grown, ground)),
                            )
                        },
                    ),
            )
            .into_any_element()
    }
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

fn initial(name: &str) -> SharedString {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default()
        .into()
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
        let kept = Originals::default();
        for n in 0..8u64 {
            keep_original(&kept, ("h".into(), Id(n)), Arc::new(vec![0; 10]));
        }
        assert!(kept_original(&kept, &("h".into(), Id(0))).is_none(), "the oldest went");
        assert!(kept_original(&kept, &("h".into(), Id(7))).is_some());
        keep_original(&kept, ("h".into(), Id(9)), Arc::new(vec![0; ORIGINALS_BYTES + 1]));
        assert!(kept_original(&kept, &("h".into(), Id(9))).is_none(), "too large to keep");
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
    fn an_animated_gif_keeps_its_frames_and_a_still_first_one() {
        let picture = decode(&gif(3, 16), None).unwrap();
        assert_eq!(picture.moving.as_ref().unwrap().frame_count(), 3);
        assert_eq!(picture.still.frame_count(), 1);
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
    fn gifs_and_webps_may_be_animated() {
        assert!(maybe_animated("image/gif", "a"));
        assert!(maybe_animated("application/octet-stream", "Party.GIF"));
        assert!(!maybe_animated("image/png", "a.png"));
    }
}
