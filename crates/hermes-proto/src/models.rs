//! Wire models (PLAN.md section 7.4). Timestamps are Unix milliseconds.

use serde::{Deserialize, Serialize};

use crate::id::Id;
use crate::permissions::Permissions;

/// A reference to an uploaded file used as an avatar or icon.
/// It is downloaded from `/api/v1/attachments/{id}/{filename}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub id: Id,
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Id,
    pub username: String,
    /// The four digits after the name, as in `name#1234`. `None`: an account the instance's owner
    /// lets go without one (and everyone on a server older than tags).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,
    pub display_name: Option<String>,
    pub avatar: Option<AttachmentRef>,
    pub status_text: Option<String>,
}

impl User {
    /// The display name if set, otherwise the username.
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.username)
    }

    /// What others find this account by: `name#1234`, or `name` without a tag.
    pub fn handle(&self) -> String {
        match &self.discriminator {
            Some(tag) => format!("{}#{tag}", self.username),
            None => self.username.clone(),
        }
    }
}

/// Splits a handle into its name and tag: `name#1234` or `name`.
pub fn split_handle(handle: &str) -> (&str, Option<&str>) {
    match handle.split_once('#') {
        Some((name, tag)) => (name, Some(tag)),
        None => (handle, None),
    }
}

/// How someone stands to me in the friend system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    Friend,
    /// They asked to be my friend.
    Incoming,
    /// I asked to be their friend.
    Outgoing,
}

/// One person in my friend list or requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    pub user: User,
    pub kind: RelationshipKind,
    /// When the request was accepted, or sent while it is pending.
    pub since: i64,
}

/// The logged-in user: `User` plus `instance_admin` (used in `ready` and `GET /users/@me`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentUser {
    #[serde(flatten)]
    pub user: User,
    pub instance_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: Id,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<AttachmentRef>,
    pub owner_id: Id,
}

/// Public preview of a space, shown for invites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpacePreview {
    pub id: Id,
    pub name: String,
    pub icon: Option<AttachmentRef>,
    pub member_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    /// The `@everyone` role has the same ID as its space.
    pub id: Id,
    pub space_id: Id,
    pub name: String,
    /// `0xRRGGBB`.
    pub color: Option<u32>,
    pub hoist: bool,
    pub mentionable: bool,
    /// Higher = more powerful; `@everyone` is 0.
    pub position: i32,
    pub permissions: Permissions,
}

impl Role {
    pub fn is_everyone(&self) -> bool {
        self.id == self.space_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub space_id: Id,
    pub user: User,
    pub nickname: Option<String>,
    /// Explicitly assigned roles. `@everyone` is implicit and not listed.
    pub role_ids: Vec<Id>,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Text,
    Voice,
    Category,
    Dm,
    GroupDm,
}

impl ChannelKind {
    /// Whether the channel holds messages (voice channels have a text chat too).
    pub const fn has_messages(self) -> bool {
        !matches!(self, ChannelKind::Category)
    }

    /// Whether the channel has a voice room.
    pub const fn has_voice(self) -> bool {
        matches!(
            self,
            ChannelKind::Voice | ChannelKind::Dm | ChannelKind::GroupDm
        )
    }

