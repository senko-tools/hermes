//! Small view helpers. Every text goes through these so `[font]` changes apply live.

pub mod context_menu;
pub mod cursor;

use iced::widget::{Text, text};

use crate::font::Fonts;

pub fn label<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.ui).size(fonts.size)
}

pub fn small<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.ui).size(fonts.small())
}

pub fn heading<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.bold()).size(fonts.large())
}

pub fn strong<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.medium()).size(fonts.size)
}

/// `content` with a small tooltip below it (for buttons that only show a symbol).
pub fn tip<'a, M: 'a>(
    content: impl Into<iced::Element<'a, M>>,
    text: &'a str,
    fonts: &Fonts,
    theme: &'a crate::theme::Theme,
) -> iced::Element<'a, M> {
    iced::widget::tooltip(
        content,
        iced::widget::container(small(text, fonts).color(theme.text.primary.0))
            .padding([3, 8])
            .style(move |_| iced::widget::container::Style {
                background: Some(theme.sidebar.background.0.into()),
                border: iced::Border {
                    color: theme.general.border.0,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..iced::widget::container::Style::default()
            }),
        iced::widget::tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}
