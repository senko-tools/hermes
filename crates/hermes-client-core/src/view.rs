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
    pub unread: bool,
    pub mentions: u32,
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
    let entry = |c: &Channel, depth| TreeChannel {
        id: c.id,
        kind: c.kind,
        name: c.name.clone().unwrap_or_default(),
        depth,
        unread: c.kind.has_messages() && store.unread(c.id),
        mentions: store.mention_count(c.id),
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

/// DMs, most recent activity first, with a display name.
pub fn dm_list(store: &InstanceStore) -> Vec<TreeChannel> {
    let mut dms: Vec<&Channel> = store.dms.values().collect();
    dms.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
    dms.into_iter()
        .map(|c| TreeChannel {
            id: c.id,
            kind: c.kind,
            name: dm_name(store, c),
            depth: 0,
            unread: store.unread(c.id),
            mentions: store.mention_count(c.id),
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
