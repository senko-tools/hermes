//! A store for tests that do not need a server.
//!
//! I am `me` (1). Space "Friends" (10) has `#general` (11, unread, 2 mentions), the category
//! "Games" (12) with `#chess` (13, read) and the voice channel "Lounge" (14) where Bob (3) is
//! muted, and the top-level `#memes` (15, unread). DMs: with Carol (4, unread) and a group DM
//! "Trio" (21). Known users: me, bob (display name "Bobby"), carol.

use hermes_client_core::proto::gateway::{
    InstanceInfo, InstanceLimits, Ready, SpaceSnapshot, VoiceInfo,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{
    Channel, ChannelKind, CurrentUser, Member, ReadState, Role, Space, User, VoiceState,
};
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::InstanceStore;

pub fn user(id: u64, name: &str, display: Option<&str>) -> User {
    User {
        id: Id(id),
        username: name.into(),
        display_name: display.map(str::to_owned),
        avatar: None,
        status_text: None,
    }
}

fn channel(id: u64, kind: ChannelKind, name: &str, parent: Option<u64>, position: i32) -> Channel {
    Channel {
        id: Id(id),
        space_id: Some(Id(10)),
        kind,
        parent_id: parent.map(Id),
        name: Some(name.into()),
        topic: None,
        position,
        bitrate: None,
        user_limit: None,
        slowmode_secs: 0,
        overrides: vec![],
        recipients: vec![],
        owner_id: None,
        last_message_id: None,
    }
}

fn member(user: User) -> Member {
    Member {
        space_id: Id(10),
        user,
        nickname: None,
        role_ids: vec![],
        joined_at: 0,
    }
}

pub fn store() -> InstanceStore {
    let me = user(1, "me", None);
    let bob = user(3, "bob", Some("Bobby"));
    let carol = user(4, "carol", None);
    let mut general = channel(11, ChannelKind::Text, "general", None, 0);
    general.last_message_id = Some(Id(100));
    let mut chess = channel(13, ChannelKind::Text, "chess", Some(12), 0);
    chess.last_message_id = Some(Id(90));
    let mut memes = channel(15, ChannelKind::Text, "memes", None, 2);
    memes.last_message_id = Some(Id(95));
    let dm = Channel {
        space_id: None,
        kind: ChannelKind::Dm,
        name: None,
        recipients: vec![me.clone(), carol.clone()],
        last_message_id: Some(Id(110)),
        ..channel(20, ChannelKind::Dm, "", None, 0)
    };
    let group = Channel {
        space_id: None,
        kind: ChannelKind::GroupDm,
        name: Some("Trio".into()),
        recipients: vec![me.clone(), bob.clone(), carol.clone()],
        last_message_id: Some(Id(105)),
        ..channel(21, ChannelKind::GroupDm, "", None, 0)
    };
    let ready = Ready {
        session_id: "s".into(),
        user: CurrentUser {
            user: me.clone(),
            instance_admin: false,
        },
        spaces: vec![SpaceSnapshot {
            space: Space {
                id: Id(10),
                name: "Friends".into(),
                description: None,
                icon: None,
                owner_id: Id(1),
            },
            roles: vec![Role {
                id: Id(10),
                space_id: Id(10),
                name: "@everyone".into(),
                color: None,
                hoist: false,
                mentionable: false,
                position: 0,
                permissions: Permissions::DEFAULT_EVERYONE,
            }],
            channels: vec![
                general,
                channel(12, ChannelKind::Category, "Games", None, 1),
                chess,
                channel(14, ChannelKind::Voice, "Lounge", Some(12), 1),
                memes,
            ],
            members: vec![member(me), member(bob)],
            voice_states: vec![VoiceState {
                user_id: Id(3),
                channel_id: Id(14),
                self_mute: true,
                self_deaf: false,
                server_mute: false,
                server_deaf: false,
                joined_at: 5,
            }],
        }],
        dms: vec![dm, group],
        read_states: vec![
            ReadState {
                channel_id: Id(11),
                last_read_id: Id(50),
                mention_count: 2,
            },
            ReadState {
                channel_id: Id(13),
                last_read_id: Id(90),
                mention_count: 0,
            },
            ReadState {
                channel_id: Id(21),
                last_read_id: Id(105),
                mention_count: 0,
            },
        ],
        presences: vec![],
        notification_settings: vec![],
        dm_voice_states: vec![],
        voice: VoiceInfo {
            enabled: true,
            quic_port: Some(7700),
            cert_sha256: None,
        },
        instance: InstanceInfo {
            name: "Test".into(),
            limits: InstanceLimits {
                max_upload_mb: 25,
                max_message_length: 4000,
            },
        },
    };
    let mut store = InstanceStore::default();
    store.apply_ready(ready);
    store
}
