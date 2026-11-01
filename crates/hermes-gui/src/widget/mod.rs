//! Small view helpers. Every text goes through these so `[font]` changes apply live.

pub mod avatar;
pub mod context_menu;
pub mod cursor;
pub mod icon;

use iced::widget::{Button, Text, button, text};

use crate::font::Fonts;
use crate::theme::style;

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
    tip_at(
        content,
        text,
        iced::widget::tooltip::Position::Bottom,
        fonts,
        theme,
    )
}

/// [`tip`] on a chosen side.
pub fn tip_at<'a, M: 'a>(
    content: impl Into<iced::Element<'a, M>>,
    text: &'a str,
    position: iced::widget::tooltip::Position,
    fonts: &Fonts,
    theme: &'a crate::theme::Theme,
) -> iced::Element<'a, M> {
    iced::widget::tooltip(
        content,
        iced::widget::container(small(text, fonts).color(theme.text.primary.0))
            .padding([5, 10])
            .style(move |_| style::popover(theme)),
        position,
    )
    .gap(6)
    .into()
}

/// A square button holding one icon; it shows its background on hover, or always (in the
/// accent color) when `active`.
pub fn icon_button<'a, M: Clone + 'a>(
    glyph: char,
    active: bool,
    theme: &'a crate::theme::Theme,
) -> Button<'a, M> {
    button(
        iced::widget::container(icon::icon(glyph, style::ICON))
            .center_x(style::ICON_BUTTON)
            .center_y(style::ICON_BUTTON),
    )
    .padding(0)
    .style(move |_, status| style::ghost(theme, status, active))
}
