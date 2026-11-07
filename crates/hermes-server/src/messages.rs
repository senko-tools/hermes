//! Messages beyond plain SQL: building wire [`Message`]s, deciding which mentions count and who
//! they notify, and slowmode (PLAN.md sections 7.4, 8 and 10).

use std::collections::HashMap;
use std::sync::Mutex;

use hermes_proto::id::Id;
use hermes_proto::markup;
use hermes_proto::models::{Mentions, Message, MessageRef, ReactionSummary, Status, User};
use hermes_proto::permissions::Permissions;

use crate::db;
use crate::db::messages::MessageRow;
use crate::state::{AppState, Cache};
use crate::time::SECOND_MS;

/// Length of [`MessageRef::excerpt`].
pub const EXCERPT_CHARS: usize = 100;
/// Distinct reaction emoji per message.
pub const MAX_REACTION_EMOJI: usize = 20;

/// Builds wire messages as `viewer` sees them (`ReactionSummary::me`). `nonce` is left empty.
pub async fn to_wire(
    state: &AppState,
    rows: Vec<MessageRow>,
    viewer: Id,
) -> Result<Vec<Message>, sqlx::Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let db = &state.db.read;
    let ids: Vec<Id> = rows.iter().map(|r| r.id).collect();
    let reply_ids: Vec<Id> = rows.iter().filter_map(|r| r.reply_to_id).collect();
    let replies = if reply_ids.is_empty() {
        Vec::new()
    } else {
        db::messages::reply_refs(db, &reply_ids).await?
    };
    let attachments = db::messages::attachments(db, &ids).await?;
    let reactions = db::messages::reaction_counts(db, &ids, viewer).await?;
    let mentions = db::messages::mentions(db, &ids).await?;

    let cache = state.cache();
    Ok(rows
        .into_iter()
        .map(|r| Message {
            id: r.id,
            channel_id: r.channel_id,
            author: author(&cache, r.author_id),
            content: r.content,
            reply_to: r.reply_to_id.and_then(|reply_id| {
                let (id, author_id, content, deleted) =
                    replies.iter().find(|(id, ..)| *id == reply_id)?;
                Some(MessageRef {
                    id: *id,
                    author_id: *author_id,
                    excerpt: if *deleted {
                        String::new()
                    } else {
                        content.chars().take(EXCERPT_CHARS).collect()
                    },
                })
            }),
            edited_at: r.edited_at,
            deleted: r.deleted,
            pinned: r.pinned,
            kind: r.kind,
            attachments: attachments
                .iter()
                .filter(|(message_id, _)| *message_id == r.id)
                .map(|(_, a)| a.clone())
                .collect(),
            reactions: reactions
                .iter()
                .filter(|c| c.message_id == r.id)
                .map(|c| ReactionSummary {
                    emoji: c.emoji.clone(),
                    count: c.count,
                    me: c.me,
                })
                .collect(),
            mentions: mentions
                .iter()
                .find(|(message_id, _)| *message_id == r.id)
                .map(|(_, m)| m.clone())
                .unwrap_or_default(),
            nonce: None,
        })
        .collect())
}

/// The form of a message broadcast to everyone who can see it: without the nonce, and with
/// `ReactionSummary::me` false, because one payload is shared by all viewers (clients keep their
/// own `me` from `reaction_add` / `reaction_remove`).
pub fn broadcast_copy(message: &Message) -> Message {
    let mut message = message.clone();
    message.nonce = None;
    for reaction in &mut message.reactions {
        reaction.me = false;
    }
    message
}

/// The cached user, or a placeholder (users are never deleted in v1, so this should not happen).
fn author(cache: &Cache, user_id: Id) -> User {
    cache.users.get(&user_id).map_or_else(
        || User {
            id: user_id,
            username: "unknown".into(),
            discriminator: None,
            display_name: None,
            avatar: None,
            status_text: None,
        },
        |u| u.user.clone(),
    )
}

/// The mentions of a message that count, and the users whose mention count goes up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedMentions {
    pub mentions: Mentions,
    /// Sorted; never the author; only users who can see the channel.
    pub notify: Vec<Id>,
}

