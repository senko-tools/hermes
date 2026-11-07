//! Colours copied from the iced client's dark theme (`hermes-gui/assets/themes/dark.toml`) until
//! the theme system is ported, and sizes close to Discord's.

use gpui_kit::{Rgba, rgb};

pub const FONT: &str = "JetBrainsMono Nerd Font";

/// The icon rail on the far left.
pub const RAIL_WIDTH: f32 = 72.0;
pub const RAIL_ICON: f32 = 48.0;
pub const SIDEBAR_WIDTH: f32 = 240.0;
pub const ROW_HEIGHT: f32 = 34.0;
pub const ROW_ICON: f32 = 20.0;
/// The space name over its channels, and the user panel's height is close to it.
pub const SIDEBAR_HEADER: f32 = 48.0;
pub const TEXT: f32 = 16.0;
pub const TEXT_SMALL: f32 = 14.0;
pub const TEXT_TINY: f32 = 12.0;
pub const AVATAR: f32 = 40.0;
pub const AVATAR_SMALL: f32 = 32.0;

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
pub fn on_accent() -> Rgba {
    rgb(0x1d1c22)
}
pub fn speaking() -> Rgba {
    rgb(0x6fcf7f)
}
/// The app icon's colours (`hermes-gui/src/desktop/tray.rs`).
pub fn app_icon_background() -> Rgba {
    rgb(0x46424e)
}
pub fn app_icon_feather() -> Rgba {
    rgb(0xe4e4ea)
}
