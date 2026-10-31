//! `/spaces/{id}/members` and `/spaces/{id}/bans`, and their gateway events.

use std::collections::HashMap;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{BanRequest, Pagination, UpdateMemberRequest};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, Ban, Member};
use hermes_proto::permissions::Permissions;
use serde_json::{Map, Value, json};

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, Query, audit, perms, user_ref};
use crate::auth::Auth;
use crate::db;
use crate::gateway::bus::Audience;
use crate::gateway::visibility::{self, Visible};
use crate::state::AppState;
use crate::time::{HOUR_MS, now_ms};

/// Members ordered by user ID; `?after&limit`.
pub async fn list(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Query(page): Query<Pagination>,
) -> ApiResult<Json<Vec<Member>>> {
    let cache = state.cache();
    let access = perms::space(&cache, space_id, auth.user_id)?;
    let after = page.after.unwrap_or_default();
    let mut ids: Vec<Id> = access
        .space
        .members
        .keys()
        .copied()
        .filter(|&id| id > after)
        .collect();
    ids.sort();
    ids.truncate(page.limit() as usize);
    Ok(Json(
        ids.into_iter()
            .filter_map(|id| cache.member(space_id, id))
            .collect(),
    ))
}

/// Changes a nickname (own: `CHANGE_NICKNAME`; others: `MANAGE_NICKNAMES`) and/or the assigned
/// roles (`MANAGE_ROLES`). Both follow the hierarchy rule; every added or removed role must be
/// below the caller's highest role. `{uid}` may be `@me`.
pub async fn update(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, uid)): Path<(Id, String)>,
    Json(req): Json<UpdateMemberRequest>,
) -> ApiResult<Json<Member>> {
    let user_id = user_ref(&uid, auth.user_id)?;
    let mut fields = Fields::new();
    let nickname = req.nickname.map(|nick| {
        nick.and_then(|n| fields.check("nickname", validate::name(&n, validate::NICKNAME_CHARS)))
    });
    fields.finish()?;

    let _writes = state.writes.lock().await;
    let is_self = user_id == auth.user_id;
    let mut details = Map::new();
    let (member, nickname_changed, roles_changed, before) = {
        let cache = state.cache();
        let access = perms::space(&cache, space_id, auth.user_id)?;
        let mut member = cache
            .member(space_id, user_id)
            .ok_or_else(|| Error::not_found("member"))?;

        let nickname_changed = nickname.as_ref().is_some_and(|n| *n != member.nickname);
        if let Some(nickname) = nickname.filter(|_| nickname_changed) {
            if is_self {
                if !access
                    .perms
                    .intersects(Permissions::CHANGE_NICKNAME | Permissions::MANAGE_NICKNAMES)
                {
                    return Err(perms::missing(Permissions::CHANGE_NICKNAME, "space"));
                }
            } else {
                access.require(Permissions::MANAGE_NICKNAMES)?;
                if !access.outranks_member(&member) {
                    return Err(access.hierarchy_error("members"));
                }
            }
            details.insert("nickname".into(), json!(nickname));
            member.nickname = nickname;
        }

        let mut roles_changed = false;
        if let Some(mut role_ids) = req.role_ids {
            role_ids.sort();
            role_ids.dedup();
            if role_ids != member.role_ids {
                access.require(Permissions::MANAGE_ROLES)?;
                if !is_self && !access.outranks_member(&member) {
                    return Err(access.hierarchy_error("members"));
                }
                let added: Vec<Id> = role_ids
                    .iter()
                    .filter(|id| !member.role_ids.contains(id))
                    .copied()
                    .collect();
                let removed: Vec<Id> = member
                    .role_ids
                    .iter()
                    .filter(|id| !role_ids.contains(id))
                    .copied()
                    .collect();
                for id in added.iter().chain(&removed) {
                    let role = access
                        .space
                        .role(*id)
                        .filter(|r| !r.is_everyone())
                        .ok_or_else(|| {
                            validate::field_error("role_ids", format!("unknown role {id}"))
                        })?;
                    if !access.outranks_role(role) {
                        return Err(access.hierarchy_error("roles"));
                    }
                }
                details.insert("added_role_ids".into(), json!(added));
                details.insert("removed_role_ids".into(), json!(removed));
                member.role_ids = role_ids;
                roles_changed = true;
            }
        }
        let before = roles_changed.then(|| Visible::of_space(&cache, space_id));
        (member, nickname_changed, roles_changed, before)
    };
    if !nickname_changed && !roles_changed {
        return Ok(Json(member));
    }

    let mut tx = state.db.write.begin().await?;
    if nickname_changed {
        db::members::set_nickname(&mut tx, space_id, user_id, member.nickname.as_deref()).await?;
    }
    if roles_changed {
        db::members::set_roles(&mut tx, space_id, user_id, &member.role_ids).await?;
    }
    // Changing your own nickname is not a moderation action.
    if !is_self || roles_changed {
        audit::record(
            &mut tx,
            &state,
            space_id,
            auth.user_id,
            AuditAction::MemberUpdate,
            Some(user_id),
            Value::Object(details),
        )
        .await?;
    }
    tx.commit().await?;
    if let Some(cached) = state
        .cache_mut()
        .spaces
        .get_mut(&space_id)
        .and_then(|s| s.members.get_mut(&user_id))
    {
        cached.nickname = member.nickname.clone();
        cached.role_ids = member.role_ids.clone();
    }
    state.publish(
        ServerEvent::MemberUpdate(member.clone()),
        Audience::space(space_id),
    );
    if let Some(before) = before {
        visibility::publish_changes(&state, space_id, &before);
    }
    Ok(Json(member))
}

