//! Context menu entries of the sidebar, messages and members (PLAN.md section
//! 15.1). Which entries show depends on what I may do, so a menu never offers something the
//! server would refuse.

use crate::InstanceId;
use crate::gateway::ConnStatus;
use crate::store::InstanceStore;
use crate::view;
use hermes_proto::id::Id;
use hermes_proto::models::{ChannelKind, NotificationLevel, PresenceStatus};
use hermes_proto::permissions::Permissions;

/// What was right-clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Instance(InstanceId),
    Space(InstanceId, Id),
    /// A space channel, a category, a DM or a group DM.
    Channel(InstanceId, Id),
    /// A message's `⋯`: instance, channel, message.
    Message(InstanceId, Id, Id),
    /// Messages selected together in a channel (deleted at once): instance, channel, messages.
    Messages(InstanceId, Id, Vec<Id>),
    /// A file in a message: instance, channel, message, attachment.
    File(InstanceId, Id, Id, Id),
    /// A person in a member list, a voice room or a message: instance, space (`None` in a DM),
    /// user.
    Member(InstanceId, Option<Id>, Id),
    /// A role in the space settings: instance, space, role.
    Role(InstanceId, Id, Id),
    /// An invite in the space settings: instance, space, code.
    Invite(InstanceId, Id, String),
    /// My own entry in the user panel (`None`: no instance yet).
    Me(Option<InstanceId>),
    /// The rail's empty space and its plus: adding instances, and each instance's menu.
    Instances,
    /// The empty space around a server's channels: instance, server.
    ChannelArea(InstanceId, Id),
}

