//! Avatars and attachment images, loaded from the server once and decoded off the UI thread.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, RenderImage, SharedString, Window, div, img};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;
use hermes_client_core::proto::models::{Status, User};
use image::Frame;

use crate::shell::Shell;
use crate::theme::{self, px};

/// The largest size an avatar is drawn at, in pixels on a 2x display.
const AVATAR_PIXELS: u32 = 192;
/// The size encrypted pictures are made into previews at, here on the device.
const SEALED_PREVIEW: u32 = 800;
/// Decoded thumbnails kept, in bytes: a few hundred of the server's (400 px), so scrolling back
/// rarely loads one again, while a long session in picture-heavy channels stays bounded.
const THUMB_BUDGET: usize = 128 << 20;

#[derive(Clone)]
pub enum Img {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

struct Entry {
    img: Img,
    /// Decoded size (0 unless ready).
    bytes: usize,
    /// The frame it was last drawn in (or stored). Drawing only has `&self`, hence the cell.
    used: Cell<u64>,
}

/// Images by instance and attachment, optionally bounded: past its budget the least recently
/// drawn pictures are dropped, and load again the next time they are shown.
pub struct ImageCache {
    /// By instance first, so a lookup needs no owned key.
    entries: HashMap<InstanceId, HashMap<Id, Entry>>,
    bytes: usize,
    budget: Option<usize>,
    /// Counts the window's draws. Whatever the last one drew is on screen: gpui draws every
    /// visible picture each time, and nothing while the window is idle.
    frame: u64,
    /// The frame in which dropping found nothing to drop: no use trying again before the next.
    stuck: Option<u64>,
    /// Dropped pictures, still to be freed from the GPU.
    retired: Vec<Arc<RenderImage>>,
}

impl ImageCache {
    /// Keeps everything.
    pub fn unbounded() -> ImageCache {
        ImageCache {
            entries: HashMap::new(),
            bytes: 0,
            budget: None,
            frame: 0,
            stuck: None,
            retired: Vec::new(),
        }
    }

    /// Keeps about `budget` bytes of decoded pictures.
    pub fn bounded(budget: usize) -> ImageCache {
        ImageCache {
            budget: Some(budget),
            ..ImageCache::unbounded()
        }
    }

    /// The image of attachment `id` on `inst`; counts as drawing it.
    pub fn get(&self, inst: &str, id: Id) -> Option<&Img> {
        let entry = self.entries.get(inst)?.get(&id)?;
        entry.used.set(self.frame);
        Some(&entry.img)
    }

    /// Whether the image is loaded or loading.
    pub fn contains(&self, inst: &str, id: Id) -> bool {
        self.entries.get(inst).is_some_and(|m| m.contains_key(&id))
    }

    /// A new frame is being drawn.
    fn next_frame(&mut self) {
        self.frame += 1;
    }

    pub fn insert(&mut self, inst: &str, id: Id, img: Img) {
        let bytes = match &img {
            Img::Ready(image) => image_bytes(image),
            _ => 0,
        };
        let entry = Entry {
            img,
            bytes,
            used: Cell::new(self.frame),
        };
        let map = match self.entries.get_mut(inst) {
            Some(map) => map,
            None => self.entries.entry(inst.to_owned()).or_default(),
        };
        self.bytes += bytes;
        if let Some(old) = map.insert(id, entry) {
            self.bytes -= old.bytes;
            if let Img::Ready(image) = old.img {
                self.retired.push(image);
            }
        }
        if self.budget.is_some_and(|budget| self.bytes > budget) && self.stuck != Some(self.frame) {
            self.evict();
        }
    }

