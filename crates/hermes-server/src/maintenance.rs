//! What the server tidies up by itself, once an hour.

use std::sync::Arc;
use std::time::Duration;

use hermes_proto::id::Id;

use crate::state::AppState;
use crate::time::now_ms;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const RETENTION_BATCH: i64 = 2000;
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

/// Deletes what `[privacy]` says is not to be kept, a batch at a time so that writers go on.
async fn retention(state: &AppState, now: i64) -> Result<(), sqlx::Error> {
    let privacy = &state.config.privacy;
    let id_at = |days: u32| {
        hermes_proto::id::Id::min_at_unix_ms(
            u64::try_from(now - i64::from(days) * DAY_MS).unwrap_or(0),
        )
        .to_i64()
    };
    let batched = async |sql: &'static str, cutoff: i64| -> Result<(), sqlx::Error> {
        loop {
            let done = sqlx::query(sql)
                .bind(cutoff)
                .bind(RETENTION_BATCH)
                .execute(&state.db.write)
                .await?;
            if done.rows_affected() < RETENTION_BATCH as u64 {
                return Ok(());
            }
        }
    };
    if privacy.message_retention_days > 0 {
        batched(
            "DELETE FROM messages WHERE id IN (SELECT id FROM messages WHERE id < ? LIMIT ?)",
            id_at(privacy.message_retention_days),
        )
        .await?;
    }
    if privacy.deleted_message_retention_days > 0 {
        batched(
            "DELETE FROM messages WHERE id IN
               (SELECT id FROM messages WHERE deleted = 1 AND id < ? LIMIT ?)",
            id_at(privacy.deleted_message_retention_days),
        )
        .await?;
    }
    if privacy.audit_retention_days > 0 {
        batched(
            "DELETE FROM audit_log WHERE id IN (SELECT id FROM audit_log WHERE id < ? LIMIT ?)",
            id_at(privacy.audit_retention_days),
        )
        .await?;
    }
    Ok(())
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

    sqlx::query!(
        "DELETE FROM bans WHERE expires_at IS NOT NULL AND expires_at < ?",
        now
    )
    .execute(&state.db.write)
    .await?;
    retention(state, now).await?;
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
        state.revoke_sessions(&sessions);
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
