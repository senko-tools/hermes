//! Context menu entries of the sidebar, the pane headers, messages and members (PLAN.md section
//! 15.1). Which entries show depends on what I may do, so a menu never offers something the
//! server would refuse.

use hermes_client_core::InstanceId;
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{ChannelKind, NotificationLevel, PresenceStatus};
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::view;
use iced::widget::pane_grid;

/// What was right-clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Instance(InstanceId),
    Space(InstanceId, Id),
    /// A space channel, a category, a DM or a group DM.
    Channel(InstanceId, Id),
    /// A pane header's `⋮`.
    Pane(pane_grid::Pane),
    /// A message's `⋯`: instance, channel, message.
    Message(InstanceId, Id, Id),
    /// Messages selected together in a channel (deleted at once): instance, channel, messages.
    Messages(InstanceId, Id, Vec<Id>),
    /// A person in a member list, a voice room or a message: instance, space (`None` in a DM),
    /// user.
    Member(InstanceId, Option<Id>, Id),
    /// A role in the space settings: instance, space, role.
    Role(InstanceId, Id, Id),
    /// An invite in the space settings: instance, space, code.
    Invite(InstanceId, Id, String),
    /// My own entry in the user panel (`None`: no instance yet).
    Me(Option<InstanceId>),
}

impl Target {
    pub fn inst(&self) -> Option<&InstanceId> {
        match self {
            Target::Instance(inst)
            | Target::Space(inst, _)
            | Target::Channel(inst, _)
            | Target::Message(inst, ..)
            | Target::Messages(inst, ..)
            | Target::Member(inst, ..)
            | Target::Role(inst, ..)
            | Target::Invite(inst, ..)
            | Target::Me(Some(inst)) => Some(inst),
            Target::Pane(_) | Target::Me(None) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // Instance
    Reconnect,
    GoOffline,
    GoOnline,
    CreateSpace,
    EditProfile,
    LogOut,
    RemoveInstance,
    // Space
    SpaceSettings,
    CreateChannel,
    CreateCategory,
    InvitePeople,
    MarkSpaceRead,
    SearchSpace,
    CopySpaceId,
    LeaveSpace,
    DeleteSpace,
    // Notifications (space or channel)
    NotifyAll,
    NotifyMentions,
    NotifyNothing,
    NotifyDefault,
    Mute,
    Unmute,
    // Channel
    MarkRead,
    OpenInNewPane,
    ShowPins,
    JoinVoice,
    EditChannel,
    ChannelPermissions,
    CopyChannelId,
    DeleteChannel,
    RenameGroup,
    LeaveGroup,
    /// Hides a 1:1 DM until a message or call reopens it.
    CloseDm,
    /// Turns a message request into a normal DM.
    AcceptRequest,
    // Pane
    SplitRight,
    SplitDown,
    MaximizePane,
    ToggleMembers,
    ClosePane,
    // Message
    Reply,
    /// Opens the quick reactions of a message.
    AddReaction,
    Edit,
    QuoteText,
    CopyText,
    CopyMessageId,
    Pin,
    Unpin,
    DeleteMessage,
    /// Starts selecting messages (Ctrl+click and Shift+click add more).
    SelectMessage,
    // Member
    SendMessage,
    Mention,
    CopyUsername,
    CopyUserId,
    ChangeNickname,
    ManageRoles,
    Kick,
    Ban,
    TransferOwnership,
    // Me
    SetOnline,
    SetIdle,
    SetDnd,
    SetInvisible,
    OpenSettings,
    ShowMentions,
    // Role / invite
    EditRole,
    DeleteRole,
    CopyInviteLink,
    RevokeInvite,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Reconnect => "Reconnect",
            Action::GoOffline => "Go offline",
            Action::GoOnline => "Go online",
            Action::CreateSpace => "Create a space…",
            Action::EditProfile => "Edit profile…",
            Action::LogOut => "Log out",
            Action::RemoveInstance => "Remove instance",
            Action::SpaceSettings => "Space settings",
            Action::CreateChannel => "Create channel…",
            Action::CreateCategory => "Create category…",
            Action::InvitePeople => "Invite people (copy link)",
            Action::MarkSpaceRead => "Mark all as read",
            Action::SearchSpace => "Search in this space",
            Action::CopySpaceId => "Copy space ID",
            Action::LeaveSpace => "Leave space",
            Action::DeleteSpace => "Delete space",
            Action::NotifyAll => "Notify: all messages",
            Action::NotifyMentions => "Notify: only @mentions",
            Action::NotifyNothing => "Notify: nothing",
            Action::NotifyDefault => "Notify: like the space",
            Action::Mute => "Mute",
            Action::Unmute => "Unmute",
            Action::MarkRead => "Mark as read",
            Action::OpenInNewPane => "Open in a new pane",
            Action::ShowPins => "Pinned messages",
            Action::JoinVoice => "Join voice",
            Action::EditChannel => "Edit channel…",
            Action::ChannelPermissions => "Permissions…",
            Action::CopyChannelId => "Copy channel ID",
            Action::DeleteChannel => "Delete channel",
            Action::RenameGroup => "Rename group…",
            Action::LeaveGroup => "Leave group",
            Action::CloseDm => "Close conversation",
            Action::AcceptRequest => "Accept request",
            Action::SplitRight => "Split right",
            Action::SplitDown => "Split down",
            Action::MaximizePane => "Maximize / restore",
            Action::ToggleMembers => "Toggle member list",
            Action::ClosePane => "Close pane",
            Action::Reply => "Reply",
            Action::AddReaction => "Add reaction",
            Action::Edit => "Edit",
            Action::QuoteText => "Quote",
            Action::CopyText => "Copy text",
            Action::CopyMessageId => "Copy message ID",
            Action::Pin => "Pin message",
            Action::Unpin => "Unpin message",
            Action::DeleteMessage => "Delete message",
            Action::SelectMessage => "Select",
            Action::SendMessage => "Message",
            Action::Mention => "Mention",
            Action::CopyUsername => "Copy username",
            Action::CopyUserId => "Copy user ID",
            Action::ChangeNickname => "Change nickname…",
            Action::ManageRoles => "Roles…",
            Action::Kick => "Kick",
            Action::Ban => "Ban…",
            Action::TransferOwnership => "Transfer ownership",
            Action::SetOnline => "Online",
            Action::SetIdle => "Idle",
            Action::SetDnd => "Do not disturb",
            Action::SetInvisible => "Invisible",
            Action::OpenSettings => "Settings",
            Action::ShowMentions => "Mentions",
            Action::EditRole => "Edit role",
            Action::DeleteRole => "Delete role",
            Action::CopyInviteLink => "Copy invite link",
            Action::RevokeInvite => "Revoke invite",
        }
    }

