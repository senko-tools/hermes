//! What someone does with their own account: two-factor login, blocking, export and deletion.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{
    DeleteAccountRequest, TwoFactorCode, TwoFactorDisable, TwoFactorPassword, TwoFactorSecret,
    TwoFactorStatus,
};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use serde_json::{Value, json};
use sqlx::Row as _;

use super::{ApiResult, Error, Json, Path};
use crate::auth::{self, Auth, totp};
use crate::db;
use crate::gateway::bus::Audience;
use crate::ratelimit::Action;
use crate::state::AppState;
use crate::time::now_ms;

/// Messages an export holds at most.
const EXPORT_MESSAGES: i64 = 100_000;

async fn confirm_password(state: &AppState, user: Id, password: String) -> ApiResult<()> {
    let hash = db::auth::password_hash(&state.db.read, user).await?;
    if auth::verify_password(password, hash).await? {
        Ok(())
    } else {
        Err(Error::new(ErrorCode::InvalidCredentials, "Wrong password"))
    }
}

/// The secret in use, or the one waiting to be confirmed.
async fn totp_column(state: &AppState, user: Id, pending: bool) -> ApiResult<Option<String>> {
    let sql = if pending {
        "SELECT totp_pending FROM users WHERE id = ?"
    } else {
        "SELECT totp_secret FROM users WHERE id = ?"
    };
    let row = sqlx::query(sql)
        .bind(user.to_i64())
        .fetch_one(&state.db.read)
        .await?;
    Ok(row.try_get(0)?)
}

fn now_secs() -> u64 {
    u64::try_from(now_ms() / 1000).unwrap_or(0)
}

fn wrong_code() -> Error {
    Error::new(ErrorCode::InvalidCredentials, "That code is not right")
}

pub async fn two_factor(
    State(state): State<Arc<AppState>>,
    auth: Auth,
) -> ApiResult<Json<TwoFactorStatus>> {
    let enabled = totp_column(&state, auth.user_id, false).await?.is_some();
    Ok(Json(TwoFactorStatus { enabled }))
}

/// Makes a secret to put in an authenticator; nothing changes until a code from it is confirmed.
pub async fn two_factor_begin(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<TwoFactorPassword>,
) -> ApiResult<Json<TwoFactorSecret>> {
    state.throttle(auth.user_id, Action::Password)?;
    confirm_password(&state, auth.user_id, req.password).await?;
    let secret = totp::new_secret();
    sqlx::query("UPDATE users SET totp_pending = ? WHERE id = ?")
        .bind(&secret)
        .bind(auth.user_id.to_i64())
        .execute(&state.db.write)
        .await?;
    Ok(Json(TwoFactorSecret { secret }))
}

