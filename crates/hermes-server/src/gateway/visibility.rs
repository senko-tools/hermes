//! Channels appearing and disappearing after permission changes (PLAN.md section 11.5): take a
//! [`Visible`] snapshot before the change, then call [`publish_changes`] after the cache is
//! updated.

use std::collections::{HashMap, HashSet};

use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;

use super::bus::Audience;
use crate::state::{AppState, Cache};

/// Which channels of one space each member can see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visible(HashMap<Id, HashSet<Id>>);

impl Visible {
    pub fn of_space(cache: &Cache, space_id: Id) -> Self {
        let Some(space) = cache.spaces.get(&space_id) else {
            return Visible::default();
        };
        let channel_ids: Vec<Id> = cache.space_channels(space_id).map(|c| c.id).collect();
        Visible(
            space
                .members
                .keys()
                .map(|&user_id| {
                    let seen = channel_ids
                        .iter()
                        .copied()
                        .filter(|&c| {
                            cache
                                .channel_permissions(c, user_id)
                                .contains(Permissions::VIEW_CHANNEL)
                        })
                        .collect();
                    (user_id, seen)
                })
                .collect(),
        )
    }

    /// Members who can see the channel.
    pub fn viewers(&self, channel_id: Id) -> HashSet<Id> {
        self.0
            .iter()
            .filter(|(_, seen)| seen.contains(&channel_id))
            .map(|(&user_id, _)| user_id)
            .collect()
    }
}

/// Sends `channel_create` / `channel_delete` to members whose view of the space changed since
/// `before`, and returns the view now. Members who joined or left in between are skipped; their
/// `space_create` / `space_delete` covers it.
pub fn publish_changes(state: &AppState, space_id: Id, before: &Visible) -> Visible {
    let (after, created, deleted) = {
        let cache = state.cache();
        let after = Visible::of_space(&cache, space_id);
        let mut created = Vec::new();
        let mut deleted = Vec::new();
        for (&user_id, now) in &after.0 {
            let Some(old) = before.0.get(&user_id) else {
                continue;
            };
            for channel_id in now.difference(old) {
                if let Some(c) = cache.channels.get(channel_id) {
                    created.push((user_id, c.channel.clone()));
                }
            }
            for &channel_id in old.difference(now) {
                deleted.push((user_id, channel_id));
            }
        }
        (after, created, deleted)
    };
    for (user_id, channel) in created {
        state.publish(ServerEvent::ChannelCreate(channel), Audience::User(user_id));
    }
    for (user_id, id) in deleted {
        state.publish(
            ServerEvent::ChannelDelete { id, space_id },
            Audience::User(user_id),
        );
    }
    after
}

/// Members who could see `channel_id` before and still can. They get `channel_update`; the
/// others got `channel_create` / `channel_delete` from [`publish_changes`].
pub fn kept_viewers(before: &Visible, after: &Visible, channel_id: Id) -> Audience {
    Audience::Users(
        before
            .viewers(channel_id)
            .intersection(&after.viewers(channel_id))
            .copied()
            .collect(),
    )
}
