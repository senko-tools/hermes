//! The normalized client state of one instance (PLAN.md section 14.1), built from `ready` and kept
//! current by dispatch events, plus per-channel message windows (14.4) and pending messages (14.3).

use std::collections::{BTreeMap, HashMap};

use hermes_proto::gateway::{
    InstanceInfo, ReactionEvent, Ready, ServerEvent, SpaceSnapshot, VoiceInfo, VoiceStateUpdate,
};
use hermes_proto::id::Id;
use hermes_proto::models::{
    Attachment, Channel, ChannelKind, CurrentUser, Member, Message, NotificationSetting,
    ReactionSummary, ReadState, Role, Space, Status, User, VoiceState,
};
use hermes_proto::permissions::{self, Permissions};

/// Messages kept per channel window.
pub const WINDOW_MAX: usize = 500;
/// Messages per history page, and what an idle window is trimmed to.
pub const PAGE: u32 = 50;
/// How long a typing indicator stays without a new `typing_start`.
pub const TYPING_MS: i64 = 10_000;
/// Windows not viewed for this long are trimmed to the last page.
pub const IDLE_WINDOW_MS: i64 = 10 * 60 * 1000;

/// `ready`/`space_create` carry at most this many members; bigger spaces load the rest over REST.
pub const SNAPSHOT_MEMBER_CAP: usize = 1000;

/// Identifies a pending message until the server assigns an ID.
pub type LocalId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingState {
    /// Waiting for the connection (sent in order on reconnect).
    Queued,
    Sending,
    /// Shown dimmed in red with "Retry" / "Delete".
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMessage {
    pub local_id: LocalId,
    pub nonce: String,
    pub channel_id: Id,
    pub content: String,
    pub reply_to: Option<Id>,
    pub attachments: Vec<Attachment>,
    pub state: PendingState,
    /// Unix ms.
    pub created_at: i64,
}

/// Where to load history from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAnchor {
    /// The newest page (opening a channel, jump to present).
    Latest,
    /// Older messages (scrolling up).
    Before(Id),
    /// Newer messages (scrolling down after a jump).
    After(Id),
    /// Jump to a message.
    Around(Id),
}

/// A contiguous slice of a channel's history, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageWindow {
    pub messages: Vec<Message>,
    /// Always shown after `messages`.
    pub pending: Vec<PendingMessage>,
    /// Nothing older exists on the server.
    pub reached_start: bool,
    /// The window ends with the newest message, so new messages are appended live.
    pub reached_end: bool,
    /// At least one page was loaded.
    pub loaded: bool,
    /// Unix ms of the last time a UI showed this window.
    pub last_viewed: i64,
}

impl MessageWindow {
    fn position(&self, id: Id) -> Result<usize, usize> {
        self.messages.binary_search_by_key(&id, |m| m.id)
    }

    pub fn get(&self, id: Id) -> Option<&Message> {
        self.position(id).ok().map(|i| &self.messages[i])
    }

    fn get_mut(&mut self, id: Id) -> Option<&mut Message> {
        self.position(id).ok().map(|i| &mut self.messages[i])
    }

    /// Inserts or replaces a message, keeping the order.
    fn upsert(&mut self, message: Message) {
        match self.position(message.id) {
            Ok(i) => self.messages[i] = message,
            Err(i) => self.messages.insert(i, message),
        }
    }

    /// Drops the oldest messages beyond the cap.
    fn cap_start(&mut self) {
        if self.messages.len() > WINDOW_MAX {
            let extra = self.messages.len() - WINDOW_MAX;
            self.messages.drain(..extra);
            self.reached_start = false;
        }
    }

    /// Drops the newest messages beyond the cap.
    fn cap_end(&mut self) {
        if self.messages.len() > WINDOW_MAX {
            self.messages.truncate(WINDOW_MAX);
            self.reached_end = false;
        }
    }

    pub fn pending(&self, local_id: LocalId) -> Option<&PendingMessage> {
        self.pending.iter().find(|p| p.local_id == local_id)
    }
}

/// What changed, for UIs that redraw selectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Changed {
    /// A new `ready`: everything.
    All,
    Me,
    /// The list of spaces.
    Spaces,
    Space(Id),
    /// A channel's settings or its place in the tree.
    Channel(Id),
    Messages(Id),
    Members(Id),
    Users,
    Presence(Id),
    ReadState(Id),
    Typing(Id),
    Dms,
    VoiceStates,
    NotificationSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceState {
    pub space: Space,
    /// Highest position first.
    pub roles: Vec<Role>,
    pub channels: BTreeMap<Id, Channel>,
    pub members: BTreeMap<Id, Member>,
    /// `members` holds the full list (from the snapshot, or over REST for big spaces).
    pub members_loaded: bool,
}

impl SpaceState {
    fn new(snapshot: SpaceSnapshot) -> Self {
        let members_loaded = snapshot.members.len() < SNAPSHOT_MEMBER_CAP;
        let mut state = SpaceState {
            space: snapshot.space,
            roles: snapshot.roles,
            channels: snapshot.channels.into_iter().map(|c| (c.id, c)).collect(),
            members: snapshot
                .members
                .into_iter()
                .map(|m| (m.user.id, m))
                .collect(),
            members_loaded,
        };
        state.sort_roles();
        state
    }

    fn sort_roles(&mut self) {
        self.roles
            .sort_by(|a, b| b.position.cmp(&a.position).then(a.id.cmp(&b.id)));
    }

