//! The bundled fonts and icons: the JetBrainsMono Nerd Font and Material icons the iced client
//! uses. Everything else (the kit's own icons) comes from the kit.

use std::borrow::Cow;

use gpui_kit::prelude::*;
use gpui_kit::{App, AssetSource, Result, Rgba, SharedString, Svg, px, svg};

pub const FEATHER: &str = "hermes/feather.svg";
pub const POUND: &str = "hermes/pound.svg";
pub const VOLUME: &str = "hermes/volume-high.svg";
pub const PLUS: &str = "hermes/plus.svg";
pub const COG: &str = "hermes/cog.svg";
pub const HANG_UP: &str = "hermes/phone-hangup.svg";

const ICONS: [(&str, &[u8]); 6] = [
    (FEATHER, include_bytes!("../assets/icons/mdi/feather.svg")),
    (POUND, include_bytes!("../assets/icons/mdi/pound.svg")),
    (
        VOLUME,
        include_bytes!("../assets/icons/mdi/volume-high.svg"),
    ),
    (PLUS, include_bytes!("../assets/icons/mdi/plus.svg")),
    (COG, include_bytes!("../assets/icons/mdi/cog.svg")),
    (
        HANG_UP,
        include_bytes!("../assets/icons/mdi/phone-hangup.svg"),
    ),
];

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

pub fn icon(path: &'static str, size: f32, color: Rgba) -> Svg {
    svg()
        .path(path)
        .flex_shrink_0()
        .size(px(size))
        .text_color(color)
}