    pub const fn is_private(self) -> bool {
        matches!(self, ChannelKind::Dm | ChannelKind::GroupDm)
    }
}

/// A per-role permission override on a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Override {
    pub role_id: Id,
    pub allow: Permissions,
    pub deny: Permissions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: Id,
    /// `None` for DMs and group DMs.
    pub space_id: Option<Id>,
    pub kind: ChannelKind,
    pub parent_id: Option<Id>,
    /// `None` for 1:1 DMs.
    pub name: Option<String>,
    pub topic: Option<String>,
    pub position: i32,
    /// Voice only, bits/s.
    pub bitrate: Option<u32>,
    /// Voice only; `None` or 0 = unlimited.
    pub user_limit: Option<u32>,
    pub slowmode_secs: u32,
    pub overrides: Vec<Override>,
    /// DMs and group DMs only.
    pub recipients: Vec<User>,
    /// Group DM owner.
    pub owner_id: Option<Id>,
    pub last_message_id: Option<Id>,
    /// DMs only, and only as sent to you: someone you do not accept direct messages from
    /// started it, so it waits in your message requests until you accept it (sending a message
    /// accepts it too). Absent (`false`) everywhere else.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub request: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Default,
    /// Join, call started, pin, …
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRef {
    pub id: Id,
    pub author_id: Id,
    /// First 100 characters, empty if the message was deleted.
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Id,
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub has_thumb: bool,
    /// The key of an encrypted file, from inside its message. Never on the wire.
    #[serde(skip)]
    pub sealed: Option<crate::e2ee::SealedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionSummary {
    /// Unicode emoji sequence.
    pub emoji: String,
    pub count: u32,
    /// Whether the receiving user reacted with it.
    pub me: bool,
}

/// Who a message mentions (from `markup::extract_mentions`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mentions {
    pub users: Vec<Id>,
    pub roles: Vec<Id>,
    pub everyone: bool,
    pub here: bool,
}

impl Mentions {
    pub fn is_empty(&self) -> bool {
        self.users.is_empty() && self.roles.is_empty() && !self.everyone && !self.here
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: Id,
    pub channel_id: Id,
    pub author: User,
    /// Raw markup, see `markup`.
    pub content: String,
    pub reply_to: Option<MessageRef>,
    pub edited_at: Option<i64>,
    pub deleted: bool,
    pub pinned: bool,
    pub kind: MessageKind,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<ReactionSummary>,
    pub mentions: Mentions,
    /// Only echoed to the author's sessions; omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    /// The encrypted form: `content` is empty on the wire and the client opens this into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<crate::e2ee::Envelope>,
    /// How the client found this message. Never on the wire.
    #[serde(skip)]
    pub lock: crate::e2ee::MessageLock,
}

impl Message {
    /// Creation time in Unix milliseconds, derived from the ID.
    pub const fn created_at(&self) -> u64 {
        self.id.timestamp_ms()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadState {
    pub channel_id: Id,
    pub last_read_id: Id,
    pub mention_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceState {
    pub user_id: Id,
    pub channel_id: Id,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub server_mute: bool,
    pub server_deaf: bool,
    pub joined_at: i64,
}

/// A user's presence as others see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Online,
    Idle,
    Dnd,
    Offline,
}

/// The status a client asks for. `Invisible` appears as `Offline` to others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Online,
    Idle,
    Dnd,
    Invisible,
}

impl PresenceStatus {
    /// What other users see.
    pub const fn visible_status(self) -> Status {
        match self {
            PresenceStatus::Online => Status::Online,
            PresenceStatus::Idle => Status::Idle,
            PresenceStatus::Dnd => Status::Dnd,
            PresenceStatus::Invisible => Status::Offline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub user_id: Id,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    /// `None` for instance-only invites.
    pub space: Option<SpacePreview>,
    /// `None` for invites created by the server itself (first-start setup invite, the
    /// `hermes-server invite create` command).
    pub creator_id: Option<Id>,
    pub max_uses: Option<u32>,
    pub uses: u32,
    pub expires_at: Option<i64>,
}

/// One entry of `GET /spaces/{id}/bans`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ban {
    pub user: User,
    pub reason: Option<String>,
    pub created_at: i64,
}

/// What an audit log entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    SpaceUpdate,
    SpaceTransfer,
    MemberUpdate,
    MemberKick,
    MemberBan,
    MemberUnban,
    RoleCreate,
    RoleUpdate,
    RoleDelete,
    RolesReorder,
    ChannelCreate,
    ChannelUpdate,
    ChannelDelete,
    ChannelsReorder,
    OverrideUpdate,
    OverrideDelete,
    /// A moderator deleted someone else's message.
    MessageDelete,
    /// Server mute or deafen changed (`details`: the changed fields).
    MemberVoiceUpdate,
    /// Moved to another voice channel (`details.channel_id`).
    MemberMove,
    /// Disconnected from voice.
    MemberDisconnect,
}

impl AuditAction {
    pub const ALL: [AuditAction; 20] = [
        AuditAction::SpaceUpdate,
        AuditAction::SpaceTransfer,
        AuditAction::MemberUpdate,
        AuditAction::MemberKick,
        AuditAction::MemberBan,
        AuditAction::MemberUnban,
        AuditAction::RoleCreate,
        AuditAction::RoleUpdate,
        AuditAction::RoleDelete,
        AuditAction::RolesReorder,
        AuditAction::ChannelCreate,
        AuditAction::ChannelUpdate,
        AuditAction::ChannelDelete,
        AuditAction::ChannelsReorder,
        AuditAction::OverrideUpdate,
        AuditAction::OverrideDelete,
        AuditAction::MessageDelete,
        AuditAction::MemberVoiceUpdate,
        AuditAction::MemberMove,
        AuditAction::MemberDisconnect,
    ];

