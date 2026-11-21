//! Sealed channel keys, who is waiting for them, and who may get them (docs/e2ee.md).

use hermes_proto::e2ee::{KeyHandover, KeyPolicy, KeyPolicyOverride, KeyRequest, SealedChannelKey};
use hermes_proto::id::Id;
use sqlx::{SqliteConnection, SqlitePool};

fn handover_name(handover: KeyHandover) -> &'static str {
    match handover {
        KeyHandover::Automatic => "automatic",
        KeyHandover::Approved => "approved",
    }
}

fn parse_handover(name: &str) -> KeyHandover {
    match name {
        "approved" => KeyHandover::Approved,
        _ => KeyHandover::Automatic,
    }
}

/// The newest epoch of a channel; 0 while it has none.
pub async fn current_epoch(pool: &SqlitePool, channel: Id) -> Result<u32, sqlx::Error> {
    let id = channel.to_i64();
    let epoch = sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(epoch), 0) AS "epoch!: i64" FROM channel_key_epochs
           WHERE channel_id = ?"#,
        id
    )
    .fetch_one(pool)
    .await?;
    Ok(epoch as u32)
}

/// Whether epoch `epoch` of the channel exists.
pub async fn has_epoch(pool: &SqlitePool, channel: Id, epoch: u32) -> Result<bool, sqlx::Error> {
    let id = channel.to_i64();
    let found = sqlx::query_scalar!(
        "SELECT 1 FROM channel_key_epochs WHERE channel_id = ? AND epoch = ?",
        id,
        epoch
    )
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

/// Starts `epoch` if it is the next one. `false` if someone else got there first.
pub async fn start_epoch(
    conn: &mut SqliteConnection,
    channel: Id,
    epoch: u32,
    by: Id,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let (id, by) = (channel.to_i64(), by.to_i64());
    let current = sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(epoch), 0) AS "epoch!: i64" FROM channel_key_epochs
           WHERE channel_id = ?"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if i64::from(epoch) != current + 1 {
        return Ok(false);
    }
    sqlx::query!(
        "INSERT INTO channel_key_epochs (channel_id, epoch, created_by, created_at)
         VALUES (?, ?, ?, ?)",
        id,
        epoch,
        by,
        now
    )
    .execute(conn)
    .await?;
    Ok(true)
}

