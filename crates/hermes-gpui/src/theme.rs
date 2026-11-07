//! Colours copied from the iced client's dark theme (`hermes-gui/assets/themes/dark.toml`) until
//! the theme system is ported, and sizes close to Discord's.

use gpui_kit::{Rgba, rgb, rgba};
use hermes_client_core::proto::models::Status;

pub const FONT: &str = "JetBrainsMono Nerd Font";

/// The smallest window that still fits the rail, a narrow sidebar and a usable chat with its
/// header buttons.
pub const WINDOW_MIN_W: f32 = 720.0;
pub const WINDOW_MIN_H: f32 = 480.0;
/// The icon rail on the far left.
pub const RAIL_WIDTH: f32 = 72.0;
pub const RAIL_ICON: f32 = 48.0;
pub const SIDEBAR_WIDTH: f32 = 240.0;
pub const ROW_HEIGHT: f32 = 34.0;
pub const ROW_ICON: f32 = 20.0;
/// The space name over its channels, and the header over the main panel next to it.
pub const SIDEBAR_HEADER: f32 = 48.0;
pub const TEXT: f32 = 16.0;
pub const TEXT_SMALL: f32 = 14.0;
pub const TEXT_TINY: f32 = 12.0;
pub const AVATAR: f32 = 40.0;
pub const AVATAR_SMALL: f32 = 32.0;

/// A colour given as `0xRRGGBB` (role colours).
pub fn rgb_u32(color: u32) -> Rgba {
    rgb(color)
}

pub fn background() -> Rgba {
    rgb(0x1d1c22)
}
pub fn sidebar() -> Rgba {
    rgb(0x17161c)
}
pub fn rail() -> Rgba {
    rgb(0x121116)
}
pub fn hover() -> Rgba {
    rgb(0x221f29)
}
pub fn hover_soft() -> Rgba {
    rgba(0x221f2988)
}
pub fn selected() -> Rgba {
    rgb(0x2b2833)
}
pub fn border() -> Rgba {
    rgb(0x2e2c36)
}
pub fn text() -> Rgba {
    rgb(0xece7e0)
}
pub fn text_secondary() -> Rgba {
    rgb(0xaca5a1)
}
pub fn text_tertiary() -> Rgba {
    rgb(0x77717c)
}
pub fn accent() -> Rgba {
    rgb(0xe5a550)
}
pub fn accent_soft() -> Rgba {
    rgba(0xe5a55029)
}
pub fn on_accent() -> Rgba {
    rgb(0x1d1c22)
}
pub fn speaking() -> Rgba {
    rgb(0x6fcf7f)
}
pub fn danger() -> Rgba {
    rgb(0xe06c75)
}
pub fn link() -> Rgba {
    rgb(0x7cb7e8)
}
pub fn code_background() -> Rgba {
    rgb(0x141318)
}
pub fn mention_soft() -> Rgba {
    rgba(0xe5a5501f)
}
/// Behind dialogs.
pub fn scrim() -> Rgba {
    rgba(0x000000a0)
}
/// Behind the image viewer.
pub fn scrim_strong() -> Rgba {
    rgba(0x000000d8)
}
/// The app icon's colours (`hermes-gui/src/desktop/tray.rs`).
pub fn app_icon_background() -> Rgba {
    rgb(0x46424e)
}
pub fn app_icon_feather() -> Rgba {
    rgb(0xe4e4ea)
}
/// A presence dot.
pub fn status(status: Status) -> Rgba {
    match status {
        Status::Online => rgb(0x6fcf7f),
        Status::Idle => rgb(0xe5c07b),
        Status::Dnd => rgb(0xe06c75),
        Status::Offline => rgb(0x77717c),
    }
}
