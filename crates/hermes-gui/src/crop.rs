//! The crop view shown after picking an avatar, a banner or a server icon: the picture under a fixed
//! frame, dragged to move and scrolled (or slid) to zoom. Saving crops it and uploads that: a
//! PNG, or a GIF of every frame cropped alike when the picture is animated.

use std::sync::Arc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, MouseButton, ObjectFit, Pixels, Point, Subscription, Window, div,
    img,
};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::api::{PICTURE_MAX_BYTES, UpdateMeRequest, UpdateSpaceRequest};
use hermes_client_core::proto::id::Id;

use crate::images::Picture;
use crate::modal::{dialog, heading, paragraph};
use crate::shell::Shell;
use crate::theme::{self, px};

/// The room around the frame where the rest of the picture shows dimmed.
const MARGIN: f32 = 40.;
/// How far one can zoom in.
const MAX_ZOOM: f32 = 5.;

/// What the cropped picture becomes.
#[derive(Clone)]
pub enum CropFor {
    Avatar(InstanceId),
    SpaceIcon(InstanceId, Id),
    /// My profile banner, 5:2.
    Banner(InstanceId),
}

/// The frame a picture is cropped to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    /// The frame's size on screen, in layout units.
    width: f32,
    height: f32,
    /// The corner radius on screen.
    radius: f32,
    /// The largest uploaded size, still and animated (a GIF of many frames grows fast).
    still: (u32, u32),
    moving: (u32, u32),
}

const SQUARE: f32 = 240.;
/// The largest side of the picture shown while cropping.
const PREVIEW_SIDE: u32 = 1024;

/// Why a picture was refused, in words.
fn too_big() -> String {
    format!("it is larger than {} MB", PICTURE_MAX_BYTES / (1024 * 1024))
}

impl CropFor {
    fn shape(&self) -> Shape {
        match self {
            CropFor::Avatar(_) => Shape {
                width: SQUARE,
                height: SQUARE,
                radius: SQUARE / 2.,
                still: (512, 512),
                moving: (256, 256),
            },
            CropFor::SpaceIcon(..) => Shape {
                width: SQUARE,
                height: SQUARE,
                radius: SQUARE * 14. / theme::RAIL_ICON,
                still: (512, 512),
                moving: (256, 256),
            },
            CropFor::Banner(_) => Shape {
                width: 400.,
                height: 160.,
                radius: 12.,
                still: (1000, 400),
                moving: (600, 240),
            },
        }
    }

    fn inst(&self) -> &InstanceId {
        match self {
            CropFor::Avatar(inst) | CropFor::SpaceIcon(inst, _) | CropFor::Banner(inst) => inst,
        }
    }
}

pub struct Crop {
    pub purpose: CropFor,
    /// The file as picked, its size in pixels, and its preview.
    bytes: Arc<Vec<u8>>,
    size: (u32, u32),
    preview: Picture,
    /// 1 = the picture just covers the frame.
    zoom: f32,
    /// How far the picture's centre is from the frame's, in layout units.
    offset: (f32, f32),
    /// A drag: where the pointer went down, and the offset then.
    drag: Option<(Point<Pixels>, (f32, f32))>,
    saving: bool,
    slider: Entity<SliderState>,
    _slider: Subscription,
}

/// The picture's size in the frame at `zoom`, in layout units: at 1 it just covers the frame.
fn shown_size(shape: Shape, (width, height): (u32, u32), zoom: f32) -> (f32, f32) {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (shape.width / w).max(shape.height / h) * zoom;
    (w * scale, h * scale)
}

/// Keeps the frame covered: the picture cannot move further than its overhang.
fn clamp_offset(shape: Shape, shown: (f32, f32), offset: (f32, f32)) -> (f32, f32) {
    let room_x = ((shown.0 - shape.width) / 2.).max(0.);
    let room_y = ((shown.1 - shape.height) / 2.).max(0.);
    (
        offset.0.clamp(-room_x, room_x),
        offset.1.clamp(-room_y, room_y),
    )
}

