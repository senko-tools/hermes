//! Full-size image viewer (PLAN.md section 15.2): the original image over the whole window.
//! A click anywhere or Escape closes it.

use hermes_client_core::InstanceId;
use hermes_client_core::proto::models::Attachment;
use iced::widget::{center, column, container, image, mouse_area, opaque, row, space, stack};
use iced::{Alignment, Color, ContentFit, Element, Fill};

use crate::buffer::channel::message::{Img, human_size};
use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, label};

#[derive(Debug, Clone)]
pub struct Viewer {
    pub inst: InstanceId,
    pub attachment: Attachment,
    pub image: Img,
}

pub fn view<'a, M: Clone + 'a>(
    base: Element<'a, M>,
    viewer: &'a Viewer,
    fonts: &Fonts,
    theme: &'a Theme,
    on_close: M,
    on_download: M,
) -> Element<'a, M> {
    let picture: Element<'a, M> = match &viewer.image {
        Img::Ready(handle) => image(handle.clone())
            .content_fit(ContentFit::ScaleDown)
            .width(Fill)
            .height(Fill)
            .into(),
        Img::Loading => center(label("Loading…", fonts).color(Color::WHITE)).into(),
        Img::Failed => {
            center(label("The image could not be loaded.", fonts).color(Color::WHITE)).into()
        }
    };
    let bar = row![
        label(viewer.attachment.filename.clone(), fonts).color(Color::WHITE),
        caption(human_size(viewer.attachment.size), fonts).color(Color::from_rgb(0.7, 0.7, 0.7)),
        space::horizontal(),
        text_button("Download", ButtonKind::Secondary, fonts, theme).on_press(on_download),
        text_button("Close", ButtonKind::Secondary, fonts, theme).on_press(on_close.clone()),
    ]
    .spacing(style::GAP)
    .align_y(Alignment::Center);
    let overlay = container(
        column![
            mouse_area(container(picture).width(Fill).height(Fill)).on_press(on_close),
            bar
        ]
        .spacing(style::GAP),
    )
    .padding(style::GAP_XL)
    .width(Fill)
    .height(Fill)
    .style(|_| container::Style {
        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.88).into()),
        ..container::Style::default()
    });
    stack![base, opaque(overlay)].into()
}