    /// The snake_case name, as on the wire and in `audit_log.action`.
    pub const fn as_str(self) -> &'static str {
        match self {
            AuditAction::SpaceUpdate => "space_update",
            AuditAction::SpaceTransfer => "space_transfer",
            AuditAction::MemberUpdate => "member_update",
            AuditAction::MemberKick => "member_kick",
            AuditAction::MemberBan => "member_ban",
            AuditAction::MemberUnban => "member_unban",
            AuditAction::RoleCreate => "role_create",
            AuditAction::RoleUpdate => "role_update",
            AuditAction::RoleDelete => "role_delete",
            AuditAction::RolesReorder => "roles_reorder",
            AuditAction::ChannelCreate => "channel_create",
            AuditAction::ChannelUpdate => "channel_update",
            AuditAction::ChannelDelete => "channel_delete",
            AuditAction::ChannelsReorder => "channels_reorder",
            AuditAction::OverrideUpdate => "override_update",
            AuditAction::OverrideDelete => "override_delete",
            AuditAction::MessageDelete => "message_delete",
            AuditAction::MemberVoiceUpdate => "member_voice_update",
            AuditAction::MemberMove => "member_move",
            AuditAction::MemberDisconnect => "member_disconnect",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == name)
    }
}

/// One entry of `GET /spaces/{id}/audit-log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: Id,
    pub actor_id: Id,
    pub action: AuditAction,
    /// The member, role, channel or message acted on.
    pub target_id: Option<Id>,
    /// A JSON object with action-specific details, usually the changed fields.
    pub details: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    All,
    Mentions,
    None,
}

/// Who may start a direct message with you (`PrivacySettings::dm_from`). The server's
/// `privacy.dm_policy` applies first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmFrom {
    #[default]
    Everyone,
    /// People who share a space with you.
    Spaces,
    Nobody,
}

/// Your privacy settings on one instance (`GET`/`PATCH /users/@me/privacy`, `ready.privacy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacySettings {
    pub dm_from: DmFrom,
    /// DMs from people `dm_from` leaves out become message requests (one notification, then
    /// quiet until accepted) instead of being refused.
    pub message_requests: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        PrivacySettings {
            dm_from: DmFrom::Everyone,
            message_requests: true,
        }
    }
}

/// Notification setting for one space or channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSetting {
    /// Space ID or channel ID.
    pub target_id: Id,
    pub level: NotificationLevel,
    /// `None` = not muted, `-1` = muted forever, otherwise Unix ms.
    pub muted_until: Option<i64>,
}

impl NotificationSetting {
    pub const MUTED_FOREVER: i64 = -1;