/// The part of the source picture under the frame: `(x, y, width, height)` in source pixels.
fn crop_rect(
    shape: Shape,
    (width, height): (u32, u32),
    zoom: f32,
    offset: (f32, f32),
) -> (u32, u32, u32, u32) {
    let shown = shown_size(shape, (width, height), zoom);
    let scale = shown.0 / width.max(1) as f32;
    let (ox, oy) = clamp_offset(shape, shown, offset);
    let left = (shown.0 - shape.width) / 2. - ox;
    let top = (shown.1 - shape.height) / 2. - oy;
    let w = ((shape.width / scale).round() as u32).clamp(1, width.max(1));
    let h = ((shape.height / scale).round() as u32).clamp(1, height.max(1));
    let x = ((left / scale).round().max(0.) as u32).min(width.saturating_sub(w));
    let y = ((top / scale).round().max(0.) as u32).min(height.saturating_sub(h));
    (x, y, w, h)
}

/// The cropped size: the crop itself, made smaller to fit `max`.
fn output_size((w, h): (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let scale = (max.0 as f32 / w as f32)
        .min(max.1 as f32 / h as f32)
        .min(1.);
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

/// What saving uploads: the file, its name and its type.
struct Output {
    bytes: Vec<u8>,
    filename: &'static str,
    content_type: &'static str,
}

/// Crops the picture: every frame of an animated one into a GIF, else a PNG.
fn crop_file(shape: Shape, file: &[u8], zoom: f32, offset: (f32, f32)) -> Result<Output, String> {
    let err = |e: image::ImageError| e.to_string();
    let first = image::load_from_memory(file).map_err(err)?;
    let (x, y, w, h) = crop_rect(shape, (first.width(), first.height()), zoom, offset);
    if let Some(frames) = animated_frames(file)? {
        let (ow, oh) = output_size((w, h), shape.moving);
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut bytes, 10);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .map_err(err)?;
            for frame in frames {
                let frame = frame.map_err(err)?;
                let delay = frame.delay();
                let cropped = image::imageops::crop_imm(frame.buffer(), x, y, w, h).to_image();
                let scaled = image::imageops::resize(
                    &cropped,
                    ow,
                    oh,
                    image::imageops::FilterType::Triangle,
                );
                encoder
                    .encode_frame(image::Frame::from_parts(scaled, 0, 0, delay))
                    .map_err(err)?;
            }
        }
        return Ok(Output {
            bytes,
            filename: "picture.gif",
            content_type: "image/gif",
        });
    }
    let (ow, oh) = output_size((w, h), shape.still);
    let cropped =
        first
            .crop_imm(x, y, w, h)
            .resize_exact(ow, oh, image::imageops::FilterType::Lanczos3);
    let mut bytes = Vec::new();
    cropped
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(err)?;
    Ok(Output {
        bytes,
        filename: "picture.png",
        content_type: "image/png",
    })
}

/// The frames of an animated GIF, WebP or PNG, as they come; `None` for a still picture.
fn animated_frames(file: &[u8]) -> Result<Option<image::Frames<'_>>, String> {
    // Only something with more than one frame is cropped as an animation; two tell.
    let Ok(Some(frames)) = crate::images::animation_decoder(file) else {
        return Ok(None);
    };
    if frames.take(2).filter(Result::is_ok).count() < 2 {
        return Ok(None);
    }
    crate::images::animation_decoder(file)
}

