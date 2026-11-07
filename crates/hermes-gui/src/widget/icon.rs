//! Icons: Material Design Icons (Pictogrammers, Apache-2.0) as SVG files in
//! `assets/icons/mdi`. Every icon is drawn on the same 24×24 grid, so an icon drawn in a square
//! box is centered by design: no per-icon offsets to measure or forget. To add one, put its SVG
//! (from the same pinned MaterialDesign-SVG release) in that folder and add a line below.

use std::collections::HashMap;
use std::sync::LazyLock;

use iced::widget::svg;
use iced::{Color, Element};

/// One icon, named by its file in `assets/icons/mdi` (without `.svg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Glyph(&'static str);

impl Glyph {
    /// The parsed-once SVG handle for this icon.
    fn handle(self) -> svg::Handle {
        HANDLES
            .get(self.0)
            .cloned()
            .unwrap_or_else(|| svg::Handle::from_memory(&[][..]))
    }
}

macro_rules! icons {
    ($($name:ident = $file:literal;)*) => {
        $(pub const $name: Glyph = Glyph($file);)*

        /// Every icon with its SVG source.
        const ALL: &[(Glyph, &[u8])] = &[
            $(($name, include_bytes!(concat!("../../assets/icons/mdi/", $file, ".svg")))),*
        ];
    };
}

icons! {
    HASH = "pound";
    VOLUME = "volume-high";
    MIC = "microphone";
    MIC_OFF = "microphone-off";
    HEADPHONES = "headphones";
    HEADPHONES_OFF = "headphones-off";
    HANG_UP = "phone-hangup";
    SETTINGS = "cog";
    AT = "at";
    PIN = "pin";
    SEARCH = "magnify";
    USERS = "account-multiple";
    PLUS = "plus";
    REACT = "emoticon-happy-outline";
    REPLY = "reply";
    PENCIL = "pencil";
    MORE = "dots-horizontal";
    MORE_VERTICAL = "dots-vertical";
    CLOSE = "close";
    CHEVRON_DOWN = "chevron-down";
    CHEVRON_RIGHT = "chevron-right";
    ATTACH = "paperclip";
    MAXIMIZE = "fullscreen";
    RESTORE = "fullscreen-exit";
    SPLIT_RIGHT = "view-split-vertical";
    SPLIT_DOWN = "view-split-horizontal";
    CHAT = "message-outline";
    PHONE = "phone";
    BELL = "bell-outline";
    BELL_OFF = "bell-off-outline";
    ARROW_DOWN = "arrow-down";
    ARROW_LEFT = "arrow-left";
    CHECK = "check";
    ACCOUNT = "account-circle";
    LOCK = "lock";
    FILE = "file-outline";
    IMAGE = "image-outline";
    WAVEFORM = "waveform";
    MONITOR = "monitor";
    MONITOR_SHARE = "monitor-share";
    LINK = "link-variant";
    FEATHER = "feather";
    BAN = "cancel";
    SERVER = "server";
    SHIELD = "shield-account";
    DOWNLOAD = "download";
    DOT = "circle";
    COPY = "content-copy";
    DELETE = "delete-outline";
    PALETTE = "palette";
    KEYBOARD = "keyboard";
    INFO = "information-outline";
    LOG_OUT = "logout";
    ADD_USER = "account-plus";
    HOME = "home-outline";
    FOLDER = "folder";
    FOLDER_PLUS = "folder-plus-outline";
}

/// One handle per icon, so the SVG is parsed and cached by the renderer only once.
static HANDLES: LazyLock<HashMap<&'static str, svg::Handle>> = LazyLock::new(|| {
    ALL.iter()
        .map(|(glyph, bytes)| (glyph.0, svg::Handle::from_memory(*bytes)))
        .collect()
});

/// An icon: `glyph` drawn in a `size` logical-pixel square, in the text color unless `color`
/// is set.
pub struct Icon {
    glyph: Glyph,
    size: f32,
    color: Option<Color>,
}

impl Icon {
    pub fn color(mut self, color: impl Into<Color>) -> Self {
        self.color = Some(color.into());
        self
    }
}

pub fn icon(glyph: Glyph, size: f32) -> Icon {
    Icon {
        glyph,
        size,
        color: None,
    }
}

impl<'a, M: 'a> From<Icon> for Element<'a, M> {
    fn from(icon: Icon) -> Self {
        let color = icon.color;
        svg(icon.glyph.handle())
            .width(icon.size)
            .height(icon.size)
            .style(move |theme: &iced::Theme, _| svg::Style {
                color: Some(color.unwrap_or(theme.palette().text)),
            })
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon is a single-color glyph on the 24×24 Material grid, which is what keeps
    /// them centered and lets `svg::Style::color` recolor them.
    #[test]
    fn every_icon_is_on_the_24_grid() {
        for (glyph, bytes) in ALL {
            let svg = std::str::from_utf8(bytes).unwrap();
            assert!(svg.starts_with("<svg"), "{} is not an SVG", glyph.0);
            assert!(
                svg.contains(r#"viewBox="0 0 24 24""#),
                "{} is not on the 24×24 grid",
                glyph.0
            );
            assert!(svg.contains("<path"), "{} has no path", glyph.0);
        }
    }

    #[test]
    fn every_icon_has_a_handle() {
        assert_eq!(HANDLES.len(), ALL.len(), "two icons share a file name");
    }
}