impl Target {
    pub fn inst(&self) -> Option<&InstanceId> {
        match self {
            Target::Instance(inst)
            | Target::Space(inst, _)
            | Target::Channel(inst, _)
            | Target::Message(inst, ..)
            | Target::Messages(inst, ..)
            | Target::File(inst, ..)
            | Target::Member(inst, ..)
            | Target::Role(inst, ..)
            | Target::Invite(inst, ..)
            | Target::ChannelArea(inst, _)
            | Target::Me(Some(inst)) => Some(inst),
            Target::Me(None) | Target::Instances => None,
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
    MuteFor15Minutes,
    MuteForHour,
    MuteFor8Hours,
    MuteForDay,
    /// Until I unmute it.
    Mute,
    Unmute,
    // Channel
    MarkRead,
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
    // Message
    Reply,
    /// Opens the quick reactions of a message.
    AddReaction,
    Edit,
    QuoteText,
    CopyText,
    CopyMessageId,
    /// Keeps the message in my saved messages (on this device).
    Save,
    Unsave,
    Pin,
    Unpin,
    DeleteMessage,
    /// Starts selecting messages (Ctrl+click and Shift+click add more).
    SelectMessage,
    // Member
    Profile,
    /// Opens the volume slider of someone in voice.
    Volume,
    MuteForMe,
    UnmuteForMe,
    MoveServerUp,
    MoveServerDown,
    MarkUnreadHere,
    CopyLink,
    Forward,
    EditNote,
    EditLocalName,
    Report,
    Block,
    Unblock,
    Timeout10Minutes,
    Timeout1Hour,
    Timeout1Day,
    EndTimeout,
    ServerMute,
    ServerUnmute,
    ServerDeafen,
    ServerUndeafen,
    /// Stands for the "Move to" submenu of voice channels, which the menu builds.
    MoveVoice,
    DisconnectVoice,
    SendMessage,
    Mention,
    CopyUsername,
    CopyUserId,
    AddFriend,
    AcceptFriend,
    DeclineFriend,
    CancelFriendRequest,
    RemoveFriend,
    /// Instance admins: lets the account go without a `#1234` tag, or gives it one again.
    GiveNoTag,
    TakeNoTag,
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
            Action::CreateSpace => "Create a server…",
            Action::EditProfile => "Edit profile…",
            Action::LogOut => "Log out",
            Action::RemoveInstance => "Remove instance",
            Action::SpaceSettings => "Server settings",
            Action::CreateChannel => "Create channel…",
            Action::CreateCategory => "Create category…",
            Action::InvitePeople => "Invite people (copy link)",
            Action::MarkSpaceRead => "Mark all as read",
            Action::SearchSpace => "Search in this server",
            Action::CopySpaceId => "Copy server ID",
            Action::LeaveSpace => "Leave server",
            Action::DeleteSpace => "Delete server",
            Action::NotifyAll => "Notify: all messages",
            Action::NotifyMentions => "Notify: only @mentions",
            Action::NotifyNothing => "Notify: nothing",
            Action::NotifyDefault => "Notify: like the server",
            Action::MuteFor15Minutes => "Mute for 15 minutes",
            Action::MuteForHour => "Mute for 1 hour",
            Action::MuteFor8Hours => "Mute for 8 hours",
            Action::MuteForDay => "Mute for 24 hours",
            Action::Mute => "Mute",
            Action::Unmute => "Unmute",
            Action::MarkRead => "Mark as read",
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
            Action::Reply => "Reply",
            Action::AddReaction => "Add reaction",
            Action::Edit => "Edit",
            Action::QuoteText => "Quote",
            Action::CopyText => "Copy text",
            Action::CopyMessageId => "Copy message ID",
            Action::Save => "Save message",
            Action::Unsave => "Remove from saved",
            Action::Pin => "Pin message",
            Action::Unpin => "Unpin message",
            Action::DeleteMessage => "Delete message",
            Action::SelectMessage => "Select",
            Action::Profile => "Profile",
            Action::Volume => "Volume…",
            Action::MuteForMe => "Mute for me",
            Action::UnmuteForMe => "Unmute for me",
            Action::ServerMute => "Server mute",
            Action::ServerUnmute => "Server unmute",
            Action::ServerDeafen => "Server deafen",
            Action::ServerUndeafen => "Server undeafen",
            Action::MoveVoice => "Move to",
            Action::DisconnectVoice => "Disconnect from voice",
            Action::SendMessage => "Message",
            Action::Mention => "Mention",
            Action::CopyUsername => "Copy username",
            Action::AddFriend => "Add friend",
            Action::AcceptFriend => "Accept friend request",
            Action::DeclineFriend => "Decline friend request",
            Action::CancelFriendRequest => "Cancel friend request",
            Action::RemoveFriend => "Remove friend",
            Action::GiveNoTag => "Remove their # number",
            Action::TakeNoTag => "Give them a # number",
            Action::CopyUserId => "Copy user ID",
            Action::ChangeNickname => "Change nickname…",
            Action::ManageRoles => "Roles…",
            Action::MoveServerUp => "Move up",
            Action::MoveServerDown => "Move down",
            Action::MarkUnreadHere => "Mark unread from here",
            Action::CopyLink => "Copy link",
            Action::Forward => "Forward…",
            Action::EditNote => "Personal note…",
            Action::EditLocalName => "Name them for me…",
            Action::Report => "Report…",
            Action::Block => "Block",
            Action::Unblock => "Unblock",
            Action::Timeout10Minutes => "Timeout for 10 minutes",
            Action::Timeout1Hour => "Timeout for 1 hour",
            Action::Timeout1Day => "Timeout for 1 day",
            Action::EndTimeout => "End timeout",
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

    /// The submenu this action goes into when others of the group are next to it.
    fn group(self) -> Option<&'static str> {
        match self {
            Action::NotifyAll
            | Action::NotifyMentions
            | Action::NotifyNothing
            | Action::NotifyDefault => Some("Notifications"),
            Action::MuteFor15Minutes
            | Action::MuteForHour
            | Action::MuteFor8Hours
            | Action::MuteForDay
            | Action::Mute
            | Action::Unmute => Some("Mute"),
            // What a message's menu offers less often.
            Action::QuoteText
            | Action::MarkUnreadHere
            | Action::CopyLink
            | Action::SelectMessage
            | Action::Report => Some("More"),
            Action::CreateChannel | Action::CreateCategory => Some("Create"),
            Action::SetOnline | Action::SetIdle | Action::SetDnd | Action::SetInvisible => {
                Some("Status")
            }
            _ => None,
        }
    }

    /// The label in the channel area's menu, which has no submenus.
    pub fn area_label(self) -> &'static str {
        match self {
            Action::CreateChannel => "Create Channel",
            Action::CreateCategory => "Create Category",
            Action::InvitePeople => "Invite to Server",
            other => other.label(),
        }
    }

