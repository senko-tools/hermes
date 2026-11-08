//! The crop view shown after picking a server icon or an avatar: the picture under a fixed
//! frame, dragged to move and scrolled (or slid) to zoom. Saving crops it to a square PNG and
//! uploads that.

use std::sync::Arc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, MouseButton, ObjectFit, Pixels, Point, RenderImage, Subscription,
    Window, div, img,
};
use hermes_client_core::proto::api::{UpdateMeRequest, UpdateSpaceRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::{CoreError, InstanceId};

use crate::modal::{dialog, heading, paragraph};
use crate::shell::Shell;
use crate::theme::{self, px};

/// The frame's side, and the area around it where the rest of the picture shows dimmed.
const FRAME: f32 = 240.;
const AREA: f32 = 320.;
/// How far one can zoom in.
const MAX_ZOOM: f32 = 5.;
/// The side of the uploaded picture, at most.
const OUTPUT: u32 = 512;

/// What the cropped picture becomes.
#[derive(Clone)]
pub enum CropFor {
    Avatar(InstanceId),
    SpaceIcon(InstanceId, Id),
}

pub struct Crop {
    pub purpose: CropFor,
    /// The picture as picked, and its preview.
    source: Arc<image::DynamicImage>,
    preview: Arc<RenderImage>,
    /// 1 = the picture's short side just fills the frame.
    zoom: f32,
    /// How far the picture's centre is from the frame's, in layout units.
    offset: (f32, f32),
    /// A drag: where the pointer went down, and the offset then.
    drag: Option<(Point<Pixels>, (f32, f32))>,
    saving: bool,
    slider: Entity<SliderState>,
    _slider: Subscription,
}

/// The picture's size in the frame at `zoom`, in layout units.
fn shown_size(width: u32, height: u32, zoom: f32) -> (f32, f32) {
    let scale = FRAME / width.min(height).max(1) as f32 * zoom;
    (width as f32 * scale, height as f32 * scale)
}

/// Keeps the frame covered: the picture cannot move further than its overhang.
fn clamp_offset(shown: (f32, f32), offset: (f32, f32)) -> (f32, f32) {
    let room_x = ((shown.0 - FRAME) / 2.).max(0.);
    let room_y = ((shown.1 - FRAME) / 2.).max(0.);
    (
        offset.0.clamp(-room_x, room_x),
        offset.1.clamp(-room_y, room_y),
    )
}

/// The square of the source picture under the frame: `(x, y, side)` in source pixels.
fn crop_rect(width: u32, height: u32, zoom: f32, offset: (f32, f32)) -> (u32, u32, u32) {
    let shown = shown_size(width, height, zoom);
    let scale = shown.0 / width.max(1) as f32;
    let (ox, oy) = clamp_offset(shown, offset);
    let left = (shown.0 - FRAME) / 2. - ox;
    let top = (shown.1 - FRAME) / 2. - oy;
    let side = ((FRAME / scale).round() as u32).clamp(1, width.min(height).max(1));
    let x = ((left / scale).round().max(0.) as u32).min(width.saturating_sub(side));
    let y = ((top / scale).round().max(0.) as u32).min(height.saturating_sub(side));
    (x, y, side)
}

/// Crops and scales the picture to a PNG.
fn crop_png(
    source: &image::DynamicImage,
    zoom: f32,
    offset: (f32, f32),
) -> Result<Vec<u8>, String> {
    let (x, y, side) = crop_rect(source.width(), source.height(), zoom, offset);
    let mut cropped = source.crop_imm(x, y, side, side);
    if side > OUTPUT {
        cropped = cropped.resize_exact(OUTPUT, OUTPUT, image::imageops::FilterType::Lanczos3);
    }
    let mut bytes = Vec::new();
    cropped
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

impl Shell {
    /// Asks for a picture and opens the crop view on it.
    pub fn pick_picture(&mut self, purpose: CropFor, cx: &mut Context<Self>) {
        let title = match purpose {
            CropFor::Avatar(_) => "Choose a picture",
            CropFor::SpaceIcon(..) => "Choose a server icon",
        };
        self.run(
            cx,
            async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .set_title(title)
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                    .pick_file()
                    .await
                else {
                    return Ok(None);
                };
                let bytes = tokio::fs::read(file.path())
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::task::spawn_blocking(move || {
                    let source = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
                    let preview = crate::images::decode(&bytes, Some(1024))?;
                    Ok(Some((Arc::new(source), preview)))
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |this, result: Result<_, String>, window, cx| match result {
                Ok(Some((source, preview))) => this.open_crop(purpose, source, preview, window, cx),
                Ok(None) => {}
                Err(err) => this.error(format!("could not open the picture: {err}"), cx),
            },
        );
    }

    fn open_crop(
        &mut self,
        purpose: CropFor,
        source: Arc<image::DynamicImage>,
        preview: Arc<RenderImage>,
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
            source,
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
        let (source, zoom, offset) = (crop.source.clone(), crop.zoom, crop.offset);
        let purpose = crop.purpose.clone();
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let png = tokio::task::spawn_blocking(move || crop_png(&source, zoom, offset))
                    .await
                    .map_err(|e| e.to_string())??;
                let inst = match &purpose {
                    CropFor::Avatar(inst) | CropFor::SpaceIcon(inst, _) => inst.clone(),
                };
                let work = async {
                    let rest = core.rest(&inst)?;
                    let picture = rest.upload("picture.png", "image/png", png).await?;
                    match purpose {
                        CropFor::Avatar(_) => {
                            rest.update_me(&UpdateMeRequest {
                                avatar_id: Some(Some(picture.id)),
                                ..UpdateMeRequest::default()
                            })
                            .await?;
                            Ok::<_, CoreError>("Avatar changed")
                        }
                        CropFor::SpaceIcon(_, space) => {
                            rest.update_space(
                                space,
                                &UpdateSpaceRequest {
                                    icon_id: Some(Some(picture.id)),
                                    ..UpdateSpaceRequest::default()
                                },
                            )
                            .await?;
                            Ok("Icon changed")
                        }
                    }
                };
                work.await.map_err(|e| e.to_string())
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
        let (width, height) = (crop.source.width(), crop.source.height());
        let shown = shown_size(width, height, crop.zoom);
        let (ox, oy) = crop.offset;
        let round = matches!(crop.purpose, CropFor::Avatar(_));
        // A ring as thick as the area dims everything outside the frame; its inner edge is the
        // frame's shape.
        let ring = AREA;
        let inner_radius = if round {
            FRAME / 2.
        } else {
            FRAME * 14. / theme::RAIL_ICON
        };
        let area = div()
            .id("crop-area")
            .relative()
            .flex_shrink_0()
            .size(px(AREA))
            .overflow_hidden()
            .rounded(px(8.))
            .bg(theme::sidebar())
            .cursor_grab()
            .child(
                img(crop.preview.clone())
                    .absolute()
                    .left(px((AREA - shown.0) / 2. + ox))
                    .top(px((AREA - shown.1) / 2. + oy))
                    .w(px(shown.0))
                    .h(px(shown.1))
                    .object_fit(ObjectFit::Fill),
            )
            .child(
                div()
                    .absolute()
                    .left(px((AREA - FRAME) / 2. - ring))
                    .top(px((AREA - FRAME) / 2. - ring))
                    .size(px(FRAME + 2. * ring))
                    .rounded(px(inner_radius + ring))
                    .border(px(ring))
                    .border_color(theme::scrim()),
            )
            .child(
                div()
                    .absolute()
                    .left(px((AREA - FRAME) / 2.))
                    .top(px((AREA - FRAME) / 2.))
                    .size(px(FRAME))
                    .rounded(px(inner_radius))
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
        let card = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .child(div().w_full().child(heading(if round {
                "Crop your picture"
            } else {
                "Crop the server icon"
            })))
            .child(
                div()
                    .w_full()
                    .child(paragraph("Drag to move it, scroll to zoom.")),
            )
            .child(area)
            .child(
                div()
                    .w(px(AREA))
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
                            .label(if saving { "Saving…" } else { "Save" })
                            .on_click(cx.listener(|this, _, _, cx| this.save_crop(cx))),
                    ),
            );
        Some(dialog("crop", AREA + 40., card))
    }
}

impl Crop {
    fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(1., MAX_ZOOM);
        self.set_offset(self.offset);
    }

    fn set_offset(&mut self, offset: (f32, f32)) {
        let shown = shown_size(self.source.width(), self.source.height(), self.zoom);
        self.offset = clamp_offset(shown, offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_picture_crops_its_middle_square() {
        assert_eq!(crop_rect(400, 200, 1., (0., 0.)), (100, 0, 200));
    }

    #[test]
    fn moving_the_picture_right_crops_further_left() {
        let (x, _, side) = crop_rect(400, 200, 1., (FRAME / 2., 0.));
        assert_eq!((x, side), (0, 200));
    }

    #[test]
    fn the_offset_stops_at_the_overhang() {
        let shown = shown_size(400, 200, 1.);
        let (x, y) = clamp_offset(shown, (1000., 50.));
        assert!((x - FRAME / 2.).abs() < 0.01 && y.abs() < 0.01, "{x} {y}");
    }

    #[test]
    fn zooming_crops_a_smaller_square() {
        assert_eq!(crop_rect(200, 200, 2., (0., 0.)), (50, 50, 100));
    }
}
