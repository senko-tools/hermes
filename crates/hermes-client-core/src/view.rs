//! Pure view helpers used by both UIs (PLAN.md section 14.1): the channel tree with unread and
//! mention flags, the member list grouped by hoisted roles, message grouping and autocomplete.

use hermes_proto::id::Id;
use hermes_proto::models::{Channel, ChannelKind, Message, MessageKind, Status};
use hermes_proto::permissions::Permissions;

use crate::store::InstanceStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeChannel {
    pub id: Id,
    pub kind: ChannelKind,
    pub name: String,
    /// 0 = top level, 1 = inside a category.
    pub depth: u8,
    /// Unread messages; never set while muted (mentions still count).
    pub unread: bool,
    pub mentions: u32,
    /// Muted, or set to notify about nothing (shown dimmed).
    pub muted: bool,
}

/// Whether a channel is muted or set to "nothing" (its own setting or its space's).
pub fn is_quiet(store: &InstanceStore, channel: Id) -> bool {
    crate::notify::level(store, channel, crate::core::now_ms())
        .is_none_or(|l| l == hermes_proto::models::NotificationLevel::None)
}

fn order(a: &Channel, b: &Channel) -> std::cmp::Ordering {
    a.position.cmp(&b.position).then(a.id.cmp(&b.id))
}

/// A space's channels in display order: top-level channels and categories by position, each
/// category followed by its children. Channels I cannot see are not in the store at all.
pub fn channel_tree(store: &InstanceStore, space: Id) -> Vec<TreeChannel> {
    let Some(state) = store.spaces.get(&space) else {
        return Vec::new();
    };
    let mut top: Vec<&Channel> = state
        .channels
        .values()
        .filter(|c| c.parent_id.is_none_or(|p| !state.channels.contains_key(&p)))
        .collect();
    top.sort_by(|a, b| order(a, b));
    let entry = |c: &Channel, depth| {
        let muted = c.kind.has_messages() && is_quiet(store, c.id);
        TreeChannel {
            id: c.id,
            kind: c.kind,
            name: c.name.clone().unwrap_or_default(),
            depth,
            unread: c.kind.has_messages() && !muted && store.unread(c.id),
            mentions: store.mention_count(c.id),
            muted,
        }
    };
    let mut out = Vec::new();
    for c in top {
        out.push(entry(c, 0));
        if c.kind == ChannelKind::Category {
            let mut children: Vec<&Channel> = state
                .channels
                .values()
                .filter(|child| child.parent_id == Some(c.id))
                .collect();
            children.sort_by(|a, b| order(a, b));
            out.extend(children.into_iter().map(|child| entry(child, 1)));
        }
    }
    out
}

/// The new positions after dropping channel `moved` onto `onto` in the space's tree (for
/// `PUT /spaces/{id}/channels/order`): `moved` takes `onto`'s place among its siblings. A channel
/// dropped on a category goes first into it; a category dropped inside a category takes that
/// category's place. Only entries whose position or parent change are returned.
pub fn move_channel(
    store: &InstanceStore,
    space: Id,
    moved: Id,
    onto: Id,
) -> Vec<hermes_proto::api::ChannelPosition> {
    use hermes_proto::api::ChannelPosition;

    let Some(state) = store.spaces.get(&space) else {
        return Vec::new();
    };
    let (Some(m), Some(t)) = (state.channels.get(&moved), state.channels.get(&onto)) else {
        return Vec::new();
    };
    if moved == onto {
        return Vec::new();
    }
    let parent_of = |c: &Channel| c.parent_id.filter(|p| state.channels.contains_key(p));
    let (parent, anchor) = if m.kind == ChannelKind::Category {
        // Categories live at the top level; aim at the item there.
        match parent_of(t) {
            Some(p) => (None, Some(p)),
            None => (None, Some(onto)),
        }
    } else if t.kind == ChannelKind::Category {
        (Some(onto), None)
    } else {
        (parent_of(t), Some(onto))
    };
    if anchor == Some(moved) {
        return Vec::new();
    }
    let mut siblings: Vec<&Channel> = state
        .channels
        .values()
        .filter(|c| parent_of(c) == parent && c.id != moved)
        .collect();
    siblings.sort_by(|a, b| order(a, b));
    let index = match anchor {
        None => 0,
        Some(anchor) => {
            let at = siblings.iter().position(|c| c.id == anchor).unwrap_or(0);
            // Moving down within the same list lands after the anchor.
            let was_before = parent_of(m) == parent && order(m, &state.channels[&anchor]).is_lt();
            if was_before { at + 1 } else { at }
        }
    };
    siblings.insert(index.min(siblings.len()), m);
    siblings
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let position = i as i32;
            let reparent = c.id == moved && parent_of(c) != parent;
            (c.position != position || reparent).then_some(ChannelPosition {
                id: c.id,
                position,
                parent_id: (c.id == moved).then_some(parent),
            })
        })
        .collect()
}

