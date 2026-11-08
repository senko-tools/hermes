//! Dialogs over the whole window. The kit's dialogs rebuild their content from a closure that holds
//! no state of ours, so these draw from the shell's state like everything else.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, Context, Div, Pixels, Point, SharedString, Size, anchored, deferred, div,
};

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

/// Which corner of a popup sits at the click: it opens away from the window's nearest edges, so
/// it lies beside what was clicked instead of being pushed back by an edge.
fn popup_anchor(at: Point<Pixels>, viewport: Size<Pixels>) -> Anchor {
    let below = at.y > viewport.height / 2.;
    let right = at.x > viewport.width / 2.;
    match (below, right) {
        (false, false) => Anchor::TopLeft,
        (false, true) => Anchor::TopRight,
        (true, false) => Anchor::BottomLeft,
        (true, true) => Anchor::BottomRight,
    }
}

/// A popup opened by a click at `at` (the share menu, the network details, a profile): `card`
/// opens from there towards the middle of the window, fading in as it slides out a little.
pub fn popup(
    id: &'static str,
    at: Point<Pixels>,
    viewport: Size<Pixels>,
    card: impl IntoElement + 'static,
) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _, ease_out_quint};
    let anchor = popup_anchor(at, viewport);
    // It slides away from the click, in the direction it opens.
    let away = match anchor {
        Anchor::BottomLeft | Anchor::BottomRight => 1.,
        _ => -1.,
    };
    // A gap from the click, so the button that opened it stays visible.
    let gap = px(18.) * away;
    deferred(
        anchored()
            .anchor(anchor)
            .position(at)
            .offset(gpui_kit::point(px(0.), -gap))
            .snap_to_window_with_margin(px(8.))
            .child(div().relative().child(card).with_animation(
                id,
                Animation::new(std::time::Duration::from_millis(150)).with_easing(ease_out_quint()),
                move |d, t| d.opacity(t).top(px(6. * away * (1. - t))),
            )),
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
        out.extend(self.render_keys_dialog(cx));
        out.extend(self.render_viewer(cx));
        out.extend(self.render_network(cx));
        out.extend(self.render_volume_menu(cx));
        out.extend(self.render_profile(cx));
        out.extend(self.render_share_menu(cx));
        out.extend(self.render_command_bar(cx));
        out.extend(self.render_crop(cx));
        out.extend(self.render_video_full(cx));
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
                    .map_or("This server", |st| st.space.name.as_str())
            ),
            (Target::Space(_, space), _) => format!(
                "You leave {} and need an invite to come back.",
                s.and_then(|s| s.spaces.get(space))
                    .map_or("this server", |st| st.space.name.as_str())
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
                "{} is removed from the server. They can come back with an invite.",
                s.map(|s| s.display_name(*space, *user)).unwrap_or_default()
            ),
            (Target::Member(_, space, user), _) => format!(
                "{} becomes the owner of this server, and you no longer are.",
                s.map(|s| s.display_name(*space, *user)).unwrap_or_default()
            ),
            (Target::Role(..), _) => "This role is deleted and taken from everyone.".into(),
            (Target::Invite(..), _) => "This invite link stops working.".into(),
            _ => action.label().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::popup_anchor;
    use gpui_kit::{Anchor, point, px, size};

    #[test]
    fn popups_open_towards_the_middle() {
        let window = size(px(1000.), px(800.));
        assert_eq!(
            popup_anchor(point(px(50.), px(750.)), window),
            Anchor::BottomLeft
        );
        assert_eq!(
            popup_anchor(point(px(950.), px(100.)), window),
            Anchor::TopRight
        );
        assert_eq!(
            popup_anchor(point(px(950.), px(700.)), window),
            Anchor::BottomRight
        );
        assert_eq!(
            popup_anchor(point(px(10.), px(10.)), window),
            Anchor::TopLeft
        );
    }
}
