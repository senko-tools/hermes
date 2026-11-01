//! `/voice/*`, `PATCH /spaces/{id}/voice-states/{uid}` and `POST /channels/{id}/call/ring`
//! (PLAN.md sections 10 and 12).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use hermes_proto::ErrorCode;
use hermes_proto::api::{
    CallRingRequest, QuicEndpoint, VoiceJoinRequest, VoiceJoinResponse, VoiceSelfStateRequest,
    VoiceStatePatchRequest,
};
use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, ChannelKind};
use hermes_proto::permissions::Permissions;
use hermes_proto::voice::{KickReason, TOKEN_LIFETIME_SECS};
use serde_json::{Map, Value, json};

use super::{ApiResult, Error, Json, Path, audit, perms, user_ref};
use crate::auth::Auth;
use crate::state::AppState;

fn disabled() -> Error {
    Error::new(ErrorCode::VoiceDisabled, "Voice is disabled on this server")
}

/// The host clients should connect to: `instance.public_host`, else the host of the request.
fn quic_host(state: &AppState, headers: &HeaderMap) -> String {
    let public = state.config.instance.public_host.trim();
    let host = if public.is_empty() {
        headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost")
    } else {
        public
    };
    strip_port(host).to_owned()
}

/// `host`, `host:port`, `[v6]` or `[v6]:port` → the bare host.
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        // A bare IPv6 address has more than one colon.
        Some((name, port)) if !name.contains(':') && port.parse::<u16>().is_ok() => name,
        _ => host,
    }
}

/// `CONNECT` in a voice channel, DM or group DM; the user limit applies unless the caller has
/// `MOVE_MEMBERS` (`voice.max_users_per_room` always applies).
pub async fn join(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    headers: HeaderMap,
    Json(req): Json<VoiceJoinRequest>,
) -> ApiResult<Json<VoiceJoinResponse>> {
    let Some(info) = state.voice.endpoint().cloned() else {
        return Err(disabled());
    };
    let (bypass_limit, user_limit) = {
        let cache = state.cache();
        let access = perms::channel(&cache, req.channel_id, auth.user_id)?;
        if !access.channel.kind.has_voice() {
            return Err(Error::new(
                ErrorCode::BadRequest,
                "This channel has no voice room",
            ));
        }
        access.require(Permissions::CONNECT)?;
        (
            access.perms.contains(Permissions::MOVE_MEMBERS),
            access.channel.user_limit.unwrap_or(0),
        )
    };
    let others = state.voice.room_size(req.channel_id, auth.user_id);
    let full_by_limit = user_limit > 0 && !bypass_limit && others >= user_limit as usize;
    if full_by_limit || others >= state.config.voice.max_users_per_room as usize {
        return Err(Error::new(
            ErrorCode::VoiceFull,
            "This voice channel is full",
        ));
    }
    let token = state.voice.issue(
        auth.user_id,
        req.channel_id,
        req.self_mute,
        req.self_deaf,
        bypass_limit,
    );
    Ok(Json(VoiceJoinResponse {
        token,
        expires_in_secs: TOKEN_LIFETIME_SECS,
        quic: QuicEndpoint {
            host: quic_host(&state, &headers),
            port: info.port,
            cert_sha256: info.cert_sha256,
        },
    }))
}

