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
    Ready(Arc<RenderImage>),
    Failed,
}

#[derive(Default)]
pub struct Images {
    pub thumbs: HashMap<(InstanceId, Id), Img>,
    pub avatars: HashMap<(InstanceId, Id), Img>,
}

/// Decodes image bytes for drawing. gpui wants BGRA.
/// The size encrypted pictures are made into previews at, here on the device.
const SEALED_PREVIEW: u32 = 800;

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
            store
                .instances
                .iter()
                .flat_map(|(inst, s)| {
                    s.users
                        .values()
                        .chain(s.me.as_ref().map(|me| &me.user))
                        .filter_map(|u| u.avatar.as_ref())
                        .map(move |a| (inst.clone(), a.id))
                })
                .filter(|key| !self.images.avatars.contains_key(key))
                .collect()
        };
        for key in wanted {
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
                    this.images.avatars.insert(key, img);
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
                this.images.thumbs.insert(key, img);
            },
        );
    }

    /// A round avatar: the user's picture, else the first letter of their name.
    pub fn avatar(&self, inst: &str, user: Option<&User>, name: &str, size: f32) -> AnyElement {
        let picture = user
            .and_then(|u| u.avatar.as_ref())
            .and_then(|a| self.images.avatars.get(&(inst.to_owned(), a.id)));
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
