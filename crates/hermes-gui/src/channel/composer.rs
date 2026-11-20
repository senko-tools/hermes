//! The message box: suggestions, editing, sending and slash commands.

use super::*;

impl Shell {
    pub(super) fn composer_changed(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        let Some(view) = self.chats.get(key) else {
            return;
        };
        let text = view.input.read(cx).value().to_string();
        if !text.trim().is_empty() && !text.starts_with('/') && view.editing.is_none() {
            self.core.typing(&key.0, key.1);
        }
        self.refresh_suggestions(key, cx);
    }

    /// Autocomplete for the token at the cursor.
    pub(super) fn refresh_suggestions(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        let Some(view) = self.chats.get(key) else {
            return;
        };
        let (text, cursor) = {
            let input = view.input.read(cx);
            (input.value().to_string(), input.cursor())
        };
        let suggestions = match token_before(&text, cursor) {
            Some((_, token)) => self
                .core
                .store()
                .instance(&key.0)
                .map(|s| view::complete(s, key.1, &token, MAX_SUGGESTIONS))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        if suggestions != view.suggestions {
            view.suggestion = 0;
        }
        view.suggestions = suggestions;
        cx.notify();
    }

    /// Replaces the token before the cursor with suggestion `n`.
    pub fn accept_suggestion(
        &mut self,
        key: &ChannelKey,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        let Some(insert) = view.suggestions.get(n).map(|s| s.insert.clone()) else {
            return;
        };
        view.suggestions.clear();
        let input = view.input.clone();
        input.update(cx, |input, cx| {
            let text = input.value().to_string();
            if let Some((start, _)) = token_before(&text, input.cursor()) {
                let end = input.cursor();
                let new = format!("{}{insert}{}", &text[..start], &text[end..]);
                let cursor = start + insert.len();
                input.set_value(new, window, cx);
                let position = input.text().offset_to_position(cursor);
                input.set_cursor_position(position, window, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn composer_enter(
        &mut self,
        key: &ChannelKey,
        shift: bool,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let send = if self.config.buffer.enter_sends {
            !shift
        } else {
            shift || secondary
        };
        if send {
            self.submit(key.clone(), window, cx);
        } else if let Some(view) = self.chats.get(key) {
            view.input
                .update(cx, |input, cx| input.insert("\n", window, cx));
        }
    }

    pub(super) fn set_composer_text(
        &mut self,
        key: &ChannelKey,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.chats.get(key) {
            let text = text.to_owned();
            view.input.update(cx, |input, cx| {
                let end = text.len();
                input.set_value(text, window, cx);
                let position = input.text().offset_to_position(end);
                input.set_cursor_position(position, window, cx);
            });
        }
    }

    /// Inserts text at the cursor of a channel's message box and gives it the keyboard.
    pub fn insert_text(
        &mut self,
        key: &ChannelKey,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.chats.get(key) {
            let text = text.to_owned();
            view.input.update(cx, |input, cx| {
                input.insert(text, window, cx);
                input.focus(window, cx);
            });
        }
    }

    pub fn start_edit(
        &mut self,
        key: &ChannelKey,
        message: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Mentions are edited as typed: `@bob`, not `<@3>`.
        let content = {
            let store = self.core.store();
            store.instance(&key.0).and_then(|s| {
                let content = &s.window(key.1)?.get(message)?.content;
                Some(view::readable_mentions(s, key.1, content))
            })
        };
        let Some(content) = content else { return };
        if let Some(view) = self.chats.get_mut(key) {
            view.reply_to = None;
            view.suggestions.clear();
            view.editing = Some(message);
        }
        self.set_composer_text(key, &content, window, cx);
        cx.notify();
    }

    /// Up in an empty box: edit my last message.
    pub fn edit_last(&mut self, key: &ChannelKey, window: &mut Window, cx: &mut Context<Self>) {
        let last = {
            let store = self.core.store();
            let s = store.instance(&key.0);
            let me = s.and_then(|s| s.me_id());
            s.and_then(|s| s.window(key.1)).and_then(|w| {
                w.messages
                    .iter()
                    .rev()
                    .find(|m| Some(m.author.id) == me && !m.deleted)
                    .map(|m| m.id)
            })
        };
        if let Some(id) = last {
            self.start_edit(key, id, window, cx);
        }
    }

    /// Escape in the message box: closes the popup, else leaves reply/edit mode, else marks read.
    pub fn composer_cancel(
        &mut self,
        key: &ChannelKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        if !view.suggestions.is_empty() {
            view.suggestions.clear();
        } else if view.editing.is_some() {
            view.editing = None;
            self.set_composer_text(key, "", window, cx);
        } else if view.reply_to.is_some() {
            view.reply_to = None;
        } else {
            self.run_action(
                Target::Channel(key.0.clone(), key.1),
                Action::MarkRead,
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// Sends the draft (or saves the edit, or runs a slash command).
    pub(super) fn submit(&mut self, key: ChannelKey, window: &mut Window, cx: &mut Context<Self>) {
        let (inst, channel) = key.clone();
        let Some(view) = self.chats.get(&key) else {
            return;
        };
        let text = view.input.read(cx).value().trim_end().to_owned();
        // `@bob` and `#general` as typed become real mentions.
        let resolve = |this: &Self, text: String| {
            let store = this.core.store();
            match store.instance(&inst) {
                Some(s) => view::resolve_mentions(s, channel, &text),
                None => text,
            }
        };
        let too_long = |this: &Self, text: &str| {
            let store = this.core.store();
            store.instance(&inst).and_then(|s| s.message_refusal(text))
        };
        if let Some(message) = view.editing {
            let text = resolve(self, text);
            if text.trim().is_empty() {
                self.error("A message cannot be empty; delete it instead.", cx);
                return;
            }
            if let Some(refusal) = too_long(self, &text) {
                self.info(refusal, cx);
                return;
            }
            if let Some(view) = self.chats.get_mut(&key) {
                view.clear_mode();
            }
            self.set_composer_text(&key, "", window, cx);
            let core = self.core.clone();
            self.run(
                cx,
                async move { core.edit_message(&inst, channel, message, &text).await },
                |this, result, _, cx| {
                    if let Err(err) = result {
                        this.error(format!("could not edit: {err}"), cx);
                    }
                },
            );
            return;
        }
        let attachments = match view.ready_attachments() {
            Ok(a) => a,
            Err(err) => {
                self.info(err, cx);
                return;
            }
        };
        if text.trim().is_empty() && attachments.is_empty() {
            return;
        }
        let command = if text.trim().is_empty() {
            SlashCommand::Send(String::new())
        } else {
            match commands::parse(&text) {
                Ok(c) => c,
                Err(err) => {
                    self.error(err.to_string(), cx);
                    return;
                }
            }
        };
        if let Some(refusal) = command
            .message_text()
            .and_then(|content| too_long(self, &resolve(self, content)))
        {
            self.info(refusal, cx);
            return;
        }
        let reply_to = view.reply_to;
        if let Some(view) = self.chats.get_mut(&key) {
            view.clear_mode();
        }
        self.set_composer_text(&key, "", window, cx);
        if let Some(content) = command.message_text() {
            let content = resolve(self, content);
            let draft = Draft {
                content,
                reply_to,
                attachments,
            };
            if let Err(err) = self.core.send_message(&inst, channel, draft) {
                self.error(err.to_string(), cx);
            }
            self.sync_rows(&key);
            // Sending glides to the newest message.
            if let Some(view) = self.chats.get(&key) {
                crate::smooth_scroll::glide_to_end(view.list.clone(), window, cx);
            }
            return;
        }
        self.run_slash_command(inst, channel, command, window, cx);
    }

    pub(super) fn run_slash_command(
        &mut self,
        inst: InstanceId,
        channel: Id,
        command: SlashCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let space = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.space_of(channel));
        match command {
            SlashCommand::Send(_) | SlashCommand::Me(_) => {}
            SlashCommand::Join(name) => {
                let found = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| view::find_voice_channel(s, Some(channel), &name));
                match found {
                    Some(room) => self.join_voice(inst, room, cx),
                    None => self.error(format!("No voice channel {name:?} here"), cx),
                }
            }
            SlashCommand::Leave => self.leave_voice(cx),
            SlashCommand::Mute => self.toggle_mute(cx),
            SlashCommand::Deafen => self.toggle_deafen(cx),
            SlashCommand::Status(status) => {
                self.core.set_presence(status);
                self.info(format!("Status: {status:?}"), cx);
            }
            SlashCommand::Nick(nick) => {
                let Some(space) = space else {
                    self.error("Nicknames are per server; open a server channel.", cx);
                    return;
                };
                let core = self.core.clone();
                self.run_status(cx, async move {
                    let req = hermes_client_core::proto::api::UpdateMemberRequest {
                        nickname: Some(nick),
                        role_ids: None,
                    };
                    let rest = core.rest(&inst)?;
                    rest.update_member(space, None, &req).await?;
                    Ok("Nickname changed".to_owned())
                });
            }
            SlashCommand::Dm(name) => {
                let wanted = name.trim().trim_start_matches('@').to_lowercase();
                let user = self.core.store().instance(&inst).and_then(|s| {
                    let me = s.me_id();
                    s.users
                        .values()
                        .filter(|u| Some(u.id) != me)
                        .find(|u| u.username == wanted)
                        .or_else(|| {
                            s.users
                                .values()
                                .filter(|u| Some(u.id) != me)
                                .find(|u| u.name().to_lowercase() == wanted)
                        })
                        .map(|u| u.id)
                });
                match user {
                    Some(user) => self.message_user(inst, user, window, cx),
                    None => self.error(format!("No user {name:?} known here"), cx),
                }
            }
            SlashCommand::Invite => match space {
                Some(space) => {
                    self.run_action(Target::Space(inst, space), Action::InvitePeople, window, cx);
                }
                None => self.error("Invites are for servers; open a server channel.", cx),
            },
        }
    }

    // ---- files --------------------------------------------------------------------------------
}
