//! Space channels and permission overrides (PLAN.md section 7.3). Inserting is in `spaces`.

use hermes_proto::id::Id;
use hermes_proto::models::{Channel, Override};
use sqlx::SqliteConnection;

/// Writes the editable columns of `channel`: name, topic, parent, position, bitrate, user limit
/// and slowmode.
pub async fn update_channel(
    conn: &mut SqliteConnection,
    channel: &Channel,
) -> Result<(), sqlx::Error> {
    let id = channel.id.to_i64();
    let parent_id = channel.parent_id.map(Id::to_i64);
    let bitrate = channel.bitrate.map(i64::from);
    let user_limit = channel.user_limit.map(i64::from);
    let slowmode = i64::from(channel.slowmode_secs);
    sqlx::query!(
        "UPDATE channels SET name = ?, topic = ?, parent_id = ?, position = ?, bitrate = ?,
                             user_limit = ?, slowmode_secs = ?
         WHERE id = ?",
        channel.name,
        channel.topic,
        parent_id,
        channel.position,
        bitrate,
        user_limit,
        slowmode,
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Deletes a channel with its messages and overrides. Children of a deleted category move to the
/// top level (`ON DELETE SET NULL`).
pub async fn delete_channel(
    conn: &mut SqliteConnection,
    channel_id: Id,
) -> Result<(), sqlx::Error> {
    let id = channel_id.to_i64();
    sqlx::query!("DELETE FROM channels WHERE id = ?", id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Inserts or replaces the override of `o.role_id` on a channel.
pub async fn put_override(
    conn: &mut SqliteConnection,
    channel_id: Id,
    o: &Override,
) -> Result<(), sqlx::Error> {
    let (channel_id, role_id) = (channel_id.to_i64(), o.role_id.to_i64());
    let (allow, deny) = (o.allow.bits() as i64, o.deny.bits() as i64);
    sqlx::query!(
        "INSERT INTO channel_overrides (channel_id, role_id, allow, deny) VALUES (?, ?, ?, ?)
         ON CONFLICT (channel_id, role_id) DO UPDATE SET allow = excluded.allow, deny = excluded.deny",
        channel_id,
        role_id,
        allow,
        deny
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// `false` if the override did not exist.
pub async fn delete_override(
    conn: &mut SqliteConnection,
    channel_id: Id,
    role_id: Id,
) -> Result<bool, sqlx::Error> {
    let (channel_id, role_id) = (channel_id.to_i64(), role_id.to_i64());
    let done = sqlx::query!(
        "DELETE FROM channel_overrides WHERE channel_id = ? AND role_id = ?",
        channel_id,
        role_id
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}
