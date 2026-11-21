//! Chats that are not end-to-end encrypted say so. Encrypted is the normal case and shows
//! nothing; an unencrypted chat has an amber note above its composer for as long as it is open,
//! and opening one may show a popup (`privacy.warn_unencrypted`: every time, the first time for
//! each chat, or never). The popup's choices and the setting are the same config value.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div};
use hermes_client_core::InstanceId;
use hermes_client_core::config::WarnUnencrypted;
use hermes_client_core::proto::id::Id;

use crate::assets::{self, icon};
use crate::shell::Shell;
use crate::theme::{self, px};

/// The choices of `privacy.warn_unencrypted`: (value, label).
pub const WARN_CHOICES: [(WarnUnencrypted, &str); 3] = [
    (WarnUnencrypted::Every, "Every time I open one"),
    (WarnUnencrypted::First, "First time for each chat"),
    (WarnUnencrypted::Never, "Never"),
];

/// How many chats the "first time" list remembers; the oldest are forgotten.
const WARNED_MAX: usize = 500;

/// The popup's id for `close_later`.
const POPUP: &str = "unencrypted-popup";

/// How a chat is named in `privacy.warned_unencrypted`.
fn warned_key(inst: &str, channel: Id) -> String {
    format!("{inst}/{channel}")
}

/// Whether a popup is due for a chat, given the setting and the chats already warned about.
pub fn popup_due(setting: WarnUnencrypted, warned: &[String], key: &str) -> bool {
    match setting {
        WarnUnencrypted::Every => true,
        WarnUnencrypted::Never => false,
        WarnUnencrypted::First => !warned.iter().any(|w| w == key),
    }
}

impl Shell {
    /// Whether the chat holds messages the server can read.
    pub fn chat_unencrypted(&self, inst: &InstanceId, channel: Id) -> bool {
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return false;
        };
        s.channel(channel)
            .is_some_and(|c| c.kind.has_messages() && !s.channel_encrypted(channel))
    }

    /// A chat was opened: shows the popup when the setting asks for it.
    pub fn on_chat_opened(&mut self, inst: &InstanceId, channel: Id, cx: &mut Context<Self>) {
        if self.unencrypted_popup.as_ref() != Some(&(inst.clone(), channel)) {
            self.unencrypted_popup = None;
        }
        if !self.chat_unencrypted(inst, channel) {
            return;
        }
        let privacy = self.config.privacy.clone();
        let key = warned_key(inst, channel);
        if !popup_due(privacy.warn_unencrypted, &privacy.warned_unencrypted, &key) {
            return;
        }
        self.cancel_closing(POPUP);
        self.unencrypted_popup = Some((inst.clone(), channel));
        if privacy.warn_unencrypted == WarnUnencrypted::First {
            let mut warned = privacy.warned_unencrypted;
            warned.push(key);
            let over = warned.len().saturating_sub(WARNED_MAX);
            warned.drain(..over);
            self.set_config(|c| c.privacy.warned_unencrypted = warned, cx);
        }
        cx.notify();
    }

    /// Sets `privacy.warn_unencrypted`, from the popup or from settings.
    pub fn set_warn_unencrypted(&mut self, value: WarnUnencrypted, cx: &mut Context<Self>) {
        self.set_config(|c| c.privacy.warn_unencrypted = value, cx);
    }

    /// The choices as a row of chips; `id` keeps the popup's apart from the setting's.
    pub fn warn_unencrypted_choices(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let current = self.config.privacy.warn_unencrypted;
        crate::ui::segments()
            .when(id == "unencrypted-popup-choice", |d| d.justify_start())
            .children(
                WARN_CHOICES
                    .into_iter()
                    .enumerate()
                    .map(|(n, (value, label))| {
                        crate::ui::segment(self, (id, n), label, value == current, cx).on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_warn_unencrypted(value, cx);
                            }),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The amber note above an unencrypted chat's composer. Always there while it is open.
    pub fn render_unencrypted_bar(&self, inst: &InstanceId, channel: Id) -> Option<AnyElement> {
        if !self.chat_unencrypted(inst, channel) {
            return None;
        }
        let amber = theme::warning();
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .mx(px(16.))
                .mb(px(4.))
                .px(px(10.))
                .py(px(5.))
                .rounded(theme::row_radius())
                .bg(theme::alpha(amber, 0.12))
                .border_1()
                .border_color(theme::alpha(amber, 0.35))
                .text_size(theme::caption_size())
                .text_color(amber)
                .child(icon(assets::LOCK_OPEN, 14., amber))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child("Not end-to-end encrypted — the server can read messages here."),
                )
                .into_any_element(),
        )
    }

    /// The popup over a chat that was just opened and is not encrypted: centred near the top.
    pub fn render_unencrypted_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use gpui_kit::{Animation, AnimationExt as _, ease_out_quint};
        let (inst, channel) = self.unencrypted_popup.as_ref()?;
        if self.open_channel() != Some((inst.clone(), *channel)) {
            return None;
        }
        let amber = theme::warning();
        let close = |this: &mut Shell, cx: &mut Context<Shell>| {
            this.close_later(POPUP, |s| s.unencrypted_popup = None, cx);
        };
        let way = if crate::modal::closing(POPUP) {
            "out"
        } else {
            "in"
        };
        let card = div()
            .id(POPUP)
            .occlude()
            .flex()
            .flex_col()
            .gap(px(10.))
            .w(px(380.))
            .p(px(16.))
            .rounded(theme::radius_large())
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .shadow_lg()
            .font_family(theme::font())
            .text_color(theme::text())
            .text_size(theme::secondary_size())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(assets::LOCK_OPEN, 18., amber))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(theme::body_size())
                            .child("This chat isn't end-to-end encrypted"),
                    ),
            )
            .child(
                div()
                    .text_color(theme::text_secondary())
                    .child("Messages here are stored so the server can read them."),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        Button::new("unencrypted-ok")
                            .cursor_pointer()
                            .primary()
                            .label("Got it")
                            .on_click(cx.listener(move |this, _, _, cx| close(this, cx))),
                    )
                    .child(
                        div()
                            .id("unencrypted-never")
                            .cursor_pointer()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .hover(|d| d.text_color(theme::accent()).underline())
                            .child("Don't show again")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_warn_unencrypted(WarnUnencrypted::Never, cx);
                                close(this, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child("Show this:"),
                    )
                    .child(self.warn_unencrypted_choices("unencrypted-popup-choice", cx)),
            );
        Some(
            gpui_kit::deferred(
                div()
                    .absolute()
                    .top(px(72.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(div().relative().child(card).with_animation(
                        SharedString::from(format!("{POPUP}-{way}")),
                        Animation::new(crate::modal::pop()).with_easing(ease_out_quint()),
                        move |d, t| {
                            let t = if way == "out" { 1. - t } else { t };
                            d.opacity(t).top(px(-6. * (1. - t)))
                        },
                    )),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_popup_shows_as_the_setting_says() {
        let warned = vec!["a/1".to_owned()];
        assert!(popup_due(WarnUnencrypted::Every, &warned, "a/1"));
        assert!(!popup_due(WarnUnencrypted::Never, &[], "a/2"));
        assert!(popup_due(WarnUnencrypted::First, &warned, "a/2"));
        assert!(!popup_due(WarnUnencrypted::First, &warned, "a/1"));
    }
}