    /// Drops the least recently drawn pictures until a little under the budget, never one that
    /// the last two frames drew (on screen). Loading and failed entries cost nothing and stay.
    fn evict(&mut self) {
        let Some(budget) = self.budget else {
            return;
        };
        let target = budget - budget / 8;
        let recent = self.frame.saturating_sub(1);
        let mut old: Vec<(u64, &InstanceId, Id, usize)> = self
            .entries
            .iter()
            .flat_map(|(inst, m)| m.iter().map(move |(id, e)| (inst, *id, e)))
            .filter(|(.., e)| e.bytes > 0 && e.used.get() < recent)
            .map(|(inst, id, e)| (e.used.get(), inst, id, e.bytes))
            .collect();
        if old.is_empty() {
            self.stuck = Some(self.frame);
            return;
        }
        old.sort_unstable_by_key(|(used, ..)| *used);
        let mut left = self.bytes;
        let mut gone = Vec::new();
        for (_, inst, id, bytes) in old {
            if left <= target {
                break;
            }
            left -= bytes;
            gone.push((inst.clone(), id));
        }
        for (inst, id) in gone {
            let Some(map) = self.entries.get_mut(&inst) else {
                continue;
            };
            if let Some(entry) = map.remove(&id) {
                self.bytes -= entry.bytes;
                if let Img::Ready(image) = entry.img {
                    self.retired.push(image);
                }
            }
            if map.is_empty() {
                self.entries.remove(&inst);
            }
        }
    }
}

/// The decoded size of a picture.
fn image_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .map(|frame| image.as_bytes(frame).map_or(0, <[u8]>::len))
        .sum()
}

pub struct Images {
    /// Attachment thumbnails: bounded, since every picture scrolled past is one more.
    pub thumbs: ImageCache,
    /// Avatars: one per person the store knows, each fetched only once, so they all stay.
    pub avatars: ImageCache,
    /// The viewer's picture as last drawn, freed from the GPU once the viewer lets go of it.
    viewer_drawn: Option<Arc<RenderImage>>,
}

impl Default for Images {
    fn default() -> Self {
        Images {
            thumbs: ImageCache::bounded(THUMB_BUDGET),
            avatars: ImageCache::unbounded(),
            viewer_drawn: None,
        }
    }
}

impl Images {
    /// At the start of each draw: frees the GPU copies of pictures nothing draws any more
    /// (dropped or replaced ones, and the full-size picture of a closed viewer; gpui keeps every
    /// drawn image until told), and starts a new frame.
    pub fn new_frame(&mut self, viewer: Option<&Img>, window: &mut Window) {
        self.thumbs.next_frame();
        self.avatars.next_frame();
        let retired = self.thumbs.retired.drain(..);
        for image in retired.chain(self.avatars.retired.drain(..)) {
            let _ = window.drop_image(image);
        }
        let shown = match viewer {
            Some(Img::Ready(image)) => Some(image),
            _ => None,
        };
        if let Some(old) = self
            .viewer_drawn
            .take_if(|old| shown.is_none_or(|shown| !Arc::ptr_eq(old, shown)))
        {
            let _ = window.drop_image(old);
        }
        self.viewer_drawn = shown.cloned();
    }
}

/// Decodes image bytes for drawing. gpui wants BGRA.
pub fn decode(bytes: &[u8], max: Option<u32>) -> Result<Arc<RenderImage>, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let image = match max {
        Some(max) if image.width() > max || image.height() > max => {
            image.resize(max, max, image::imageops::FilterType::Lanczos3)
        }
        _ => image,
    };
    let mut pixels = image.to_rgba8();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![Frame::new(pixels)])))
}