/// Leaves voice (if in it). The QUIC connection gets `kicked {disconnected}` and stays open.
pub async fn leave(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<StatusCode> {
    state
        .voice
        .disconnect(&state, auth.user_id, KickReason::Disconnected);
    Ok(StatusCode::NO_CONTENT)
}

/// Self mute/deafen; does nothing while not in voice.
pub async fn update_me(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<VoiceSelfStateRequest>,
) -> ApiResult<StatusCode> {
    state
        .voice
        .set_self_state(&state, auth.user_id, req.self_mute, req.self_deaf);
    Ok(StatusCode::NO_CONTENT)
}

/// `server_mute` needs `MUTE_MEMBERS`, `server_deaf` `DEAFEN_MEMBERS` and `channel_id`
/// `MOVE_MEMBERS` (in the current and the target channel), all checked in the channel the user is
/// in. `channel_id: null` disconnects. Recorded in the audit log.
pub async fn update_state(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, uid)): Path<(Id, String)>,
    Json(req): Json<VoiceStatePatchRequest>,
) -> ApiResult<StatusCode> {
    let user_id = user_ref(&uid, auth.user_id)?;
    let current = {
        let cache = state.cache();
        perms::space(&cache, space_id, auth.user_id)?;
        let not_in_voice = || Error::not_found("voice state");
        let current = cache
            .voice_states
            .get(&user_id)
            .copied()
            .ok_or_else(not_in_voice)?;
        let in_space = cache
            .channels
            .get(&current.channel_id)
            .is_some_and(|c| c.channel.space_id == Some(space_id));
        if !in_space {
            return Err(not_in_voice());
        }
        let access = perms::channel(&cache, current.channel_id, auth.user_id)?;
        if req.server_mute.is_some() {
            access.require(Permissions::MUTE_MEMBERS)?;
        }
        if req.server_deaf.is_some() {
            access.require(Permissions::DEAFEN_MEMBERS)?;
        }
        if req.channel_id.is_some() {
            access.require(Permissions::MOVE_MEMBERS)?;
        }
        if let Some(Some(target)) = req.channel_id {
            let not_voice = || Error::not_found("voice channel");
            let channel = cache.channels.get(&target).ok_or_else(not_voice)?;
            if channel.channel.space_id != Some(space_id)
                || channel.channel.kind != ChannelKind::Voice
            {
                return Err(not_voice());
            }
            perms::require_channel(&cache, target, auth.user_id, Permissions::MOVE_MEMBERS)?;
            let theirs = cache.channel_permissions(target, user_id);
            if !theirs.contains(Permissions::VIEW_CHANNEL | Permissions::CONNECT) {
                return Err(Error::new(
                    ErrorCode::MissingPermission,
                    "That member cannot connect to that channel",
                ));
            }
        }
        current
    };

    let mut tx = state.db.write.begin().await?;
    let mut details = Map::new();
    if let Some(mute) = req.server_mute.filter(|&m| m != current.server_mute) {
        details.insert("server_mute".into(), Value::Bool(mute));
    }
    if let Some(deaf) = req.server_deaf.filter(|&d| d != current.server_deaf) {
        details.insert("server_deaf".into(), Value::Bool(deaf));
    }
    if !details.is_empty() {
        audit::record(
            &mut tx,
            &state,
            space_id,
            auth.user_id,
            AuditAction::MemberVoiceUpdate,
            Some(user_id),
            Value::Object(details),
        )
        .await?;
    }
    let moved = match req.channel_id {
        Some(Some(target)) if target != current.channel_id => Some(Some(target)),
        Some(None) => Some(None),
        _ => None,
    };
    match moved {
        Some(Some(target)) => {
            audit::record(
                &mut tx,
                &state,
                space_id,
                auth.user_id,
                AuditAction::MemberMove,
                Some(user_id),
                json!({ "channel_id": target, "from_channel_id": current.channel_id }),
            )
            .await?;
        }
        Some(None) => {
            audit::record(
                &mut tx,
                &state,
                space_id,
                auth.user_id,
                AuditAction::MemberDisconnect,
                Some(user_id),
                json!({ "channel_id": current.channel_id }),
            )
            .await?;
        }
        None => {}
    }
    tx.commit().await?;

    let voice = &state.voice;
    voice.set_server_state(&state, user_id, req.server_mute, req.server_deaf);
    match moved {
        Some(Some(target)) => {
            let token = voice.issue(user_id, target, current.self_mute, current.self_deaf, true);
            voice.send_moved(user_id, token);
        }
        Some(None) => {
            voice.disconnect(&state, user_id, KickReason::Disconnected);
        }
        None => {}
    }
    Ok(StatusCode::NO_CONTENT)
}

/// DM / group DM only, by a recipient: `call_ring` to the listed recipients (default: all) who
/// are not in the call.
pub async fn ring(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
    Json(req): Json<CallRingRequest>,
) -> ApiResult<StatusCode> {
    if state.voice.endpoint().is_none() {
        return Err(disabled());
    }
    let targets = {
        let cache = state.cache();
        let access =
            perms::require_channel(&cache, channel_id, auth.user_id, Permissions::CONNECT)?;
        if !access.channel.kind.is_private() {
            return Err(Error::new(
                ErrorCode::BadRequest,
                "Only DMs and group DMs can ring",
            ));
        }
        let recipients = &cache.channels[&channel_id].recipient_ids;
        match req.user_ids {
            Some(ids) => ids
                .into_iter()
                .filter(|id| recipients.binary_search(id).is_ok())
                .collect(),
            None => recipients.clone(),
        }
    };
    super::dms::reopen(&state, channel_id).await?;
    state.voice.ring(&state, channel_id, auth.user_id, &targets);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::strip_port;

    #[test]
    fn hosts_lose_their_port() {
        assert_eq!(strip_port("hermes.senko.tools"), "hermes.senko.tools");
        assert_eq!(strip_port("hermes.senko.tools:7700"), "hermes.senko.tools");
        assert_eq!(strip_port("127.0.0.1:80"), "127.0.0.1");
        assert_eq!(strip_port("[::1]:7700"), "::1");
        assert_eq!(strip_port("[::1]"), "::1");
        assert_eq!(strip_port("::1"), "::1");
    }
}