/// Kicks a member (`KICK_MEMBERS` + hierarchy), or leaves the space when `{uid}` is the caller or
/// `@me` (the owner cannot leave).
pub async fn remove(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, uid)): Path<(Id, String)>,
) -> ApiResult<StatusCode> {
    let user_id = user_ref(&uid, auth.user_id)?;
    let leaving = user_id == auth.user_id;
    let _writes = state.writes.lock().await;
    {
        let cache = state.cache();
        let access = perms::space(&cache, space_id, auth.user_id)?;
        if leaving {
            if access.is_owner() {
                return Err(Error::new(
                    ErrorCode::BadRequest,
                    "The owner cannot leave the space; transfer or delete it instead",
                ));
            }
        } else {
            access.require(Permissions::KICK_MEMBERS)?;
            let target = cache
                .member(space_id, user_id)
                .ok_or_else(|| Error::not_found("member"))?;
            if !access.outranks_member(&target) {
                return Err(access.hierarchy_error("members"));
            }
        }
    }
    let mut tx = state.db.write.begin().await?;
    db::members::delete_member(&mut tx, space_id, user_id).await?;
    if !leaving {
        audit::record(
            &mut tx,
            &state,
            space_id,
            auth.user_id,
            AuditAction::MemberKick,
            Some(user_id),
            json!({}),
        )
        .await?;
    }
    tx.commit().await?;
    state.cache_mut().remove_member(space_id, user_id);
    publish_removal(&state, space_id, user_id);
    Ok(StatusCode::NO_CONTENT)
}

/// `member_remove` to the remaining members, `space_delete` to the removed user.
fn publish_removal(state: &AppState, space_id: Id, user_id: Id) {
    state.publish(
        ServerEvent::MemberRemove { space_id, user_id },
        Audience::space(space_id),
    );
    state.publish(
        ServerEvent::SpaceDelete { id: space_id },
        Audience::User(user_id),
    );
    state.voice.revalidate(state);
}

