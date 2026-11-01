//! Space settings: opening the buffer, its messages, the lists it loads and drag and drop.

use hermes_client_core::proto::api::{
    CreateRoleRequest, OverrideRequest, UpdateMemberRequest, UpdateSpaceRequest,
    VoiceStatePatchRequest,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::rest::Rest;
use hermes_client_core::{InstanceId, view};
use iced::Task;

use super::App;
use crate::buffer::Buffer;
use crate::buffer::space_settings::{self as ss, Message as SsMsg, Remote, Tab, Tri};
use crate::drag::{Item, Kind};
use crate::message::Message;
use crate::sidebar::menu::{Action, Target};

const AUDIT_PAGE: u32 = 50;

impl App {
    /// A REST call with the instance's client; `done` turns the result into a message.
    pub(super) fn call<T, Fut>(
        &self,
        inst: &InstanceId,
        f: impl FnOnce(Rest) -> Fut + Send + 'static,
        done: impl Fn(Result<T, String>) -> Message + Send + 'static,
    ) -> Task<Message>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, hermes_client_core::rest::Error>> + Send + 'static,
    {
        let rest = self.core.rest(inst);
        Task::perform(
            async move {
                match rest {
                    Ok(rest) => f(rest).await.map_err(|e| e.to_string()),
                    Err(err) => Err(err.to_string()),
                }
            },
            done,
        )
    }

    /// A REST call whose only outcome is a status line.
    pub(super) fn call_status<T, Fut>(
        &self,
        inst: &InstanceId,
        ok: impl Into<String>,
        f: impl FnOnce(Rest) -> Fut + Send + 'static,
    ) -> Task<Message>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, hermes_client_core::rest::Error>> + Send + 'static,
    {
        let ok = ok.into();
        self.call(inst, f, move |r| Message::Done(r.map(|_| ok.clone())))
    }

    pub(super) fn open_space_settings(
        &mut self,
        inst: InstanceId,
        space: Id,
        tab: Tab,
    ) -> Task<Message> {
        let state = self
            .space_settings
            .entry((inst.clone(), space))
            .or_default();
        state.tab = tab;
        self.page = Some(Buffer::SpaceSettings {
            inst: inst.clone(),
            space,
        });
        self.load_space_tab(&inst, space, tab, false)
    }

    /// Loads the REST list of `tab` (unless already loaded and not `force`).
    fn load_space_tab(
        &mut self,
        inst: &InstanceId,
        space: Id,
        tab: Tab,
        force: bool,
    ) -> Task<Message> {
        let Some(state) = self.space_settings.get_mut(&(inst.clone(), space)) else {
            return Task::none();
        };
        let wrap = {
            let inst = inst.clone();
            move |m: SsMsg| Message::SpaceSettings(inst.clone(), space, m)
        };
        match tab {
            Tab::Invites => {
                if !force && matches!(state.invites, Remote::Ready(_) | Remote::Loading) {
                    return Task::none();
                }
                let manage = {
                    let store = self.core.store();
                    store.instance(inst).is_some_and(|s| {
                        view::can_in_space(
                            s,
                            space,
                            hermes_client_core::proto::permissions::Permissions::MANAGE_INVITES,
                        )
                    })
                };
                if !manage {
                    return Task::none();
                }
                state.invites = Remote::Loading;
                self.call(
                    inst,
                    move |r| async move { r.invites(space).await },
                    move |r| wrap(SsMsg::Invites(r)),
                )
            }
            Tab::Bans => {
                if !force && matches!(state.bans, Remote::Ready(_) | Remote::Loading) {
                    return Task::none();
                }
                state.bans = Remote::Loading;
                self.call(
                    inst,
                    move |r| async move { r.bans(space).await },
                    move |r| wrap(SsMsg::Bans(r)),
                )
            }
            Tab::Audit => {
                if !force && matches!(state.audit, Remote::Ready(_) | Remote::Loading) {
                    return Task::none();
                }
                state.audit = Remote::Loading;
                self.call(
                    inst,
                    move |r| async move { r.audit_log(space, None, AUDIT_PAGE).await },
                    move |r| wrap(SsMsg::Audit(false, r)),
                )
            }
            _ => Task::none(),
        }
    }

    pub(super) fn on_space_settings(
        &mut self,
        inst: InstanceId,
        space: Id,
        message: SsMsg,
    ) -> Task<Message> {
        let key = (inst.clone(), space);
        self.space_settings.entry(key.clone()).or_default();
        let wrap = {
            let inst = inst.clone();
            move |m: SsMsg| Message::SpaceSettings(inst.clone(), space, m)
        };
        macro_rules! state {
            () => {
                self.space_settings.get_mut(&key).expect("inserted above")
            };
        }
        match message {
            SsMsg::Tab(tab) => {
                state!().tab = tab;
                return self.load_space_tab(&inst, space, tab, false);
            }
            SsMsg::Close => {
                self.page = None;
                self.panes.clear_where(
                    |b| matches!(b, Buffer::SpaceSettings { inst: i, space: s } if *i == inst && *s == space),
                );
            }
            SsMsg::Hover(item) => {
                self.drag.hover(item.map(|(kind, id)| Item {
                    inst: inst.clone(),
                    space,
                    kind,
                    id,
                }));
            }
            SsMsg::Leave((kind, id)) => self.drag.leave(&Item {
                inst: inst.clone(),
                space,
                kind,
                id,
            }),
            SsMsg::Name(name) => state!().name = Some(name),
            SsMsg::Description(text) => state!().description = Some(text),
            SsMsg::ResetOverview => {
                let st = state!();
                st.name = None;
                st.description = None;
            }
            SsMsg::SaveOverview => {
                let st = state!();
                let req = UpdateSpaceRequest {
                    name: st.name.clone().map(|n| n.trim().to_owned()),
                    description: st.description.clone().map(|d| {
                        let d = d.trim().to_owned();
                        (!d.is_empty()).then_some(d)
                    }),
                    icon_id: None,
                };
                return self.call(
                    &inst,
                    move |r| async move { r.update_space(space, &req).await },
                    move |r| wrap(SsMsg::OverviewSaved(r.map(|_| ()))),
                );
            }
            SsMsg::OverviewSaved(result) => match result {
                Ok(()) => {
                    let st = state!();
                    st.name = None;
                    st.description = None;
                    self.info("Space saved".into());
                }
                Err(err) => self.error(format!("could not save: {err}")),
            },
            SsMsg::PickIcon => {
                let inst = inst.clone();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Choose a space icon")
                            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    move |path| Message::IconPicked(inst.clone(), space, path),
                );
            }
            SsMsg::RemoveIcon => {
                let req = UpdateSpaceRequest {
                    icon_id: Some(None),
                    ..UpdateSpaceRequest::default()
                };
                return self.call_status(&inst, "Icon removed", move |r| async move {
                    r.update_space(space, &req).await
                });
            }
            SsMsg::InvitePeople => {
                return self.run_action(Target::Space(inst, space), Action::InvitePeople);
            }
            SsMsg::LeaveSpace => {
                self.confirm = Some((Target::Space(inst, space), Action::LeaveSpace))
            }
            SsMsg::DeleteSpace => {
                self.confirm = Some((Target::Space(inst, space), Action::DeleteSpace));
            }
            SsMsg::CopyId => return self.update(Message::Copy(space.to_string(), "Space ID")),

            SsMsg::SelectRole(role) => {
                let store = self.core.store();
                if let Some(s) = store.instance(&inst) {
                    self.space_settings
                        .get_mut(&key)
                        .expect("inserted above")
                        .select_role(s, space, role);
                }
            }
            SsMsg::CreateRole => {
                let req = CreateRoleRequest {
                    name: "new role".into(),
                    color: None,
                    hoist: None,
                    mentionable: None,
                    permissions: None,
                };
                return self.call(
                    &inst,
                    move |r| async move { r.create_role(space, &req).await },
                    move |r| wrap(SsMsg::RoleCreated(r.map(|role| role.id))),
                );
            }
            SsMsg::RoleCreated(result) => match result {
                Ok(role) => {
                    // The gateway event may still be on its way; select once the store has it.
                    let found = {
                        let store = self.core.store();
                        store
                            .instance(&inst)
                            .and_then(|s| s.spaces.get(&space)?.role(role).cloned())
                    };
                    let st = state!();
                    st.role = Some(match found {
                        Some(r) => ss::RoleDraft::of(&r),
                        None => ss::RoleDraft {
                            id: role,
                            name: "new role".into(),
                            color: String::new(),
                            hoist: false,
                            mentionable: false,
                            permissions: Default::default(),
                        },
                    });
                    self.info("Role created: give it a name and permissions".into());
                }
                Err(err) => self.error(format!("could not create the role: {err}")),
            },
            SsMsg::RoleName(name) => {
                if let Some(d) = &mut state!().role {
                    d.name = name;
                }
            }
            SsMsg::RoleColor(color) => {
                if let Some(d) = &mut state!().role {
                    d.color = color;
                }
            }
            SsMsg::RoleHoist(on) => {
                if let Some(d) = &mut state!().role {
                    d.hoist = on;
                }
            }
            SsMsg::RoleMentionable(on) => {
                if let Some(d) = &mut state!().role {
                    d.mentionable = on;
                }
            }
            SsMsg::RolePermission(flag, on) => {
                if let Some(d) = &mut state!().role {
                    d.permissions.set(flag, on);
                }
            }
            SsMsg::ResetRole => {
                if let Some(id) = state!().role.as_ref().map(|r| r.id) {
                    return self.on_space_settings(inst, space, SsMsg::SelectRole(id));
                }
            }
            SsMsg::SaveRole => {
                let Some(draft) = state!().role.clone() else {
                    return Task::none();
                };
                let role = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.spaces.get(&space)?.role(draft.id).cloned())
                };
                let Some(req) = role.and_then(|r| draft.changes(&r)) else {
                    self.error("the color must be six hex digits, like #3498DB".into());
                    return Task::none();
                };
                let id = draft.id;
                return self.call(
                    &inst,
                    move |r| async move { r.update_role(space, id, &req).await },
                    move |r| wrap(SsMsg::RoleSaved(r)),
                );
            }
            SsMsg::RoleSaved(result) => match result {
                Ok(role) => {
                    state!().role = Some(ss::RoleDraft::of(&role));
                    self.info(format!("Role {} saved", role.name));
                }
                Err(err) => self.error(format!("could not save the role: {err}")),
            },
            SsMsg::DeleteRole(role) => {
                self.confirm = Some((Target::Role(inst, space, role), Action::DeleteRole));
            }
            SsMsg::MoveRole(role, delta) => {
                let moves = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.spaces.get(&space))
                        .map(|st| {
                            let list: Vec<_> =
                                st.roles.iter().filter(|r| !r.is_everyone()).collect();
                            let i = list.iter().position(|r| r.id == role);
                            let j = i.and_then(|i| i.checked_add_signed(delta as isize));
                            match (i, j.and_then(|j| list.get(j))) {
                                (Some(_), Some(onto)) => {
                                    ss::reorder_roles(&st.roles, role, onto.id)
                                }
                                _ => Vec::new(),
                            }
                        })
                        .unwrap_or_default()
                };
                return self.reorder_roles(&inst, space, moves);
            }

            SsMsg::SelectChannel(channel) => {
                let first = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.channel(channel))
                        .and_then(|c| c.overrides.first().map(|o| o.role_id))
                };
                let st = state!();
                st.channel = Some(channel);
                st.override_role = Some(first.unwrap_or(space));
            }
            SsMsg::CreateChannel(parent) => {
                return self.open_form_create_channel(inst, space, parent);
            }
            SsMsg::CreateCategory => return self.open_form_create_category(inst, space),
            SsMsg::EditChannel(channel) => return self.open_form_edit_channel(inst, channel),
            SsMsg::DeleteChannel(channel) => {
                self.confirm = Some((Target::Channel(inst, channel), Action::DeleteChannel));
            }
            SsMsg::MoveChannel(channel, delta) => {
                let moves = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .map(|s| view::step_channel(s, space, channel, delta))
                        .unwrap_or_default()
                };
                return self.reorder_channels(&inst, space, moves);
            }
            SsMsg::OverrideRole(role) => state!().override_role = Some(role),
            SsMsg::SetOverride(flag, tri) => {
                let st = state!();
                let (Some(channel), Some(role)) = (st.channel, st.override_role) else {
                    return Task::none();
                };
                let current = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.channel(channel))
                        .and_then(|c| c.overrides.iter().find(|o| o.role_id == role).copied())
                };
                let (allow, deny) = current.map_or_else(Default::default, |o| (o.allow, o.deny));
                let (allow, deny) = tri.apply(allow, deny, flag);
                if allow.is_empty() && deny.is_empty() {
                    return self.call_status(&inst, "Permission reset", move |r| async move {
                        r.delete_override(channel, role).await
                    });
                }
                let req = OverrideRequest { allow, deny };
                let text = match tri {
                    Tri::Allow => "Allowed",
                    Tri::Deny => "Denied",
                    Tri::Inherit => "Permission reset",
                };
                return self.call_status(&inst, text, move |r| async move {
                    r.set_override(channel, role, &req).await
                });
            }
            SsMsg::ClearOverride => {
                let st = state!();
                if let (Some(channel), Some(role)) = (st.channel, st.override_role) {
                    return self.call_status(&inst, "Override removed", move |r| async move {
                        r.delete_override(channel, role).await
                    });
                }
            }

            SsMsg::MemberQuery(q) => state!().member_query = q,
            SsMsg::SelectMember(user) => state!().member = Some(user),
            SsMsg::MemberRole(user, role, on) => {
                let roles = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.member(space, user))
                        .map(|m| m.role_ids.clone())
                };
                let Some(mut roles) = roles else {
                    return Task::none();
                };
                roles.retain(|r| *r != role);
                if on {
                    roles.push(role);
                }
                let req = UpdateMemberRequest {
                    nickname: None,
                    role_ids: Some(roles),
                };
                return self.call_status(
                    &inst,
                    if on { "Role given" } else { "Role taken away" },
                    move |r| async move { r.update_member(space, Some(user), &req).await },
                );
            }
            SsMsg::Nickname(user) => return self.open_form_nickname(inst, space, user),
            SsMsg::Kick(user) => {
                self.confirm = Some((Target::Member(inst, Some(space), user), Action::Kick));
            }
            SsMsg::Ban(user) => return self.open_form_ban(inst, space, user),
            SsMsg::DmMember(user) => return self.message_user(inst, user),
            SsMsg::ServerMute(user, on) => {
                let req = VoiceStatePatchRequest {
                    server_mute: Some(on),
                    ..VoiceStatePatchRequest::default()
                };
                return self.call_status(
                    &inst,
                    if on { "Server muted" } else { "Server unmuted" },
                    move |r| async move { r.update_voice_state(space, user, &req).await },
                );
            }
            SsMsg::ServerDeafen(user, on) => {
                let req = VoiceStatePatchRequest {
                    server_deaf: Some(on),
                    ..VoiceStatePatchRequest::default()
                };
                return self.call_status(
                    &inst,
                    if on {
                        "Server deafened"
                    } else {
                        "Server undeafened"
                    },
                    move |r| async move { r.update_voice_state(space, user, &req).await },
                );
            }
            SsMsg::Disconnect(user) => {
                let req = VoiceStatePatchRequest {
                    channel_id: Some(None),
                    ..VoiceStatePatchRequest::default()
                };
                return self.call_status(&inst, "Disconnected from voice", move |r| async move {
                    r.update_voice_state(space, user, &req).await
                });
            }
            SsMsg::Transfer(user) => {
                self.confirm = Some((
                    Target::Member(inst, Some(space), user),
                    Action::TransferOwnership,
                ));
            }

            SsMsg::CreateInvite => return self.open_form_invite(inst, space),
            SsMsg::CopyInvite(code) => {
                if let Some(link) = self.core.invite_link(&inst, &code) {
                    return self.update(Message::Copy(link, "Invite link"));
                }
            }
            SsMsg::RevokeInvite(code) => {
                self.confirm = Some((Target::Invite(inst, space, code), Action::RevokeInvite));
            }
            SsMsg::Unban(user) => {
                let reload = wrap(SsMsg::Refresh);
                return self.call(
                    &inst,
                    move |r| async move { r.unban(space, user).await },
                    move |r| match r {
                        Ok(()) => reload.clone(),
                        Err(err) => Message::Done(Err(format!("could not unban: {err}"))),
                    },
                );
            }
            SsMsg::Refresh => {
                let tab = state!().tab;
                return self.load_space_tab(&inst, space, tab, true);
            }
            SsMsg::MoreAudit => {
                let before = state!().audit.ready().and_then(|l| l.last()).map(|e| e.id);
                return self.call(
                    &inst,
                    move |r| async move { r.audit_log(space, before, AUDIT_PAGE).await },
                    move |r| wrap(SsMsg::Audit(true, r)),
                );
            }
            SsMsg::Invites(result) => {
                state!().invites = match result {
                    Ok(list) => Remote::Ready(list),
                    Err(err) => Remote::Failed(err),
                };
            }
            SsMsg::Bans(result) => {
                state!().bans = match result {
                    Ok(list) => Remote::Ready(list),
                    Err(err) => Remote::Failed(err),
                };
            }
            SsMsg::Audit(append, result) => {
                let st = state!();
                match result {
                    Ok(list) => {
                        st.audit_more = list.len() as u32 >= AUDIT_PAGE;
                        match (&mut st.audit, append) {
                            (Remote::Ready(old), true) => old.extend(list),
                            _ => st.audit = Remote::Ready(list),
                        }
                    }
                    Err(err) => st.audit = Remote::Failed(err),
                }
            }
        }
        Task::none()
    }

    pub(super) fn reorder_roles(
        &self,
        inst: &InstanceId,
        space: Id,
        moves: Vec<hermes_client_core::proto::api::RolePosition>,
    ) -> Task<Message> {
        if moves.is_empty() {
            return Task::none();
        }
        self.call(
            inst,
            move |r| async move { r.reorder_roles(space, &moves).await },
            |r| match r {
                Ok(_) => Message::Nothing,
                Err(err) => Message::Done(Err(format!("could not reorder the roles: {err}"))),
            },
        )
    }

    pub(super) fn reorder_channels(
        &self,
        inst: &InstanceId,
        space: Id,
        moves: Vec<hermes_client_core::proto::api::ChannelPosition>,
    ) -> Task<Message> {
        if moves.is_empty() {
            return Task::none();
        }
        self.call(
            inst,
            move |r| async move { r.reorder_channels(space, &moves).await },
            |r| match r {
                Ok(_) => Message::Nothing,
                Err(err) => Message::Done(Err(format!("could not move the channel: {err}"))),
            },
        )
    }

    /// The mouse button went up: drop a dragged role or channel.
    pub(super) fn on_mouse_released(&mut self) -> Task<Message> {
        if let Some(resize) = self.resize.take() {
            if self.panel_width(resize.edge) != resize.start_width {
                self.save_panel_width(resize.edge);
            }
            return Task::none();
        }
        let Some((picked, target)) = self.drag.release() else {
            return Task::none();
        };
        let moves = {
            let store = self.core.store();
            let Some(s) = store.instance(&picked.inst) else {
                return Task::none();
            };
            match picked.kind {
                Kind::Role => {
                    if !view::can_manage_role(s, picked.space, picked.id) {
                        return Task::none();
                    }
                    let roles = s
                        .spaces
                        .get(&picked.space)
                        .map(|st| ss::reorder_roles(&st.roles, picked.id, target.id))
                        .unwrap_or_default();
                    return self.reorder_roles(&picked.inst, picked.space, roles);
                }
                Kind::Channel => {
                    if !view::can_in_space(
                        s,
                        picked.space,
                        hermes_client_core::proto::permissions::Permissions::MANAGE_CHANNELS,
                    ) {
                        return Task::none();
                    }
                    view::move_channel(s, picked.space, picked.id, target.id)
                }
            }
        };
        self.reorder_channels(&picked.inst, picked.space, moves)
    }
}