    /// The icon in front of the label (status entries show a colored dot instead).
    pub fn icon(self) -> char {
        use crate::widget::icon as i;
        match self {
            Action::Reconnect => i::LINK,
            Action::GoOffline | Action::LogOut => i::LOG_OUT,
            Action::GoOnline => i::SERVER,
            Action::CreateSpace | Action::CreateChannel | Action::CreateCategory => i::PLUS,
            Action::EditProfile | Action::ChangeNickname => i::ACCOUNT,
            Action::RemoveInstance
            | Action::DeleteSpace
            | Action::DeleteChannel
            | Action::DeleteMessage
            | Action::DeleteRole => i::DELETE,
            Action::SpaceSettings | Action::OpenSettings => i::SETTINGS,
            Action::ChannelPermissions | Action::ManageRoles | Action::EditRole => i::SHIELD,
            Action::InvitePeople => i::ADD_USER,
            Action::MarkSpaceRead | Action::MarkRead | Action::SelectMessage => i::CHECK,
            Action::SearchSpace => i::SEARCH,
            Action::CopySpaceId
            | Action::CopyChannelId
            | Action::CopyText
            | Action::CopyMessageId
            | Action::CopyUsername
            | Action::CopyUserId
            | Action::CopyInviteLink => i::COPY,
            Action::LeaveSpace | Action::LeaveGroup => i::LOG_OUT,
            Action::CloseDm => i::CLOSE,
            Action::AcceptRequest => i::CHECK,
            Action::NotifyAll | Action::NotifyMentions | Action::NotifyDefault => i::BELL,
            Action::NotifyNothing | Action::Mute => i::BELL_OFF,
            Action::Unmute => i::BELL,
            Action::OpenInNewPane | Action::SplitRight => i::SPLIT_RIGHT,
            Action::SplitDown => i::SPLIT_DOWN,
            Action::ShowPins | Action::Pin | Action::Unpin => i::PIN,
            Action::JoinVoice => i::PHONE,
            Action::EditChannel | Action::RenameGroup | Action::Edit => i::PENCIL,
            Action::MaximizePane => i::MAXIMIZE,
            Action::ToggleMembers => i::USERS,
            Action::ClosePane | Action::RevokeInvite => i::CLOSE,
            Action::Reply | Action::QuoteText => i::REPLY,
            Action::AddReaction => i::REACT,
            Action::SendMessage => i::CHAT,
            Action::Mention | Action::ShowMentions => i::AT,
            Action::Kick => i::LOG_OUT,
            Action::Ban => i::BAN,
            Action::TransferOwnership => i::SHIELD,
            Action::SetOnline | Action::SetIdle | Action::SetDnd | Action::SetInvisible => i::DOT,
        }
    }

