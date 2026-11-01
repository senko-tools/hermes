//! Messages, mentions, reactions and read states (PLAN.md section 7.3).
//!
//! Batch loads take the message IDs as a JSON array (`json_each`), because SQLite has no array
//! parameters.

use hermes_proto::id::Id;
use hermes_proto::models::{Attachment, Mentions, MessageKind};
use sqlx::{SqliteConnection, SqliteExecutor};

use super::int;

/// A `messages` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: Id,
    pub channel_id: Id,
    pub author_id: Id,
    pub content: String,
    pub reply_to_id: Option<Id>,
    pub edited_at: Option<i64>,
    pub deleted: bool,
    pub pinned: bool,
    pub kind: MessageKind,
}

struct Raw {
    id: i64,
    channel_id: i64,
    author_id: i64,
    content: String,
    reply_to_id: Option<i64>,
    edited_at: Option<i64>,
    deleted: bool,
    pinned: bool,
    kind: String,
}

impl Raw {
    fn into_row(self) -> Result<MessageRow, sqlx::Error> {
        Ok(MessageRow {
            id: Id::from_i64(self.id),
            channel_id: Id::from_i64(self.channel_id),
            author_id: Id::from_i64(self.author_id),
            content: self.content,
            reply_to_id: self.reply_to_id.map(Id::from_i64),
            edited_at: self.edited_at,
            deleted: self.deleted,
            pinned: self.pinned,
            kind: message_kind(&self.kind)?,
        })
    }
}

fn message_kind(kind: &str) -> Result<MessageKind, sqlx::Error> {
    match kind {
        "default" => Ok(MessageKind::Default),
        "system" => Ok(MessageKind::System),
        other => Err(sqlx::Error::ColumnDecode {
            index: "messages.kind".into(),
            source: format!("unknown message kind {other:?}").into(),
        }),
    }
}

fn message_kind_name(kind: MessageKind) -> &'static str {
    match kind {
        MessageKind::Default => "default",
        MessageKind::System => "system",
    }
}

