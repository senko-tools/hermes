//! The keys of encrypted server channels in the client's flow (docs/e2ee.md). Each channel has
//! numbered keys, kept here in a key ring. Whoever writes first makes the first one; a new one
//! replaces it when someone who held it can no longer read the channel; members hand them to
//! devices that lack them, by themselves or only when someone approves, as the channel's rules say.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hermes_proto::ErrorCode;
use hermes_proto::e2ee::{
    KeyHandover, KeyPolicy, KeyPolicyOverride, KeyRequest, MAX_CHANNEL_KEYS, SealedChannelKey,
};
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::config::write_atomic;
use crate::core::{Core, CoreError, CoreEvent, Result, now_ms};
use crate::e2ee;
use crate::rest;
use crate::secure::{AccountKeys, DEVICES_MAX_AGE_MS};
use crate::store::{Changed, HistoryAnchor};
use crate::view;

/// A device asks for a channel's keys at most this often.
const REQUEST_EVERY_MS: i64 = 30_000;
/// How long a member waits, at most, before serving a request: others may be quicker.
const SERVE_JITTER_MS: u64 = 1500;

pub const WAITING_FOR_KEYS: &str =
    "Encrypted. This will show once someone in the channel has shared its keys with this device.";

/// A channel's keys, encrypted on disk with a key from the OS credential store. They do not go in
/// the store itself: Windows credentials hold far too little.
pub(crate) struct KeyRing {
    path: Option<PathBuf>,
    wrap: Zeroizing<[u8; 32]>,
    keys: BTreeMap<(Id, u32), Zeroizing<[u8; 32]>>,
    pins: Pins,
    /// The file could not be read and was set aside.
    damaged: bool,
}

/// What a server must not be able to talk this device out of.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Pins {
    /// Channels seen to need approval before a key is handed over.
    approved: BTreeSet<Id>,
    /// Channels seen to keep the past from new members.
    hidden_history: BTreeSet<Id>,
    /// The first sealed message seen in each conversation.
    sealed_since: BTreeMap<Id, Id>,
    /// This instance was seen to encrypt its channels.
    instance_sealed: bool,
    /// What was being written in each conversation.
    #[serde(default)]
    drafts: BTreeMap<Id, String>,
    /// Messages that had not gone out when Hermes closed: channel, text, what they answer.
    #[serde(default)]
    outbox: Vec<(Id, String, Option<Id>)>,
    /// My own notes on people.
    #[serde(default)]
    notes: BTreeMap<Id, String>,
    /// Names I gave people, shown instead of theirs.
    #[serde(default)]
    local_names: BTreeMap<Id, String>,
    /// Messages I saved to come back to, newest saved last.
    #[serde(default)]
    saved: Vec<SavedMessage>,
}

/// A message I saved: where it is, and a copy of what it said when saved (the list shows
/// without asking the server, and an encrypted message stays readable here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMessage {
    pub channel: Id,
    pub message: Id,
    /// Unix ms.
    pub saved_at: i64,
    pub author: String,
    pub text: String,
}

/// Saved messages kept at most.
const MAX_SAVED: usize = 1000;

/// The ring's file.
#[derive(Serialize, Deserialize)]
struct Stored<K> {
    keys: Vec<(Id, u32, K)>,
    pins: Pins,
}

impl KeyRing {
    pub(crate) fn load(path: Option<PathBuf>, wrap: Zeroizing<[u8; 32]>) -> KeyRing {
        let mut keys = BTreeMap::new();
        let file = path.as_ref().and_then(|path| std::fs::read(path).ok());
        let stored = file.as_ref().and_then(|sealed| {
            let plain = e2ee::open_local(&wrap, sealed).ok()?;
            serde_json::from_slice::<Stored<String>>(&plain)
                .ok()
                .or_else(|| {
                    // The form from before pins were kept.
                    let keys = serde_json::from_slice(&plain).ok()?;
                    Some(Stored {
                        keys,
                        pins: Pins::default(),
                    })
                })
        });
        let damaged = file.is_some() && stored.is_none();
        if damaged && let Some(path) = &path {
            let _ = std::fs::rename(path, path.with_extension("damaged"));
        }
        let (list, pins) = stored.map_or_else(Default::default, |s| (s.keys, s.pins));
        for (channel, epoch, key) in list {
            if let Ok(key) = e2ee::decode_key(&key) {
                keys.insert((channel, epoch), key);
            }
        }
        KeyRing {
            path,
            wrap,
            keys,
            pins,
            damaged,
        }
    }

