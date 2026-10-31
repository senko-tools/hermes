//! The sidebar tree as a flat list of rows (PLAN.md section 15.1), built from the store without
//! any widgets so it can be tested.

use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{InstanceId, Store, view};

use super::collapsed::{Collapsed, Node};

/// The channel shown in the main area.
pub type Selection = (InstanceId, Id);

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
    },
    /// Someone in a voice channel, listed under it whether I am in voice or not.
    VoiceUser {
        inst: InstanceId,
        channel: Id,
        user: Id,
        name: String,
        muted: bool,
        deafened: bool,
    },
    Dms {
        inst: InstanceId,
        collapsed: bool,
        badge: Badge,
    },
    Dm {
        inst: InstanceId,
        id: Id,
        name: String,
        group: bool,
        badge: Badge,
        selected: bool,
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

/// Every visible row, instances in config order. A collapsed space or category shows the sum of
/// its channels' badges; an expanded one shows none (the channels do).
pub fn rows(
    store: &Store,
    instances: &[(InstanceId, ConnStatus)],
    collapsed: &Collapsed,
    selected: Option<&Selection>,
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
        for (space_id, space) in &s.spaces {
            space_rows(
                s,
                inst,
                *space_id,
                &space.space.name,
                collapsed,
                selected_here,
                &mut rows,
            );
        }
        dm_rows(s, inst, collapsed, selected_here, &mut rows);
    }
    rows
}

fn space_rows(
    s: &InstanceStore,
    inst: &InstanceId,
    space: Id,
    name: &str,
    collapsed: &Collapsed,
    selected: Option<Id>,
    rows: &mut Vec<Row>,
) {
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
        });
        if c.kind == ChannelKind::Voice {
            let mut occupants: Vec<_> = s
                .voice_states
                .values()
                .filter(|v| v.channel_id == c.id)
                .collect();
            occupants.sort_by_key(|v| (v.joined_at, v.user_id));
            for v in occupants {
                rows.push(Row::VoiceUser {
                    inst: inst.clone(),
                    channel: c.id,
                    user: v.user_id,
                    name: s.display_name(Some(space), v.user_id),
                    muted: v.self_mute || v.server_mute,
                    deafened: v.self_deaf || v.server_deaf,
                });
            }
        }
    }
}

fn dm_rows(
    s: &InstanceStore,
    inst: &InstanceId,
    collapsed: &Collapsed,
    selected: Option<Id>,
    rows: &mut Vec<Row>,
) {
    let dms = view::dm_list(s);
    let dms_collapsed = collapsed.is_collapsed(&Node::Dms(inst.clone()));
    let mut total = Badge::default();
    if dms_collapsed {
        for dm in &dms {
            total.add(Badge {
                unread: dm.unread,
                mentions: dm.mentions,
            });
        }
    }
    rows.push(Row::Dms {
        inst: inst.clone(),
        collapsed: dms_collapsed,
        badge: total,
    });
    if dms_collapsed {
        return;
    }
    rows.extend(dms.into_iter().map(|dm| Row::Dm {
        inst: inst.clone(),
        id: dm.id,
        name: dm.name,
        group: dm.kind == ChannelKind::GroupDm,
        badge: Badge {
            unread: dm.unread,
            mentions: dm.mentions,
        },
        selected: selected == Some(dm.id),
    }));
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
                    ..
                } => format!("    voice {name} muted={muted} deaf={deafened}"),
                Row::Dms { badge, .. } => format!("dms {badge:?}"),
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
                "dms Badge { unread: false, mentions: 0 }",
                "dm carol group=false unread=true",
                "dm Trio group=true unread=false",
            ]
        );
    }

    #[test]
    fn collapsed_nodes_hide_children_and_sum_badges() {
        let mut collapsed = Collapsed::default();
        collapsed
            .toggle(&Node::Category("h".into(), Id(12)))
            .unwrap();
        collapsed.toggle(&Node::Dms("h".into())).unwrap();
        let list = rows(&store(), &connected(), &collapsed, None);
        let shown = names(&list);
        assert!(shown.contains(&"category Games Badge { unread: false, mentions: 0 }".into()));
        assert!(
            !shown
                .iter()
                .any(|n| n.contains("chess") || n.contains("Bobby"))
        );
        assert!(shown.contains(&"memes unread=true @0".into()), "{shown:?}");
        assert!(shown.contains(&"dms Badge { unread: true, mentions: 0 }".into()));
        assert!(!shown.iter().any(|n| n.starts_with("dm ")));

        collapsed.toggle(&Node::Space("h".into(), Id(10))).unwrap();
        let list = rows(&store(), &connected(), &collapsed, None);
        assert_eq!(
            names(&list)[..2],
            [
                "instance h".to_owned(),
                "space Friends Badge { unread: true, mentions: 2 }".to_owned()
            ]
        );

        collapsed.toggle(&Node::Instance("h".into())).unwrap();
        let list = rows(&store(), &connected(), &collapsed, None);
        assert_eq!(names(&list), ["instance h"]);
    }

    #[test]
    fn instances_without_a_store_are_one_row() {
        let rows = rows(
            &Store::default(),
            &[("other".into(), ConnStatus::LoggedOut)],
            &Collapsed::default(),
            None,
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