    pub fn role(&self, id: Id) -> Option<&Role> {
        self.roles.iter().find(|r| r.id == id)
    }
}

/// Everything known about one instance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstanceStore {
    pub me: Option<CurrentUser>,
    pub instance: Option<InstanceInfo>,
    pub voice: Option<VoiceInfo>,
    pub spaces: BTreeMap<Id, SpaceState>,
    pub dms: BTreeMap<Id, Channel>,
    pub users: HashMap<Id, User>,
    pub read_states: HashMap<Id, ReadState>,
    /// Users not listed are offline.
    pub presences: HashMap<Id, Status>,
    pub notification_settings: HashMap<Id, NotificationSetting>,
    pub windows: HashMap<Id, MessageWindow>,
    /// channel → user → expiry (Unix ms).
    pub typing: HashMap<Id, BTreeMap<Id, i64>>,
    pub voice_states: HashMap<Id, VoiceState>,
    /// channel → space, for space channels.
    channel_space: HashMap<Id, Id>,
}

impl InstanceStore {
    pub fn me_id(&self) -> Option<Id> {
        self.me.as_ref().map(|m| m.user.id)
    }

    /// Replaces everything with a fresh `ready`. Message windows are stale and get reloaded;
    /// pending messages are kept so they can still be sent.
    pub fn apply_ready(&mut self, ready: Ready) {
        let windows = std::mem::take(&mut self.windows);
        *self = InstanceStore::default();
        self.windows = windows
            .into_iter()
            .filter(|(_, w)| !w.pending.is_empty())
            .map(|(id, w)| {
                (
                    id,
                    MessageWindow {
                        pending: w.pending,
                        ..MessageWindow::default()
                    },
                )
            })
            .collect();
        self.users
            .insert(ready.user.user.id, ready.user.user.clone());
        self.me = Some(ready.user);
        self.instance = Some(ready.instance);
        self.voice = Some(ready.voice);
        for snapshot in ready.spaces {
            self.insert_space(snapshot);
        }
        for dm in ready.dms {
            self.insert_dm(dm);
        }
        for state in ready.dm_voice_states {
            self.voice_states.insert(state.user_id, state);
        }
        self.read_states = ready
            .read_states
            .into_iter()
            .map(|r| (r.channel_id, r))
            .collect();
        self.presences = ready
            .presences
            .into_iter()
            .filter(|p| p.status != Status::Offline)
            .map(|p| (p.user_id, p.status))
            .collect();
        self.notification_settings = ready
            .notification_settings
            .into_iter()
            .map(|n| (n.target_id, n))
            .collect();
    }

    fn insert_space(&mut self, snapshot: SpaceSnapshot) {
        let space_id = snapshot.space.id;
        for channel in &snapshot.channels {
            self.channel_space.insert(channel.id, space_id);
        }
        for member in &snapshot.members {
            self.users.insert(member.user.id, member.user.clone());
        }
        for state in &snapshot.voice_states {
            self.voice_states.insert(state.user_id, *state);
        }
        self.spaces.insert(space_id, SpaceState::new(snapshot));
    }

    fn insert_dm(&mut self, dm: Channel) {
        for user in &dm.recipients {
            self.users.insert(user.id, user.clone());
        }
        self.dms.insert(dm.id, dm);
    }

    /// The full member list of a space (from REST).
    pub fn set_members(&mut self, space: Id, members: Vec<Member>) -> bool {
        let Some(state) = self.spaces.get_mut(&space) else {
            return false;
        };
        for m in &members {
            self.users.insert(m.user.id, m.user.clone());
        }
        state.members = members.into_iter().map(|m| (m.user.id, m)).collect();
        state.members_loaded = true;
        true
    }

    pub fn channel(&self, id: Id) -> Option<&Channel> {
        match self.channel_space.get(&id) {
            Some(space) => self.spaces.get(space)?.channels.get(&id),
            None => self.dms.get(&id),
        }
    }

    fn channel_mut(&mut self, id: Id) -> Option<&mut Channel> {
        match self.channel_space.get(&id) {
            Some(space) => self.spaces.get_mut(space)?.channels.get_mut(&id),
            None => self.dms.get_mut(&id),
        }
    }

    /// The space of a space channel (`None` for DMs and unknown channels).
    pub fn space_of(&self, channel: Id) -> Option<Id> {
        self.channel_space.get(&channel).copied()
    }

    pub fn user(&self, id: Id) -> Option<&User> {
        self.users.get(&id)
    }

    pub fn member(&self, space: Id, user: Id) -> Option<&Member> {
        self.spaces.get(&space)?.members.get(&user)
    }

    pub fn status(&self, user: Id) -> Status {
        self.presences
            .get(&user)
            .copied()
            .unwrap_or(Status::Offline)
    }

    /// Display name in a space (nickname, display name, username) or globally.
    pub fn display_name(&self, space: Option<Id>, user: Id) -> String {
        if let Some(member) = space.and_then(|s| self.member(s, user)) {
            if let Some(nick) = &member.nickname {
                return nick.clone();
            }
            return member.user.name().to_owned();
        }
        self.user(user)
            .map_or_else(|| format!("user {user}"), |u| u.name().to_owned())
    }