    pub(crate) fn damaged(&self) -> bool {
        self.damaged
    }

    /// The newest key held for a channel; 0 when there is none.
    fn newest_epoch(&self, channel: Id) -> u32 {
        self.epochs(channel).last().copied().unwrap_or(0)
    }

    /// Applies what was pinned to what the server says, and pins what is stricter. Whether the
    /// server was asking for less than has been seen before.
    fn pin_policy(&mut self, channel: Id, policy: &mut KeyPolicy) -> bool {
        let mut loosened = false;
        let mut changed = false;
        if policy.handover == KeyHandover::Approved {
            changed |= self.pins.approved.insert(channel);
        } else if self.pins.approved.contains(&channel) {
            policy.handover = KeyHandover::Approved;
            loosened = true;
        }
        if !policy.new_members_read_history {
            changed |= self.pins.hidden_history.insert(channel);
        } else if self.pins.hidden_history.contains(&channel) {
            policy.new_members_read_history = false;
            loosened = true;
        }
        if changed {
            self.save();
        }
        loosened
    }

    /// I changed this channel's rules myself, so whatever the server says next is real.
    fn unpin_policy(&mut self, channel: Id) {
        let removed = self.pins.approved.remove(&channel);
        if self.pins.hidden_history.remove(&channel) || removed {
            self.save();
        }
    }

    /// Remembers the earliest sealed message seen in a conversation.
    pub(crate) fn note_sealed(&mut self, channel: Id, message: Id) {
        let first = self.pins.sealed_since.entry(channel).or_insert(message);
        let earlier = message < *first;
        let new = *first == message;
        if earlier {
            *first = message;
        }
        if earlier || new {
            self.save();
        }
    }

    /// The first sealed message seen in a conversation.
    pub(crate) fn sealed_since(&self, channel: Id) -> Option<Id> {
        self.pins.sealed_since.get(&channel).copied()
    }

    fn remember_instance_sealed(&mut self) {
        if !self.pins.instance_sealed {
            self.pins.instance_sealed = true;
            self.save();
        }
    }

    fn get(&self, channel: Id, epoch: u32) -> Option<Zeroizing<[u8; 32]>> {
        self.keys.get(&(channel, epoch)).cloned()
    }

    /// The epochs of a channel I hold keys for, oldest first.
    fn epochs(&self, channel: Id) -> Vec<u32> {
        self.keys
            .range((channel, 0)..=(channel, u32::MAX))
            .map(|((_, epoch), _)| *epoch)
            .collect()
    }

    fn insert(&mut self, channel: Id, epoch: u32, key: Zeroizing<[u8; 32]>) {
        // A key already held is never replaced: whoever hands one over cannot swap it.
        if let std::collections::btree_map::Entry::Vacant(free) = self.keys.entry((channel, epoch))
        {
            free.insert(key);
            self.save();
        }
    }

    fn remove(&mut self, channel: Id, epoch: u32) {
        if self.keys.remove(&(channel, epoch)).is_some() {
            self.save();
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let encoded: Vec<(Id, u32, Zeroizing<String>)> = self
            .keys
            .iter()
            .map(|((channel, epoch), key)| (*channel, *epoch, e2ee::encode_key(key)))
            .collect();
        let list: Vec<(Id, u32, &str)> = encoded
            .iter()
            .map(|(channel, epoch, key)| (*channel, *epoch, key.as_str()))
            .collect();
        let stored = Stored {
            keys: list,
            pins: self.pins.clone(),
        };
        let Ok(plain) = serde_json::to_vec(&stored).map(Zeroizing::new) else {
            return;
        };
        let Ok(sealed) = e2ee::seal_local(&self.wrap, &plain) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(err) = write_atomic(path, &sealed) {
            tracing::error!("could not keep this channel's keys: {err}");
        }
    }
}

/// What is known of a channel's keys.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelState {
    fetched_at: i64,
    current: u32,
    rotate_needed: bool,
    policy: KeyPolicy,
}

