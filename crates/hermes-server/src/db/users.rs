//! Profile, notification settings and admin flags of users.

use hermes_proto::id::Id;
use hermes_proto::models::{NotificationLevel, NotificationSetting, User};
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
