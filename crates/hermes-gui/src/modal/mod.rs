//! Dialogs shown over the window (PLAN.md section 15.4).

pub mod add_instance;
pub mod command_bar;
pub mod confirm;
pub mod fingerprint;
pub mod form;
pub mod image_viewer;
pub mod new_dm;

use iced::widget::{center, container, mouse_area, opaque, stack};
use iced::{Color, Element, Fill};

use crate::theme::Theme;

/// `content` in a centered card over a dimmed `base`. Clicks on the backdrop send `on_blur`.
pub fn overlay<'a, M: Clone + 'a>(
    base: impl Into<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
    theme: &'a Theme,
    on_blur: Option<M>,
) -> Element<'a, M> {
    let card = container(content)
        .padding(24)
        .max_width(540)
        .style(move |_| crate::theme::style::card(theme));
    let backdrop = center(opaque(card))
        .width(Fill)
        .height(Fill)
        .style(|_| container::Style {
            background: Some(
                Color {
                    a: 0.6,
                    ..Color::BLACK
                }
                .into(),
            ),
            ..container::Style::default()
        });
    let backdrop = match on_blur {
        Some(message) => mouse_area(backdrop).on_press(message).into(),
        None => Element::from(backdrop),
    };
    stack![base.into(), opaque(backdrop)].into()
}