impl Shell {
    /// Starts loading the avatar of everyone the store knows whose avatar is not loaded (or
    /// loading) yet. A changed avatar is a new attachment, so each is fetched once.
    pub fn load_avatars(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(InstanceId, Id)> = {
            let store = self.core.store();
            let avatars = &self.images.avatars;
            store
                .instances
                .iter()
                .flat_map(|(inst, s)| {
                    s.users
                        .values()
                        .chain(s.me.as_ref().map(|me| &me.user))
                        .filter_map(|u| u.avatar.as_ref())
                        .filter(|a| !avatars.contains(inst, a.id))
                        .map(move |a| (inst.clone(), a.id))
                })
                .collect()
        };
        for key in wanted {
            if self.images.avatars.contains(&key.0, key.1) {
                continue; // two users with the same avatar
            }
            self.images.avatars.insert(&key.0, key.1, Img::Loading);
            let core = self.core.clone();
            let (inst, id) = key.clone();
            self.run(
                cx,
                async move {
                    let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                    let bytes = rest.thumbnail(id).await.map_err(|e| e.to_string())?;
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
                    this.images.avatars.insert(&key.0, key.1, img);
                },
            );
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
        if self.images.thumbs.contains(inst, id) {
            return;
        }
        self.images.thumbs.insert(inst, id, Img::Loading);
        let core = self.core.clone();
        let (inst, done_inst) = (inst.clone(), inst.clone());
        self.run(
            cx,
            async move {
                // An encrypted picture has no thumbnail on the server: the server cannot see it.
                let (bytes, max) = if attachment.sealed.is_some() {
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
                this.images.thumbs.insert(&done_inst, id, img);
            },
        );
    }

    /// A round avatar: the user's picture, else the first letter of their name.
    pub fn avatar(&self, inst: &str, user: Option<&User>, name: &str, size: f32) -> AnyElement {
        let picture = user
            .and_then(|u| u.avatar.as_ref())
            .and_then(|a| self.images.avatars.get(inst, a.id));
        if let Some(Img::Ready(image)) = picture {
            return img(image.clone())
                .flex_shrink_0()
                .size(px(size))
                .rounded(px(size / 2.))
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
mod tests {
    use super::*;

    /// A ready picture of `kib` KiB.
    fn picture(kib: u32) -> Img {
        let pixels = image::RgbaImage::new(16, kib * 16);
        Img::Ready(Arc::new(RenderImage::new(vec![Frame::new(pixels)])))
    }

    #[test]
    fn the_least_recently_drawn_pictures_go_first() {
        // Room for 10 KiB, trimmed to 8.75 KiB when over.
        let mut cache = ImageCache::bounded(10 << 10);
        for id in 0..10 {
            cache.insert("h", Id(id), picture(1));
            cache.next_frame();
        }
        cache.insert("h", Id(100), Img::Loading);
        cache.insert("h", Id(101), Img::Failed);
        assert_eq!(cache.bytes, 10 << 10);
        assert!(cache.retired.is_empty());

        // Picture 1 is drawn again, then one more comes.
        assert!(cache.get("h", Id(1)).is_some());
        for _ in 0..5 {
            cache.next_frame();
        }
        cache.insert("other", Id(1), picture(1));
        assert_eq!(cache.bytes, 8 << 10);
        assert!(cache.contains("h", Id(1)), "drawn recently");
        assert!(
            [0, 2, 3].iter().all(|id| !cache.contains("h", Id(*id))),
            "the least recently drawn"
        );
        assert!(cache.contains("h", Id(4)));
        assert!(cache.contains("h", Id(100)) && cache.contains("h", Id(101)));
        assert!(cache.get("other", Id(1)).is_some());
        assert_eq!(cache.retired.len(), 3, "to be freed from the GPU");
    }

    #[test]
    fn pictures_on_screen_stay_even_over_budget() {
        let mut cache = ImageCache::bounded(2 << 10);
        for id in 0..4 {
            cache.insert("h", Id(id), picture(1));
        }
        // All were stored (or drawn) in this frame, so they are on screen however long the
        // window stays idle.
        assert_eq!(cache.bytes, 4 << 10);
        assert_eq!(cache.stuck, Some(0));

        // Two frames later only picture 0 is still drawn; replacing it frees its old copy, and
        // the others are trimmed.
        cache.next_frame();
        cache.next_frame();
        assert!(cache.get("h", Id(0)).is_some());
        cache.insert("h", Id(0), picture(2));
        assert_eq!(cache.retired.len(), 4);
        assert_eq!(cache.bytes, 2 << 10);
        assert!(cache.contains("h", Id(0)));

        let mut avatars = ImageCache::unbounded();
        for id in 0..100 {
            avatars.insert("h", Id(id), picture(1));
        }
        assert_eq!(avatars.bytes, 100 << 10);
    }
}