/// Per account: channels' states and the requests waiting for someone to approve them.
#[derive(Default)]
pub(crate) struct ChannelKeyCache {
    states: HashMap<Id, ChannelState>,
    asked_at: HashMap<Id, i64>,
    waiting: Vec<KeyRequest>,
    /// Channels already warned about, so that it is said once.
    warned: HashSet<Id>,
}

fn locked(err: impl std::fmt::Display) -> CoreError {
    CoreError::Voice(format!("Could not encrypt: {err}"))
}

impl Core {
    /// Whether what is written in this channel is encrypted.
    pub fn channel_is_encrypted(&self, inst: &str, channel: Id) -> bool {
        let (said, server_channel) = self.store().instance(inst).map_or((false, false), |s| {
            (
                s.channel_encrypted(channel),
                s.channel(channel)
                    .is_some_and(|c| !c.kind.is_private() && c.kind.has_messages()),
            )
        });
        let Some(keys) = self.account_keys(inst) else {
            return said;
        };
        let mut ring = keys.ring.lock().expect("ring");
        if said && server_channel {
            ring.remember_instance_sealed();
        }
        // One word from the server must not be enough to turn encryption off.
        said || (server_channel && ring.pins.instance_sealed)
    }

    fn forget_state(keys: &AccountKeys, channel: Id) {
        let mut cache = keys.channels.lock().expect("channel keys");
        cache.states.remove(&channel);
    }

    /// What the server says about a channel's keys, no older than `max_age_ms`. Any new key it
    /// holds for this device goes in the ring.
    async fn channel_state(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
        max_age_ms: i64,
    ) -> Result<ChannelState> {
        let now = now_ms();
        if let Some(state) = keys
            .channels
            .lock()
            .expect("channel keys")
            .states
            .get(&channel)
            .filter(|s| now - s.fetched_at < max_age_ms)
        {
            return Ok(*state);
        }
        let response = self
            .rest(inst)?
            .channel_keys(channel, keys.identity.device_id())
            .await?;
        let held = keys.ring.lock().expect("ring").epochs(channel);
        for sealed in &response.keys {
            if !held.contains(&sealed.epoch) {
                self.open_sealed_key(inst, keys, channel, sealed).await;
            }
        }
        let mut policy = response.policy;
        let loosened = keys
            .ring
            .lock()
            .expect("ring")
            .pin_policy(channel, &mut policy);
        if loosened
            && keys
                .channels
                .lock()
                .expect("channel keys")
                .warned
                .insert(channel)
        {
            self.emit(CoreEvent::Error(
                "The server now says a channel's keys need less approval than before. This device \
                 keeps to the stricter rule."
                    .to_owned(),
            ));
        }
        let state = ChannelState {
            fetched_at: now,
            current: response.current_epoch,
            rotate_needed: response.rotate_needed,
            policy,
        };
        keys.channels
            .lock()
            .expect("channel keys")
            .states
            .insert(channel, state);
        Ok(state)
    }

    /// Checks a key handed to this device and keeps it. Only what its pinned sender signed counts.
    async fn open_sealed_key(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
        sealed: &SealedChannelKey,
    ) {
        let known = |keys: &AccountKeys| {
            keys.directory
                .lock()
                .expect("directory")
                .device(sealed.sender_id, &sealed.sender_device)
                .cloned()
        };
        let mut sender = known(keys);
        if sender.is_none() {
            let _ = self.fetch_devices(inst, keys, &[sealed.sender_id], 0).await;
            sender = known(keys);
        }
        let Some(sender) = sender else {
            tracing::warn!("a key of channel {channel} came from an unknown device");
            return;
        };
        match e2ee::open_channel_key(&keys.identity, channel, sealed, &sender) {
            Ok(key) => keys
                .ring
                .lock()
                .expect("ring")
                .insert(channel, sealed.epoch, key),
            Err(err) => tracing::warn!("a key of channel {channel} failed to open: {err}"),
        }
    }