/// The new positions after moving channel `id` one place up (`delta < 0`) or down among its
/// siblings (same parent); empty at either end.
pub fn step_channel(
    store: &InstanceStore,
    space: Id,
    id: Id,
    delta: i32,
) -> Vec<hermes_proto::api::ChannelPosition> {
    use hermes_proto::api::ChannelPosition;

    let Some(state) = store.spaces.get(&space) else {
        return Vec::new();
    };
    let Some(channel) = state.channels.get(&id) else {
        return Vec::new();
    };
    let parent_of = |c: &Channel| c.parent_id.filter(|p| state.channels.contains_key(p));
    let parent = parent_of(channel);
    let mut siblings: Vec<&Channel> = state
        .channels
        .values()
        .filter(|c| parent_of(c) == parent)
        .collect();
    siblings.sort_by(|a, b| order(a, b));
    let Some(i) = siblings.iter().position(|c| c.id == id) else {
        return Vec::new();
    };
    let j = i as i64 + i64::from(delta.signum());
    if j < 0 || j as usize >= siblings.len() {
        return Vec::new();
    }
    siblings.swap(i, j as usize);
    siblings
        .iter()
        .enumerate()
        .filter(|(i, c)| c.position != *i as i32)
        .map(|(i, c)| ChannelPosition {
            id: c.id,
            position: i as i32,
            parent_id: None,
        })
        .collect()
}

/// DMs, most recent activity first, with a display name.
pub fn dm_list(store: &InstanceStore) -> Vec<TreeChannel> {
    let mut dms: Vec<&Channel> = store.dms.values().collect();
    dms.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
    dms.into_iter()
        .map(|c| {
            let muted = is_quiet(store, c.id);
            TreeChannel {
                id: c.id,
                kind: c.kind,
                name: dm_name(store, c),
                depth: 0,
                unread: !muted && store.unread(c.id),
                mentions: store.mention_count(c.id),
                muted,
            }
        })
        .collect()
}

/// A 1:1 DM is named after the other person; a group DM by its name or its members.
pub fn dm_name(store: &InstanceStore, dm: &Channel) -> String {
    if let Some(name) = dm.name.as_deref().filter(|n| !n.is_empty()) {
        return name.to_owned();
    }
    if let Some(user) = store.dm_partner(dm) {
        return user.name().to_owned();
    }
    let me = store.me_id();
    let names: Vec<&str> = dm
        .recipients
        .iter()
        .filter(|u| Some(u.id) != me)
        .map(|u| u.name())
        .collect();
    if names.is_empty() {
        "(empty group)".into()
    } else {
        names.join(", ")
    }
}

/// The name shown for any channel: `#name` for text channels, the DM name for DMs.
pub fn channel_title(store: &InstanceStore, channel: &Channel) -> String {
    match channel.kind {
        ChannelKind::Dm | ChannelKind::GroupDm => dm_name(store, channel),
        ChannelKind::Text => format!("#{}", channel.name.as_deref().unwrap_or("")),
        _ => channel.name.clone().unwrap_or_default(),
    }
}

