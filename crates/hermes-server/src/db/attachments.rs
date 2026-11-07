//! Uploaded files (`attachments`); the bytes live under `<data_dir>/files/` (see `crate::files`).

use hermes_proto::id::Id;
use hermes_proto::models::Attachment;
use sqlx::SqliteExecutor;

use super::int;

/// An `attachments` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRow {
    pub id: Id,
    pub uploader_id: Id,
    pub message_id: Option<Id>,
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub has_thumb: bool,
    pub sha256: [u8; 32],
    pub created_at: i64,
}

impl AttachmentRow {
    pub fn to_wire(&self) -> Attachment {
        Attachment {
            id: self.id,
            filename: self.filename.clone(),
            content_type: self.content_type.clone(),
            size: self.size,
            width: self.width,
            height: self.height,
            has_thumb: self.has_thumb,
            sealed: None,
        }
    }

    /// Only decoded raster images get dimensions; everything else is served as a download.
    pub fn is_image(&self) -> bool {
        self.width.is_some()
    }
}

pub async fn insert(e: impl SqliteExecutor<'_>, row: &AttachmentRow) -> Result<(), sqlx::Error> {
    let (id, uploader_id) = (row.id.to_i64(), row.uploader_id.to_i64());
    let message_id = row.message_id.map(Id::to_i64);
    let size = i64::try_from(row.size).unwrap_or(i64::MAX);
    let (width, height) = (row.width.map(i64::from), row.height.map(i64::from));
    let sha256 = row.sha256.as_slice();
    sqlx::query!(
        "INSERT INTO attachments (id, uploader_id, message_id, filename, content_type, size,
                                  width, height, has_thumb, sha256, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        id,
        uploader_id,
        message_id,
        row.filename,
        row.content_type,
        size,
        width,
        height,
        row.has_thumb,
        sha256,
        row.created_at
    )
    .execute(e)
    .await?;
    Ok(())
}

pub async fn get(e: impl SqliteExecutor<'_>, id: Id) -> Result<Option<AttachmentRow>, sqlx::Error> {
    let id = id.to_i64();
    let Some(r) = sqlx::query!(
        r#"SELECT id, uploader_id, message_id, filename, content_type, size, width, height,
                  has_thumb AS "has_thumb: bool", sha256, created_at
           FROM attachments WHERE id = ?"#,
        id
    )
    .fetch_optional(e)
    .await?
    else {
        return Ok(None);
    };
    let sha256 = r.sha256.as_slice().try_into().map_err(|_| {
        sqlx::Error::Decode(format!("attachments.sha256 of {} is not 32 bytes", r.id).into())
    })?;
    Ok(Some(AttachmentRow {
        id: Id::from_i64(r.id),
        uploader_id: Id::from_i64(r.uploader_id),
        message_id: r.message_id.map(Id::from_i64),
        filename: r.filename,
        content_type: r.content_type,
        size: int(r.size, "attachments.size")?,
        width: r.width.map(|w| int(w, "attachments.width")).transpose()?,
        height: r.height.map(|h| int(h, "attachments.height")).transpose()?,
        has_thumb: r.has_thumb,
        sha256,
        created_at: r.created_at,
    }))
}

/// The channel of a message, `None` if it does not exist or is deleted.
pub async fn message_channel(
    e: impl SqliteExecutor<'_>,
    message_id: Id,
) -> Result<Option<Id>, sqlx::Error> {
    let id = message_id.to_i64();
    let channel = sqlx::query_scalar!(
        "SELECT channel_id FROM messages WHERE id = ? AND deleted = 0",
        id
    )
    .fetch_optional(e)
    .await?;
    Ok(channel.map(Id::from_i64))
}

/// Whether the upload is some user's avatar or some space's icon (visible to every user).
pub async fn is_avatar_or_icon(e: impl SqliteExecutor<'_>, id: Id) -> Result<bool, sqlx::Error> {
    let id = id.to_i64();
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM users WHERE avatar_id = ?1)
               OR EXISTS (SELECT 1 FROM spaces WHERE icon_id = ?1) AS "used!: bool""#,
        id
    )
    .fetch_one(e)
    .await
}

/// Bytes used by all uploads (originals only; thumbnails are not counted).
pub async fn total_size(e: impl SqliteExecutor<'_>) -> Result<u64, sqlx::Error> {
    let total =
        sqlx::query_scalar!(r#"SELECT COALESCE(SUM(size), 0) AS "total!: i64" FROM attachments"#)
            .fetch_one(e)
            .await?;
    int(total, "SUM(attachments.size)")
}

/// An upload to delete: `(id, sha256, size)`.
pub type Expired = (Id, [u8; 32], u64);

/// Uploads created before `cutoff` that are attached to no message and are no avatar or icon.
pub async fn expired_unattached(
    e: impl SqliteExecutor<'_>,
    cutoff: i64,
) -> Result<Vec<Expired>, sqlx::Error> {
    sqlx::query!(
        "SELECT id, sha256, size FROM attachments a
         WHERE message_id IS NULL AND created_at < ?
           AND NOT EXISTS (SELECT 1 FROM users WHERE avatar_id = a.id)
           AND NOT EXISTS (SELECT 1 FROM spaces WHERE icon_id = a.id)",
        cutoff
    )
    .fetch_all(e)
    .await?
    .into_iter()
    .filter_map(|r| {
        let sha: [u8; 32] = r.sha256.as_slice().try_into().ok()?;
        Some(int(r.size, "attachments.size").map(|size| (Id::from_i64(r.id), sha, size)))
    })
    .collect()
}

/// Deletes the row unless it was attached (or made an avatar/icon) in the meantime.
pub async fn delete_if_unused(e: impl SqliteExecutor<'_>, id: Id) -> Result<bool, sqlx::Error> {
    let id = id.to_i64();
    let done = sqlx::query!(
        "DELETE FROM attachments
         WHERE id = ?1 AND message_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM users WHERE avatar_id = ?1)
           AND NOT EXISTS (SELECT 1 FROM spaces WHERE icon_id = ?1)",
        id
    )
    .execute(e)
    .await?;
    Ok(done.rows_affected() > 0)
}
