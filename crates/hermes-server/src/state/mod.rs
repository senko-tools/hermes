//! In-memory state cache (PLAN.md sections 4 and 9.4).
//!
//! All spaces, channels, roles, members, overrides, (lightweight) users, presences and voice
//! states live here, so permission checks never hit the database. Messages are not cached.
//! Lock rule: never hold the cache lock across an `.await`.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use axum::http::HeaderMap;
use hermes_proto::gateway::{ServerEvent, SpaceSnapshot};
use hermes_proto::id::Id;
use hermes_proto::models::{
    Channel, CurrentUser, Member, Presence, PresenceStatus, PrivacySettings, Role, Space,
    SpacePreview, Status, User, VoiceState,
};
use hermes_proto::permissions::{self, Permissions};

use crate::config::Config;
use crate::db::Db;
use crate::db::state::Snapshot;
use crate::files::Files;
use crate::gateway::bus::{Audience, Gateway};
use crate::messages::Slowmode;
use crate::ratelimit::KeyedLimiter;
use crate::snowflake::Snowflake;
use crate::tls::Fingerprint;

/// Everything the request handlers share.
#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub db: Db,
    pub ids: Snowflake,
    pub cache: RwLock<Cache>,
    /// Held by handlers that read the cache, write the database and then update the cache, so
    /// two such changes cannot interleave and leave the cache different from the database.
    /// Never take it while holding the cache lock.
    pub writes: tokio::sync::Mutex<()>,
    pub slowmode: Slowmode,
    /// `limits.messages_per_5s`, keyed by `(channel_id, user_id)`.
    pub message_limiter: KeyedLimiter<(Id, Id)>,
    /// Uploaded files on disk.
    pub files: Files,
    /// Gateway sessions and the EventBus.
    pub gateway: Gateway,
    /// Voice rooms, join tokens and the QUIC endpoint's address.
    pub voice: crate::voice::Voice,
    /// Fingerprint of the self-signed HTTPS certificate, for invite links. Set by the caller of
    /// [`load`](Self::load) once TLS is set up.
    pub fingerprint: Option<Fingerprint>,
}

impl AppState {
    /// Opens the database in `storage.data_dir`, applies migrations and loads the cache.
    pub async fn load(config: Config) -> Result<Self, sqlx::Error> {
        let db = Db::open(&config.storage.data_dir).await?;
        let cache = load_from_db(&db).await?;
        let used = crate::db::attachments::total_size(&db.read).await?;
        let files = Files::open(&config.storage.data_dir, used)?;
        let voice = crate::voice::Voice::new(config.voice.enabled);
        Ok(Self {
            config,
            db,
            ids: Snowflake::new(),
            cache: RwLock::new(cache),
            writes: tokio::sync::Mutex::new(()),
            slowmode: Slowmode::default(),
            message_limiter: KeyedLimiter::default(),
            files,
            gateway: Gateway::default(),
            voice,
            fingerprint: None,
        })
    }

    /// Publishes a dispatch event to `audience`, resolved with the cache as it is now. Call it
    /// after the change is in the cache, and never while holding the cache lock.
    pub fn publish(&self, event: ServerEvent, audience: Audience) {
        self.publish_inner(&event, &audience, None);
    }

    /// [`publish`](Self::publish) to everyone in `audience` except `user_id`.
    pub fn publish_except(&self, event: ServerEvent, audience: Audience, user_id: Id) {
        self.publish_inner(&event, &audience, Some(user_id));
    }

    fn publish_inner(&self, event: &ServerEvent, audience: &Audience, except: Option<Id>) {
        let users = audience.resolve(&self.cache());
        self.gateway.send(&users, except, event);
    }

    /// Read access to the cache. A poisoned lock is still used: every write is a short,
    /// self-contained update, so the data stays consistent.
    pub fn cache(&self) -> RwLockReadGuard<'_, Cache> {
        self.cache.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn cache_mut(&self) -> RwLockWriteGuard<'_, Cache> {
        self.cache.write().unwrap_or_else(|e| e.into_inner())
    }

    /// The client's IP; see [`client_ip`].
    pub fn client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        client_ip(&self.config.network.trusted_proxies, peer, headers)
    }
}

