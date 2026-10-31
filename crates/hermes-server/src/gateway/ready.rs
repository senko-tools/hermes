//! The `ready` snapshot sent after `identify` (PLAN.md section 11.3).

use hermes_proto::gateway::{InstanceInfo, InstanceLimits, Ready, VoiceInfo};
use hermes_proto::id::Id;
use hermes_proto::models::{Channel, NotificationSetting, ReadState, Status};
use hermes_proto::permissions::Permissions;

use crate::db;
use crate::state::AppState;

/// The parts of `ready` that come from the database, loaded before the cache is read.
#[derive(Debug, Default)]
pub struct Rows {
    pub read_states: Vec<ReadState>,
    pub notification_settings: Vec<NotificationSetting>,
}

pub async fn load_rows(state: &AppState, user_id: Id) -> Result<Rows, sqlx::Error> {
    Ok(Rows {
        read_states: db::gateway::read_states(&state.db.read, user_id).await?,
        notification_settings: db::gateway::notification_settings(&state.db.read, user_id).await?,
    })
}

/// Everything `user_id` sees, from the cache. `None` if the user is not cached.
pub fn build(state: &AppState, user_id: Id, session_id: String, rows: Rows) -> Option<Ready> {
    let config = &state.config;
    let cache = state.cache();
    let user = cache.users.get(&user_id)?.current_user();

    let mut space_ids: Vec<Id> = cache
        .spaces
        .values()
        .filter(|s| s.members.contains_key(&user_id))
        .map(|s| s.space.id)
        .collect();
    space_ids.sort();
    let spaces = space_ids
        .into_iter()
        .filter_map(|id| cache.space_snapshot(id, user_id))
        .collect();

    // Most recently active first.
    let mut dms: Vec<Channel> = cache
        .dm_ids(user_id)
        .filter_map(|id| cache.dm_channel(id))
        .collect();
    dms.sort_by_key(|c| std::cmp::Reverse((c.last_message_id, c.id)));

    let read_states = rows
        .read_states
        .into_iter()
        .filter(|r| {
            cache
                .channel_permissions(r.channel_id, user_id)
                .contains(Permissions::VIEW_CHANNEL)
        })
        .collect();

    // Offline users are left out; clients treat missing users as offline.
    let mut presences: Vec<_> = cache
        .presence_audience(user_id)
        .into_iter()
        .map(|id| cache.presence(id))
        .filter(|p| p.status != Status::Offline)
        .collect();
    presences.sort_by_key(|p| p.user_id);

    let dm_voice_states = {
        let dms: Vec<Id> = cache.dm_ids(user_id).collect();
        let mut states: Vec<_> = cache
            .voice_states
            .values()
            .filter(|v| dms.contains(&v.channel_id))
            .copied()
            .collect();
        states.sort_by_key(|v| v.user_id);
        states
    };
    let endpoint = state.voice.endpoint();
    Some(Ready {
        session_id,
        user,
        spaces,
        dms,
        read_states,
        presences,
        notification_settings: rows.notification_settings,
        dm_voice_states,
        voice: VoiceInfo {
            enabled: endpoint.is_some(),
            quic_port: endpoint.map(|e| e.port),
            cert_sha256: endpoint.map(|e| e.cert_sha256.clone()),
        },
        instance: InstanceInfo {
            name: config.instance.name.clone(),
            limits: InstanceLimits {
                max_upload_mb: config.storage.max_upload_mb,
                max_message_length: config.limits.max_message_length,
            },
        },
    })
}