    /// My permissions in a space (`channel = None`) or a channel. Until the member list is
    /// loaded, only `@everyone` is counted (owners always have everything).
    pub fn permissions(&self, space: Id, channel: Option<Id>) -> Permissions {
        let (Some(me), Some(state)) = (self.me_id(), self.spaces.get(&space)) else {
            return Permissions::empty();
        };
        let fallback;
        let member = match state.members.get(&me) {
            Some(m) => m,
            None => {
                fallback = Member {
                    space_id: space,
                    user: self.me.as_ref().expect("me is set").user.clone(),
                    nickname: None,
                    role_ids: Vec::new(),
                    joined_at: 0,
                };
                &fallback
            }
        };
        let channel = channel.and_then(|c| state.channels.get(&c));
        permissions::compute(&state.space, &state.roles, member, channel)
    }

    /// My permissions in any channel, DMs included.
    pub fn channel_permissions(&self, channel: Id) -> Permissions {
        match self.space_of(channel) {
            Some(space) => self.permissions(space, Some(channel)),
            None if self.dms.contains_key(&channel) => Permissions::DM_RECIPIENT,
            None => Permissions::empty(),
        }
    }

    /// Whether the channel has messages newer than my read marker.
    pub fn unread(&self, channel: Id) -> bool {
        let Some(last) = self.channel(channel).and_then(|c| c.last_message_id) else {
            return false;
        };
        self.read_states
            .get(&channel)
            .is_none_or(|r| r.last_read_id < last)
    }

    pub fn mention_count(&self, channel: Id) -> u32 {
        self.read_states
            .get(&channel)
            .map_or(0, |r| r.mention_count)
    }

    /// Whether `message` mentions me (directly, through one of my roles, or @everyone/@here).
    pub fn mentions_me(&self, message: &Message) -> bool {
        let Some(me) = self.me_id() else {
            return false;
        };
        let m = &message.mentions;
        if m.everyone || m.here || m.users.contains(&me) {
            return true;
        }
        self.space_of(message.channel_id)
            .and_then(|s| self.member(s, me))
            .is_some_and(|member| member.role_ids.iter().any(|r| m.roles.contains(r)))
    }

    /// Users typing in a channel right now (not me), oldest first.
    pub fn typing(&self, channel: Id, now: i64) -> Vec<Id> {
        self.typing.get(&channel).map_or_else(Vec::new, |users| {
            users
                .iter()
                .filter(|&(_, &until)| until > now)
                .map(|(&u, _)| u)
                .collect()
        })
    }

    /// Removes expired typing entries; returns the channels that changed.
    pub fn expire_typing(&mut self, now: i64) -> Vec<Id> {
        let mut changed = Vec::new();
        self.typing.retain(|&channel, users| {
            let before = users.len();
            users.retain(|_, until| *until > now);
            if users.len() != before {
                changed.push(channel);
            }
            !users.is_empty()
        });
        changed
    }

    pub fn window(&self, channel: Id) -> Option<&MessageWindow> {
        self.windows.get(&channel)
    }

    pub fn window_mut(&mut self, channel: Id) -> &mut MessageWindow {
        self.windows.entry(channel).or_default()
    }

    /// Puts a loaded page of history into the channel's window.
    pub fn merge_history(
        &mut self,
        channel: Id,
        anchor: HistoryAnchor,
        page: Vec<Message>,
        limit: u32,
    ) {
        for m in &page {
            self.users.insert(m.author.id, m.author.clone());
        }
        let full = page.len() >= limit as usize;
        let newest = self.channel(channel).and_then(|c| c.last_message_id);
        let window = self.window_mut(channel);
        window.loaded = true;
        match anchor {
            HistoryAnchor::Latest => {
                // Messages that arrived live while the page was loading are newer than it.
                let newest_in_page = page.last().map(|m| m.id);
                let live: Vec<Message> = std::mem::take(&mut window.messages)
                    .into_iter()
                    .filter(|m| newest_in_page.is_none_or(|n| m.id > n))
                    .collect();
                window.messages = page;
                for m in live {
                    window.upsert(m);
                }
                window.reached_end = true;
                window.reached_start = !full;
                window.cap_start();
            }
            HistoryAnchor::Before(_) => {
                for m in page {
                    window.upsert(m);
                }
                window.reached_start = !full;
                window.cap_end();
            }
            HistoryAnchor::After(_) => {
                for m in page {
                    window.upsert(m);
                }
                if !full {
                    window.reached_end = true;
                }
                window.cap_start();
            }
            HistoryAnchor::Around(_) => {
                window.messages = page;
                window.reached_start = false;
                window.reached_end = match (newest, window.messages.last()) {
                    (None, _) => true,
                    (Some(newest), Some(last)) => last.id >= newest,
                    (Some(_), None) => false,
                };
            }
        }
    }

    /// Adds a pending message at the bottom of its channel.
    pub fn add_pending(&mut self, pending: PendingMessage) {
        self.window_mut(pending.channel_id).pending.push(pending);
    }

    pub fn pending_mut(&mut self, channel: Id, local_id: LocalId) -> Option<&mut PendingMessage> {
        self.windows
            .get_mut(&channel)?
            .pending
            .iter_mut()
            .find(|p| p.local_id == local_id)
    }

    pub fn remove_pending(&mut self, channel: Id, local_id: LocalId) -> Option<PendingMessage> {
        let window = self.windows.get_mut(&channel)?;
        let i = window.pending.iter().position(|p| p.local_id == local_id)?;
        Some(window.pending.remove(i))
    }

    /// The server accepted a pending message (REST response): replace it with the real one.
    pub fn resolve_pending(&mut self, local_id: LocalId, message: Message) {
        let channel = message.channel_id;
        self.remove_pending(channel, local_id);
        self.insert_message(message);
    }

