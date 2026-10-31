//! Messages (PLAN.md section 7.3).

use hermes_proto::id::Id;
use sqlx::SqliteConnection;

/// After a soft delete: removes the message's reactions and mentions and unlinks its attachments
/// (unattached uploads are cleaned up later).
pub async fn clear_deleted_extras(
    conn: &mut SqliteConnection,
    message_id: Id,
) -> Result<(), sqlx::Error> {
    let id = message_id.to_i64();
    sqlx::query!("DELETE FROM reactions WHERE message_id = ?", id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM message_mentions WHERE message_id = ?", id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "UPDATE attachments SET message_id = NULL WHERE message_id = ?",
        id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}
