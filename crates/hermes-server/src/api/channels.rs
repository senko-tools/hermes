//! Space channels and their permission overrides: `/spaces/{id}/channels`, `/channels/{id}` and
//! `/channels/{id}/overrides/{role_id}`, and their gateway events. Renaming and leaving group
//! DMs through `/channels/{id}` is handled in `dms`.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::api::{
    ChannelPosition, CreateChannelRequest, OverrideRequest, UpdateChannelRequest,
};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, Channel, ChannelKind, Override};
use hermes_proto::permissions::Permissions;
use serde_json::{Map, Value, json};

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, audit, dms, perms};
use crate::auth::Auth;
use crate::config::Config;
use crate::db;
use crate::gateway::bus::Audience;
use crate::gateway::visibility::{self, Visible};
use crate::state::{AppState, Cache};
use crate::time::now_ms;

/// Opus's lowest bitrate; the highest is `voice.max_bitrate`.
const MIN_BITRATE: u32 = 6000;

fn check_bitrate(fields: &mut Fields, config: &Config, bitrate: u32) {
    let max = config.voice.max_bitrate;
    if !(MIN_BITRATE..=max).contains(&bitrate) {
        fields.add("bitrate", format!("must be {MIN_BITRATE} to {max}"));
    }
}

/// `0` means unlimited and is stored as `None`.
fn check_user_limit(fields: &mut Fields, config: &Config, limit: Option<u32>) -> Option<u32> {
    let max = config.voice.max_users_per_room;
    match limit {
        Some(n) if n > max => {
            fields.add(
                "user_limit",
                format!("must be at most {max} (0 = unlimited)"),
            );
            None
        }
        Some(0) | None => None,
        Some(n) => Some(n),
    }
}

/// A valid parent for a `kind` channel in `space_id`: a category of that space, and never for a
/// category itself.
fn check_parent(
    cache: &Cache,
    space_id: Id,
    kind: ChannelKind,
    parent_id: Id,
) -> Result<&Channel, String> {
    if kind == ChannelKind::Category {
        return Err("a category cannot be inside a category".into());
    }
    cache
        .channels
        .get(&parent_id)
        .map(|c| &c.channel)
        .filter(|c| c.space_id == Some(space_id) && c.kind == ChannelKind::Category)
        .ok_or_else(|| "must be a category in this space".to_owned())
}

/// `MANAGE_CHANNELS`. The channel goes last; inside a category it gets a copy of the category's
/// overrides (PLAN.md section 8.2).
pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Json(req): Json<CreateChannelRequest>,
) -> ApiResult<Json<Channel>> {
    let config = &state.config;
    let mut fields = Fields::new();
    if !matches!(
        req.kind,
        ChannelKind::Text | ChannelKind::Voice | ChannelKind::Category
    ) {
        fields.add("kind", "must be text, voice or category");
    }
    let name = fields.check(
        "name",
        validate::name(&req.name, validate::CHANNEL_NAME_CHARS),
    );
    let topic = match &req.topic {
        Some(topic) => fields
            .check(
                "topic",
                validate::optional_text(topic, validate::CHANNEL_TOPIC_CHARS),
            )
            .flatten(),
        None => None,
    };
    let (bitrate, user_limit) = if req.kind == ChannelKind::Voice {
        let bitrate = req.bitrate.unwrap_or(config.voice.default_bitrate);
        check_bitrate(&mut fields, config, bitrate);
        (
            Some(bitrate),
            check_user_limit(&mut fields, config, req.user_limit),
        )
    } else {
        if req.bitrate.is_some() {
            fields.add("bitrate", "only voice channels have a bitrate");
        }
        if req.user_limit.is_some() {
            fields.add("user_limit", "only voice channels have a user limit");
        }
        (None, None)
    };

    let _writes = state.writes.lock().await;
    let channel = {
        let cache = state.cache();
        perms::require_space(&cache, space_id, auth.user_id, Permissions::MANAGE_CHANNELS)?;
        let mut overrides = Vec::new();
        if let Some(parent_id) = req.parent_id
            && let Some(parent) = fields.check(
                "parent_id",
                check_parent(&cache, space_id, req.kind, parent_id),
            )
        {
            overrides = parent.overrides.clone();
        }
        fields.finish()?;
        let Some(name) = name else {
            unreachable!("finish() returned Ok, so every field is valid");
        };
        let position = cache
            .space_channels(space_id)
            .map(|c| c.position)
            .max()
            .map_or(0, |p| p.saturating_add(1));
        Channel {
            id: state.ids.next(),
            space_id: Some(space_id),
            kind: req.kind,
            parent_id: req.parent_id,
            name: Some(name),
            topic,
            position,
            bitrate,
            user_limit,
            slowmode_secs: 0,
            overrides,
            recipients: Vec::new(),
            owner_id: None,
            last_message_id: None,
        }
    };

    let mut tx = state.db.write.begin().await?;
    db::spaces::insert_channel(&mut tx, &channel, now_ms()).await?;
    for o in &channel.overrides {
        db::channels::put_override(&mut tx, channel.id, o).await?;
    }
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::ChannelCreate,
        Some(channel.id),
        json!({ "name": channel.name, "kind": channel.kind }),
    )
    .await?;
    tx.commit().await?;
    state
        .cache_mut()
        .insert_channel(channel.clone(), Vec::new());
    state.publish(
        ServerEvent::ChannelCreate(channel.clone()),
        Audience::Channel(channel.id),
    );
    Ok(Json(channel))
}