    /// Adds a new message to its window (if the window is live) and updates the channel.
    fn insert_message(&mut self, message: Message) {
        let channel_id = message.channel_id;
        self.users.insert(message.author.id, message.author.clone());
        if let Some(channel) = self.channel_mut(channel_id)
            && channel.last_message_id.is_none_or(|l| l < message.id)
        {
            channel.last_message_id = Some(message.id);
        }
        // A window that is still loading keeps live messages too (see `merge_history`).
        if let Some(window) = self.windows.get_mut(&channel_id)
            && (!window.loaded || window.reached_end)
            && window.get(message.id).is_none()
        {
            window.upsert(message);
            window.cap_start();
        }
    }

    /// Trims windows not viewed for 10 minutes (except `keep`) to the last page.
    pub fn trim_windows(&mut self, now: i64, keep: &[Id]) -> Vec<Id> {
        let mut trimmed = Vec::new();
        for (&channel, window) in &mut self.windows {
            if keep.contains(&channel) || now - window.last_viewed < IDLE_WINDOW_MS {
                continue;
            }
            if window.reached_end && window.messages.len() > PAGE as usize {
                let extra = window.messages.len() - PAGE as usize;
                window.messages.drain(..extra);
                window.reached_start = false;
                trimmed.push(channel);
            } else if !window.reached_end && window.loaded {
                // A window in the middle of history is dropped; reopening loads the latest.
                window.messages.clear();
                window.loaded = false;
                trimmed.push(channel);
            }
        }
        trimmed
    }

    fn update_message(&mut self, message: Message) -> bool {
        self.users.insert(message.author.id, message.author.clone());
        let Some(window) = self.windows.get_mut(&message.channel_id) else {
            return false;
        };
        let Some(old) = window.get_mut(message.id) else {
            return false;
        };
        // Broadcast updates always say `me: false`; keep our own flags.
        let mut message = message;
        for r in &mut message.reactions {
            r.me = old.reactions.iter().any(|o| o.emoji == r.emoji && o.me);
        }
        *old = message;
        true
    }

    fn delete_message(&mut self, channel: Id, id: Id) -> bool {
        let Some(m) = self.windows.get_mut(&channel).and_then(|w| w.get_mut(id)) else {
            return false;
        };
        m.deleted = true;
        m.content.clear();
        m.pinned = false;
        m.attachments.clear();
        m.reactions.clear();
        m.mentions = Default::default();
        true
    }

    fn react(&mut self, r: &ReactionEvent, add: bool) -> bool {
        let me = self.me_id() == Some(r.user_id);
        let Some(m) = self
            .windows
            .get_mut(&r.channel_id)
            .and_then(|w| w.get_mut(r.message_id))
        else {
            return false;
        };
        match m.reactions.iter().position(|s| s.emoji == r.emoji) {
            Some(i) if add => {
                let summary = &mut m.reactions[i];
                if !(me && summary.me) {
                    summary.count += 1;
                }
                summary.me |= me;
            }
            Some(i) => {
                let summary = &mut m.reactions[i];
                summary.count = summary.count.saturating_sub(1);
                if me {
                    summary.me = false;
                }
                if summary.count == 0 {
                    m.reactions.remove(i);
                }
            }
            None if add => m.reactions.push(ReactionSummary {
                emoji: r.emoji.clone(),
                count: 1,
                me,
            }),
            None => return false,
        }
        true
    }

    fn update_user_everywhere(&mut self, user: &User) {
        self.users.insert(user.id, user.clone());
        if let Some(me) = &mut self.me
            && me.user.id == user.id
        {
            me.user = user.clone();
        }
        for state in self.spaces.values_mut() {
            if let Some(m) = state.members.get_mut(&user.id) {
                m.user = user.clone();
            }
        }
        for dm in self.dms.values_mut() {
            for r in &mut dm.recipients {
                if r.id == user.id {
                    *r = user.clone();
                }
            }
        }
    }

    fn remove_channel_data(&mut self, channel: Id) {
        self.windows.remove(&channel);
        self.typing.remove(&channel);
        self.read_states.remove(&channel);
        self.channel_space.remove(&channel);
    }

