//! Voice (PLAN.md section 12): join tokens, the room registry, forwarding rules and voice states.
//!
//! The registry is one `std` mutex with short critical sections. Lock order: the registry first,
//! then the cache. Never take the registry while holding the cache lock. Gateway events about
//! voice are published while the registry is held, so they go out in the order of the changes.

pub mod calls;
pub mod endpoint;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use hermes_proto::gateway::{ServerEvent, VoiceStateUpdate};
use hermes_proto::id::Id;
use hermes_proto::models::VoiceState;
use hermes_proto::permissions::Permissions;
use hermes_proto::voice::{self, AudioHeader, KickReason, Participant, ServerControl};
use tokio::sync::mpsc;

use crate::gateway::bus::Audience;
use crate::state::{AppState, Cache};
use crate::time::now_ms;

pub use calls::After;

/// How often a speaker's `SPEAK` permission is re-checked while they send audio.
const SPEAK_RECHECK: Duration = Duration::from_secs(1);
/// Rings older than this are forgotten (clients stop ringing after 30 s on their own).
const RING_LIFETIME: Duration = Duration::from_secs(60);

pub type ConnId = u64;

/// Where clients connect: the public UDP port and the certificate fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointInfo {
    /// `"3F:A9:…"`.
    pub cert_sha256: String,
    pub port: u16,
}

/// What a join token allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub user_id: Id,
    pub channel_id: Id,
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Ignore `user_limit` (`MOVE_MEMBERS`, or moved by a moderator).
    pub bypass_limit: bool,
    expires: Instant,
}

/// A connection's way back to its client.
#[derive(Debug, Clone)]
pub struct Link {
    pub control: mpsc::UnboundedSender<ServerControl>,
    pub conn: quinn::Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinError {
    Full,
    NoAccess,
    /// The connection is authenticated as someone else.
    OtherUser,
}

#[derive(Debug)]
struct Conn {
    link: Link,
    user_id: Option<Id>,
    /// `(channel_id, participant_id)`.
    room: Option<(Id, u16)>,
}

#[derive(Debug)]
struct Member {
    conn_id: ConnId,
    state: VoiceState,
    can_speak: bool,
    checked: Instant,
}

impl Member {
    fn deaf(&self) -> bool {
        self.state.self_deaf || self.state.server_deaf
    }
}

#[derive(Debug)]
struct Room {
    bitrate: u32,
    private: bool,
    members: BTreeMap<u16, Member>,
    started_by: Id,
    started_at: i64,
}

#[derive(Debug)]
struct Ring {
    caller: Id,
    users: HashSet<Id>,
    started: Instant,
}

#[derive(Debug, Default)]
struct Registry {
    conns: HashMap<ConnId, Conn>,
    rooms: HashMap<Id, Room>,
    /// The connection holding each user's room membership.
    users: HashMap<Id, ConnId>,
    rings: HashMap<Id, Ring>,
}

/// Voice state of the server, in [`AppState::voice`].
#[derive(Debug)]
pub struct Voice {
    enabled: bool,
    info: OnceLock<EndpointInfo>,
    registry: Mutex<Registry>,
    tokens: Mutex<HashMap<String, Ticket>>,
    next_conn: AtomicU64,
    token_lifetime: Duration,
    after: mpsc::UnboundedSender<After>,
    after_rx: Mutex<Option<mpsc::UnboundedReceiver<After>>>,
}

impl Voice {
    pub fn new(enabled: bool) -> Self {
        let (after, after_rx) = mpsc::unbounded_channel();
        Voice {
            enabled,
            info: OnceLock::new(),
            registry: Mutex::default(),
            tokens: Mutex::default(),
            next_conn: AtomicU64::new(1),
            token_lifetime: Duration::from_secs(voice::TOKEN_LIFETIME_SECS),
            after,
            after_rx: Mutex::new(Some(after_rx)),
        }
    }

    /// Called once the QUIC endpoint is bound.
    pub fn set_endpoint(&self, info: EndpointInfo) {
        let _ = self.info.set(info);
    }

    /// `None` while voice is disabled or the endpoint is not running.
    pub fn endpoint(&self) -> Option<&EndpointInfo> {
        self.info.get().filter(|_| self.enabled)
    }

    /// Shortens the token lifetime (tests).
    pub fn set_token_lifetime(&mut self, lifetime: Duration) {
        self.token_lifetime = lifetime;
    }

