//! Confirmation for destructive actions (PLAN.md section 15.4).

use iced::widget::{column, row, space};
use iced::{Alignment, Element};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{heading, label};

pub fn view<'a, M: Clone + 'a>(
    title: impl iced::widget::text::IntoFragment<'a>,
    text: &'a str,
    confirm: &'a str,
    fonts: &Fonts,
    theme: &'a Theme,
    on_confirm: M,
    on_cancel: M,
) -> Element<'a, M> {
    column![
        heading(title, fonts),
        label(text, fonts),
        row![
            space::horizontal(),
            text_button("Cancel", ButtonKind::Secondary, fonts, theme).on_press(on_cancel),
            text_button(confirm, ButtonKind::Danger, fonts, theme).on_press(on_confirm),
        ]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center),
    ]
    .spacing(style::GAP)
    .into()
}
