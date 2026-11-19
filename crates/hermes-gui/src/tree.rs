//! The sidebar as a flat list of rows (PLAN.md section 15.1), built from the store without any
//! elements so it can be tested.

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{InstanceId, view};

use hermes_client_core::proto::permissions::Permissions;

use crate::collapsed::{Collapsed, Node};
use crate::drag::{Item, Kind};

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Space {
        inst: InstanceId,
        id: Id,
        name: String,
        collapsed: bool,
        badge: Badge,
    },
    Category {
        inst: InstanceId,
        id: Id,
        name: String,
        collapsed: bool,
        badge: Badge,
        /// Set when I may move it (drag and drop).
        drag: Option<Item>,
    },
    Channel {
        inst: InstanceId,
        id: Id,
        kind: ChannelKind,
        name: String,
        /// Inside a category.
        nested: bool,
        badge: Badge,
        selected: bool,
        /// Set when I may move it (drag and drop).
        drag: Option<Item>,
        /// Muted: drawn dimmed.
        muted: bool,
        /// I am in this voice channel.
        connected: bool,
    },
    /// Someone in a voice channel (or a DM call), listed under it whether I am in voice or not.
    VoiceUser {
        inst: InstanceId,
        channel: Id,
        user: Id,
        name: String,
        muted: bool,
        deafened: bool,
        speaking: bool,
        /// It is me (no volume menu).
        me: bool,
        /// Under a channel inside a category (one level deeper).
        nested: bool,
        /// In a DM's call (under the DM, one level below the instance).
        dm: bool,
    },
    /// "New message", the first row of an instance's DMs.
    NewDm { inst: InstanceId },
    /// The heading over an instance's message requests.
    Requests { inst: InstanceId, count: usize },
    /// A message request: open it to read, accept or ignore it from the row.
    Request {
        inst: InstanceId,
        id: Id,
        name: String,
        /// The other person of a 1:1 conversation (their avatar).
        peer: Option<Id>,
        group: bool,
        selected: bool,
    },
    Dm {
        inst: InstanceId,
        id: Id,
        name: String,
        /// The other person of a 1:1 conversation (their avatar).
        peer: Option<Id>,
        group: bool,
        badge: Badge,
        selected: bool,
        /// I am in this DM's call.
        connected: bool,
    },
}

/// Unread dot and mention count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Badge {
    pub unread: bool,
    pub mentions: u32,
}

impl Badge {
    pub fn add(&mut self, other: Badge) {
        self.unread |= other.unread;
        self.mentions += other.mentions;
    }
}

pub fn space_rows(
    s: &InstanceStore,
    inst: &InstanceId,
    space: Id,
    collapsed: &Collapsed,
    selected: Option<Id>,
    speaking: &dyn Fn(&str, Id) -> bool,
    rows: &mut Vec<Row>,
) {
    let name = s.spaces.get(&space).map_or("", |sp| sp.space.name.as_str());
    let tree = view::channel_tree(s, space);
    let space_collapsed = collapsed.is_collapsed(&Node::Space(inst.clone(), space));
    let mut total = Badge::default();
    for c in &tree {
        total.add(Badge {
            unread: c.unread,
            mentions: c.mentions,
        });
    }
    rows.push(Row::Space {
        inst: inst.clone(),
        id: space,
        name: name.to_owned(),
        collapsed: space_collapsed,
        badge: if space_collapsed {
            total
        } else {
            Badge::default()
        },
    });
    if space_collapsed {
        return;
    }
    let manage = view::can_in_space(s, space, Permissions::MANAGE_CHANNELS);
    let drag = |id: Id| {
        manage.then(|| Item {
            inst: inst.clone(),
            space,
            kind: Kind::Channel,
            id,
        })
    };
    let mut hidden_by: Option<Id> = None;
    for (i, c) in tree.iter().enumerate() {
        let badge = Badge {
            unread: c.unread,
            mentions: c.mentions,
        };
        if c.kind == ChannelKind::Category {
            let cat_collapsed = collapsed.is_collapsed(&Node::Category(inst.clone(), c.id));
            hidden_by = cat_collapsed.then_some(c.id);
            let mut sum = Badge::default();
            if cat_collapsed {
                for child in tree[i + 1..].iter().take_while(|t| t.depth == 1) {
                    sum.add(Badge {
                        unread: child.unread,
                        mentions: child.mentions,
                    });
                }
            }
            rows.push(Row::Category {
                inst: inst.clone(),
                id: c.id,
                name: c.name.clone(),
                collapsed: cat_collapsed,
                badge: sum,
                drag: drag(c.id),
            });
            continue;
        }
        if c.depth == 0 {
            hidden_by = None;
        }
        if hidden_by.is_some() {
            continue;
        }
        rows.push(Row::Channel {
            inst: inst.clone(),
            id: c.id,
            kind: c.kind,
            name: c.name.clone(),
            nested: c.depth == 1,
            badge,
            selected: selected == Some(c.id),
            drag: drag(c.id),
            muted: c.muted,
            connected: my_channel(s) == Some(c.id),
        });
        if c.kind == ChannelKind::Voice {
            voice_users(s, inst, c.id, Some(space), c.depth == 1, speaking, rows);
        }
    }
}