    /// Starts the next key, for everyone it is meant for. Someone else being quicker is fine.
    async fn start_epoch(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
        state: ChannelState,
    ) -> Result<()> {
        self.refuse_rollback(inst, keys, channel, state).await?;
        let rest = self.rest(inst)?;
        let members = rest.channel_key_members(channel).await?;
        let mut users: Vec<Id> = match state.policy.handover {
            KeyHandover::Automatic => members.users,
            // Nobody gets a key without having been approved: only those who hold it stay.
            KeyHandover::Approved => members
                .users
                .into_iter()
                .filter(|user| members.holders.contains(user))
                .collect(),
        };
        if !users.contains(&keys.me) {
            users.push(keys.me);
        }
        self.fetch_devices(inst, keys, &users, DEVICES_MAX_AGE_MS)
            .await?;
        let epoch = state.current + 1;
        let key = e2ee::new_channel_key();
        let mut sealed = Vec::new();
        {
            let directory = keys.directory.lock().expect("directory");
            for user in users {
                for device in directory.live_devices(user) {
                    sealed.push(
                        e2ee::seal_channel_key(
                            &keys.identity,
                            keys.me,
                            channel,
                            epoch,
                            &key,
                            user,
                            &device,
                        )
                        .map_err(locked)?,
                    );
                }
            }
            if !sealed.iter().any(|k| k.device == keys.identity.device_id()) {
                let device = keys.identity.public(keys.me);
                sealed.push(
                    e2ee::seal_channel_key(
                        &keys.identity,
                        keys.me,
                        channel,
                        epoch,
                        &key,
                        keys.me,
                        &device,
                    )
                    .map_err(locked)?,
                );
            }
        }
        let rest_of_keys = sealed.split_off(sealed.len().min(MAX_CHANNEL_KEYS));
        keys.ring.lock().expect("ring").insert(channel, epoch, key);
        if let Err(err) = rest.start_channel_epoch(channel, epoch, sealed).await {
            keys.ring.lock().expect("ring").remove(channel, epoch);
            Self::forget_state(keys, channel);
            return match err {
                rest::Error::Api(api) if api.code == ErrorCode::Conflict => Ok(()),
                err => Err(err.into()),
            };
        }
        for chunk in rest_of_keys.chunks(MAX_CHANNEL_KEYS) {
            rest.deliver_channel_keys(channel, chunk.to_vec()).await?;
        }
        Self::forget_state(keys, channel);
        Ok(())
    }

    /// Refuses a server that offers an older key than this device already holds: it would send
    /// messages back to a key someone who has left may still have.
    async fn refuse_rollback(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
        state: ChannelState,
    ) -> Result<()> {
        let newest = keys.ring.lock().expect("ring").newest_epoch(channel);
        if newest <= state.current {
            return Ok(());
        }
        // A key may have arrived since this was asked.
        let fresh = self.channel_state(inst, keys, channel, 0).await?;
        if newest > fresh.current {
            return Err(CoreError::Voice(
                "The server offered an older key for this channel than this device has, so nothing \
                 was sent."
                    .to_owned(),
            ));
        }
        Ok(())
    }

    /// The key to write in a channel with. Makes one if there is none or it has to be replaced.
    pub(crate) async fn key_for_sending(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
    ) -> Result<(u32, Zeroizing<[u8; 32]>)> {
        let mut state = self
            .channel_state(inst, keys, channel, self.inner.channel_state_max_age_ms)
            .await?;
        self.refuse_rollback(inst, keys, channel, state).await?;
        if state.current == 0 || state.rotate_needed {
            self.start_epoch(inst, keys, channel, state).await?;
            state = self.channel_state(inst, keys, channel, 0).await?;
        }
        let key = keys.ring.lock().expect("ring").get(channel, state.current);
        match key {
            Some(key) => Ok((state.current, key)),
            None => {
                self.request_keys(inst, keys, channel).await;
                Err(CoreError::Voice(
                    "This channel's keys have not been shared with this device yet".into(),
                ))
            }
        }
    }

