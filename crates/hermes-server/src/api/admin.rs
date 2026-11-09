//! `/admin/users` (PLAN.md section 10): instance admins list users, disable them and give or
//! take admin rights.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{
    AdminLogEntry, AdminSettings, AdminUser, AdminUserDetail, AdminUserPatch, Pagination,
};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;

use super::{ApiResult, Error, Json, Path, Query};
use crate::auth::Auth;
use crate::db;
use crate::gateway::bus::Audience;
use crate::state::AppState;
use crate::time::now_ms;
use sqlx::Row as _;

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

async fn write_log(
    state: &AppState,
    actor: Id,
    target: Option<Id>,
    action: &str,
    details: String,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO admin_log (id, actor_id, target_id, action, details, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(state.ids.next().to_i64())
    .bind(actor.to_i64())
    .bind(target.map(Id::to_i64))
    .bind(action)
    .bind(details)
    .bind(now_ms())
    .execute(&state.db.write)
    .await?;
    Ok(())
}

pub async fn log(
    State(state): State<Arc<AppState>>,
    auth: Auth,
) -> ApiResult<Json<Vec<AdminLogEntry>>> {
    require_admin(&state, auth.user_id)?;
    let rows = sqlx::query("SELECT * FROM admin_log ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db.read)
        .await?;
    let entries = rows
        .into_iter()
        .map(|r| {
            Ok(AdminLogEntry {
                id: Id::from_i64(r.try_get("id")?),
                actor_id: Id::from_i64(r.try_get("actor_id")?),
                target_id: r.try_get::<Option<i64>, _>("target_id")?.map(Id::from_i64),
                action: r.try_get("action")?,
                details: r.try_get("details")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(Json(entries))
}

pub async fn user_detail(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(user): Path<Id>,
) -> ApiResult<Json<AdminUserDetail>> {
    require_admin(&state, auth.user_id)?;
    let spaces = {
        let cache = state.cache();
        if !cache.users.contains_key(&user) {
            return Err(Error::not_found("user"));
        }
        cache
            .spaces
            .values()
            .filter(|s| s.members.contains_key(&user))
            .count() as u32
    };
    let db = &state.db.read;
    let created_at: i64 = sqlx::query_scalar("SELECT created_at FROM users WHERE id = ?")
        .bind(user.to_i64())
        .fetch_one(db)
        .await?;
    Ok(Json(AdminUserDetail {
        created_at,
        last_online_at: db::users::last_online(db, user).await?,
        sessions: db::auth::list_sessions(db, user).await?.len() as u32,
        storage_bytes: db::attachments::total_size_of(db, user).await?,
        spaces,
    }))
}

pub async fn settings(
    State(state): State<Arc<AppState>>,
    auth: Auth,
) -> ApiResult<Json<AdminSettings>> {
    require_admin(&state, auth.user_id)?;
    Ok(Json(AdminSettings {
        registration_mode: state.registration_mode(),
    }))
}

pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<AdminSettings>,
) -> ApiResult<Json<AdminSettings>> {
    require_admin(&state, auth.user_id)?;
    let value = serde_json::to_string(&req.registration_mode)
        .map_err(|err| Error::internal(format!("registration mode: {err}")))?;
    sqlx::query(
        "INSERT INTO instance_settings (key, value) VALUES ('registration_mode', ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(&value)
    .execute(&state.db.write)
    .await?;
    state.set_registration_mode(req.registration_mode);
    write_log(&state, auth.user_id, None, "registration_mode", value).await?;
    Ok(Json(req))
}

pub async fn reports(
    State(state): State<Arc<AppState>>,
    auth: Auth,
) -> ApiResult<Json<Vec<hermes_proto::models::Report>>> {
    require_admin(&state, auth.user_id)?;
    Ok(Json(super::reports::list_unowned(&state).await?))
}

pub async fn resolve_report(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(report): Path<Id>,
) -> ApiResult<StatusCode> {
    require_admin(&state, auth.user_id)?;
    super::reports::resolve_unowned(&state, report).await?;
    Ok(StatusCode::NO_CONTENT)
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
    if user_id != auth.user_id
        && (req.disabled == Some(true) || req.instance_admin == Some(false))
        && state.cache().first_admin() == Some(user_id)
    {
        return Err(Error::new(
            ErrorCode::MissingPermission,
            "The instance's first admin cannot be disabled or demoted by anyone else",
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
    let renamed = match req.tagless {
        Some(tagless) if tagless == current.user.discriminator.is_none() => None,
        Some(tagless) => {
            let mut user = current.user.clone();
            let cache_answer = {
                let cache = state.cache();
                if tagless {
                    (!cache.handle_taken(&user.username, None, user_id)).then_some(None)
                } else {
                    cache.free_tag(&user.username).map(Some)
                }
            };
            let Some(tag) = cache_answer else {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "Another account already uses this name without a tag",
                ));
            };
            user.discriminator = tag;
            db::users::set_handle(&mut tx, user_id, &user.handle()).await?;
            Some(user)
        }
        None => None,
    };
    tx.commit().await?;
    if let Some(user) = state.cache_mut().users.get_mut(&user_id) {
        user.disabled = disabled;
        user.instance_admin = instance_admin;
    }
    if let Some(user) = renamed {
        let audience = {
            let mut cache = state.cache_mut();
            if let Some(cached) = cache.users.get_mut(&user_id) {
                cached.user = user.clone();
            }
            cache.profile_audience(user_id)
        };
        state.publish(
            ServerEvent::UserUpdate(user),
            Audience::Users(audience.into_iter().collect()),
        );
    }
    state.gateway.revoke(&revoked);
    super::devices::revoke_sessions(&state, &revoked).await?;
    tracing::info!(%user_id, disabled, instance_admin, by = %auth.user_id, "admin changed a user");
    write_log(
        &state,
        auth.user_id,
        Some(user_id),
        "user_update",
        format!("disabled: {disabled}, admin: {instance_admin}"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
