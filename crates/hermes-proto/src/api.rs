//! REST request and response bodies (PLAN.md section 10) that are not plain models.

use serde::{Deserialize, Deserializer, Serialize};

use crate::gateway::InstanceLimits;
use crate::id::Id;
use crate::models::{ChannelKind, CurrentUser, Message, NotificationLevel, User};
use crate::permissions::Permissions;

/// A field of a `PATCH` body that can be cleared: absent = unchanged (`None`), `null` = clear
/// (`Some(None)`), a value = set (`Some(Some(v))`). Use with
/// `#[serde(default, deserialize_with = "patch", skip_serializing_if = "Option::is_none")]`.
pub type Patch<T> = Option<Option<T>>;

/// Deserializes a present field (even `null`) as `Some`, so a missing field stays `None`.
pub fn patch<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Patch<T>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// `?before&after&limit` of list endpoints (PLAN.md section 10, "Pagination").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Pagination {
    pub const DEFAULT_LIMIT: u32 = 50;
    pub const MAX_LIMIT: u32 = 100;

    /// `limit` with the default applied, clamped to `1..=MAX_LIMIT`.
    pub fn limit(&self) -> u32 {
        self.limit
            .unwrap_or(Self::DEFAULT_LIMIT)
            .clamp(1, Self::MAX_LIMIT)
    }
}

/// `registration.mode` of an instance: who may create an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    Open,
    #[default]
    InviteOnly,
    Closed,
}

/// `GET /instance` (no auth): what a client shows when adding an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceResponse {
    pub name: String,
    pub description: String,
    /// Server version (`CARGO_PKG_VERSION`).
    pub version: String,
    pub protocol_version: u32,
    pub registration_mode: RegistrationMode,
    pub limits: InstanceLimits,
    pub voice: InstanceVoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceVoice {
    pub enabled: bool,
}

/// `POST /auth/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<String>,
    pub device_name: String,
}

/// `POST /auth/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub device_name: String,
}

/// Response of register and login. `token` is sent as `Authorization: Bearer <token>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: CurrentUser,
}

/// One entry of `GET /auth/sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: Id,
    pub device_name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    /// The session making the request.
    pub current: bool,
}

/// `POST /users/@me/password`. Revokes all other sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// `PATCH /users/@me`. A field set to `null` clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMeRequest {
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Patch<String>,
    /// From an image upload.
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_id: Patch<Id>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub status_text: Patch<String>,
}

impl UpdateMeRequest {
    pub const DISPLAY_NAME_CHARS: (usize, usize) = (1, 32);
    pub const STATUS_TEXT_CHARS: usize = 128;
}

/// `GET /users/{id}`: the user and the spaces we share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    #[serde(flatten)]
    pub user: User,
    pub mutual_space_ids: Vec<Id>,
}

/// `PUT /users/@me/notification-settings/{target_id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSettingRequest {
    pub level: NotificationLevel,
    /// `None` = not muted, `-1` = forever, otherwise until this Unix ms.
    #[serde(default)]
    pub muted_until: Option<i64>,
}

/// `GET /search`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The answer of `GET /search`: newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub messages: Vec<Message>,
    /// All matches the user may see (the page holds at most `limit`).
    pub total: u64,
}

/// `GET /admin/users` entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUser {
    #[serde(flatten)]
    pub user: User,
    pub disabled: bool,
    pub instance_admin: bool,
}

/// `PATCH /admin/users/{id}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUserPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_admin: Option<bool>,
}

/// `POST /spaces/{id}/invites`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInviteRequest {
    /// `None` or `0` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// `None` = 7 days, `0` = never expires, at most 30 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
}

impl CreateInviteRequest {
    /// Lifetime used when `expires_in_secs` is absent.
    pub const DEFAULT_EXPIRES_IN_SECS: u64 = 7 * 24 * 60 * 60;
    /// Longest lifetime other than "never".
    pub const MAX_EXPIRES_IN_SECS: u64 = 30 * 24 * 60 * 60;
}

/// `POST /spaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSpaceRequest {
    pub name: String,
}

/// `PATCH /spaces/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSpaceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Patch<String>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon_id: Patch<Id>,
}

/// `POST /spaces/{id}/transfer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSpaceRequest {
    pub user_id: Id,
}

/// `PATCH /spaces/{id}/members/{uid}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMemberRequest {
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub nickname: Patch<String>,
    /// The complete new list of explicitly assigned roles (never `@everyone`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_ids: Option<Vec<Id>>,
}

/// `PUT /spaces/{id}/bans/{uid}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Soft-deletes the user's messages in the space from the last N hours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_messages_hours: Option<u32>,
}

impl BanRequest {
    pub const MAX_DELETE_MESSAGES_HOURS: u32 = 168;
}

/// `POST /spaces/{id}/roles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoist: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    /// Empty when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
}

/// `PATCH /spaces/{id}/roles/{rid}`. The position changes only through `PUT …/roles/order`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRoleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub color: Patch<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoist: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
}

/// One entry of `PUT /spaces/{id}/roles/order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolePosition {
    pub id: Id,
    pub position: i32,
}

/// `POST /spaces/{id}/channels`. Only `text`, `voice` and `category` are allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateChannelRequest {
    pub kind: ChannelKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Voice only; `voice.default_bitrate` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
    /// Voice only; absent or 0 = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_limit: Option<u32>,
}

/// `PATCH /channels/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateChannelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub topic: Patch<String>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Patch<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub user_limit: Patch<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slowmode_secs: Option<u32>,
}

impl UpdateChannelRequest {
    pub const MAX_SLOWMODE_SECS: u32 = 6 * 60 * 60;
}

