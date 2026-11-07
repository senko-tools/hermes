//! What end-to-end encryption shows in the window (docs/e2ee.md): the dialog with a conversation's
//! safety number, the warning about someone's new device, and the marks on messages.

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, div};
use hermes_client_core::proto::e2ee::MessageLock;
use hermes_client_core::proto::id::Id;
use hermes_client_core::secure::ConversationKeys;

use crate::assets::{self, icon};
use crate::modal::{dialog, error_line, heading, paragraph};
use crate::shell::Shell;
use crate::theme::{self, px};
use hermes_client_core::InstanceId;

/// The open safety number dialog.
pub struct KeysDialog {
    inst: InstanceId,
    channel: Id,
    keys: Option<Result<ConversationKeys, String>>,
}

/// The small mark next to a message's time, in a conversation that is encrypted.
pub fn lock_mark(lock: MessageLock, private: bool) -> Option<Div> {
    let (name, color) = match lock {
        MessageLock::Sealed => (assets::LOCK, theme::text_tertiary()),
        MessageLock::NotForThisDevice | MessageLock::Tampered => (assets::LOCK, theme::warning()),
        // A message in a DM that is not encrypted: from before encryption, or a server's forgery.
        MessageLock::Plain if private => (assets::LOCK_OPEN, theme::warning()),
        MessageLock::Plain => return None,
    };
    Some(div().flex_shrink_0().child(icon(name, 12., color)))
}

impl Shell {
    /// Opens the safety number of a conversation; asks the server for the latest keys.
    pub fn open_keys_dialog(&mut self, inst: InstanceId, channel: Id, cx: &mut Context<Self>) {
        self.keys_dialog = Some(KeysDialog {
            inst: inst.clone(),
            channel,
            keys: None,
        });
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.conversation_keys(&inst, channel)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, _, cx| {
                if let Some(open) = &mut this.keys_dialog
                    && open.channel == channel
                {
                    open.keys = Some(result);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub fn render_keys_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.keys_dialog.as_ref()?;
        let name_of = |user: Id| {
            let store = self.core.store();
            store
                .instance(&open.inst)
                .map_or_else(|| user.to_string(), |s| s.display_name(None, user))
        };
        let details: AnyElement = match &open.keys {
            None => paragraph("Asking for the latest keys…").into_any_element(),
            Some(Err(err)) => error_line(err.clone()).into_any_element(),
            Some(Ok(keys)) => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .p(px(12.))
                        .rounded(px(6.))
                        .bg(theme::raised())
                        .font_family(theme::mono_font())
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(theme::body_size())
                        .child(keys.safety_number.clone()),
                )
                .children(keys.devices.iter().map(|(user, devices)| {
                    let list = devices
                        .iter()
                        .map(|d| d.chars().take(8).collect::<String>())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let devices_word = if devices.len() == 1 {
                        "device"
                    } else {
                        "devices"
                    };
                    div()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child(format!(
                            "{}: {} {devices_word} ({list})",
                            name_of(*user),
                            devices.len()
                        ))
                }))
                .into_any_element(),
        };
        Some(dialog(
            "keys-dialog",
            480.,
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(heading("End-to-end encrypted"))
                .child(paragraph(
                    "Messages, files and calls here are encrypted on your device and can only be \
                     read on the devices of the people in this conversation, not by the server.",
                ))
                .child(paragraph(
                    "To be sure nobody slipped their own keys in, compare this number with them \
                     some other way, like in person. It should be the same on all your screens.",
                ))
                .child(details)
                .child(
                    div().flex().justify_end().child(
                        Button::new("keys-close")
                            .cursor_pointer()
                            .large()
                            .label("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.keys_dialog = None;
                                cx.notify();
                            })),
                    ),
                ),
        ))
    }

    /// Over the message box of a conversation in which someone has a device I do not know yet.
    pub fn render_new_devices(
        &self,
        inst: &InstanceId,
        channel: Id,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let users = self.core.new_devices(inst, channel);
        if users.is_empty() {
            return None;
        }
        let names = {
            let store = self.core.store();
            let s = store.instance(inst)?;
            let me = s.me_id();
            users
                .iter()
                .map(|u| {
                    if Some(*u) == me {
                        "You".to_owned()
                    } else {
                        s.display_name(None, *u)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (compare_inst, ok_inst) = (inst.clone(), inst.clone());
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .mx(px(16.))
                .mb(px(6.))
                .p(px(8.))
                .rounded(px(6.))
                .bg(theme::raised())
                .border_1()
                .border_color(theme::warning())
                .text_size(theme::secondary_size())
                .child(icon(assets::LOCK, 16., theme::warning()))
                .child(div().flex_1().min_w(px(200.)).child(format!(
                    "New device: {names}. Messages from now on can be read there too. If that is not \
                     expected, someone may be listening in."
                )))
                .child(
                    Button::new("new-device-compare")
                        .cursor_pointer()
                        .label("Compare numbers")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_keys_dialog(compare_inst.clone(), channel, cx);
                        })),
                )
                .child(
                    Button::new("new-device-ok")
                        .cursor_pointer()
                        .primary()
                        .label("OK")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.core.acknowledge_devices(&ok_inst, channel);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}
