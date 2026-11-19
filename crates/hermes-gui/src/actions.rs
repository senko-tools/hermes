//! What the menu entries do.

use std::future::Future;

use gpui_kit::{Context, Window};
use hermes_client_core::proto::api::{
    CreateInviteRequest, NotificationSettingRequest, TransferSpaceRequest,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{NotificationLevel, NotificationSetting, PresenceStatus};
use hermes_client_core::rest::Rest;
use hermes_client_core::{CoreError, InstanceId};

use crate::menu_entries::{Action, Target};
use crate::shell::{Page, Shell};

impl Shell {
    /// A menu entry was chosen: destructive actions ask first.
    pub fn menu_action(
        &mut self,
        target: Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_later("menu", |s| s.menu = None, cx);
        if action.is_destructive() {
            self.cancel_closing("confirm");
            self.confirm = Some((target, action));
            cx.notify();
        } else {
            self.run_action(target, action, window, cx);
        }
    }

    /// Runs a REST call and reports `ok` or the error on the status line.
    pub fn call_status<T, F>(
        &self,
        inst: &InstanceId,
        ok: &str,
        call: impl FnOnce(Rest) -> F + Send + 'static,
        cx: &mut Context<Self>,
    ) where
        F: Future<Output = Result<T, hermes_client_core::rest::Error>> + Send,
        T: Send + 'static,
    {
        let core = self.core.clone();
        let inst = inst.clone();
        let ok = ok.to_owned();
        self.run_status(cx, async move {
            let rest = core.rest(&inst)?;
            call(rest).await?;
            Ok(ok)
        });
    }

    pub fn run_action(
        &mut self,
        target: Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let core = self.core.clone();
        match (target, action) {
            (Target::Instance(inst), Action::Reconnect) => {
                let result = core
                    .set_online(&inst, false)
                    .and_then(|()| core.set_online(&inst, true));
                if let Err(err) = result {
                    self.error(err.to_string(), cx);
                }
            }
            (Target::Instance(inst), Action::GoOffline | Action::GoOnline) => {
                if let Err(err) = core.set_online(&inst, action == Action::GoOnline) {
                    self.error(err.to_string(), cx);
                }
            }
            (Target::Instance(inst), Action::LogOut) => {
                self.run_status(cx, async move {
                    core.logout(&inst)
                        .await
                        .map(|()| format!("Logged out of {inst}"))
                        .map_err(|e| e.to_string())
                });
            }
            (Target::Instance(inst), Action::RemoveInstance) => {
                self.leave_instance(&inst, cx);
                self.run_status(cx, async move {
                    core.remove_instance(&inst)
                        .await
                        .map(|()| format!("Removed {inst}"))
                        .map_err(|e| e.to_string())
                });
            }
            (Target::Instance(inst), Action::CreateSpace) => {
                self.open_form_create_space(inst, window, cx);
            }
            (Target::Instance(inst) | Target::Me(Some(inst)), Action::EditProfile) => {
                self.open_form_profile(inst, window, cx);
            }

            (Target::Channel(inst, channel), Action::JoinVoice) => {
                self.join_voice(inst, channel, cx);
            }
            (Target::Me(_), Action::OpenSettings) => self.open_settings(window, cx),
            (Target::Me(_), a) if a.status().is_some() => {
                core.set_presence(a.status().unwrap_or(PresenceStatus::Online));
            }
            (Target::Me(Some(inst)), Action::CopyUsername) => {
                let name = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.me.as_ref().map(|m| m.user.handle()));
                if let Some(name) = name {
                    self.copy(name, "Username", cx);
                }
            }

            (Target::Space(inst, space), Action::InvitePeople) => {
                self.run(
                    cx,
                    async move {
                        let invite = core
                            .rest(&inst)?
                            .create_invite(space, &CreateInviteRequest::default())
                            .await?;
                        Ok::<_, CoreError>(core.invite_link(&inst, &invite.code))
                    },
                    |this, result, _, cx| match result {
                        Ok(Some(link)) => this.copy(link, "Invite link", cx),
                        Ok(None) => this.error("no such instance", cx),
                        Err(err) => this.error(format!("could not create an invite: {err}"), cx),
                    },
                );
            }
            (Target::Space(_, space), Action::CopySpaceId) => {
                self.copy(space.to_string(), "Server ID", cx);
            }
            (Target::Space(inst, space), Action::LeaveSpace) => {
                self.leave_space(&inst, space, cx);
                self.call_status(
                    &inst,
                    "Left the server",
                    move |r| async move { r.leave_space(space).await },
                    cx,
                );
            }
            (Target::Space(inst, space), Action::DeleteSpace) => {
                self.leave_space(&inst, space, cx);
                self.call_status(
                    &inst,
                    "Server deleted",
                    move |r| async move { r.delete_space(space).await },
                    cx,
                );
            }
            (Target::Space(inst, space), Action::MarkSpaceRead) => {
                let unread: Vec<(Id, Id)> = {
                    let store = core.store();
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
                    core.mark_read(&inst, channel, last);
                }
            }
            (Target::Space(inst, space), Action::CreateChannel) => {
                self.open_form_create_channel(inst, space, None, window, cx);
            }
            (Target::Space(inst, space), Action::CreateCategory) => {
                self.open_form_create_category(inst, space, window, cx);
            }
            (
                Target::Space(inst, id) | Target::Channel(inst, id),
                a @ (Action::NotifyAll
                | Action::NotifyMentions
                | Action::NotifyNothing
                | Action::NotifyDefault
                | Action::MuteFor15Minutes
                | Action::MuteForHour
                | Action::MuteFor8Hours
                | Action::MuteForDay
                | Action::Mute
                | Action::Unmute),
            ) => self.set_notifications(inst, id, a, cx),

            (Target::Channel(inst, channel), Action::MarkRead) => {
                let last = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.channel(channel))
                    .and_then(|c| c.last_message_id);
                if let Some(last) = last {
                    core.mark_read(&inst, channel, last);
                }
            }
            (Target::Channel(_, channel), Action::CopyChannelId) => {
                self.copy(channel.to_string(), "Channel ID", cx);
            }
            (Target::Channel(inst, channel), Action::EditChannel | Action::RenameGroup) => {
                self.open_form_edit_channel(inst, channel, window, cx);
            }
            (Target::Channel(inst, category), Action::CreateChannel) => {
                // A category's menu: the new channel goes inside it.
                let space = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(category));
                if let Some(space) = space {
                    self.open_form_create_channel(inst, space, Some(category), window, cx);
                }
            }
            (Target::Channel(inst, channel), Action::DeleteChannel | Action::LeaveGroup) => {
                self.leave_channel(&inst, channel, cx);
                let ok = if action == Action::LeaveGroup {
                    "Left the group"
                } else {
                    "Channel deleted"
                };
                self.call_status(
                    &inst,
                    ok,
                    move |r| async move { r.delete_channel(channel).await },
                    cx,
                );
            }
            (Target::Channel(inst, channel), Action::AcceptRequest) => {
                self.call_status(
                    &inst,
                    "Request accepted",
                    move |r| async move { r.accept_dm(channel).await },
                    cx,
                );
            }
            (Target::Channel(inst, channel), Action::CloseDm) => {
                self.leave_channel(&inst, channel, cx);
                self.call_status(
                    &inst,
                    "Conversation closed",
                    move |r| async move { r.close_dm(channel).await },
                    cx,
                );
            }

            (Target::Space(inst, space), Action::MoveServerUp) => {
                self.move_server(&inst, space, true, cx);
            }
            (Target::Space(inst, space), Action::MoveServerDown) => {
                self.move_server(&inst, space, false, cx);
            }
            (Target::Member(inst, _, user), Action::EditNote) => {
                self.open_form_note(inst, user, window, cx);
            }
            (Target::Member(inst, _, user), Action::EditLocalName) => {
                self.open_form_local_name(inst, user, window, cx);
            }
            (Target::Member(inst, _, user), Action::Block) => {
                let core = self.core.clone();
                self.run_status(cx, async move {
                    let rest = core.rest(&inst)?;
                    // Someone blocked is no longer a friend either.
                    let _ = rest.remove_friend(user).await;
                    rest.block(user).await?;
                    core.set_blocked(&inst, user, true);
                    Ok("Blocked".to_owned())
                });
            }
            (Target::Member(inst, _, user), Action::Unblock) => {
                let core = self.core.clone();
                self.run_status(cx, async move {
                    let rest = core.rest(&inst)?;
                    rest.unblock(user).await?;
                    core.set_blocked(&inst, user, false);
                    Ok("Unblocked".to_owned())
                });
            }
            (Target::Member(inst, _, user), Action::AddFriend) => {
                let handle = {
                    let store = core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.user(user))
                        .map(hermes_client_core::proto::models::User::handle)
                };
                if let Some(handle) = handle {
                    self.call_status(
                        &inst,
                        "Friend request sent",
                        move |r| async move { r.add_friend(&handle).await.map(|_| ()) },
                        cx,
                    );
                }
            }
            (Target::Member(inst, _, user), Action::AcceptFriend) => {
                self.call_status(
                    &inst,
                    "Friend request accepted",
                    move |r| async move { r.accept_friend(user).await.map(|_| ()) },
                    cx,
                );
            }
            (
                Target::Member(inst, _, user),
                Action::DeclineFriend | Action::CancelFriendRequest | Action::RemoveFriend,
            ) => {
                let done = match action {
                    Action::DeclineFriend => "Friend request declined",
                    Action::CancelFriendRequest => "Friend request taken back",
                    _ => "Friend removed",
                };
                self.call_status(
                    &inst,
                    done,
                    move |r| async move { r.remove_friend(user).await },
                    cx,
                );
            }
            (Target::Member(inst, _, user), Action::GiveNoTag | Action::TakeNoTag) => {
                let tagless = action == Action::GiveNoTag;
                self.call_status(
                    &inst,
                    if tagless {
                        "They now go without a # number"
                    } else {
                        "They now have a # number"
                    },
                    move |r| async move {
                        r.admin_update_user(
                            user,
                            &hermes_client_core::proto::api::AdminUserPatch {
                                tagless: Some(tagless),
                                ..Default::default()
                            },
                        )
                        .await
                    },
                    cx,
                );
            }
            (Target::Member(inst, _, user), Action::SendMessage) => {
                self.message_user(inst, user, window, cx);
            }
            (Target::Member(inst, _, user), Action::CopyUsername) => {
                let name = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.user(user).map(|u| u.handle()));
                if let Some(name) = name {
                    self.copy(name, "Username", cx);
                }
            }
            (Target::Member(_, _, user), Action::CopyUserId) => {
                self.copy(user.to_string(), "User ID", cx);
            }
            (Target::Member(inst, Some(space), user), Action::ChangeNickname) => {
                self.open_form_nickname(inst, space, user, window, cx);
            }
            (
                Target::Member(inst, Some(space), user),
                action @ (Action::Timeout10Minutes
                | Action::Timeout1Hour
                | Action::Timeout1Day
                | Action::EndTimeout),
            ) => {
                let (seconds, done) = match action {
                    Action::Timeout10Minutes => (10 * 60, "Timed out for 10 minutes"),
                    Action::Timeout1Hour => (60 * 60, "Timed out for 1 hour"),
                    Action::Timeout1Day => (24 * 60 * 60, "Timed out for 1 day"),
                    _ => (0, "Timeout ended"),
                };
                self.call_status(
                    &inst,
                    done,
                    move |r| async move { r.timeout(space, user, seconds).await },
                    cx,
                );
            }
            (Target::Member(inst, Some(space), user), Action::Kick) => {
                self.call_status(
                    &inst,
                    "Kicked",
                    move |r| async move { r.kick(space, user).await },
                    cx,
                );
            }
            (Target::Member(inst, Some(space), user), Action::Ban) => {
                self.open_form_ban(inst, space, user, window, cx);
            }
            (Target::Member(inst, Some(space), user), Action::TransferOwnership) => {
                let req = TransferSpaceRequest { user_id: user };
                self.call_status(
                    &inst,
                    "Ownership transferred",
                    move |r| async move { r.transfer_space(space, req.user_id).await },
                    cx,
                );
            }

            (target, action) => self.run_later_action(target, action, window, cx),
        }
        cx.notify();
    }

    /// Actions of the later steps.
    fn run_later_action(
        &mut self,
        target: Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.run_message_action(&target, action, window, cx)
            || self.run_space_action(&target, action, window, cx)
            || self.run_member_action(&target, action, window, cx)
        {
            return;
        }
        tracing::warn!("menu action {action:?} does not apply to {target:?}");
    }

    /// Leaves whatever shows this instance.
    fn leave_instance(&mut self, inst: &str, cx: &mut Context<Self>) {
        let on_page = matches!(&self.page,
            Page::Chat(i, _) | Page::Voice(i, _) if i == inst);
        let selected = matches!(&self.selected, crate::shell::Selected::Space(i, _) if i == inst);
        if on_page || selected {
            self.open_friends(cx);
        }
    }

    fn leave_space(&mut self, inst: &str, space: Id, cx: &mut Context<Self>) {
        if self.selected == crate::shell::Selected::Space(inst.to_owned(), space) {
            self.open_friends(cx);
        }
    }

    fn leave_channel(&mut self, inst: &str, channel: Id, cx: &mut Context<Self>) {
        let open = matches!(&self.page,
            Page::Chat(i, c) | Page::Voice(i, c) if i == inst && *c == channel);
        if open {
            self.open_friends(cx);
        }
    }

    /// Notification level or mute of a space or channel (PLAN.md section 14.5).
    fn set_notifications(
        &mut self,
        inst: InstanceId,
        target: Id,
        action: Action,
        cx: &mut Context<Self>,
    ) {
        let req = {
            let store = self.core.store();
            notification_change(
                store.instance(&inst),
                target,
                action,
                hermes_client_core::now_ms(),
            )
        };
        let text = match action {
            Action::Mute => "Muted",
            a if a.mute_minutes().is_some() => "Muted for a while",
            Action::Unmute => "Unmuted",
            _ => "Notifications changed",
        };
        match req {
            Some((level, muted_until)) => {
                let req = NotificationSettingRequest { level, muted_until };
                self.call_status(
                    &inst,
                    text,
                    move |r| async move { r.set_notification_setting(target, &req).await },
                    cx,
                );
            }
            None => self.call_status(
                &inst,
                text,
                move |r| async move { r.delete_notification_setting(target).await },
                cx,
            ),
        }
    }

    /// Opens the 1:1 DM with `user`, creating it if needed.
    pub fn message_user(
        &mut self,
        inst: InstanceId,
        user: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.core.store().instance(&inst).and_then(|s| {
            s.dms
                .values()
                .find(|dm| {
                    dm.kind == hermes_client_core::proto::models::ChannelKind::Dm
                        && s.dm_partner(dm).is_some_and(|u| u.id == user)
                })
                .map(|dm| dm.id)
        });
        if let Some(dm) = existing {
            self.select(inst, dm, window, cx);
            return;
        }
        let core = self.core.clone();
        let target = inst.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&target)?;
                rest.create_dm(vec![user])
                    .await
                    .map(|c| c.id)
                    .map_err(|e| e.to_string())
            },
            move |this, result, window, cx| match result {
                Ok(channel) => this.select(inst, channel, window, cx),
                Err(err) => this.error(err, cx),
            },
        );
    }
}

