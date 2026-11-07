//! The bundled fonts and icons: the JetBrainsMono Nerd Font and Material icons the iced client
//! uses. Everything else (the kit's own icons) comes from the kit.

use std::borrow::Cow;

use gpui_kit::prelude::*;
use gpui_kit::{App, AssetSource, Result, Rgba, SharedString, Svg, px, svg};

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

pub const FEATHER: &str = "feather";
pub const POUND: &str = "pound";
pub const VOLUME: &str = "volume-high";
pub const PLUS: &str = "plus";
pub const COG: &str = "cog";
pub const HANG_UP: &str = "phone-hangup";
pub const AT: &str = "at";
pub const PIN: &str = "pin";
pub const SEARCH: &str = "magnify";
pub const USERS: &str = "account-multiple";
pub const REPLY: &str = "reply";
pub const PENCIL: &str = "pencil";
pub const CLOSE: &str = "close";
pub const CHEVRON_DOWN: &str = "chevron-down";
pub const CHEVRON_RIGHT: &str = "chevron-right";
pub const ATTACH: &str = "paperclip";
pub const CHAT: &str = "message-outline";
pub const PHONE: &str = "phone";
pub const CHECK: &str = "check";
pub const FILE: &str = "file-outline";
pub const DOWNLOAD: &str = "download";
pub const MIC: &str = "microphone";
pub const MIC_OFF: &str = "microphone-off";
pub const HEADPHONES: &str = "headphones";
pub const HEADPHONES_OFF: &str = "headphones-off";
pub const MONITOR_SHARE: &str = "monitor-share";
pub const WAVEFORM: &str = "waveform";
pub const ARROW_LEFT: &str = "arrow-left";
pub const ACCOUNT: &str = "account-circle";
pub const INFO: &str = "information-outline";
pub const LOCK: &str = "lock";
pub const LOCK_OPEN: &str = "lock-open";
pub const KEYBOARD: &str = "keyboard";
pub const PALETTE: &str = "palette";
pub const SERVER: &str = "server";
pub const SHIELD: &str = "shield-account";
pub const BELL: &str = "bell-outline";
pub const LINK: &str = "link-variant";
pub const BAN: &str = "cancel";
pub const FOLDER_PLUS: &str = "folder-plus-outline";

const FONTS: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Medium.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Italic.ttf"),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ICONS.iter().find(|(name, _)| *name == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

pub fn load_fonts(cx: &mut App) {
    let fonts = FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    cx.text_system()
        .add_fonts(fonts)
        .expect("the bundled fonts load");
}

pub fn icon(name: &str, size: f32, color: Rgba) -> Svg {
    svg()
        .path(SharedString::from(format!("hermes/{name}.svg")))
        .flex_shrink_0()
        .size(px(size))
        .text_color(color)
}