    /// Applies a dispatch event. `now` is Unix ms (for typing).
    pub fn apply(&mut self, event: ServerEvent, now: i64) -> Vec<Changed> {
        use Changed as C;
        match event {
            ServerEvent::Hello { .. }
            | ServerEvent::HeartbeatAck {}
            | ServerEvent::Resumed {}
            | ServerEvent::InvalidSession { .. } => vec![],
            ServerEvent::Ready(ready) => {
                self.apply_ready(*ready);
                vec![C::All]
            }
            ServerEvent::MessageCreate(message) => {
                let channel = message.channel_id;
                let me = self.me_id();
                let mut changed = vec![C::Messages(channel), C::Channel(channel)];
                if let Some(nonce) = &message.nonce
                    && let Some(window) = self.windows.get_mut(&channel)
                {
                    window.pending.retain(|p| &p.nonce != nonce);
                }
                if let Some(users) = self.typing.get_mut(&channel)
                    && users.remove(&message.author.id).is_some()
                {
                    changed.push(C::Typing(channel));
                }
                if Some(message.author.id) == me {
                    // My own message: the channel is read up to here.
                    let state = self.read_states.entry(channel).or_insert(ReadState {
                        channel_id: channel,
                        last_read_id: message.id,
                        mention_count: 0,
                    });
                    state.last_read_id = state.last_read_id.max(message.id);
                    state.mention_count = 0;
                    changed.push(C::ReadState(channel));
                } else if self.mentions_me(&message) {
                    let state = self.read_states.entry(channel).or_insert(ReadState {
                        channel_id: channel,
                        last_read_id: Id(0),
                        mention_count: 0,
                    });
                    if state.last_read_id < message.id {
                        state.mention_count += 1;
                        changed.push(C::ReadState(channel));
                    }
                }
                self.insert_message(message);
                changed
            }
            ServerEvent::MessageUpdate(message) => {
                let channel = message.channel_id;
                if self.update_message(message) {
                    vec![C::Messages(channel)]
                } else {
                    vec![]
                }
            }
            ServerEvent::MessageDelete { id, channel_id } => {
                if self.delete_message(channel_id, id) {
                    vec![C::Messages(channel_id)]
                } else {
                    vec![]
                }
            }
            ServerEvent::MessageDeleteBulk { ids, channel_id } => {
                let mut any = false;
                for id in ids {
                    any |= self.delete_message(channel_id, id);
                }
                if any {
                    vec![C::Messages(channel_id)]
                } else {
                    vec![]
                }
            }
            ServerEvent::ReactionAdd(r) => {
                if self.react(&r, true) {
                    vec![C::Messages(r.channel_id)]
                } else {
                    vec![]
                }
            }
            ServerEvent::ReactionRemove(r) => {
                if self.react(&r, false) {
                    vec![C::Messages(r.channel_id)]
                } else {
                    vec![]
                }
            }
            ServerEvent::TypingStart {
                channel_id,
                user_id,
                ..
            } => {
                if Some(user_id) == self.me_id() {
                    return vec![];
                }
                self.typing
                    .entry(channel_id)
                    .or_default()
                    .insert(user_id, now + TYPING_MS);
                vec![C::Typing(channel_id)]
            }
            ServerEvent::ReadStateUpdate(state) => {
                let channel = state.channel_id;
                self.read_states.insert(channel, state);
                vec![C::ReadState(channel)]
            }
            ServerEvent::ChannelCreate(channel) | ServerEvent::ChannelUpdate(channel) => {
                let id = channel.id;
                match channel.space_id {
                    Some(space) => {
                        let Some(state) = self.spaces.get_mut(&space) else {
                            return vec![];
                        };
                        state.channels.insert(id, channel);
                        self.channel_space.insert(id, space);
                        vec![C::Channel(id), C::Space(space)]
                    }
                    None => {
                        self.insert_dm(channel);
                        vec![C::Channel(id), C::Dms]
                    }
                }
            }
            ServerEvent::ChannelDelete { id, space_id } => {
                if let Some(state) = self.spaces.get_mut(&space_id) {
                    state.channels.remove(&id);
                }
                self.remove_channel_data(id);
                vec![C::Channel(id), C::Space(space_id)]
            }
            ServerEvent::SpaceCreate(snapshot) => {
                let id = snapshot.space.id;
                self.insert_space(*snapshot);
                vec![C::Spaces, C::Space(id)]
            }
            ServerEvent::SpaceUpdate(space) => {
                let id = space.id;
                match self.spaces.get_mut(&id) {
                    Some(state) => {
                        state.space = space;
                        vec![C::Space(id)]
                    }
                    None => vec![],
                }
            }
            ServerEvent::SpaceDelete { id } => {
                if let Some(state) = self.spaces.remove(&id) {
                    for channel in state.channels.keys() {
                        self.remove_channel_data(*channel);
                    }
                }
                vec![C::Spaces, C::Space(id)]
            }
            ServerEvent::RoleCreate(role) | ServerEvent::RoleUpdate(role) => {
                let space = role.space_id;
                let Some(state) = self.spaces.get_mut(&space) else {
                    return vec![];
                };
                match state.roles.iter_mut().find(|r| r.id == role.id) {
                    Some(r) => *r = role,
                    None => state.roles.push(role),
                }
                state.sort_roles();
                vec![C::Space(space), C::Members(space)]
            }
            ServerEvent::RoleDelete { id, space_id } => {
                let Some(state) = self.spaces.get_mut(&space_id) else {
                    return vec![];
                };
                state.roles.retain(|r| r.id != id);
                for m in state.members.values_mut() {
                    m.role_ids.retain(|&r| r != id);
                }
                vec![C::Space(space_id), C::Members(space_id)]
            }
            ServerEvent::MemberAdd(member) | ServerEvent::MemberUpdate(member) => {
                let space = member.space_id;
                self.users.insert(member.user.id, member.user.clone());
                let Some(state) = self.spaces.get_mut(&space) else {
                    return vec![];
                };
                state.members.insert(member.user.id, member);
                vec![C::Members(space)]
            }
            ServerEvent::MemberRemove { space_id, user_id } => {
                if let Some(state) = self.spaces.get_mut(&space_id) {
                    state.members.remove(&user_id);
                }
                vec![C::Members(space_id)]
            }
            ServerEvent::UserUpdate(user) => {
                let me = self.me_id() == Some(user.id);
                self.update_user_everywhere(&user);
                if me {
                    vec![C::Users, C::Me]
                } else {
                    vec![C::Users]
                }
            }
            ServerEvent::PresenceUpdate(p) => {
                if p.status == Status::Offline {
                    self.presences.remove(&p.user_id);
                } else {
                    self.presences.insert(p.user_id, p.status);
                }
                vec![C::Presence(p.user_id)]
            }
            ServerEvent::VoiceStateUpdate(update) => {
                match update {
                    VoiceStateUpdate::Joined(state) => {
                        self.voice_states.insert(state.user_id, state);
                    }
                    VoiceStateUpdate::Left { user_id, .. } => {
                        self.voice_states.remove(&user_id);
                    }
                }
                vec![C::VoiceStates]
            }
            ServerEvent::CallRing { .. } | ServerEvent::CallRingStop { .. } => vec![],
            ServerEvent::DmCreate(dm) | ServerEvent::DmUpdate(dm) => {
                let id = dm.id;
                self.insert_dm(dm);
                vec![C::Dms, C::Channel(id)]
            }
            ServerEvent::DmDelete { id } => {
                self.dms.remove(&id);
                self.remove_channel_data(id);
                vec![C::Dms, C::Channel(id)]
            }
            ServerEvent::NotificationSettingsUpdate(setting) => {
                self.notification_settings
                    .insert(setting.target_id, setting);
                vec![C::NotificationSettings]
            }
            ServerEvent::NotificationSettingsDelete { target_id } => {
                self.notification_settings.remove(&target_id);
                vec![C::NotificationSettings]
            }
        }
    }

