//! `/dms`: 1:1 DMs and group DMs (PLAN.md sections 8.2 and 10), plus renaming and leaving group
//! DMs through `/channels/{id}`. Messages in DMs use the normal message endpoints.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{CreateDmRequest, UpdateChannelRequest};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{Channel, ChannelKind};
use hermes_proto::permissions::Permissions;

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, perms, user_ref};
use crate::auth::Auth;
use crate::config::DmPolicy;
use crate::db;
use crate::gateway::bus::Audience;
use crate::state::{AppState, Cache};
use crate::time::now_ms;

/// The caller's DMs and group DMs, most recently active first.
pub async fn list(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<Channel>>> {
    let cache = state.cache();
    let mut dms: Vec<Channel> = cache
        .dm_ids(auth.user_id)
        .filter_map(|id| cache.dm_channel(id))
        .collect();
    dms.sort_by_key(|c| std::cmp::Reverse((c.last_message_id, c.id)));
    Ok(Json(dms))
}

/// `dm_policy`: with `shared_space`, the caller must share a space with `other`.
fn check_policy(state: &AppState, cache: &Cache, user_id: Id, other: Id) -> ApiResult<()> {
    match state.config.privacy.dm_policy {
        DmPolicy::Anyone => Ok(()),
        DmPolicy::SharedSpace if cache.shares_space(user_id, other) => Ok(()),
        DmPolicy::SharedSpace => Err(Error::new(
            ErrorCode::MissingPermission,
            "You can only message users who share a space with you",
        )),
    }
}

fn group_too_large(max: u32) -> Error {
    Error::new(
        ErrorCode::BadRequest,
        format!("A group DM can have at most {max} people"),
    )
}

/// One other user: their existing 1:1 DM, or a new one. Two or more: a new group DM owned by the
/// caller, at most `limits.max_group_dm_size` people including the caller.
pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<CreateDmRequest>,
) -> ApiResult<Json<Channel>> {
    let mut others = req.recipient_ids;
    others.sort();
    others.dedup();
    others.retain(|&id| id != auth.user_id);
    if others.is_empty() {
        return Err(validate::field_error(
            "recipient_ids",
            "must name at least one other user",
        ));
    }
    let max = state.config.limits.max_group_dm_size;
    if others.len() > 1 && others.len() + 1 > max as usize {
        return Err(group_too_large(max));
    }

    let _writes = state.writes.lock().await;
    {
        let cache = state.cache();
        for &other in &others {
            if !cache.users.contains_key(&other) {
                return Err(validate::field_error(
                    "recipient_ids",
                    format!("unknown user {other}"),
                ));
            }
            check_policy(&state, &cache, auth.user_id, other)?;
        }
        if let [other] = others[..] {
            let existing = cache.dm_ids(auth.user_id).find(|id| {
                let c = &cache.channels[id];
                c.channel.kind == ChannelKind::Dm && c.recipient_ids.binary_search(&other).is_ok()
            });
            if let Some(dm) = existing.and_then(|id| cache.dm_channel(id)) {
                return Ok(Json(dm));
            }
        }
    }

    let group = others.len() > 1;
    let channel = Channel {
        id: state.ids.next(),
        space_id: None,
        kind: if group {
            ChannelKind::GroupDm
        } else {
            ChannelKind::Dm
        },
        parent_id: None,
        name: None,
        topic: None,
        position: 0,
        bitrate: None,
        user_limit: None,
        slowmode_secs: 0,
        overrides: Vec::new(),
        recipients: Vec::new(),
        owner_id: group.then_some(auth.user_id),
        last_message_id: None,
    };
    let mut recipients = others;
    recipients.push(auth.user_id);
    let mut tx = state.db.write.begin().await?;
    db::spaces::insert_channel(&mut tx, &channel, now_ms()).await?;
    for &user_id in &recipients {
        db::dms::add_recipient(&mut tx, channel.id, user_id).await?;
    }
    tx.commit().await?;
    let dm = {
        let mut cache = state.cache_mut();
        cache.insert_channel(channel.clone(), recipients.clone());
        cache.dm_channel(channel.id)
    }
    .ok_or_else(|| Error::internal("new DM missing from the cache"))?;
    state.publish(
        ServerEvent::DmCreate(dm.clone()),
        Audience::Users(recipients),
    );
    Ok(Json(dm))
}

/// A group DM the caller is a recipient of. `NOT_FOUND` if they are not; `BAD_REQUEST` for a
/// 1:1 DM, whose recipients never change.
fn group_dm(cache: &Cache, channel_id: Id, user_id: Id) -> ApiResult<(Channel, Vec<Id>)> {
    let access = perms::channel(cache, channel_id, user_id)?;
    match access.channel.kind {
        ChannelKind::GroupDm => Ok((
            access.channel.clone(),
            cache.channels[&channel_id].recipient_ids.clone(),
        )),
        ChannelKind::Dm => Err(Error::new(
            ErrorCode::BadRequest,
            "The recipients of a 1:1 DM cannot change",
        )),
        _ => Err(Error::not_found("channel")),
    }
}