impl Shell {
    /// Asks for a picture and opens the crop view on it.
    pub fn pick_picture(&mut self, purpose: CropFor, cx: &mut Context<Self>) {
        let title = match purpose {
            CropFor::Avatar(_) => "Choose a picture",
            CropFor::SpaceIcon(..) => "Choose a server icon",
            CropFor::Banner(_) => "Choose a banner",
        };
        self.run(
            cx,
            async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .set_title(title)
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "apng"])
                    .pick_file()
                    .await
                else {
                    return Ok(None);
                };
                let len = tokio::fs::metadata(file.path())
                    .await
                    .map_err(|e| e.to_string())?
                    .len();
                if len > PICTURE_MAX_BYTES {
                    return Err(too_big());
                }
                let bytes = tokio::fs::read(file.path())
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::task::spawn_blocking(move || {
                    let size = image::ImageReader::new(std::io::Cursor::new(&bytes))
                        .with_guessed_format()
                        .map_err(|e| e.to_string())?
                        .into_dimensions()
                        .map_err(|e| e.to_string())?;
                    let preview = crate::images::decode_first_frame(&bytes, PREVIEW_SIDE)?;
                    Ok(Some((Arc::new(bytes), size, preview)))
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |this, result: Result<_, String>, window, cx| match result {
                Ok(Some((bytes, size, preview))) => {
                    this.load_crop_animation(bytes.clone(), cx);
                    this.open_crop(purpose, bytes, size, preview, window, cx);
                }
                Ok(None) => {}
                Err(err) => this.error(format!("could not open the picture: {err}"), cx),
            },
        );
    }