    /// The label inside the action's submenu, which already says what it is about.
    pub fn short_label(self) -> &'static str {
        match self {
            Action::NotifyAll => "All messages",
            Action::NotifyMentions => "Only @mentions",
            Action::NotifyNothing => "Nothing",
            Action::NotifyDefault => "Like the server",
            Action::Mute => "Mute until I unmute",
            Action::CreateChannel => "Channel…",
            Action::CreateCategory => "Category…",
            other => other.label(),
        }
    }

    /// The status a status entry sets, for its colored dot.
    pub fn status(self) -> Option<hermes_proto::models::PresenceStatus> {
        use hermes_proto::models::PresenceStatus as P;
        match self {
            Action::SetOnline => Some(P::Online),
            Action::SetIdle => Some(P::Idle),
            Action::SetDnd => Some(P::Dnd),
            Action::SetInvisible => Some(P::Invisible),
            _ => None,
        }
    }

    /// Shown in red: it removes, ends or refuses something. Not all are confirmed first.
    pub fn is_negative(self) -> bool {
        self.is_destructive()
            || matches!(
                self,
                Action::CloseDm
                    | Action::DisconnectVoice
                    | Action::Ban
                    | Action::LogOut
                    | Action::RemoveFriend
                    | Action::DeclineFriend
                    | Action::CancelFriendRequest
            )
    }

    /// Confirmed first (Ban opens its own dialog instead).
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
}

/// One menu entry: an action and whether it is the current choice (a check mark).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    pub checked: bool,
    /// For "Unmute": until when it is muted (Unix ms), when not for ever.
    pub until: Option<i64>,
}

impl From<Action> for Entry {
    fn from(action: Action) -> Entry {
        Entry {
            action,
            checked: false,
            until: None,
        }
    }
}

impl Action {
    /// How long a mute entry mutes for, in minutes; `None` for anything else.
    pub const fn mute_minutes(self) -> Option<i64> {
        match self {
            Action::MuteFor15Minutes => Some(15),
            Action::MuteForHour => Some(60),
            Action::MuteFor8Hours => Some(8 * 60),
            Action::MuteForDay => Some(24 * 60),
            _ => None,
        }
    }

