//! Things that change on a spring while the pointer is over them. gpui's own `.hover` style
//! switches at once, so these note the hover in `Shell::hovered` and spring towards it.

use gpui_kit::{App, Context, SharedString, SpringConfig, Window};

use crate::shell::Shell;

/// Quick and barely bouncy: follows the pointer in about a fifth of a second.
pub const HOVER_SPRING: SpringConfig = SpringConfig::new(520., 40., 1.);

impl Shell {
    /// Whether the pointer is over what `key` names.
    pub fn is_hovered(&self, key: &str) -> bool {
        self.hovered.contains(key)
    }

    /// An `on_hover` listener that notes the pointer over `key`. Several can be true at once, so
    /// something inside a hovered tile does not end the tile's hover.
    pub fn track_hover(
        &self,
        key: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let key = key.into();
        cx.listener(move |this, hovered: &bool, _, cx| {
            let changed = if *hovered {
                this.hovered.insert(key.clone())
            } else {
                this.hovered.remove(&key)
            };
            if changed {
                cx.notify();
            }
        })
    }
}