/// One entry of `PUT /spaces/{id}/channels/order`. `parent_id`: absent = unchanged, `null` = top
/// level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPosition {
    pub id: Id,
    pub position: i32,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Patch<Id>,
}

/// `PUT /channels/{id}/overrides/{role_id}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideRequest {
    pub allow: Permissions,
    pub deny: Permissions,
}

/// `GET /channels/{id}/messages`: at most one of `before`, `after` and `around`. Without any,
/// the newest messages. Results are oldest first ("newest last").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// The message itself plus messages before and after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl HistoryQuery {
    /// `limit` with the default applied, clamped to `1..=Pagination::MAX_LIMIT`.
    pub fn limit(&self) -> u32 {
        Pagination {
            limit: self.limit,
            ..Pagination::default()
        }
        .limit()
    }
}

/// `POST /channels/{id}/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageRequest {
    /// Raw markup; may be empty only when there are attachments.
    #[serde(default)]
    pub content: String,
    /// Client-generated (a UUID), echoed back to the author's sessions.
    pub nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_id: Option<Id>,
    /// Own uploads that are not attached to a message yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ids: Vec<Id>,
}

impl SendMessageRequest {
    pub const MAX_NONCE_CHARS: usize = 64;
    pub const MAX_ATTACHMENTS: usize = 10;
}

/// `PATCH /channels/{id}/messages/{mid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditMessageRequest {
    pub content: String,
}

/// `POST /channels/{id}/ack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckRequest {
    pub message_id: Id,
}

/// `POST /dms`: one other user reuses (or creates) the 1:1 DM, two or more create a group DM.
/// The caller is implicit; listing them is allowed and ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDmRequest {
    pub recipient_ids: Vec<Id>,
}

/// `POST /voice/join`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceJoinRequest {
    pub channel_id: Id,
    #[serde(default)]
    pub self_mute: bool,
    #[serde(default)]
    pub self_deaf: bool,
}

/// Where to connect for voice: `host:port` over UDP, certificate pinned to `cert_sha256`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuicEndpoint {
    pub host: String,
    pub port: u16,
    /// `"3F:A9:…"`.
    pub cert_sha256: String,
}

/// Answer to `POST /voice/join`: a single-use token for `auth` or `switch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceJoinResponse {
    pub token: String,
    pub expires_in_secs: u64,
    pub quic: QuicEndpoint,
}

/// `PATCH /voice/@me`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSelfStateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_mute: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_deaf: Option<bool>,
}

/// `PATCH /spaces/{id}/voice-states/{uid}`. `channel_id: null` disconnects the user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceStatePatchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_mute: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_deaf: Option<bool>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub channel_id: Patch<Id>,
}

/// `POST /channels/{id}/call/ring`. Without `user_ids`, every recipient not in the call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRingRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_ids: Option<Vec<Id>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::models::User;

    #[test]
    fn register_invite_code_is_optional() {
        let req: RegisterRequest = serde_json::from_value(json!({
            "username": "senko", "password": "hunter22", "device_name": "PC"
        }))
        .unwrap();
        assert_eq!(req.invite_code, None);
        assert!(
            serde_json::to_value(&req)
                .unwrap()
                .get("invite_code")
                .is_none()
        );
    }

    #[test]
    fn auth_response_flattens_the_user() {
        let res = AuthResponse {
            token: "hms_x".into(),
            user: CurrentUser {
                user: User {
                    id: Id(5),
                    username: "senko".into(),
                    display_name: None,
                    avatar: None,
                    status_text: None,
                },
                instance_admin: true,
            },
        };
        let value = serde_json::to_value(&res).unwrap();
        assert_eq!(value["user"]["id"], "5");
        assert_eq!(value["user"]["instance_admin"], true);
        assert_eq!(serde_json::from_value::<AuthResponse>(value).unwrap(), res);
    }

    #[test]
    fn patch_fields_tell_absent_from_null() {
        let req: UpdateSpaceRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!(req, UpdateSpaceRequest::default());
        let req: UpdateSpaceRequest =
            serde_json::from_value(json!({ "description": null, "icon_id": "7" })).unwrap();
        assert_eq!(req.description, Some(None));
        assert_eq!(req.icon_id, Some(Some(Id(7))));
        assert_eq!(req.name, None);
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({ "description": null, "icon_id": "7" })
        );
    }

    #[test]
    fn channel_position_parent_is_a_patch() {
        let p: ChannelPosition =
            serde_json::from_value(json!({ "id": "1", "position": 2 })).unwrap();
        assert_eq!(p.parent_id, None);
        let p: ChannelPosition =
            serde_json::from_value(json!({ "id": "1", "position": 2, "parent_id": null })).unwrap();
        assert_eq!(p.parent_id, Some(None));
    }

    #[test]
    fn pagination_limit_is_clamped() {
        assert_eq!(Pagination::default().limit(), 50);
        let page = |limit| Pagination {
            limit: Some(limit),
            ..Pagination::default()
        };
        assert_eq!(page(0).limit(), 1);
        assert_eq!(page(500).limit(), 100);
    }

    #[test]
    fn send_message_defaults() {
        let req: SendMessageRequest = serde_json::from_value(json!({ "nonce": "n1" })).unwrap();
        assert_eq!(req.content, "");
        assert!(req.attachment_ids.is_empty());
        assert_eq!(req.reply_to_id, None);
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({ "content": "", "nonce": "n1" })
        );
        assert!(serde_json::from_value::<SendMessageRequest>(json!({ "content": "hi" })).is_err());
    }

    #[test]
    fn create_invite_defaults() {
        let req: CreateInviteRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!(req, CreateInviteRequest::default());
        assert_eq!(serde_json::to_value(&req).unwrap(), json!({}));
    }
}
