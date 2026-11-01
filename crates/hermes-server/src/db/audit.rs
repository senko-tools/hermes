//! The audit log (PLAN.md sections 7.3 and 13).

use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, AuditEntry};
use sqlx::{SqliteConnection, SqliteExecutor};

pub async fn insert(
    conn: &mut SqliteConnection,
    space_id: Id,
    entry: &AuditEntry,
) -> Result<(), sqlx::Error> {
    let (id, space_id, actor_id) = (
        entry.id.to_i64(),
        space_id.to_i64(),
        entry.actor_id.to_i64(),
    );
    let action = entry.action.as_str();
    let target_id = entry.target_id.map(Id::to_i64);
    let details = entry.details.to_string();
    sqlx::query!(
        "INSERT INTO audit_log (id, space_id, actor_id, action, target_id, details, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        id,
        space_id,
        actor_id,
        action,
        target_id,
        details,
        entry.created_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Entries of a space, newest first, older than `before` if given.
pub async fn list(
    e: impl SqliteExecutor<'_>,
    space_id: Id,
    before: Option<Id>,
    limit: u32,
) -> Result<Vec<AuditEntry>, sqlx::Error> {
    let space_id = space_id.to_i64();
    let before = before.map_or(i64::MAX, Id::to_i64);
    let limit = i64::from(limit);
    sqlx::query!(
        "SELECT id, actor_id, action, target_id, details, created_at FROM audit_log
         WHERE space_id = ? AND id < ? ORDER BY id DESC LIMIT ?",
        space_id,
        before,
        limit
    )
    .fetch_all(e)
    .await?
    .into_iter()
    .map(|r| {
        let action =
            AuditAction::from_name(&r.action).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "audit_log.action".into(),
                source: format!("unknown audit action {:?}", r.action).into(),
            })?;
        let details =
            serde_json::from_str(&r.details).map_err(|err| sqlx::Error::ColumnDecode {
                index: "audit_log.details".into(),
                source: err.into(),
            })?;
        Ok(AuditEntry {
            id: Id::from_i64(r.id),
            actor_id: Id::from_i64(r.actor_id),
            action,
            target_id: r.target_id.map(Id::from_i64),
            details,
            created_at: r.created_at,
        })
    })
    .collect()
}