    /// The receiver of follow-up work ([`calls::task`]); `None` after the first call.
    pub fn take_after(&self) -> Option<mpsc::UnboundedReceiver<After>> {
        self.after_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tokens(&self) -> MutexGuard<'_, HashMap<String, Ticket>> {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner())
    }

    // -----------------------------------------------------------------------------------------
    // Tokens
    // -----------------------------------------------------------------------------------------

    /// A single-use token valid for [`voice::TOKEN_LIFETIME_SECS`] (unless shortened for tests).
    pub fn issue(
        &self,
        user_id: Id,
        channel_id: Id,
        self_mute: bool,
        self_deaf: bool,
        bypass_limit: bool,
    ) -> String {
        let token = crate::auth::new_token();
        let now = Instant::now();
        let mut tokens = self.tokens();
        tokens.retain(|_, t| t.expires > now);
        tokens.insert(
            token.clone(),
            Ticket {
                user_id,
                channel_id,
                self_mute,
                self_deaf,
                bypass_limit,
                expires: now + self.token_lifetime,
            },
        );
        token
    }

    /// Takes a token (it cannot be used again). With `user_id`, it must belong to that user.
    pub fn redeem(&self, token: &str, user_id: Option<Id>) -> Option<Ticket> {
        let ticket = self.tokens().remove(token)?;
        let valid = ticket.expires > Instant::now() && user_id.is_none_or(|u| u == ticket.user_id);
        valid.then_some(ticket)
    }

    // -----------------------------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------------------------

    /// How many users other than `except` are in the room of `channel_id`.
    pub fn room_size(&self, channel_id: Id, except: Id) -> usize {
        self.registry().rooms.get(&channel_id).map_or(0, |room| {
            room.members
                .values()
                .filter(|m| m.state.user_id != except)
                .count()
        })
    }

    /// The user of an authenticated connection.
    pub fn conn_user(&self, conn_id: ConnId) -> Option<Id> {
        self.registry().conns.get(&conn_id)?.user_id
    }

    // -----------------------------------------------------------------------------------------
    // Connections
    // -----------------------------------------------------------------------------------------

    pub fn register(&self, link: Link) -> ConnId {
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        self.registry().conns.insert(
            conn_id,
            Conn {
                link,
                user_id: None,
                room: None,
            },
        );
        conn_id
    }

    /// The connection closed: leave its room.
    pub fn unregister(&self, state: &AppState, conn_id: ConnId) {
        let mut reg = self.registry();
        reg.remove_member(state, &self.after, conn_id, None);
        reg.conns.remove(&conn_id);
    }

