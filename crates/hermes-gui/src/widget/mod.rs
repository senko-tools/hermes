//! Small view helpers. Every text goes through these so `[font]` changes apply live.

pub mod avatar;
pub mod context_menu;
pub mod cursor;
pub mod icon;

use iced::widget::{Button, Text, button, text};

use crate::font::Fonts;
use crate::theme::style;

// The type scale (`Fonts::caption` .. `Fonts::display`): use these instead of sizing text by hand.

/// Body text: rows, buttons, messages, everything without a reason to differ.
pub fn label<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.ui).size(fonts.size)
}

/// Captions: timestamps, hints, badges.
pub fn caption<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.ui).size(fonts.caption())
}

/// Body-sized emphasis: names, setting names.
pub fn strong<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.medium()).size(fonts.size)
}

/// Titles: the pane header, the space name, the sidebar brand.
pub fn title<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.bold()).size(fonts.title())
}

/// Page headings.
pub fn heading<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.bold()).size(fonts.heading())
}

/// Welcome and empty screens.
pub fn display<'a>(content: impl text::IntoFragment<'a>, fonts: &Fonts) -> Text<'a> {
    text(content).font(fonts.bold()).size(fonts.display())
}

/// A small upper-case title over a group of rows (categories, member groups, settings groups).
pub fn section_title<'a>(content: &str, fonts: &Fonts) -> Text<'a> {
    text(content.to_uppercase())
        .font(fonts.bold())
        .size(fonts.caption())
        .wrapping(text::Wrapping::None)
}

/// What a [`text_button`] looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

/// A button with a text label: every one of them has the same height and text size.
pub fn text_button<'a, M: Clone + 'a>(
    content: impl text::IntoFragment<'a>,
    kind: ButtonKind,
    fonts: &Fonts,
    theme: &'a crate::theme::Theme,
) -> Button<'a, M> {
    button(iced::widget::container(label(content, fonts)).center_y(style::CONTROL_H))
        .padding([0.0, style::BUTTON_PAD.left])
        .style(move |_, status| match kind {
            ButtonKind::Primary => style::primary(theme, status),
            ButtonKind::Secondary => style::secondary(theme, status),
            ButtonKind::Danger => style::danger(theme, status),
        })
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
        iced::widget::container(caption(text, fonts).color(theme.text.primary.0))
            .padding([style::GAP_XS, style::GAP_S])
            .style(move |_| style::popover(theme)),
        position,
    )
    .gap(style::GAP_S)
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
