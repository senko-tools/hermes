//! Avatars and attachment images, loaded from the server once and decoded off the UI thread.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, RenderImage, SharedString, div, img};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;
use hermes_client_core::proto::models::{Status, User};
use image::Frame;

use crate::shell::Shell;
use crate::theme::{self, px};

/// The largest size an avatar is drawn at, in pixels on a 2x display.
const AVATAR_PIXELS: u32 = 192;

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
/// Below this many pixels on its long side an animation stays still instead.
const ANIMATION_MIN_SIDE: u32 = 48;

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
    pub avatars: HashMap<(InstanceId, Id), Img>,
}

/// Decodes image bytes for drawing. gpui wants BGRA.
/// The size encrypted pictures are made into previews at, here on the device.
const SEALED_PREVIEW: u32 = 800;

pub fn decode(bytes: &[u8], max: Option<u32>) -> Result<Picture, String> {
    let moving = match animation_frames(bytes, max) {
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
    let image = match max {
        Some(max) if image.width() > max || image.height() > max => {
            image.resize(max, max, image::imageops::FilterType::Lanczos3)
        }
        _ => image,
    };
    let mut pixels = image.to_rgba8();
    to_bgra(&mut pixels);
    Ok(Picture {
        still: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
        moving: None,
    })
}

/// gpui wants BGRA.
fn to_bgra(pixels: &mut image::RgbaImage) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

/// The frames of an animated GIF, WebP or PNG, at most `max` pixels on a side and within
/// [`ANIMATION_BUDGET`], in BGRA. `None`: one frame only, or too long to keep moving.
pub fn animation_frames(bytes: &[u8], max: Option<u32>) -> Result<Option<Vec<Frame>>, String> {
    frames_within(bytes, max, ANIMATION_BUDGET)
}

fn frames_within(
    bytes: &[u8],
    max: Option<u32>,
    budget: usize,
) -> Result<Option<Vec<Frame>>, String> {
    use image::AnimationDecoder as _;
    use std::io::Cursor;
    let err = |e: image::ImageError| e.to_string();
    let frames: image::Frames<'_> = match image::guess_format(bytes).map_err(err)? {
        image::ImageFormat::Gif => image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .map_err(err)?
            .into_frames(),
        image::ImageFormat::WebP => {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).map_err(err)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            decoder.into_frames()
        }
        image::ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).map_err(err)?;
            if !decoder.is_apng().map_err(err)? {
                return Ok(None);
            }
            decoder.apng().map_err(err)?.into_frames()
        }
        _ => return Ok(None),
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
    let (w, h) = pixels.dimensions();
    if w <= side && h <= side {
        return pixels;
    }
    let scale = side as f32 / w.max(h) as f32;
    let (nw, nh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    image::imageops::resize(&pixels, nw, nh, image::imageops::FilterType::Triangle)
}

impl Shell {
    /// Starts loading the avatar of everyone the store knows whose avatar is not loaded (or
    /// loading) yet. A changed avatar is a new attachment, so each is fetched once.
    pub fn load_avatars(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(InstanceId, Id, String)> = {
            let store = self.core.store();
            store
                .instances
                .iter()
                .flat_map(|(inst, s)| {
                    s.users
                        .values()
                        .chain(s.me.as_ref().map(|me| &me.user))
                        .filter_map(|u| u.avatar.as_ref())
                        // Server icons load the same way.
                        .chain(s.spaces.values().filter_map(|st| st.space.icon.as_ref()))
                        .map(move |a| (inst.clone(), a.id, a.filename.clone()))
                })
                .filter(|(inst, id, _)| !self.images.avatars.contains_key(&(inst.clone(), *id)))
                .collect()
        };
        for (inst, id, filename) in wanted {
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
                    let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                    // The thumbnail of an animated picture is still: its original moves.
                    let bytes = if maybe_animated("", &filename) {
                        rest.download(id, &filename).await
                    } else {
                        rest.thumbnail(id).await
                    }
                    .map_err(|e| e.to_string())?;
                    tokio::task::spawn_blocking(move || decode(&bytes, Some(AVATAR_PIXELS)))
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
            Some(Img::Ready(picture)) => Some(picture.shown(self.focused)),
            _ => None,
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
        self.images.thumbs.insert(key.clone(), Img::Loading);
        let core = self.core.clone();
        let inst = inst.clone();
        self.run(
            cx,
            async move {
                // An encrypted picture has no thumbnail on the server: the server cannot see it.
                // The thumbnail of an animated one is still, so its original is loaded too.
                let animated = maybe_animated(&attachment.content_type, &attachment.filename);
                let (bytes, max) = if attachment.sealed.is_some() || animated {
                    let bytes = core.download(&inst, &attachment).await;
                    (bytes.map_err(|e| e.to_string())?, Some(SEALED_PREVIEW))
                } else {
                    let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                    (rest.thumbnail(id).await.map_err(|e| e.to_string())?, None)
                };
                tokio::task::spawn_blocking(move || decode(&bytes, max))
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
            return img(picture.shown(self.focused))
                .flex_shrink_0()
                .size(px(size))
                .rounded(px(size / 2.))
                .object_fit(gpui_kit::ObjectFit::Cover)
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

    /// An avatar with the presence dot on its corner.
    pub fn avatar_with_status(
        &self,
        inst: &str,
        user: Option<&User>,
        name: &str,
        size: f32,
        status: Status,
    ) -> AnyElement {
        let dot = (size / 3.).max(10.);
        div()
            .relative()
            .flex_shrink_0()
            .size(px(size))
            .child(self.avatar(inst, user, name, size))
            .child(
                div()
                    .absolute()
                    .bottom(px(-1.))
                    .right(px(-1.))
                    .size(px(dot))
                    .rounded(px(dot / 2.))
                    .border_2()
                    .border_color(theme::sidebar())
                    .bg(theme::status(status)),
            )
            .into_any_element()
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
    fn gifs_and_webps_may_be_animated() {
        assert!(maybe_animated("image/gif", "a"));
        assert!(maybe_animated("application/octet-stream", "Party.GIF"));
        assert!(!maybe_animated("image/png", "a.png"));
    }
}
