//! Everything that opens over the window, in one list: what Escape closes first, what counts
//! as a dialog (the keyboard is its own then), and how each one goes. Before, three lists
//! (Escape, "a dialog is open", drawing) were kept by hand and had drifted apart: Escape did
//! not close the profile card, the emoji picker or the unencrypted-chat popup, and five
//! overlays did not count as open.

use gpui_kit::Context;

use crate::shell::Shell;

/// Something open over the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    KeysDialog,
    Crop,
    MediaFull,
    Viewer,
    Profile,
    EmojiPicker,
    VolumeMenu,
    SharePicker,
    Network,
    Unencrypted,
    Menu,
    CommandBar,
    Confirm,
    Form,
    NewDm,
    AddInstance,
}

impl Overlay {
    /// Every overlay, in the order Escape closes them: what sits on top first.
    pub const ESCAPE_ORDER: [Overlay; 16] = [
        Overlay::KeysDialog,
        Overlay::Crop,
        Overlay::MediaFull,
        Overlay::Viewer,
        Overlay::Profile,
        Overlay::EmojiPicker,
        Overlay::VolumeMenu,
        Overlay::SharePicker,
        Overlay::Network,
        Overlay::Unencrypted,
        Overlay::Menu,
        Overlay::CommandBar,
        Overlay::Confirm,
        Overlay::Form,
        Overlay::NewDm,
        Overlay::AddInstance,
    ];

    /// While it is open, keys are its own: they do not reach the chat or the keybinds. The
    /// menu is left out on purpose: it handles its keys itself, and is asked for separately.
    pub fn takes_keys(self) -> bool {
        !matches!(self, Overlay::Menu)
    }
}

impl Shell {
    /// Whether `overlay` is open (and not already on its way out).
    pub fn overlay_open(&self, overlay: Overlay) -> bool {
        let gone = |id: &str| crate::modal::closing(id);
        match overlay {
            Overlay::KeysDialog => self.keys_dialog.is_some() && !gone("keys-dialog"),
            Overlay::Crop => self.crop.is_some() && !gone("crop"),
            Overlay::MediaFull => self.media_full_open(),
            Overlay::Viewer => self.viewer.is_some(),
            Overlay::Profile => self.profile.is_some() && !gone("profile-card-in"),
            Overlay::EmojiPicker => self.emoji_picker.is_some() && !gone("emoji-picker-in"),
            Overlay::VolumeMenu => self.volume_menu.is_some() && !gone("volume-menu-in"),
            Overlay::SharePicker => self.share_menu.is_some(),
            Overlay::Network => self.network_open.is_some() && !gone("network-in"),
            Overlay::Unencrypted => self.unencrypted_popup.is_some(),
            Overlay::Menu => self.menu.is_some() && !gone("menu"),
            Overlay::CommandBar => self.command_bar.is_some() && !gone("command-bar"),
            Overlay::Confirm => self.confirm.is_some() && !gone("confirm"),
            Overlay::Form => self.form.is_some() && !gone("form"),
            Overlay::NewDm => self.new_dm.is_some() && !gone("new-dm"),
            Overlay::AddInstance => self.add_instance.is_some() && !gone("add-instance"),
        }
    }

    /// Closes `overlay` the way it goes (playing its way out where it has one).
    pub fn close_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        match overlay {
            Overlay::KeysDialog => self.close_later("keys-dialog", |s| s.keys_dialog = None, cx),
            Overlay::Crop => self.close_later("crop", |s| s.crop = None, cx),
            Overlay::MediaFull => {
                self.close_media_full();
            }
            Overlay::Viewer => self.viewer = None,
            Overlay::Profile => self.close_later("profile-card-in", |s| s.profile = None, cx),
            Overlay::EmojiPicker => {
                self.close_later("emoji-picker-in", |s| s.emoji_picker = None, cx);
            }
            Overlay::VolumeMenu => {
                self.close_later("volume-menu-in", |s| s.volume_menu = None, cx);
            }
            Overlay::SharePicker => {
                self.take_share_picker();
            }
            Overlay::Network => self.close_later("network-in", |s| s.network_open = None, cx),
            Overlay::Unencrypted => self.unencrypted_popup = None,
            Overlay::Menu => self.close_later("menu", |s| s.menu = None, cx),
            Overlay::CommandBar => self.close_later("command-bar", |s| s.command_bar = None, cx),
            Overlay::Confirm => self.close_later("confirm", |s| s.confirm = None, cx),
            Overlay::Form => self.close_later("form", |s| s.form = None, cx),
            Overlay::NewDm => self.close_later("new-dm", |s| s.new_dm = None, cx),
            Overlay::AddInstance => {
                self.close_later("add-instance", |s| s.add_instance = None, cx);
            }
        }
        cx.notify();
    }

    /// The overlay on top, the one Escape closes.
    pub fn top_overlay(&self) -> Option<Overlay> {
        Overlay::ESCAPE_ORDER
            .into_iter()
            .find(|o| self.overlay_open(*o))
    }

    /// Whether something covers the window that keys belong to.
    pub fn dialog_open(&self) -> bool {
        Overlay::ESCAPE_ORDER
            .into_iter()
            .any(|o| o.takes_keys() && self.overlay_open(o))
    }
}

#[cfg(test)]
mod tests {
    use super::Overlay;

    #[test]
    fn every_overlay_is_in_the_escape_order_once() {
        let order = Overlay::ESCAPE_ORDER;
        for (n, o) in order.iter().enumerate() {
            assert!(!order[n + 1..].contains(o), "{o:?} twice");
        }
        // Adding a variant without placing it here fails to compile in `overlay_open`'s
        // match, and this length keeps the two together.
        assert_eq!(order.len(), 16);
    }
}
