//! `GET /search` (PLAN.md section 10): FTS5 over message content, filtered to the channels the
//! user may read.

use std::sync::Arc;

use axum::extract::State;
use hermes_proto::api::{Pagination, SearchQuery, SearchResponse, SearchSort};
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
/// Results skipped at most (relevance pages go on by `offset`).
const MAX_OFFSET: u32 = 10_000;
/// The substring index needs this many characters per word.
const MIN_PART_CHARS: usize = 3;
/// Longer queries are refused.
const MAX_QUERY_CHARS: usize = 256;

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

/// The user's words as a query of the substring index (`messages_trigram`): every word must
/// appear anywhere in the text. `None` unless every word has at least three characters (shorter
/// ones are looked for as the start of a word instead, [`fts_query`]).
pub fn parts_query(input: &str) -> Option<String> {
    // Quotes and punctuation around a word are not part of what is looked for (the word index
    // drops them too).
    let words: Vec<&str> = input
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .take(MAX_TERMS)
        .collect();
    if words.is_empty() || words.iter().any(|w| w.chars().count() < MIN_PART_CHARS) {
        return None;
    }
    Some(
        words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchResponse>> {
    if q.q.chars().count() > MAX_QUERY_CHARS {
        return Err(validate::field_error(
            "q",
            format!("must be at most {MAX_QUERY_CHARS} characters"),
        ));
    }
    let fts = fts_query(&q.q);
    if fts.is_none() && !q.has_filters() {
        return Err(validate::field_error("q", "type something to search for"));
    }
    let filter = db::messages::SearchFilter {
        author: q.author_id,
        before: q.before,
        after: q.after,
        has: q.has,
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
    let parts = parts_query(&q.q);
    let text = match (&parts, &fts) {
        (Some(parts), _) => db::messages::SearchText::Parts(parts),
        (None, Some(words)) => db::messages::SearchText::Words(words),
        (None, None) => db::messages::SearchText::Nothing,
    };
    let relevance = q.sort == Some(SearchSort::Relevance);
    let offset = q.offset.unwrap_or(0).min(MAX_OFFSET);
    let first_page = q.before.is_none() && offset == 0;
    // Counting every match costs as much as the search: once, for the first page. A later page
    // (`before` is the last one shown) needs no count. The page and the total are independent
    // reads, so the first page runs them on two pooled connections at once.
    let page = db::messages::search(db, text, &channels, filter, relevance, limit, offset);
    let (rows, total) = if first_page {
        tokio::try_join!(
            page,
            db::messages::search_count(db, text, &channels, filter)
        )?
    } else {
        (page.await?, 0)
    };
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

    #[test]
    fn parts_need_three_characters_a_word() {
        assert_eq!(
            parts_query("crypted mess"),
            Some("\"crypted\" \"mess\"".into())
        );
        assert_eq!(parts_query("a\"bc"), Some("\"a\"\"bc\"".into()));
        assert_eq!(
            parts_query("\"party friday!"),
            Some("\"party\" \"friday\"".into())
        );
        assert_eq!(parts_query("hi there"), None);
        assert_eq!(parts_query(" "), None);
    }
}