    /// The key a message was written with, if this device has it; otherwise asks for it.
    pub(crate) async fn key_for_reading(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
        epoch: u32,
    ) -> Option<Zeroizing<[u8; 32]>> {
        if let Some(key) = keys.ring.lock().expect("ring").get(channel, epoch) {
            return Some(key);
        }
        let _ = self
            .channel_state(inst, keys, channel, self.inner.channel_state_max_age_ms)
            .await;
        let key = keys.ring.lock().expect("ring").get(channel, epoch);
        if key.is_none() {
            self.request_keys(inst, keys, channel).await;
        }
        key
    }

    /// Tells the channel this device lacks its keys.
    async fn request_keys(&self, inst: &str, keys: &AccountKeys, channel: Id) {
        {
            let now = now_ms();
            let mut cache = keys.channels.lock().expect("channel keys");
            if cache
                .asked_at
                .get(&channel)
                .is_some_and(|at| now - at < REQUEST_EVERY_MS)
            {
                return;
            }
            cache.asked_at.insert(channel, now);
        }
        let Ok(rest) = self.rest(inst) else {
            return;
        };
        if let Err(err) = rest
            .request_channel_keys(channel, keys.identity.device_id())
            .await
        {
            tracing::warn!("could not ask for the keys of channel {channel}: {err}");
        }
    }

    /// A device lacks a channel's keys.
    pub(crate) fn key_requested(&self, inst: &str, request: KeyRequest) {
        let (core, inst) = (self.clone(), inst.to_owned());
        tokio::spawn(async move {
            let Some(keys) = core.account_keys(&inst) else {
                return;
            };
            // Whoever draws the shortest wait serves it; the others then find it served.
            tokio::time::sleep(Duration::from_millis(rand::random_range(
                0..SERVE_JITTER_MS,
            )))
            .await;
            if let Err(err) = core.serve_request(&inst, &keys, &request, true).await {
                tracing::warn!("could not hand over keys: {err}");
            }
        });
    }

    /// New keys wait for this device: takes them and opens what could not be read.
    pub(crate) fn keys_delivered(&self, inst: &str, channel: Id) {
        let (core, inst) = (self.clone(), inst.to_owned());
        tokio::spawn(async move {
            let Some(keys) = core.account_keys(&inst) else {
                return;
            };
            Self::forget_state(&keys, channel);
            if let Err(err) = core.channel_state(&inst, &keys, channel, 0).await {
                tracing::warn!("could not fetch the keys of channel {channel}: {err}");
                return;
            }
            let open = core
                .store()
                .instance(&inst)
                .is_some_and(|s| s.window(channel).is_some());
            if open {
                let _ = core
                    .load_history(&inst, channel, HistoryAnchor::Latest)
                    .await;
            }
        });
    }

    /// The channel moved to another key.
    pub(crate) fn key_epoch_changed(&self, inst: &str, channel: Id) {
        if let Some(keys) = self.account_keys(inst) {
            Self::forget_state(&keys, channel);
        }
    }

    /// After `ready`: serves the devices that waited while I was away.
    pub(crate) async fn serve_waiting_requests(&self, inst: &str, keys: &Arc<AccountKeys>) {
        let Ok(rest) = self.rest(inst) else {
            return;
        };
        let requests = match rest.waiting_key_requests().await {
            Ok(requests) => requests,
            Err(err) => {
                tracing::debug!("could not list who waits for keys: {err}");
                return;
            }
        };
        for request in requests {
            if let Err(err) = self.serve_request(inst, keys, &request, false).await {
                tracing::warn!("could not hand over keys: {err}");
            }
        }
    }

    /// Answers a request the way the channel's rules say, if this device can. `check` first asks
    /// whether someone else answered during the wait.
    async fn serve_request(
        &self,
        inst: &str,
        keys: &AccountKeys,
        request: &KeyRequest,
        check: bool,
    ) -> Result<()> {
        let channel = request.channel_id;
        if request.device == keys.identity.device_id() {
            return Ok(());
        }
        let state = self.channel_state(inst, keys, channel, 0).await?;
        if keys.ring.lock().expect("ring").epochs(channel).is_empty() {
            return Ok(());
        }
        if check {
            let waiting = self.rest(inst)?.waiting_key_requests().await?;
            if !waiting.contains(request) {
                return Ok(());
            }
        }
        if state.policy.handover == KeyHandover::Approved {
            let mut cache = keys.channels.lock().expect("channel keys");
            if !cache.waiting.contains(request) {
                cache.waiting.push(request.clone());
            }
            let new = cache.waiting.len();
            drop(cache);
            self.changed(inst, [Changed::Channel(channel)]);
            let can_approve = self.store().instance(inst).is_some_and(|s| {
                s.space_of(channel)
                    .is_some_and(|space| view::can_in_space(s, space, Permissions::MANAGE_SPACE))
            });
            if can_approve {
                self.emit(CoreEvent::Info(format!(
                    "Waiting for you to share keys ({new}): Server settings, Encryption"
                )));
            }
            return Ok(());
        }
        self.hand_over(inst, keys, request, state).await
    }

