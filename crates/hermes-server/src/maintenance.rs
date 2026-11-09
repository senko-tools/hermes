//! What the server tidies up by itself, once an hour.

use std::sync::Arc;
use std::time::Duration;

use hermes_proto::id::Id;

use crate::state::AppState;
use crate::time::now_ms;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A nonce only has to outlive a client's retries.
const NONCE_KEEP_MS: i64 = DAY_MS;
/// An invite that has run out is kept this long, so its link can still say so.
const DEAD_INVITE_KEEP_MS: i64 = 7 * DAY_MS;

/// Sessions to end: unused for `limits.session_idle_days`, or beyond a person's
/// `limits.max_sessions_per_user`, least recently used first.
async fn expired_sessions(state: &AppState, now: i64) -> Result<Vec<Id>, sqlx::Error> {
    let limits = &state.config.limits;
    let idle_days = i64::from(limits.session_idle_days);
    let cutoff = if idle_days == 0 {
        i64::MIN
    } else {
        now - idle_days * DAY_MS
    };
    let max = if limits.max_sessions_per_user == 0 {
        i64::MAX
    } else {
        i64::from(limits.max_sessions_per_user)
    };
    let rows = sqlx::query!(
        r#"SELECT id AS "id!" FROM (
             SELECT id, last_seen_at,
                    ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY last_seen_at DESC) AS place
             FROM sessions
           ) WHERE last_seen_at < ? OR place > ?"#,
        cutoff,
        max
    )
    .fetch_all(&state.db.read)
    .await?;
    Ok(rows.into_iter().map(|r| Id::from_i64(r.id)).collect())
}

/// One round of tidying.
pub async fn run(state: &AppState, now: i64) -> Result<(), sqlx::Error> {
    let nonces = now - NONCE_KEEP_MS;
    sqlx::query!("DELETE FROM message_nonces WHERE created_at < ?", nonces)
        .execute(&state.db.write)
        .await?;
    let invites = now - DEAD_INVITE_KEEP_MS;
    sqlx::query!(
        "DELETE FROM invites WHERE expires_at IS NOT NULL AND expires_at < ?",
        invites
    )
    .execute(&state.db.write)
    .await?;
    // What is left of a person in a server they are no longer in.
    sqlx::query!(
        "DELETE FROM read_states WHERE channel_id IN (
           SELECT c.id FROM channels c
           WHERE c.space_id IS NOT NULL AND NOT EXISTS (
             SELECT 1 FROM space_members m
             WHERE m.space_id = c.space_id AND m.user_id = read_states.user_id
           )
         )"
    )
    .execute(&state.db.write)
    .await?;
    sqlx::query!(
        "DELETE FROM notification_settings WHERE
           target_id NOT IN (SELECT id FROM channels)
           AND target_id NOT IN (SELECT id FROM spaces)"
    )
    .execute(&state.db.write)
    .await?;

    let sessions = expired_sessions(state, now).await?;
    if !sessions.is_empty() {
        tracing::info!("ending {} unused sessions", sessions.len());
        crate::api::devices::revoke_sessions(state, &sessions).await?;
        for session in &sessions {
            let id = session.to_i64();
            sqlx::query!("DELETE FROM sessions WHERE id = ?", id)
                .execute(&state.db.write)
                .await?;
        }
        state.gateway.revoke(&sessions);
    }
    sqlx::query("PRAGMA optimize")
        .execute(&state.db.write)
        .await?;
    Ok(())
}

pub async fn task(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
    loop {
        interval.tick().await;
        if let Err(err) = run(&state, now_ms()).await {
            tracing::error!("maintenance failed: {err}");
        }
    }
}
