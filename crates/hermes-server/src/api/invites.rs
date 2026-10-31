//! `/spaces/{id}/invites` and `/invites/{code}`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::CreateInviteRequest;
use hermes_proto::gateway::SpaceSnapshot;
use hermes_proto::id::Id;
use hermes_proto::models::Invite;
use hermes_proto::permissions::Permissions;

use super::validate::Fields;
use super::{ApiResult, Error, Json, Path, perms};
use crate::auth::Auth;
use crate::db;
use crate::db::invites::InviteRow;
use crate::invites;
use crate::state::{AppState, Cache};
use crate::time::{SECOND_MS, now_ms};

fn invite_invalid() -> Error {
    Error::new(
        ErrorCode::InviteInvalid,
        "This invite is invalid, expired or used up",
    )
}

fn to_wire(row: InviteRow, cache: &Cache) -> Invite {
    Invite {
        space: row.space_id.and_then(|id| cache.space_preview(id)),
        code: row.code,
        creator_id: row.creator_id,
        max_uses: row.max_uses,
        uses: row.uses,
        expires_at: row.expires_at,
    }
}

/// `user_id`'s permissions in a space; `NOT_FOUND` if the space does not exist or they are not
/// a member (non-members must not learn whether a space exists).
fn space_permissions(cache: &Cache, space_id: Id, user_id: Id) -> ApiResult<Permissions> {
    perms::space(cache, space_id, user_id).map(|access| access.perms)
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Json(req): Json<CreateInviteRequest>,
) -> ApiResult<Json<Invite>> {
    let perms = space_permissions(&state.cache(), space_id, auth.user_id)?;
    if !perms.contains(Permissions::CREATE_INVITES) {
        return Err(Error::missing_permission("CREATE_INVITES"));
    }
    let expires_in = req
        .expires_in_secs
        .unwrap_or(CreateInviteRequest::DEFAULT_EXPIRES_IN_SECS);
    let mut fields = Fields::new();
    if expires_in > CreateInviteRequest::MAX_EXPIRES_IN_SECS {
        fields.add(
            "expires_in_secs",
            format!(
                "must be at most {} (30 days), or 0 for never",
                CreateInviteRequest::MAX_EXPIRES_IN_SECS
            ),
        );
    }
    fields.finish()?;

    let now = now_ms();
    // Bounded by MAX_EXPIRES_IN_SECS above, so the multiplication cannot overflow.
    let expires_at = (expires_in > 0).then(|| now + expires_in as i64 * SECOND_MS);
    let max_uses = req.max_uses.filter(|&n| n > 0);
    let row = invites::create(
        &state.db.write,
        Some(space_id),
        Some(auth.user_id),
        max_uses,
        expires_at,
        now,
    )
    .await?;
    Ok(Json(to_wire(row, &state.cache())))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
) -> ApiResult<Json<Vec<Invite>>> {
    let perms = space_permissions(&state.cache(), space_id, auth.user_id)?;
    if !perms.contains(Permissions::MANAGE_INVITES) {
        return Err(Error::missing_permission("MANAGE_INVITES"));
    }
    let rows = db::invites::list_for_space(&state.db.read, space_id).await?;
    let cache = state.cache();
    Ok(Json(rows.into_iter().map(|r| to_wire(r, &cache)).collect()))
}

/// Loads a usable invite; `INVITE_INVALID` otherwise.
async fn usable_invite(state: &AppState, code: &str) -> ApiResult<InviteRow> {
    if !invites::is_valid_code(code) {
        return Err(invite_invalid());
    }
    db::invites::get(&state.db.read, code)
        .await?
        .filter(|invite| invite.is_usable(now_ms()))
        .ok_or_else(invite_invalid)
}

/// Public preview (no auth): the space's name, icon and member count.
pub async fn preview(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> ApiResult<Json<Invite>> {
    let row = usable_invite(&state, &code).await?;
    let cache = state.cache();
    if row
        .space_id
        .is_some_and(|id| !cache.spaces.contains_key(&id))
    {
        return Err(invite_invalid());
    }
    Ok(Json(to_wire(row, &cache)))
}

/// Joins the invite's space and returns it as in `ready`. Already being a member is not an error
/// and does not use up the invite.
pub async fn join(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(code): Path<String>,
) -> ApiResult<Json<SpaceSnapshot>> {
    let row = usable_invite(&state, &code).await?;
    let space_id = row.space_id.ok_or_else(|| {
        Error::new(
            ErrorCode::InviteInvalid,
            "This invite is only for creating an account",
        )
    })?;
    if let Some(snapshot) = existing_member_snapshot(&state, space_id, auth.user_id) {
        return Ok(Json(snapshot));
    }
    if db::spaces::is_banned(&state.db.read, space_id, auth.user_id).await? {
        return Err(Error::new(
            ErrorCode::Banned,
            "You are banned from this space",
        ));
    }

    let now = now_ms();
    let mut tx = state.db.write.begin().await?;
    if db::invites::take_use(&mut tx, &code, now).await?.is_none() {
        return Err(invite_invalid());
    }
    match db::spaces::insert_member(&mut tx, space_id, auth.user_id, now).await {
        // Joined concurrently through another request; that one counted the use.
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            drop(tx);
            return existing_member_snapshot(&state, space_id, auth.user_id)
                .map(Json)
                .ok_or_else(|| Error::internal("member missing from the cache after joining"));
        }
        result => result?,
    }
    tx.commit().await?;

    let mut cache = state.cache_mut();
    cache.insert_member(space_id, auth.user_id, now);
    cache
        .space_snapshot(space_id, auth.user_id)
        .map(Json)
        .ok_or_else(invite_invalid)
}

fn existing_member_snapshot(state: &AppState, space_id: Id, user_id: Id) -> Option<SpaceSnapshot> {
    let cache = state.cache();
    if cache.is_member(space_id, user_id) {
        cache.space_snapshot(space_id, user_id)
    } else {
        None
    }
}

/// The creator, members with `MANAGE_INVITES`, or (for instance-only invites) instance admins.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(code): Path<String>,
) -> ApiResult<StatusCode> {
    let not_found = || Error::not_found("invite");
    if !invites::is_valid_code(&code) {
        return Err(not_found());
    }
    let row = db::invites::get(&state.db.read, &code)
        .await?
        .ok_or_else(not_found)?;
    {
        let cache = state.cache();
        let allowed = row.creator_id == Some(auth.user_id)
            || match row.space_id {
                Some(space_id) => {
                    let perms = space_permissions(&cache, space_id, auth.user_id)
                        .map_err(|_| not_found())?;
                    perms.contains(Permissions::MANAGE_INVITES)
                }
                None => cache
                    .users
                    .get(&auth.user_id)
                    .is_some_and(|u| u.instance_admin),
            };
        if !allowed {
            return Err(Error::missing_permission("MANAGE_INVITES"));
        }
    }
    if db::invites::delete(&state.db.write, &code).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}