/// The caller's view of a space channel they may manage (`MANAGE_CHANNELS` plus `extra`), and its
/// space. DMs answer `MISSING_PERMISSION` because recipients have no management bits.
fn manageable(
    cache: &Cache,
    channel_id: Id,
    user_id: Id,
    extra: Permissions,
) -> ApiResult<(Channel, Id, Permissions)> {
    let access = perms::require_channel(
        cache,
        channel_id,
        user_id,
        Permissions::MANAGE_CHANNELS | extra,
    )?;
    let space_id = access
        .channel
        .space_id
        .ok_or_else(|| perms::missing(Permissions::MANAGE_CHANNELS, "channel"))?;
    Ok((access.channel.clone(), space_id, access.perms))
}

/// `MANAGE_CHANNELS` in the channel.
pub async fn update(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
    Json(req): Json<UpdateChannelRequest>,
) -> ApiResult<Json<Channel>> {
    if is_private(&state, channel_id) {
        return dms::update(state, auth, channel_id, req).await;
    }
    let config = &state.config;
    let _writes = state.writes.lock().await;
    let (channel, space_id, details) = {
        let cache = state.cache();
        let (old, space_id, _) =
            manageable(&cache, channel_id, auth.user_id, Permissions::empty())?;
        let mut channel = old.clone();
        let mut fields = Fields::new();
        if let Some(name) = &req.name
            && let Some(name) =
                fields.check("name", validate::name(name, validate::CHANNEL_NAME_CHARS))
        {
            channel.name = Some(name);
        }
        if let Some(topic) = &req.topic {
            let checked = match topic {
                Some(t) => validate::optional_text(t, validate::CHANNEL_TOPIC_CHARS),
                None => Ok(None),
            };
            if let Some(topic) = fields.check("topic", checked) {
                channel.topic = topic;
            }
        }
        if let Some(parent_id) = req.parent_id {
            let checked = match parent_id {
                Some(id) => check_parent(&cache, space_id, old.kind, id).map(|_| Some(id)),
                None => Ok(None),
            };
            if let Some(parent_id) = fields.check("parent_id", checked) {
                channel.parent_id = parent_id;
            }
        }
        let is_voice = old.kind == ChannelKind::Voice;
        if let Some(bitrate) = req.bitrate {
            if is_voice {
                check_bitrate(&mut fields, config, bitrate);
                channel.bitrate = Some(bitrate);
            } else {
                fields.add("bitrate", "only voice channels have a bitrate");
            }
        }
        if let Some(limit) = req.user_limit {
            if is_voice {
                channel.user_limit = check_user_limit(&mut fields, config, limit);
            } else {
                fields.add("user_limit", "only voice channels have a user limit");
            }
        }
        if let Some(slowmode) = req.slowmode_secs {
            if old.kind == ChannelKind::Category {
                fields.add("slowmode_secs", "categories have no messages");
            } else if slowmode > UpdateChannelRequest::MAX_SLOWMODE_SECS {
                fields.add(
                    "slowmode_secs",
                    format!(
                        "must be at most {}",
                        UpdateChannelRequest::MAX_SLOWMODE_SECS
                    ),
                );
            }
            channel.slowmode_secs = slowmode;
        }
        fields.finish()?;

        let mut details = Map::new();
        if channel.name != old.name {
            details.insert("name".into(), json!(channel.name));
        }
        if channel.topic != old.topic {
            details.insert("topic".into(), json!(channel.topic));
        }
        if channel.parent_id != old.parent_id {
            details.insert("parent_id".into(), json!(channel.parent_id));
        }
        if channel.bitrate != old.bitrate {
            details.insert("bitrate".into(), json!(channel.bitrate));
        }
        if channel.user_limit != old.user_limit {
            details.insert("user_limit".into(), json!(channel.user_limit));
        }
        if channel.slowmode_secs != old.slowmode_secs {
            details.insert("slowmode_secs".into(), json!(channel.slowmode_secs));
        }
        (channel, space_id, details)
    };
    if details.is_empty() {
        return Ok(Json(channel));
    }

    let mut tx = state.db.write.begin().await?;
    db::channels::update_channel(&mut tx, &channel).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::ChannelUpdate,
        Some(channel_id),
        Value::Object(details),
    )
    .await?;
    tx.commit().await?;
    if let Some(cached) = state.cache_mut().channels.get_mut(&channel_id) {
        cached.channel = channel.clone();
    }
    state.publish(
        ServerEvent::ChannelUpdate(channel.clone()),
        Audience::Channel(channel_id),
    );
    Ok(Json(channel))
}