    /// The crop view opens on the first frame; an animation joins it once decoded, if that
    /// picture is still being cropped.
    fn load_crop_animation(&self, bytes: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        let picked = bytes.clone();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || crate::images::decode_animation(&bytes))
                    .await
                    .map_err(|e| e.to_string())?
            },
            move |this, result: Result<_, String>, _, _| {
                if let Ok(Some(moving)) = result
                    && let Some(crop) = &mut this.crop
                    && Arc::ptr_eq(&crop.bytes, &picked)
                {
                    crop.preview.moving = Some(moving);
                }
            },
        );
    }

    fn open_crop(
        &mut self,
        purpose: CropFor,
        bytes: Arc<Vec<u8>>,
        size: (u32, u32),
        preview: Picture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(MAX_ZOOM)
                .step(0.01)
                .default_value(1.)
        });
        let subscription =
            cx.subscribe_in(&slider, window, |this, _, event: &SliderEvent, _, cx| {
                let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
                if let Some(crop) = &mut this.crop {
                    crop.set_zoom(value.end());
                }
                cx.notify();
            });
        self.crop = Some(Crop {
            purpose,
            bytes,
            size,
            preview,
            zoom: 1.,
            offset: (0., 0.),
            drag: None,
            saving: false,
            slider,
            _slider: subscription,
        });
        cx.notify();
    }

    fn save_crop(&mut self, cx: &mut Context<Self>) {
        let Some(crop) = &mut self.crop else {
            return;
        };
        crop.saving = true;
        let (file, zoom, offset) = (crop.bytes.clone(), crop.zoom, crop.offset);
        let purpose = crop.purpose.clone();
        let shape = purpose.shape();
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let output =
                    tokio::task::spawn_blocking(move || crop_file(shape, &file, zoom, offset))
                        .await
                        .map_err(|e| e.to_string())??;
                // A long animation can grow when cropped.
                if output.bytes.len() as u64 > PICTURE_MAX_BYTES {
                    return Err(too_big());
                }
                let inst = purpose.inst().clone();
                fn failed(e: impl std::fmt::Display) -> String {
                    e.to_string()
                }
                let rest = core.rest(&inst).map_err(failed)?;
                let picture = rest
                    .upload(output.filename, output.content_type, output.bytes)
                    .await
                    .map_err(failed)?;
                match purpose {
                    CropFor::Avatar(_) => {
                        rest.update_me(&UpdateMeRequest {
                            avatar_id: Some(Some(picture.id)),
                            ..UpdateMeRequest::default()
                        })
                        .await
                        .map_err(failed)?;
                        Ok::<_, String>("Avatar changed")
                    }
                    CropFor::SpaceIcon(_, space) => {
                        rest.update_space(
                            space,
                            &UpdateSpaceRequest {
                                icon_id: Some(Some(picture.id)),
                                ..UpdateSpaceRequest::default()
                            },
                        )
                        .await
                        .map_err(failed)?;
                        Ok("Icon changed")
                    }
                    CropFor::Banner(_) => {
                        let me = rest
                            .update_me(&UpdateMeRequest {
                                banner_id: Some(Some(picture.id)),
                                ..UpdateMeRequest::default()
                            })
                            .await
                            .map_err(failed)?;
                        // A server from before banners ignores the field and still says yes.
                        if me.user.banner.as_ref().map(|b| b.id) != Some(picture.id) {
                            return Err("this server is too old for banners".to_owned());
                        }
                        Ok("Banner changed")
                    }
                }
            },
            |this, result, _, cx| match result {
                Ok(text) => {
                    this.crop = None;
                    this.info(text, cx);
                }
                Err(err) => {
                    if let Some(crop) = &mut this.crop {
                        crop.saving = false;
                    }
                    this.error(format!("could not save the picture: {err}"), cx);
                }
            },
        );
    }

    pub fn render_crop(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let crop = self.crop.as_ref()?;
        let shape = crop.purpose.shape();
        let shown = shown_size(shape, crop.size, crop.zoom);
        let (ox, oy) = crop.offset;
        let (area_w, area_h) = (shape.width + 2. * MARGIN, shape.height + 2. * MARGIN);
        // A ring as thick as the area dims everything outside the frame; its inner edge is the
        // frame's shape.
        let ring = area_w.max(area_h);
        let area = div()
            .id("crop-area")
            .relative()
            .flex_shrink_0()
            .w(px(area_w))
            .h(px(area_h))
            .overflow_hidden()
            .rounded(px(8.))
            .bg(theme::sidebar())
            .cursor_grab()
            .child(
                img(crop.preview.shown(true))
                    .absolute()
                    .left(px((area_w - shown.0) / 2. + ox))
                    .top(px((area_h - shown.1) / 2. + oy))
                    .w(px(shown.0))
                    .h(px(shown.1))
                    .object_fit(ObjectFit::Fill)
                    .id("crop-picture"),
            )
            .child(
                div()
                    .absolute()
                    .left(px(MARGIN - ring))
                    .top(px(MARGIN - ring))
                    .w(px(shape.width + 2. * ring))
                    .h(px(shape.height + 2. * ring))
                    .rounded(px(shape.radius + ring))
                    .border(px(ring))
                    .border_color(theme::scrim()),
            )
            .child(
                div()
                    .absolute()
                    .left(px(MARGIN))
                    .top(px(MARGIN))
                    .w(px(shape.width))
                    .h(px(shape.height))
                    .rounded(px(shape.radius))
                    .border_2()
                    .border_color(theme::accent()),
            )
            // Drags start on press, like every drag.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    if let Some(crop) = &mut this.crop {
                        crop.drag = Some((event.position, crop.offset));
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    if let Some(crop) = &mut this.crop
                        && let Some((start, offset)) = crop.drag
                    {
                        if event.pressed_button != Some(MouseButton::Left) {
                            crop.drag = None;
                            return;
                        }
                        let scale = theme::scale();
                        let moved = (
                            (event.position.x - start.x).as_f32() / scale,
                            (event.position.y - start.y).as_f32() / scale,
                        );
                        crop.set_offset((offset.0 + moved.0, offset.1 + moved.1));
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(crop) = &mut this.crop {
                        crop.drag = None;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(crop) = &mut this.crop {
                        crop.drag = None;
                        cx.notify();
                    }
                }),
            )
            .on_scroll(
                cx.listener(|this, event: &gpui_kit::ScrollEvent, window, cx| {
                    cx.stop_propagation();
                    let Some(crop) = &mut this.crop else {
                        return;
                    };
                    let dy = event.delta.pixel_delta(px(20.)).y.as_f32();
                    let zoom = crop.zoom * (dy * 0.004).exp();
                    crop.set_zoom(zoom);
                    let zoom = crop.zoom;
                    crop.slider
                        .update(cx, |slider, cx| slider.set_value(zoom, window, cx));
                    cx.notify();
                }),
            );
        let saving = crop.saving;
        let title = match crop.purpose {
            CropFor::Avatar(_) => "Crop your picture",
            CropFor::SpaceIcon(..) => "Crop the server icon",
            CropFor::Banner(_) => "Crop your banner",
        };
        let card = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .child(div().w_full().child(heading(title)))
            .child(
                div()
                    .w_full()
                    .child(paragraph("Drag to move it, scroll to zoom.")),
            )
            .child(area)
            .child(
                div()
                    .w(px(area_w))
                    .child(Slider::new(&crop.slider).cursor_pointer()),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("crop-cancel")
                            .cursor_pointer()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.crop = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("crop-save")
                            .cursor_pointer()
                            .primary()
                            .disabled(saving)
                            .loading(saving)
                            .label(if saving { "Saving…" } else { "Save" })
                            .on_click(cx.listener(|this, _, _, cx| this.save_crop(cx))),
                    ),
            );
        Some(dialog("crop", area_w + 40., card))
    }
}

