//! The keys of encrypted channels (docs/e2ee.md). Everything here is sealed for specific devices,
//! so the server only checks who may hand what to whom. Clients keep the real protection: an
//! automatic handover trusts this server's word on who is a member, an approved one does not.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::e2ee::{
    ChannelKeysQuery, ChannelKeysResponse, DeliverKeysRequest, KeyHandover, KeyMembers, KeyPolicy,
    KeyPolicyOverride, KeyRequest, MAX_CHANNEL_KEYS, RequestKeysRequest, SealedChannelKey,
    StartEpochRequest,
};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;

use super::{ApiResult, Error, Json, Path, perms, validate};
use crate::auth::Auth;
use crate::db;
use crate::gateway::bus::Audience;
use crate::state::{AppState, Cache};
use crate::time::now_ms;

/// Longest base64 or ID value in a sealed key.
const MAX_FIELD_CHARS: usize = 256;

/// The server of a channel `user` can read. Anything else has no keys.
fn space_of(cache: &Cache, channel: Id, user: Id) -> ApiResult<Id> {
    let access = perms::channel(cache, channel, user)?;
    access.require(Permissions::VIEW_CHANNEL)?;
    match access.channel.space_id {
        Some(space) if access.channel.kind.has_messages() => Ok(space),
        _ => Err(Error::new(
            ErrorCode::BadRequest,
            "This channel does not use channel keys",
        )),
    }
}

fn check_keys(cache: &Cache, channel: Id, sender: Id, keys: &[SealedChannelKey]) -> ApiResult<()> {
    if keys.len() > MAX_CHANNEL_KEYS {
        return Err(validate::field_error("keys", "are too many"));
    }
    for key in keys {
        let fields = [
            &key.device,
            &key.sender_device,
            &key.eph,
            &key.key,
            &key.sig,
        ];
        if key.sender_id != sender
            || fields
                .iter()
                .any(|f| f.is_empty() || f.len() > MAX_FIELD_CHARS)
        {
            return Err(validate::field_error("keys", "contains a malformed key"));
        }
        let can_read = cache
            .channel_permissions(channel, key.user_id)
            .contains(Permissions::VIEW_CHANNEL);
        if !can_read {
            return Err(validate::field_error(
                "keys",
                format!("{} cannot read this channel", key.user_id),
            ));
        }
    }
    Ok(())
}

/// Tells each person who got a key.
fn publish_delivered(state: &AppState, channel: Id, keys: &[SealedChannelKey]) {
    let users: HashSet<Id> = keys.iter().map(|k| k.user_id).collect();
    for user_id in users {
        state.publish(
            ServerEvent::ChannelKeysDelivered {
                channel_id: channel,
            },
            Audience::User(user_id),
        );
    }
}

/// The current epoch, this device's keys, the rules, and whether the key has to be replaced.
pub async fn get(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel): Path<Id>,
    Query(query): Query<ChannelKeysQuery>,
) -> ApiResult<Json<ChannelKeysResponse>> {
    let space = space_of(&state.cache(), channel, auth.user_id)?;
    let current_epoch = db::channel_keys::current_epoch(&state.db.read, channel).await?;
    let keys =
        db::channel_keys::keys_for_device(&state.db.read, channel, auth.user_id, &query.device)
            .await?;
    let policy = db::channel_keys::policy(&state.db.read, channel, space).await?;
    let holders = db::channel_keys::holders(&state.db.read, channel, current_epoch).await?;
    let rotate_needed = {
        let cache = state.cache();
        holders.iter().any(|(user, live)| {
            !live
                || !cache
                    .channel_permissions(channel, *user)
                    .contains(Permissions::VIEW_CHANNEL)
        })
    };
    Ok(Json(ChannelKeysResponse {
        current_epoch,
        rotate_needed,
        policy,
        keys,
    }))
}

/// Who can read the channel, and who of them holds its current key.
pub async fn members(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel): Path<Id>,
) -> ApiResult<Json<KeyMembers>> {
    let users: Vec<Id> = {
        let cache = state.cache();
        space_of(&cache, channel, auth.user_id)?;
        let mut users: Vec<Id> = cache.channel_viewers(channel).into_iter().collect();
        users.sort();
        users
    };
    let current = db::channel_keys::current_epoch(&state.db.read, channel).await?;
    let holders = db::channel_keys::holders(&state.db.read, channel, current)
        .await?
        .into_iter()
        .map(|(user, _)| user)
        .collect();
    Ok(Json(KeyMembers { users, holders }))
}