    /// Channels of a DM that are 1:1 (for naming): the other recipient.
    pub fn dm_partner<'a>(&self, dm: &'a Channel) -> Option<&'a User> {
        if dm.kind != ChannelKind::Dm {
            return None;
        }
        let me = self.me_id();
        dm.recipients.iter().find(|u| Some(u.id) != me)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use hermes_proto::gateway::InstanceLimits;
    use hermes_proto::models::{Mentions, MessageKind, Presence};

    pub fn user(id: u64, name: &str) -> User {
        User {
            id: Id(id),
            username: name.into(),
            display_name: None,
            avatar: None,
            status_text: None,
        }
    }

    pub fn channel(id: u64, space: Option<u64>, kind: ChannelKind, name: &str) -> Channel {
        Channel {
            id: Id(id),
            space_id: space.map(Id),
            kind,
            parent_id: None,
            name: Some(name.into()),
            topic: None,
            position: 0,
            bitrate: None,
            user_limit: None,
            slowmode_secs: 0,
            overrides: vec![],
            recipients: vec![],
            owner_id: None,
            last_message_id: None,
        }
    }

    pub fn message(id: u64, channel: u64, author: &User, content: &str) -> Message {
        Message {
            id: Id(id),
            channel_id: Id(channel),
            author: author.clone(),
            content: content.into(),
            reply_to: None,
            edited_at: None,
            deleted: false,
            pinned: false,
            kind: MessageKind::Default,
            attachments: vec![],
            reactions: vec![],
            mentions: Mentions::default(),
            nonce: None,
        }
    }

    /// Space 10 (`@everyone` = role 10) owned by user 2 with `#general` (11); I am user 1.
    pub fn ready() -> Ready {
        let me = user(1, "me");
        Ready {
            session_id: "s".into(),
            user: CurrentUser {
                user: me,
                instance_admin: false,
            },
            spaces: vec![SpaceSnapshot {
                space: Space {
                    id: Id(10),
                    name: "Friends".into(),
                    description: None,
                    icon: None,
                    owner_id: Id(2),
                },
                roles: vec![Role {
                    id: Id(10),
                    space_id: Id(10),
                    name: "@everyone".into(),
                    color: None,
                    hoist: false,
                    mentionable: false,
                    position: 0,
                    permissions: Permissions::DEFAULT_EVERYONE,
                }],
                channels: vec![channel(11, Some(10), ChannelKind::Text, "general")],
                members: vec![],
                voice_states: vec![],
            }],
            dms: vec![],
            read_states: vec![],
            presences: vec![Presence {
                user_id: Id(2),
                status: Status::Online,
            }],
            notification_settings: vec![],
            dm_voice_states: vec![],
            voice: VoiceInfo {
                enabled: true,
                quic_port: Some(7700),
                cert_sha256: None,
            },
            instance: InstanceInfo {
                name: "Test".into(),
                limits: InstanceLimits {
                    max_upload_mb: 25,
                    max_message_length: 4000,
                },
            },
        }
    }

    fn store() -> InstanceStore {
        let mut s = InstanceStore::default();
        s.apply_ready(ready());
        s
    }

    fn live_window(s: &mut InstanceStore, channel: u64) {
        s.merge_history(Id(channel), HistoryAnchor::Latest, vec![], PAGE);
    }

    #[test]
    fn ready_fills_the_store() {
        let s = store();
        assert_eq!(s.me_id(), Some(Id(1)));
        assert_eq!(s.space_of(Id(11)), Some(Id(10)));
        assert_eq!(s.channel(Id(11)).unwrap().name.as_deref(), Some("general"));
        assert_eq!(s.status(Id(2)), Status::Online);
        assert_eq!(s.status(Id(3)), Status::Offline);
        assert!(
            s.permissions(Id(10), Some(Id(11)))
                .contains(Permissions::SEND_MESSAGES)
        );
        assert!(
            !s.permissions(Id(10), None)
                .contains(Permissions::MANAGE_SPACE)
        );
    }

    #[test]
    fn messages_are_appended_only_to_live_windows() {
        let mut s = store();
        let bob = user(2, "bob");
        s.apply(ServerEvent::MessageCreate(message(100, 11, &bob, "hi")), 0);
        assert!(s.window(Id(11)).is_none(), "no window opened yet");
        assert_eq!(s.channel(Id(11)).unwrap().last_message_id, Some(Id(100)));
        assert!(s.unread(Id(11)));

        live_window(&mut s, 11);
        s.apply(
            ServerEvent::MessageCreate(message(101, 11, &bob, "again")),
            0,
        );
        let ids: Vec<u64> = s
            .window(Id(11))
            .unwrap()
            .messages
            .iter()
            .map(|m| m.id.0)
            .collect();
        assert_eq!(ids, [101]);
        // Duplicate delivery is ignored.
        s.apply(
            ServerEvent::MessageCreate(message(101, 11, &bob, "again")),
            0,
        );
        assert_eq!(s.window(Id(11)).unwrap().messages.len(), 1);
    }

    #[test]
    fn pending_messages_are_replaced_by_nonce_or_response() {
        let mut s = store();
        live_window(&mut s, 11);
        let me = user(1, "me");
        for (local_id, nonce) in [(1, "n1"), (2, "n2")] {
            s.add_pending(PendingMessage {
                local_id,
                nonce: nonce.into(),
                channel_id: Id(11),
                content: "x".into(),
                reply_to: None,
                attachments: vec![],
                state: PendingState::Sending,
                created_at: 0,
            });
        }
        // Gateway first, then the REST response.
        let mut m = message(200, 11, &me, "x");
        m.nonce = Some("n1".into());
        s.apply(ServerEvent::MessageCreate(m.clone()), 0);
        assert_eq!(s.window(Id(11)).unwrap().pending.len(), 1);
        s.resolve_pending(1, m);
        // REST first, then the gateway.
        let mut m2 = message(201, 11, &me, "x");
        s.resolve_pending(2, m2.clone());
        m2.nonce = Some("n2".into());
        s.apply(ServerEvent::MessageCreate(m2), 0);

        let w = s.window(Id(11)).unwrap();
        assert!(w.pending.is_empty());
        let ids: Vec<u64> = w.messages.iter().map(|m| m.id.0).collect();
        assert_eq!(ids, [200, 201]);
        assert!(!s.unread(Id(11)), "own messages mark the channel read");
    }

    #[test]
    fn live_messages_survive_a_slower_first_page() {
        let mut s = store();
        let bob = user(2, "bob");
        let me = user(1, "me");
        // The window exists (a pending message) but its first page has not arrived yet.
        s.add_pending(PendingMessage {
            local_id: 1,
            nonce: "n1".into(),
            channel_id: Id(11),
            content: "mine".into(),
            reply_to: None,
            attachments: vec![],
            state: PendingState::Sending,
            created_at: 0,
        });
        let mut mine = message(30, 11, &me, "mine");
        mine.nonce = Some("n1".into());
        s.apply(ServerEvent::MessageCreate(mine), 0);
        // The page was read before message 30 existed.
        s.merge_history(
            Id(11),
            HistoryAnchor::Latest,
            vec![message(10, 11, &bob, "a"), message(20, 11, &bob, "b")],
            PAGE,
        );
        let w = s.window(Id(11)).unwrap();
        let ids: Vec<u64> = w.messages.iter().map(|m| m.id.0).collect();
        assert_eq!(ids, [10, 20, 30]);
        assert!(w.pending.is_empty());
    }

    #[test]
    fn edits_deletes_and_reactions() {
        let mut s = store();
        let bob = user(2, "bob");
        let mut m = message(100, 11, &bob, "hi");
        s.merge_history(Id(11), HistoryAnchor::Latest, vec![m.clone()], PAGE);

        let r = |user: u64| ReactionEvent {
            message_id: Id(100),
            channel_id: Id(11),
            emoji: "😀".into(),
            user_id: Id(user),
        };
        s.apply(ServerEvent::ReactionAdd(r(2)), 0);
        s.apply(ServerEvent::ReactionAdd(r(1)), 0);
        let get = |s: &InstanceStore| s.window(Id(11)).unwrap().get(Id(100)).unwrap().clone();
        assert_eq!(
            get(&s).reactions,
            [ReactionSummary {
                emoji: "😀".into(),
                count: 2,
                me: true
            }]
        );

        // A broadcast edit says `me: false`; ours stays.
        m.content = "edited".into();
        m.reactions = vec![ReactionSummary {
            emoji: "😀".into(),
            count: 2,
            me: false,
        }];
        s.apply(ServerEvent::MessageUpdate(m), 0);
        assert_eq!(get(&s).content, "edited");
        assert!(get(&s).reactions[0].me);

        s.apply(ServerEvent::ReactionRemove(r(1)), 0);
        s.apply(ServerEvent::ReactionRemove(r(2)), 0);
        assert!(get(&s).reactions.is_empty());

        s.apply(
            ServerEvent::MessageDelete {
                id: Id(100),
                channel_id: Id(11),
            },
            0,
        );
        assert!(get(&s).deleted);
        assert!(get(&s).content.is_empty());
    }

    #[test]
    fn mentions_raise_the_count_until_read() {
        let mut s = store();
        let bob = user(2, "bob");
        let mut m = message(100, 11, &bob, "<@1>");
        m.mentions.users = vec![Id(1)];
        s.apply(ServerEvent::MessageCreate(m), 0);
        assert_eq!(s.mention_count(Id(11)), 1);
        s.apply(
            ServerEvent::ReadStateUpdate(ReadState {
                channel_id: Id(11),
                last_read_id: Id(100),
                mention_count: 0,
            }),
            0,
        );
        assert_eq!(s.mention_count(Id(11)), 0);
        assert!(!s.unread(Id(11)));
    }

    #[test]
    fn typing_expires_and_stops_on_message() {
        let mut s = store();
        let typing = |user| ServerEvent::TypingStart {
            channel_id: Id(11),
            user_id: Id(user),
            timestamp: 0,
        };
        s.apply(typing(2), 1000);
        s.apply(typing(1), 1000); // me: ignored
        assert_eq!(s.typing(Id(11), 1000), [Id(2)]);
        assert_eq!(s.expire_typing(1000 + TYPING_MS), [Id(11)]);
        assert!(s.typing(Id(11), 1000).is_empty());

        s.apply(typing(2), 0);
        s.apply(
            ServerEvent::MessageCreate(message(5, 11, &user(2, "bob"), "x")),
            0,
        );
        assert!(s.typing(Id(11), 0).is_empty());
    }

    #[test]
    fn history_windows_merge_and_cap() {
        let mut s = store();
        let bob = user(2, "bob");
        let page = |from: u64, n: u64| -> Vec<Message> {
            (from..from + n)
                .map(|i| message(i, 11, &bob, "m"))
                .collect()
        };
        s.apply(ServerEvent::MessageCreate(message(1000, 11, &bob, "x")), 0);
        s.merge_history(Id(11), HistoryAnchor::Latest, page(951, 50), PAGE);
        let w = s.window(Id(11)).unwrap();
        assert!(w.reached_end && !w.reached_start);

        for i in 0..10 {
            let first = 951 - 50 * (i + 1);
            s.merge_history(
                Id(11),
                HistoryAnchor::Before(Id(first + 50)),
                page(first, 50),
                PAGE,
            );
        }
        let w = s.window(Id(11)).unwrap();
        assert_eq!(w.messages.len(), WINDOW_MAX);
        assert!(!w.reached_end, "capped at the newest end");
        assert_eq!(w.messages[0].id, Id(451));

        s.merge_history(Id(11), HistoryAnchor::Before(Id(451)), page(440, 11), PAGE);
        assert!(s.window(Id(11)).unwrap().reached_start);

        s.merge_history(Id(11), HistoryAnchor::Around(Id(990)), page(965, 36), PAGE);
        let w = s.window(Id(11)).unwrap();
        assert!(w.reached_end, "ends at the newest message");
    }

    #[test]
    fn idle_windows_are_trimmed() {
        let mut s = store();
        let bob = user(2, "bob");
        let msgs: Vec<Message> = (1..=120).map(|i| message(i, 11, &bob, "m")).collect();
        s.merge_history(Id(11), HistoryAnchor::Latest, msgs, 200);
        s.window_mut(Id(11)).last_viewed = 0;
        assert!(s.trim_windows(IDLE_WINDOW_MS - 1, &[]).is_empty());
        assert!(s.trim_windows(IDLE_WINDOW_MS, &[Id(11)]).is_empty(), "kept");
        assert_eq!(s.trim_windows(IDLE_WINDOW_MS, &[]), [Id(11)]);
        let w = s.window(Id(11)).unwrap();
        assert_eq!(w.messages.len(), PAGE as usize);
        assert_eq!(w.messages.last().unwrap().id, Id(120));
    }

    #[test]
    fn space_lifecycle_and_members() {
        let mut s = store();
        let bob = user(2, "bob");
        assert!(s.set_members(
            Id(10),
            vec![Member {
                space_id: Id(10),
                user: bob.clone(),
                nickname: Some("Bobby".into()),
                role_ids: vec![],
                joined_at: 0,
            }]
        ));
        assert_eq!(s.display_name(Some(Id(10)), Id(2)), "Bobby");
        assert_eq!(s.display_name(None, Id(2)), "bob");

        let mut renamed = bob.clone();
        renamed.display_name = Some("Robert".into());
        s.apply(ServerEvent::UserUpdate(renamed), 0);
        assert_eq!(s.member(Id(10), Id(2)).unwrap().user.name(), "Robert");

        s.apply(
            ServerEvent::ChannelCreate(channel(12, Some(10), ChannelKind::Voice, "Lounge")),
            0,
        );
        assert_eq!(s.space_of(Id(12)), Some(Id(10)));
        s.apply(
            ServerEvent::ChannelDelete {
                id: Id(12),
                space_id: Id(10),
            },
            0,
        );
        assert!(s.channel(Id(12)).is_none());

        s.apply(ServerEvent::SpaceDelete { id: Id(10) }, 0);
        assert!(s.spaces.is_empty());
        assert!(s.channel(Id(11)).is_none());
    }

    #[test]
    fn a_new_ready_keeps_pending_messages_only() {
        let mut s = store();
        live_window(&mut s, 11);
        s.add_pending(PendingMessage {
            local_id: 1,
            nonce: "n".into(),
            channel_id: Id(11),
            content: "queued".into(),
            reply_to: None,
            attachments: vec![],
            state: PendingState::Queued,
            created_at: 0,
        });
        s.apply_ready(ready());
        let w = s.window(Id(11)).unwrap();
        assert!(!w.loaded);
        assert_eq!(w.pending.len(), 1);
    }
}
