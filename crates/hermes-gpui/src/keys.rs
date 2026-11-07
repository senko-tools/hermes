//! Keys that act anywhere, before any element sees them.

use gpui_kit::{Context, Focusable as _, KeystrokeEvent, Window};

use crate::shell::Shell;

impl Shell {
    pub fn on_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let plain = !keystroke.modifiers.modified();
        if key == "escape" && plain && self.close_top(window, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.composer_key(key, plain, window, cx) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Closes the top dialog or menu. `false` if none is open.
    fn close_top(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        if self.viewer.take().is_some()
            || self.volume_menu.take().is_some()
            || self.share_menu.take().is_some()
            || self.network_open.take().is_some()
            || self.menu.take().is_some()
        {
            return true;
        }
        if self.confirm.take().is_some() || self.form.take().is_some() {
            return true;
        }
        self.new_dm.take().is_some() || self.add_instance.take().is_some()
    }

    /// Whether a dialog covers the window.
    pub fn dialog_open(&self) -> bool {
        self.viewer.is_some()
            || self.confirm.is_some()
            || self.form.is_some()
            || self.new_dm.is_some()
            || self.add_instance.is_some()
    }

    /// The keys the open chat's message box treats specially while it has the keyboard.
    fn composer_key(
        &mut self,
        key: &str,
        plain: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !plain || self.dialog_open() || self.menu.is_some() {
            return false;
        }
        let Some(channel) = self.open_channel() else {
            return false;
        };
        let Some(view) = self.chats.get_mut(&channel) else {
            return false;
        };
        if !view.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let count = view.suggestions.len();
        if count > 0 {
            match key {
                "up" => view.suggestion = (view.suggestion + count - 1) % count,
                "down" => view.suggestion = (view.suggestion + 1) % count,
                "enter" | "tab" => {
                    let chosen = view.suggestion;
                    self.accept_suggestion(&channel, chosen, window, cx);
                }
                "escape" => view.suggestions.clear(),
                _ => return false,
            }
            return true;
        }
        match key {
            "up" if view.input.read(cx).value().trim().is_empty() => {
                self.edit_last(&channel, window, cx);
                true
            }
            "escape" => {
                self.composer_cancel(&channel, window, cx);
                true
            }
            _ => false,
        }
    }
}
