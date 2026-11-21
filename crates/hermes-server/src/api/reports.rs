//! Reporting a message to whoever moderates it. The server cannot read encrypted messages, so a
//! report holds what the reporter chose to show.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::api::ReportRequest;
use hermes_proto::id::Id;
use hermes_proto::models::Report;
use hermes_proto::permissions::Permissions;
use sqlx::Row as _;

use super::{ApiResult, Error, Json, Path, perms, validate};
use crate::auth::Auth;
use crate::db;
use crate::ratelimit::Action;
use crate::state::AppState;
use crate::time::now_ms;

const REPORTS_SHOWN: i64 = 200;

pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((channel_id, message_id)): Path<(Id, Id)>,
    Json(req): Json<ReportRequest>,
) -> ApiResult<StatusCode> {
    state.throttle(auth.user_id, Action::Friend)?;
    let reason = validate::name(&req.reason, (1, ReportRequest::MAX_REASON_CHARS))
        .map_err(|problem| validate::field_error("reason", problem))?;
    if req.excerpt.chars().count() > ReportRequest::MAX_EXCERPT_CHARS {
        return Err(validate::field_error("excerpt", "is too long"));
    }
    let space = {
        let cache = state.cache();
        perms::channel(&cache, channel_id, auth.user_id)?
            .channel
            .space_id
    };
    let message = db::messages::get(&state.db.read, channel_id, message_id)
        .await?
        .ok_or_else(|| Error::not_found("message"))?;
    sqlx::query(
        "INSERT INTO reports
           (id, space_id, channel_id, message_id, reporter_id, author_id, reason, excerpt, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(state.ids.next().to_i64())
    .bind(space.map(Id::to_i64))
    .bind(channel_id.to_i64())
    .bind(message_id.to_i64())
    .bind(auth.user_id.to_i64())
    .bind(message.author_id.to_i64())
    .bind(reason)
    .bind(req.excerpt)
    .bind(now_ms())
    .execute(&state.db.write)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn rows_to_reports(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<Report>, sqlx::Error> {
    rows.into_iter()
        .map(|r| {
            Ok(Report {
                id: Id::from_i64(r.try_get("id")?),
                channel_id: Id::from_i64(r.try_get("channel_id")?),
                message_id: Id::from_i64(r.try_get("message_id")?),
                reporter_id: Id::from_i64(r.try_get("reporter_id")?),
                author_id: Id::from_i64(r.try_get("author_id")?),
                reason: r.try_get("reason")?,
                excerpt: r.try_get("excerpt")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

/// `MANAGE_MESSAGES`. Newest first.
pub async fn list(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
) -> ApiResult<Json<Vec<Report>>> {
    perms::require_space(
        &state.cache(),
        space_id,
        auth.user_id,
        Permissions::MANAGE_MESSAGES,
    )?;
    let rows = sqlx::query("SELECT * FROM reports WHERE space_id = ? ORDER BY id DESC LIMIT ?")
        .bind(space_id.to_i64())
        .bind(REPORTS_SHOWN)
        .fetch_all(&state.db.read)
        .await?;
    Ok(Json(rows_to_reports(rows).map_err(db_error)?))
}

fn db_error(err: sqlx::Error) -> Error {
    Error::internal(err)
}

/// `MANAGE_MESSAGES`: dealt with.
pub async fn resolve(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, report)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    perms::require_space(
        &state.cache(),
        space_id,
        auth.user_id,
        Permissions::MANAGE_MESSAGES,
    )?;
    let done = sqlx::query("DELETE FROM reports WHERE id = ? AND space_id = ?")
        .bind(report.to_i64())
        .bind(space_id.to_i64())
        .execute(&state.db.write)
        .await?;
    if done.rows_affected() == 0 {
        return Err(Error::not_found("report"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Reports from DMs, which no server moderates: for instance admins.
pub(super) async fn list_unowned(state: &AppState) -> ApiResult<Vec<Report>> {
    let rows = sqlx::query("SELECT * FROM reports WHERE space_id IS NULL ORDER BY id DESC LIMIT ?")
        .bind(REPORTS_SHOWN)
        .fetch_all(&state.db.read)
        .await?;
    rows_to_reports(rows).map_err(db_error)
}

/// Deletes a DM report.
pub(super) async fn resolve_unowned(state: &AppState, report: Id) -> ApiResult<()> {
    db::messages::delete_report_unowned(&state.db.write, report).await?;
    Ok(())
}