/// Whether the channel is a DM or group DM (whose `/channels/{id}` requests `dms` handles).
fn is_private(state: &AppState, channel_id: Id) -> bool {
    state
        .cache()
        .channels
        .get(&channel_id)
        .is_some_and(|c| c.channel.kind.is_private())
}

/// `MANAGE_CHANNELS`. Deletes the messages too; children of a category move to the top level.
/// For a group DM this means leaving it.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(channel_id): Path<Id>,
) -> ApiResult<StatusCode> {
    if is_private(&state, channel_id) {
        return dms::leave(state, auth, channel_id).await;
    }
    let _writes = state.writes.lock().await;
    let (channel, space_id, viewers) = {
        let cache = state.cache();
        let (channel, space_id, _) =
            manageable(&cache, channel_id, auth.user_id, Permissions::empty())?;
        (channel, space_id, cache.channel_viewers(channel_id))
    };
    let mut tx = state.db.write.begin().await?;
    db::channels::delete_channel(&mut tx, channel_id).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::ChannelDelete,
        Some(channel_id),
        json!({ "name": channel.name, "kind": channel.kind }),
    )
    .await?;
    tx.commit().await?;
    let children: Vec<Channel> = {
        let mut cache = state.cache_mut();
        cache.channels.remove(&channel_id);
        let mut children = Vec::new();
        for child in cache.channels.values_mut() {
            if child.channel.parent_id == Some(channel_id) {
                child.channel.parent_id = None;
                children.push(child.channel.clone());
            }
        }
        children
    };
    state.publish(
        ServerEvent::ChannelDelete {
            id: channel_id,
            space_id,
        },
        Audience::Users(viewers.into_iter().collect()),
    );
    for child in children {
        let id = child.id;
        state.publish(ServerEvent::ChannelUpdate(child), Audience::Channel(id));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `MANAGE_CHANNELS` in the space. Sets positions (and optionally parents) as given; returns the
/// space's channels the caller can see, sorted by position.
pub async fn reorder(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Json(req): Json<Vec<ChannelPosition>>,
) -> ApiResult<Json<Vec<Channel>>> {
    let _writes = state.writes.lock().await;
    let changed = {
        let cache = state.cache();
        perms::require_space(&cache, space_id, auth.user_id, Permissions::MANAGE_CHANNELS)?;
        let mut seen = HashSet::new();
        let mut changed = Vec::new();
        for p in &req {
            let unknown = || validate::field_error("channels", format!("unknown channel {}", p.id));
            let old = cache
                .channels
                .get(&p.id)
                .map(|c| &c.channel)
                .filter(|c| c.space_id == Some(space_id))
                .ok_or_else(unknown)?;
            if !seen.insert(p.id) {
                return Err(validate::field_error(
                    "channels",
                    format!("channel {} is listed twice", p.id),
                ));
            }
            let mut channel = old.clone();
            channel.position = p.position;
            if let Some(parent_id) = p.parent_id {
                if let Some(parent_id) = parent_id {
                    check_parent(&cache, space_id, old.kind, parent_id)
                        .map_err(|problem| validate::field_error("channels", problem))?;
                }
                channel.parent_id = parent_id;
            }
            if channel != *old {
                changed.push(channel);
            }
        }
        changed
    };

    if !changed.is_empty() {
        let mut tx = state.db.write.begin().await?;
        for channel in &changed {
            db::channels::update_channel(&mut tx, channel).await?;
        }
        let positions: Vec<Value> = changed
            .iter()
            .map(|c| json!({ "id": c.id, "position": c.position, "parent_id": c.parent_id }))
            .collect();
        audit::record(
            &mut tx,
            &state,
            space_id,
            auth.user_id,
            AuditAction::ChannelsReorder,
            None,
            json!({ "channels": positions }),
        )
        .await?;
        tx.commit().await?;
    }

    {
        let mut cache = state.cache_mut();
        for channel in &changed {
            if let Some(cached) = cache.channels.get_mut(&channel.id) {
                cached.channel = channel.clone();
            }
        }
    }
    for channel in changed {
        let id = channel.id;
        state.publish(ServerEvent::ChannelUpdate(channel), Audience::Channel(id));
    }
    let cache = state.cache();
    let mut visible: Vec<Channel> = cache
        .space_channels(space_id)
        .filter(|c| {
            cache
                .channel_permissions(c.id, auth.user_id)
                .contains(Permissions::VIEW_CHANNEL)
        })
        .cloned()
        .collect();
    visible.sort_by_key(|c| (c.position, c.id));
    Ok(Json(visible))
}

/// `MANAGE_CHANNELS` + `MANAGE_ROLES` in the channel. `allow` and `deny` must not share bits, and
/// may only contain bits the caller has in the channel.
pub async fn put_override(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, role_id)): Path<(Id, Id)>,
    Json(req): Json<OverrideRequest>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let (space_id, before) = {
        let cache = state.cache();
        let (_, space_id, have) =
            manageable(&cache, channel_id, auth.user_id, Permissions::MANAGE_ROLES)?;
        let role_exists = cache
            .spaces
            .get(&space_id)
            .is_some_and(|s| s.role(role_id).is_some());
        if !role_exists {
            return Err(Error::not_found("role"));
        }
        if req.allow.intersects(req.deny) {
            return Err(validate::field_error(
                "deny",
                "must not contain bits that are in allow",
            ));
        }
        let bits = req.allow | req.deny;
        if !have.contains(bits) {
            return Err(perms::missing(bits.difference(have), "channel"));
        }
        (space_id, Visible::of_space(&cache, space_id))
    };

    let o = Override {
        role_id,
        allow: req.allow,
        deny: req.deny,
    };
    let mut tx = state.db.write.begin().await?;
    db::channels::put_override(&mut tx, channel_id, &o).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::OverrideUpdate,
        Some(channel_id),
        json!({ "role_id": role_id, "allow": o.allow, "deny": o.deny }),
    )
    .await?;
    tx.commit().await?;
    if let Some(cached) = state.cache_mut().channels.get_mut(&channel_id) {
        let overrides = &mut cached.channel.overrides;
        overrides.retain(|existing| existing.role_id != role_id);
        overrides.push(o);
        overrides.sort_by_key(|o| o.role_id);
    }
    publish_override_change(&state, space_id, channel_id, &before);
    Ok(StatusCode::NO_CONTENT)
}