/// The IDs as a JSON array for `json_each`.
fn id_list(ids: &[Id]) -> String {
    serde_json::to_string(&ids.iter().map(|id| id.to_i64()).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".into())
}

fn rows(raw: Vec<Raw>) -> Result<Vec<MessageRow>, sqlx::Error> {
    raw.into_iter().map(Raw::into_row).collect()
}

pub async fn insert(conn: &mut SqliteConnection, m: &MessageRow) -> Result<(), sqlx::Error> {
    let (id, channel_id, author_id) = (m.id.to_i64(), m.channel_id.to_i64(), m.author_id.to_i64());
    let reply_to_id = m.reply_to_id.map(Id::to_i64);
    let kind = message_kind_name(m.kind);
    sqlx::query!(
        "INSERT INTO messages (id, channel_id, author_id, content, reply_to_id, edited_at, deleted,
                               pinned, kind)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        id,
        channel_id,
        author_id,
        m.content,
        reply_to_id,
        m.edited_at,
        m.deleted,
        m.pinned,
        kind
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// A message of `channel_id` (deleted ones included).
pub async fn get(
    e: impl SqliteExecutor<'_>,
    channel_id: Id,
    message_id: Id,
) -> Result<Option<MessageRow>, sqlx::Error> {
    let (channel_id, id) = (channel_id.to_i64(), message_id.to_i64());
    sqlx::query_as!(
        Raw,
        r#"SELECT id, channel_id, author_id, content, reply_to_id, edited_at,
                  deleted AS "deleted: bool", pinned AS "pinned: bool", kind
           FROM messages WHERE channel_id = ? AND id = ?"#,
        channel_id,
        id
    )
    .fetch_optional(e)
    .await?
    .map(Raw::into_row)
    .transpose()
}

/// Up to `limit` messages older than `before`, oldest first.
pub async fn before(
    e: impl SqliteExecutor<'_>,
    channel_id: Id,
    before: Option<Id>,
    limit: u32,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    let channel_id = channel_id.to_i64();
    let before = before.map_or(i64::MAX, Id::to_i64);
    let limit = i64::from(limit);
    let mut found = rows(
        sqlx::query_as!(
            Raw,
            r#"SELECT id, channel_id, author_id, content, reply_to_id, edited_at,
                      deleted AS "deleted: bool", pinned AS "pinned: bool", kind
               FROM messages WHERE channel_id = ? AND id < ? ORDER BY id DESC LIMIT ?"#,
            channel_id,
            before,
            limit
        )
        .fetch_all(e)
        .await?,
    )?;
    found.reverse();
    Ok(found)
}

/// Up to `limit` messages newer than `after`, oldest first.
pub async fn after(
    e: impl SqliteExecutor<'_>,
    channel_id: Id,
    after: Id,
    limit: u32,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    let (channel_id, after) = (channel_id.to_i64(), after.to_i64());
    let limit = i64::from(limit);
    rows(
        sqlx::query_as!(
            Raw,
            r#"SELECT id, channel_id, author_id, content, reply_to_id, edited_at,
                      deleted AS "deleted: bool", pinned AS "pinned: bool", kind
               FROM messages WHERE channel_id = ? AND id > ? ORDER BY id LIMIT ?"#,
            channel_id,
            after,
            limit
        )
        .fetch_all(e)
        .await?,
    )
}

/// Pinned messages of a channel, newest first.
pub async fn pinned(
    e: impl SqliteExecutor<'_>,
    channel_id: Id,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    let channel_id = channel_id.to_i64();
    rows(
        sqlx::query_as!(
            Raw,
            r#"SELECT id, channel_id, author_id, content, reply_to_id, edited_at,
                      deleted AS "deleted: bool", pinned AS "pinned: bool", kind
               FROM messages WHERE channel_id = ? AND pinned = 1 ORDER BY id DESC"#,
            channel_id
        )
        .fetch_all(e)
        .await?,
    )
}

/// `(id, author_id, content, deleted)` of the given messages, for reply excerpts.
pub async fn reply_refs(
    e: impl SqliteExecutor<'_>,
    ids: &[Id],
) -> Result<Vec<(Id, Id, String, bool)>, sqlx::Error> {
    let ids = id_list(ids);
    let rows = sqlx::query!(
        r#"SELECT id, author_id, content, deleted AS "deleted: bool" FROM messages
           WHERE id IN (SELECT value FROM json_each(?))"#,
        ids
    )
    .fetch_all(e)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                Id::from_i64(r.id),
                Id::from_i64(r.author_id),
                r.content,
                r.deleted,
            )
        })
        .collect())
}

/// Attachments of the given messages, `(message_id, attachment)`, in upload order.
pub async fn attachments(
    e: impl SqliteExecutor<'_>,
    ids: &[Id],
) -> Result<Vec<(Id, Attachment)>, sqlx::Error> {
    let ids = id_list(ids);
    sqlx::query!(
        r#"SELECT id, message_id AS "message_id!", filename, content_type, size, width, height,
                  has_thumb AS "has_thumb: bool"
           FROM attachments WHERE message_id IN (SELECT value FROM json_each(?)) ORDER BY id"#,
        ids
    )
    .fetch_all(e)
    .await?
    .into_iter()
    .map(|r| {
        Ok((
            Id::from_i64(r.message_id),
            Attachment {
                id: Id::from_i64(r.id),
                filename: r.filename,
                content_type: r.content_type,
                size: int(r.size, "attachments.size")?,
                width: r.width.map(|w| int(w, "attachments.width")).transpose()?,
                height: r.height.map(|h| int(h, "attachments.height")).transpose()?,
                has_thumb: r.has_thumb,
            },
        ))
    })
    .collect()
}

/// One reaction emoji on one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionCount {
    pub message_id: Id,
    pub emoji: String,
    pub count: u32,
    /// Whether `viewer` reacted with it.
    pub me: bool,
}