/// The voice channel or DM call I am in on this instance.
fn my_channel(s: &InstanceStore) -> Option<Id> {
    let me = s.me_id()?;
    s.voice_states.get(&me).map(|v| v.channel_id)
}

/// The people in the voice room of `channel`, in join order.
fn voice_users(
    s: &InstanceStore,
    inst: &InstanceId,
    channel: Id,
    space: Option<Id>,
    nested: bool,
    speaking: &dyn Fn(&str, Id) -> bool,
    rows: &mut Vec<Row>,
) {
    let me = s.me_id();
    let mut occupants: Vec<_> = s
        .voice_states
        .values()
        .filter(|v| v.channel_id == channel)
        .collect();
    occupants.sort_by_key(|v| (v.joined_at, v.user_id));
    for v in occupants {
        rows.push(Row::VoiceUser {
            inst: inst.clone(),
            channel,
            user: v.user_id,
            name: s.display_name(space, v.user_id),
            muted: v.self_mute || v.server_mute,
            deafened: v.self_deaf || v.server_deaf,
            speaking: speaking(inst, v.user_id),
            me: Some(v.user_id) == me,
            nested,
            dm: space.is_none(),
        });
    }
}

pub fn dm_rows(
    s: &InstanceStore,
    inst: &InstanceId,
    selected: Option<Id>,
    speaking: &dyn Fn(&str, Id) -> bool,
    rows: &mut Vec<Row>,
) {
    rows.push(Row::NewDm { inst: inst.clone() });
    let peer = |id: Id| s.dms.get(&id).and_then(|c| s.dm_partner(c)).map(|u| u.id);
    let requests = view::dm_requests(s);
    if !requests.is_empty() {
        rows.push(Row::Requests {
            inst: inst.clone(),
            count: requests.len(),
        });
        for r in requests {
            rows.push(Row::Request {
                inst: inst.clone(),
                id: r.id,
                peer: peer(r.id),
                name: r.name,
                group: r.kind == ChannelKind::GroupDm,
                selected: selected == Some(r.id),
            });
        }
    }
    for dm in view::dm_list(s) {
        let id = dm.id;
        rows.push(Row::Dm {
            inst: inst.clone(),
            id,
            peer: peer(id),
            name: dm.name,
            group: dm.kind == ChannelKind::GroupDm,
            badge: Badge {
                unread: dm.unread,
                mentions: dm.mentions,
            },
            selected: selected == Some(id),
            connected: my_channel(s) == Some(id),
        });
        // People in the DM's call.
        voice_users(s, inst, id, None, false, speaking, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    /// A short picture of each row: kind, name and the flags that matter.
    fn describe(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Space {
                    name,
                    collapsed,
                    badge,
                    ..
                } => format!("space {name} collapsed={collapsed} {badge:?}"),
                Row::Category {
                    name,
                    collapsed,
                    badge,
                    drag,
                    ..
                } => format!(
                    "category {name} collapsed={collapsed} drag={} {badge:?}",
                    drag.is_some()
                ),
                Row::Channel {
                    name,
                    nested,
                    badge,
                    selected,
                    ..
                } => format!("channel {name} nested={nested} selected={selected} {badge:?}"),
                Row::VoiceUser {
                    name,
                    muted,
                    nested,
                    dm,
                    ..
                } => format!("voice {name} muted={muted} nested={nested} dm={dm}"),
                Row::NewDm { .. } => "new dm".to_owned(),
                Row::Requests { count, .. } => format!("requests {count}"),
                Row::Request { name, .. } => format!("request {name}"),
                Row::Dm {
                    name,
                    group,
                    badge,
                    selected,
                    ..
                } => format!("dm {name} group={group} selected={selected} {badge:?}"),
            })
            .collect()
    }

    fn space(collapsed: &Collapsed, selected: Option<Id>) -> Vec<String> {
        let store = fixtures::store();
        let mut rows = Vec::new();
        space_rows(
            &store,
            &"inst".to_owned(),
            Id(10),
            collapsed,
            selected,
            &|_, _| false,
            &mut rows,
        );
        describe(&rows)
    }

    #[test]
    fn a_space_lists_its_channels_with_people_in_voice_under_theirs() {
        let rows = space(&Collapsed::default(), Some(Id(13)));
        assert_rows(
            &rows,
            &[
                "space Friends collapsed=false Badge { unread: false, mentions: 0 }",
                "channel general nested=false selected=false Badge { unread: true, mentions: 2 }",
                "category Games collapsed=false drag=true Badge { unread: false, mentions: 0 }",
                "channel chess nested=true selected=true Badge { unread: false, mentions: 0 }",
                "channel Lounge nested=true selected=false Badge { unread: false, mentions: 0 }",
                "voice Bobby muted=true nested=true dm=false",
                "channel memes nested=false selected=false Badge { unread: true, mentions: 0 }",
            ],
        );
    }

    #[test]
    fn collapsing_hides_rows_and_sums_their_badges() {
        let mut collapsed = Collapsed::default();
        collapsed
            .toggle(&Node::Category("inst".to_owned(), Id(12)))
            .unwrap();
        let rows = space(&collapsed, None);
        assert!(
            rows.iter()
                .all(|r| !r.contains("chess") && !r.contains("Bobby")),
            "{rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.starts_with("category Games collapsed=true")),
            "{rows:#?}"
        );

        collapsed
            .toggle(&Node::Space("inst".to_owned(), Id(10)))
            .unwrap();
        assert_eq!(
            space(&collapsed, None),
            ["space Friends collapsed=true Badge { unread: true, mentions: 2 }"],
            "a folded space is one row carrying everything unread inside"
        );
    }

    #[test]
    fn direct_messages_start_with_new_message_and_list_newest_first() {
        let store = fixtures::store();
        let mut rows = Vec::new();
        dm_rows(
            &store,
            &"inst".to_owned(),
            Some(Id(21)),
            &|_, _| false,
            &mut rows,
        );
        assert_rows(
            &describe(&rows),
            &[
                "new dm",
                "dm carol group=false selected=false Badge { unread: true, mentions: 0 }",
                "dm Trio group=true selected=true Badge { unread: false, mentions: 0 }",
            ],
        );
    }

    /// `assert_eq!` that prints both lists one row per line.
    #[track_caller]
    fn assert_rows(got: &[String], want: &[&str]) {
        assert_eq!(got, want, "\ngot:\n{}\n", got.join("\n"));
    }
}