/// Stores handed-over keys. A key already there is replaced only by the member who stored it, so
/// nobody can swap the key someone else handed to a device.
pub async fn store_keys(
    conn: &mut SqliteConnection,
    channel: Id,
    keys: &[SealedChannelKey],
) -> Result<(), sqlx::Error> {
    let id = channel.to_i64();
    for key in keys {
        let (user, sender) = (key.user_id.to_i64(), key.sender_id.to_i64());
        sqlx::query!(
            "INSERT INTO channel_keys
               (channel_id, epoch, user_id, device_id, sender_id, sender_device, eph, key, sig)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (channel_id, epoch, user_id, device_id) DO UPDATE SET
               sender_device = excluded.sender_device, eph = excluded.eph,
               key = excluded.key, sig = excluded.sig
             WHERE channel_keys.sender_id = excluded.sender_id",
            id,
            key.epoch,
            user,
            key.device,
            sender,
            key.sender_device,
            key.eph,
            key.key,
            key.sig
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "DELETE FROM channel_key_requests
             WHERE channel_id = ? AND user_id = ? AND device_id = ?",
            id,
            user,
            key.device
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Every key of the channel sealed for this device.
pub async fn keys_for_device(
    pool: &SqlitePool,
    channel: Id,
    user: Id,
    device: &str,
) -> Result<Vec<SealedChannelKey>, sqlx::Error> {
    let (id, user_id) = (channel.to_i64(), user.to_i64());
    let rows = sqlx::query!(
        r#"SELECT epoch AS "epoch!: i64", sender_id, sender_device, eph, key, sig
           FROM channel_keys
           WHERE channel_id = ? AND user_id = ? AND device_id = ? ORDER BY epoch"#,
        id,
        user_id,
        device
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SealedChannelKey {
            epoch: r.epoch as u32,
            user_id: user,
            device: device.to_owned(),
            sender_id: Id::from_i64(r.sender_id),
            sender_device: r.sender_device,
            eph: r.eph,
            key: r.key,
            sig: r.sig,
        })
        .collect())
}

/// Who holds the current key: each user, and whether all their holding devices are still in use.
pub async fn holders(
    pool: &SqlitePool,
    channel: Id,
    epoch: u32,
) -> Result<Vec<(Id, bool)>, sqlx::Error> {
    let id = channel.to_i64();
    let rows = sqlx::query!(
        r#"SELECT k.user_id AS "user_id!: i64",
                  COALESCE(MIN(d.revoked_at IS NULL), 1) AS "live!: i64"
           FROM channel_keys k
           LEFT JOIN devices d ON d.user_id = k.user_id AND d.device_id = k.device_id
           WHERE k.channel_id = ? AND k.epoch = ?
           GROUP BY k.user_id"#,
        id,
        epoch
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (Id::from_i64(r.user_id), r.live != 0))
        .collect())
}

/// Notes that a device is waiting for keys. `false` if it already was.
pub async fn add_request(
    pool: &SqlitePool,
    channel: Id,
    user: Id,
    device: &str,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let (id, user) = (channel.to_i64(), user.to_i64());
    let done = sqlx::query!(
        "INSERT OR IGNORE INTO channel_key_requests (channel_id, user_id, device_id, created_at)
         VALUES (?, ?, ?, ?)",
        id,
        user,
        device,
        now
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Every device waiting for keys, oldest first.
pub async fn all_requests(pool: &SqlitePool) -> Result<Vec<KeyRequest>, sqlx::Error> {
    let rows = sqlx::query!(
        "SELECT channel_id, user_id, device_id FROM channel_key_requests ORDER BY created_at"
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| KeyRequest {
            channel_id: Id::from_i64(r.channel_id),
            user_id: Id::from_i64(r.user_id),
            device: r.device_id,
        })
        .collect())
}

/// What the channel and its server say, the channel's choices first.
pub async fn policy(pool: &SqlitePool, channel: Id, space: Id) -> Result<KeyPolicy, sqlx::Error> {
    let own = channel_policy(pool, channel).await?;
    let default = space_policy(pool, space).await?;
    Ok(KeyPolicy {
        handover: own.handover.unwrap_or(default.handover),
        new_members_read_history: own
            .new_members_read_history
            .unwrap_or(default.new_members_read_history),
    })
}

pub async fn space_policy(pool: &SqlitePool, space: Id) -> Result<KeyPolicy, sqlx::Error> {
    let id = space.to_i64();
    let row = sqlx::query!(
        "SELECT key_handover, new_members_read_history FROM spaces WHERE id = ?",
        id
    )
    .fetch_one(pool)
    .await?;
    Ok(KeyPolicy {
        handover: parse_handover(&row.key_handover),
        new_members_read_history: row.new_members_read_history != 0,
    })
}

pub async fn channel_policy(
    pool: &SqlitePool,
    channel: Id,
) -> Result<KeyPolicyOverride, sqlx::Error> {
    let id = channel.to_i64();
    let row = sqlx::query!(
        "SELECT key_handover, new_members_read_history FROM channels WHERE id = ?",
        id
    )
    .fetch_one(pool)
    .await?;
    Ok(KeyPolicyOverride {
        handover: row.key_handover.as_deref().map(parse_handover),
        new_members_read_history: row.new_members_read_history.map(|v| v != 0),
    })
}

pub async fn set_space_policy(
    pool: &SqlitePool,
    space: Id,
    policy: KeyPolicy,
) -> Result<(), sqlx::Error> {
    let id = space.to_i64();
    let handover = handover_name(policy.handover);
    sqlx::query!(
        "UPDATE spaces SET key_handover = ?, new_members_read_history = ? WHERE id = ?",
        handover,
        policy.new_members_read_history,
        id
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_channel_policy(
    pool: &SqlitePool,
    channel: Id,
    policy: KeyPolicyOverride,
) -> Result<(), sqlx::Error> {
    let id = channel.to_i64();
    let handover = policy.handover.map(handover_name);
    sqlx::query!(
        "UPDATE channels SET key_handover = ?, new_members_read_history = ? WHERE id = ?",
        handover,
        policy.new_members_read_history,
        id
    )
    .execute(pool)
    .await?;
    Ok(())
}
