//! The rows of a channel's message list (PLAN.md section 15.2), built from the store without any
//! widgets: day separators, the "New messages" divider, grouped messages with reply previews,
//! and pending messages at the end. Deleted messages are left out.

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{Message, MessageKind};
use hermes_client_core::store::{InstanceStore, LocalId, MessageWindow};
use hermes_client_core::{time, view};

/// Identifies a row across rebuilds (for the height cache).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKey {
    /// The "loading older messages" line at the top.
    Older,
    /// A local day, in days since 1970.
    Day(i64),
    NewMessages,
    Message(Id),
    Pending(LocalId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// More history exists above (it loads when scrolled near).
    Older,
    Day {
        day: i64,
        label: String,
    },
    NewMessages,
    Message(MessageRow),
    Pending(PendingRow),
}

impl Row {
    pub fn key(&self) -> RowKey {
        match self {
            Row::Older => RowKey::Older,
            Row::Day { day, .. } => RowKey::Day(*day),
            Row::NewMessages => RowKey::NewMessages,
            Row::Message(m) => RowKey::Message(m.id),
            Row::Pending(p) => RowKey::Pending(p.local_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: Id,
    /// Continues the previous message's group: no author line.
    pub grouped: bool,
    pub system: bool,
    pub mentions_me: bool,
    pub reply: Option<Reply>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub id: Id,
    pub author: String,
    /// Empty if the message was deleted.
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingRow {
    pub local_id: LocalId,
    pub grouped: bool,
}

pub struct Options {
    pub group_minutes: u32,
    pub utc_offset: i64,
    /// My read marker when the channel was opened: the divider goes before the first newer
    /// message by someone else. `None`: no divider.
    pub read_marker: Option<Id>,
}

pub fn build(store: &InstanceStore, window: &MessageWindow, options: &Options) -> Vec<Row> {
    let me = store.me_id();
    let space = window
        .messages
        .first()
        .and_then(|m| store.space_of(m.channel_id));
    let mut rows = Vec::new();
    if window.loaded && !window.reached_start {
        rows.push(Row::Older);
    }
    let mut divider_done = options.read_marker.is_none();
    let mut prev: Option<&Message> = None;
    let mut last_day: Option<i64> = None;
    for m in window.messages.iter().filter(|m| !m.deleted) {
        let created = m.created_at() as i64;
        let day = time::day(created, options.utc_offset);
        let new_day = last_day != Some(day);
        if new_day {
            rows.push(Row::Day {
                day,
                label: time::format(created, "%Y-%m-%d", options.utc_offset),
            });
            last_day = Some(day);
        }
        let mut divider = false;
        if !divider_done && options.read_marker.is_some_and(|r| m.id > r) && Some(m.author.id) != me
        {
            rows.push(Row::NewMessages);
            divider_done = true;
            divider = true;
        }
        let grouped = !new_day
            && !divider
            && prev.is_some_and(|p| view::is_grouped(p, m, options.group_minutes));
        rows.push(Row::Message(MessageRow {
            id: m.id,
            grouped,
            system: m.kind == MessageKind::System,
            mentions_me: store.mentions_me(m),
            reply: m.reply_to.as_ref().map(|r| Reply {
                id: r.id,
                author: store.display_name(space, r.author_id),
                excerpt: r.excerpt.clone(),
            }),
        }));
        prev = Some(m);
    }
    for p in &window.pending {
        let day = time::day(p.created_at, options.utc_offset);
        if last_day != Some(day) {
            rows.push(Row::Day {
                day,
                label: time::format(p.created_at, "%Y-%m-%d", options.utc_offset),
            });
            last_day = Some(day);
        }
        let grouped = p.reply_to.is_none()
            && prev.is_some_and(|m| {
                Some(m.author.id) == me
                    && m.kind == MessageKind::Default
                    && p.created_at - (m.created_at() as i64)
                        <= i64::from(options.group_minutes) * 60_000
            });
        rows.push(Row::Pending(PendingRow {
            local_id: p.local_id,
            grouped: grouped || pending_follows_pending(&rows),
        }));
    }
    rows
}

/// A pending message right after another pending one is grouped with it.
fn pending_follows_pending(rows: &[Row]) -> bool {
    matches!(rows.last(), Some(Row::Pending(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use hermes_client_core::proto::models::{MessageRef, User};
    use hermes_client_core::store::{PendingMessage, PendingState};

    fn id_at(unix_ms: u64, seq: u16) -> Id {
        Id::from_parts(
            unix_ms - hermes_client_core::proto::id::HERMES_EPOCH_MS,
            0,
            seq,
        )
    }

    fn message(id: Id, author: &User, content: &str) -> Message {
        Message {
            id,
            channel_id: Id(11),
            author: author.clone(),
            content: content.into(),
            reply_to: None,
            edited_at: None,
            deleted: false,
            pinned: false,
            kind: MessageKind::Default,
            attachments: vec![],
            reactions: vec![],
            mentions: Default::default(),
            nonce: None,
        }
    }

    fn keys(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Older => "older".into(),
                Row::Day { label, .. } => format!("day {label}"),
                Row::NewMessages => "new".into(),
                Row::Message(m) => format!(
                    "{}{}{}{}",
                    if m.grouped { "  " } else { "" },
                    m.id.0 & 0xff,
                    if m.mentions_me { " @me" } else { "" },
                    m.reply
                        .as_ref()
                        .map(|r| format!(" ↪{}:{}", r.author, r.excerpt))
                        .unwrap_or_default()
                ),
                Row::Pending(p) => format!(
                    "{}pending {}",
                    if p.grouped { "  " } else { "" },
                    p.local_id
                ),
            })
            .collect()
    }

    #[test]
    fn grouping_days_divider_replies_and_pending() {
        let store = fixtures::store();
        let me = fixtures::user(1, "me", None);
        let bob = fixtures::user(3, "bob", Some("Bobby"));
        // 2026-10-31 12:00 UTC and later.
        let t = 1_793_448_000_000;
        let mut window = MessageWindow {
            loaded: true,
            reached_start: false,
            reached_end: true,
            ..Default::default()
        };
        let mut push = |m: Message| window.messages.push(m);
        push(message(id_at(t, 1), &bob, "one"));
        push(message(id_at(t + 60_000, 2), &bob, "two (grouped)"));
        push(message(
            id_at(t + 9 * 60_000, 3),
            &bob,
            "three (9 min later)",
        ));
        let mut deleted = message(id_at(t + 9 * 60_000 + 1, 4), &bob, "gone");
        deleted.deleted = true;
        push(deleted);
        push(message(id_at(t + 10 * 60_000, 5), &me, "mine"));
        let mut mention = message(id_at(t + 11 * 60_000, 6), &bob, "hey <@1>");
        mention.mentions.users.push(Id(1));
        push(mention);
        let mut reply = message(id_at(t + 11 * 60_000 + 1, 7), &bob, "re");
        reply.reply_to = Some(MessageRef {
            id: id_at(t, 1),
            author_id: Id(3),
            excerpt: "one".into(),
        });
        push(reply);
        // The next day (UTC).
        push(message(id_at(t + 13 * 3_600_000, 8), &bob, "tomorrow"));
        window.pending.push(PendingMessage {
            local_id: 1,
            nonce: "n".into(),
            channel_id: Id(11),
            content: "sending".into(),
            reply_to: None,
            attachments: vec![],
            state: PendingState::Sending,
            created_at: (t + 13 * 3_600_000 + 1000) as i64,
        });
        window.pending.push(PendingMessage {
            local_id: 2,
            state: PendingState::Failed("x".into()),
            ..window.pending[0].clone()
        });

        let rows = build(
            &store,
            &window,
            &Options {
                group_minutes: 7,
                utc_offset: 0,
                read_marker: Some(id_at(t + 10 * 60_000, 5)),
            },
        );
        assert_eq!(
            keys(&rows),
            [
                "older",
                "day 2026-10-31",
                "1",
                "  2",
                "3",
                "5",
                "new",
                "6 @me",
                "7 ↪Bobby:one",
                "day 2026-11-01",
                "8",
                "pending 1",
                "  pending 2",
            ]
        );
        assert!(
            rows.iter()
                .all(|r| r.key() != RowKey::Message(id_at(t + 9 * 60_000 + 1, 4)))
        );

        // My own newer messages never get the divider; without a marker there is none.
        let rows = build(
            &store,
            &window,
            &Options {
                group_minutes: 7,
                utc_offset: 0,
                read_marker: None,
            },
        );
        assert!(!rows.contains(&Row::NewMessages));
    }

    #[test]
    fn timezone_moves_day_separators() {
        let store = fixtures::store();
        let bob = fixtures::user(3, "bob", None);
        let t = 1_793_491_200_000 - 3_600_000; // 2026-10-31 23:00 UTC
        let window = MessageWindow {
            messages: vec![
                message(id_at(t, 1), &bob, "a"),
                message(id_at(t + 2 * 3_600_000, 2), &bob, "b"),
            ],
            loaded: true,
            reached_start: true,
            reached_end: true,
            ..Default::default()
        };
        let days = |offset| {
            build(
                &store,
                &window,
                &Options {
                    group_minutes: 7,
                    utc_offset: offset,
                    read_marker: None,
                },
            )
            .iter()
            .filter(|r| matches!(r, Row::Day { .. }))
            .count()
        };
        assert_eq!(days(0), 2);
        assert_eq!(days(-3 * 3600), 1, "both on Oct 31 at UTC-3");
    }
}