/// Group DMs only; any recipient may add people. Adding a current recipient is not an error.
pub async fn add_recipient(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, user_id)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let recipients = {
        let cache = state.cache();
        let (_, recipients) = group_dm(&cache, channel_id, auth.user_id)?;
        if !cache.users.contains_key(&user_id) {
            return Err(Error::not_found("user"));
        }
        if recipients.contains(&user_id) {
            return Ok(StatusCode::NO_CONTENT);
        }
        let max = state.config.limits.max_group_dm_size;
        if recipients.len() >= max as usize {
            return Err(group_too_large(max));
        }
        check_policy(&state, &cache, auth.user_id, user_id)?;
        recipients
    };

    let mut conn = state.db.write.acquire().await?;
    db::dms::add_recipient(&mut conn, channel_id, user_id).await?;
    drop(conn);
    let dm = {
        let mut cache = state.cache_mut();
        if let Some(c) = cache.channels.get_mut(&channel_id) {
            c.recipient_ids.push(user_id);
            c.recipient_ids.sort();
        }
        cache.dm_channel(channel_id)
    };
    if let Some(dm) = dm {
        state.publish(ServerEvent::DmCreate(dm.clone()), Audience::User(user_id));
        state.publish(ServerEvent::DmUpdate(dm), Audience::Users(recipients));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Group DMs only: the owner removes others; anyone removes themselves (`{uid}` may be `@me`).
pub async fn remove_recipient(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, uid)): Path<(Id, String)>,
) -> ApiResult<StatusCode> {
    let user_id = user_ref(&uid, auth.user_id)?;
    remove(&state, auth.user_id, channel_id, user_id).await
}

/// `DELETE /channels/{id}` on a DM: leaves a group DM.
pub async fn leave(state: Arc<AppState>, auth: Auth, channel_id: Id) -> ApiResult<StatusCode> {
    remove(&state, auth.user_id, channel_id, auth.user_id).await
}

/// Removes `user_id` from a group DM. When the owner leaves, the remaining recipient with the
/// lowest ID becomes the owner; when the last recipient leaves, the group DM is deleted.
async fn remove(state: &AppState, actor: Id, channel_id: Id, user_id: Id) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let (remaining, new_owner) = {
        let cache = state.cache();
        let (channel, recipients) = group_dm(&cache, channel_id, actor)?;
        if user_id != actor && channel.owner_id != Some(actor) {
            return Err(Error::new(
                ErrorCode::MissingPermission,
                "Only the owner of the group DM can remove others",
            ));
        }
        if !recipients.contains(&user_id) {
            return Err(Error::not_found("recipient"));
        }
        let remaining: Vec<Id> = recipients.into_iter().filter(|&id| id != user_id).collect();
        let new_owner = if channel.owner_id == Some(user_id) {
            remaining.first().copied()
        } else {
            None
        };
        (remaining, new_owner)
    };

    let mut tx = state.db.write.begin().await?;
    if remaining.is_empty() {
        db::channels::delete_channel(&mut tx, channel_id).await?;
    } else {
        db::dms::remove_recipient(&mut tx, channel_id, user_id).await?;
        if let Some(owner) = new_owner {
            db::dms::set_owner(&mut tx, channel_id, owner).await?;
        }
    }
    tx.commit().await?;
    let dm = {
        let mut cache = state.cache_mut();
        if remaining.is_empty() {
            cache.channels.remove(&channel_id);
        } else if let Some(c) = cache.channels.get_mut(&channel_id) {
            c.recipient_ids.retain(|&id| id != user_id);
            if new_owner.is_some() {
                c.channel.owner_id = new_owner;
            }
        }
        cache.dm_channel(channel_id)
    };
    state.publish(
        ServerEvent::DmDelete { id: channel_id },
        Audience::User(user_id),
    );
    if let Some(dm) = dm {
        state.publish(ServerEvent::DmUpdate(dm), Audience::Users(remaining));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `PATCH /channels/{id}` on a DM: any recipient may rename a group DM. Group DMs have only a
/// name; 1:1 DMs cannot be changed.
pub async fn update(
    state: Arc<AppState>,
    auth: Auth,
    channel_id: Id,
    req: UpdateChannelRequest,
) -> ApiResult<Json<Channel>> {
    let _writes = state.writes.lock().await;
    let (mut channel, recipients) = {
        let cache = state.cache();
        let access = perms::channel(&cache, channel_id, auth.user_id)?;
        if access.channel.kind != ChannelKind::GroupDm {
            return Err(perms::missing(Permissions::MANAGE_CHANNELS, "channel"));
        }
        (
            access.channel.clone(),
            cache.channels[&channel_id].recipient_ids.clone(),
        )
    };
    let mut fields = Fields::new();
    for (field, given) in [
        ("topic", req.topic.is_some()),
        ("parent_id", req.parent_id.is_some()),
        ("bitrate", req.bitrate.is_some()),
        ("user_limit", req.user_limit.is_some()),
        ("slowmode_secs", req.slowmode_secs.is_some()),
    ] {
        if given {
            fields.add(field, "group DMs only have a name");
        }
    }
    let name = req
        .name
        .and_then(|n| fields.check("name", validate::name(&n, validate::CHANNEL_NAME_CHARS)));
    fields.finish()?;
    let dm = |state: &AppState| {
        state
            .cache()
            .dm_channel(channel_id)
            .ok_or_else(|| Error::not_found("channel"))
    };
    if name.is_none() || name == channel.name {
        return Ok(Json(dm(&state)?));
    }

    channel.name = name;
    let mut conn = state.db.write.acquire().await?;
    db::channels::update_channel(&mut conn, &channel).await?;
    drop(conn);
    if let Some(cached) = state.cache_mut().channels.get_mut(&channel_id) {
        cached.channel.name = channel.name.clone();
    }
    let dm = dm(&state)?;
    state.publish(
        ServerEvent::DmUpdate(dm.clone()),
        Audience::Users(recipients),
    );
    Ok(Json(dm))
}