/// Same permissions as [`put_override`].
pub async fn delete_override(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, role_id)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let (space_id, before) = {
        let cache = state.cache();
        let (_, space_id, _) =
            manageable(&cache, channel_id, auth.user_id, Permissions::MANAGE_ROLES)?;
        (space_id, Visible::of_space(&cache, space_id))
    };
    let mut tx = state.db.write.begin().await?;
    if !db::channels::delete_override(&mut tx, channel_id, role_id).await? {
        return Err(Error::not_found("override"));
    }
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::OverrideDelete,
        Some(channel_id),
        json!({ "role_id": role_id }),
    )
    .await?;
    tx.commit().await?;
    if let Some(cached) = state.cache_mut().channels.get_mut(&channel_id) {
        cached
            .channel
            .overrides
            .retain(|existing| existing.role_id != role_id);
    }
    publish_override_change(&state, space_id, channel_id, &before);
    Ok(StatusCode::NO_CONTENT)
}

/// `channel_update` to members who still see the channel; `channel_create` / `channel_delete`
/// to those who gained or lost it.
fn publish_override_change(state: &AppState, space_id: Id, channel_id: Id, before: &Visible) {
    let after = visibility::publish_changes(state, space_id, before);
    let channel = state
        .cache()
        .channels
        .get(&channel_id)
        .map(|c| c.channel.clone());
    if let Some(channel) = channel {
        state.publish(
            ServerEvent::ChannelUpdate(channel),
            visibility::kept_viewers(before, &after, channel_id),
        );
    }
}