    pub fn is_muted(&self, now_ms: i64) -> bool {
        match self.muted_until {
            None => false,
            Some(Self::MUTED_FOREVER) => true,
            Some(until) => until > now_ms,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn user(id: u64) -> User {
        User {
            id: Id(id),
            username: format!("user{id}"),
            discriminator: None,
            display_name: None,
            avatar: None,
            status_text: None,
        }
    }

    #[test]
    fn user_json_shape() {
        let mut u = user(5);
        u.avatar = Some(AttachmentRef {
            id: Id(9),
            filename: "a.png".into(),
        });
        let value = serde_json::to_value(&u).unwrap();
        assert_eq!(
            value,
            json!({
                "id": "5", "username": "user5", "display_name": null,
                "avatar": { "id": "9", "filename": "a.png" }, "status_text": null
            })
        );
        assert_eq!(serde_json::from_value::<User>(value).unwrap(), u);
    }

    #[test]
    fn current_user_is_flattened() {
        let cu = CurrentUser {
            user: user(1),
            instance_admin: true,
        };
        let value = serde_json::to_value(&cu).unwrap();
        assert_eq!(value["id"], "1");
        assert_eq!(value["instance_admin"], true);
        assert_eq!(serde_json::from_value::<CurrentUser>(value).unwrap(), cu);
    }

    #[test]
    fn enums_are_snake_case() {
        assert_eq!(json!(ChannelKind::GroupDm), json!("group_dm"));
        assert_eq!(json!(MessageKind::System), json!("system"));
        assert_eq!(json!(Status::Dnd), json!("dnd"));
        assert_eq!(json!(PresenceStatus::Invisible), json!("invisible"));
        assert_eq!(json!(NotificationLevel::Mentions), json!("mentions"));
    }

    #[test]
    fn role_permissions_are_strings() {
        let role = Role {
            id: Id(1),
            space_id: Id(1),
            name: "@everyone".into(),
            color: Some(0xff0000),
            hoist: false,
            mentionable: false,
            position: 0,
            permissions: Permissions::SEND_MESSAGES | Permissions::CONNECT,
        };
        assert!(role.is_everyone());
        let value = serde_json::to_value(&role).unwrap();
        assert_eq!(
            value["permissions"],
            json!(((1u64 << 17) | (1 << 32)).to_string())
        );
        assert_eq!(serde_json::from_value::<Role>(value).unwrap(), role);
    }

    #[test]
    fn message_nonce_is_omitted_when_absent() {
        let mut msg = Message {
            id: Id::from_parts(1000, 0, 0),
            channel_id: Id(2),
            author: user(3),
            content: "hi".into(),
            reply_to: None,
            edited_at: None,
            deleted: false,
            pinned: false,
            kind: MessageKind::Default,
            attachments: vec![],
            reactions: vec![],
            mentions: Mentions::default(),
            nonce: None,
            e2ee: None,
            lock: Default::default(),
        };
        let value = serde_json::to_value(&msg).unwrap();
        assert!(value.get("nonce").is_none());
        assert_eq!(serde_json::from_value::<Message>(value).unwrap(), msg);
        assert_eq!(msg.created_at(), crate::id::HERMES_EPOCH_MS + 1000);

        msg.nonce = Some("abc".into());
        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(value["nonce"], "abc");
    }

    #[test]
    fn audit_action_names_match_serde() {
        for action in AuditAction::ALL {
            assert_eq!(json!(action), json!(action.as_str()));
            assert_eq!(AuditAction::from_name(action.as_str()), Some(action));
        }
        assert_eq!(AuditAction::from_name("nope"), None);
    }

    #[test]
    fn muted() {
        let mut s = NotificationSetting {
            target_id: Id(1),
            level: NotificationLevel::All,
            muted_until: None,
        };
        assert!(!s.is_muted(100));
        s.muted_until = Some(NotificationSetting::MUTED_FOREVER);
        assert!(s.is_muted(100));
        s.muted_until = Some(200);
        assert!(s.is_muted(100));
        assert!(!s.is_muted(200));
    }

    #[test]
    fn presence_invisible_shows_offline() {
        assert_eq!(PresenceStatus::Invisible.visible_status(), Status::Offline);
        assert_eq!(PresenceStatus::Dnd.visible_status(), Status::Dnd);
    }
}
