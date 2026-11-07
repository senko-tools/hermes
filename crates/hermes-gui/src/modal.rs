//! Dialogs over the whole window. The kit's dialogs rebuild their content from a closure that holds
//! no state of ours, so these draw from the shell's state like everything else.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, SharedString, deferred, div};

use crate::menu_entries::{Action, Target};
use crate::shell::Shell;
use crate::theme::{self, px};

/// A heading for a dialog.
pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::heading_size())
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(text.into())
}

/// A paragraph for a dialog, in the secondary colour.
pub fn paragraph(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::secondary_size())
        .text_color(theme::text_secondary())
        .child(text.into())
}

/// An error line for a dialog.
pub fn error_line(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::secondary_size())
        .text_color(theme::danger())
        .child(text.into())
}

/// The dimmed backdrop with `card` in the middle. `id` names the dialog.
pub fn dialog(id: &'static str, width: f32, card: impl IntoElement) -> AnyElement {
    deferred(
        div()
            .id(id)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::scrim())
            // The dialog is modal: nothing behind it reacts.
            .occlude()
            .child(
                div()
                    .id("dialog-card")
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .w(px(width))
                    .max_h_full()
                    .overflow_y_scroll()
                    .p(px(20.))
                    .rounded(theme::radius_large())
                    .bg(theme::background())
                    .border_1()
                    .border_color(theme::border())
                    .text_color(theme::text())
                    .font_family(theme::font())
                    .child(card),
            ),
    )
    .with_priority(gpui_kit::base::POPUP_PRIORITY)
    .into_any_element()
}

impl Shell {
    pub fn render_modals(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        out.extend(self.render_add_instance(cx));
        out.extend(self.render_form(cx));
        out.extend(self.render_new_dm(cx));
        out.extend(self.render_viewer(cx));
        out.extend(self.render_network(cx));
        out.extend(self.render_volume_menu(cx));
        out.extend(self.render_share_menu(cx));
        out.extend(self.render_command_bar(cx));
        out.extend(self.render_confirm(cx));
        out
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, action) = self.confirm.as_ref()?;
        let text = self.confirm_text(target, *action);
        let cancel = Button::new("confirm-cancel")
            .cursor_pointer()
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                this.confirm = None;
                cx.notify();
            }));
        let ok = Button::new("confirm-ok")
            .cursor_pointer()
            .label(action.label())
            .danger()
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some((target, action)) = this.confirm.take() {
                    this.run_action(target, action, window, cx);
                }
                cx.notify();
            }));
        Some(dialog(
            "confirm",
            420.,
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(heading("Are you sure?"))
                .child(paragraph(text))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(cancel.large())
                        .child(ok.large()),
                ),
        ))
    }

    /// What a destructive action does, in words.
    fn confirm_text(&self, target: &Target, action: Action) -> String {
        let store = self.core.store();
        let s = target.inst().and_then(|i| store.instance(i));
        match (target, action) {
            (Target::Instance(inst), _) => {
                format!("{inst} is removed from this client, with its saved sign-in.")
            }
            (Target::Space(_, space), Action::DeleteSpace) => format!(
                "{} and everything in it are deleted for everyone. This cannot be undone.",
                s.and_then(|s| s.spaces.get(space))
                    .map_or("This space", |st| st.space.name.as_str())
            ),
            (Target::Space(_, space), _) => format!(
                "You leave {} and need an invite to come back.",
                s.and_then(|s| s.spaces.get(space))
                    .map_or("this space", |st| st.space.name.as_str())
            ),
            (Target::Channel(_, channel), Action::LeaveGroup) => format!(
                "You leave {} and stop receiving its messages.",
                s.and_then(|s| s
                    .channel(*channel)
                    .map(|c| hermes_client_core::view::dm_name(s, c)))
                    .unwrap_or_else(|| "this group".into())
            ),
            (Target::Channel(_, channel), _) => format!(
                "{} and its messages are deleted for everyone. This cannot be undone.",
                s.and_then(|s| s
                    .channel(*channel)
                    .map(|c| hermes_client_core::view::channel_title(s, c)))
                    .unwrap_or_else(|| "This channel".into())
            ),
            (Target::Message(..), _) => "This message is deleted for everyone.".into(),
            (Target::Messages(_, _, messages), _) => {
                format!("{} messages are deleted for everyone.", messages.len())
            }
            (Target::Member(_, space, user), Action::Kick) => format!(
                "{} is removed from the space. They can come back with an invite.",
                s.map(|s| s.display_name(*space, *user)).unwrap_or_default()
            ),
            (Target::Member(_, space, user), _) => format!(
                "{} becomes the owner of this space, and you no longer are.",
                s.map(|s| s.display_name(*space, *user)).unwrap_or_default()
            ),
            (Target::Role(..), _) => "This role is deleted and taken from everyone.".into(),
            (Target::Invite(..), _) => "This invite link stops working.".into(),
            _ => action.label().to_owned(),
        }
    }
}