pub async fn two_factor_confirm(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<TwoFactorCode>,
) -> ApiResult<StatusCode> {
    state.throttle(auth.user_id, Action::Password)?;
    let pending = totp_column(&state, auth.user_id, true).await?;
    let secret =
        pending.ok_or_else(|| Error::bad_request("Start setting up two-factor login first"))?;
    if !totp::check(&secret, &req.code, now_secs()) {
        return Err(wrong_code());
    }
    sqlx::query("UPDATE users SET totp_secret = totp_pending, totp_pending = NULL WHERE id = ?")
        .bind(auth.user_id.to_i64())
        .execute(&state.db.write)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn two_factor_disable(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<TwoFactorDisable>,
) -> ApiResult<StatusCode> {
    state.throttle(auth.user_id, Action::Password)?;
    confirm_password(&state, auth.user_id, req.password).await?;
    let secret = totp_column(&state, auth.user_id, false).await?;
    if secret.is_some_and(|secret| !totp::check(&secret, &req.code, now_secs())) {
        return Err(wrong_code());
    }
    sqlx::query("UPDATE users SET totp_secret = NULL, totp_pending = NULL WHERE id = ?")
        .bind(auth.user_id.to_i64())
        .execute(&state.db.write)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn blocks(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<Id>>> {
    let cache = state.cache();
    let mut list: Vec<Id> = cache
        .blocks
        .get(&auth.user_id)
        .into_iter()
        .flatten()
        .copied()
        .collect();
    list.sort();
    Ok(Json(list))
}

pub async fn block(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(user): Path<Id>,
) -> ApiResult<StatusCode> {
    if user == auth.user_id {
        return Err(Error::bad_request("You cannot block yourself"));
    }
    if !state.cache().users.contains_key(&user) {
        return Err(Error::not_found("user"));
    }
    sqlx::query(
        "INSERT INTO user_blocks (user_id, blocked_id, created_at) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(auth.user_id.to_i64())
    .bind(user.to_i64())
    .bind(now_ms())
    .execute(&state.db.write)
    .await?;
    state
        .cache_mut()
        .blocks
        .entry(auth.user_id)
        .or_default()
        .insert(user);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unblock(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(user): Path<Id>,
) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM user_blocks WHERE user_id = ? AND blocked_id = ?")
        .bind(auth.user_id.to_i64())
        .bind(user.to_i64())
        .execute(&state.db.write)
        .await?;
    if let Some(list) = state.cache_mut().blocks.get_mut(&auth.user_id) {
        list.remove(&user);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Everything the server holds about me, as JSON. Encrypted messages stay as they are stored.
pub async fn export(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Value>> {
    state.throttle(auth.user_id, Action::Password)?;
    let id = auth.user_id.to_i64();
    let db = &state.db.read;
    let messages = sqlx::query(
        "SELECT id, channel_id, content, e2ee, edited_at, deleted FROM messages
         WHERE author_id = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(id)
    .bind(EXPORT_MESSAGES)
    .fetch_all(db)
    .await?;
    let messages: Vec<Value> = messages
        .iter()
        .map(|m| {
            let e2ee: Option<String> = m.try_get("e2ee").unwrap_or_default();
            json!({
                "id": m.try_get::<i64, _>("id").unwrap_or_default().to_string(),
                "channel_id": m.try_get::<i64, _>("channel_id").unwrap_or_default().to_string(),
                "content": m.try_get::<String, _>("content").unwrap_or_default(),
                "sealed": e2ee.is_some(),
                "edited_at": m.try_get::<Option<i64>, _>("edited_at").unwrap_or_default(),
                "deleted": m.try_get::<bool, _>("deleted").unwrap_or_default(),
            })
        })
        .collect();
    let files = sqlx::query(
        "SELECT id, filename, content_type, size, created_at FROM attachments WHERE uploader_id = ?",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let files: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({
                "id": f.try_get::<i64, _>("id").unwrap_or_default().to_string(),
                "filename": f.try_get::<String, _>("filename").unwrap_or_default(),
                "content_type": f.try_get::<String, _>("content_type").unwrap_or_default(),
                "size": f.try_get::<i64, _>("size").unwrap_or_default(),
                "created_at": f.try_get::<i64, _>("created_at").unwrap_or_default(),
            })
        })
        .collect();
    let sessions = db::auth::list_sessions(db, auth.user_id).await?;
    let sessions: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "device_name": s.device_name,
                "created_at": s.created_at,
                "last_seen_at": s.last_seen_at,
            })
        })
        .collect();
    let cache = state.cache();
    let me = cache
        .users
        .get(&auth.user_id)
        .ok_or_else(|| Error::internal("authenticated user missing from the cache"))?;
    let spaces: Vec<Value> = cache
        .spaces
        .values()
        .filter(|s| s.members.contains_key(&auth.user_id))
        .map(|s| json!({ "id": s.space.id, "name": s.space.name }))
        .collect();
    Ok(Json(json!({
        "exported_at": now_ms(),
        "user": me.user,
        "privacy": me.privacy,
        "instance_admin": me.instance_admin,
        "spaces": spaces,
        "friends": cache.relationships_of(auth.user_id),
        "blocked": cache.blocks.get(&auth.user_id),
        "sessions": sessions,
        "messages": messages,
        "files": files,
    })))
}

/// Removes what identifies the person. Messages stay under a nameless account unless asked to
/// go too, as other people's conversations would otherwise lose their sense.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<DeleteAccountRequest>,
) -> ApiResult<StatusCode> {
    state.throttle(auth.user_id, Action::Password)?;
    confirm_password(&state, auth.user_id, req.password).await?;
    let _writes = state.writes.lock().await;
    let spaces: Vec<Id> = {
        let cache = state.cache();
        if cache
            .spaces
            .values()
            .any(|s| s.space.owner_id == auth.user_id)
        {
            return Err(Error::bad_request(
                "Give away or delete the servers you own first",
            ));
        }
        cache
            .spaces
            .values()
            .filter(|s| s.members.contains_key(&auth.user_id))
            .map(|s| s.space.id)
            .collect()
    };
    let sessions: Vec<Id> = db::auth::list_sessions(&state.db.read, auth.user_id)
        .await?
        .iter()
        .map(|s| s.id)
        .collect();
    super::devices::revoke_sessions(&state, &sessions).await?;

    let id = auth.user_id.to_i64();
    let name = format!("deleted-{}", auth.user_id);
    let mut tx = state.db.write.begin().await?;
    for sql in [
        "DELETE FROM space_members WHERE user_id = ?",
        "DELETE FROM sessions WHERE user_id = ?",
        "DELETE FROM relationships WHERE user_low = ?1 OR user_high = ?1",
        "DELETE FROM user_blocks WHERE user_id = ?1 OR blocked_id = ?1",
        "DELETE FROM notification_settings WHERE user_id = ?",
    ] {
        sqlx::query(sql).bind(id).execute(&mut *tx).await?;
    }
    sqlx::query(
        "UPDATE users SET username = ?, display_name = NULL, status_text = NULL, avatar_id = NULL,
           banner_id = NULL, password_hash = '!', disabled = 1, totp_secret = NULL,
           totp_pending = NULL, last_online_at = NULL
         WHERE id = ?",
    )
    .bind(&name)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let mut by_channel: HashMap<Id, Vec<Id>> = HashMap::new();
    if req.delete_messages {
        let rows = sqlx::query(
            "UPDATE messages SET content = '', e2ee = NULL, deleted = 1, pinned = 0
             WHERE author_id = ? AND deleted = 0 RETURNING id, channel_id",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let mut all = Vec::with_capacity(rows.len());
        for row in rows {
            let message = Id::from_i64(row.try_get("id")?);
            all.push(message);
            by_channel
                .entry(Id::from_i64(row.try_get("channel_id")?))
                .or_default()
                .push(message);
        }
        db::messages::clear_deleted_extras_of(&mut tx, &all).await?;
    }
    tx.commit().await?;

    let user = {
        let mut cache = state.cache_mut();
        for space in &spaces {
            cache.remove_member(*space, auth.user_id);
        }
        if let Some(others) = cache.relationships.remove(&auth.user_id) {
            for other in others.keys() {
                if let Some(list) = cache.relationships.get_mut(other) {
                    list.remove(&auth.user_id);
                }
            }
        }
        cache.blocks.remove(&auth.user_id);
        for list in cache.blocks.values_mut() {
            list.remove(&auth.user_id);
        }
        cache.users.get_mut(&auth.user_id).map(|cached| {
            cached.disabled = true;
            cached.user.username = name;
            cached.user.discriminator = None;
            cached.user.display_name = None;
            cached.user.avatar = None;
            cached.user.banner = None;
            cached.user.clone()
        })
    };
    state.gateway.revoke(&sessions);
    for space in spaces {
        super::members::publish_removal(&state, space, auth.user_id);
    }
    for (channel_id, mut ids) in by_channel {
        ids.sort();
        state.publish(
            ServerEvent::MessageDeleteBulk { ids, channel_id },
            Audience::Channel(channel_id),
        );
    }
    if let Some(user) = user {
        let audience = state.cache().profile_audience(auth.user_id);
        state.publish(
            ServerEvent::UserUpdate(user),
            Audience::Users(audience.into_iter().collect()),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}