/// Which mentions in `content` count when written by `author_id` with `perms` in the channel:
/// `@everyone`/`@here` need `MENTION_EVERYONE`; roles must be mentionable unless the author has
/// `MENTION_EVERYONE`; user mentions must name existing users. `@here` notifies only users who
/// are online (not invisible).
pub fn resolve_mentions(
    cache: &Cache,
    channel_id: Id,
    author_id: Id,
    perms: Permissions,
    content: &str,
) -> ResolvedMentions {
    let Some(channel) = cache.channels.get(&channel_id) else {
        return ResolvedMentions::default();
    };
    let raw = markup::extract_mentions(content);
    let can_everyone = perms.contains(Permissions::MENTION_EVERYONE);
    let space = channel
        .channel
        .space_id
        .and_then(|id| cache.spaces.get(&id));
    let mentions = Mentions {
        users: raw
            .users
            .into_iter()
            .filter(|id| cache.users.contains_key(id))
            .collect(),
        roles: raw
            .roles
            .into_iter()
            .filter(|&id| {
                space
                    .and_then(|s| s.role(id))
                    .is_some_and(|r| !r.is_everyone() && (r.mentionable || can_everyone))
            })
            .collect(),
        everyone: raw.everyone && can_everyone,
        here: raw.here && can_everyone,
    };
    if mentions.is_empty() {
        return ResolvedMentions::default();
    }

    let viewers: Vec<Id> = match space {
        Some(space) => space
            .members
            .keys()
            .copied()
            .filter(|&user_id| {
                cache
                    .channel_permissions(channel_id, user_id)
                    .contains(Permissions::VIEW_CHANNEL)
            })
            .collect(),
        None => channel.recipient_ids.clone(),
    };
    let online = |user_id: &Id| {
        cache
            .presences
            .get(user_id)
            .is_some_and(|p| p.visible_status() != Status::Offline)
    };
    let has_mentioned_role = |user_id: &Id| {
        space
            .and_then(|s| s.members.get(user_id))
            .is_some_and(|m| m.role_ids.iter().any(|r| mentions.roles.contains(r)))
    };
    let mut notify: Vec<Id> = viewers
        .into_iter()
        .filter(|&user_id| user_id != author_id)
        .filter(|user_id| {
            mentions.everyone
                || (mentions.here && online(user_id))
                || mentions.users.contains(user_id)
                || has_mentioned_role(user_id)
        })
        .collect();
    notify.sort();
    ResolvedMentions { mentions, notify }
}

/// Last send time per `(channel, user)` for slowmode. Runtime only.
#[derive(Debug, Default)]
pub struct Slowmode(Mutex<HashMap<(Id, Id), i64>>);

/// Entries older than the longest slowmode are dropped once the map grows this large.
const SLOWMODE_PRUNE_AT: usize = 10_000;
const MAX_SLOWMODE_MS: i64 =
    hermes_proto::api::UpdateChannelRequest::MAX_SLOWMODE_SECS as i64 * SECOND_MS;

impl Slowmode {
    /// Records a send at `now` if the user may send in the channel; otherwise returns how many
    /// milliseconds they still have to wait.
    pub fn try_send(&self, channel_id: Id, user_id: Id, secs: u32, now: i64) -> Result<(), u64> {
        let mut last_sent = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let wait = i64::from(secs) * SECOND_MS;
        if let Some(&last) = last_sent.get(&(channel_id, user_id)) {
            let elapsed = now - last;
            if elapsed < wait {
                return Err((wait - elapsed) as u64);
            }
        }
        if last_sent.len() >= SLOWMODE_PRUNE_AT {
            last_sent.retain(|_, &mut last| now - last < MAX_SLOWMODE_MS);
        }
        last_sent.insert((channel_id, user_id), now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slowmode_waits_per_channel_and_user() {
        let s = Slowmode::default();
        let (c1, c2, u1, u2) = (Id(1), Id(2), Id(10), Id(11));
        assert_eq!(s.try_send(c1, u1, 10, 1_000), Ok(()));
        assert_eq!(s.try_send(c1, u1, 10, 4_000), Err(7_000));
        assert_eq!(s.try_send(c2, u1, 10, 4_000), Ok(()));
        assert_eq!(s.try_send(c1, u2, 10, 4_000), Ok(()));
        assert_eq!(s.try_send(c1, u1, 10, 11_000), Ok(()));
    }
}
