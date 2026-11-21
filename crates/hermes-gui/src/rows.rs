//! The rows of a channel's message list (PLAN.md section 15.2), built from the store without any
//! widgets: day separators, the "New messages" divider, grouped messages with reply previews,
//! and pending messages at the end. Deleted messages are left out, except a just-deleted one
//! closing its gap (`Options::gone`).

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
    Gone(Id),
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
    /// A message just deleted: its gap closes instead of the rows below jumping up.
    Gone(Id),
}

impl Row {
    pub fn key(&self) -> RowKey {
        match self {
            Row::Older => RowKey::Older,
            Row::Day { day, .. } => RowKey::Day(*day),
            Row::NewMessages => RowKey::NewMessages,
            Row::Message(m) => RowKey::Message(m.id),
            Row::Pending(p) => RowKey::Pending(p.local_id),
            Row::Gone(id) => RowKey::Gone(*id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: Id,
    /// Continues the previous message's group: no author line.
    pub grouped: bool,
    /// The next row does not continue this message's group.
    pub ends_group: bool,
    /// A file is the first thing in it: no author line, reply or text before it.
    pub starts_with_file: bool,
    /// The next row continues the group and starts with a file.
    pub next_starts_with_file: bool,
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
    pub ends_group: bool,
}

pub struct Options {
    pub group_minutes: u32,
    pub utc_offset: i64,
    /// My read marker when the channel was opened: the divider goes before the first newer
    /// message by someone else. `None`: no divider.
    pub read_marker: Option<Id>,
    /// Deleted messages whose gap is still closing.
    pub gone: std::collections::HashSet<Id>,
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
    for m in window
        .messages
        .iter()
        .filter(|m| !m.deleted || options.gone.contains(&m.id))
    {
        if m.deleted {
            // Neither a group nor a day of its own: only its closing gap.
            rows.push(Row::Gone(m.id));
            continue;
        }
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
        let starts_with_file = grouped
            && m.reply_to.is_none()
            && m.content.is_empty()
            && m.edited_at.is_none()
            && !m.attachments.is_empty()
            && !store.blocked.contains(&m.author.id);
        rows.push(Row::Message(MessageRow {
            id: m.id,
            grouped,
            ends_group: true,
            starts_with_file,
            next_starts_with_file: false,
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
        let new_day = last_day != Some(day);
        if new_day {
            rows.push(Row::Day {
                day,
                label: time::format(p.created_at, "%Y-%m-%d", options.utc_offset),
            });
            last_day = Some(day);
        }
        // Like a sent message, a pending one never continues a group across a day separator.
        let grouped = !new_day
            && p.reply_to.is_none()
            && prev.is_some_and(|m| {
                Some(m.author.id) == me
                    && m.kind == MessageKind::Default
                    && p.created_at - (m.created_at() as i64)
                        <= i64::from(options.group_minutes) * 60_000
            });
        rows.push(Row::Pending(PendingRow {
            local_id: p.local_id,
            grouped: grouped || pending_follows_pending(&rows),
            ends_group: true,
        }));
    }
    mark_group_ends(&mut rows);
    rows
}

/// A message ends its group unless the next row continues it.
fn mark_group_ends(rows: &mut [Row]) {
    for i in 0..rows.len() {
        let (next_grouped, next_file) = match rows.get(i + 1) {
            Some(Row::Message(m)) => (m.grouped, m.starts_with_file),
            Some(Row::Pending(p)) => (p.grouped, false),
            _ => (false, false),
        };
        match &mut rows[i] {
            Row::Message(m) => {
                m.ends_group = !next_grouped;
                m.next_starts_with_file = next_file;
            }
            Row::Pending(p) => p.ends_group = !next_grouped,
            _ => {}
        }
    }
}

/// Space above a group's first message and below its last.
pub const GROUP_SPACE: f32 = 8.;
/// Space between two messages of a group, and the extra before a file that starts one.
const BETWEEN: f32 = 2.;
const COMPACT_BETWEEN: f32 = 1.;
const COMPACT_HEADER: f32 = 3.;
pub const FILE_SPACE: f32 = 4.;

/// The space above and below a message row. Inside a group, the space between two messages is
/// split evenly between them, so a hovered message's highlight has as much room under it as over
/// it, while what is shown keeps its spacing: a file that starts a message stays as far from the
/// file above it as from text.
pub fn padding(m: &MessageRow, compact: bool) -> (f32, f32) {
    let (header, between) = if compact {
        (COMPACT_HEADER, COMPACT_BETWEEN)
    } else {
        (GROUP_SPACE, BETWEEN)
    };
    let gap = |file: bool| between + if file { FILE_SPACE } else { 0. };
    let top = if m.grouped {
        gap(m.starts_with_file) / 2.
    } else {
        header
    };
    let bottom = if m.ends_group {
        GROUP_SPACE
    } else {
        gap(m.next_starts_with_file) / 2.
    };
    (top, bottom)
}

/// A message still being sent: spaced like the message it becomes (text, no files), so
/// nothing moves when the server confirms it.
pub fn pending_padding(p: &PendingRow, compact: bool) -> (f32, f32) {
    padding(
        &MessageRow {
            id: hermes_client_core::proto::id::Id(0),
            grouped: p.grouped,
            ends_group: p.ends_group,
            starts_with_file: false,
            next_starts_with_file: false,
            system: false,
            mentions_me: false,
            reply: None,
        },
        compact,
    )
}

/// A pending message right after another pending one is grouped with it.
fn pending_follows_pending(rows: &[Row]) -> bool {
    matches!(rows.last(), Some(Row::Pending(_)))
}

/// The first message under the "new messages" divider and how many messages are under it;
/// `None` without a divider.
pub fn first_new(rows: &[Row]) -> Option<(Id, usize)> {
    let at = rows.iter().position(|r| matches!(r, Row::NewMessages))?;
    let mut new = rows[at + 1..].iter().filter_map(|r| match r {
        Row::Message(m) => Some(m.id),
        _ => None,
    });
    let first = new.next()?;
    Some((first, 1 + new.count()))
}

/// Where the read marker goes when the list changes: while I watch the newest messages
/// (`watching`: the window has the focus and the list is at its end) and no divider shows, it
/// follows the newest message, so what arrives in front of me is not "new". Before the first
/// rows it stays, so what was unread on opening gets its divider.
pub fn followed_marker(
    marker: Option<Id>,
    rows: &[Row],
    watching: bool,
    newest: Option<Id>,
) -> Option<Id> {
    if !watching || rows.is_empty() || rows.contains(&Row::NewMessages) {
        return marker;
    }
    marker.max(newest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_follows_only_while_watching_without_a_divider() {
        let rows = vec![row(1, false, false), row(2, false, false)];
        assert_eq!(
            followed_marker(Some(Id(2)), &rows, true, Some(Id(3))),
            Some(Id(3)),
            "seen arriving: no divider"
        );
        assert_eq!(
            followed_marker(Some(Id(2)), &rows, false, Some(Id(3))),
            Some(Id(2)),
            "unfocused or scrolled up: it stays"
        );
        assert_eq!(
            followed_marker(Some(Id(2)), &[], true, Some(Id(3))),
            Some(Id(2)),
            "not loaded yet: unread on opening keeps its divider"
        );
        let with_divider = vec![row(1, false, false), Row::NewMessages, row(3, false, false)];
        assert_eq!(
            followed_marker(Some(Id(1)), &with_divider, true, Some(Id(4))),
            Some(Id(1)),
            "a divider stays for the visit"
        );
        assert_eq!(
            followed_marker(Some(Id(5)), &rows, true, Some(Id(3))),
            Some(Id(5)),
            "never backwards"
        );
    }

    #[test]
    fn the_jump_goes_to_the_first_message_under_the_divider() {
        assert_eq!(
            first_new(&[row(1, false, false), row(2, true, false)]),
            None
        );
        let rows = vec![
            row(1, false, false),
            Row::NewMessages,
            Row::Day {
                day: 1,
                label: String::new(),
            },
            row(3, false, false),
            row(4, true, false),
        ];
        assert_eq!(first_new(&rows), Some((Id(3), 2)));
    }

    #[test]
    fn the_last_message_of_a_group_ends_it() {
        let mut rows = vec![
            row(1, false, false),
            row(2, true, false),
            row(3, false, false),
        ];
        mark_group_ends(&mut rows);
        let ends: Vec<bool> = rows
            .iter()
            .map(|r| matches!(r, Row::Message(m) if m.ends_group))
            .collect();
        assert_eq!(ends, [false, true, true]);
    }

    fn row(id: u64, grouped: bool, starts_with_file: bool) -> Row {
        Row::Message(MessageRow {
            id: Id(id),
            grouped,
            ends_group: false,
            starts_with_file,
            next_starts_with_file: false,
            system: false,
            mentions_me: false,
            reply: None,
        })
    }

    /// A video+audio message, then two follow-ups with sounds, then one with text.
    #[test]
    fn the_space_between_grouped_messages_is_split_evenly() {
        let mut rows = vec![
            row(1, false, false),
            row(2, true, true),
            row(3, true, true),
            row(4, true, false),
        ];
        mark_group_ends(&mut rows);
        for compact in [false, true] {
            let pads: Vec<(f32, f32)> = rows
                .iter()
                .map(|r| match r {
                    Row::Message(m) => padding(m, compact),
                    _ => unreachable!(),
                })
                .collect();
            let between = if compact { 1. } else { 2. };
            // Each gap keeps what it was: the rows' space plus a file's own.
            assert_eq!(pads[0].1 + pads[1].0, between + FILE_SPACE);
            assert_eq!(pads[1].1 + pads[2].0, between + FILE_SPACE);
            assert_eq!(pads[2].1 + pads[3].0, between);
            // ...split evenly between the two.
            for pair in pads.windows(2) {
                assert_eq!(pair[0].1, pair[1].0);
            }
            assert_eq!(pads[3].1, GROUP_SPACE, "the last one ends its group");
        }
        assert_eq!(
            match &rows[0] {
                Row::Message(m) => padding(m, false).0,
                _ => unreachable!(),
            },
            GROUP_SPACE
        );
    }
    use hermes_client_core::proto::models::{MessageRef, User};
    use hermes_client_core::store::{PendingMessage, PendingState};
    use hermes_client_core::testing::fixtures;

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
            e2ee: None,
            lock: Default::default(),
            message_key: None,
        }
    }

    fn keys(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Older => "older".into(),
                Row::Day { label, .. } => format!("day {label}"),
                Row::NewMessages => "new".into(),
                Row::Gone(id) => format!("gone {}", id.0 & 0xff),
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
                gone: Default::default(),
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

        // Just deleted: its gap is a row of its own, and the rows around it are as before.
        let closing = build(
            &store,
            &window,
            &Options {
                group_minutes: 7,
                utc_offset: 0,
                gone: [id_at(t + 9 * 60_000 + 1, 4)].into_iter().collect(),
                read_marker: Some(id_at(t + 10 * 60_000, 5)),
            },
        );
        let mut expected = keys(&rows);
        expected.insert(5, "gone 4".into());
        assert_eq!(keys(&closing), expected);

        // My own newer messages never get the divider; without a marker there is none.
        let rows = build(
            &store,
            &window,
            &Options {
                group_minutes: 7,
                utc_offset: 0,
                read_marker: None,
                gone: Default::default(),
            },
        );
        assert!(!rows.contains(&Row::NewMessages));
    }

    #[test]
    fn pending_messages_start_a_group_after_midnight() {
        let store = fixtures::store();
        let me = fixtures::user(1, "me", None);
        let t = 1_793_491_200_000 - 60_000; // 2026-10-31 23:59 UTC
        let pending = |local_id, created_at: u64| PendingMessage {
            local_id,
            nonce: format!("n{local_id}"),
            channel_id: Id(11),
            content: "later".into(),
            reply_to: None,
            attachments: vec![],
            state: PendingState::Sending,
            created_at: created_at as i64,
        };
        let window = MessageWindow {
            messages: vec![message(id_at(t, 1), &me, "before midnight")],
            // Two minutes later, on the next day, and one more right after it.
            pending: vec![pending(1, t + 120_000), pending(2, t + 121_000)],
            loaded: true,
            reached_start: true,
            reached_end: true,
            ..Default::default()
        };
        let rows = build(
            &store,
            &window,
            &Options {
                group_minutes: 7,
                utc_offset: 0,
                read_marker: None,
                gone: Default::default(),
            },
        );
        assert_eq!(
            keys(&rows),
            [
                "day 2026-10-31",
                "1",
                "day 2026-11-01",
                "pending 1",
                "  pending 2"
            ]
        );
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
                    gone: Default::default(),
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
