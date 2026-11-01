//! Space members and bans (PLAN.md section 7.3). Inserting members is in `spaces`.

use hermes_proto::id::Id;
use sqlx::{SqliteConnection, SqliteExecutor};

/// Removes a membership with its roles (`ON DELETE CASCADE`).
pub async fn delete_member(
    conn: &mut SqliteConnection,
    space_id: Id,
    user_id: Id,
) -> Result<(), sqlx::Error> {
    let (space_id, user_id) = (space_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "DELETE FROM space_members WHERE space_id = ? AND user_id = ?",
        space_id,
        user_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_nickname(
    conn: &mut SqliteConnection,
    space_id: Id,
    user_id: Id,
    nickname: Option<&str>,
) -> Result<(), sqlx::Error> {
    let (space_id, user_id) = (space_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "UPDATE space_members SET nickname = ? WHERE space_id = ? AND user_id = ?",
        nickname,
        space_id,
        user_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Replaces a member's explicitly assigned roles.
pub async fn set_roles(
    conn: &mut SqliteConnection,
    space_id: Id,
    user_id: Id,
    role_ids: &[Id],
) -> Result<(), sqlx::Error> {
    let (space_id, user_id) = (space_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "DELETE FROM member_roles WHERE space_id = ? AND user_id = ?",
        space_id,
        user_id
    )
    .execute(&mut *conn)
    .await?;
    for role_id in role_ids {
        let role_id = role_id.to_i64();
        sqlx::query!(
            "INSERT INTO member_roles (space_id, user_id, role_id) VALUES (?, ?, ?)",
            space_id,
            user_id,
            role_id
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BanRow {
    pub user_id: Id,
    pub reason: Option<String>,
    pub created_at: i64,
}

/// A space's bans, oldest first.
pub async fn list_bans(
    e: impl SqliteExecutor<'_>,
    space_id: Id,
) -> Result<Vec<BanRow>, sqlx::Error> {
    let space_id = space_id.to_i64();
    let rows = sqlx::query!(
        "SELECT user_id, reason, created_at FROM bans WHERE space_id = ?
         ORDER BY created_at, user_id",
        space_id
    )
    .fetch_all(e)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BanRow {
            user_id: Id::from_i64(r.user_id),
            reason: r.reason,
            created_at: r.created_at,
        })
        .collect())
}

/// Bans a user; banning again replaces the reason and the moderator but keeps the date.
pub async fn upsert_ban(
    conn: &mut SqliteConnection,
    space_id: Id,
    user_id: Id,
    reason: Option<&str>,
    banned_by: Id,
    now: i64,
) -> Result<(), sqlx::Error> {
    let (space_id, user_id, banned_by) = (space_id.to_i64(), user_id.to_i64(), banned_by.to_i64());
    sqlx::query!(
        "INSERT INTO bans (space_id, user_id, reason, banned_by, created_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (space_id, user_id)
         DO UPDATE SET reason = excluded.reason, banned_by = excluded.banned_by",
        space_id,
        user_id,
        reason,
        banned_by,
        now
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// `false` if the user was not banned.
pub async fn delete_ban(
    conn: &mut SqliteConnection,
    space_id: Id,
    user_id: Id,
) -> Result<bool, sqlx::Error> {
    let (space_id, user_id) = (space_id.to_i64(), user_id.to_i64());
    let done = sqlx::query!(
        "DELETE FROM bans WHERE space_id = ? AND user_id = ?",
        space_id,
        user_id
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Soft-deletes `author_id`'s messages in the space's channels with an ID of at least `min_id`
/// (content cleared, unpinned, reactions and mentions removed, attachments unlinked). Returns
/// `(message_id, channel_id)` of the messages deleted now.
pub async fn soft_delete_messages_since(
    conn: &mut SqliteConnection,
    space_id: Id,
    author_id: Id,
    min_id: Id,
) -> Result<Vec<(Id, Id)>, sqlx::Error> {
    let (space_id, author_id, min_id) = (space_id.to_i64(), author_id.to_i64(), min_id.to_i64());
    let rows = sqlx::query!(
        "UPDATE messages SET content = '', deleted = 1, pinned = 0
         WHERE author_id = ? AND id >= ? AND deleted = 0
           AND channel_id IN (SELECT id FROM channels WHERE space_id = ?)
         RETURNING id, channel_id",
        author_id,
        min_id,
        space_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in &rows {
        super::messages::clear_deleted_extras(&mut *conn, Id::from_i64(row.id)).await?;
    }
    Ok(rows
        .into_iter()
        .map(|r| (Id::from_i64(r.id), Id::from_i64(r.channel_id)))
        .collect())
}
