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