/// Reaction counts of the given messages, per message in the order the oldest remaining reaction
/// of each emoji was added.
pub async fn reaction_counts(
    e: impl SqliteExecutor<'_>,
    ids: &[Id],
    viewer: Id,
) -> Result<Vec<ReactionCount>, sqlx::Error> {
    let ids = id_list(ids);
    let viewer = viewer.to_i64();
    sqlx::query!(
        r#"SELECT message_id AS "message_id!", emoji AS "emoji!", COUNT(*) AS "count!: i64",
                  MAX(user_id = ?) AS "me!: bool"
           FROM reactions WHERE message_id IN (SELECT value FROM json_each(?))
           GROUP BY message_id, emoji ORDER BY message_id, MIN(rowid)"#,
        viewer,
        ids
    )
    .fetch_all(e)
    .await?
    .into_iter()
    .map(|r| {
        Ok(ReactionCount {
            message_id: Id::from_i64(r.message_id),
            emoji: r.emoji,
            count: int(r.count, "reactions.count")?,
            me: r.me,
        })
    })
    .collect()
}

/// Stored mentions of the given messages.
pub async fn mentions(
    e: impl SqliteExecutor<'_>,
    ids: &[Id],
) -> Result<Vec<(Id, Mentions)>, sqlx::Error> {
    let ids = id_list(ids);
    let rows = sqlx::query!(
        "SELECT message_id, kind, target_id FROM message_mentions
         WHERE message_id IN (SELECT value FROM json_each(?)) ORDER BY message_id, rowid",
        ids
    )
    .fetch_all(e)
    .await?;
    let mut out: Vec<(Id, Mentions)> = Vec::new();
    for r in rows {
        let message_id = Id::from_i64(r.message_id);
        if out.last().is_none_or(|(id, _)| *id != message_id) {
            out.push((message_id, Mentions::default()));
        }
        let Some((_, m)) = out.last_mut() else {
            continue;
        };
        match (r.kind.as_str(), r.target_id.map(Id::from_i64)) {
            ("user", Some(id)) => m.users.push(id),
            ("role", Some(id)) => m.roles.push(id),
            ("everyone", _) => m.everyone = true,
            ("here", _) => m.here = true,
            _ => {}
        }
    }
    Ok(out)
}

