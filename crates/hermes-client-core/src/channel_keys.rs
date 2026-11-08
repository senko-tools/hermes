//! The keys of encrypted server channels in the client's flow (docs/e2ee.md). Each channel has
//! numbered keys, kept here in a key ring. Whoever writes first makes the first one; a new one
//! replaces it when someone who held it can no longer read the channel; members hand them to
//! devices that lack them, by themselves or only when someone approves, as the channel's rules say.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hermes_proto::ErrorCode;
use hermes_proto::e2ee::{
    KeyHandover, KeyPolicy, KeyPolicyOverride, KeyRequest, MAX_CHANNEL_KEYS, SealedChannelKey,
};
use hermes_proto::id::Id;
use zeroize::Zeroizing;

use crate::config::write_atomic;
use crate::core::{Core, CoreError, Result, now_ms};
use crate::e2ee;
use crate::rest;
use crate::secure::{AccountKeys, DEVICES_MAX_AGE_MS};
use crate::store::{Changed, HistoryAnchor};

/// What the server said about a channel's keys is used for this long.
const STATE_MAX_AGE_MS: i64 = 15_000;
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
}

impl KeyRing {
    pub(crate) fn load(path: Option<PathBuf>, wrap: Zeroizing<[u8; 32]>) -> KeyRing {
        let mut keys = BTreeMap::new();
        let stored = path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|sealed| e2ee::open_local(&wrap, &sealed).ok())
            .and_then(|plain| serde_json::from_slice::<Vec<(Id, u32, String)>>(&plain).ok());
        for (channel, epoch, key) in stored.unwrap_or_default() {
            if let Ok(key) = e2ee::decode_key(&key) {
                keys.insert((channel, epoch), key);
            }
        }
        KeyRing { path, wrap, keys }
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
        if self.keys.insert((channel, epoch), key).is_none() {
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
        let Ok(plain) = serde_json::to_vec(&list).map(Zeroizing::new) else {
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
}

fn locked(err: impl std::fmt::Display) -> CoreError {
    CoreError::Voice(format!("Could not encrypt: {err}"))
}

impl Core {
    /// Whether what is written in this channel is encrypted.
    pub fn channel_is_encrypted(&self, inst: &str, channel: Id) -> bool {
        if self.channel_is_private(inst, channel) {
            return true;
        }
        let store = self.store();
        let Some(s) = store.instance(inst) else {
            return false;
        };
        let required = s
            .instance
            .as_ref()
            .is_some_and(|i| i.limits.channels_encrypted);
        required && s.channel(channel).is_some_and(|c| c.kind.has_messages())
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
        let state = ChannelState {
            fetched_at: now,
            current: response.current_epoch,
            rotate_needed: response.rotate_needed,
            policy: response.policy,
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

    /// The key to write in a channel with. Makes one if there is none or it has to be replaced.
    pub(crate) async fn key_for_sending(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
    ) -> Result<(u32, Zeroizing<[u8; 32]>)> {
        let mut state = self
            .channel_state(inst, keys, channel, STATE_MAX_AGE_MS)
            .await?;
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
            .channel_state(inst, keys, channel, STATE_MAX_AGE_MS)
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
            drop(cache);
            self.changed(inst, [Changed::Channel(channel)]);
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
        self.account_keys(inst).map_or_else(Vec::new, |keys| {
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
        Ok(self.rest(inst)?.set_space_key_policy(space, policy).await?)
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
        Ok(self
            .rest(inst)?
            .set_channel_key_policy(channel, policy)
            .await?)
    }
}