    /// The status a status entry sets, for its colored dot.
    pub fn status(self) -> Option<hermes_client_core::proto::models::PresenceStatus> {
        use hermes_client_core::proto::models::PresenceStatus as P;
        match self {
            Action::SetOnline => Some(P::Online),
            Action::SetIdle => Some(P::Idle),
            Action::SetDnd => Some(P::Dnd),
            Action::SetInvisible => Some(P::Invisible),
            _ => None,
        }
    }

    /// Shown in red and confirmed first (Ban opens its own dialog instead).
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            Action::RemoveInstance
                | Action::LeaveSpace
                | Action::DeleteSpace
                | Action::DeleteMessage
                | Action::DeleteChannel
                | Action::LeaveGroup
                | Action::Kick
                | Action::DeleteRole
                | Action::RevokeInvite
                | Action::TransferOwnership
        )
    }

    /// Red, but with its own dialog rather than a yes/no confirmation.
    pub fn is_red(self) -> bool {
        self.is_destructive() || self == Action::Ban
    }
}

/// One menu entry: an action and whether it is the current choice (a check mark).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    pub checked: bool,
}

impl From<Action> for Entry {
    fn from(action: Action) -> Entry {
        Entry {
            action,
            checked: false,
        }
    }
}

/// `None`, or a separator before the next entry.
pub type Entries = Vec<Option<Entry>>;

fn push(out: &mut Entries, actions: impl IntoIterator<Item = Action>) {
    out.extend(actions.into_iter().map(|a| Some(a.into())));
}

/// I may delete the message: my own, or anyone's with Manage Messages outside DMs.
pub fn can_delete(s: &InstanceStore, channel: Id, message: Id) -> bool {
    let Some(msg) = s.window(channel).and_then(|w| w.get(message)) else {
        return false;
    };
    s.me_id() == Some(msg.author.id)
        || (!s.dms.contains_key(&channel) && view::can(s, channel, Permissions::MANAGE_MESSAGES))
}

/// The entries for `target`; `status` is the instance's connection status and `s` its store.
#[cfg(test)]
pub fn entries(target: &Target, status: &ConnStatus, s: Option<&InstanceStore>) -> Entries {
    entries_with(target, status, s, None)
}

