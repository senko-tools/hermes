//! Menu actions beyond the basic ones in `App::run_action`: space and channel management,
//! notification levels, message and member actions.

use hermes_client_core::InstanceId;
use hermes_client_core::proto::api::{NotificationSettingRequest, TransferSpaceRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{NotificationLevel, NotificationSetting};
use iced::Task;
use iced::widget::operation;
use iced::widget::pane_grid::Axis;

use super::App;
use crate::buffer::Buffer;
use crate::buffer::results::Key;
use crate::buffer::space_settings::{self as ss, Tab};
use crate::message::Message;
use crate::sidebar::menu::{Action, Target};
use crate::voice::VoiceMsg;

impl App {
    pub(super) fn run_more_action(&mut self, target: Target, action: Action) -> Task<Message> {
        match (target, action) {
            (Target::Instance(inst), Action::CreateSpace) => self.open_form_create_space(inst),
            (
                Target::Me(_),
                a @ (Action::SetOnline | Action::SetIdle | Action::SetDnd | Action::SetInvisible),
            ) => {
                use hermes_client_core::proto::models::PresenceStatus as P;
                let status = match a {
                    Action::SetIdle => P::Idle,
                    Action::SetDnd => P::Dnd,
                    Action::SetInvisible => P::Invisible,
                    _ => P::Online,
                };
                self.core.set_presence(status);
                Task::none()
            }
            (Target::Me(Some(inst)), Action::EditProfile) => self.open_form_profile(inst),
            (Target::Me(Some(inst)), Action::CopyUsername) => {
                let name = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.me.as_ref().map(|m| m.user.username.clone()));
                match name {
                    Some(name) => self.update(Message::Copy(name, "Username")),
                    None => Task::none(),
                }
            }
            (Target::Me(_), Action::OpenSettings) => self.update(Message::OpenSettings),
            (Target::Me(_), Action::ShowMentions) => {
                self.update(Message::OpenResults(Key::Mentions))
            }
            (Target::Instance(inst), Action::EditProfile) => self.open_form_profile(inst),

            (Target::Space(inst, space), Action::SpaceSettings) => {
                self.open_space_settings(inst, space, Tab::Overview)
            }
            (Target::Space(inst, space), Action::CreateChannel) => {
                self.open_form_create_channel(inst, space, None)
            }
            (Target::Space(inst, space), Action::CreateCategory) => {
                self.open_form_create_category(inst, space)
            }
            (Target::Space(inst, space), Action::MarkSpaceRead) => {
                let unread: Vec<(Id, Id)> = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.spaces.get(&space).map(|st| (s, st)))
                        .map(|(s, st)| {
                            st.channels
                                .values()
                                .filter(|c| s.unread(c.id) || s.mention_count(c.id) > 0)
                                .filter_map(|c| Some((c.id, c.last_message_id?)))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                for (channel, last) in unread {
                    self.core.mark_read(&inst, channel, last);
                }
                Task::none()
            }
            (Target::Space(inst, space), Action::DeleteSpace) => {
                if matches!(&self.page, Some(Buffer::SpaceSettings { inst: i, space: s }) if *i == inst && *s == space)
                {
                    self.page = None;
                }
                self.panes.clear_where(|b| match b {
                    Buffer::SpaceSettings { inst: i, space: s } => *i == inst && *s == space,
                    _ => false,
                });
                self.call_status(&inst, "Space deleted", move |r| async move {
                    r.delete_space(space).await
                })
            }
            (
                Target::Space(inst, id) | Target::Channel(inst, id),
                a @ (Action::NotifyAll
                | Action::NotifyMentions
                | Action::NotifyNothing
                | Action::NotifyDefault
                | Action::Mute
                | Action::Unmute),
            ) => self.set_notifications(inst, id, a),

            (Target::Channel(inst, channel), Action::OpenInNewPane) => {
                self.panes.split(self.panes.focus, Axis::Vertical);
                self.select(inst, channel)
            }
            (Target::Space(inst, space), Action::SearchSpace) => {
                self.update(Message::OpenResults(Key::Search(inst, Some(space))))
            }
            (Target::Channel(inst, channel), Action::ShowPins) => {
                self.update(Message::OpenResults(Key::Pins(inst, channel)))
            }
            (Target::Channel(inst, channel), Action::JoinVoice) => {
                self.on_voice(VoiceMsg::Join(inst, channel))
            }
            (Target::Channel(inst, channel), Action::EditChannel | Action::RenameGroup) => {
                self.open_form_edit_channel(inst, channel)
            }
            (Target::Channel(inst, channel), Action::ChannelPermissions) => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                let Some(space) = space else {
                    return Task::none();
                };
                let open = self.open_space_settings(inst.clone(), space, Tab::Channels);
                let select =
                    self.on_space_settings(inst, space, ss::Message::SelectChannel(channel));
                open.chain(select)
            }
            (Target::Channel(inst, space_or_category), Action::CreateChannel) => {
                // A category's menu: the new channel goes inside it.
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(space_or_category));
                match space {
                    Some(space) => {
                        self.open_form_create_channel(inst, space, Some(space_or_category))
                    }
                    None => Task::none(),
                }
            }
            (Target::Channel(inst, channel), Action::DeleteChannel | Action::LeaveGroup) => {
                self.panes.clear_where(
                    |b| matches!(b, Buffer::Channel { inst: i, channel: c } if *i == inst && *c == channel),
                );
                self.call_status(
                    &inst,
                    if action == Action::LeaveGroup {
                        "Left the group"
                    } else {
                        "Channel deleted"
                    },
                    move |r| async move { r.delete_channel(channel).await },
                )
            }

            (Target::Channel(inst, channel), Action::AcceptRequest) => {
                self.call_status(&inst, "Request accepted", move |r| async move {
                    r.accept_dm(channel).await
                })
            }
            (Target::Channel(inst, channel), Action::CloseDm) => {
                self.panes.clear_where(
                    |b| matches!(b, Buffer::Channel { inst: i, channel: c } if *i == inst && *c == channel),
                );
                self.call_status(&inst, "Conversation closed", move |r| async move {
                    r.close_dm(channel).await
                })
            }

            (Target::Message(inst, channel, message), Action::Reply) => {
                let composer = self.composers.entry((inst.clone(), channel)).or_default();
                if composer.editing.is_some() {
                    composer.clear();
                }
                composer.reply_to = Some(message);
                let focus = operation::focus(composer.editor_id.clone());
                self.show_channel(inst, channel).chain(focus)
            }
            (Target::Message(inst, channel, message), Action::AddReaction) => {
                for (_, pane) in self.panes.state.iter_mut() {
                    if pane.buffer
                        == (Buffer::Channel {
                            inst: inst.clone(),
                            channel,
                        })
                    {
                        pane.channel.palette = Some(message);
                    }
                }
                Task::none()
            }
            (Target::Message(inst, channel, message), Action::Edit) => {
                self.start_edit(inst, channel, message)
            }
            (Target::Message(inst, channel, message), Action::QuoteText) => {
                let quote = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.window(channel)?.get(message).cloned())
                        .map(|m| {
                            let mut q: String =
                                m.content.lines().map(|l| format!("> {l}\n")).collect();
                            q.push('\n');
                            q
                        })
                };
                match quote {
                    Some(q) => {
                        let focus = self.insert_text(&inst, channel, &q);
                        self.show_channel(inst, channel).chain(focus)
                    }
                    None => Task::none(),
                }
            }

            (Target::Member(inst, _, user), Action::SendMessage) => self.message_user(inst, user),
            (Target::Member(inst, _, user), Action::Mention) => {
                let username = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.user(user).map(|u| u.username.clone()));
                let target = self.panes.focused_channel().filter(|(i, _)| *i == inst);
                match (username, target) {
                    (Some(name), Some((inst, channel))) => {
                        self.insert_text(&inst, channel, &format!("@{name} "))
                    }
                    _ => {
                        self.info("Open a channel first to mention someone".into());
                        Task::none()
                    }
                }
            }
            (Target::Member(inst, _, user), Action::CopyUsername) => {
                let name = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.user(user).map(|u| u.username.clone()));
                match name {
                    Some(name) => self.update(Message::Copy(name, "Username")),
                    None => Task::none(),
                }
            }
            (Target::Member(_, _, user), Action::CopyUserId) => {
                self.update(Message::Copy(user.to_string(), "User ID"))
            }
            (Target::Member(inst, Some(space), user), Action::ChangeNickname) => {
                self.open_form_nickname(inst, space, user)
            }
            (Target::Member(inst, Some(space), user), Action::ManageRoles) => {
                let open = self.open_space_settings(inst.clone(), space, Tab::Members);
                let select = self.on_space_settings(inst, space, ss::Message::SelectMember(user));
                open.chain(select)
            }
            (Target::Member(inst, Some(space), user), Action::Kick) => {
                self.call_status(&inst, "Kicked", move |r| async move {
                    r.kick(space, user).await
                })
            }
            (Target::Member(inst, Some(space), user), Action::Ban) => {
                self.open_form_ban(inst, space, user)
            }
            (Target::Member(inst, Some(space), user), Action::TransferOwnership) => {
                let req = TransferSpaceRequest { user_id: user };
                self.call_status(&inst, "Ownership transferred", move |r| async move {
                    r.transfer_space(space, req.user_id).await
                })
            }

            (Target::Role(inst, space, role), Action::EditRole) => {
                let open = self.open_space_settings(inst.clone(), space, Tab::Roles);
                let select = self.on_space_settings(inst, space, ss::Message::SelectRole(role));
                open.chain(select)
            }
            (Target::Role(inst, space, role), Action::DeleteRole) => {
                if let Some(st) = self.space_settings.get_mut(&(inst.clone(), space))
                    && st.role.as_ref().is_some_and(|r| r.id == role)
                {
                    st.role = None;
                }
                self.call_status(&inst, "Role deleted", move |r| async move {
                    r.delete_role(space, role).await
                })
            }
            (Target::Invite(inst, space, code), Action::CopyInviteLink) => {
                self.on_space_settings(inst, space, ss::Message::CopyInvite(code))
            }
            (Target::Invite(inst, space, code), Action::RevokeInvite) => {
                let reload = Message::SpaceSettings(inst.clone(), space, ss::Message::Refresh);
                self.call(
                    &inst,
                    move |r| async move { r.delete_invite(&code).await },
                    move |r| match r {
                        Ok(()) => reload.clone(),
                        Err(err) => Message::Done(Err(format!("could not revoke: {err}"))),
                    },
                )
            }
            (target, action) => {
                tracing::warn!("menu action {action:?} does not apply to {target:?}");
                Task::none()
            }
        }
    }

    /// Shows the channel in the focused pane unless some pane shows it already.
    fn show_channel(&mut self, inst: InstanceId, channel: Id) -> Task<Message> {
        let shown = self
            .panes
            .visible_channels()
            .contains(&(inst.clone(), channel));
        if shown {
            Task::none()
        } else {
            self.select(inst, channel)
        }
    }

    /// Inserts text at the cursor of a channel's input and focuses it.
    fn insert_text(&mut self, inst: &InstanceId, channel: Id, text: &str) -> Task<Message> {
        let composer = self.composers.entry((inst.clone(), channel)).or_default();
        composer
            .content
            .perform(iced::widget::text_editor::Action::Edit(
                iced::widget::text_editor::Edit::Paste(std::sync::Arc::new(text.to_owned())),
            ));
        let focus = operation::focus(composer.editor_id.clone());
        self.refresh_suggestions(inst, channel);
        focus
    }

    /// Notification level or mute of a space or channel (PLAN.md section 14.5).
    fn set_notifications(&mut self, inst: InstanceId, target: Id, action: Action) -> Task<Message> {
        let (current, fallback) = {
            let store = self.core.store();
            let s = store.instance(&inst);
            let current = s.and_then(|s| s.notification_settings.get(&target).copied());
            let fallback = s.map_or(NotificationLevel::Mentions, |s| {
                if s.spaces.contains_key(&target) {
                    NotificationLevel::Mentions
                } else if s.dms.contains_key(&target) {
                    NotificationLevel::All
                } else {
                    // A space channel follows its space.
                    s.space_of(target)
                        .and_then(|sp| s.notification_settings.get(&sp).map(|n| n.level))
                        .unwrap_or(NotificationLevel::Mentions)
                }
            });
            (current, fallback)
        };
        let level = current.map_or(fallback, |c| c.level);
        let muted_until = current.and_then(|c| c.muted_until);
        let req = match action {
            Action::NotifyAll => Some((NotificationLevel::All, muted_until)),
            Action::NotifyMentions => Some((NotificationLevel::Mentions, muted_until)),
            Action::NotifyNothing => Some((NotificationLevel::None, muted_until)),
            Action::Mute => Some((level, Some(NotificationSetting::MUTED_FOREVER))),
            Action::Unmute => Some((level, None)),
            _ => None,
        };
        let text = match action {
            Action::Mute => "Muted",
            Action::Unmute => "Unmuted",
            _ => "Notifications changed",
        };
        match req {
            Some((level, muted_until)) => {
                let req = NotificationSettingRequest { level, muted_until };
                self.call_status(&inst, text, move |r| async move {
                    r.set_notification_setting(target, &req).await
                })
            }
            None => self.call_status(&inst, text, move |r| async move {
                r.delete_notification_setting(target).await
            }),
        }
    }
}
