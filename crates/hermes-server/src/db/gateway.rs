//! Per-user rows that only the gateway `ready` payload needs (PLAN.md section 11.3).

use hermes_proto::id::Id;
use hermes_proto::models::{NotificationLevel, NotificationSetting, ReadState};
use sqlx::SqliteExecutor;

pub async fn read_states(
    e: impl SqliteExecutor<'_>,
    user_id: Id,
) -> Result<Vec<ReadState>, sqlx::Error> {
    let user_id = user_id.to_i64();
    let rows = sqlx::query!(
        "SELECT channel_id, last_read_id, mention_count FROM read_states WHERE user_id = ?",
        user_id
    )
    .fetch_all(e)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ReadState {
            channel_id: Id::from_i64(r.channel_id),
            last_read_id: Id::from_i64(r.last_read_id),
            mention_count: u32::try_from(r.mention_count).unwrap_or(0),
        })
        .collect())
}

pub async fn notification_settings(
    e: impl SqliteExecutor<'_>,
    user_id: Id,
) -> Result<Vec<NotificationSetting>, sqlx::Error> {
    let user_id = user_id.to_i64();
    let rows = sqlx::query!(
        "SELECT target_id, level, muted_until FROM notification_settings WHERE user_id = ?",
        user_id
    )
    .fetch_all(e)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let level = match r.level.as_str() {
                "all" => NotificationLevel::All,
                "mentions" => NotificationLevel::Mentions,
                "none" => NotificationLevel::None,
                _ => return None,
            };
            Some(NotificationSetting {
                target_id: Id::from_i64(r.target_id),
                level,
                muted_until: r.muted_until,
            })
        })
        .collect())
}
