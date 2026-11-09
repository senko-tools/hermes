//! `GET /spaces/{id}/audit-log`, and recording entries for moderation actions.

use std::sync::Arc;

use axum::extract::State;
use hermes_proto::api::Pagination;
use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, AuditEntry};
use hermes_proto::permissions::Permissions;
use sqlx::SqliteConnection;

use super::{ApiResult, Json, Path, Query, perms};
use crate::auth::Auth;
use crate::db;
use crate::state::AppState;
use crate::time::now_ms;

/// Writes an audit log entry inside the transaction of the action it records.
pub async fn record(
    conn: &mut SqliteConnection,
    state: &AppState,
    space_id: Id,
    actor_id: Id,
    action: AuditAction,
    target_id: Option<Id>,
    details: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let entry = AuditEntry {
        id: state.ids.next(),
        actor_id,
        action,
        target_id,
        details,
        created_at: now_ms(),
    };
    db::audit::insert(conn, space_id, &entry).await
}

/// `?before&limit&action&actor`.
#[derive(Debug, Default, serde::Deserialize)]
pub struct AuditQuery {
    before: Option<Id>,
    limit: Option<u32>,
    /// One kind of entry, by its name.
    action: Option<String>,
    /// What one person did.
    actor: Option<Id>,
}

/// Newest first; `?before&limit&action&actor`.
pub async fn list(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Query(query): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    perms::require_space(
        &state.cache(),
        space_id,
        auth.user_id,
        Permissions::VIEW_AUDIT_LOG,
    )?;
    let page = Pagination {
        before: query.before,
        limit: query.limit,
        ..Pagination::default()
    };
    let entries = db::audit::list(
        &state.db.read,
        space_id,
        page.before,
        page.limit(),
        query.action.as_deref(),
        query.actor,
    )
    .await?;
    Ok(Json(entries))
}