    /// Puts the connection's user into the ticket's room, leaving any room they were in (on this
    /// connection: a switch; on another: that one gets `kicked {joined_elsewhere}`).
    pub fn join(&self, state: &AppState, conn_id: ConnId, ticket: Ticket) -> Result<(), JoinError> {
        let user_id = ticket.user_id;
        let channel_id = ticket.channel_id;
        let mut reg = self.registry();
        let (bitrate, private, limit) = {
            let cache = state.cache();
            let perms = cache.channel_permissions(channel_id, user_id);
            let channel = cache.channels.get(&channel_id).map(|c| &c.channel);
            let Some(channel) = channel.filter(|c| c.kind.has_voice()) else {
                return Err(JoinError::NoAccess);
            };
            if !perms.contains(Permissions::VIEW_CHANNEL | Permissions::CONNECT) {
                return Err(JoinError::NoAccess);
            }
            let config = &state.config.voice;
            let bitrate = channel
                .bitrate
                .unwrap_or(config.default_bitrate)
                .min(config.max_bitrate);
            let limit = channel
                .user_limit
                .filter(|&l| l > 0 && !ticket.bypass_limit);
            (bitrate, channel.kind.is_private(), limit)
        };
        match reg.conns.get(&conn_id).map(|c| c.user_id) {
            None => return Err(JoinError::NoAccess),
            Some(Some(other)) if other != user_id => return Err(JoinError::OtherUser),
            Some(_) => {}
        }
        let others = reg.rooms.get(&channel_id).map_or(0, |room| {
            room.members
                .values()
                .filter(|m| m.state.user_id != user_id)
                .count()
        });
        let max = state.config.voice.max_users_per_room as usize;
        if others >= max || limit.is_some_and(|l| others >= l as usize) {
            return Err(JoinError::Full);
        }

        if let Some(&old) = reg.users.get(&user_id) {
            let reason = (old != conn_id).then_some(KickReason::JoinedElsewhere);
            reg.remove_member(state, &self.after, old, reason);
        }

        let now = now_ms();
        let Registry {
            conns,
            rooms,
            users,
            rings,
        } = &mut *reg;
        let room = rooms.entry(channel_id).or_insert_with(|| Room {
            bitrate,
            private,
            members: BTreeMap::new(),
            started_by: user_id,
            started_at: now,
        });
        let new_room = room.members.is_empty();
        let participant_id = (1..=u16::MAX)
            .find(|id| !room.members.contains_key(id))
            .ok_or(JoinError::Full)?;
        let participants = room
            .members
            .iter()
            .map(|(&participant_id, m)| Participant {
                participant_id,
                user_id: m.state.user_id,
            })
            .collect();
        for member in room.members.values() {
            if let Some(conn) = conns.get(&member.conn_id) {
                let _ = conn
                    .link
                    .control
                    .send(ServerControl::ParticipantJoined(Participant {
                        participant_id,
                        user_id,
                    }));
            }
        }
        let state_now = VoiceState {
            user_id,
            channel_id,
            self_mute: ticket.self_mute,
            self_deaf: ticket.self_deaf,
            server_mute: false,
            server_deaf: false,
            joined_at: now,
        };
        room.members.insert(
            participant_id,
            Member {
                conn_id,
                state: state_now,
                can_speak: true,
                checked: Instant::now() - SPEAK_RECHECK,
            },
        );
        let conn = conns.get_mut(&conn_id).expect("checked above");
        conn.user_id = Some(user_id);
        conn.room = Some((channel_id, participant_id));
        users.insert(user_id, conn_id);
        let _ = conn.link.control.send(ServerControl::Ready {
            participant_id,
            room_id: channel_id,
            bitrate: room.bitrate,
            participants,
        });

        state.cache_mut().voice_states.insert(user_id, state_now);
        state.publish(
            ServerEvent::VoiceStateUpdate(VoiceStateUpdate::Joined(state_now)),
            Audience::Channel(channel_id),
        );
        if let Some(ring) = rings.get_mut(&channel_id)
            && ring.users.remove(&user_id)
        {
            state.publish(
                ServerEvent::CallRingStop { channel_id },
                Audience::User(user_id),
            );
        }
        if new_room && private {
            let _ = self.after.send(After::CallStarted {
                channel_id,
                user_id,
            });
        }
        Ok(())
    }

    /// The connection's user leaves their room (the `leave` op); the connection stays open.
    pub fn leave_conn(&self, state: &AppState, conn_id: ConnId) {
        self.registry()
            .remove_member(state, &self.after, conn_id, None);
    }

    /// Takes `user_id` out of voice, telling their connection why. Returns whether they were in.
    pub fn disconnect(&self, state: &AppState, user_id: Id, reason: KickReason) -> bool {
        let mut reg = self.registry();
        let Some(&conn_id) = reg.users.get(&user_id) else {
            return false;
        };
        reg.remove_member(state, &self.after, conn_id, Some(reason));
        true
    }

    /// The channel `user_id` is in, if any.
    pub fn channel_of(&self, user_id: Id) -> Option<Id> {
        let reg = self.registry();
        let conn = reg.conns.get(reg.users.get(&user_id)?)?;
        conn.room.map(|(channel_id, _)| channel_id)
    }

    // -----------------------------------------------------------------------------------------
    // State changes
    // -----------------------------------------------------------------------------------------

    /// Self mute/deafen; nothing is published when nothing changes. Returns whether the user is
    /// in voice.
    pub fn set_self_state(
        &self,
        state: &AppState,
        user_id: Id,
        self_mute: Option<bool>,
        self_deaf: Option<bool>,
    ) -> bool {
        self.update_member(state, user_id, |s| {
            s.self_mute = self_mute.unwrap_or(s.self_mute);
            s.self_deaf = self_deaf.unwrap_or(s.self_deaf);
        })
        .is_some()
    }

    /// Server mute/deafen, also sent to the user's connection as `server_state`.
    pub fn set_server_state(
        &self,
        state: &AppState,
        user_id: Id,
        server_mute: Option<bool>,
        server_deaf: Option<bool>,
    ) -> bool {
        self.update_member(state, user_id, |s| {
            s.server_mute = server_mute.unwrap_or(s.server_mute);
            s.server_deaf = server_deaf.unwrap_or(s.server_deaf);
        })
        .is_some()
    }

