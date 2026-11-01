//! Icons from the bundled Lucide font (`assets/fonts/Lucide.ttf`, a subset of
//! lucide-static 0.468.0). To add one, re-subset the font with its code point from
//! lucide-static's `font/lucide.css` and add the constant here.

use iced::widget::{Text, text};

pub const HASH: char = '\u{e0f2}';
pub const VOLUME_2: char = '\u{e1aa}';
pub const MIC: char = '\u{e11b}';
pub const MIC_OFF: char = '\u{e11c}';
pub const HEADPHONES: char = '\u{e0f4}';
pub const HEADPHONE_OFF: char = '\u{e62d}';
pub const PHONE_OFF: char = '\u{e13b}';
pub const SETTINGS: char = '\u{e157}';
pub const AT_SIGN: char = '\u{e052}';
pub const PIN: char = '\u{e258}';
pub const SEARCH: char = '\u{e154}';
pub const USERS: char = '\u{e1a3}';
pub const PLUS: char = '\u{e140}';
pub const SMILE_PLUS: char = '\u{e300}';
pub const REPLY: char = '\u{e229}';
pub const PENCIL: char = '\u{e1f8}';
pub const ELLIPSIS: char = '\u{e0ba}';
pub const ELLIPSIS_VERTICAL: char = '\u{e0bb}';
pub const X: char = '\u{e1b1}';
pub const CHEVRON_DOWN: char = '\u{e071}';
pub const CHEVRON_RIGHT: char = '\u{e073}';
pub const PAPERCLIP: char = '\u{e130}';
pub const MAXIMIZE_2: char = '\u{e116}';
pub const MINIMIZE_2: char = '\u{e11e}';
pub const MESSAGE_SQUARE: char = '\u{e11a}';
pub const PHONE: char = '\u{e136}';
pub const BELL: char = '\u{e05d}';
pub const ARROW_DOWN: char = '\u{e046}';
pub const CHECK: char = '\u{e070}';
pub const USER_PLUS: char = '\u{e1a1}';
pub const LOCK: char = '\u{e10e}';
pub const FILE: char = '\u{e0c4}';
pub const AUDIO_LINES: char = '\u{e55e}';
pub const LINK: char = '\u{e107}';
pub const FEATHER: char = '\u{e0c2}';
pub const CIRCLE_MINUS: char = '\u{e083}';
pub const SERVER: char = '\u{e156}';
pub const SHIELD: char = '\u{e15b}';

/// `glyph` at `size` logical pixels, centered in a square box so icons line up with each other.
pub fn icon<'a>(glyph: char, size: f32) -> Text<'a> {
    text(glyph.to_string())
        .font(crate::font::ICONS)
        .size(size)
        .line_height(iced::widget::text::LineHeight::Absolute(size.into()))
        .width(size)
        .align_x(iced::alignment::Horizontal::Center)
}
