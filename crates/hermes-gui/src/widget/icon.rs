//! Icons: Material Design glyphs from the bundled JetBrainsMono Nerd Font, so they render the
//! same whatever `font.family` is. Code points are the `md-*` entries of Nerd Fonts'
//! `glyphnames.json`.

use iced::widget::{Text, text};

pub const HASH: char = '\u{f0423}'; // md-pound
pub const VOLUME: char = '\u{f057e}'; // md-volume_high
pub const MIC: char = '\u{f036c}'; // md-microphone
pub const MIC_OFF: char = '\u{f036d}'; // md-microphone_off
pub const HEADPHONES: char = '\u{f02cb}'; // md-headphones
pub const HEADPHONES_OFF: char = '\u{f07ce}'; // md-headphones_off
pub const HANG_UP: char = '\u{f03f5}'; // md-phone_hangup
pub const SETTINGS: char = '\u{f0493}'; // md-cog
pub const AT: char = '\u{f0065}'; // md-at
pub const PIN: char = '\u{f0403}'; // md-pin
pub const SEARCH: char = '\u{f0349}'; // md-magnify
pub const USERS: char = '\u{f000e}'; // md-account_multiple
pub const PLUS: char = '\u{f0415}'; // md-plus
pub const REACT: char = '\u{f01f5}'; // md-emoticon_happy_outline
pub const REPLY: char = '\u{f045a}'; // md-reply
pub const PENCIL: char = '\u{f03eb}'; // md-pencil
pub const MORE: char = '\u{f01d8}'; // md-dots_horizontal
pub const MORE_VERTICAL: char = '\u{f01d9}'; // md-dots_vertical
pub const CLOSE: char = '\u{f0156}'; // md-close
pub const CHEVRON_DOWN: char = '\u{f0140}'; // md-chevron_down
pub const CHEVRON_RIGHT: char = '\u{f0142}'; // md-chevron_right
pub const ATTACH: char = '\u{f03e2}'; // md-paperclip
pub const MAXIMIZE: char = '\u{f0293}'; // md-fullscreen
pub const RESTORE: char = '\u{f0294}'; // md-fullscreen_exit
pub const SPLIT_RIGHT: char = '\u{f0bcc}'; // md-view_split_vertical
pub const SPLIT_DOWN: char = '\u{f0bcb}'; // md-view_split_horizontal
pub const CHAT: char = '\u{f0365}'; // md-message_outline
pub const PHONE: char = '\u{f03f2}'; // md-phone
pub const BELL: char = '\u{f009c}'; // md-bell_outline
pub const BELL_OFF: char = '\u{f0a91}'; // md-bell_off_outline
pub const ARROW_DOWN: char = '\u{f0045}'; // md-arrow_down
pub const CHECK: char = '\u{f012c}'; // md-check
pub const ACCOUNT: char = '\u{f0009}'; // md-account_circle
pub const LOCK: char = '\u{f033e}'; // md-lock
pub const FILE: char = '\u{f0224}'; // md-file_outline
pub const IMAGE: char = '\u{f0976}'; // md-image_outline
pub const WAVEFORM: char = '\u{f147d}'; // md-waveform
pub const LINK: char = '\u{f0339}'; // md-link_variant
pub const FEATHER: char = '\u{f06d3}'; // md-feather
pub const BAN: char = '\u{f073a}'; // md-cancel
pub const SERVER: char = '\u{f048b}'; // md-server
pub const SHIELD: char = '\u{f088f}'; // md-shield_account
pub const DOWNLOAD: char = '\u{f01da}'; // md-download
pub const DOT: char = '\u{f0765}'; // md-circle
pub const COPY: char = '\u{f018f}'; // md-content_copy
pub const DELETE: char = '\u{f09e7}'; // md-delete_outline
pub const PALETTE: char = '\u{f03d8}'; // md-palette
pub const KEYBOARD: char = '\u{f030c}'; // md-keyboard
pub const INFO: char = '\u{f02fd}'; // md-information_outline
pub const LOG_OUT: char = '\u{f0343}'; // md-logout
pub const ADD_USER: char = '\u{f0014}'; // md-account_plus
pub const HOME: char = '\u{f06a1}'; // md-home_outline

/// `glyph` at `size` logical pixels, centered in a square box so icons line up with each other.
pub fn icon<'a>(glyph: char, size: f32) -> Text<'a> {
    text(glyph.to_string())
        .font(crate::font::ICONS)
        .size(size)
        .line_height(iced::widget::text::LineHeight::Absolute(size.into()))
        .width(size)
        .height(size)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
}
