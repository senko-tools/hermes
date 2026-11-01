//! Gateway (real-time WebSocket) protocol (PLAN.md section 11).
//!
//! Every frame is one JSON envelope `{ "op": "…", "s": 1042, "d": { … } }`.
//! `s` is the sequence number of dispatch events and `null` for control ops. Client ops carry no
//! sequence; the server ignores `s` on them.

use serde::{Deserialize, Serialize};

use crate::id::Id;
use crate::models::{
    Channel, CurrentUser, Member, Message, NotificationSetting, Presence, PresenceStatus,
    ReadState, Role, Space, User, VoiceState,
};

pub use crate::PROTOCOL_VERSION;

/// Default heartbeat interval announced in `hello`.
pub const HEARTBEAT_INTERVAL_MS: u64 = 30_000;

// ---------------------------------------------------------------------------------------------
// Client → server
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// e.g. `"hermes-gui"`.
    pub name: String,
    pub version: String,
    pub os: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identify {
    pub token: String,
    pub protocol_version: u32,
    pub client: ClientInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resume {
    pub token: String,
    pub session_id: String,
    pub last_seq: u64,
}

/// Ops sent by the client. All other actions use REST.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "d", rename_all = "snake_case")]
pub enum ClientMessage {
    Identify(Identify),
    Resume(Resume),
    /// `last_seq` is the last dispatch sequence received (sequences start at 1, 0 = none yet).
    Heartbeat {
        last_seq: u64,
    },
    PresenceUpdate {
        status: PresenceStatus,
    },
}

/// Why a client frame could not be decoded; maps to close codes 4001 and 4002.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnknownOp(String),
    Malformed(String),
}

