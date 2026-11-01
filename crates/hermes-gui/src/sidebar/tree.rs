//! The sidebar tree as a flat list of rows (PLAN.md section 15.1), built from the store without
//! any widgets so it can be tested.

use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{InstanceId, Store, view};

use hermes_client_core::proto::permissions::Permissions;

use super::collapsed::{Collapsed, Node};
use crate::drag::{Item, Kind};

/// The channel shown in the main area.
pub type Selection = (InstanceId, Id);

/// The sidebar shows either the spaces or the direct messages of every instance, so DMs never
/// look like one more channel list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Spaces,
    Dms,
}

impl Tab {
    /// The tab that lists a channel of `kind`.
    pub fn of(kind: ChannelKind) -> Tab {
        match kind {
            ChannelKind::Dm | ChannelKind::GroupDm => Tab::Dms,
            _ => Tab::Spaces,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Instance {
        inst: InstanceId,
        status: ConnStatus,
        collapsed: bool,
    },
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
    Dm {
        inst: InstanceId,
        id: Id,
        name: String,
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
    fn add(&mut self, other: Badge) {
        self.unread |= other.unread;
        self.mentions += other.mentions;
    }
}

/// The unread dot and mention count of everything under each tab (spaces, DMs), for the tab
/// buttons.
pub fn tab_badges(store: &Store, instances: &[InstanceId]) -> (Badge, Badge) {
    let (mut spaces, mut dms) = (Badge::default(), Badge::default());
    for s in instances.iter().filter_map(|inst| store.instance(inst)) {
        for space in s.spaces.keys() {
            for c in view::channel_tree(s, *space) {
                spaces.add(Badge {
                    unread: c.unread,
                    mentions: c.mentions,
                });
            }
        }
        for dm in view::dm_list(s) {
            dms.add(Badge {
                unread: dm.unread,
                mentions: dm.mentions,
            });
        }
    }
    (spaces, dms)
}

/// Every visible row of `tab`, instances in config order. A collapsed space or category shows
/// the sum of its channels' badges; an expanded one shows none (the channels do).
pub fn rows(
    store: &Store,
    instances: &[(InstanceId, ConnStatus)],
    collapsed: &Collapsed,
    selected: Option<&Selection>,
    tab: Tab,
    speaking: &dyn Fn(&str, Id) -> bool,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for (inst, status) in instances {
        let inst_collapsed = collapsed.is_collapsed(&Node::Instance(inst.clone()));
        rows.push(Row::Instance {
            inst: inst.clone(),
            status: status.clone(),
            collapsed: inst_collapsed,
        });
        let Some(s) = store.instance(inst) else {
            continue;
        };
        if inst_collapsed {
            continue;
        }
        let selected_here = selected.filter(|(i, _)| i == inst).map(|(_, id)| *id);
        match tab {
            Tab::Spaces => {
                for space_id in s.spaces.keys() {
                    space_rows(
                        s,
                        inst,
                        *space_id,
                        collapsed,
                        selected_here,
                        speaking,
                        &mut rows,
                    );
                }
            }
            Tab::Dms => dm_rows(s, inst, selected_here, speaking, &mut rows),
        }
    }
    rows
}

fn space_rows(
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

fn dm_rows(
    s: &InstanceStore,
    inst: &InstanceId,
    selected: Option<Id>,
    speaking: &dyn Fn(&str, Id) -> bool,
    rows: &mut Vec<Row>,
) {
    rows.push(Row::NewDm { inst: inst.clone() });
    for dm in view::dm_list(s) {
        let id = dm.id;
        rows.push(Row::Dm {
            inst: inst.clone(),
            id,
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

    fn store() -> Store {
        let mut store = Store::default();
        store.instances.insert("h".into(), fixtures::store());
        store
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Instance { inst, .. } => format!("instance {inst}"),
                Row::Space { name, badge, .. } => format!("space {name} {badge:?}"),
                Row::Category { name, badge, .. } => format!("category {name} {badge:?}"),
                Row::Channel {
                    name,
                    nested,
                    badge,
                    selected,
                    ..
                } => format!(
                    "{}{name} unread={} @{}{}",
                    if *nested { "  " } else { "" },
                    badge.unread,
                    badge.mentions,
                    if *selected { " selected" } else { "" }
                ),
                Row::VoiceUser {
                    name,
                    muted,
                    deafened,
                    speaking,
                    ..
                } => format!(
                    "    voice {name} muted={muted} deaf={deafened}{}",
                    if *speaking { " speaking" } else { "" }
                ),
                Row::NewDm { .. } => "new dm".to_owned(),
                Row::Dm {
                    name, group, badge, ..
                } => format!("dm {name} group={group} unread={}", badge.unread),
            })
            .collect()
    }

    fn connected() -> Vec<(InstanceId, ConnStatus)> {
        vec![("h".into(), ConnStatus::Connected)]
    }

    #[test]
    fn full_tree_with_badges_and_voice_users() {
        let selected = ("h".to_owned(), Id(13));
        let rows = rows(
            &store(),
            &connected(),
            &Collapsed::default(),
            Some(&selected),
            Tab::Spaces,
            &|_, _| false,
        );
        assert_eq!(
            names(&rows),
            [
                "instance h",
                "space Friends Badge { unread: false, mentions: 0 }",
                "general unread=true @2",
                "category Games Badge { unread: false, mentions: 0 }",
                "  chess unread=false @0 selected",
                "  Lounge unread=false @0",
                "    voice Bobby muted=true deaf=false",
                "memes unread=true @0",
            ]
        );
    }

    #[test]
    fn the_dm_tab_lists_only_conversations() {
        let list = rows(
            &store(),
            &connected(),
            &Collapsed::default(),
            None,
            Tab::Dms,
            &|_, _| false,
        );
        assert_eq!(
            names(&list),
            [
                "instance h",
                "new dm",
                "dm carol group=false unread=true",
                "dm Trio group=true unread=false",
            ]
        );
        let (spaces, dms) = tab_badges(&store(), &["h".to_owned()]);
        assert_eq!(
            spaces,
            Badge {
                unread: true,
                mentions: 2
            }
        );
        assert_eq!(
            dms,
            Badge {
                unread: true,
                mentions: 0
            }
        );
        assert_eq!(Tab::of(ChannelKind::GroupDm), Tab::Dms);
        assert_eq!(Tab::of(ChannelKind::Voice), Tab::Spaces);
    }

    #[test]
    fn collapsed_nodes_hide_children_and_sum_badges() {
        let mut collapsed = Collapsed::default();
        collapsed
            .toggle(&Node::Category("h".into(), Id(12)))
            .unwrap();
        let list = rows(
            &store(),
            &connected(),
            &collapsed,
            None,
            Tab::Spaces,
            &|_, _| false,
        );
        let shown = names(&list);
        assert!(shown.contains(&"category Games Badge { unread: false, mentions: 0 }".into()));
        assert!(
            !shown
                .iter()
                .any(|n| n.contains("chess") || n.contains("Bobby"))
        );
        assert!(shown.contains(&"memes unread=true @0".into()), "{shown:?}");

        collapsed.toggle(&Node::Space("h".into(), Id(10))).unwrap();
        let list = rows(
            &store(),
            &connected(),
            &collapsed,
            None,
            Tab::Spaces,
            &|_, _| false,
        );
        assert_eq!(
            names(&list)[..2],
            [
                "instance h".to_owned(),
                "space Friends Badge { unread: true, mentions: 2 }".to_owned()
            ]
        );

        collapsed.toggle(&Node::Instance("h".into())).unwrap();
        let list = rows(
            &store(),
            &connected(),
            &collapsed,
            None,
            Tab::Spaces,
            &|_, _| false,
        );
        assert_eq!(names(&list), ["instance h"]);
    }

    #[test]
    fn speaking_rings_and_dm_call_participants() {
        use hermes_client_core::proto::models::VoiceState;

        let mut store = store();
        let s = store.instances.get_mut("h").unwrap();
        let carol_dm = s
            .dms
            .values()
            .find(|d| d.kind == ChannelKind::Dm)
            .unwrap()
            .id;
        let carol = s.dms[&carol_dm]
            .recipients
            .iter()
            .find(|u| u.username == "carol")
            .unwrap()
            .id;
        s.voice_states.insert(
            carol,
            VoiceState {
                user_id: carol,
                channel_id: carol_dm,
                self_mute: false,
                self_deaf: true,
                server_mute: false,
                server_deaf: false,
                joined_at: 5,
            },
        );
        let speaking = |_: &str, user: Id| user == carol;
        let mut shown = names(&rows(
            &store,
            &connected(),
            &Collapsed::default(),
            None,
            Tab::Dms,
            &speaking,
        ));
        shown.extend(names(&rows(
            &store,
            &connected(),
            &Collapsed::default(),
            None,
            Tab::Spaces,
            &speaking,
        )));
        let dm = shown
            .iter()
            .position(|n| n.starts_with("dm carol"))
            .unwrap();
        assert_eq!(
            shown[dm + 1],
            "    voice carol muted=false deaf=true speaking"
        );
        assert!(shown.contains(&"    voice Bobby muted=true deaf=false".to_owned()));
    }

    #[test]
    fn the_channel_i_am_in_is_connected() {
        use hermes_client_core::proto::models::VoiceState;

        let mut store = store();
        let s = store.instances.get_mut("h").unwrap();
        let me = s.me_id().unwrap();
        let lounge = Id(14); // "Lounge" in the fixtures
        s.voice_states.insert(
            me,
            VoiceState {
                user_id: me,
                channel_id: lounge,
                self_mute: false,
                self_deaf: false,
                server_mute: false,
                server_deaf: false,
                joined_at: 9,
            },
        );
        let list = rows(
            &store,
            &connected(),
            &Collapsed::default(),
            None,
            Tab::Spaces,
            &|_, _| false,
        );
        let connected: Vec<&str> = list
            .iter()
            .filter_map(|row| match row {
                Row::Channel {
                    name,
                    connected: true,
                    ..
                } => Some(name.as_str()),
                Row::Dm {
                    name,
                    connected: true,
                    ..
                } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(connected, ["Lounge"]);
        // Lounge is inside the Games category, so its people are one level deeper.
        assert!(list.iter().any(|row| matches!(
            row,
            Row::VoiceUser {
                nested: true,
                me: true,
                ..
            }
        )));
    }

    #[test]
    fn instances_without_a_store_are_one_row() {
        let rows = rows(
            &Store::default(),
            &[("other".into(), ConnStatus::LoggedOut)],
            &Collapsed::default(),
            None,
            Tab::Dms,
            &|_, _| false,
        );
        assert_eq!(
            rows,
            [Row::Instance {
                inst: "other".into(),
                status: ConnStatus::LoggedOut,
                collapsed: false
            }]
        );
    }
}