/// The voice channel called `name` (`/join Lounge`, `:voice lounge`), case-insensitive. One in
/// the space of `near` (the channel being viewed) wins over one in another space.
pub fn find_voice_channel(store: &InstanceStore, near: Option<Id>, name: &str) -> Option<Id> {
    let wanted = name
        .trim()
        .trim_start_matches(['#', '♪'])
        .trim()
        .to_lowercase();
    let space = near.and_then(|c| store.space_of(c));
    let mut found: Vec<&Channel> = store
        .spaces
        .values()
        .flat_map(|sp| sp.channels.values())
        .filter(|c| {
            c.kind == ChannelKind::Voice
                && c.name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase() == wanted)
        })
        .collect();
    found.sort_by_key(|c| (c.space_id != space, c.id));
    found.first().map(|c| c.id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub user_id: Id,
    pub name: String,
    pub status: Status,
    /// Color of the highest colored role.
    pub color: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberGroup {
    /// A hoisted role's name, `Online` or `Offline`.
    pub title: String,
    pub members: Vec<MemberRow>,
}

/// Members grouped like section 16.1: online members under their highest hoisted role, then
/// other online members, then everyone offline. Sorted by name inside each group.
pub fn member_groups(store: &InstanceStore, space: Id) -> Vec<MemberGroup> {
    let Some(state) = store.spaces.get(&space) else {
        return Vec::new();
    };
    // Roles are sorted highest first.
    let hoisted: Vec<_> = state
        .roles
        .iter()
        .filter(|r| r.hoist && !r.is_everyone())
        .collect();
    let mut groups: Vec<MemberGroup> = hoisted
        .iter()
        .map(|r| MemberGroup {
            title: r.name.clone(),
            members: Vec::new(),
        })
        .collect();
    let mut online = Vec::new();
    let mut offline = Vec::new();
    for member in state.members.values() {
        let status = store.status(member.user.id);
        let color = state
            .roles
            .iter()
            .find(|r| r.color.is_some() && member.role_ids.contains(&r.id))
            .and_then(|r| r.color);
        let row = MemberRow {
            user_id: member.user.id,
            name: member
                .nickname
                .clone()
                .unwrap_or_else(|| member.user.name().to_owned()),
            status,
            color,
        };
        if status == Status::Offline {
            offline.push(row);
        } else if let Some(i) = hoisted.iter().position(|r| member.role_ids.contains(&r.id)) {
            groups[i].members.push(row);
        } else {
            online.push(row);
        }
    }
    groups.push(MemberGroup {
        title: "Online".into(),
        members: online,
    });
    groups.push(MemberGroup {
        title: "Offline".into(),
        members: offline,
    });
    for g in &mut groups {
        g.members
            .sort_by_key(|m| (m.name.to_lowercase(), m.user_id));
    }
    groups.retain(|g| !g.members.is_empty());
    groups
}

/// Whether `message` continues `prev`'s group: same author, within `minutes`, not a reply,
/// neither a system message.
pub fn is_grouped(prev: &Message, message: &Message, minutes: u32) -> bool {
    prev.author.id == message.author.id
        && prev.kind == MessageKind::Default
        && message.kind == MessageKind::Default
        && message.reply_to.is_none()
        && message.created_at().saturating_sub(prev.created_at()) <= u64::from(minutes) * 60_000
}

/// Whether I may do `perm` in a channel (for greying out actions).
pub fn can(store: &InstanceStore, channel: Id, perm: Permissions) -> bool {
    store.channel_permissions(channel).contains(perm)
}

/// Whether I have `perm` space-wide.
pub fn can_in_space(store: &InstanceStore, space: Id, perm: Permissions) -> bool {
    store.permissions(space, None).contains(perm)
}

/// Whether I may edit, delete or assign `role`: `MANAGE_ROLES` and a higher role (the owner
/// always may). `@everyone` can be edited but never assigned or deleted.
pub fn can_manage_role(store: &InstanceStore, space: Id, role: Id) -> bool {
    let (Some(me), Some(state)) = (store.me_id(), store.spaces.get(&space)) else {
        return false;
    };
    if !can_in_space(store, space, Permissions::MANAGE_ROLES) {
        return false;
    }
    if state.space.owner_id == me {
        return true;
    }
    match (state.members.get(&me), state.role(role)) {
        (Some(actor), Some(role)) => {
            hermes_proto::permissions::can_manage_role(&state.space, &state.roles, actor, role)
        }
        _ => false,
    }
}

/// Whether I am above `user` in the role hierarchy (kick, ban, roles, nickname). Nobody is above
/// the owner, and I am never above myself.
pub fn outranks(store: &InstanceStore, space: Id, user: Id) -> bool {
    let (Some(me), Some(state)) = (store.me_id(), store.spaces.get(&space)) else {
        return false;
    };
    if me == user || user == state.space.owner_id {
        return false;
    }
    if state.space.owner_id == me {
        return true;
    }
    match (state.members.get(&me), state.members.get(&user)) {
        (Some(actor), Some(target)) => {
            hermes_proto::permissions::can_manage_member(&state.space, &state.roles, actor, target)
        }
        _ => false,
    }
}

/// Message markup as one line of plain text (for result lists and previews): formatting is
/// dropped, mentions show as `@name` / `#channel` / `@role`, spoilers as `▒▒▒`, line breaks as
/// spaces. At most `max` characters, with `…` when cut.
pub fn plain_text(store: &InstanceStore, space: Option<Id>, content: &str, max: usize) -> String {
    use hermes_proto::markup::{Span, parse};

    fn walk(store: &InstanceStore, space: Option<Id>, spans: &[Span], out: &mut String) {
        for span in spans {
            match span {
                Span::Text(t) | Span::Code(t) => out.push_str(t),
                Span::LineBreak => out.push(' '),
                Span::CodeBlock { code, .. } => {
                    out.push(' ');
                    out.push_str(&code.replace('\n', " "));
                    out.push(' ');
                }
                Span::Bold(c)
                | Span::Italic(c)
                | Span::Underline(c)
                | Span::Strike(c)
                | Span::Quote(c) => walk(store, space, c, out),
                Span::Link { children, .. } => walk(store, space, children, out),
                Span::Spoiler(_) => out.push_str("▒▒▒"),
                Span::UserMention(id) => {
                    out.push('@');
                    out.push_str(&store.display_name(space, *id));
                }
                Span::RoleMention(id) => {
                    let name = space
                        .and_then(|sp| store.spaces.get(&sp))
                        .and_then(|st| st.role(*id))
                        .map_or("role", |r| r.name.as_str());
                    out.push('@');
                    out.push_str(name);
                }
                Span::ChannelMention(id) => {
                    out.push('#');
                    out.push_str(
                        store
                            .channel(*id)
                            .and_then(|c| c.name.as_deref())
                            .unwrap_or("channel"),
                    );
                }
                Span::Everyone => out.push_str("@everyone"),
                Span::Here => out.push_str("@here"),
            }
        }
    }

    let mut out = String::new();
    walk(store, space, &parse(content), &mut out);
    let collapsed: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max {
        let mut cut: String = collapsed.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        collapsed
    }
}

/// One autocomplete suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Shown in the popup.
    pub label: String,
    /// Replaces the typed token (e.g. `<@123> `).
    pub insert: String,
}