    /// Gives a device what the channel's rules allow. Where the past stays hidden, that is a new
    /// key and nothing older.
    async fn hand_over(
        &self,
        inst: &str,
        keys: &AccountKeys,
        request: &KeyRequest,
        state: ChannelState,
    ) -> Result<()> {
        let channel = request.channel_id;
        if !state.policy.new_members_read_history {
            return self.start_epoch(inst, keys, channel, state).await;
        }
        self.fetch_devices(inst, keys, &[request.user_id], 0)
            .await?;
        let device = keys
            .directory
            .lock()
            .expect("directory")
            .live_devices(request.user_id)
            .into_iter()
            .find(|d| d.device_id == request.device);
        let Some(device) = device else {
            return Ok(());
        };
        let held: Vec<(u32, Zeroizing<[u8; 32]>)> = {
            let ring = keys.ring.lock().expect("ring");
            ring.epochs(channel)
                .into_iter()
                .filter_map(|epoch| Some((epoch, ring.get(channel, epoch)?)))
                .collect()
        };
        let mut sealed = Vec::with_capacity(held.len());
        for (epoch, key) in held {
            sealed.push(
                e2ee::seal_channel_key(
                    &keys.identity,
                    keys.me,
                    channel,
                    epoch,
                    &key,
                    request.user_id,
                    &device,
                )
                .map_err(locked)?,
            );
        }
        for chunk in sealed.chunks(MAX_CHANNEL_KEYS) {
            self.rest(inst)?
                .deliver_channel_keys(channel, chunk.to_vec())
                .await?;
        }
        Ok(())
    }

    /// Those waiting for someone to approve them, in channels of this instance.
    pub fn key_requests(&self, inst: &str) -> Vec<KeyRequest> {
        // Called while drawing, with the store locked: account_keys would lock it again.
        let keys = self
            .inner
            .accounts
            .lock()
            .expect("accounts")
            .get(inst)
            .cloned();
        keys.map_or_else(Vec::new, |keys| {
            keys.channels.lock().expect("channel keys").waiting.clone()
        })
    }

    /// Hands the keys over by hand: the only way in for an approved channel.
    pub async fn approve_key_request(&self, inst: &str, request: &KeyRequest) -> Result<()> {
        let keys = self
            .account_keys(inst)
            .ok_or_else(|| CoreError::Voice("Not signed in yet".into()))?;
        let state = self
            .channel_state(inst, &keys, request.channel_id, 0)
            .await?;
        self.hand_over(inst, &keys, request, state).await?;
        keys.channels
            .lock()
            .expect("channel keys")
            .waiting
            .retain(|r| r != request);
        self.changed(inst, [Changed::Channel(request.channel_id)]);
        Ok(())
    }

    /// Turns a request down for now: it disappears here until the person asks again.
    pub fn dismiss_key_request(&self, inst: &str, request: &KeyRequest) {
        if let Some(keys) = self.account_keys(inst) {
            keys.channels
                .lock()
                .expect("channel keys")
                .waiting
                .retain(|r| r != request);
        }
        self.changed(inst, [Changed::Channel(request.channel_id)]);
    }

    pub async fn space_key_policy(&self, inst: &str, space: Id) -> Result<KeyPolicy> {
        Ok(self.rest(inst)?.space_key_policy(space).await?)
    }