/// Starts the next epoch with its key for everyone it is meant for.
pub async fn start_epoch(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel): Path<Id>,
    Json(req): Json<StartEpochRequest>,
) -> ApiResult<StatusCode> {
    {
        let cache = state.cache();
        space_of(&cache, channel, auth.user_id)?;
        // Replacing the key stops everyone's sends for a moment, so reading is not enough.
        let perms = cache.channel_permissions(channel, auth.user_id);
        if !perms.intersects(Permissions::SEND_MESSAGES | Permissions::MANAGE_SPACE) {
            return Err(Error::new(
                ErrorCode::MissingPermission,
                "You cannot replace this channel's key",
            ));
        }
        check_keys(&cache, channel, auth.user_id, &req.keys)?;
    }
    if req.keys.iter().any(|k| k.epoch != req.epoch) {
        return Err(validate::field_error("keys", "must all be of this epoch"));
    }
    let mut tx = state.db.write.begin().await?;
    if !db::channel_keys::start_epoch(&mut tx, channel, req.epoch, auth.user_id, now_ms()).await? {
        return Err(Error::new(
            ErrorCode::Conflict,
            "Someone else replaced this channel's key first",
        ));
    }
    db::channel_keys::store_keys(&mut tx, channel, &req.keys).await?;
    tx.commit().await?;
    publish_delivered(&state, channel, &req.keys);
    state.publish(
        ServerEvent::ChannelKeyEpoch {
            channel_id: channel,
            epoch: req.epoch,
        },
        Audience::Channel(channel),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Hands keys of existing epochs to devices. In an approved channel, only for those who may
/// approve.
pub async fn deliver(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel): Path<Id>,
    Json(req): Json<DeliverKeysRequest>,
) -> ApiResult<StatusCode> {
    let space = {
        let cache = state.cache();
        let space = space_of(&cache, channel, auth.user_id)?;
        check_keys(&cache, channel, auth.user_id, &req.keys)?;
        space
    };
    let policy = db::channel_keys::policy(&state.db.read, channel, space).await?;
    if policy.handover == KeyHandover::Approved {
        let cache = state.cache();
        perms::require_space(&cache, space, auth.user_id, Permissions::MANAGE_SPACE)?;
    }
    for epoch in req.keys.iter().map(|k| k.epoch).collect::<HashSet<_>>() {
        if !db::channel_keys::has_epoch(&state.db.read, channel, epoch).await? {
            return Err(validate::field_error("keys", "name an unknown epoch"));
        }
    }
    let mut tx = state.db.write.begin().await?;
    db::channel_keys::store_keys(&mut tx, channel, &req.keys).await?;
    tx.commit().await?;
    publish_delivered(&state, channel, &req.keys);
    Ok(StatusCode::NO_CONTENT)
}

/// This device has no keys for the channel: asks whoever can hand them over.
pub async fn request(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel): Path<Id>,
    Json(req): Json<RequestKeysRequest>,
) -> ApiResult<StatusCode> {
    space_of(&state.cache(), channel, auth.user_id)?;
    let mine = db::devices::of_user(&state.db.read, auth.user_id).await?;
    if !mine.iter().any(|d| d.device_id == req.device && !d.revoked) {
        return Err(validate::field_error("device", "is not a device of yours"));
    }
    let new = db::channel_keys::add_request(
        &state.db.write,
        channel,
        auth.user_id,
        &req.device,
        now_ms(),
    )
    .await?;
    if new {
        state.publish(
            ServerEvent::ChannelKeyRequest(KeyRequest {
                channel_id: channel,
                user_id: auth.user_id,
                device: req.device,
            }),
            Audience::Channel(channel),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The devices waiting for keys of channels I can read, so they are served when I come online.
pub async fn requests(
    State(state): State<Arc<AppState>>,
    auth: Auth,
) -> ApiResult<Json<Vec<KeyRequest>>> {
    let all = db::channel_keys::all_requests(&state.db.read).await?;
    let cache = state.cache();
    let can_read = |channel, user| {
        cache
            .channel_permissions(channel, user)
            .contains(Permissions::VIEW_CHANNEL)
    };
    Ok(Json(
        all.into_iter()
            .filter(|r| can_read(r.channel_id, auth.user_id) && can_read(r.channel_id, r.user_id))
            .collect(),
    ))
}

/// A server's rules for its channels.
pub async fn space_policy(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space): Path<Id>,
) -> ApiResult<Json<KeyPolicy>> {
    perms::space(&state.cache(), space, auth.user_id)?;
    Ok(Json(
        db::channel_keys::space_policy(&state.db.read, space).await?,
    ))
}

/// Only the owner changes what a whole server does.
pub async fn set_space_policy(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space): Path<Id>,
    Json(policy): Json<KeyPolicy>,
) -> ApiResult<StatusCode> {
    {
        let cache = state.cache();
        if !perms::space(&cache, space, auth.user_id)?.is_owner() {
            return Err(Error::new(
                ErrorCode::MissingPermission,
                "Only the owner can change this",
            ));
        }
    }
    db::channel_keys::set_space_policy(&state.db.write, space, policy).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// What a channel changes about its server's rules.
pub async fn channel_policy(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel): Path<Id>,
) -> ApiResult<Json<KeyPolicyOverride>> {
    space_of(&state.cache(), channel, auth.user_id)?;
    Ok(Json(
        db::channel_keys::channel_policy(&state.db.read, channel).await?,
    ))
}

/// The owner or an administrator decides for one channel.
pub async fn set_channel_policy(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel): Path<Id>,
    Json(policy): Json<KeyPolicyOverride>,
) -> ApiResult<StatusCode> {
    {
        let cache = state.cache();
        let space = space_of(&cache, channel, auth.user_id)?;
        let access = perms::space(&cache, space, auth.user_id)?;
        if !access.is_owner() {
            access.require(Permissions::ADMINISTRATOR)?;
        }
    }
    db::channel_keys::set_channel_policy(&state.db.write, channel, policy).await?;
    Ok(StatusCode::NO_CONTENT)
}