/// [`entries`], with my presence for the user panel's menu.
pub fn entries_with(
    target: &Target,
    status: &ConnStatus,
    s: Option<&InstanceStore>,
    presence: Option<PresenceStatus>,
) -> Entries {
    let mut out = Entries::new();
    match target {
        Target::Instance(_) => {
            match status {
                ConnStatus::LoggedOut => {}
                ConnStatus::Offline => push(&mut out, [Action::GoOnline]),
                _ => push(&mut out, [Action::Reconnect, Action::GoOffline]),
            }
            if *status == ConnStatus::Connected {
                out.push(None);
                push(&mut out, [Action::CreateSpace, Action::EditProfile]);
            }
            out.push(None);
            if *status != ConnStatus::LoggedOut {
                push(&mut out, [Action::LogOut]);
            }
            push(&mut out, [Action::RemoveInstance]);
        }
        Target::Space(_, space) => {
            let Some(s) = s else { return out };
            let can = |p| view::can_in_space(s, *space, p);
            push(&mut out, [Action::MarkSpaceRead, Action::SearchSpace]);
            if can(Permissions::CREATE_INVITES) {
                push(&mut out, [Action::InvitePeople]);
            }
            out.push(None);
            push(&mut out, [Action::SpaceSettings]);
            if can(Permissions::MANAGE_CHANNELS) {
                push(&mut out, [Action::CreateChannel, Action::CreateCategory]);
            }
            out.push(None);
            notification_entries(s, *space, Some(NotificationLevel::Mentions), &mut out);
            out.push(None);
            push(&mut out, [Action::CopySpaceId]);
            let owner = s
                .spaces
                .get(space)
                .is_some_and(|st| Some(st.space.owner_id) == s.me_id());
            push(
                &mut out,
                [if owner {
                    Action::DeleteSpace
                } else {
                    Action::LeaveSpace
                }],
            );
        }
        Target::Channel(_, channel) => {
            let Some(s) = s else { return out };
            let Some(c) = s.channel(*channel) else {
                return out;
            };
            match c.kind {
                ChannelKind::Category => {
                    let manage = c
                        .space_id
                        .is_some_and(|sp| view::can_in_space(s, sp, Permissions::MANAGE_CHANNELS));
                    if manage {
                        push(
                            &mut out,
                            [
                                Action::CreateChannel,
                                Action::EditChannel,
                                Action::ChannelPermissions,
                            ],
                        );
                        out.push(None);
                    }
                    push(&mut out, [Action::CopyChannelId]);
                    if manage {
                        push(&mut out, [Action::DeleteChannel]);
                    }
                }
                ChannelKind::Dm | ChannelKind::GroupDm => {
                    if c.request {
                        push(&mut out, [Action::AcceptRequest]);
                        out.push(None);
                    }
                    push(
                        &mut out,
                        [Action::MarkRead, Action::OpenInNewPane, Action::ShowPins],
                    );
                    push(&mut out, [Action::JoinVoice]);
                    out.push(None);
                    notification_entries(s, *channel, Some(NotificationLevel::All), &mut out);
                    out.push(None);
                    if c.kind == ChannelKind::GroupDm {
                        push(&mut out, [Action::RenameGroup]);
                    }
                    push(&mut out, [Action::CopyChannelId]);
                    if c.kind == ChannelKind::GroupDm {
                        push(&mut out, [Action::LeaveGroup]);
                    } else {
                        push(&mut out, [Action::CloseDm]);
                    }
                }
                ChannelKind::Text | ChannelKind::Voice => {
                    push(
                        &mut out,
                        [Action::MarkRead, Action::OpenInNewPane, Action::ShowPins],
                    );
                    if c.kind == ChannelKind::Voice {
                        push(&mut out, [Action::JoinVoice]);
                    }
                    out.push(None);
                    notification_entries(s, *channel, None, &mut out);
                    let manage = view::can(s, *channel, Permissions::MANAGE_CHANNELS);
                    if manage {
                        out.push(None);
                        push(&mut out, [Action::EditChannel, Action::ChannelPermissions]);
                    }
                    out.push(None);
                    push(&mut out, [Action::CopyChannelId]);
                    if manage {
                        push(&mut out, [Action::DeleteChannel]);
                    }
                }
            }
        }
        Target::Pane(_) => push(
            &mut out,
            [
                Action::SplitRight,
                Action::SplitDown,
                Action::MaximizePane,
                Action::ToggleMembers,
                Action::ClosePane,
            ],
        ),
        Target::Message(_, channel, message) => {
            let Some(s) = s else { return out };
            let msg = s.window(*channel).and_then(|w| w.get(*message));
            let own = msg.is_some_and(|m| s.me_id() == Some(m.author.id));
            let deletable = can_delete(s, *channel, *message);
            let manage = view::can(s, *channel, Permissions::MANAGE_MESSAGES)
                || (s.dms.contains_key(channel));
            let pinned = msg.is_some_and(|m| m.pinned);
            let can_send = view::can(s, *channel, Permissions::SEND_MESSAGES);
            if can_send {
                push(&mut out, [Action::Reply]);
            }
            if view::can(s, *channel, Permissions::ADD_REACTIONS) {
                push(&mut out, [Action::AddReaction]);
            }
            if own {
                push(&mut out, [Action::Edit]);
            }
            if can_send {
                push(&mut out, [Action::QuoteText]);
            }
            push(&mut out, [Action::CopyText]);
            if manage {
                push(&mut out, [if pinned { Action::Unpin } else { Action::Pin }]);
            }
            out.push(None);
            push(&mut out, [Action::CopyMessageId]);
            if deletable {
                push(&mut out, [Action::SelectMessage, Action::DeleteMessage]);
            }
        }
        Target::Member(_, space, user) => {
            let me = s.and_then(InstanceStore::me_id) == Some(*user);
            if !me {
                push(&mut out, [Action::SendMessage]);
            }
            push(&mut out, [Action::Mention]);
            out.push(None);
            push(&mut out, [Action::CopyUsername, Action::CopyUserId]);
            if let (Some(s), Some(space)) = (s, space) {
                let can = |p| view::can_in_space(s, *space, p);
                let above = view::outranks(s, *space, *user);
                let mut moderation = Vec::new();
                if (me && can(Permissions::CHANGE_NICKNAME))
                    || (above && can(Permissions::MANAGE_NICKNAMES))
                {
                    moderation.push(Action::ChangeNickname);
                }
                if can(Permissions::MANAGE_ROLES) {
                    moderation.push(Action::ManageRoles);
                }
                if above && can(Permissions::KICK_MEMBERS) {
                    moderation.push(Action::Kick);
                }
                if above && can(Permissions::BAN_MEMBERS) {
                    moderation.push(Action::Ban);
                }
                if !moderation.is_empty() {
                    out.push(None);
                    push(&mut out, moderation);
                }
            }
        }
        Target::Role(_, space, role) => {
            push(&mut out, [Action::EditRole]);
            if let Some(s) = s
                && *role != *space
                && view::can_manage_role(s, *space, *role)
            {
                push(&mut out, [Action::DeleteRole]);
            }
        }
        Target::Invite(..) => push(&mut out, [Action::CopyInviteLink, Action::RevokeInvite]),
        // Selected messages have a bar of their own, not a menu.
        Target::Messages(..) => {}
        Target::Me(inst) => {
            let current = presence.unwrap_or(PresenceStatus::Online);
            for (action, status) in [
                (Action::SetOnline, PresenceStatus::Online),
                (Action::SetIdle, PresenceStatus::Idle),
                (Action::SetDnd, PresenceStatus::Dnd),
                (Action::SetInvisible, PresenceStatus::Invisible),
            ] {
                out.push(Some(Entry {
                    action,
                    checked: current == status,
                }));
            }
            out.push(None);
            if inst.is_some() {
                push(&mut out, [Action::EditProfile, Action::CopyUsername]);
            }
            push(&mut out, [Action::ShowMentions, Action::OpenSettings]);
        }
    }
    // No separator first, last or twice.
    let mut clean = Entries::new();
    for e in out {
        if e.is_some() || clean.last().is_some_and(Option::is_some) {
            clean.push(e);
        }
    }
    if clean.last().is_some_and(Option::is_none) {
        clean.pop();
    }
    clean
}

