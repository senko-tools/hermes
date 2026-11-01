//! DM and group DM recipients (PLAN.md section 7.3). Channels themselves are inserted with
//! `spaces::insert_channel` and deleted with `channels::delete_channel`.

use hermes_proto::id::Id;
use sqlx::SqliteConnection;

pub async fn add_recipient(
    conn: &mut SqliteConnection,
    channel_id: Id,
    user_id: Id,
) -> Result<(), sqlx::Error> {
    let (channel_id, user_id) = (channel_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "INSERT INTO dm_recipients (channel_id, user_id) VALUES (?, ?) ON CONFLICT DO NOTHING",
        channel_id,
        user_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn remove_recipient(
    conn: &mut SqliteConnection,
    channel_id: Id,
    user_id: Id,
) -> Result<(), sqlx::Error> {
    let (channel_id, user_id) = (channel_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "DELETE FROM dm_recipients WHERE channel_id = ? AND user_id = ?",
        channel_id,
        user_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Sets the group DM owner.
pub async fn set_owner(
    conn: &mut SqliteConnection,
    channel_id: Id,
    owner_id: Id,
) -> Result<(), sqlx::Error> {
    let (channel_id, owner_id) = (channel_id.to_i64(), owner_id.to_i64());
    sqlx::query!(
        "UPDATE channels SET owner_id = ? WHERE id = ?",
        owner_id,
        channel_id
    )
    .execute(conn)
    .await?;
    Ok(())
}
