//! `/admin/users` (PLAN.md section 10): instance admins list users, disable them and give or
//! take admin rights.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{AdminUser, AdminUserPatch, Pagination};
use hermes_proto::id::Id;

use super::{ApiResult, Error, Json, Path, Query};
use crate::auth::Auth;
use crate::db;
use crate::state::AppState;

fn require_admin(state: &AppState, user_id: Id) -> ApiResult<()> {
    if state
        .cache()
        .users
        .get(&user_id)
        .is_some_and(|u| u.instance_admin)
    {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::MissingPermission,
            "Only instance admins can do this",
        ))
    }
}

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Query(q): Query<Pagination>,
) -> ApiResult<Json<Vec<AdminUser>>> {
    require_admin(&state, auth.user_id)?;
    let cache = state.cache();
    let mut users: Vec<AdminUser> = cache
        .users
        .values()
        .filter(|u| q.after.is_none_or(|after| u.user.id > after))
        .map(|u| AdminUser {
            user: u.user.clone(),
            disabled: u.disabled,
            instance_admin: u.instance_admin,
        })
        .collect();
    users.sort_by_key(|u| u.user.id);
    users.truncate(q.limit() as usize);
    Ok(Json(users))
}

/// Disabling signs the user out everywhere. Admins cannot disable or demote themselves, so an
/// instance always keeps the admin who made the change.
pub async fn update_user(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(user_id): Path<Id>,
    Json(req): Json<AdminUserPatch>,
) -> ApiResult<StatusCode> {
    require_admin(&state, auth.user_id)?;
    let _writes = state.writes.lock().await;
    let current = state
        .cache()
        .users
        .get(&user_id)
        .cloned()
        .ok_or_else(|| Error::not_found("user"))?;
    if user_id == auth.user_id && (req.disabled == Some(true) || req.instance_admin == Some(false))
    {
        return Err(Error::new(
            ErrorCode::BadRequest,
            "You cannot disable yourself or remove your own admin rights",
        ));
    }
    let disabled = req.disabled.unwrap_or(current.disabled);
    let instance_admin = req.instance_admin.unwrap_or(current.instance_admin);
    let mut tx = state.db.write.begin().await?;
    db::users::set_flags(&mut tx, user_id, disabled, instance_admin).await?;
    let revoked = if disabled && !current.disabled {
        db::auth::delete_other_sessions(&mut tx, user_id, Id(0)).await?
    } else {
        Vec::new()
    };
    tx.commit().await?;
    if let Some(user) = state.cache_mut().users.get_mut(&user_id) {
        user.disabled = disabled;
        user.instance_admin = instance_admin;
    }
    state.gateway.revoke(&revoked);
    tracing::info!(%user_id, disabled, instance_admin, by = %auth.user_id, "admin changed a user");
    Ok(StatusCode::NO_CONTENT)
}