    /// Mutes or unmutes: these get a separator of their own in the notifications submenu.
    const fn is_mute(self) -> bool {
        self.mute_minutes().is_some() || matches!(self, Action::Mute | Action::Unmute)
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

/// The actions `target`'s menu offers on a connected instance, without separators or
/// submenus' structure: for a client that draws its own menus but must offer the same things
/// (the TUI).
pub fn allowed_actions(target: &Target, s: &InstanceStore) -> Vec<Action> {
    entries_with(target, &ConnStatus::Connected, Some(s), Extra::default())
        .into_iter()
        .flatten()
        .map(|entry| entry.action)
        .collect()
}

/// What the entries depend on beyond the store.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extra {
    /// My presence, for the user panel's menu.
    pub presence: Option<PresenceStatus>,
    /// The person is muted for me (a member's menu).
    pub locally_muted: bool,
    /// The message is in my saved messages (a message's menu).
    pub saved: bool,
    /// Developer mode: menus offer copying IDs.
    pub developer: bool,
}

/// The entries for `target`; `status` is the instance's connection status and `s` its store.
#[cfg(test)]
pub fn entries(target: &Target, status: &ConnStatus, s: Option<&InstanceStore>) -> Entries {
    entries_with(target, status, s, Extra::default())
}

/// [`entries`], with what they depend on outside the store.
pub fn entries_with(
    target: &Target,
    status: &ConnStatus,
    s: Option<&InstanceStore>,
    extra: Extra,
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
            push(&mut out, [Action::MoveServerUp, Action::MoveServerDown]);
            out.push(None);
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
            if extra.developer {
                push(&mut out, [Action::CopySpaceId]);
            }
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
                // From a newer server: nothing here can act on it.
                ChannelKind::Unknown => {}
                ChannelKind::Category => {
                    // As the server checks them: creating space-wide, editing and deleting in the
                    // category itself (an override there can take Manage channels away).
                    let create = c
                        .space_id
                        .is_some_and(|sp| view::can_in_space(s, sp, Permissions::MANAGE_CHANNELS));
                    let manage = view::can(s, *channel, Permissions::MANAGE_CHANNELS);
                    if create {
                        push(&mut out, [Action::CreateChannel]);
                    }
                    if manage {
                        push(&mut out, [Action::EditChannel, Action::ChannelPermissions]);
                    }
                    if create || manage {
                        out.push(None);
                    }
                    if extra.developer {
                        push(&mut out, [Action::CopyChannelId]);
                    }
                    if manage {
                        push(&mut out, [Action::DeleteChannel]);
                    }
                }
                ChannelKind::Dm | ChannelKind::GroupDm => {
                    if c.request {
                        push(&mut out, [Action::AcceptRequest]);
                        out.push(None);
                    }
                    push(&mut out, [Action::MarkRead, Action::ShowPins]);
                    push(&mut out, [Action::JoinVoice]);
                    out.push(None);
                    notification_entries(s, *channel, Some(NotificationLevel::All), &mut out);
                    out.push(None);
                    if c.kind == ChannelKind::GroupDm {
                        push(&mut out, [Action::RenameGroup]);
                    }
                    if extra.developer {
                        push(&mut out, [Action::CopyChannelId]);
                    }
                    if c.kind == ChannelKind::GroupDm {
                        push(&mut out, [Action::LeaveGroup]);
                    } else {
                        push(&mut out, [Action::CloseDm]);
                    }
                }
                ChannelKind::Text | ChannelKind::Voice => {
                    push(&mut out, [Action::MarkRead, Action::ShowPins]);
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
                    if extra.developer {
                        push(&mut out, [Action::CopyChannelId]);
                    }
                    if manage {
                        push(&mut out, [Action::DeleteChannel]);
                    }
                }
            }
        }
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
            push(&mut out, [Action::CopyText]);
            push(
                &mut out,
                [if extra.saved {
                    Action::Unsave
                } else {
                    Action::Save
                }],
            );
            if msg.is_some_and(|m| !m.content.is_empty() || !m.attachments.is_empty()) {
                push(&mut out, [Action::Forward]);
            }
            if manage {
                push(&mut out, [if pinned { Action::Unpin } else { Action::Pin }]);
            }
            // More ▸
            if can_send {
                push(&mut out, [Action::QuoteText]);
            }
            push(&mut out, [Action::MarkUnreadHere, Action::CopyLink]);
            if deletable {
                push(&mut out, [Action::SelectMessage]);
            }
            if !own {
                push(&mut out, [Action::Report]);
            }
            if extra.developer || deletable {
                out.push(None);
            }
            if extra.developer {
                push(&mut out, [Action::CopyMessageId]);
            }
            if deletable {
                push(&mut out, [Action::DeleteMessage]);
            }
        }
        Target::Member(_, space, user) => {
            let me = s.and_then(InstanceStore::me_id) == Some(*user);
            push(&mut out, [Action::Profile]);
            if !me {
                push(&mut out, [Action::SendMessage]);
            }
            push(&mut out, [Action::Mention]);
            if let (false, Some(s)) = (me, s) {
                use hermes_proto::models::RelationshipKind as R;
                match s.relationships.get(user).map(|r| r.kind) {
                    None => push(&mut out, [Action::AddFriend]),
                    Some(R::Friend) => push(&mut out, [Action::RemoveFriend]),
                    Some(R::Incoming) => {
                        push(&mut out, [Action::AcceptFriend, Action::DeclineFriend]);
                    }
                    Some(R::Outgoing) => push(&mut out, [Action::CancelFriendRequest]),
                }
            }
            if let (false, Some(s)) = (me, s) {
                push(&mut out, [Action::EditNote, Action::EditLocalName]);
                push(
                    &mut out,
                    [if s.blocked.contains(user) {
                        Action::Unblock
                    } else {
                        Action::Block
                    }],
                );
                voice_entries(s, *user, extra.locally_muted, &mut out);
            }
            out.push(None);
            push(&mut out, [Action::CopyUsername]);
            if extra.developer {
                push(&mut out, [Action::CopyUserId]);
            }
            if let Some(s) = s
                && s.me.as_ref().is_some_and(|m| m.instance_admin)
                && let Some(them) = s.user(*user)
            {
                push(
                    &mut out,
                    [if them.discriminator.is_some() {
                        Action::GiveNoTag
                    } else {
                        Action::TakeNoTag
                    }],
                );
            }
            if let (Some(s), Some(space)) = (s, space) {
                let can = |p| view::can_in_space(s, *space, p);
                let above = view::outranks(s, *space, *user);
                let mut moderation = Vec::new();
                // My own nickname needs either permission (as on the server).
                if (me && (can(Permissions::CHANGE_NICKNAME) || can(Permissions::MANAGE_NICKNAMES)))
                    || (above && can(Permissions::MANAGE_NICKNAMES))
                {
                    moderation.push(Action::ChangeNickname);
                }
                if can(Permissions::MANAGE_ROLES) {
                    moderation.push(Action::ManageRoles);
                }
                if above && can(Permissions::KICK_MEMBERS) {
                    moderation.push(Action::Timeout10Minutes);
                    moderation.push(Action::Timeout1Hour);
                    moderation.push(Action::Timeout1Day);
                    moderation.push(Action::EndTimeout);
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
        // The instances are listed by the menu itself, with their own entries.
        Target::Instances => {}
        Target::ChannelArea(_, space) => {
            let Some(s) = s else { return out };
            if view::can_in_space(s, *space, Permissions::MANAGE_CHANNELS) {
                push(&mut out, [Action::CreateChannel, Action::CreateCategory]);
            }
            if view::can_in_space(s, *space, Permissions::CREATE_INVITES) {
                push(&mut out, [Action::InvitePeople]);
            }
        }
        // Selected messages have a bar of their own, not a menu.
        Target::Messages(..) | Target::File(..) => {}
        Target::Me(inst) => {
            let current = extra.presence.unwrap_or(PresenceStatus::Online);
            for (action, status) in [
                (Action::SetOnline, PresenceStatus::Online),
                (Action::SetIdle, PresenceStatus::Idle),
                (Action::SetDnd, PresenceStatus::Dnd),
                (Action::SetInvisible, PresenceStatus::Invisible),
            ] {
                out.push(Some(Entry {
                    action,
                    checked: current == status,
                    until: None,
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

/// A line of a menu after grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grouped {
    Entry(Entry),
    Separator,
    Submenu {
        label: &'static str,
        entries: Entries,
    },
}

/// Puts neighbouring entries of a group into a submenu. A lone entry stays as it is: a category's
/// menu only has "Create channel", and a submenu of one is an extra step for nothing. Mute gets a
/// separator of its own, since it is not one of the levels above it.
pub fn grouped(entries: &[Option<Entry>]) -> Vec<Grouped> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let Some(entry) = entries[i] else {
            out.push(Grouped::Separator);
            i += 1;
            continue;
        };
        let Some(label) = entry.action.group() else {
            out.push(Grouped::Entry(entry));
            i += 1;
            continue;
        };
        let members: Vec<Entry> = entries[i..]
            .iter()
            .map_while(|e| e.filter(|e| e.action.group() == Some(label)))
            .collect();
        i += members.len();
        if let [only] = members.as_slice() {
            out.push(Grouped::Entry(*only));
            continue;
        }
        let mut inside = Entries::new();
        let mut after_mute = false;
        for member in members {
            let mute = member.action.is_mute();
            if mute && !after_mute && !inside.is_empty() {
                inside.push(None);
            }
            after_mute = mute;
            inside.push(Some(member));
        }
        out.push(Grouped::Submenu {
            label,
            entries: inside,
        });
    }
    out
}

/// Someone else in voice: how I hear them, then what moderators may do, checked like the server
/// does in the channel they are in and only for members below my highest role.
fn voice_entries(s: &InstanceStore, user: Id, locally_muted: bool, out: &mut Entries) {
    let Some(state) = s.voice_states.get(&user) else {
        return;
    };
    out.push(None);
    push(
        out,
        [
            Action::Volume,
            if locally_muted {
                Action::UnmuteForMe
            } else {
                Action::MuteForMe
            },
        ],
    );
    let channel = state.channel_id;
    let Some(space) = s.space_of(channel) else {
        return;
    };
    if !view::outranks(s, space, user) {
        return;
    }
    let can = |p| view::can(s, channel, p);
    let mut moderation = Vec::new();
    if can(Permissions::MUTE_MEMBERS) {
        moderation.push(if state.server_mute {
            Action::ServerUnmute
        } else {
            Action::ServerMute
        });
    }
    if can(Permissions::DEAFEN_MEMBERS) {
        moderation.push(if state.server_deaf {
            Action::ServerUndeafen
        } else {
            Action::ServerDeafen
        });
    }
    if can(Permissions::MOVE_MEMBERS) {
        moderation.extend([Action::MoveVoice, Action::DisconnectVoice]);
    }
    if !moderation.is_empty() {
        out.push(None);
        push(out, moderation);
    }
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
            until: None,
        }));
    }
    let muted = setting.filter(|n| n.is_muted(crate::now_ms()));
    match muted {
        Some(n) => out.push(Some(Entry {
            until: n
                .muted_until
                .filter(|u| *u != hermes_proto::models::NotificationSetting::MUTED_FOREVER),
            ..Action::Unmute.into()
        })),
        None => push(
            out,
            [
                Action::MuteFor15Minutes,
                Action::MuteForHour,
                Action::MuteFor8Hours,
                Action::MuteForDay,
                Action::Mute,
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(entries: &Entries) -> Vec<Action> {
        entries.iter().flatten().map(|e| e.action).collect()
    }

    #[test]
    fn neighbours_of_a_group_share_a_submenu() {
        let entry = |action: Action| Some(Entry::from(action));
        let lines = grouped(&[
            entry(Action::MarkRead),
            None,
            entry(Action::NotifyAll),
            entry(Action::NotifyNothing),
            entry(Action::Mute),
            None,
            entry(Action::CreateChannel),
            entry(Action::EditChannel),
        ]);
        assert_eq!(
            lines,
            [
                Grouped::Entry(Action::MarkRead.into()),
                Grouped::Separator,
                Grouped::Submenu {
                    label: "Notifications",
                    entries: vec![entry(Action::NotifyAll), entry(Action::NotifyNothing)],
                },
                // The mutes have a submenu of their own; one alone is a plain entry.
                Grouped::Entry(Action::Mute.into()),
                Grouped::Separator,
                Grouped::Entry(Action::CreateChannel.into()),
                Grouped::Entry(Action::EditChannel.into()),
            ],
            "a lone group member stays a plain entry"
        );
    }

    #[test]
    fn channel_and_member_entries_follow_the_server_rules() {
        use hermes_proto::models::Override;
        let mut s = crate::testing::fixtures::store();
        let (inst, space, games, me) = ("h".to_owned(), Id(10), Id(12), Id(1));
        let st = s.spaces.get_mut(&space).unwrap();
        // Bob owns the space; I manage channels and nicknames through @everyone, but the
        // "Games" category takes Manage channels away from @everyone.
        st.space.owner_id = Id(3);
        st.roles[0].permissions = (st.roles[0].permissions
            | Permissions::MANAGE_CHANNELS
            | Permissions::MANAGE_NICKNAMES)
            - Permissions::CHANGE_NICKNAME;
        st.channels.get_mut(&games).unwrap().overrides = vec![Override {
            role_id: space,
            allow: Permissions::empty(),
            deny: Permissions::MANAGE_CHANNELS,
        }];
        let category = actions(&entries(
            &Target::Channel(inst.clone(), games),
            &ConnStatus::Connected,
            Some(&s),
        ));
        assert!(!category.contains(&Action::EditChannel), "{category:?}");
        assert!(!category.contains(&Action::DeleteChannel), "{category:?}");
        assert!(
            category.contains(&Action::CreateChannel),
            "creating is space-wide: {category:?}"
        );
        let general = actions(&entries(
            &Target::Channel(inst.clone(), Id(11)),
            &ConnStatus::Connected,
            Some(&s),
        ));
        assert!(general.contains(&Action::EditChannel), "{general:?}");

        let mine = actions(&entries(
            &Target::Member(inst, Some(space), me),
            &ConnStatus::Connected,
            Some(&s),
        ));
        assert!(
            mine.contains(&Action::ChangeNickname),
            "Manage nicknames covers my own: {mine:?}"
        );
    }

    #[test]
    fn the_mutes_get_a_submenu_beside_the_levels() {
        let entry = |action: Action| Some(Entry::from(action));
        let lines = grouped(&[
            entry(Action::NotifyAll),
            entry(Action::NotifyMentions),
            entry(Action::MuteFor15Minutes),
            entry(Action::MuteForHour),
            entry(Action::Mute),
        ]);
        let [
            Grouped::Submenu {
                label: "Notifications",
                entries: levels,
            },
            Grouped::Submenu {
                label: "Mute",
                entries: mutes,
            },
        ] = lines.as_slice()
        else {
            panic!("{lines:?}");
        };
        assert_eq!(levels.len(), 2);
        assert_eq!(mutes.len(), 3, "no separator inside: {mutes:?}");
        assert_eq!(Action::MuteForDay.mute_minutes(), Some(24 * 60));
        assert_eq!(Action::Mute.mute_minutes(), None);
    }

    #[test]
    fn ids_need_developer_mode_and_rare_message_actions_go_under_more() {
        let s = crate::testing::fixtures::store();
        let target = Target::Channel("h".into(), Id(11));
        let plain = actions(&entries_with(
            &target,
            &ConnStatus::Connected,
            Some(&s),
            Extra::default(),
        ));
        assert!(!plain.contains(&Action::CopyChannelId), "{plain:?}");
        let dev = actions(&entries_with(
            &target,
            &ConnStatus::Connected,
            Some(&s),
            Extra {
                developer: true,
                ..Extra::default()
            },
        ));
        assert!(dev.contains(&Action::CopyChannelId), "{dev:?}");
        for more in [
            Action::QuoteText,
            Action::MarkUnreadHere,
            Action::CopyLink,
            Action::SelectMessage,
            Action::Report,
        ] {
            assert_eq!(more.group(), Some("More"), "{more:?}");
        }
    }

    fn member_entries(s: &InstanceStore, locally_muted: bool) -> Vec<Action> {
        let target = Target::Member("h".into(), Some(Id(10)), Id(3));
        let extra = Extra {
            presence: None,
            locally_muted,
            saved: false,
            developer: true,
        };
        actions(&entries_with(
            &target,
            &ConnStatus::Connected,
            Some(s),
            extra,
        ))
    }

    #[test]
    fn someone_in_voice_gets_volume_and_moderation() {
        // I own the server; Bob is in its voice channel.
        let mut s = crate::testing::fixtures::store();
        let all = member_entries(&s, false);
        for action in [
            Action::Profile,
            Action::Volume,
            Action::MuteForMe,
            Action::ServerMute,
            Action::ServerDeafen,
            Action::MoveVoice,
            Action::DisconnectVoice,
            Action::Kick,
        ] {
            assert!(all.contains(&action), "{action:?} missing from {all:?}");
        }
        assert_eq!(all[0], Action::Profile);
        assert!(member_entries(&s, true).contains(&Action::UnmuteForMe));

        s.voice_states.get_mut(&Id(3)).expect("bob").server_mute = true;
        let muted = member_entries(&s, false);
        assert!(muted.contains(&Action::ServerUnmute) && !muted.contains(&Action::ServerMute));

        // Bob owns the server now: I cannot act on him, but still choose how I hear him.
        s.spaces.get_mut(&Id(10)).expect("space").space.owner_id = Id(3);
        let plain = member_entries(&s, false);
        assert!(plain.contains(&Action::Volume) && plain.contains(&Action::MuteForMe));
        for action in [
            Action::ServerUnmute,
            Action::MoveVoice,
            Action::DisconnectVoice,
        ] {
            assert!(!plain.contains(&action), "{action:?} offered: {plain:?}");
        }

        // Out of voice: no voice entries at all.
        s.voice_states.remove(&Id(3));
        assert!(!member_entries(&s, false).contains(&Action::Volume));
        assert!(Action::DisconnectVoice.is_negative());
    }

    #[test]
    fn the_channel_area_and_the_rail_have_their_own_menus() {
        let s = crate::testing::fixtures::store();
        let area = Target::ChannelArea("h".into(), Id(10));
        assert_eq!(
            actions(&entries(&area, &ConnStatus::Connected, Some(&s))),
            [
                Action::CreateChannel,
                Action::CreateCategory,
                Action::InvitePeople
            ]
        );
        assert_eq!(Action::InvitePeople.area_label(), "Invite to Server");
        assert!(entries(&Target::Instances, &ConnStatus::Connected, None).is_empty());
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
        assert!(!Action::Ban.is_destructive());
        assert!(!Action::MarkRead.is_destructive());
        assert!(Action::CloseDm.is_negative() && !Action::CloseDm.is_destructive());
        assert!(Action::LeaveSpace.is_negative() && !Action::MarkRead.is_negative());
    }
}
