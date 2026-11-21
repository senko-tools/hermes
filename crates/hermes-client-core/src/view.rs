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
        // Kinds from a newer server: nothing here could open them.
        .filter(|c| c.kind != ChannelKind::Unknown)
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
                .filter(|child| child.parent_id == Some(c.id) && child.kind != ChannelKind::Unknown)
                .collect();
            children.sort_by(|a, b| order(a, b));
            out.extend(children.into_iter().map(|child| entry(child, 1)));
        }
    }
    out
}

/// Who is in a voice channel of a space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceHere {
    /// People in its voice channels that I can see.
    pub count: usize,
    /// I am one of them.
    pub me: bool,
}

/// Who is in voice in `space`, counting only channels I can see (others are not in the store).
pub fn voice_in_space(store: &InstanceStore, space: Id) -> VoiceHere {
    let Some(state) = store.spaces.get(&space) else {
        return VoiceHere::default();
    };
    let me = store.me.as_ref().map(|m| m.user.id);
    let mut here = VoiceHere::default();
    for v in store.voice_states.values() {
        if state.channels.contains_key(&v.channel_id) {
            here.count += 1;
            here.me |= Some(v.user_id) == me;
        }
    }
    here
}

/// Where a dragged channel lands on the row it is dropped on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    /// Just above the row.
    Before,
    /// Just below the row (below a category's channels for a category).
    After,
    /// First inside the category the row is (a channel only; a category takes it as `Before`).
    Into,
}

/// The new positions after dropping channel `moved` at `place` of `onto` in the space's tree
/// (for `PUT /spaces/{id}/channels/order`). A category stays at the top level: dropped next to
/// a channel inside a category, it goes after that category. Only entries whose position or
/// parent change are returned.
pub fn move_channel(
    store: &InstanceStore,
    space: Id,
    moved: Id,
    onto: Id,
    place: Place,
) -> Vec<hermes_proto::api::ChannelPosition> {
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
        match (parent_of(t), place) {
            (Some(p), _) => (None, Some((p, Place::After))),
            (None, Place::Into) => (None, Some((onto, Place::Before))),
            (None, place) => (None, Some((onto, place))),
        }
    } else if t.kind == ChannelKind::Category && place == Place::Into {
        (Some(onto), None)
    } else {
        let place = if place == Place::Into {
            Place::Before
        } else {
            place
        };
        (parent_of(t), Some((onto, place)))
    };
    if anchor.is_some_and(|(a, _)| a == moved) {
        return Vec::new();
    }
    place_channel(state, m, parent, anchor)
}

/// The new positions after taking channel `moved` out of its category: first at the top level,
/// above everything. Empty for a category, or a channel already there.
pub fn move_channel_out(
    store: &InstanceStore,
    space: Id,
    moved: Id,
) -> Vec<hermes_proto::api::ChannelPosition> {
    let Some(state) = store.spaces.get(&space) else {
        return Vec::new();
    };
    let Some(m) = state.channels.get(&moved) else {
        return Vec::new();
    };
    if m.kind == ChannelKind::Category {
        return Vec::new();
    }
    place_channel(state, m, None, None)
}

