//! Presences (PLAN.md section 11.4): a user is online while at least one gateway session is
//! connected, with the status they last asked for. `invisible` looks offline to others.

use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{Presence, PresenceStatus, Status};

use super::bus::Audience;
use crate::state::{AppState, Cache};

/// A connection was attached to a session of `user_id`.
pub fn connected(state: &AppState, user_id: Id) {
    update(state, user_id, |cache| {
        cache
            .presences
            .entry(user_id)
            .or_insert(PresenceStatus::Online);
    });
}

/// A connection of `user_id` ended; offline once no session is connected anymore.
pub fn disconnected(state: &AppState, user_id: Id) {
    if state.gateway.is_connected(user_id) {
        return;
    }
    update(state, user_id, |cache| {
        cache.presences.remove(&user_id);
    });
}

/// The `presence_update` op.
pub fn set(state: &AppState, user_id: Id, status: PresenceStatus) {
    update(state, user_id, |cache| {
        cache.presences.insert(user_id, status);
    });
}

/// Applies `change` and broadcasts the presence if what others see changed.
fn update(state: &AppState, user_id: Id, change: impl FnOnce(&mut Cache)) {
    let changed = {
        let mut cache = state.cache_mut();
        let before = cache.presence(user_id);
        change(&mut cache);
        let after = cache.presence(user_id);
        (before != after).then(|| (after, cache.presence_audience(user_id)))
    };
    if let Some((presence, users)) = changed {
        state.publish(
            ServerEvent::PresenceUpdate(presence),
            Audience::Users(users.into_iter().collect()),
        );
    }
}

/// After `user_id` joined a space: they and its members learn each other's presences.
pub fn joined_space(state: &AppState, space_id: Id, user_id: Id) {
    let (own, others): (Presence, Vec<Presence>) = {
        let cache = state.cache();
        let Some(space) = cache.spaces.get(&space_id) else {
            return;
        };
        let others = space
            .members
            .keys()
            .filter(|&&id| id != user_id)
            .map(|&id| cache.presence(id))
            .filter(|p| p.status != Status::Offline)
            .collect();
        (cache.presence(user_id), others)
    };
    if own.status != Status::Offline {
        state.publish_except(
            ServerEvent::PresenceUpdate(own),
            Audience::space(space_id),
            user_id,
        );
    }
    for presence in others {
        state.publish(
            ServerEvent::PresenceUpdate(presence),
            Audience::User(user_id),
        );
    }
}
