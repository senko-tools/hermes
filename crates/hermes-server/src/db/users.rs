//! Profile, notification settings and admin flags of users.

use hermes_proto::id::Id;
use hermes_proto::models::{DmFrom, NotificationLevel, NotificationSetting, PrivacySettings, User};
use sqlx::SqliteConnection;

pub async fn update_profile(conn: &mut SqliteConnection, user: &User) -> Result<(), sqlx::Error> {
    let id = user.id.to_i64();
    let avatar_id = user.avatar.as_ref().map(|a| a.id.to_i64());
    sqlx::query!(
        "UPDATE users SET display_name = ?, avatar_id = ?, status_text = ? WHERE id = ?",
        user.display_name,
        avatar_id,
        user.status_text,
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_flags(
    conn: &mut SqliteConnection,
    user_id: Id,
    disabled: bool,
    instance_admin: bool,
) -> Result<(), sqlx::Error> {
    let id = user_id.to_i64();
    sqlx::query!(
        "UPDATE users SET disabled = ?, instance_admin = ? WHERE id = ?",
        disabled,
        instance_admin,
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_privacy(
    conn: &mut SqliteConnection,
    user_id: Id,
    privacy: &PrivacySettings,
) -> Result<(), sqlx::Error> {
    let id = user_id.to_i64();
    let dm_from = dm_from_name(privacy.dm_from);
    sqlx::query!(
        "UPDATE users SET dm_from = ?, message_requests = ? WHERE id = ?",
        dm_from,
        privacy.message_requests,
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub const fn dm_from_name(dm_from: DmFrom) -> &'static str {
    match dm_from {
        DmFrom::Everyone => "everyone",
        DmFrom::Spaces => "spaces",
        DmFrom::Nobody => "nobody",
    }
}

/// The stored `users.dm_from`; anything unknown counts as the default.
pub fn parse_dm_from(name: &str) -> DmFrom {
    match name {
        "spaces" => DmFrom::Spaces,
        "nobody" => DmFrom::Nobody,
        _ => DmFrom::Everyone,
    }
}

pub const fn level_name(level: NotificationLevel) -> &'static str {
    match level {
        NotificationLevel::All => "all",
        NotificationLevel::Mentions => "mentions",
        NotificationLevel::None => "none",
    }
}

pub async fn set_notification_setting(
    conn: &mut SqliteConnection,
    user_id: Id,
    setting: &NotificationSetting,
) -> Result<(), sqlx::Error> {
    let (user_id, target_id) = (user_id.to_i64(), setting.target_id.to_i64());
    let level = level_name(setting.level);
    sqlx::query!(
        "INSERT INTO notification_settings (user_id, target_id, level, muted_until)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id, target_id)
         DO UPDATE SET level = excluded.level, muted_until = excluded.muted_until",
        user_id,
        target_id,
        level,
        setting.muted_until
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_notification_setting(
    conn: &mut SqliteConnection,
    user_id: Id,
    target_id: Id,
) -> Result<(), sqlx::Error> {
    let (user_id, target_id) = (user_id.to_i64(), target_id.to_i64());
    sqlx::query!(
        "DELETE FROM notification_settings WHERE user_id = ? AND target_id = ?",
        user_id,
        target_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// `(id, disabled, instance_admin)` of every user, for the admin refresh and the CLI.
pub async fn flags(e: impl sqlx::SqliteExecutor<'_>) -> Result<Vec<(Id, bool, bool)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, disabled AS "disabled: bool", instance_admin AS "instance_admin: bool"
           FROM users ORDER BY id"#
    )
    .fetch_all(e)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (Id::from_i64(r.id), r.disabled, r.instance_admin))
        .collect())
}

/// `(id, username, disabled, instance_admin)` of the user called `username` (any case).
pub async fn by_username(
    e: impl sqlx::SqliteExecutor<'_>,
    username: &str,
) -> Result<Option<(Id, String, bool, bool)>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, username, disabled AS "disabled: bool",
                  instance_admin AS "instance_admin: bool"
           FROM users WHERE username = ?"#,
        username
    )
    .fetch_optional(e)
    .await?;
    Ok(row.map(|r| (Id::from_i64(r.id), r.username, r.disabled, r.instance_admin)))
}

/// `(id, username, display_name, disabled, instance_admin, created_at)` of every user.
pub async fn list(
    e: impl sqlx::SqliteExecutor<'_>,
) -> Result<Vec<(Id, String, Option<String>, bool, bool, i64)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, username, display_name, disabled AS "disabled: bool",
                  instance_admin AS "instance_admin: bool", created_at
           FROM users ORDER BY id"#
    )
    .fetch_all(e)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                Id::from_i64(r.id),
                r.username,
                r.display_name,
                r.disabled,
                r.instance_admin,
                r.created_at,
            )
        })
        .collect())
}

/// Every live session ID.
pub async fn session_ids(e: impl sqlx::SqliteExecutor<'_>) -> Result<Vec<Id>, sqlx::Error> {
    let ids = sqlx::query_scalar!("SELECT id FROM sessions")
        .fetch_all(e)
        .await?;
    Ok(ids.into_iter().map(Id::from_i64).collect())
}
