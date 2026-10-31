//! Users and sessions (PLAN.md sections 7.3 and 13).

use hermes_proto::id::Id;
use sqlx::{SqliteConnection, SqliteExecutor};

pub async fn count_users(e: impl SqliteExecutor<'_>) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!: i64" FROM users"#)
        .fetch_one(e)
        .await
}

pub async fn username_taken(
    e: impl SqliteExecutor<'_>,
    username: &str,
) -> Result<bool, sqlx::Error> {
    let found = sqlx::query_scalar!("SELECT id FROM users WHERE username = ?", username)
        .fetch_optional(e)
        .await?;
    Ok(found.is_some())
}

/// What login needs to know about a user.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub user_id: Id,
    pub password_hash: String,
    pub disabled: bool,
}

pub async fn credentials_by_username(
    e: impl SqliteExecutor<'_>,
    username: &str,
) -> Result<Option<Credentials>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, password_hash, disabled AS "disabled: bool" FROM users WHERE username = ?"#,
        username
    )
    .fetch_optional(e)
    .await?;
    Ok(row.map(|r| Credentials {
        user_id: Id::from_i64(r.id),
        password_hash: r.password_hash,
        disabled: r.disabled,
    }))
}

pub async fn password_hash(
    e: impl SqliteExecutor<'_>,
    user_id: Id,
) -> Result<Option<String>, sqlx::Error> {
    let id = user_id.to_i64();
    sqlx::query_scalar!("SELECT password_hash FROM users WHERE id = ?", id)
        .fetch_optional(e)
        .await
}

pub struct NewUser<'a> {
    pub id: Id,
    pub username: &'a str,
    pub password_hash: &'a str,
    pub instance_admin: bool,
    pub created_at: i64,
}

pub async fn insert_user(
    conn: &mut SqliteConnection,
    user: &NewUser<'_>,
) -> Result<(), sqlx::Error> {
    let id = user.id.to_i64();
    sqlx::query!(
        "INSERT INTO users (id, username, password_hash, instance_admin, created_at)
         VALUES (?, ?, ?, ?, ?)",
        id,
        user.username,
        user.password_hash,
        user.instance_admin,
        user.created_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_password_hash(
    conn: &mut SqliteConnection,
    user_id: Id,
    hash: &str,
) -> Result<(), sqlx::Error> {
    let id = user_id.to_i64();
    sqlx::query!("UPDATE users SET password_hash = ? WHERE id = ?", hash, id)
        .execute(conn)
        .await?;
    Ok(())
}

pub struct NewSession<'a> {
    pub id: Id,
    pub user_id: Id,
    pub token_hash: &'a [u8],
    pub device_name: &'a str,
    pub created_at: i64,
    pub ip: Option<&'a str>,
}

pub async fn insert_session(
    conn: &mut SqliteConnection,
    s: &NewSession<'_>,
) -> Result<(), sqlx::Error> {
    let (id, user_id) = (s.id.to_i64(), s.user_id.to_i64());
    sqlx::query!(
        "INSERT INTO sessions (id, user_id, token_hash, device_name, created_at, last_seen_at, last_ip)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        id,
        user_id,
        s.token_hash,
        s.device_name,
        s.created_at,
        s.created_at,
        s.ip
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// The session a bearer token belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionRow {
    pub id: Id,
    pub user_id: Id,
    pub last_seen_at: i64,
}

pub async fn session_by_token_hash(
    e: impl SqliteExecutor<'_>,
    token_hash: &[u8],
) -> Result<Option<SessionRow>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT id, user_id, last_seen_at FROM sessions WHERE token_hash = ?",
        token_hash
    )
    .fetch_optional(e)
    .await?;
    Ok(row.map(|r| SessionRow {
        id: Id::from_i64(r.id),
        user_id: Id::from_i64(r.user_id),
        last_seen_at: r.last_seen_at,
    }))
}

pub async fn touch_session(
    e: impl SqliteExecutor<'_>,
    session_id: Id,
    now: i64,
    ip: Option<&str>,
) -> Result<(), sqlx::Error> {
    let id = session_id.to_i64();
    sqlx::query!(
        "UPDATE sessions SET last_seen_at = ?, last_ip = COALESCE(?, last_ip) WHERE id = ?",
        now,
        ip,
        id
    )
    .execute(e)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListRow {
    pub id: Id,
    pub device_name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// A user's sessions, oldest first.
pub async fn list_sessions(
    e: impl SqliteExecutor<'_>,
    user_id: Id,
) -> Result<Vec<SessionListRow>, sqlx::Error> {
    let id = user_id.to_i64();
    let rows = sqlx::query!(
        "SELECT id, device_name, created_at, last_seen_at FROM sessions
         WHERE user_id = ? ORDER BY id",
        id
    )
    .fetch_all(e)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SessionListRow {
            id: Id::from_i64(r.id),
            device_name: r.device_name,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
        })
        .collect())
}

/// Deletes one session of `user_id`; `false` if it does not exist or belongs to someone else.
pub async fn delete_session(
    e: impl SqliteExecutor<'_>,
    user_id: Id,
    session_id: Id,
) -> Result<bool, sqlx::Error> {
    let (uid, sid) = (user_id.to_i64(), session_id.to_i64());
    let done = sqlx::query!(
        "DELETE FROM sessions WHERE id = ? AND user_id = ?",
        sid,
        uid
    )
    .execute(e)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Deletes every session of `user_id` except `keep`; returns the deleted IDs.
pub async fn delete_other_sessions(
    conn: &mut SqliteConnection,
    user_id: Id,
    keep: Id,
) -> Result<Vec<Id>, sqlx::Error> {
    let (uid, keep) = (user_id.to_i64(), keep.to_i64());
    let ids = sqlx::query_scalar!(
        "DELETE FROM sessions WHERE user_id = ? AND id != ? RETURNING id",
        uid,
        keep
    )
    .fetch_all(conn)
    .await?;
    Ok(ids.into_iter().map(Id::from_i64).collect())
}
