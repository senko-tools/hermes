//! DM and group DM recipients (PLAN.md section 7.3). Channels themselves are inserted with
//! `spaces::insert_channel` and deleted with `channels::delete_channel`.

use hermes_proto::id::Id;
use sqlx::SqliteConnection;

/// `request`: the DM is a message request for this recipient.
pub async fn add_recipient(
    conn: &mut SqliteConnection,
    channel_id: Id,
    user_id: Id,
    request: bool,
) -> Result<(), sqlx::Error> {
    let (channel_id, user_id) = (channel_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "INSERT INTO dm_recipients (channel_id, user_id, request) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
        channel_id,
        user_id,
        request
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

/// Closes (or reopens) a DM for one recipient.
pub async fn set_closed(
    conn: &mut SqliteConnection,
    channel_id: Id,
    user_id: Id,
    closed: bool,
) -> Result<(), sqlx::Error> {
    let (channel_id, user_id) = (channel_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "UPDATE dm_recipients SET closed = ? WHERE channel_id = ? AND user_id = ?",
        closed,
        channel_id,
        user_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Accepts a message request: the DM becomes a normal (and open) one for this recipient.
pub async fn accept_request(
    conn: &mut SqliteConnection,
    channel_id: Id,
    user_id: Id,
) -> Result<(), sqlx::Error> {
    let (channel_id, user_id) = (channel_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "UPDATE dm_recipients SET request = 0, closed = 0 WHERE channel_id = ? AND user_id = ?",
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
