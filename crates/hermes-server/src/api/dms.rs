//! `/dms`: 1:1 DMs and group DMs (PLAN.md sections 8.2 and 10), plus renaming and leaving group
//! DMs through `/channels/{id}`. Messages in DMs use the normal message endpoints.
//!
//! A recipient can close a 1:1 DM (`DELETE /dms/{id}`, docs/DECISIONS.md 2026-11-02): it is no
//! longer listed for them, and reopens when a message or a call arrives or they message the
//! other person again.
//!
//! Users choose who may start DMs with them (`PrivacySettings`). A new DM from anyone else is
//! refused, or becomes a message request for that recipient: listed apart, never rung, no
//! presence shared, until they accept it (`POST /dms/{id}/accept`, or by sending a message).
//! Ignoring a request closes it, and it stays closed when more messages arrive.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{CreateDmRequest, UpdateChannelRequest};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{Channel, ChannelKind, DmFrom};
use hermes_proto::permissions::Permissions;

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, perms, user_ref};
use crate::auth::Auth;
use crate::config::DmPolicy;
use crate::db;
use crate::gateway::bus::Audience;
use crate::state::{AppState, Cache};
use crate::time::now_ms;

/// The caller's DMs and group DMs (not the closed ones), most recently active first.
pub async fn list(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<Channel>>> {
    let cache = state.cache();
    let mut dms: Vec<Channel> = cache
        .open_dm_ids(auth.user_id)
        .filter_map(|id| cache.dm_for(id, auth.user_id))
        .collect();
    dms.sort_by_key(|c| std::cmp::Reverse((c.last_message_id, c.id)));
    Ok(Json(dms))
}

/// `dm_policy`: with `shared_space`, the caller must share a space with `other` or be their friend.
fn check_policy(state: &AppState, cache: &Cache, user_id: Id, other: Id) -> ApiResult<()> {
    match state.config.privacy.dm_policy {
        DmPolicy::Anyone => Ok(()),
        DmPolicy::SharedSpace
            if cache.shares_space(user_id, other) || cache.are_friends(user_id, other) =>
        {
            Ok(())
        }
        DmPolicy::SharedSpace => Err(Error::new(
            ErrorCode::MissingPermission,
            "You can only message users who share a space with you",
        )),
    }
}

/// How a new DM from `sender` reaches `other` under `other`'s privacy settings: directly
/// (`false`), as a message request (`true`), or not at all.
fn as_request(cache: &Cache, sender: Id, other: Id) -> ApiResult<bool> {
    let privacy = cache
        .users
        .get(&other)
        .map(|u| u.privacy)
        .unwrap_or_default();
    let allowed = cache.are_friends(sender, other)
        || match privacy.dm_from {
            DmFrom::Everyone => true,
            DmFrom::Spaces => cache.shares_space(sender, other),
            DmFrom::Nobody => false,
        };
    if allowed {
        Ok(false)
    } else if privacy.message_requests {
        Ok(true)
    } else {
        Err(Error::new(
            ErrorCode::MissingPermission,
            "This user does not accept direct messages from you",
        ))
    }
}

/// Sends `event` with the DM as each of `users` sees it (`request` differs per recipient).
fn publish_each(state: &AppState, channel_id: Id, users: &[Id], event: fn(Channel) -> ServerEvent) {
    let events: Vec<(Id, Channel)> = {
        let cache = state.cache();
        users
            .iter()
            .filter_map(|&user| Some((user, cache.dm_for(channel_id, user)?)))
            .collect()
    };
    for (user, dm) in events {
        state.publish(event(dm), Audience::User(user));
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
    let (existing, requests) = {
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
        let existing = match others[..] {
            [other] => cache.dm_ids(auth.user_id).find(|id| {
                let c = &cache.channels[id];
                c.channel.kind == ChannelKind::Dm && c.recipient_ids.binary_search(&other).is_ok()
            }),
            _ => None,
        };
        // An existing 1:1 DM is reused as it is; a new DM follows everyone's privacy settings.
        let mut requests = Vec::new();
        if existing.is_none() {
            for &other in &others {
                if as_request(&cache, auth.user_id, other)? {
                    requests.push(other);
                }
            }
        }
        (existing, requests)
    };
    if let Some(id) = existing {
        // Messaging someone again reopens a DM the caller closed.
        reopen_for(&state, id, &[auth.user_id]).await?;
        return state
            .cache()
            .dm_for(id, auth.user_id)
            .map(Json)
            .ok_or_else(|| Error::not_found("channel"));
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
        request: false,
    };
    let mut recipients = others;
    recipients.push(auth.user_id);
    let mut tx = state.db.write.begin().await?;
    db::spaces::insert_channel(&mut tx, &channel, now_ms()).await?;
    for &user_id in &recipients {
        let request = requests.contains(&user_id);
        db::dms::add_recipient(&mut tx, channel.id, user_id, request).await?;
    }
    tx.commit().await?;
    let dm = {
        let mut cache = state.cache_mut();
        cache.insert_channel(channel.clone(), recipients.clone());
        if let Some(c) = cache.channels.get_mut(&channel.id) {
            c.requests = requests;
        }
        cache.dm_for(channel.id, auth.user_id)
    }
    .ok_or_else(|| Error::internal("new DM missing from the cache"))?;
    publish_each(&state, channel.id, &recipients, ServerEvent::DmCreate);
    Ok(Json(dm))
}

/// Accepts a message request: the DM becomes a normal one for the caller (`dm_update` to their
/// sessions). Accepting a DM that is no request is not an error.
pub async fn accept(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
) -> ApiResult<Json<Channel>> {
    perms::channel(&state.cache(), channel_id, auth.user_id)?;
    accept_request(&state, channel_id, auth.user_id).await?;
    state
        .cache()
        .dm_for(channel_id, auth.user_id)
        .map(Json)
        .ok_or_else(|| Error::not_found("channel"))
}

/// Accepts `user_id`'s message request in a DM, if it is one; it is reopened too. Their
/// sessions get `dm_update` (`dm_create` if they had ignored it).
pub async fn accept_request(state: &AppState, channel_id: Id, user_id: Id) -> ApiResult<()> {
    let was_closed = {
        let cache = state.cache();
        let Some(c) = cache.channels.get(&channel_id) else {
            return Ok(());
        };
        if c.requests.binary_search(&user_id).is_err() {
            return Ok(());
        }
        c.closed.binary_search(&user_id).is_ok()
    };
    let mut conn = state.db.write.acquire().await?;
    db::dms::accept_request(&mut conn, channel_id, user_id).await?;
    drop(conn);
    if let Some(c) = state.cache_mut().channels.get_mut(&channel_id) {
        c.requests.retain(|&u| u != user_id);
        c.closed.retain(|&u| u != user_id);
    }
    let event = if was_closed {
        ServerEvent::DmCreate
    } else {
        ServerEvent::DmUpdate
    };
    publish_each(state, channel_id, &[user_id], event);
    Ok(())
}

/// Closes a 1:1 DM for the caller: it disappears from their list (`dm_delete` to their sessions)
/// until it reopens. Group DMs are left instead (`DELETE /channels/{id}`). Closing twice is not an
/// error.
pub async fn close(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    {
        let cache = state.cache();
        let access = perms::channel(&cache, channel_id, auth.user_id)?;
        match access.channel.kind {
            ChannelKind::Dm => {}
            ChannelKind::GroupDm => {
                return Err(Error::new(
                    ErrorCode::BadRequest,
                    "Group DMs are left, not closed",
                ));
            }
            _ => return Err(Error::not_found("channel")),
        }
        if cache.channels[&channel_id]
            .closed
            .binary_search(&auth.user_id)
            .is_ok()
        {
            return Ok(StatusCode::NO_CONTENT);
        }
    }
    let mut conn = state.db.write.acquire().await?;
    db::dms::set_closed(&mut conn, channel_id, auth.user_id, true).await?;
    drop(conn);
    if let Some(c) = state.cache_mut().channels.get_mut(&channel_id)
        && let Err(at) = c.closed.binary_search(&auth.user_id)
    {
        c.closed.insert(at, auth.user_id);
    }
    state.publish(
        ServerEvent::DmDelete { id: channel_id },
        Audience::User(auth.user_id),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Reopens a DM for every recipient who closed it (a message or a call arrived), sending them
/// `dm_create` first so their clients know the channel before its events. An ignored message
/// request (closed while still a request) stays closed. The caller holds no cache lock.
pub async fn reopen(state: &AppState, channel_id: Id) -> ApiResult<()> {
    let closed: Vec<Id> = state
        .cache()
        .channels
        .get(&channel_id)
        .map(|c| {
            c.closed
                .iter()
                .copied()
                .filter(|u| c.requests.binary_search(u).is_err())
                .collect()
        })
        .unwrap_or_default();
    reopen_for(state, channel_id, &closed).await
}

/// Reopens a DM for those of `users` who closed it.
async fn reopen_for(state: &AppState, channel_id: Id, users: &[Id]) -> ApiResult<()> {
    let users: Vec<Id> = {
        let cache = state.cache();
        let Some(c) = cache.channels.get(&channel_id) else {
            return Ok(());
        };
        users
            .iter()
            .copied()
            .filter(|u| c.closed.binary_search(u).is_ok())
            .collect()
    };
    if users.is_empty() {
        return Ok(());
    }
    let mut tx = state.db.write.begin().await?;
    for &user_id in &users {
        db::dms::set_closed(&mut tx, channel_id, user_id, false).await?;
    }
    tx.commit().await?;
    if let Some(c) = state.cache_mut().channels.get_mut(&channel_id) {
        c.closed.retain(|u| !users.contains(u));
    }
    publish_each(state, channel_id, &users, ServerEvent::DmCreate);
    Ok(())
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
    let (recipients, request) = {
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
        let request = as_request(&cache, auth.user_id, user_id)?;
        (recipients, request)
    };

    let mut conn = state.db.write.acquire().await?;
    db::dms::add_recipient(&mut conn, channel_id, user_id, request).await?;
    drop(conn);
    if let Some(c) = state.cache_mut().channels.get_mut(&channel_id) {
        c.recipient_ids.push(user_id);
        c.recipient_ids.sort();
        if request && let Err(at) = c.requests.binary_search(&user_id) {
            c.requests.insert(at, user_id);
        }
    }
    publish_each(&state, channel_id, &[user_id], ServerEvent::DmCreate);
    publish_each(&state, channel_id, &recipients, ServerEvent::DmUpdate);
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
    {
        let mut cache = state.cache_mut();
        if remaining.is_empty() {
            cache.channels.remove(&channel_id);
        } else if let Some(c) = cache.channels.get_mut(&channel_id) {
            c.recipient_ids.retain(|&id| id != user_id);
            c.closed.retain(|&id| id != user_id);
            c.requests.retain(|&id| id != user_id);
            if new_owner.is_some() {
                c.channel.owner_id = new_owner;
            }
        }
    }
    state.publish(
        ServerEvent::DmDelete { id: channel_id },
        Audience::User(user_id),
    );
    publish_each(state, channel_id, &remaining, ServerEvent::DmUpdate);
    state.voice.revalidate(state);
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
            .dm_for(channel_id, auth.user_id)
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
    publish_each(&state, channel_id, &recipients, ServerEvent::DmUpdate);
    Ok(Json(dm))
}