/// `m` among the children of `parent`: before or after `anchor`, or first without one.
fn place_channel(
    state: &crate::store::SpaceState,
    m: &Channel,
    parent: Option<Id>,
    anchor: Option<(Id, Place)>,
) -> Vec<hermes_proto::api::ChannelPosition> {
    use hermes_proto::api::ChannelPosition;

    let moved = m.id;
    let parent_of = |c: &Channel| c.parent_id.filter(|p| state.channels.contains_key(p));
    let mut siblings: Vec<&Channel> = state
        .channels
        .values()
        .filter(|c| parent_of(c) == parent && c.id != moved)
        .collect();
    siblings.sort_by(|a, b| order(a, b));
    let index = match anchor {
        None => 0,
        Some((anchor, place)) => {
            let at = siblings.iter().position(|c| c.id == anchor).unwrap_or(0);
            if place == Place::After { at + 1 } else { at }
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
    dms_where(store, false)
}

/// Message requests waiting for me to accept them, newest first. They never count as unread.
pub fn dm_requests(store: &InstanceStore) -> Vec<TreeChannel> {
    dms_where(store, true)
}

fn dms_where(store: &InstanceStore, requests: bool) -> Vec<TreeChannel> {
    let mut dms: Vec<&Channel> = store
        .dms
        .values()
        .filter(|c| c.request == requests)
        .collect();
    dms.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
    dms.into_iter()
        .map(|c| {
            let muted = is_quiet(store, c.id);
            TreeChannel {
                id: c.id,
                kind: c.kind,
                name: dm_name(store, c),
                depth: 0,
                unread: !requests && !muted && store.unread(c.id),
                mentions: if requests {
                    0
                } else {
                    store.mention_count(c.id)
                },
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
pub fn member_groups(store: &InstanceStore, space: Id, channel: Option<Id>) -> Vec<MemberGroup> {
    let Some(state) = store.spaces.get(&space) else {
        return Vec::new();
    };
    // In a channel, only the people who can see it (improvements.md #3).
    let channel = channel.and_then(|c| state.channels.get(&c));
    let sees = |member: &hermes_proto::models::Member| {
        channel.is_none_or(|c| {
            hermes_proto::permissions::compute_for(
                &state.space,
                &state.roles,
                member.user.id,
                &member.role_ids,
                Some(c),
            )
            .contains(Permissions::VIEW_CHANNEL)
        })
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
    for member in state.members.values().filter(|m| sees(m)) {
        let status = store.status(member.user.id);
        let color = role_color(store, space, member.user.id);
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
        // Cached: the key allocates, and big spaces have thousands of members.
        g.members
            .sort_by_cached_key(|m| (m.name.to_lowercase(), m.user_id));
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
                    out.push('@');
                    out.push_str(&role_mention_name(store, space, *id));
                }
                Span::ChannelMention(id) => {
                    out.push('#');
                    out.push_str(&channel_mention_name(store, *id));
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

/// What can be mentioned in a channel, as typed: `@name`, `@name#1234`, `@role`, `#channel`,
/// with the markup each stands for. A plain `@name` is only there when one person has that name.
fn mention_names(store: &InstanceStore, channel: Id) -> Vec<(String, String)> {
    let space = store.space_of(channel);
    let state = space.and_then(|s| store.spaces.get(&s));
    let mut users: Vec<hermes_proto::models::User> = match state {
        Some(state) => state.members.values().map(|m| m.user.clone()).collect(),
        None => store
            .channel(channel)
            .map(|c| c.recipients.clone())
            .unwrap_or_default(),
    };
    if let Some(me) = &store.me
        && !users.iter().any(|u| u.id == me.user.id)
    {
        users.push(me.user.clone());
    }
    let mut out = Vec::new();
    for user in &users {
        let id = user.id;
        out.push((format!("@{}", user.handle()), format!("<@{id}>")));
        let same = users
            .iter()
            .filter(|u| u.username.eq_ignore_ascii_case(&user.username))
            .count();
        if same == 1 && user.discriminator.is_some() {
            out.push((format!("@{}", user.username), format!("<@{id}>")));
        }
    }
    if let Some(state) = state {
        for role in state.roles.iter().filter(|r| !r.is_everyone()) {
            out.push((format!("@{}", role.name), format!("<@&{}>", role.id)));
        }
        for c in state
            .channels
            .values()
            .filter(|c| c.kind == ChannelKind::Text)
        {
            if let Some(name) = &c.name {
                out.push((format!("#{name}"), format!("<#{}>", c.id)));
            }
        }
    }
    // Longest first: `@bob#1234` before `@bob`, `@Mods team` before `@Mods`.
    out.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    out
}

/// Turns typed mentions into markup before sending: `@bob` into `<@3>`, `#general` into
/// `<#11>`. Not inside code, not in the middle of a word (an email), and only whole names.
pub fn resolve_mentions(store: &InstanceStore, channel: Id, text: &str) -> String {
    let names = mention_names(store, channel);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut previous: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        // Code is copied as it is: ```blocks``` and `spans`.
        if c == '`' {
            let fence = if rest.starts_with("```") { "```" } else { "`" };
            let end = rest[fence.len()..]
                .find(fence)
                .map_or(rest.len(), |i| i + 2 * fence.len());
            out.push_str(&rest[..end]);
            previous = rest[..end].chars().last();
            rest = &rest[end..];
            continue;
        }
        let at_start = previous.is_none_or(|p| !p.is_alphanumeric() && p != '_');
        if (c == '@' || c == '#') && at_start {
            let found = names.iter().find(|(name, _)| {
                rest.len() >= name.len()
                    && rest.is_char_boundary(name.len())
                    && rest[..name.len()].eq_ignore_ascii_case(name)
                    && rest[name.len()..]
                        .chars()
                        .next()
                        .is_none_or(|n| !n.is_alphanumeric() && n != '_' && n != '#')
            });
            if let Some((name, markup)) = found {
                out.push_str(markup);
                previous = name.chars().last();
                rest = &rest[name.len()..];
                continue;
            }
        }
        out.push(c);
        previous = Some(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The other way, for editing a sent message: `<@3>` back into `@bob`.
pub fn readable_mentions(store: &InstanceStore, channel: Id, text: &str) -> String {
    let names = mention_names(store, channel);
    let mut out = text.to_owned();
    // The shortest name of each markup: `@bob` rather than `@bob#1234` where that is enough.
    let mut seen = std::collections::HashSet::new();
    for (name, markup) in names.iter().rev() {
        if seen.insert(markup) {
            out = out.replace(markup.as_str(), name);
        }
    }
    out
}

/// One autocomplete suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Shown in the popup.
    pub label: String,
    /// Replaces the typed token (e.g. `@bob `); [`resolve_mentions`] turns it into markup.
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
            let names = mention_names(store, channel);
            for (id, username, display) in users {
                let key = format!("{username} {display}");
                // The shortest typed form that stands for them alone.
                let markup = format!("<@{id}>");
                let typed = names
                    .iter()
                    .rev()
                    .find(|(_, m)| *m == markup)
                    .map_or_else(|| format!("@{username}"), |(name, _)| name.clone());
                push(
                    &key,
                    format!("@{display} ({username})"),
                    format!("{typed} "),
                );
            }
            if let Some(state) = space.and_then(|s| store.spaces.get(&s)) {
                for role in state.roles.iter().filter(|r| !r.is_everyone()) {
                    push(
                        &role.name,
                        format!("@{} (role)", role.name),
                        format!("@{} ", role.name),
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
                    push(&name, format!("#{name}"), format!("#{name} "));
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
    // One entry per insert (an emoji with two matching names), the best-ranked one; the
    // duplicates are not always next to each other after sorting.
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .map(|(_, s)| s)
        .filter(|s| seen.insert(s.insert.clone()))
        .take(max)
        .collect()
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

/// The emoji picker's tabs, in Unicode's order: a name and every emoji of the group.
pub fn emoji_groups() -> Vec<(&'static str, Vec<&'static str>)> {
    use emojis::Group;
    Group::iter()
        .map(|group| {
            let name = match group {
                Group::SmileysAndEmotion => "Smileys",
                Group::PeopleAndBody => "People",
                Group::AnimalsAndNature => "Nature",
                Group::FoodAndDrink => "Food",
                Group::TravelAndPlaces => "Travel",
                Group::Activities => "Activities",
                Group::Objects => "Objects",
                Group::Symbols => "Symbols",
                Group::Flags => "Flags",
            };
            (name, group.emojis().map(emojis::Emoji::as_str).collect())
        })
        .collect()
}

/// An emoji's first shortcode, for the picker's hint (`thumbsup`).
pub fn emoji_name(emoji: &str) -> Option<&'static str> {
    emojis::get(emoji).and_then(emojis::Emoji::shortcode)
}

/// The colour of `user`'s name in `space`: their first coloured role, in the space's role order.
pub fn role_color(s: &InstanceStore, space: Id, user: Id) -> Option<u32> {
    let state = s.spaces.get(&space)?;
    let member = state.members.get(&user)?;
    state
        .roles
        .iter()
        .filter(|r| member.role_ids.contains(&r.id))
        .find_map(|r| r.color)
}

/// The name a role mention shows (without the `@`); `deleted-role` once the role is gone.
pub fn role_mention_name(s: &InstanceStore, space: Option<Id>, role: Id) -> String {
    space
        .and_then(|sp| s.spaces.get(&sp))
        .and_then(|st| st.role(role))
        .map_or_else(|| "deleted-role".to_owned(), |r| r.name.clone())
}

/// The name a channel mention shows (without the `#`); `deleted-channel` once it is gone.
pub fn channel_mention_name(s: &InstanceStore, channel: Id) -> String {
    s.channel(channel)
        .and_then(|c| c.name.clone())
        .unwrap_or_else(|| "deleted-channel".to_owned())
}

/// `Bob is typing…`, `Bob and Carol are typing…`, `Bob, Carol and Dan are typing…`,
/// `Several people are typing…`.
pub fn typing_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [a] => Some(format!("{a} is typing…")),
        [a, b] => Some(format!("{a} and {b} are typing…")),
        [a, b, c] => Some(format!("{a}, {b} and {c} are typing…")),
        _ => Some("Several people are typing…".into()),
    }
}

/// `1.4 MB`, `820 KB`, `12 B`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_channel_lists_only_who_can_see_it() {
        use hermes_proto::models::Override;
        let mut s = crate::testing::fixtures::store();
        let (space, chess) = (Id(10), Id(13));
        let names = |s: &InstanceStore, channel| -> Vec<String> {
            member_groups(s, space, channel)
                .into_iter()
                .flat_map(|g| g.members.into_iter().map(|m| m.name))
                .collect()
        };
        assert_eq!(
            names(&s, Some(chess)).len(),
            2,
            "everyone sees #chess at first"
        );
        // @everyone may not see #chess: Bobby is gone, the owner (me) still sees it.
        s.spaces
            .get_mut(&space)
            .unwrap()
            .channels
            .get_mut(&chess)
            .unwrap()
            .overrides
            .push(Override {
                role_id: space,
                allow: Permissions::empty(),
                deny: Permissions::VIEW_CHANNEL,
            });
        assert_eq!(names(&s, Some(chess)), ["me"]);
        assert_eq!(
            names(&s, None).len(),
            2,
            "the server as a whole still lists both"
        );
    }

    #[test]
    fn sizes_and_typing_read_naturally() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(820 * 1024), "820 KB");
        assert_eq!(human_size(1_468_006), "1.4 MB");
        assert_eq!(typing_text(&[]), None);
        assert_eq!(typing_text(&["A".into()]).unwrap(), "A is typing…");
        assert_eq!(
            typing_text(&["A".into(), "B".into(), "C".into()]).unwrap(),
            "A, B and C are typing…"
        );
        assert_eq!(
            typing_text(&["A".into(), "B".into(), "C".into(), "D".into()]).unwrap(),
            "Several people are typing…"
        );
    }

    use super::*;
    use crate::store::tests::{channel, message, ready, user};

    #[test]
    fn emoji_groups_cover_the_lot() {
        let groups = emoji_groups();
        assert_eq!(groups.len(), 9);
        assert_eq!(groups[0].0, "Smileys");
        assert!(groups[0].1.contains(&"😀"));
        assert_eq!(emoji_name("👍"), Some("+1"));
    }
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
    fn voice_counts_only_channels_i_can_see() {
        use hermes_proto::models::VoiceState;
        let mut s = store();
        s.apply(
            ServerEvent::ChannelCreate(channel(21, Some(10), ChannelKind::Voice, "Lounge")),
            0,
        );
        assert_eq!(voice_in_space(&s, Id(10)), VoiceHere::default());
        let me = s.me.as_ref().unwrap().user.id;
        let state = |user, channel| VoiceState {
            user_id: Id(user),
            channel_id: Id(channel),
            self_mute: false,
            self_deaf: false,
            server_mute: false,
            server_deaf: false,
            joined_at: 0,
        };
        // Someone in Lounge, someone in a channel I cannot see.
        s.voice_states.insert(Id(500), state(500, 21));
        s.voice_states.insert(Id(501), state(501, 99));
        assert_eq!(
            voice_in_space(&s, Id(10)),
            VoiceHere {
                count: 1,
                me: false
            }
        );
        s.voice_states.insert(me, state(me.0, 21));
        assert_eq!(voice_in_space(&s, Id(10)), VoiceHere { count: 2, me: true });
        // Another space, or none, has nobody.
        assert_eq!(voice_in_space(&s, Id(77)), VoiceHere::default());
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

        // memes before general: memes goes first.
        let moves = move_channel(&s, Id(10), Id(22), general, Place::Before);
        apply(&mut s, moves);
        assert_eq!(names(&s)[0].0, "memes");
        assert_eq!(names(&s)[1].0, "general");

        // memes into the category: first in it.
        let moves = move_channel(&s, Id(10), Id(22), Id(20), Place::Into);
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

        // After the last channel of the category.
        let moves = move_channel(&s, Id(10), Id(22), Id(21), Place::After);
        apply(&mut s, moves);
        assert_eq!(names(&s)[2].0, "Lounge");
        assert_eq!(names(&s)[3].0, "memes");

        // The category before general: it goes first, children follow it.
        let moves = move_channel(&s, Id(10), Id(20), general, Place::Before);
        apply(&mut s, moves);
        assert_eq!(names(&s)[0].0, "Voice");
        assert_eq!(names(&s)[3].0, "general");

        assert!(move_channel(&s, Id(10), Id(22), Id(22), Place::After).is_empty());

        // Out of the category: first at the top level.
        let moves = move_channel_out(&s, Id(10), Id(22));
        assert_eq!(
            moves.iter().find(|p| p.id == Id(22)).unwrap().parent_id,
            Some(None)
        );
        apply(&mut s, moves);
        assert_eq!(names(&s)[0], ("memes".to_owned(), 0));
        assert!(
            move_channel_out(&s, Id(10), Id(22)).is_empty(),
            "already out"
        );
        assert!(
            move_channel_out(&s, Id(10), Id(20)).is_empty(),
            "a category"
        );
        // Back into the category for the steps below.
        let moves = move_channel(&s, Id(10), Id(22), Id(21), Place::Before);
        apply(&mut s, moves);

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

        // general after the category, the last row at the top level: it goes to the very end.
        let moves = move_channel(&s, Id(10), general, Id(20), Place::After);
        apply(&mut s, moves);
        assert_eq!(names(&s).last().unwrap(), &("general".to_owned(), 0));
        // A category dropped next to a channel inside a category goes after that category.
        assert!(
            move_channel(&s, Id(10), Id(20), Id(21), Place::Before).is_empty(),
            "next to its own channel"
        );
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
        let groups = member_groups(&s, Id(10), None);
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
        assert_eq!(at[0].insert, "@alice ");
        assert_eq!(complete(&s, Id(11), "#gen", 5)[0].insert, "#general ");
        assert!(
            complete(&s, Id(11), ":smi", 5)
                .iter()
                .any(|s| s.insert == "😄 ")
        );
        assert_eq!(complete(&s, Id(11), "/shr", 5)[0].insert, "/shrug ");
        assert_eq!(search_emoji("thumbsup", 1)[0].0, "👍");
    }

    #[test]
    fn completion_lists_each_emoji_once() {
        let s = store();
        // 🚗 is both `car` (a prefix match) and `red_car` (a match inside).
        for token in [":ca", ":car", ":smi", ":po", ":+1"] {
            let all = complete(&s, Id(11), token, usize::MAX);
            let mut inserts: Vec<&str> = all.iter().map(|s| s.insert.as_str()).collect();
            let before = inserts.len();
            inserts.sort_unstable();
            inserts.dedup();
            assert_eq!(inserts.len(), before, "{token}: {all:?}");
        }
        let car = complete(&s, Id(11), ":car", 1);
        assert_eq!(car[0].label, "🚗 :car:", "the best-ranked name is kept");
    }

    #[test]
    fn typed_mentions_become_markup_and_back() {
        let mut s = store();
        let tagged = |id: u64, name: &str, tag: &str| hermes_proto::models::User {
            discriminator: Some(tag.into()),
            ..user(id, name)
        };
        let member = |user| Member {
            space_id: Id(10),
            user,
            nickname: None,
            role_ids: vec![],
            joined_at: 0,
        };
        s.set_members(
            Id(10),
            vec![
                member(user(2, "alice")),
                member(tagged(3, "bob", "1234")),
                member(tagged(4, "bob", "5678")),
                member(tagged(5, "carol", "1111")),
            ],
        );
        let typed = "hi @Alice, mail a@alice.com `@alice` #general @bob#5678 @bob @carol @alicex";
        let sent = resolve_mentions(&s, Id(11), typed);
        assert_eq!(
            sent, "hi <@2>, mail a@alice.com `@alice` <#11> <@4> @bob <@5> @alicex",
            "two bobs need their tag; a word that only starts with a name stays"
        );
        assert_eq!(
            readable_mentions(&s, Id(11), "<@2> <@4> <@5> <#11>"),
            "@alice @bob#5678 @carol #general"
        );
        assert_eq!(complete(&s, Id(11), "@car", 5)[0].insert, "@carol ");
    }
}