impl Crop {
    /// Zooms around the frame's middle: what is there stays there.
    fn set_zoom(&mut self, zoom: f32) {
        let zoom = zoom.clamp(1., MAX_ZOOM);
        let grow = zoom / self.zoom;
        self.zoom = zoom;
        self.set_offset((self.offset.0 * grow, self.offset.1 * grow));
    }

    fn set_offset(&mut self, offset: (f32, f32)) {
        let shape = self.purpose.shape();
        let shown = shown_size(shape, self.size, self.zoom);
        self.offset = clamp_offset(shape, shown, offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Shape {
        CropFor::Avatar(String::new()).shape()
    }

    #[test]
    fn a_wide_picture_crops_its_middle_square() {
        assert_eq!(
            crop_rect(square(), (400, 200), 1., (0., 0.)),
            (100, 0, 200, 200)
        );
    }

    #[test]
    fn moving_the_picture_right_crops_further_left() {
        let (x, _, w, _) = crop_rect(square(), (400, 200), 1., (SQUARE / 2., 0.));
        assert_eq!((x, w), (0, 200));
    }

    #[test]
    fn the_offset_stops_at_the_overhang() {
        let shown = shown_size(square(), (400, 200), 1.);
        let (x, y) = clamp_offset(square(), shown, (1000., 50.));
        assert!((x - SQUARE / 2.).abs() < 0.01 && y.abs() < 0.01, "{x} {y}");
    }

    #[test]
    fn zooming_crops_a_smaller_square() {
        assert_eq!(
            crop_rect(square(), (200, 200), 2., (0., 0.)),
            (50, 50, 100, 100)
        );
    }

    #[test]
    fn a_banner_crops_five_by_two() {
        let banner = CropFor::Banner(String::new()).shape();
        // A square picture: its full width, and the middle 2/5 of its height.
        assert_eq!(
            crop_rect(banner, (500, 500), 1., (0., 0.)),
            (0, 150, 500, 200)
        );
    }

    #[test]
    fn big_crops_are_made_smaller_keeping_their_shape() {
        assert_eq!(output_size((1000, 400), (1000, 400)), (1000, 400));
        assert_eq!(output_size((2000, 800), (1000, 400)), (1000, 400));
        assert_eq!(output_size((100, 100), (512, 512)), (100, 100));
    }

    #[test]
    fn an_animated_picture_stays_animated_when_cropped() {
        let gif = crate::images::tests::gif(3, 64);
        let out = crop_file(square(), &gif, 1., (0., 0.)).unwrap();
        assert_eq!(out.content_type, "image/gif");
        let frames = crate::images::animation_decoder(&out.bytes)
            .unwrap()
            .unwrap();
        assert_eq!(frames.count(), 3);
    }

    #[test]
    fn a_gif_of_one_frame_is_cropped_as_a_still_picture() {
        let gif = crate::images::tests::gif(1, 64);
        let out = crop_file(square(), &gif, 1., (0., 0.)).unwrap();
        assert_eq!(out.content_type, "image/png");
    }

    #[test]
    fn a_still_picture_is_cropped_to_a_png() {
        let png = {
            let mut bytes = Vec::new();
            image::RgbaImage::new(40, 20)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            bytes
        };
        let out = crop_file(square(), &png, 1., (0., 0.)).unwrap();
        assert_eq!(out.content_type, "image/png");
        let cropped = image::load_from_memory(&out.bytes).unwrap();
        assert_eq!((cropped.width(), cropped.height()), (20, 20));
    }
}
