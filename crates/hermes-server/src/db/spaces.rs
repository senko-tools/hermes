//! Spaces, roles, members and channels (PLAN.md section 7.3).

use hermes_proto::id::Id;
use hermes_proto::models::{AttachmentRef, Channel, ChannelKind, Role, Space};
use sqlx::{SqliteConnection, SqliteExecutor};

pub async fn insert_space(
    conn: &mut SqliteConnection,
    space: &Space,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    let (id, owner_id) = (space.id.to_i64(), space.owner_id.to_i64());
    let icon_id = space.icon.as_ref().map(|i| i.id.to_i64());
    sqlx::query!(
        "INSERT INTO spaces (id, name, description, icon_id, owner_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        id,
        space.name,
        space.description,
        icon_id,
        owner_id,
        created_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn insert_role(conn: &mut SqliteConnection, role: &Role) -> Result<(), sqlx::Error> {
    let (id, space_id) = (role.id.to_i64(), role.space_id.to_i64());
    let color = role.color.map(i64::from);
    // Stored as i64; the cache reads it back with `from_bits_retain`.
    let permissions = role.permissions.bits() as i64;
    sqlx::query!(
        "INSERT INTO roles (id, space_id, name, color, hoist, mentionable, position, permissions)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        id,
        space_id,
        role.name,
        color,
        role.hoist,
        role.mentionable,
        role.position,
        permissions
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn insert_member(
    conn: &mut SqliteConnection,
    space_id: Id,
    user_id: Id,
    joined_at: i64,
) -> Result<(), sqlx::Error> {
    let (space_id, user_id) = (space_id.to_i64(), user_id.to_i64());
    sqlx::query!(
        "INSERT INTO space_members (space_id, user_id, joined_at) VALUES (?, ?, ?)",
        space_id,
        user_id,
        joined_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Inserts a space channel. Overrides and DM recipients are not written.
pub async fn insert_channel(
    conn: &mut SqliteConnection,
    channel: &Channel,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    let id = channel.id.to_i64();
    let space_id = channel.space_id.map(Id::to_i64);
    let parent_id = channel.parent_id.map(Id::to_i64);
    let owner_id = channel.owner_id.map(Id::to_i64);
    let kind = channel_kind_name(channel.kind);
    let bitrate = channel.bitrate.map(i64::from);
    let user_limit = channel.user_limit.map(i64::from);
    let slowmode = i64::from(channel.slowmode_secs);
    sqlx::query!(
        "INSERT INTO channels (id, space_id, kind, parent_id, name, topic, position, bitrate,
                               user_limit, slowmode_secs, owner_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        id,
        space_id,
        kind,
        parent_id,
        channel.name,
        channel.topic,
        channel.position,
        bitrate,
        user_limit,
        slowmode,
        owner_id,
        created_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// The `channels.kind` value; the inverse of `state::channel_kind`.
fn channel_kind_name(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Text => "text",
        ChannelKind::Voice => "voice",
        ChannelKind::Category => "category",
        ChannelKind::Dm => "dm",
        ChannelKind::GroupDm => "group_dm",
    }
}

/// Writes name, description and icon.
pub async fn update_space(conn: &mut SqliteConnection, space: &Space) -> Result<(), sqlx::Error> {
    let id = space.id.to_i64();
    let icon_id = space.icon.as_ref().map(|i| i.id.to_i64());
    sqlx::query!(
        "UPDATE spaces SET name = ?, description = ?, icon_id = ? WHERE id = ?",
        space.name,
        space.description,
        icon_id,
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_owner(
    conn: &mut SqliteConnection,
    space_id: Id,
    owner_id: Id,
) -> Result<(), sqlx::Error> {
    let (space_id, owner_id) = (space_id.to_i64(), owner_id.to_i64());
    sqlx::query!(
        "UPDATE spaces SET owner_id = ? WHERE id = ?",
        owner_id,
        space_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Deletes a space with everything in it (`ON DELETE CASCADE`).
pub async fn delete_space(conn: &mut SqliteConnection, space_id: Id) -> Result<(), sqlx::Error> {
    let id = space_id.to_i64();
    sqlx::query!("DELETE FROM spaces WHERE id = ?", id)
        .execute(conn)
        .await?;
    Ok(())
}

/// An image uploaded by `uploader_id`, usable as an icon.
pub async fn own_image(
    e: impl SqliteExecutor<'_>,
    attachment_id: Id,
    uploader_id: Id,
) -> Result<Option<AttachmentRef>, sqlx::Error> {
    let (id, uploader_id) = (attachment_id.to_i64(), uploader_id.to_i64());
    let filename = sqlx::query_scalar!(
        "SELECT filename FROM attachments
         WHERE id = ? AND uploader_id = ? AND content_type LIKE 'image/%'",
        id,
        uploader_id
    )
    .fetch_optional(e)
    .await?;
    Ok(filename.map(|filename| AttachmentRef {
        id: attachment_id,
        filename,
    }))
}

pub async fn is_banned(
    e: impl SqliteExecutor<'_>,
    space_id: Id,
    user_id: Id,
) -> Result<bool, sqlx::Error> {
    let (space_id, user_id) = (space_id.to_i64(), user_id.to_i64());
    let found = sqlx::query_scalar!(
        "SELECT user_id FROM bans WHERE space_id = ? AND user_id = ?",
        space_id,
        user_id
    )
    .fetch_optional(e)
    .await?;
    Ok(found.is_some())
}
