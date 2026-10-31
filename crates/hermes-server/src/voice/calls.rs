//! DM call system messages (PLAN.md 12.6), posted by a background task because the registry
//! cannot wait for the database.

use std::sync::Arc;

use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::MessageKind;
use tokio::sync::mpsc;

use crate::db;
use crate::db::messages::MessageRow;
use crate::gateway::bus::Audience;
use crate::messages::{broadcast_copy, to_wire};
use crate::state::AppState;
use crate::time::now_ms;

/// Work the registry hands to [`task`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After {
    /// A DM room went from empty to one participant.
    CallStarted { channel_id: Id, user_id: Id },
    /// A DM room emptied; `user_id` started the call at `started_at`.
    CallEnded {
        channel_id: Id,
        user_id: Id,
        started_at: i64,
    },
}

/// Runs until the sender ([`super::Voice`]) is dropped.
pub async fn task(state: Arc<AppState>, mut rx: mpsc::UnboundedReceiver<After>) {
    while let Some(after) = rx.recv().await {
        let (channel_id, author_id, content) = match after {
            After::CallStarted {
                channel_id,
                user_id,
            } => (
                channel_id,
                user_id,
                format!("{} started a call", display_name(&state, user_id)),
            ),
            After::CallEnded {
                channel_id,
                user_id,
                started_at,
            } => (channel_id, user_id, call_ended(now_ms() - started_at)),
        };
        if let Err(err) = post_system_message(&state, channel_id, author_id, content).await {
            tracing::error!("could not post a call message: {err}");
        }
    }
}

/// `"Call ended · 12 min"`; at least 1 minute.
pub fn call_ended(duration_ms: i64) -> String {
    let minutes = (duration_ms / 60_000).max(1);
    format!("Call ended · {minutes} min")
}

fn display_name(state: &AppState, user_id: Id) -> String {
    state.cache().users.get(&user_id).map_or_else(
        || "Someone".into(),
        |u| {
            u.user
                .display_name
                .clone()
                .unwrap_or_else(|| u.user.username.clone())
        },
    )
}

/// Inserts a `system` message and publishes `message_create`.
async fn post_system_message(
    state: &AppState,
    channel_id: Id,
    author_id: Id,
    content: String,
) -> Result<(), sqlx::Error> {
    if !state.cache().channels.contains_key(&channel_id) {
        return Ok(());
    }
    let row = MessageRow {
        id: state.ids.next(),
        channel_id,
        author_id,
        content,
        reply_to_id: None,
        edited_at: None,
        deleted: false,
        pinned: false,
        kind: MessageKind::System,
    };
    let mut tx = state.db.write.begin().await?;
    db::messages::insert(&mut tx, &row).await?;
    db::messages::set_last_message_id(&mut tx, channel_id, row.id).await?;
    tx.commit().await?;
    if let Some(cached) = state.cache_mut().channels.get_mut(&channel_id) {
        let last = &mut cached.channel.last_message_id;
        *last = (*last).max(Some(row.id));
    }
    if let Some(message) = to_wire(state, vec![row], author_id).await?.pop() {
        state.publish(
            ServerEvent::MessageCreate(broadcast_copy(&message)),
            Audience::Channel(channel_id),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_ended_text() {
        assert_eq!(call_ended(5_000), "Call ended · 1 min");
        assert_eq!(call_ended(12 * 60_000 + 59_000), "Call ended · 12 min");
    }
}
