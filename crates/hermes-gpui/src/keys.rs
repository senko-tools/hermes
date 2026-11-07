//! Keys that act anywhere, before any element sees them: Escape, the message box's special keys
//! and the keybinds (PLAN.md section 15.6).

use gpui_kit::{Context, Focusable as _, KeystrokeEvent, Window};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::{InstanceId, view};

use crate::keybinds::Action;
use crate::menu_entries::{Action as MenuAction, Target};
use crate::results::Key;
use crate::shell::{Page, Selected, Shell};
use crate::tree::Row;

impl Shell {
    pub fn on_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let plain = !keystroke.modifiers.modified();
        if self.command_bar_key(key, plain) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.composer_key(key, plain, window, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape" && plain && self.close_top() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Shift+F10 or the Menu key: the menu of the open channel, as a right-click would.
        let menu_key = key == "menu" || (key == "f10" && keystroke.modifiers.shift);
        if menu_key && self.menu.is_none() {
            if let Some((inst, channel)) = self.open_channel() {
                let bounds = window.viewport_size();
                let at = gpui_kit::point(bounds.width / 3., bounds.height / 4.);
                self.open_menu(Target::Channel(inst, channel), at, window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.dialog_open() || self.menu.is_some() {
            return;
        }
        // Typing goes to a focused text field; only combos with Ctrl, Alt or Super count there.
        let typing = window.focused(cx).is_some();
        let m = keystroke.modifiers;
        if typing && !(m.control || m.alt || m.platform) {
            return;
        }
        if let Some(action) = self.keybinds.action(keystroke) {
            self.key_action(action, window, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Typing while nothing has the keyboard goes into the open channel's message box, as in
        // other chat apps.
        if !typing && !(m.control || m.alt || m.platform) {
            self.type_into_composer(event, window, cx);
        }
    }

    fn type_into_composer(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_some() || self.open_space_page.is_some() {
            return;
        }
        let Some(text) = event.keystroke.key_char.clone() else {
            return;
        };
        let Some(key) = self.open_channel() else {
            return;
        };
        let can_send = self
            .core
            .store()
            .instance(&key.0)
            .is_some_and(|s| view::can(s, key.1, Permissions::SEND_MESSAGES));
        if can_send {
            self.insert_text(&key, &text, window, cx);
            cx.stop_propagation();
        }
    }

    /// Closes the top dialog, menu or page. `false` if none is open.
    fn close_top(&mut self) -> bool {
        if self.viewer.take().is_some()
            || self.volume_menu.take().is_some()
            || self.share_menu.take().is_some()
            || self.network_open.take().is_some()
            || self.menu.take().is_some()
            || self.command_bar.take().is_some()
            || self.confirm.take().is_some()
            || self.form.take().is_some()
            || self.new_dm.take().is_some()
            || self.add_instance.take().is_some()
        {
            return true;
        }
        if self.open_space_page.take().is_some() {
            return true;
        }
        if let Some(page) = self.settings.take() {
            self.settings_tab = page.tab;
            if page.mic_test {
                self.core.set_mic_test(false);
            }
            self.core.set_level_meter(false);
            return true;
        }
        if matches!(self.page, Page::Voice(..)) && self.enlarged_share.take().is_some() {
            return true;
        }
        if let Some(key) = self.open_channel()
            && let Some(view) = self.chats.get_mut(&key)
            && !view.selected.is_empty()
        {
            view.clear_selection();
            return true;
        }
        false
    }

    /// Whether a dialog covers the window.
    pub fn dialog_open(&self) -> bool {
        self.viewer.is_some()
            || self.confirm.is_some()
            || self.form.is_some()
            || self.new_dm.is_some()
            || self.add_instance.is_some()
            || self.command_bar.is_some()
            || self.volume_menu.is_some()
            || self.share_menu.is_some()
    }

    fn command_bar_key(&mut self, key: &str, plain: bool) -> bool {
        let Some(bar) = &mut self.command_bar else {
            return false;
        };
        if !plain {
            return false;
        }
        match key {
            "up" => bar.move_selection(-1),
            "down" => bar.move_selection(1),
            _ => return false,
        }
        true
    }

    /// The keys the open chat's message box treats specially while it has the keyboard.
    fn composer_key(
        &mut self,
        key: &str,
        plain: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !plain
            || self.dialog_open()
            || self.menu.is_some()
            || self.settings.is_some()
            || self.open_space_page.is_some()
        {
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

    pub fn key_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::CommandBar => self.open_command_bar(window, cx),
            Action::NextChannel => self.step_channel(1, false, window, cx),
            Action::PrevChannel => self.step_channel(-1, false, window, cx),
            Action::NextUnread => self.step_channel(1, true, window, cx),
            Action::PrevUnread => self.step_channel(-1, true, window, cx),
            // Split panes come later.
            Action::FocusNextPane
            | Action::FocusPrevPane
            | Action::SplitRight
            | Action::SplitDown
            | Action::ClosePane
            | Action::MaximizePane => {}
            Action::ToggleMemberList => self.show_members = !self.show_members,
            Action::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            Action::Settings => self.open_settings(window, cx),
            Action::ZoomIn => self.set_ui_scale(self.config.font.ui_scale + 0.1, window, cx),
            Action::ZoomOut => self.set_ui_scale(self.config.font.ui_scale - 0.1, window, cx),
            Action::ZoomReset => self.set_ui_scale(1.0, window, cx),
            Action::MarkRead => {
                if let Some((inst, channel)) = self.open_channel() {
                    self.run_action(
                        Target::Channel(inst, channel),
                        MenuAction::MarkRead,
                        window,
                        cx,
                    );
                }
            }
            Action::Search => match self.open_channel() {
                Some((inst, channel)) => self.search_from(inst, channel, window, cx),
                None => {
                    if let Some(inst) = self.core.instance_ids().first().cloned() {
                        self.open_results(Key::Search(inst, None), window, cx);
                    }
                }
            },
            Action::ToggleMute => self.toggle_mute(cx),
            Action::ToggleDeafen => self.toggle_deafen(cx),
        }
        cx.notify();
    }

    /// Opens the next or previous channel of the sidebar (only unread ones if `unread`), wrapping
    /// around.
    fn step_channel(
        &mut self,
        delta: i32,
        unread: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channels: Vec<(InstanceId, Id, bool)> = self
            .current_tree_rows()
            .into_iter()
            .filter_map(|row| match row {
                Row::Channel {
                    inst, id, badge, ..
                }
                | Row::Dm {
                    inst, id, badge, ..
                } => Some((inst, id, badge.unread || badge.mentions > 0)),
                _ => None,
            })
            .collect();
        if channels.is_empty() {
            return;
        }
        let current = self.open_channel();
        let start =
            current.and_then(|(i, c)| channels.iter().position(|(ci, cc, _)| *ci == i && *cc == c));
        let len = channels.len() as i32;
        for step in 1..=len {
            let index = match start {
                Some(s) => (s as i32 + delta * step).rem_euclid(len),
                None if delta > 0 => step - 1,
                None => len - step,
            } as usize;
            let (inst, id, is_unread) = &channels[index];
            if !unread || *is_unread {
                self.select(inst.clone(), *id, window, cx);
                return;
            }
        }
    }

    /// The rows of what the sidebar lists now.
    fn current_tree_rows(&self) -> Vec<Row> {
        let store = self.core.store();
        let core = &self.core;
        let mut out = Vec::new();
        match &self.selected {
            Selected::Home => {
                for inst in core.instance_ids() {
                    if let Some(s) = store.instance(&inst) {
                        crate::tree::dm_rows(
                            s,
                            &inst,
                            None,
                            &|i, u| core.is_speaking(i, u),
                            &mut out,
                        );
                    }
                }
            }
            Selected::Space(inst, space) => {
                if let Some(s) = store.instance(inst) {
                    crate::tree::space_rows(
                        s,
                        inst,
                        *space,
                        &self.collapsed,
                        None,
                        &|i, u| core.is_speaking(i, u),
                        &mut out,
                    );
                }
            }
        }
        out
    }
}