/// Replaces the stored mentions of a message.
pub async fn set_mentions(
    conn: &mut SqliteConnection,
    message_id: Id,
    m: &Mentions,
) -> Result<(), sqlx::Error> {
    let id = message_id.to_i64();
    sqlx::query!("DELETE FROM message_mentions WHERE message_id = ?", id)
        .execute(&mut *conn)
        .await?;
    let targets = m
        .users
        .iter()
        .map(|u| ("user", Some(u.to_i64())))
        .chain(m.roles.iter().map(|r| ("role", Some(r.to_i64()))))
        .chain(m.everyone.then_some(("everyone", None)))
        .chain(m.here.then_some(("here", None)));
    for (kind, target_id) in targets {
        sqlx::query!(
            "INSERT INTO message_mentions (message_id, kind, target_id) VALUES (?, ?, ?)",
            id,
            kind,
            target_id
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Attaches an upload of `uploader_id` that is not attached yet; `false` otherwise.
pub async fn link_attachment(
    conn: &mut SqliteConnection,
    message_id: Id,
    attachment_id: Id,
    uploader_id: Id,
) -> Result<bool, sqlx::Error> {
    let (message_id, id, uploader_id) = (
        message_id.to_i64(),
        attachment_id.to_i64(),
        uploader_id.to_i64(),
    );
    let done = sqlx::query!(
        "UPDATE attachments SET message_id = ?
         WHERE id = ? AND uploader_id = ? AND message_id IS NULL",
        message_id,
        id,
        uploader_id
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn count_attachments(
    e: impl SqliteExecutor<'_>,
    message_id: Id,
) -> Result<i64, sqlx::Error> {
    let id = message_id.to_i64();
    sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM attachments WHERE message_id = ?"#,
        id
    )
    .fetch_one(e)
    .await
}

pub async fn set_last_message_id(
    conn: &mut SqliteConnection,
    channel_id: Id,
    message_id: Id,
) -> Result<(), sqlx::Error> {
    let (channel_id, message_id) = (channel_id.to_i64(), message_id.to_i64());
    // Concurrent sends may commit out of order; never move backwards.
    sqlx::query!(
        "UPDATE channels SET last_message_id = MAX(COALESCE(last_message_id, 0), ?) WHERE id = ?",
        message_id,
        channel_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn edit_content(
    conn: &mut SqliteConnection,
    message_id: Id,
    content: &str,
    edited_at: i64,
) -> Result<(), sqlx::Error> {
    let id = message_id.to_i64();
    sqlx::query!(
        "UPDATE messages SET content = ?, edited_at = ? WHERE id = ?",
        content,
        edited_at,
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Soft delete: content cleared, unpinned, then [`clear_deleted_extras`].
pub async fn soft_delete(conn: &mut SqliteConnection, message_id: Id) -> Result<(), sqlx::Error> {
    let id = message_id.to_i64();
    sqlx::query!(
        "UPDATE messages SET content = '', deleted = 1, pinned = 0 WHERE id = ?",
        id
    )
    .execute(&mut *conn)
    .await?;
    clear_deleted_extras(conn, message_id).await
}

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

pub async fn set_pinned(
    e: impl SqliteExecutor<'_>,
    message_id: Id,
    pinned: bool,
) -> Result<(), sqlx::Error> {
    let id = message_id.to_i64();
    sqlx::query!("UPDATE messages SET pinned = ? WHERE id = ?", pinned, id)
        .execute(e)
        .await?;
    Ok(())
}

/// The distinct emoji on a message.
pub async fn reaction_emoji(
    e: impl SqliteExecutor<'_>,
    message_id: Id,
) -> Result<Vec<String>, sqlx::Error> {
    let id = message_id.to_i64();
    sqlx::query_scalar!(
        "SELECT DISTINCT emoji FROM reactions WHERE message_id = ?",
        id
    )
    .fetch_all(e)
    .await
}

/// `false` if the user had already reacted with that emoji.
pub async fn add_reaction(
    e: impl SqliteExecutor<'_>,
    message_id: Id,
    emoji: &str,
    user_id: Id,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let (id, user_id) = (message_id.to_i64(), user_id.to_i64());
    let done = sqlx::query!(
        "INSERT INTO reactions (message_id, emoji, user_id, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
        id,
        emoji,
        user_id,
        now
    )
    .execute(e)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// `false` if there was no such reaction.
pub async fn remove_reaction(
    e: impl SqliteExecutor<'_>,
    message_id: Id,
    emoji: &str,
    user_id: Id,
) -> Result<bool, sqlx::Error> {
    let (id, user_id) = (message_id.to_i64(), user_id.to_i64());
    let done = sqlx::query!(
        "DELETE FROM reactions WHERE message_id = ? AND emoji = ? AND user_id = ?",
        id,
        emoji,
        user_id
    )
    .execute(e)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Adds one mention to each user's read state of the channel, unless they already read past
/// `message_id`.
pub async fn add_mention_counts(
    conn: &mut SqliteConnection,
    channel_id: Id,
    user_ids: &[Id],
    message_id: Id,
) -> Result<(), sqlx::Error> {
    let (channel_id, message_id) = (channel_id.to_i64(), message_id.to_i64());
    for user_id in user_ids {
        let user_id = user_id.to_i64();
        sqlx::query!(
            "INSERT INTO read_states (user_id, channel_id, last_read_id, mention_count)
             VALUES (?, ?, 0, 1)
             ON CONFLICT (user_id, channel_id) DO UPDATE SET mention_count = mention_count + 1
             WHERE last_read_id < ?",
            user_id,
            channel_id,
            message_id
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Marks a channel read up to `message_id` and resets the mention count.
pub async fn set_read(
    e: impl SqliteExecutor<'_>,
    user_id: Id,
    channel_id: Id,
    message_id: Id,
) -> Result<(), sqlx::Error> {
    let (user_id, channel_id, message_id) =
        (user_id.to_i64(), channel_id.to_i64(), message_id.to_i64());
    sqlx::query!(
        "INSERT INTO read_states (user_id, channel_id, last_read_id, mention_count)
         VALUES (?, ?, ?, 0)
         ON CONFLICT (user_id, channel_id)
         DO UPDATE SET last_read_id = excluded.last_read_id, mention_count = 0",
        user_id,
        channel_id,
        message_id
    )
    .execute(e)
    .await?;
    Ok(())
}