/// `BAN_MEMBERS`. Oldest first.
pub async fn list_bans(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
) -> ApiResult<Json<Vec<Ban>>> {
    perms::require_space(
        &state.cache(),
        space_id,
        auth.user_id,
        Permissions::BAN_MEMBERS,
    )?;
    let rows = db::members::list_bans(&state.db.read, space_id).await?;
    let cache = state.cache();
    Ok(Json(
        rows.into_iter()
            .filter_map(|row| {
                Some(Ban {
                    user: cache.users.get(&row.user_id)?.user.clone(),
                    reason: row.reason,
                    created_at: row.created_at,
                })
            })
            .collect(),
    ))
}

/// `BAN_MEMBERS` (+ hierarchy if the user is a member). Removes the membership and optionally
/// soft-deletes the user's recent messages in the space. Users who are not members can be banned
/// too, so they cannot join later.
pub async fn ban(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, user_id)): Path<(Id, Id)>,
    Json(req): Json<BanRequest>,
) -> ApiResult<StatusCode> {
    let mut fields = Fields::new();
    let reason = match &req.reason {
        Some(reason) => fields.check(
            "reason",
            validate::optional_text(reason, validate::BAN_REASON_CHARS),
        ),
        None => Some(None),
    };
    let hours = req.delete_messages_hours.unwrap_or(0);
    if hours > BanRequest::MAX_DELETE_MESSAGES_HOURS {
        fields.add(
            "delete_messages_hours",
            format!("must be at most {}", BanRequest::MAX_DELETE_MESSAGES_HOURS),
        );
    }
    fields.finish()?;
    let reason = reason.flatten();

    let _writes = state.writes.lock().await;
    let was_member = {
        let cache = state.cache();
        let access =
            perms::require_space(&cache, space_id, auth.user_id, Permissions::BAN_MEMBERS)?;
        if user_id == auth.user_id {
            return Err(Error::new(ErrorCode::BadRequest, "You cannot ban yourself"));
        }
        if !cache.users.contains_key(&user_id) {
            return Err(Error::not_found("user"));
        }
        match cache.member(space_id, user_id) {
            Some(target) if !access.outranks_member(&target) => {
                return Err(access.hierarchy_error("members"));
            }
            Some(_) => true,
            None => false,
        }
    };

    let now = now_ms();
    let mut tx = state.db.write.begin().await?;
    db::members::upsert_ban(
        &mut tx,
        space_id,
        user_id,
        reason.as_deref(),
        auth.user_id,
        now,
    )
    .await?;
    if was_member {
        db::members::delete_member(&mut tx, space_id, user_id).await?;
    }
    let deleted = if hours > 0 {
        let since = now - i64::from(hours) * HOUR_MS;
        let min_id = Id::min_at_unix_ms(u64::try_from(since).unwrap_or(0));
        db::members::soft_delete_messages_since(&mut tx, space_id, user_id, min_id).await?
    } else {
        Vec::new()
    };
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::MemberBan,
        Some(user_id),
        json!({
            "reason": reason,
            "delete_messages_hours": hours,
            "deleted_messages": deleted.len(),
        }),
    )
    .await?;
    tx.commit().await?;
    state.cache_mut().remove_member(space_id, user_id);
    if was_member {
        publish_removal(&state, space_id, user_id);
    }
    let mut by_channel: HashMap<Id, Vec<Id>> = HashMap::new();
    for (message_id, channel_id) in deleted {
        by_channel.entry(channel_id).or_default().push(message_id);
    }
    for (channel_id, mut ids) in by_channel {
        ids.sort();
        state.publish(
            ServerEvent::MessageDeleteBulk { ids, channel_id },
            Audience::Channel(channel_id),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `BAN_MEMBERS`.
pub async fn unban(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, user_id)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    perms::require_space(
        &state.cache(),
        space_id,
        auth.user_id,
        Permissions::BAN_MEMBERS,
    )?;
    let mut tx = state.db.write.begin().await?;
    if !db::members::delete_ban(&mut tx, space_id, user_id).await? {
        return Err(Error::not_found("ban"));
    }
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::MemberUnban,
        Some(user_id),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
