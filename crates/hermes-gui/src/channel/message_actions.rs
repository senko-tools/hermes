//! What a message's menu entries do.

use super::*;

impl Shell {
    /// Menu actions on messages. `false` if this is not one.
    pub fn run_message_action(
        &mut self,
        target: &Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let core = self.core.clone();
        match (target.clone(), action) {
            (Target::Message(inst, channel, message), Action::CopyText) => {
                let text = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel))
                    .and_then(|w| w.get(message))
                    .map(|m| m.content.clone());
                if let Some(text) = text {
                    self.copy(text, "Message text", cx);
                }
            }
            (Target::Message(_, _, message), Action::CopyMessageId) => {
                self.copy(message.to_string(), "Message ID", cx);
            }
            (Target::Message(inst, channel, message), Action::Save) => {
                let saved = {
                    let store = self.core.store();
                    store.instance(&inst).and_then(|s| {
                        let m = s.window(channel)?.get(message)?;
                        let space = s.space_of(channel);
                        let text = view::plain_text(s, space, &m.content, 4000);
                        Some(hermes_client_core::channel_keys::SavedMessage {
                            channel,
                            message,
                            saved_at: hermes_client_core::now_ms(),
                            author: s.display_name(space, m.author.id),
                            text: if text.is_empty() && !m.attachments.is_empty() {
                                format!("[{} attachment(s)]", m.attachments.len())
                            } else {
                                text
                            },
                        })
                    })
                };
                if saved.is_some_and(|saved| self.core.set_saved(&inst, message, Some(saved))) {
                    self.success("Saved. Find it under Saved messages.", cx);
                } else {
                    self.error("could not save this message here", cx);
                }
            }
            (Target::Message(inst, _, message), Action::Unsave) => {
                self.core.set_saved(&inst, message, None);
                self.success("Removed from saved messages", cx);
            }
            (Target::Message(inst, channel, message), Action::Pin | Action::Unpin) => {
                let pin = action == Action::Pin;
                self.call_status(
                    &inst,
                    if pin { "Pinned" } else { "Unpinned" },
                    move |r| async move {
                        if pin {
                            r.pin(channel, message).await
                        } else {
                            r.unpin(channel, message).await
                        }
                    },
                    cx,
                );
            }
            (Target::Message(inst, channel, message), Action::SelectMessage) => {
                if let Some(view) = self.chats.get_mut(&(inst, channel)) {
                    view.clear_selection();
                    view.selected.insert(message);
                    view.anchor = Some(message);
                }
            }
            (Target::Message(inst, channel, message), Action::MarkUnreadHere) => {
                self.core.mark_unread(&inst, channel, message);
                let key = (inst, channel);
                if let Some(view) = self.chats.get_mut(&key) {
                    // The divider moves there at once.
                    view.read_marker = Some(Id(message.0.saturating_sub(1)));
                    view.unread_jump_done = false;
                    view.divider_seen.set(false);
                }
                self.sync_rows(&key);
                cx.notify();
            }
            (Target::Message(inst, channel, message), Action::Forward) => {
                self.open_forward(inst, channel, message, window, cx);
            }
            (Target::Message(inst, channel, message), Action::CopyLink) => {
                self.copy(
                    format!("hermes://{inst}/channels/{channel}/{message}"),
                    "Message link",
                    cx,
                );
            }
            (Target::Message(inst, channel, message), Action::Report) => {
                let excerpt = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel))
                    .and_then(|w| w.get(message).map(|m| m.content.clone()))
                    .unwrap_or_default();
                self.open_form_report(inst, channel, message, excerpt, window, cx);
            }
            (Target::Messages(inst, channel, messages), Action::DeleteMessage) => {
                if let Some(view) = self.chats.get_mut(&(inst.clone(), channel)) {
                    view.clear_selection();
                }
                self.run_status(cx, async move {
                    let total = messages.len();
                    let mut failed = 0;
                    let mut last_error = None;
                    let rest = core.rest(&inst)?;
                    for ids in messages.chunks(BULK_DELETE_AT_ONCE) {
                        if let Err(err) = rest.bulk_delete(channel, ids.to_vec()).await {
                            failed += ids.len();
                            last_error = Some(err.to_string());
                        }
                    }
                    match last_error {
                        None => Ok(format!(
                            "Deleted {total} message{}",
                            if total == 1 { "" } else { "s" }
                        )),
                        Some(err) => Err(format!(
                            "could not delete {failed} of {total} messages: {err}"
                        )),
                    }
                });
            }
            (Target::Message(inst, channel, message), Action::DeleteMessage) => {
                self.run(
                    cx,
                    async move { core.delete_message(&inst, channel, message).await },
                    |this, result, _, cx| {
                        if let Err(err) = result {
                            this.error(format!("could not delete: {err}"), cx);
                        }
                    },
                );
            }
            (Target::Message(inst, channel, message), Action::Reply) => {
                let key = (inst.clone(), channel);
                self.select_if_hidden(&key, window, cx);
                if let Some(view) = self.chats.get_mut(&key) {
                    view.editing = None;
                    view.reply_to = Some(message);
                }
                self.focus_composer = true;
            }
            (Target::Message(inst, channel, message), Action::AddReaction) => {
                if let Some(view) = self.chats.get_mut(&(inst, channel)) {
                    view.palette = Some(message);
                }
            }
            (Target::Message(inst, channel, message), Action::Edit) => {
                self.start_edit(&(inst, channel), message, window, cx);
            }
            (Target::Message(inst, channel, message), Action::QuoteText) => {
                let quote = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel)?.get(message).cloned())
                    .map(|m| {
                        let mut q: String = m.content.lines().map(|l| format!("> {l}\n")).collect();
                        q.push('\n');
                        q
                    });
                if let Some(quote) = quote {
                    let key = (inst, channel);
                    self.select_if_hidden(&key, window, cx);
                    self.insert_text(&key, &quote, window, cx);
                }
            }
            _ => return false,
        }
        true
    }

    /// Shows a channel unless it already shows.
    pub(super) fn select_if_hidden(
        &mut self,
        key: &ChannelKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_channel().as_ref() != Some(key) {
            self.select(key.0.clone(), key.1, window, cx);
        }
    }

    // ---- drawing ------------------------------------------------------------------------------
}
