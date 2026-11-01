//! Confirmation for destructive actions (PLAN.md section 15.4).

use iced::widget::{button, column, row, space};
use iced::{Alignment, Element};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{heading, label};

pub fn view<'a, M: Clone + 'a>(
    title: &'a str,
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
            button(label("Cancel", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(on_cancel),
            button(label(confirm, fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::danger(theme, s))
                .on_press(on_confirm),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(12)
    .into()
}