/// Suggestions for the token being typed (`@us`, `#ch`, `:emo`, `/cmd`), best first.
pub fn complete(store: &InstanceStore, channel: Id, token: &str, max: usize) -> Vec<Suggestion> {
    let Some(first) = token.chars().next() else {
        return Vec::new();
    };
    let query = token[first.len_utf8()..].to_lowercase();
    let space = store.space_of(channel);
    let mut out: Vec<(bool, Suggestion)> = Vec::new();
    let mut push = |name: &str, label: String, insert: String| {
        let lower = name.to_lowercase();
        if lower.contains(&query) {
            out.push((!lower.starts_with(&query), Suggestion { label, insert }));
        }
    };
    match first {
        '@' => {
            for special in ["everyone", "here"] {
                push(special, format!("@{special}"), format!("@{special} "));
            }
            let users: Vec<(Id, String, String)> = match space.and_then(|s| store.spaces.get(&s)) {
                Some(state) => state
                    .members
                    .values()
                    .map(|m| {
                        let display = store.display_name(space, m.user.id);
                        (m.user.id, m.user.username.clone(), display)
                    })
                    .collect(),
                None => store
                    .channel(channel)
                    .map(|c| {
                        c.recipients
                            .iter()
                            .map(|u| (u.id, u.username.clone(), u.name().to_owned()))
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            for (id, username, display) in users {
                let key = format!("{username} {display}");
                push(
                    &key,
                    format!("@{display} ({username})"),
                    format!("<@{id}> "),
                );
            }
            if let Some(state) = space.and_then(|s| store.spaces.get(&s)) {
                for role in state.roles.iter().filter(|r| !r.is_everyone()) {
                    push(
                        &role.name,
                        format!("@{} (role)", role.name),
                        format!("<@&{}> ", role.id),
                    );
                }
            }
        }
        '#' => {
            if let Some(state) = space.and_then(|s| store.spaces.get(&s)) {
                for c in state
                    .channels
                    .values()
                    .filter(|c| c.kind == ChannelKind::Text)
                {
                    let name = c.name.clone().unwrap_or_default();
                    push(&name, format!("#{name}"), format!("<#{}> ", c.id));
                }
            }
        }
        ':' => {
            if query.len() >= 2 {
                for emoji in emojis::iter() {
                    for code in emoji.shortcodes() {
                        push(
                            code,
                            format!("{} :{code}:", emoji.as_str()),
                            format!("{} ", emoji.as_str()),
                        );
                    }
                }
            }
        }
        '/' => {
            for (name, usage) in crate::commands::COMMANDS {
                push(name, (*usage).to_owned(), format!("/{name} "));
            }
        }
        _ => {}
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.len().cmp(&b.1.label.len())));
    out.dedup_by(|a, b| a.1.insert == b.1.insert);
    out.into_iter().take(max).map(|(_, s)| s).collect()
}

/// Emoji matching a search (for the reaction picker): `(emoji, shortcode)`.
pub fn search_emoji(query: &str, max: usize) -> Vec<(String, String)> {
    let query = query.trim().trim_matches(':').to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(bool, String, String)> = Vec::new();
    for emoji in emojis::iter() {
        if let Some(code) = emoji.shortcodes().find(|c| c.contains(&query)) {
            out.push((
                !code.starts_with(&query),
                emoji.as_str().to_owned(),
                code.to_owned(),
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.len().cmp(&b.2.len())));
    out.into_iter().take(max).map(|(_, e, c)| (e, c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{channel, message, ready, user};
    use hermes_proto::gateway::ServerEvent;
    use hermes_proto::models::{Member, Presence, Role};

    fn store() -> InstanceStore {
        let mut s = InstanceStore::default();
        s.apply_ready(ready());
        s
    }

    #[test]
    fn tree_orders_categories_and_children() {
        let mut s = store();
        let mut cat = channel(20, Some(10), ChannelKind::Category, "Voice");
        cat.position = 1;
        let mut lounge = channel(21, Some(10), ChannelKind::Voice, "Lounge");
        lounge.parent_id = Some(Id(20));
        let mut memes = channel(22, Some(10), ChannelKind::Text, "memes");
        memes.position = 2;
        memes.last_message_id = Some(Id(5));
        for c in [lounge, memes, cat] {
            s.apply(ServerEvent::ChannelCreate(c), 0);
        }
        let tree = channel_tree(&s, Id(10));
        let names: Vec<(&str, u8)> = tree.iter().map(|c| (c.name.as_str(), c.depth)).collect();
        assert_eq!(
            names,
            [("general", 0), ("Voice", 0), ("Lounge", 1), ("memes", 0)]
        );
        assert!(tree[3].unread);
        assert!(!tree[0].unread);
    }

    #[test]
    fn dropping_a_channel_takes_the_target_place() {
        let mut s = store();
        let general = channel_tree(&s, Id(10))[0].id;
        let mut cat = channel(20, Some(10), ChannelKind::Category, "Voice");
        cat.position = 1;
        let mut lounge = channel(21, Some(10), ChannelKind::Voice, "Lounge");
        lounge.parent_id = Some(Id(20));
        let mut memes = channel(22, Some(10), ChannelKind::Text, "memes");
        memes.position = 2;
        for c in [lounge, memes, cat] {
            s.apply(ServerEvent::ChannelCreate(c), 0);
        }
        let apply = |s: &mut InstanceStore, moves: Vec<hermes_proto::api::ChannelPosition>| {
            for p in moves {
                let mut c = s.channel(p.id).unwrap().clone();
                c.position = p.position;
                if let Some(parent) = p.parent_id {
                    c.parent_id = parent;
                }
                s.apply(ServerEvent::ChannelUpdate(c), 0);
            }
        };
        let names = |s: &InstanceStore| -> Vec<(String, u8)> {
            channel_tree(s, Id(10))
                .into_iter()
                .map(|c| (c.name, c.depth))
                .collect()
        };

        // memes onto general: memes goes first.
        let moves = move_channel(&s, Id(10), Id(22), general);
        apply(&mut s, moves);
        assert_eq!(names(&s)[0].0, "memes");
        assert_eq!(names(&s)[1].0, "general");

        // memes onto the category: into it, first.
        let moves = move_channel(&s, Id(10), Id(22), Id(20));
        assert_eq!(
            moves.iter().find(|p| p.id == Id(22)).unwrap().parent_id,
            Some(Some(Id(20)))
        );
        apply(&mut s, moves);
        assert_eq!(
            names(&s),
            [
                ("general".to_owned(), 0),
                ("Voice".to_owned(), 0),
                ("memes".to_owned(), 1),
                ("Lounge".to_owned(), 1)
            ]
        );

        // Moving down inside the category lands after the target.
        let moves = move_channel(&s, Id(10), Id(22), Id(21));
        apply(&mut s, moves);
        assert_eq!(names(&s)[2].0, "Lounge");
        assert_eq!(names(&s)[3].0, "memes");

        // The category onto general: it goes first, children follow it.
        let moves = move_channel(&s, Id(10), Id(20), general);
        apply(&mut s, moves);
        assert_eq!(names(&s)[0].0, "Voice");
        assert_eq!(names(&s)[3].0, "general");

        assert!(move_channel(&s, Id(10), Id(22), Id(22)).is_empty());

        // One step up and down among siblings.
        let moves = step_channel(&s, Id(10), Id(22), -1);
        apply(&mut s, moves);
        assert_eq!(names(&s)[1].0, "memes");
        assert!(
            step_channel(&s, Id(10), Id(22), -1).is_empty(),
            "already first"
        );
        let moves = step_channel(&s, Id(10), Id(20), 1);
        apply(&mut s, moves);
        assert_eq!(
            names(&s)[0].0,
            "general",
            "the category moved below general"
        );
        assert_eq!(names(&s)[1].0, "Voice");
    }

    #[test]
    fn plain_text_flattens_markup() {
        let s = store();
        let general = channel_tree(&s, Id(10))[0].id;
        let text = format!("**hi** <#{general}>\n||secret|| `x`  y");
        assert_eq!(
            plain_text(&s, Some(Id(10)), &text, 100),
            "hi #general ▒▒▒ x y"
        );
        assert_eq!(plain_text(&s, None, "abcdef", 4), "abc…");
    }

    #[test]
    fn voice_channels_are_found_by_name() {
        let mut s = store();
        s.apply(
            ServerEvent::ChannelCreate(channel(21, Some(10), ChannelKind::Voice, "Lounge")),
            0,
        );
        s.apply(
            ServerEvent::ChannelCreate(channel(22, Some(10), ChannelKind::Text, "chat")),
            0,
        );
        assert_eq!(find_voice_channel(&s, None, "lounge"), Some(Id(21)));
        assert_eq!(
            find_voice_channel(&s, Some(Id(22)), " #LOUNGE "),
            Some(Id(21))
        );
        assert_eq!(
            find_voice_channel(&s, None, "chat"),
            None,
            "text channels are not rooms"
        );
        assert_eq!(find_voice_channel(&s, None, "nowhere"), None);
    }

    #[test]
    fn members_grouped_by_hoisted_role() {
        let mut s = store();
        s.apply(
            ServerEvent::RoleCreate(Role {
                id: Id(30),
                space_id: Id(10),
                name: "Admin".into(),
                color: Some(0xff0000),
                hoist: true,
                mentionable: false,
                position: 1,
                permissions: Permissions::empty(),
            }),
            0,
        );
        let member = |id, name: &str, roles: Vec<Id>| Member {
            space_id: Id(10),
            user: user(id, name),
            nickname: None,
            role_ids: roles,
            joined_at: 0,
        };
        s.set_members(
            Id(10),
            vec![
                member(1, "me", vec![]),
                member(2, "alice", vec![Id(30)]),
                member(3, "bob", vec![]),
                member(4, "carol", vec![Id(30)]),
            ],
        );
        for (id, status) in [(1, Status::Online), (3, Status::Idle)] {
            s.apply(
                ServerEvent::PresenceUpdate(Presence {
                    user_id: Id(id),
                    status,
                }),
                0,
            );
        }
        // ready() has user 2 online.
        let groups = member_groups(&s, Id(10));
        let flat: Vec<(String, Vec<String>)> = groups
            .iter()
            .map(|g| {
                (
                    g.title.clone(),
                    g.members.iter().map(|m| m.name.clone()).collect(),
                )
            })
            .collect();
        assert_eq!(
            flat,
            [
                ("Admin".to_owned(), vec!["alice".to_owned()]),
                ("Online".to_owned(), vec!["bob".to_owned(), "me".to_owned()]),
                ("Offline".to_owned(), vec!["carol".to_owned()]),
            ]
        );
        assert_eq!(groups[0].members[0].color, Some(0xff0000));
    }

    #[test]
    fn grouping_within_minutes() {
        let a = user(1, "a");
        let b = user(2, "b");
        let at = |minute: u64, author: &hermes_proto::models::User| {
            let id = Id::from_parts(minute * 60_000, 0, 0);
            message(id.0, 11, author, "x")
        };
        assert!(is_grouped(&at(0, &a), &at(7, &a), 7));
        assert!(!is_grouped(&at(0, &a), &at(8, &a), 7));
        assert!(!is_grouped(&at(0, &a), &at(1, &b), 7));
    }

    #[test]
    fn completion() {
        let mut s = store();
        s.set_members(
            Id(10),
            vec![Member {
                space_id: Id(10),
                user: user(2, "alice"),
                nickname: Some("Ally".into()),
                role_ids: vec![],
                joined_at: 0,
            }],
        );
        let at = complete(&s, Id(11), "@al", 5);
        assert_eq!(at[0].insert, "<@2> ");
        assert_eq!(complete(&s, Id(11), "#gen", 5)[0].insert, "<#11> ");
        assert!(
            complete(&s, Id(11), ":smi", 5)
                .iter()
                .any(|s| s.insert == "😄 ")
        );
        assert_eq!(complete(&s, Id(11), "/shr", 5)[0].insert, "/shrug ");
        assert_eq!(search_emoji("thumbsup", 1)[0].0, "👍");
    }
}