/// The client's IP: the TCP peer, or, when the peer is one of `trusted` (proxies), the last
/// `X-Forwarded-For` entry that is not a trusted proxy. IPv4-mapped IPv6 addresses
/// (`::ffff:a.b.c.d`, seen on dual-stack sockets) are converted to plain IPv4.
pub fn client_ip(trusted: &[IpAddr], peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let is_trusted = |ip: &IpAddr| trusted.iter().any(|t| t.to_canonical() == *ip);
    let mut ip = peer.ip().to_canonical();
    if !is_trusted(&ip) {
        return ip;
    }
    let forwarded = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .collect::<Vec<_>>();
    for entry in forwarded.iter().rev() {
        let Ok(hop) = entry.trim().parse::<IpAddr>() else {
            break;
        };
        ip = hop.to_canonical();
        if !is_trusted(&ip) {
            break;
        }
    }
    ip
}

pub async fn load_from_db(db: &Db) -> Result<Cache, sqlx::Error> {
    Ok(Cache::from_snapshot(
        crate::db::state::load(&db.read).await?,
    ))
}

/// Members included in a [`SpaceSnapshot`]; larger spaces lazy-load the rest via REST.
pub const SNAPSHOT_MEMBER_CAP: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedUser {
    pub user: User,
    pub instance_admin: bool,
    pub disabled: bool,
    pub privacy: PrivacySettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceState {
    pub space: Space,
    /// Sorted by position, then ID; includes `@everyone`.
    pub roles: Vec<Role>,
    pub members: HashMap<Id, MemberState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberState {
    pub nickname: Option<String>,
    /// Explicitly assigned roles, sorted; `@everyone` is implicit.
    pub role_ids: Vec<Id>,
    pub joined_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelState {
    /// `overrides` is filled; `recipients` is always empty because users are cached once, in
    /// [`Cache::users`]. Use `recipient_ids`.
    pub channel: Channel,
    /// DM and group DM recipients, sorted.
    pub recipient_ids: Vec<Id>,
    /// Recipients who closed the DM, sorted: it is not listed for them until it reopens.
    pub closed: Vec<Id>,
    /// Recipients for whom the DM is a message request they have not accepted, sorted.
    pub requests: Vec<Id>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cache {
    pub users: HashMap<Id, CachedUser>,
    pub spaces: HashMap<Id, SpaceState>,
    pub channels: HashMap<Id, ChannelState>,
    /// Runtime only, keyed by user. Users without an entry are offline.
    pub presences: HashMap<Id, PresenceStatus>,
    /// Runtime only, keyed by user (one voice room per user).
    pub voice_states: HashMap<Id, VoiceState>,
}

impl CachedUser {
    pub fn current_user(&self) -> CurrentUser {
        CurrentUser {
            user: self.user.clone(),
            instance_admin: self.instance_admin,
        }
    }
}

impl SpaceState {
    /// A new space's state: `@everyone` must be in `roles`; members are added separately.
    pub fn new(space: Space, mut roles: Vec<Role>) -> Self {
        roles.sort_by_key(|r| (r.position, r.id));
        SpaceState {
            space,
            roles,
            members: HashMap::new(),
        }
    }

    pub fn role(&self, role_id: Id) -> Option<&Role> {
        self.roles.iter().find(|r| r.id == role_id)
    }

    /// Restores the sort order of `roles` after positions changed.
    pub fn sort_roles(&mut self) {
        self.roles.sort_by_key(|r| (r.position, r.id));
    }
}

impl Cache {
    /// Builds the cache from loaded rows. Rows that point at missing parents are skipped
    /// (foreign keys make that impossible in a healthy database).
    pub fn from_snapshot(s: Snapshot) -> Self {
        let mut cache = Cache {
            users: s
                .users
                .into_iter()
                .map(|r| {
                    let user = CachedUser {
                        user: r.user,
                        instance_admin: r.instance_admin,
                        disabled: r.disabled,
                        privacy: r.privacy,
                    };
                    (user.user.id, user)
                })
                .collect(),
            ..Cache::default()
        };

        for space in s.spaces {
            let state = SpaceState {
                space,
                roles: Vec::new(),
                members: HashMap::new(),
            };
            cache.spaces.insert(state.space.id, state);
        }
        for role in s.roles {
            if let Some(space) = cache.spaces.get_mut(&role.space_id) {
                space.roles.push(role);
            }
        }
        for m in s.members {
            if let Some(space) = cache.spaces.get_mut(&m.space_id) {
                let member = MemberState {
                    nickname: m.nickname,
                    role_ids: Vec::new(),
                    joined_at: m.joined_at,
                };
                space.members.insert(m.user_id, member);
            }
        }
        for mr in s.member_roles {
            if let Some(member) = cache
                .spaces
                .get_mut(&mr.space_id)
                .and_then(|space| space.members.get_mut(&mr.user_id))
            {
                member.role_ids.push(mr.role_id);
            }
        }
        for space in cache.spaces.values_mut() {
            space.roles.sort_by_key(|r| (r.position, r.id));
            for member in space.members.values_mut() {
                member.role_ids.sort();
            }
        }

        for channel in s.channels {
            let state = ChannelState {
                channel,
                recipient_ids: Vec::new(),
                closed: Vec::new(),
                requests: Vec::new(),
            };
            cache.channels.insert(state.channel.id, state);
        }
        for (channel_id, o) in s.overrides {
            if let Some(state) = cache.channels.get_mut(&channel_id) {
                state.channel.overrides.push(o);
            }
        }
        for r in s.dm_recipients {
            if let Some(state) = cache.channels.get_mut(&r.channel_id) {
                state.recipient_ids.push(r.user_id);
                if r.closed {
                    state.closed.push(r.user_id);
                }
                if r.request {
                    state.requests.push(r.user_id);
                }
            }
        }
        for state in cache.channels.values_mut() {
            state.channel.overrides.sort_by_key(|o| o.role_id);
            state.recipient_ids.sort();
            state.closed.sort();
            state.requests.sort();
        }

        cache
    }

    pub fn insert_user(&mut self, user: CachedUser) {
        self.users.insert(user.user.id, user);
    }

    pub fn insert_space(&mut self, space: SpaceState) {
        self.spaces.insert(space.space.id, space);
    }

    /// Adds a space or DM channel. `channel.recipients` is ignored (see [`ChannelState`]).
    pub fn insert_channel(&mut self, mut channel: Channel, mut recipient_ids: Vec<Id>) {
        channel.recipients.clear();
        channel.overrides.sort_by_key(|o| o.role_id);
        recipient_ids.sort();
        self.channels.insert(
            channel.id,
            ChannelState {
                channel,
                recipient_ids,
                closed: Vec::new(),
                requests: Vec::new(),
            },
        );
    }

    /// Adds a member without roles. Does nothing if the space is not cached.
    pub fn insert_member(&mut self, space_id: Id, user_id: Id, joined_at: i64) {
        if let Some(space) = self.spaces.get_mut(&space_id) {
            space.members.insert(
                user_id,
                MemberState {
                    nickname: None,
                    role_ids: Vec::new(),
                    joined_at,
                },
            );
        }
    }

    /// Removes a space and its channels.
    pub fn remove_space(&mut self, space_id: Id) {
        self.spaces.remove(&space_id);
        self.channels
            .retain(|_, c| c.channel.space_id != Some(space_id));
    }

    pub fn remove_member(&mut self, space_id: Id, user_id: Id) {
        if let Some(space) = self.spaces.get_mut(&space_id) {
            space.members.remove(&user_id);
        }
    }

    /// The channels of a space, in no particular order.
    pub fn space_channels(&self, space_id: Id) -> impl Iterator<Item = &Channel> {
        self.channels
            .values()
            .map(|c| &c.channel)
            .filter(move |c| c.space_id == Some(space_id))
    }

    pub fn is_member(&self, space_id: Id, user_id: Id) -> bool {
        self.spaces
            .get(&space_id)
            .is_some_and(|s| s.members.contains_key(&user_id))
    }

    pub fn space_preview(&self, space_id: Id) -> Option<SpacePreview> {
        let state = self.spaces.get(&space_id)?;
        Some(SpacePreview {
            id: space_id,
            name: state.space.name.clone(),
            icon: state.space.icon.clone(),
            member_count: u32::try_from(state.members.len()).unwrap_or(u32::MAX),
        })
    }

    /// Everything `user_id` sees of a space, as in `ready` (PLAN.md section 11.3): channels they
    /// can view, sorted by position; at most [`SNAPSHOT_MEMBER_CAP`] members, oldest first.
    pub fn space_snapshot(&self, space_id: Id, user_id: Id) -> Option<SpaceSnapshot> {
        let state = self.spaces.get(&space_id)?;
        let mut channels: Vec<Channel> = self
            .channels
            .values()
            .filter(|c| c.channel.space_id == Some(space_id))
            .filter(|c| {
                self.channel_permissions(c.channel.id, user_id)
                    .contains(Permissions::VIEW_CHANNEL)
            })
            .map(|c| c.channel.clone())
            .collect();
        channels.sort_by_key(|c| (c.position, c.id));
        let mut member_ids: Vec<(i64, Id)> = state
            .members
            .iter()
            .map(|(id, m)| (m.joined_at, *id))
            .collect();
        member_ids.sort();
        let members = member_ids
            .into_iter()
            .take(SNAPSHOT_MEMBER_CAP)
            .filter_map(|(_, id)| self.member(space_id, id))
            .collect();
        let voice_states = self
            .voice_states
            .values()
            .filter(|v| {
                self.channels
                    .get(&v.channel_id)
                    .is_some_and(|c| c.channel.space_id == Some(space_id))
            })
            .copied()
            .collect();
        Some(SpaceSnapshot {
            space: state.space.clone(),
            roles: state.roles.clone(),
            channels,
            members,
            voice_states,
        })
    }

    /// The wire `Member` of `user_id` in `space_id`, if they are a member.
    pub fn member(&self, space_id: Id, user_id: Id) -> Option<Member> {
        let member = self.spaces.get(&space_id)?.members.get(&user_id)?;
        Some(Member {
            space_id,
            user: self.users.get(&user_id)?.user.clone(),
            nickname: member.nickname.clone(),
            role_ids: member.role_ids.clone(),
            joined_at: member.joined_at,
        })
    }

    /// Users who get a channel's events: DM recipients (except those who ignored it as a
    /// message request), or space members with `VIEW_CHANNEL`.
    pub fn channel_viewers(&self, channel_id: Id) -> HashSet<Id> {
        let Some(state) = self.channels.get(&channel_id) else {
            return HashSet::new();
        };
        let Some(space_id) = state.channel.space_id else {
            let ignored = |id: &Id| {
                state.closed.binary_search(id).is_ok() && state.requests.binary_search(id).is_ok()
            };
            return state
                .recipient_ids
                .iter()
                .copied()
                .filter(|id| !ignored(id))
                .collect();
        };
        let Some(space) = self.spaces.get(&space_id) else {
            return HashSet::new();
        };
        space
            .members
            .keys()
            .copied()
            .filter(|&user_id| {
                self.channel_permissions(channel_id, user_id)
                    .contains(Permissions::VIEW_CHANNEL)
            })
            .collect()
    }

    /// The wire form of a DM or group DM, with `recipients` filled in (everyone, sorted by ID).
    pub fn dm_channel(&self, channel_id: Id) -> Option<Channel> {
        let state = self
            .channels
            .get(&channel_id)
            .filter(|c| c.channel.kind.is_private())?;
        let mut channel = state.channel.clone();
        channel.recipients = state
            .recipient_ids
            .iter()
            .filter_map(|id| self.users.get(id).map(|u| u.user.clone()))
            .collect();
        Some(channel)
    }

    /// [`Cache::dm_channel`] as `viewer` sees it: `request` is set when it is a message request
    /// for them.
    pub fn dm_for(&self, channel_id: Id, viewer: Id) -> Option<Channel> {
        let mut channel = self.dm_channel(channel_id)?;
        channel.request = self.channels[&channel_id]
            .requests
            .binary_search(&viewer)
            .is_ok();
        Some(channel)
    }

    /// The DMs and group DMs `user_id` is a recipient of, in no particular order.
    pub fn dm_ids(&self, user_id: Id) -> impl Iterator<Item = Id> + '_ {
        self.channels
            .values()
            .filter(move |c| {
                c.channel.kind.is_private() && c.recipient_ids.binary_search(&user_id).is_ok()
            })
            .map(|c| c.channel.id)
    }

    /// The DMs `user_id` lists: [`Cache::dm_ids`] without the ones they closed.
    pub fn open_dm_ids(&self, user_id: Id) -> impl Iterator<Item = Id> + '_ {
        self.dm_ids(user_id)
            .filter(move |id| self.channels[id].closed.binary_search(&user_id).is_err())
    }

    /// Whether two users are members of at least one common space.
    pub fn shares_space(&self, a: Id, b: Id) -> bool {
        self.spaces
            .values()
            .any(|s| s.members.contains_key(&a) && s.members.contains_key(&b))
    }

    /// Users who see `user_id`'s presence: everyone sharing a space or a DM with them, and
    /// themselves.
    pub fn presence_audience(&self, user_id: Id) -> HashSet<Id> {
        let mut users = HashSet::from([user_id]);
        for space in self.spaces.values() {
            if space.members.contains_key(&user_id) {
                users.extend(space.members.keys().copied());
            }
        }
        // A message request shares no presence until it is accepted, in either direction.
        for dm in self.dm_ids(user_id) {
            let c = &self.channels[&dm];
            if c.requests.binary_search(&user_id).is_ok() {
                continue;
            }
            users.extend(
                c.recipient_ids
                    .iter()
                    .copied()
                    .filter(|id| c.requests.binary_search(id).is_err()),
            );
        }
        users
    }

    /// What other users see of `user_id`'s presence.
    pub fn presence(&self, user_id: Id) -> Presence {
        Presence {
            user_id,
            status: self
                .presences
                .get(&user_id)
                .map_or(Status::Offline, |p| p.visible_status()),
        }
    }

    /// A user's space-level permissions; empty if they are not a member.
    pub fn space_permissions(&self, space_id: Id, user_id: Id) -> Permissions {
        let (Some(space), Some(member)) =
            (self.spaces.get(&space_id), self.member(space_id, user_id))
        else {
            return Permissions::empty();
        };
        permissions::compute(&space.space, &space.roles, &member, None)
    }

    /// A user's permissions in a channel; empty if they cannot see it.
    pub fn channel_permissions(&self, channel_id: Id, user_id: Id) -> Permissions {
        let Some(state) = self.channels.get(&channel_id) else {
            return Permissions::empty();
        };
        let Some(space_id) = state.channel.space_id else {
            return if state.recipient_ids.binary_search(&user_id).is_ok() {
                Permissions::DM_RECIPIENT
            } else {
                Permissions::empty()
            };
        };
        let (Some(space), Some(member)) =
            (self.spaces.get(&space_id), self.member(space_id, user_id))
        else {
            return Permissions::empty();
        };
        permissions::compute(&space.space, &space.roles, &member, Some(&state.channel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;

    const OWNER: Id = Id(1);
    const ALICE: Id = Id(2);
    const BOB: Id = Id(3);
    const SPACE: Id = Id(10);
    const MOD_ROLE: Id = Id(11);
    const GENERAL: Id = Id(20);
    const SECRET: Id = Id(21);
    const VOICE: Id = Id(22);
    const DM: Id = Id(30);

    async fn fixture(db: &Db) {
        let everyone = Permissions::DEFAULT_EVERYONE.bits() as i64;
        let manage = Permissions::MANAGE_MESSAGES.bits() as i64;
        let view = Permissions::VIEW_CHANNEL.bits() as i64;
        let statements = [
            "INSERT INTO users (id, username, password_hash, instance_admin, created_at)
             VALUES (1, 'owner', 'x', 1, 0)"
                .to_owned(),
            "INSERT INTO users (id, username, display_name, password_hash, created_at)
             VALUES (2, 'alice', 'Alice', 'x', 0)"
                .to_owned(),
            "INSERT INTO attachments (id, uploader_id, filename, content_type, size, sha256, created_at)
             VALUES (5, 2, 'cat.png', 'image/png', 3, x'00', 0)"
                .to_owned(),
            "UPDATE users SET avatar_id = 5 WHERE id = 2".to_owned(),
            "INSERT INTO users (id, username, password_hash, disabled, created_at)
             VALUES (3, 'bob', 'x', 1, 0)"
                .to_owned(),
            "INSERT INTO spaces (id, name, owner_id, created_at) VALUES (10, 'Senko', 1, 0)".to_owned(),
            format!(
                "INSERT INTO roles (id, space_id, name, position, permissions)
                 VALUES (10, 10, '@everyone', 0, {everyone})"
            ),
            format!(
                "INSERT INTO roles (id, space_id, name, color, hoist, position, permissions)
                 VALUES (11, 10, 'mod', 16711680, 1, 1, {manage})"
            ),
            "INSERT INTO space_members (space_id, user_id, joined_at) VALUES (10, 1, 100)".to_owned(),
            "INSERT INTO space_members (space_id, user_id, nickname, joined_at) VALUES (10, 2, 'ali', 200)"
                .to_owned(),
            "INSERT INTO member_roles (space_id, user_id, role_id) VALUES (10, 2, 11)".to_owned(),
            "INSERT INTO channels (id, space_id, kind, name, position, created_at)
             VALUES (20, 10, 'text', 'general', 0, 0)"
                .to_owned(),
            "INSERT INTO channels (id, space_id, kind, name, position, created_at)
             VALUES (21, 10, 'text', 'secret', 1, 0)"
                .to_owned(),
            "INSERT INTO channels (id, space_id, kind, name, bitrate, user_limit, created_at)
             VALUES (22, 10, 'voice', 'General', 64000, 0, 0)"
                .to_owned(),
            format!("INSERT INTO channel_overrides (channel_id, role_id, deny) VALUES (21, 10, {view})"),
            format!("INSERT INTO channel_overrides (channel_id, role_id, allow) VALUES (21, 11, {view})"),
            "INSERT INTO channels (id, kind, created_at) VALUES (30, 'dm', 0)".to_owned(),
            "INSERT INTO dm_recipients (channel_id, user_id) VALUES (30, 3)".to_owned(),
            "INSERT INTO dm_recipients (channel_id, user_id) VALUES (30, 2)".to_owned(),
        ];
        for sql in statements {
            // Test fixtures built from constants only.
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&db.write)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn empty_database_loads_empty_cache() {
        let dir = TempDir::new("cache-empty");
        let db = Db::open(dir.path()).await.unwrap();
        assert_eq!(load_from_db(&db).await.unwrap(), Cache::default());
        db.close().await;
    }

    #[tokio::test]
    async fn loads_every_table() {
        let dir = TempDir::new("cache-load");
        let db = Db::open(dir.path()).await.unwrap();
        fixture(&db).await;
        let cache = load_from_db(&db).await.unwrap();

        assert_eq!(cache.users.len(), 3);
        assert!(cache.users[&OWNER].instance_admin);
        assert!(cache.users[&BOB].disabled);
        let alice = &cache.users[&ALICE].user;
        assert_eq!(alice.name(), "Alice");
        assert_eq!(alice.avatar.as_ref().unwrap().filename, "cat.png");

        let space = &cache.spaces[&SPACE];
        assert_eq!(space.space.name, "Senko");
        assert_eq!(space.space.owner_id, OWNER);
        let role_ids: Vec<_> = space.roles.iter().map(|r| r.id).collect();
        assert_eq!(role_ids, [SPACE, MOD_ROLE]);
        assert!(space.roles[0].is_everyone());
        assert_eq!(space.roles[1].color, Some(0xFF0000));
        assert!(space.roles[1].hoist);
        assert_eq!(space.members.len(), 2);
        assert_eq!(space.members[&ALICE].role_ids, [MOD_ROLE]);
        assert_eq!(space.members[&ALICE].nickname.as_deref(), Some("ali"));

        let member = cache.member(SPACE, ALICE).unwrap();
        assert_eq!(member.user.username, "alice");
        assert_eq!(member.joined_at, 200);
        assert!(cache.member(SPACE, BOB).is_none());

        assert_eq!(cache.channels.len(), 4);
        assert_eq!(cache.channels[&SECRET].channel.overrides.len(), 2);
        let voice = &cache.channels[&VOICE].channel;
        assert_eq!(
            (voice.kind, voice.bitrate),
            (hermes_proto::models::ChannelKind::Voice, Some(64_000))
        );
        assert_eq!(cache.channels[&DM].recipient_ids, [ALICE, BOB]);
        assert!(cache.channels[&DM].channel.space_id.is_none());
        assert!(cache.presences.is_empty() && cache.voice_states.is_empty());
        db.close().await;
    }

    #[tokio::test]
    async fn permissions_come_from_the_cache() {
        let dir = TempDir::new("cache-perms");
        let db = Db::open(dir.path()).await.unwrap();
        fixture(&db).await;
        let cache = load_from_db(&db).await.unwrap();
        db.close().await;

        // Owner has everything, everywhere.
        assert_eq!(cache.space_permissions(SPACE, OWNER), Permissions::all());
        // Alice: @everyone + mod.
        let alice = cache.space_permissions(SPACE, ALICE);
        assert!(alice.contains(Permissions::DEFAULT_EVERYONE | Permissions::MANAGE_MESSAGES));
        assert!(!alice.contains(Permissions::ADMINISTRATOR));
        // The secret channel denies VIEW_CHANNEL to @everyone and allows it to mod.
        assert!(
            cache
                .channel_permissions(GENERAL, ALICE)
                .contains(Permissions::VIEW_CHANNEL)
        );
        assert!(
            cache
                .channel_permissions(SECRET, ALICE)
                .contains(Permissions::VIEW_CHANNEL)
        );
        // Matches compute() on the wire models.
        let space = &cache.spaces[&SPACE];
        let member = cache.member(SPACE, ALICE).unwrap();
        assert_eq!(
            cache.channel_permissions(SECRET, ALICE),
            permissions::compute(
                &space.space,
                &space.roles,
                &member,
                Some(&cache.channels[&SECRET].channel)
            )
        );
        // Non-members get nothing.
        assert!(cache.space_permissions(SPACE, BOB).is_empty());
        assert!(cache.channel_permissions(GENERAL, BOB).is_empty());
        // DMs: recipients only.
        assert_eq!(
            cache.channel_permissions(DM, BOB),
            Permissions::DM_RECIPIENT
        );
        assert!(cache.channel_permissions(DM, OWNER).is_empty());
        assert!(cache.channel_permissions(Id(999), ALICE).is_empty());
    }

    #[tokio::test]
    async fn app_state_opens_data_dir() {
        let dir = TempDir::new("app-state");
        let mut config = Config::default();
        config.storage.data_dir = dir.path().join("data");
        let state = AppState::load(config).await.unwrap();
        assert!(dir.path().join("data").join(crate::db::DB_FILE).is_file());
        assert!(state.cache.read().unwrap().spaces.is_empty());
        assert!(state.ids.next() < state.ids.next());
        state.db.close().await;
    }
}
