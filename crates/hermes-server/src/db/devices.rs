//! The `devices` table: public keys of users' devices (docs/e2ee.md).

use hermes_proto::e2ee::{DeviceKeys, RegisterDeviceRequest};
use hermes_proto::id::Id;
use sqlx::{SqliteConnection, SqlitePool};

/// Every device of `user_id`, revoked ones included, oldest first.
pub async fn of_user(pool: &SqlitePool, user_id: Id) -> Result<Vec<DeviceKeys>, sqlx::Error> {
    let id = user_id.to_i64();
    let rows = sqlx::query!(
        "SELECT device_id, sign_key, dh_key, signature, revoked_at FROM devices
         WHERE user_id = ? ORDER BY created_at, device_id",
        id
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DeviceKeys {
            device_id: r.device_id,
            sign_key: r.sign_key,
            dh_key: r.dh_key,
            signature: r.signature,
            revoked: r.revoked_at.is_some(),
        })
        .collect())
}

/// How many devices of `user_id` are in use, not counting `except`.
pub async fn count_live(pool: &SqlitePool, user_id: Id, except: &str) -> Result<i64, sqlx::Error> {
    let id = user_id.to_i64();
    sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM devices
           WHERE user_id = ? AND revoked_at IS NULL AND device_id <> ?"#,
        id,
        except
    )
    .fetch_one(pool)
    .await
}

/// Adds a device or brings one back. Its keys never change: a device ID is a hash of its signing
/// key, so a different key under a known ID is refused (`false`).
pub async fn register(
    conn: &mut SqliteConnection,
    user_id: Id,
    session_id: Id,
    req: &RegisterDeviceRequest,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let (user, session) = (user_id.to_i64(), session_id.to_i64());
    let done = sqlx::query!(
        "INSERT INTO devices (user_id, device_id, sign_key, dh_key, signature, session_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, device_id) DO UPDATE
         SET session_id = excluded.session_id, revoked_at = NULL
         WHERE sign_key = excluded.sign_key AND dh_key = excluded.dh_key",
        user,
        req.device_id,
        req.sign_key,
        req.dh_key,
        req.signature,
        session,
        now
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Whether this exact device is live under this session already.
pub async fn is_live(
    pool: &SqlitePool,
    user_id: Id,
    req: &RegisterDeviceRequest,
    session_id: Id,
) -> Result<bool, sqlx::Error> {
    let (user, session) = (user_id.to_i64(), session_id.to_i64());
    let found = sqlx::query_scalar!(
        "SELECT session_id FROM devices
         WHERE user_id = ? AND device_id = ? AND sign_key = ? AND dh_key = ?
               AND session_id = ? AND revoked_at IS NULL",
        user,
        req.device_id,
        req.sign_key,
        req.dh_key,
        session
    )
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

/// Revokes the devices these sessions registered; returns their users, once each.
pub async fn revoke_sessions(
    conn: &mut SqliteConnection,
    session_ids: &[Id],
    now: i64,
) -> Result<Vec<Id>, sqlx::Error> {
    let mut users = Vec::new();
    for session in session_ids {
        let session = session.to_i64();
        let rows = sqlx::query_scalar!(
            "UPDATE devices SET revoked_at = ?
             WHERE session_id = ? AND revoked_at IS NULL RETURNING user_id",
            now,
            session
        )
        .fetch_all(&mut *conn)
        .await?;
        for user in rows {
            let user = Id::from_i64(user);
            if !users.contains(&user) {
                users.push(user);
            }
        }
    }
    Ok(users)
}