/// The notification setting `action` gives `target` (a server, channel or DM): its level and
/// until when it is muted, or `None` to go back to the default. A mute keeps the level, and a
/// level keeps the mute; `now` is in milliseconds.
fn notification_change(
    s: Option<&hermes_client_core::store::InstanceStore>,
    target: Id,
    action: Action,
    now: i64,
) -> Option<(NotificationLevel, Option<i64>)> {
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
    let level = current.map_or(fallback, |c| c.level);
    let muted_until = current.and_then(|c| c.muted_until);
    match action {
        Action::NotifyAll => Some((NotificationLevel::All, muted_until)),
        Action::NotifyMentions => Some((NotificationLevel::Mentions, muted_until)),
        Action::NotifyNothing => Some((NotificationLevel::None, muted_until)),
        Action::Mute => Some((level, Some(NotificationSetting::MUTED_FOREVER))),
        a if a.mute_minutes().is_some() => {
            Some((level, a.mute_minutes().map(|m| now + m * 60_000)))
        }
        Action::Unmute => Some((level, None)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_client_core::testing::fixtures;

    #[test]
    fn a_mute_keeps_the_level_each_kind_of_channel_falls_back_to() {
        let store = fixtures::store();
        let s = Some(&store);
        let forever = Some(NotificationSetting::MUTED_FOREVER);
        // A server: mentions. A DM: everything. A server's channel: what its server has.
        assert_eq!(
            notification_change(s, Id(10), Action::Mute, 0),
            Some((NotificationLevel::Mentions, forever))
        );
        assert_eq!(
            notification_change(s, Id(20), Action::Mute, 0),
            Some((NotificationLevel::All, forever))
        );
        assert_eq!(
            notification_change(s, Id(11), Action::Mute, 0),
            Some((NotificationLevel::Mentions, forever))
        );
        assert_eq!(
            notification_change(s, Id(11), Action::NotifyNothing, 0),
            Some((NotificationLevel::None, None))
        );
    }

    #[test]
    fn a_level_keeps_the_mute_and_a_timed_mute_ends_later() {
        let mut store = fixtures::store();
        store.notification_settings.insert(
            Id(11),
            NotificationSetting {
                target_id: Id(11),
                level: NotificationLevel::All,
                muted_until: Some(5_000),
            },
        );
        let s = Some(&store);
        assert_eq!(
            notification_change(s, Id(11), Action::NotifyNothing, 0),
            Some((NotificationLevel::None, Some(5_000)))
        );
        assert_eq!(
            notification_change(s, Id(11), Action::Unmute, 0),
            Some((NotificationLevel::All, None))
        );
        assert_eq!(
            notification_change(s, Id(11), Action::MuteForHour, 1_000),
            Some((NotificationLevel::All, Some(1_000 + 60 * 60_000)))
        );
    }
}