    /// Changes a member's voice state; publishes and (for server flags) tells the client when
    /// something changed. `None` if the user is not in voice.
    fn update_member(
        &self,
        state: &AppState,
        user_id: Id,
        change: impl FnOnce(&mut VoiceState),
    ) -> Option<()> {
        let mut reg = self.registry();
        let Registry {
            conns,
            rooms,
            users,
            ..
        } = &mut *reg;
        let conn_id = *users.get(&user_id)?;
        let conn = conns.get(&conn_id)?;
        let (channel_id, participant_id) = conn.room?;
        let member = rooms
            .get_mut(&channel_id)?
            .members
            .get_mut(&participant_id)?;
        let before = member.state;
        change(&mut member.state);
        let after = member.state;
        if after == before {
            return Some(());
        }
        if (after.server_mute, after.server_deaf) != (before.server_mute, before.server_deaf) {
            let _ = conn.link.control.send(ServerControl::ServerState {
                server_mute: after.server_mute,
                server_deaf: after.server_deaf,
            });
        }
        state.cache_mut().voice_states.insert(user_id, after);
        state.publish(
            ServerEvent::VoiceStateUpdate(VoiceStateUpdate::Joined(after)),
            Audience::Channel(channel_id),
        );
        Some(())
    }

    /// Sends `moved {token}` to a user in voice. Returns whether they were in voice.
    pub fn send_moved(&self, user_id: Id, token: String) -> bool {
        let reg = self.registry();
        let Some(conn) = reg.users.get(&user_id).and_then(|c| reg.conns.get(c)) else {
            return false;
        };
        conn.link
            .control
            .send(ServerControl::Moved { token })
            .is_ok()
    }

    /// A voice channel's bitrate changed: tell everyone in its room.
    pub fn bitrate_changed(&self, channel_id: Id, bitrate: u32) {
        let mut reg = self.registry();
        let Registry { conns, rooms, .. } = &mut *reg;
        let Some(room) = rooms.get_mut(&channel_id) else {
            return;
        };
        if room.bitrate == bitrate {
            return;
        }
        room.bitrate = bitrate;
        for member in room.members.values() {
            if let Some(conn) = conns.get(&member.conn_id) {
                let _ = conn
                    .link
                    .control
                    .send(ServerControl::BitrateChanged { bitrate });
            }
        }
    }

