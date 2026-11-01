//! `GET /search` (PLAN.md section 10): FTS5 over message content, filtered to the channels the
//! user may read.

use std::sync::Arc;

use axum::extract::State;
use hermes_proto::api::{Pagination, SearchQuery, SearchResponse};
use hermes_proto::id::Id;
use hermes_proto::models::ChannelKind;
use hermes_proto::permissions::Permissions;

use super::{ApiResult, Json, Query, validate};
use crate::auth::Auth;
use crate::db;
use crate::messages;
use crate::state::{AppState, Cache};

/// At most this many search words are used.
const MAX_TERMS: usize = 16;

/// Channels `user_id` may read (`VIEW_CHANNEL` + `READ_HISTORY`), DMs included, optionally only
/// in one space or one channel.
pub fn readable_channels(
    cache: &Cache,
    user_id: Id,
    space_id: Option<Id>,
    channel_id: Option<Id>,
) -> Vec<Id> {
    let need = Permissions::VIEW_CHANNEL | Permissions::READ_HISTORY;
    let mut out: Vec<Id> = cache
        .channels
        .iter()
        .filter(|(id, c)| {
            c.channel.kind.has_messages()
                && channel_id.is_none_or(|want| want == **id)
                && match c.channel.space_id {
                    Some(space) => space_id.is_none_or(|want| want == space),
                    None => space_id.is_none(),
                }
        })
        .filter(|(id, c)| match c.channel.kind {
            ChannelKind::Dm | ChannelKind::GroupDm => c.recipient_ids.contains(&user_id),
            _ => cache.channel_permissions(**id, user_id).contains(need),
        })
        .map(|(id, _)| *id)
        .collect();
    out.sort();
    out
}

/// The user's words as an FTS5 query: every word must match, each as a prefix. Quotes are
/// escaped, so no input is an FTS syntax error. `None` if there are no words.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .take(MAX_TERMS)
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchResponse>> {
    let Some(fts) = fts_query(&q.q) else {
        return Err(validate::field_error("q", "type something to search for"));
    };
    let channels = readable_channels(&state.cache(), auth.user_id, q.space_id, q.channel_id);
    if channels.is_empty() {
        return Ok(Json(SearchResponse {
            messages: Vec::new(),
            total: 0,
        }));
    }
    let limit = Pagination {
        limit: q.limit,
        ..Pagination::default()
    }
    .limit();
    let db = &state.db.read;
    let rows = db::messages::search(db, &fts, &channels, q.author_id, q.before, limit).await?;
    let total = db::messages::search_count(db, &fts, &channels, q.author_id).await?;
    Ok(Json(SearchResponse {
        messages: messages::to_wire(&state, rows, auth.user_id).await?,
        total: u64::try_from(total).unwrap_or(0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_escaped_prefixes() {
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("hello wor"), Some("\"hello\"* \"wor\"*".into()));
        assert_eq!(fts_query("a\"b OR"), Some("\"a\"\"b\"* \"OR\"*".into()));
    }
}
