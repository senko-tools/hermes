//! The bundled fonts and icons: the JetBrainsMono Nerd Font and Material icons the iced client
//! uses. Everything else (the kit's own icons) comes from the kit.

use std::borrow::Cow;

use gpui_kit::prelude::*;
use gpui_kit::{App, AssetSource, Result, Rgba, SharedString, Svg, svg};

use crate::theme::px;

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

pub const FEATHER: &str = "feather";
pub const POUND: &str = "pound";
pub const VOLUME: &str = "volume-high";
pub const PLUS: &str = "plus";
pub const LAN_CONNECT: &str = "lan-connect";
pub const LAN_PENDING: &str = "lan-pending";
pub const LAN_DISCONNECT: &str = "lan-disconnect";
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
/// The handle on a row being dragged.
pub const DRAG: &str = "dots-vertical";
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
/// A monitor with a cross on its screen: stop sharing (drawn for Hermes from MDI's monitor).
pub const MONITOR_STOP: &str = "monitor-stop";
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
pub const COPY: &str = "content-copy";
pub const EMOJI: &str = "emoticon-happy-outline";
/// The media cards' icons.
pub mod media_icons {
    pub const PLAY: &str = "play";
    pub const PAUSE: &str = "pause";
    pub const VOLUME_OFF: &str = "volume-off";
    pub const MUSIC: &str = "music-note";
    pub const FULLSCREEN: &str = "fullscreen";
    pub const FULLSCREEN_EXIT: &str = "fullscreen-exit";
}
pub const BELL: &str = "bell-outline";
pub const ACCOUNT_PLUS: &str = "account-plus";
pub const LINK: &str = "link-variant";
pub const BAN: &str = "cancel";
pub const FOLDER_PLUS: &str = "folder-plus-outline";
pub const ALERT: &str = "alert";
pub const PULSE: &str = "pulse";
pub const EYE: &str = "eye-outline";
pub const BOOKMARK: &str = "bookmark-outline";
pub const CALENDAR: &str = "calendar";

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