    pub async fn set_space_key_policy(
        &self,
        inst: &str,
        space: Id,
        policy: KeyPolicy,
    ) -> Result<()> {
        self.rest(inst)?.set_space_key_policy(space, policy).await?;
        if let Some(keys) = self.account_keys(inst) {
            let channels: Vec<Id> = self.store().instance(inst).map_or_else(Vec::new, |s| {
                s.spaces
                    .get(&space)
                    .map_or_else(Vec::new, |state| state.channels.keys().copied().collect())
            });
            let mut ring = keys.ring.lock().expect("ring");
            for channel in channels {
                ring.unpin_policy(channel);
            }
        }
        Ok(())
    }

    /// The messages I saved on `inst`, the latest saved first.
    pub fn saved_messages(&self, inst: &str) -> Vec<SavedMessage> {
        let Some(keys) = self.account_keys(inst) else {
            return Vec::new();
        };
        let ring = keys.ring.lock().expect("ring");
        ring.pins.saved.iter().rev().cloned().collect()
    }

    pub fn is_saved(&self, inst: &str, message: Id) -> bool {
        self.account_keys(inst).is_some_and(|keys| {
            let ring = keys.ring.lock().expect("ring");
            ring.pins.saved.iter().any(|s| s.message == message)
        })
    }

    /// Saves a message (sealed, on this device), or with `None` forgets `message`. False when
    /// this instance keeps nothing sealed here (not signed in).
    pub fn set_saved(&self, inst: &str, message: Id, saved: Option<SavedMessage>) -> bool {
        let Some(keys) = self.account_keys(inst) else {
            return false;
        };
        let mut ring = keys.ring.lock().expect("ring");
        ring.pins.saved.retain(|s| s.message != message);
        if let Some(saved) = saved {
            ring.pins.saved.push(saved);
            let extra = ring.pins.saved.len().saturating_sub(MAX_SAVED);
            ring.pins.saved.drain(..extra);
        }
        ring.save();
        true
    }

    /// What was being written in a conversation.
    pub fn draft(&self, inst: &str, channel: Id) -> Option<String> {
        let keys = self.account_keys(inst)?;
        let ring = keys.ring.lock().expect("ring");
        ring.pins.drafts.get(&channel).cloned()
    }

