//! `/users/@me/friends`: friend requests by `name#1234`, accepting, removing. Friends see each
//! other's presence and may always open a DM, whatever their DM settings say.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::AddFriendRequest;
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{Presence, Relationship, RelationshipKind, Status, User, split_handle};

use super::{ApiResult, Error, Json, Path};
use crate::auth::Auth;
use crate::db;
use crate::gateway::bus::Audience;
use crate::state::{AppState, Cache, CachedRelationship};
use crate::time::now_ms;

pub async fn list(
    State(state): State<Arc<AppState>>,
    auth: Auth,
) -> ApiResult<Json<Vec<Relationship>>> {
    Ok(Json(state.cache().relationships_of(auth.user_id)))
}

/// The account a handle names. A handle without a tag only names an account without one, or the
/// single account of that name.
fn find_by_handle(cache: &Cache, handle: &str) -> Option<Id> {
    let handle = handle.trim().trim_start_matches('@').to_lowercase();
    let (name, tag) = split_handle(&handle);
    let mut same_name = cache
        .users
        .values()
        .filter(|u| !u.disabled && u.user.username == name);
    match tag {
        Some(tag) => same_name
            .find(|u| u.user.discriminator.as_deref() == Some(tag))
            .map(|u| u.user.id),
        None => {
            let all: Vec<&User> = same_name.by_ref().map(|u| &u.user).collect();
            all.iter()
                .find(|u| u.discriminator.is_none())
                .or(if all.len() == 1 { all.first() } else { None })
                .map(|u| u.id)
        }
    }
}

/// Tells both people about their relationship, as each of them sees it.
fn publish_both(state: &AppState, a: Id, b: Id) {
    let (for_a, for_b) = {
        let cache = state.cache();
        (cache.relationship(a, b), cache.relationship(b, a))
    };
    if let Some(r) = for_a {
        state.publish(ServerEvent::RelationshipUpdate(r), Audience::User(a));
    }
    if let Some(r) = for_b {
        state.publish(ServerEvent::RelationshipUpdate(r), Audience::User(b));
    }
}

/// Friends see each other's presence from now on.
fn share_presence(state: &AppState, a: Id, b: Id) {
    let (of_a, of_b): (Presence, Presence) = {
        let cache = state.cache();
        (cache.presence(a), cache.presence(b))
    };
    if of_a.status != Status::Offline {
        state.publish(ServerEvent::PresenceUpdate(of_a), Audience::User(b));
    }
    if of_b.status != Status::Offline {
        state.publish(ServerEvent::PresenceUpdate(of_b), Audience::User(a));
    }
}

async fn accept_inner(state: &AppState, me: Id, other: Id) -> ApiResult<Relationship> {
    let now = now_ms();
    let mut conn = state.db.write.acquire().await?;
    db::friends::accept(&mut conn, me, other, now).await?;
    drop(conn);
    state.cache_mut().accept_relationship(me, other, now);
    publish_both(state, me, other);
    share_presence(state, me, other);
    state
        .cache()
        .relationship(me, other)
        .ok_or_else(|| Error::internal("relationship missing from the cache"))
}

/// Sends a friend request; to someone who already asked me, it accepts theirs.
pub async fn add(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<AddFriendRequest>,
) -> ApiResult<Json<Relationship>> {
    let _writes = state.writes.lock().await;
    let me = auth.user_id;
    let (other, existing) = {
        let cache = state.cache();
        let other = find_by_handle(&cache, &req.handle).ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                "Nobody has that name. Check the name and the # number.",
            )
        })?;
        (other, cache.relationship(me, other))
    };
    if other == me {
        return Err(Error::new(
            ErrorCode::BadRequest,
            "You cannot add yourself as a friend",
        ));
    }
    match existing {
        Some(r) if r.kind == RelationshipKind::Incoming => {
            return Ok(Json(accept_inner(&state, me, other).await?));
        }
        Some(r) => return Ok(Json(r)),
        None => {}
    }
    let now = now_ms();
    let mut conn = state.db.write.acquire().await?;
    db::friends::insert_request(&mut conn, me, other, now).await?;
    drop(conn);
    state.cache_mut().insert_relationship(
        me,
        other,
        CachedRelationship {
            requester: me,
            created_at: now,
            accepted_at: None,
        },
    );
    publish_both(&state, me, other);
    state
        .cache()
        .relationship(me, other)
        .map(Json)
        .ok_or_else(|| Error::internal("relationship missing from the cache"))
}

/// Accepts a friend request I received.
pub async fn accept(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(other): Path<Id>,
) -> ApiResult<Json<Relationship>> {
    let _writes = state.writes.lock().await;
    let existing = state.cache().relationship(auth.user_id, other);
    match existing {
        Some(r) if r.kind == RelationshipKind::Incoming => {
            Ok(Json(accept_inner(&state, auth.user_id, other).await?))
        }
        Some(r) if r.kind == RelationshipKind::Friend => Ok(Json(r)),
        _ => Err(Error::not_found("friend request")),
    }
}

/// Removes a friend, declines a request or takes mine back.
pub async fn remove(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(other): Path<Id>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let me = auth.user_id;
    let mut conn = state.db.write.acquire().await?;
    let removed = db::friends::delete(&mut conn, me, other).await?;
    drop(conn);
    if !removed {
        return Err(Error::not_found("friend"));
    }
    state.cache_mut().remove_relationship(me, other);
    state.publish(
        ServerEvent::RelationshipRemove { user_id: other },
        Audience::User(me),
    );
    state.publish(
        ServerEvent::RelationshipRemove { user_id: me },
        Audience::User(other),
    );
    Ok(StatusCode::NO_CONTENT)
}
