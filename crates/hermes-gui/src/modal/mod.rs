//! Dialogs shown over the window (PLAN.md section 15.4).

pub mod add_instance;
pub mod confirm;
pub mod fingerprint;
pub mod new_dm;

use iced::widget::{center, container, mouse_area, opaque, stack};
use iced::{Border, Color, Element, Fill};

use crate::theme::Theme;

/// `content` in a centered card over a dimmed `base`. Clicks on the backdrop send `on_blur`.
pub fn overlay<'a, M: Clone + 'a>(
    base: impl Into<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
    theme: &'a Theme,
    on_blur: Option<M>,
) -> Element<'a, M> {
    let card = container(content)
        .padding(20)
        .max_width(520)
        .style(move |_| container::Style {
            text_color: Some(theme.text.primary.0),
            background: Some(theme.general.background.0.into()),
            border: Border {
                color: theme.general.border.0,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        });
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