    /// Keeps what is being written, sealed; an empty text forgets it.
    pub fn set_draft(&self, inst: &str, channel: Id, text: &str) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        let mut ring = keys.ring.lock().expect("ring");
        let changed = if text.trim().is_empty() {
            ring.pins.drafts.remove(&channel).is_some()
        } else if ring.pins.drafts.get(&channel).map(String::as_str) == Some(text) {
            false
        } else {
            ring.pins.drafts.insert(channel, text.to_owned());
            true
        };
        if changed {
            ring.save();
        }
    }

    /// Keeps what has not been sent yet, sealed, for the next start. Files stay behind: they
    /// are uploaded before a message waits.
    pub fn save_outbox(&self) {
        for inst in self.instance_ids() {
            let Some(keys) = self.account_keys(&inst) else {
                continue;
            };
            let waiting: Vec<(Id, String, Option<Id>)> =
                self.store().instance(&inst).map_or_else(Vec::new, |s| {
                    s.windows
                        .iter()
                        .flat_map(|(channel, w)| {
                            w.pending
                                .iter()
                                .filter(|p| p.state == crate::store::PendingState::Queued)
                                .filter(|p| p.attachments.is_empty())
                                .map(|p| (*channel, p.content.clone(), p.reply_to))
                        })
                        .collect()
                });
            let mut ring = keys.ring.lock().expect("ring");
            if ring.pins.outbox != waiting {
                ring.pins.outbox = waiting;
                ring.save();
            }
        }
    }

    /// Sends again what was waiting when Hermes closed.
    pub(crate) fn resend_outbox(&self, inst: &str) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        let waiting = {
            let mut ring = keys.ring.lock().expect("ring");
            let waiting = std::mem::take(&mut ring.pins.outbox);
            if !waiting.is_empty() {
                ring.save();
            }
            waiting
        };
        for (channel, content, reply_to) in waiting {
            let draft = crate::core::Draft {
                content,
                reply_to,
                attachments: Vec::new(),
            };
            if let Err(err) = self.send_message(inst, channel, draft) {
                tracing::warn!("a waiting message could not be queued: {err}");
            }
        }
    }

    /// My note on someone.
    pub fn note(&self, inst: &str, user: Id) -> Option<String> {
        let keys = self.account_keys(inst)?;
        let ring = keys.ring.lock().expect("ring");
        ring.pins.notes.get(&user).cloned()
    }

    pub fn set_note(&self, inst: &str, user: Id, text: &str) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        let mut ring = keys.ring.lock().expect("ring");
        if text.trim().is_empty() {
            ring.pins.notes.remove(&user);
        } else {
            ring.pins.notes.insert(user, text.to_owned());
        }
        ring.save();
    }

    /// The name I gave someone, shown only to me.
    pub fn local_name(&self, inst: &str, user: Id) -> Option<String> {
        let keys = self.account_keys(inst)?;
        let ring = keys.ring.lock().expect("ring");
        ring.pins.local_names.get(&user).cloned()
    }

    pub fn set_local_name(&self, inst: &str, user: Id, name: &str) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        {
            let mut ring = keys.ring.lock().expect("ring");
            if name.trim().is_empty() {
                ring.pins.local_names.remove(&user);
            } else {
                ring.pins.local_names.insert(user, name.trim().to_owned());
            }
            ring.save();
        }
        self.load_local_names(inst);
    }

    /// Brings the names I gave people into the store.
    pub(crate) fn load_local_names(&self, inst: &str) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        let names: HashMap<Id, String> = keys
            .ring
            .lock()
            .expect("ring")
            .pins
            .local_names
            .iter()
            .map(|(id, name)| (*id, name.clone()))
            .collect();
        self.with_store(inst, |s| s.local_names = names);
        self.changed(inst, [Changed::Users]);
    }

    pub async fn channel_key_policy(&self, inst: &str, channel: Id) -> Result<KeyPolicyOverride> {
        Ok(self.rest(inst)?.channel_key_policy(channel).await?)
    }

    pub async fn set_channel_key_policy(
        &self,
        inst: &str,
        channel: Id,
        policy: KeyPolicyOverride,
    ) -> Result<()> {
        self.rest(inst)?
            .set_channel_key_policy(channel, policy)
            .await?;
        if let Some(keys) = self.account_keys(inst) {
            keys.ring.lock().expect("ring").unpin_policy(channel);
        }
        Ok(())
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;

    fn ring() -> KeyRing {
        KeyRing::load(None, Zeroizing::new([7; 32]))
    }

    #[test]
    fn a_stricter_rule_stays_until_i_change_it() {
        let mut ring = ring();
        let strict = KeyPolicy {
            handover: KeyHandover::Approved,
            new_members_read_history: false,
        };
        let mut seen = strict;
        assert!(!ring.pin_policy(Id(1), &mut seen));
        let mut looser = KeyPolicy {
            handover: KeyHandover::Automatic,
            new_members_read_history: true,
        };
        assert!(ring.pin_policy(Id(1), &mut looser));
        assert_eq!(looser.handover, KeyHandover::Approved);
        assert!(!looser.new_members_read_history);
        ring.unpin_policy(Id(1));
        let mut looser = KeyPolicy {
            handover: KeyHandover::Automatic,
            new_members_read_history: true,
        };
        assert!(!ring.pin_policy(Id(1), &mut looser));
    }

    #[test]
    fn a_key_held_is_never_replaced() {
        let mut ring = ring();
        ring.insert(Id(1), 1, Zeroizing::new([1; 32]));
        ring.insert(Id(1), 1, Zeroizing::new([2; 32]));
        assert_eq!(*ring.get(Id(1), 1).unwrap(), [1; 32]);
        assert_eq!(ring.newest_epoch(Id(1)), 1);
    }

    #[test]
    fn the_first_sealed_message_only_moves_back() {
        let mut ring = ring();
        ring.note_sealed(Id(1), Id(50));
        ring.note_sealed(Id(1), Id(60));
        assert_eq!(ring.sealed_since(Id(1)), Some(Id(50)));
        ring.note_sealed(Id(1), Id(40));
        assert_eq!(ring.sealed_since(Id(1)), Some(Id(40)));
    }
}