/// Notification level and mute entries for a space, a DM or a space channel. `fallback` is the
/// level used without a setting: `None` for a space channel (it follows its space, and gets a
/// "like the space" entry).
fn notification_entries(
    s: &InstanceStore,
    target: Id,
    fallback: Option<NotificationLevel>,
    out: &mut Entries,
) {
    let setting = s.notification_settings.get(&target);
    let current = setting.map(|n| n.level).or(fallback);
    let mut levels = vec![
        (Action::NotifyAll, Some(NotificationLevel::All)),
        (Action::NotifyMentions, Some(NotificationLevel::Mentions)),
        (Action::NotifyNothing, Some(NotificationLevel::None)),
    ];
    if fallback.is_none() {
        levels.insert(0, (Action::NotifyDefault, None));
    }
    for (action, l) in levels {
        out.push(Some(Entry {
            action,
            checked: current == l,
        }));
    }
    let muted = setting.is_some_and(|n| n.is_muted(hermes_client_core::now_ms()));
    out.push(Some(
        if muted { Action::Unmute } else { Action::Mute }.into(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(entries: &Entries) -> Vec<Action> {
        entries.iter().flatten().map(|e| e.action).collect()
    }

    #[test]
    fn instance_entries_follow_the_status() {
        let target = Target::Instance("h".into());
        assert_eq!(
            actions(&entries(&target, &ConnStatus::Connected, None)),
            [
                Action::Reconnect,
                Action::GoOffline,
                Action::CreateSpace,
                Action::EditProfile,
                Action::LogOut,
                Action::RemoveInstance
            ]
        );
        assert_eq!(
            actions(&entries(&target, &ConnStatus::Offline, None)),
            [Action::GoOnline, Action::LogOut, Action::RemoveInstance]
        );
        let logged_out = entries(&target, &ConnStatus::LoggedOut, None);
        assert_eq!(actions(&logged_out), [Action::RemoveInstance]);
        assert!(logged_out[0].is_some(), "no leading separator");
        assert!(Action::LeaveSpace.is_destructive());
        assert!(!Action::Ban.is_destructive() && Action::Ban.is_red());
        assert!(!Action::MarkRead.is_destructive());
    }
}