impl DecodeError {
    pub const fn close_code(&self) -> CloseCode {
        match self {
            DecodeError::UnknownOp(_) => CloseCode::UnknownOp,
            DecodeError::Malformed(_) => CloseCode::DecodeError,
        }
    }
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::UnknownOp(op) => write!(f, "unknown op {op:?}"),
            DecodeError::Malformed(e) => write!(f, "malformed frame: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl ClientMessage {
    pub const OPS: [&'static str; 4] = ["identify", "resume", "heartbeat", "presence_update"];

    /// Decodes a text frame, telling unknown ops apart from malformed frames.
    pub fn decode(text: &str) -> Result<Self, DecodeError> {
        #[derive(Deserialize)]
        struct OpOnly {
            op: String,
        }

        match serde_json::from_str::<Self>(text) {
            Ok(msg) => Ok(msg),
            Err(e) => match serde_json::from_str::<OpOnly>(text) {
                Ok(OpOnly { op }) if !Self::OPS.contains(&op.as_str()) => {
                    Err(DecodeError::UnknownOp(op))
                }
                _ => Err(DecodeError::Malformed(e.to_string())),
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Server → client
// ---------------------------------------------------------------------------------------------

/// One space with everything a member needs, as in `ready` and `space_create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSnapshot {
    pub space: Space,
    pub roles: Vec<Role>,
    /// Only channels the user can `VIEW_CHANNEL`.
    pub channels: Vec<Channel>,
    /// Capped at 1000; larger spaces lazy-load the rest via REST.
    pub members: Vec<Member>,
    pub voice_states: Vec<VoiceState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceInfo {
    pub enabled: bool,
    /// `None` when voice is disabled.
    pub quic_port: Option<u16>,
    /// SHA-256 fingerprint of the voice certificate, `"3F:A9:…"`; `None` when voice is disabled.
    pub cert_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceLimits {
    pub max_upload_mb: u32,
    pub max_message_length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub name: String,
    pub limits: InstanceLimits,
}

/// The full snapshot sent after `identify` (PLAN.md section 11.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    pub session_id: String,
    pub user: CurrentUser,
    pub spaces: Vec<SpaceSnapshot>,
    pub dms: Vec<Channel>,
    pub read_states: Vec<ReadState>,
    /// Users sharing a space or DM with you.
    pub presences: Vec<Presence>,
    pub notification_settings: Vec<NotificationSetting>,
    /// Voice states of DM and group DM calls you are a recipient of (space voice states are in
    /// `spaces[].voice_states`).
    #[serde(default)]
    pub dm_voice_states: Vec<VoiceState>,
    pub voice: VoiceInfo,
    pub instance: InstanceInfo,
}

/// Payload of `voice_state_update`: a full state, or `{user_id, channel_id: null}` when the user
/// left voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VoiceStateUpdate {
    Joined(VoiceState),
    Left {
        user_id: Id,
        /// Always `None`.
        channel_id: Option<Id>,
    },
}

impl VoiceStateUpdate {
    pub const fn left(user_id: Id) -> Self {
        VoiceStateUpdate::Left {
            user_id,
            channel_id: None,
        }
    }

    pub const fn user_id(&self) -> Id {
        match self {
            VoiceStateUpdate::Joined(state) => state.user_id,
            VoiceStateUpdate::Left { user_id, .. } => *user_id,
        }
    }
}

/// Payload of `reaction_add` / `reaction_remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionEvent {
    pub message_id: Id,
    pub channel_id: Id,
    pub emoji: String,
    pub user_id: Id,
}

/// Everything the server sends: control ops (no sequence) and dispatch events (with sequence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "d", rename_all = "snake_case")]
pub enum ServerEvent {
    // Control ops (PLAN.md section 11.6)
    Hello {
        heartbeat_interval_ms: u64,
    },
    HeartbeatAck {},
    Ready(Box<Ready>),
    Resumed {},
    InvalidSession {
        resumable: bool,
    },

    // Dispatch events (PLAN.md section 11.5)
    /// `nonce` is included only for the author's sessions.
    MessageCreate(Message),
    MessageUpdate(Message),
    MessageDelete {
        id: Id,
        channel_id: Id,
    },
    /// Ban with message deletion.
    MessageDeleteBulk {
        ids: Vec<Id>,
        channel_id: Id,
    },
    ReactionAdd(ReactionEvent),
    ReactionRemove(ReactionEvent),
    /// Clients show the indicator for 10 s.
    TypingStart {
        channel_id: Id,
        user_id: Id,
        timestamp: i64,
    },
    /// Own sessions only.
    ReadStateUpdate(ReadState),
    ChannelCreate(Channel),
    ChannelUpdate(Channel),
    ChannelDelete {
        id: Id,
        space_id: Id,
    },
    /// You joined or created a space.
    SpaceCreate(Box<SpaceSnapshot>),
    SpaceUpdate(Space),
    /// Space deleted, or you left / were kicked / were banned.
    SpaceDelete {
        id: Id,
    },
    RoleCreate(Role),
    RoleUpdate(Role),
    RoleDelete {
        id: Id,
        space_id: Id,
    },
    MemberAdd(Member),
    MemberUpdate(Member),
    MemberRemove {
        space_id: Id,
        user_id: Id,
    },
    /// Profile changes, sent to everyone who can see the user.
    UserUpdate(User),
    PresenceUpdate(Presence),
    VoiceStateUpdate(VoiceStateUpdate),
    CallRing {
        channel_id: Id,
        caller_id: Id,
    },
    CallRingStop {
        channel_id: Id,
    },
    DmCreate(Channel),
    /// Recipients changed, rename.
    DmUpdate(Channel),
    /// You left or were removed.
    DmDelete {
        id: Id,
    },
    /// Own sessions only.
    NotificationSettingsUpdate(NotificationSetting),
    /// The setting for a space or channel was removed (back to the default). Own sessions only.
    NotificationSettingsDelete {
        target_id: Id,
    },
}

impl ServerEvent {
    /// Whether this is a dispatch event (carries a sequence number and goes into the resume buffer).
    pub const fn is_dispatch(&self) -> bool {
        !matches!(
            self,
            ServerEvent::Hello { .. }
                | ServerEvent::HeartbeatAck {}
                | ServerEvent::Ready(_)
                | ServerEvent::Resumed {}
                | ServerEvent::InvalidSession { .. }
        )
    }
}

/// A server frame: an event plus its sequence number (`None` for control ops).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerMessage {
    pub s: Option<u64>,
    #[serde(flatten)]
    pub event: ServerEvent,
}

impl ServerMessage {
    pub fn control(event: ServerEvent) -> Self {
        debug_assert!(!event.is_dispatch(), "dispatch event sent as control op");
        ServerMessage { s: None, event }
    }

    pub fn dispatch(seq: u64, event: ServerEvent) -> Self {
        debug_assert!(event.is_dispatch(), "control op sent as dispatch event");
        ServerMessage {
            s: Some(seq),
            event,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Close codes
// ---------------------------------------------------------------------------------------------

/// WebSocket close codes (PLAN.md section 11.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum CloseCode {
    UnknownError = 4000,
    UnknownOp = 4001,
    DecodeError = 4002,
    /// Sent an op before identify.
    NotAuthenticated = 4003,
    AuthenticationFailed = 4004,
    AlreadyAuthenticated = 4005,
    InvalidSeq = 4007,
    RateLimited = 4008,
    /// Session timed out or its queue overflowed.
    SessionTimedOut = 4009,
    UnsupportedProtocolVersion = 4010,
    /// Logged out elsewhere.
    SessionRevoked = 4011,
    ServerRestarting = 4100,
}

/// What the client does after the gateway closed with a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Reconnect with the normal backoff, resuming if possible.
    Reconnect,
    /// Reconnect after this many milliseconds.
    ReconnectAfterMs(u64),
    /// Reconnect and resume the session.
    Resume,
    /// Reconnect with a fresh `identify` (state is stale).
    IdentifyFresh,
    /// Do not reconnect; show the login screen.
    ShowLogin,
    /// Do not reconnect; tell the user to update the client.
    UpdateClient,
}

impl CloseAction {
    pub const fn reconnects(self) -> bool {
        !matches!(self, CloseAction::ShowLogin | CloseAction::UpdateClient)
    }
}

impl CloseCode {
    pub const ALL: [CloseCode; 12] = [
        CloseCode::UnknownError,
        CloseCode::UnknownOp,
        CloseCode::DecodeError,
        CloseCode::NotAuthenticated,
        CloseCode::AuthenticationFailed,
        CloseCode::AlreadyAuthenticated,
        CloseCode::InvalidSeq,
        CloseCode::RateLimited,
        CloseCode::SessionTimedOut,
        CloseCode::UnsupportedProtocolVersion,
        CloseCode::SessionRevoked,
        CloseCode::ServerRestarting,
    ];

    pub const fn code(self) -> u16 {
        self as u16
    }

    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code() == code)
    }

    pub const fn action(self) -> CloseAction {
        match self {
            CloseCode::UnknownError
            | CloseCode::UnknownOp
            | CloseCode::DecodeError
            | CloseCode::NotAuthenticated
            | CloseCode::AlreadyAuthenticated
            | CloseCode::ServerRestarting => CloseAction::Reconnect,
            CloseCode::AuthenticationFailed | CloseCode::SessionRevoked => CloseAction::ShowLogin,
            CloseCode::InvalidSeq => CloseAction::IdentifyFresh,
            CloseCode::RateLimited => CloseAction::ReconnectAfterMs(5000),
            CloseCode::SessionTimedOut => CloseAction::Resume,
            CloseCode::UnsupportedProtocolVersion => CloseAction::UpdateClient,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::tests::user;
    use crate::models::{NotificationLevel, Status};
    use serde_json::{Value, json};

    fn round_trip(msg: &ServerMessage) -> Value {
        let value = serde_json::to_value(msg).unwrap();
        let back: ServerMessage = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&back, msg);
        value
    }

    #[test]
    fn hello_is_a_control_op() {
        let msg = ServerMessage::control(ServerEvent::Hello {
            heartbeat_interval_ms: HEARTBEAT_INTERVAL_MS,
        });
        assert_eq!(
            round_trip(&msg),
            json!({ "op": "hello", "s": null, "d": { "heartbeat_interval_ms": 30000 } })
        );
    }

    #[test]
    fn empty_payloads_are_objects() {
        for (event, op) in [
            (ServerEvent::HeartbeatAck {}, "heartbeat_ack"),
            (ServerEvent::Resumed {}, "resumed"),
        ] {
            let value = round_trip(&ServerMessage::control(event));
            assert_eq!(value, json!({ "op": op, "s": null, "d": {} }));
        }
    }

    #[test]
    fn dispatch_carries_seq() {
        let msg = ServerMessage::dispatch(
            1042,
            ServerEvent::MessageDelete {
                id: Id(1),
                channel_id: Id(2),
            },
        );
        assert_eq!(
            round_trip(&msg),
            json!({ "op": "message_delete", "s": 1042, "d": { "id": "1", "channel_id": "2" } })
        );
    }

    #[test]
    fn dispatch_classification() {
        assert!(!ServerEvent::InvalidSession { resumable: false }.is_dispatch());
        assert!(!ServerEvent::HeartbeatAck {}.is_dispatch());
        assert!(ServerEvent::SpaceDelete { id: Id(1) }.is_dispatch());
        assert!(ServerEvent::UserUpdate(user(1)).is_dispatch());
    }

    #[test]
    fn op_names() {
        let cases: Vec<(ServerEvent, &str)> = vec![
            (
                ServerEvent::InvalidSession { resumable: true },
                "invalid_session",
            ),
            (
                ServerEvent::MessageDeleteBulk {
                    ids: vec![Id(1)],
                    channel_id: Id(2),
                },
                "message_delete_bulk",
            ),
            (
                ServerEvent::ReactionAdd(ReactionEvent {
                    message_id: Id(1),
                    channel_id: Id(2),
                    emoji: "👍".into(),
                    user_id: Id(3),
                }),
                "reaction_add",
            ),
            (
                ServerEvent::TypingStart {
                    channel_id: Id(1),
                    user_id: Id(2),
                    timestamp: 5,
                },
                "typing_start",
            ),
            (
                ServerEvent::ReadStateUpdate(ReadState {
                    channel_id: Id(1),
                    last_read_id: Id(2),
                    mention_count: 0,
                }),
                "read_state_update",
            ),
            (
                ServerEvent::MemberRemove {
                    space_id: Id(1),
                    user_id: Id(2),
                },
                "member_remove",
            ),
            (
                ServerEvent::PresenceUpdate(Presence {
                    user_id: Id(1),
                    status: Status::Idle,
                }),
                "presence_update",
            ),
            (
                ServerEvent::CallRing {
                    channel_id: Id(1),
                    caller_id: Id(2),
                },
                "call_ring",
            ),
            (
                ServerEvent::CallRingStop { channel_id: Id(1) },
                "call_ring_stop",
            ),
            (ServerEvent::DmDelete { id: Id(1) }, "dm_delete"),
            (
                ServerEvent::NotificationSettingsUpdate(NotificationSetting {
                    target_id: Id(1),
                    level: NotificationLevel::None,
                    muted_until: None,
                }),
                "notification_settings_update",
            ),
        ];
        for (event, op) in cases {
            let msg = if event.is_dispatch() {
                ServerMessage::dispatch(1, event)
            } else {
                ServerMessage::control(event)
            };
            assert_eq!(round_trip(&msg)["op"], op);
        }
    }

    #[test]
    fn voice_state_update_joined_and_left() {
        let state = VoiceState {
            user_id: Id(1),
            channel_id: Id(2),
            self_mute: true,
            self_deaf: false,
            server_mute: false,
            server_deaf: false,
            joined_at: 10,
        };
        let joined = ServerMessage::dispatch(
            1,
            ServerEvent::VoiceStateUpdate(VoiceStateUpdate::Joined(state)),
        );
        assert_eq!(round_trip(&joined)["d"]["channel_id"], "2");

        let left = ServerMessage::dispatch(
            2,
            ServerEvent::VoiceStateUpdate(VoiceStateUpdate::left(Id(1))),
        );
        assert_eq!(
            round_trip(&left),
            json!({ "op": "voice_state_update", "s": 2, "d": { "user_id": "1", "channel_id": null } })
        );
        assert_eq!(VoiceStateUpdate::left(Id(7)).user_id(), Id(7));
    }

    #[test]
    fn ready_round_trip() {
        let ready = Ready {
            session_id: "abc".into(),
            user: CurrentUser {
                user: user(1),
                instance_admin: false,
            },
            spaces: vec![SpaceSnapshot {
                space: Space {
                    id: Id(10),
                    name: "s".into(),
                    description: None,
                    icon: None,
                    owner_id: Id(1),
                },
                roles: vec![],
                channels: vec![],
                members: vec![],
                voice_states: vec![],
            }],
            dms: vec![],
            read_states: vec![],
            presences: vec![],
            notification_settings: vec![],
            dm_voice_states: vec![],
            voice: VoiceInfo {
                enabled: true,
                quic_port: Some(7700),
                cert_sha256: Some("3F:A9".into()),
            },
            instance: InstanceInfo {
                name: "Hermes".into(),
                limits: InstanceLimits {
                    max_upload_mb: 25,
                    max_message_length: 4000,
                },
            },
        };
        let value = round_trip(&ServerMessage::control(ServerEvent::Ready(Box::new(ready))));
        assert_eq!(value["op"], "ready");
        assert_eq!(value["s"], Value::Null);
        assert_eq!(value["d"]["user"]["instance_admin"], false);
        assert_eq!(value["d"]["spaces"][0]["space"]["id"], "10");
        assert_eq!(value["d"]["voice"]["quic_port"], 7700);
        assert_eq!(value["d"]["instance"]["limits"]["max_message_length"], 4000);
    }

    #[test]
    fn client_identify_shape() {
        let msg = ClientMessage::Identify(Identify {
            token: "t".into(),
            protocol_version: PROTOCOL_VERSION,
            client: ClientInfo {
                name: "hermes-gui".into(),
                version: "0.1.0".into(),
                os: "windows".into(),
            },
        });
        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            value,
            json!({ "op": "identify", "d": {
                "token": "t", "protocol_version": 1,
                "client": { "name": "hermes-gui", "version": "0.1.0", "os": "windows" }
            }})
        );
        assert_eq!(ClientMessage::decode(&value.to_string()).unwrap(), msg);
    }

    #[test]
    fn client_decode_accepts_seq_field() {
        let msg = ClientMessage::decode(r#"{"op":"heartbeat","s":null,"d":{"last_seq":12}}"#);
        assert_eq!(msg.unwrap(), ClientMessage::Heartbeat { last_seq: 12 });
        let msg = ClientMessage::decode(r#"{"op":"presence_update","d":{"status":"invisible"}}"#);
        assert_eq!(
            msg.unwrap(),
            ClientMessage::PresenceUpdate {
                status: PresenceStatus::Invisible
            }
        );
        let msg = ClientMessage::decode(
            r#"{"op":"resume","d":{"token":"t","session_id":"s","last_seq":3}}"#,
        );
        assert!(matches!(msg, Ok(ClientMessage::Resume(r)) if r.last_seq == 3));
    }

    #[test]
    fn client_decode_errors() {
        let err = ClientMessage::decode(r#"{"op":"nope","d":{}}"#).unwrap_err();
        assert_eq!(err, DecodeError::UnknownOp("nope".into()));
        assert_eq!(err.close_code(), CloseCode::UnknownOp);

        for bad in [
            "not json",
            r#"{"d":{}}"#,
            r#"{"op":"heartbeat","d":{}}"#,
            r#"{"op":"presence_update","d":{"status":"offline"}}"#,
        ] {
            let err = ClientMessage::decode(bad).unwrap_err();
            assert!(matches!(err, DecodeError::Malformed(_)), "{bad}");
            assert_eq!(err.close_code(), CloseCode::DecodeError);
        }
    }

    #[test]
    fn close_codes() {
        for code in CloseCode::ALL {
            assert_eq!(CloseCode::from_code(code.code()), Some(code));
        }
        assert_eq!(CloseCode::from_code(1000), None);
        assert_eq!(CloseCode::from_code(4006), None);
        assert_eq!(CloseCode::SessionTimedOut.code(), 4009);
        assert_eq!(CloseCode::ServerRestarting.code(), 4100);

        let no_reconnect: Vec<_> = CloseCode::ALL
            .into_iter()
            .filter(|c| !c.action().reconnects())
            .collect();
        assert_eq!(
            no_reconnect,
            [
                CloseCode::AuthenticationFailed,
                CloseCode::UnsupportedProtocolVersion,
                CloseCode::SessionRevoked
            ]
        );
        assert_eq!(
            CloseCode::RateLimited.action(),
            CloseAction::ReconnectAfterMs(5000)
        );
        assert_eq!(CloseCode::InvalidSeq.action(), CloseAction::IdentifyFresh);
        assert_eq!(CloseCode::SessionTimedOut.action(), CloseAction::Resume);
    }
}