    /// Removes everyone who can no longer see their room's channel (deleted, left or removed
    /// from the space or group DM, lost `VIEW_CHANNEL`). Call after such changes are cached.
    pub fn revalidate(&self, state: &AppState) {
        let mut reg = self.registry();
        let kicks: Vec<(ConnId, KickReason)> = {
            let cache = state.cache();
            reg.rooms
                .iter()
                .flat_map(|(&channel_id, room)| {
                    let cache = &cache;
                    room.members.values().filter_map(move |m| {
                        kick_reason(cache, channel_id, m.state.user_id).map(|r| (m.conn_id, r))
                    })
                })
                .collect()
        };
        for (conn_id, reason) in kicks {
            reg.remove_member(state, &self.after, conn_id, Some(reason));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------------------------------

    /// Rings `targets` (recipients of the DM `channel_id`, already filtered) who are not in the
    /// call. Returns who was rung.
    pub fn ring(&self, state: &AppState, channel_id: Id, caller: Id, targets: &[Id]) -> Vec<Id> {
        let mut reg = self.registry();
        reg.rings.retain(|_, r| r.started.elapsed() < RING_LIFETIME);
        let in_call: HashSet<Id> = reg.rooms.get(&channel_id).map_or_else(HashSet::new, |r| {
            r.members.values().map(|m| m.state.user_id).collect()
        });
        let mut rung: Vec<Id> = targets
            .iter()
            .copied()
            .filter(|u| *u != caller && !in_call.contains(u))
            .collect();
        rung.sort();
        rung.dedup();
        if rung.is_empty() {
            return rung;
        }
        let ring = reg.rings.entry(channel_id).or_insert_with(|| Ring {
            caller,
            users: HashSet::new(),
            started: Instant::now(),
        });
        ring.caller = caller;
        ring.started = Instant::now();
        ring.users.extend(rung.iter().copied());
        state.publish(
            ServerEvent::CallRing {
                channel_id,
                caller_id: caller,
            },
            Audience::Users(rung.clone()),
        );
        rung
    }

    // -----------------------------------------------------------------------------------------
    // Audio
    // -----------------------------------------------------------------------------------------

    /// Forwards one datagram (PLAN.md 12.4 "Server forwarding rule"): dropped if the sender is
    /// muted (self or server), lacks `SPEAK`, or the packet is malformed or too large; otherwise
    /// sent with the sender's participant ID to everyone else in the room who is not deafened.
    pub fn forward(&self, state: &AppState, conn_id: ConnId, data: Bytes) {
        if AudioHeader::decode(&data).is_err() {
            return;
        }
        let mut reg = self.registry();
        let Registry { conns, rooms, .. } = &mut *reg;
        let Some((channel_id, participant_id)) = conns.get(&conn_id).and_then(|c| c.room) else {
            return;
        };
        let Some(room) = rooms.get_mut(&channel_id) else {
            return;
        };
        let Some(sender) = room.members.get_mut(&participant_id) else {
            return;
        };
        if sender.state.self_mute || sender.state.server_mute {
            return;
        }
        if sender.checked.elapsed() >= SPEAK_RECHECK {
            sender.can_speak = state
                .cache()
                .channel_permissions(channel_id, sender.state.user_id)
                .contains(Permissions::SPEAK);
            sender.checked = Instant::now();
        }
        if !sender.can_speak {
            return;
        }
        let mut packet = data.to_vec();
        voice::set_sender_id(&mut packet, participant_id);
        let packet = Bytes::from(packet);
        for (&id, member) in &room.members {
            if id == participant_id || member.deaf() {
                continue;
            }
            if let Some(conn) = conns.get(&member.conn_id) {
                // Never blocks; a full or closed peer just loses the packet.
                let _ = conn.link.conn.send_datagram(packet.clone());
            }
        }
    }
}

/// Why `user_id` must leave the room of `channel_id`, if they must.
fn kick_reason(cache: &Cache, channel_id: Id, user_id: Id) -> Option<KickReason> {
    if !cache.channels.contains_key(&channel_id) {
        return Some(KickReason::ChannelDeleted);
    }
    let perms = cache.channel_permissions(channel_id, user_id);
    (!perms.contains(Permissions::VIEW_CHANNEL)).then_some(KickReason::Removed)
}

impl Registry {
    /// Takes the connection's user out of their room: `participant_left` to the others, the
    /// voice state removed and published, and the call ended when the room empties.
    fn remove_member(
        &mut self,
        state: &AppState,
        after: &mpsc::UnboundedSender<After>,
        conn_id: ConnId,
        reason: Option<KickReason>,
    ) {
        let Some(conn) = self.conns.get_mut(&conn_id) else {
            return;
        };
        let Some((channel_id, participant_id)) = conn.room.take() else {
            return;
        };
        if let Some(reason) = reason {
            let _ = conn.link.control.send(ServerControl::Kicked { reason });
        }
        let Some(user_id) = conn.user_id else {
            return;
        };
        if self.users.get(&user_id) == Some(&conn_id) {
            self.users.remove(&user_id);
        }
        let Some(room) = self.rooms.get_mut(&channel_id) else {
            return;
        };
        room.members.remove(&participant_id);
        for member in room.members.values() {
            if let Some(other) = self.conns.get(&member.conn_id) {
                let _ = other
                    .link
                    .control
                    .send(ServerControl::ParticipantLeft { participant_id });
            }
        }
        state.cache_mut().voice_states.remove(&user_id);
        state.publish(
            ServerEvent::VoiceStateUpdate(VoiceStateUpdate::left(user_id)),
            Audience::Channel(channel_id),
        );

        let empty = room.members.is_empty();
        let ring_over = self
            .rings
            .get(&channel_id)
            .is_some_and(|r| empty || r.caller == user_id);
        if ring_over && let Some(ring) = self.rings.remove(&channel_id) {
            state.publish(
                ServerEvent::CallRingStop { channel_id },
                Audience::Users(ring.users.into_iter().collect()),
            );
        }
        if empty
            && let Some(room) = self.rooms.remove(&channel_id)
            && room.private
        {
            let _ = after.send(After::CallEnded {
                channel_id,
                user_id: room.started_by,
                started_at: room.started_at,
            });
        }
    }
}
