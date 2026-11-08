//! End-to-end encryption in the client's flow (docs/e2ee.md; the cryptography is in [`crate::e2ee`]):
//! this device's keys, everyone's public keys, and sealing and opening messages and files.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use hermes_proto::e2ee::{DeviceKeys, MessageLock, SealedFile};
use hermes_proto::id::Id;
use hermes_proto::models::{Attachment, ChannelKind, Message, MessageKind};

use crate::core::{Core, CoreError, InstanceId, Result, content_type, now_ms};
use crate::e2ee::{self, Directory, E2eeError, Identity, Inner, InnerAttachment};
use crate::store::Changed;

/// Devices asked for longer ago than this are asked for again before sending to them. Changes are
/// announced too, so this only matters for an announcement that got lost.
pub(crate) const DEVICES_MAX_AGE_MS: i64 = 60_000;
/// What an encrypted file is called on the server, which never learns the real name.
const STORED_NAME: &str = "encrypted.bin";

pub const NOT_FOR_THIS_DEVICE: &str =
    "Encrypted before this device was signed in, so it cannot be read here.";
pub const TAMPERED: &str = "This encrypted message could not be verified, so it is not shown.";

/// This account's keys on one instance.
pub(crate) struct AccountKeys {
    pub(crate) me: Id,
    pub(crate) identity: Identity,
    pub(crate) directory: Mutex<Directory>,
    /// The keys of encrypted server channels.
    pub(crate) ring: Mutex<crate::channel_keys::KeyRing>,
    pub(crate) channels: Mutex<crate::channel_keys::ChannelKeyCache>,
}

/// The keys of each instance's account, from its first `ready` on.
pub(crate) type Accounts = Mutex<HashMap<InstanceId, Arc<AccountKeys>>>;

/// What the encryption of a conversation shows to the people in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationKeys {
    /// Compare this with the others over another channel.
    pub safety_number: String,
    /// `(user, device IDs)` of everyone in it.
    pub devices: Vec<(Id, Vec<String>)>,
}

fn safe_file_name(inst: &str) -> String {
    inst.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

impl Core {
    fn secret_slot(me: Id) -> String {
        format!("e2ee-device-{me}")
    }

    /// This account's keys on `inst`, made on first use. `None` until signed in.
    pub(crate) fn account_keys(&self, inst: &str) -> Option<Arc<AccountKeys>> {
        let me = self.store().instance(inst).and_then(|s| s.me_id())?;
        let mut accounts = self.inner.accounts.lock().expect("accounts");
        if let Some(keys) = accounts.get(inst)
            && keys.me == me
        {
            return Some(keys.clone());
        }
        let slot = Self::secret_slot(me);
        let tokens = self.tokens();
        let identity = tokens
            .get(inst, &slot)
            .and_then(|secret| Identity::from_secret(&secret));
        let identity = identity.unwrap_or_else(|| {
            let identity = Identity::generate();
            if let Err(err) = tokens.set(inst, &slot, &identity.to_secret()) {
                tracing::error!("could not keep this device's encryption key: {err}");
            }
            identity
        });
        let path = self.paths().map(|paths| {
            paths
                .dir
                .join("e2ee")
                .join(format!("{}-{me}.json", safe_file_name(inst)))
        });
        let ring_slot = format!("e2ee-ring-{me}");
        let wrap = tokens
            .get(inst, &ring_slot)
            .and_then(|secret| e2ee::decode_key(&secret).ok())
            .unwrap_or_else(|| {
                let wrap = e2ee::new_channel_key();
                if let Err(err) = tokens.set(inst, &ring_slot, &e2ee::encode_key(&wrap)) {
                    tracing::error!("could not keep this device's channel key ring: {err}");
                }
                wrap
            });
        let ring_path = self.paths().map(|paths| {
            paths
                .dir
                .join("e2ee")
                .join(format!("{}-{me}-channels.bin", safe_file_name(inst)))
        });
        let keys = Arc::new(AccountKeys {
            me,
            identity,
            directory: Mutex::new(Directory::load(path)),
            ring: Mutex::new(crate::channel_keys::KeyRing::load(ring_path, wrap)),
            channels: Mutex::default(),
        });
        accounts.insert(inst.to_owned(), keys.clone());
        Some(keys)
    }

    /// After `ready`: publishes this device's public keys.
    pub(crate) fn publish_device(&self, inst: &str) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        let Ok(rest) = self.rest(inst) else {
            return;
        };
        let (core, inst) = (self.clone(), inst.to_owned());
        tokio::spawn(async move {
            let request = keys.identity.registration(keys.me);
            if let Err(err) = rest.register_device(&request).await {
                tracing::warn!("could not publish this device's encryption keys: {err}");
            }
            // My own other devices get a copy of what I send.
            let _ = core.fetch_devices(&inst, &keys, &[keys.me], 0).await;
            core.serve_waiting_requests(&inst, &keys).await;
        });
    }

    /// Someone's devices changed. Pinned here, so a change is noticed even if I never write to them.
    pub(crate) fn devices_changed(&self, inst: &str, user: Id, devices: Vec<DeviceKeys>) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        keys.directory
            .lock()
            .expect("directory")
            .update(user, devices, now_ms());
        self.changed(inst, [Changed::Users]);
    }

    /// Asks the server for the devices of those not asked about for `max_age_ms`.
    pub(crate) async fn fetch_devices(
        &self,
        inst: &str,
        keys: &AccountKeys,
        users: &[Id],
        max_age_ms: i64,
    ) -> Result<()> {
        let now = now_ms();
        let stale: Vec<Id> = {
            let directory = keys.directory.lock().expect("directory");
            users
                .iter()
                .copied()
                .filter(|user| {
                    directory
                        .fetched_at(*user)
                        .is_none_or(|at| now - at >= max_age_ms)
                })
                .collect()
        };
        if stale.is_empty() {
            return Ok(());
        }
        let mut found = Vec::new();
        for chunk in stale.chunks(hermes_proto::e2ee::MAX_DEVICE_QUERY) {
            found.extend(self.rest(inst)?.query_devices(chunk).await?);
        }
        let mut directory = keys.directory.lock().expect("directory");
        for user in found {
            directory.update(user.user_id, user.devices, now);
        }
        Ok(())
    }

    /// Everyone in a DM or group DM, me included.
    fn recipients(&self, inst: &str, channel: Id, me: Id) -> Vec<Id> {
        let store = self.store();
        let mut users: Vec<Id> = store
            .instance(inst)
            .and_then(|s| s.dms.get(&channel))
            .map(|c| c.recipients.iter().map(|u| u.id).collect())
            .unwrap_or_default();
        if !users.contains(&me) {
            users.push(me);
        }
        users
    }

    /// Whether what is written in this channel is encrypted.
    pub fn channel_is_private(&self, inst: &str, channel: Id) -> bool {
        self.store()
            .instance(inst)
            .and_then(|s| s.channel(channel))
            .is_some_and(|c| matches!(c.kind, ChannelKind::Dm | ChannelKind::GroupDm))
    }

    /// Who to encrypt to in a conversation. An error names who cannot be written to safely yet: no
    /// message goes out in a form they cannot read, and none goes out in clear.
    async fn devices_for(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
    ) -> Result<Vec<DeviceKeys>> {
        let users = self.recipients(inst, channel, keys.me);
        self.fetch_devices(inst, keys, &users, DEVICES_MAX_AGE_MS)
            .await?;
        let mut devices = vec![keys.identity.public(keys.me)];
        let directory = keys.directory.lock().expect("directory");
        for user in users {
            let live = directory.live_devices(user);
            if live.is_empty() && user != keys.me {
                let name = self
                    .store()
                    .instance(inst)
                    .map_or_else(|| user.to_string(), |s| s.display_name(None, user));
                return Err(CoreError::Voice(format!(
                    "{name} has not opened a version of Hermes with encryption yet, so this cannot \
                     be sent safely"
                )));
            }
            for device in live {
                if !devices.iter().any(|d| d.device_id == device.device_id) {
                    devices.push(device);
                }
            }
        }
        Ok(devices)
    }

    /// Seals what is about to be sent to a DM.
    pub(crate) async fn seal(
        &self,
        inst: &str,
        channel: Id,
        content: &str,
        attachments: &[Attachment],
        reply_to: Option<Id>,
    ) -> Result<hermes_proto::e2ee::Envelope> {
        let keys = self
            .account_keys(inst)
            .ok_or_else(|| CoreError::Voice("Not signed in yet".into()))?;
        let reply_excerpt = reply_to.and_then(|id| {
            let store = self.store();
            let window = store.instance(inst)?.window(channel)?;
            let message = window.messages.iter().find(|m| m.id == id)?;
            Some(message.content.chars().take(100).collect())
        });
        let inner = Inner {
            content: content.to_owned(),
            attachments: attachments.iter().filter_map(inner_attachment).collect(),
            reply_to,
            reply_excerpt,
        };
        let sealed = if self.channel_is_private(inst, channel) {
            let devices = self.devices_for(inst, &keys, channel).await?;
            e2ee::seal_message(&keys.identity, keys.me, channel, &devices, &inner)
        } else {
            let (epoch, key) = self.key_for_sending(inst, &keys, channel).await?;
            e2ee::seal_channel_message(&keys.identity, keys.me, channel, epoch, &key, &inner)
        };
        sealed.map_err(|err| CoreError::Voice(format!("Could not encrypt: {err}")))
    }

    /// Opens a message from the server in place: `content` and the attachments become what the
    /// author wrote, or say why it cannot be shown. The server's copy is dropped either way.
    pub async fn open_message(&self, inst: &str, message: &mut Message) {
        let Some(envelope) = message.e2ee.take() else {
            message.lock = MessageLock::Plain;
            return;
        };
        let Some(keys) = self.account_keys(inst) else {
            refuse(message, MessageLock::NotForThisDevice, NOT_FOR_THIS_DEVICE);
            return;
        };
        let author = message.author.id;
        let known = keys
            .directory
            .lock()
            .expect("directory")
            .device(author, &envelope.device)
            .cloned();
        let sender = match known {
            Some(sender) => Some(sender),
            None => {
                if let Err(err) = self.fetch_devices(inst, &keys, &[author], 0).await {
                    tracing::warn!("could not ask for {author}'s encryption keys: {err}");
                }
                keys.directory
                    .lock()
                    .expect("directory")
                    .device(author, &envelope.device)
                    .cloned()
            }
        };
        let Some(sender) = sender else {
            refuse(message, MessageLock::Tampered, TAMPERED);
            return;
        };
        let opened = match envelope.epoch {
            Some(epoch) => {
                let key = self
                    .key_for_reading(inst, &keys, message.channel_id, epoch)
                    .await;
                let Some(key) = key else {
                    refuse(
                        message,
                        MessageLock::NotForThisDevice,
                        crate::channel_keys::WAITING_FOR_KEYS,
                    );
                    return;
                };
                e2ee::open_channel_message(author, message.channel_id, &sender, &envelope, &key)
            }
            None => e2ee::open_message_keyed(
                &keys.identity,
                author,
                message.channel_id,
                &sender,
                &envelope,
            ),
        };
        match opened {
            Ok((inner, key)) => {
                apply_inner(message, inner);
                open_reactions(message, &key);
                message.message_key = Some(key.to_string());
            }
            Err(E2eeError::NotForThisDevice) => {
                refuse(message, MessageLock::NotForThisDevice, NOT_FOR_THIS_DEVICE);
            }
            Err(err) => {
                tracing::warn!("message {} failed to open: {err}", message.id);
                refuse(message, MessageLock::Tampered, TAMPERED);
            }
        }
    }

    /// Opens a list of messages (history, search results, pins).
    pub async fn open_messages(&self, inst: &str, messages: &mut [Message]) {
        for message in messages {
            self.open_message(inst, message).await;
        }
    }

    /// Uploads a file to attach in `channel`. In a DM it is encrypted first, and the server sees
    /// neither its name nor its type, nor what `info` says of it.
    pub async fn upload_to(
        &self,
        inst: &str,
        channel: Id,
        path: &Path,
        info: FileInfo,
    ) -> Result<Attachment> {
        // The server's limit, checked before anything is sent.
        let refusal = |size: u64| {
            self.store()
                .instance(inst)
                .and_then(|s| s.upload_refusal(size))
                .map_or(Ok(()), |why| Err(CoreError::Io(why)))
        };
        if !self.channel_is_encrypted(inst, channel) {
            let size = tokio::fs::metadata(path)
                .await
                .map_err(|e| CoreError::Io(format!("{}: {e}", path.display())))?
                .len();
            refusal(size)?;
            return self
                .upload_measured(inst, path, info.waveform.as_deref(), info.duration_ms)
                .await;
        }
        let size = info.size;
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| CoreError::Io(format!("{}: {e}", path.display())))?;
        let (sealed, key, nonce, plain_size, compression) =
            tokio::task::spawn_blocking(move || {
                let shrunk = crate::compress::shrink(bytes);
                let (sealed, key, nonce) = e2ee::seal_file(&shrunk.bytes)?;
                Ok::<_, E2eeError>((sealed, key, nonce, shrunk.plain_size, shrunk.compression))
            })
            .await
            .map_err(|e| CoreError::Io(e.to_string()))?
            .map_err(|e| CoreError::Io(e.to_string()))?;
        // Sealed, it is a little larger: that is what the server counts.
        refusal(sealed.len() as u64)?;
        let stored = self
            .rest(inst)?
            .upload(STORED_NAME, "application/octet-stream", sealed)
            .await?;
        let filename = path
            .file_name()
            .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
        Ok(Attachment {
            id: stored.id,
            content_type: content_type(&filename).to_owned(),
            filename,
            size: plain_size,
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            has_thumb: false,
            waveform: info.waveform,
            duration_ms: info.duration_ms,
            sealed: Some(SealedFile {
                key,
                nonce,
                stored_name: stored.filename,
                compression,
            }),
        })
    }

    /// Downloads an attachment, decrypting it if it is encrypted.
    pub async fn download(&self, inst: &str, attachment: &Attachment) -> Result<Vec<u8>> {
        let rest = self.rest(inst)?;
        let Some(sealed) = &attachment.sealed else {
            return Ok(rest.download(attachment.id, &attachment.filename).await?);
        };
        let bytes = rest.download(attachment.id, &sealed.stored_name).await?;
        let (key, nonce) = (sealed.key.clone(), sealed.nonce.clone());
        let (compression, plain_size) = (sealed.compression, attachment.size);
        tokio::task::spawn_blocking(move || {
            let opened = e2ee::open_file(&bytes, &key, &nonce).map_err(|e| e.to_string())?;
            crate::compress::restore(opened, compression, plain_size)
        })
        .await
        .map_err(|e| CoreError::Io(e.to_string()))?
        .map_err(|e| CoreError::Io(format!("The file could not be decrypted: {e}")))
    }

    /// The safety number and devices of a conversation, asking the server for the latest.
    pub async fn conversation_keys(&self, inst: &str, channel: Id) -> Result<ConversationKeys> {
        let keys = self
            .account_keys(inst)
            .ok_or_else(|| CoreError::Voice("Not signed in yet".into()))?;
        let users = self.recipients(inst, channel, keys.me);
        self.fetch_devices(inst, &keys, &users, 0).await?;
        Ok(keys_of(&keys, &users))
    }

    /// The people in a conversation who have a device I have not been told about yet.
    pub fn new_devices(&self, inst: &str, channel: Id) -> Vec<Id> {
        let Some(keys) = self.account_keys(inst) else {
            return Vec::new();
        };
        let users = self.recipients(inst, channel, keys.me);
        let directory = keys.directory.lock().expect("directory");
        users
            .into_iter()
            .filter(|user| directory.unseen(*user) > 0)
            .collect()
    }

    /// I have been told about the new devices in this conversation.
    pub fn acknowledge_devices(&self, inst: &str, channel: Id) {
        let Some(keys) = self.account_keys(inst) else {
            return;
        };
        let users = self.recipients(inst, channel, keys.me);
        let mut directory = keys.directory.lock().expect("directory");
        for user in users {
            directory.acknowledge(user);
        }
    }

    /// This device's ID on `inst`.
    pub fn my_device_id(&self, inst: &str) -> Option<String> {
        Some(self.account_keys(inst)?.identity.device_id().to_owned())
    }
}

/// The safety number and devices of these people.
pub(crate) fn keys_of(keys: &AccountKeys, users: &[Id]) -> ConversationKeys {
    let directory = keys.directory.lock().expect("directory");
    let all: Vec<(Id, Vec<DeviceKeys>)> = users
        .iter()
        .map(|user| {
            let mut live = directory.live_devices(*user);
            if *user == keys.me {
                let mine = keys.identity.public(keys.me);
                if !live.iter().any(|d| d.device_id == mine.device_id) {
                    live.push(mine);
                }
            }
            (*user, live)
        })
        .collect();
    ConversationKeys {
        safety_number: e2ee::safety_number(&all),
        devices: all
            .into_iter()
            .map(|(user, devices)| (user, devices.into_iter().map(|d| d.device_id).collect()))
            .collect(),
    }
}

/// What the uploader knows of a file beyond its bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileInfo {
    /// Its pixels, if it is a picture.
    pub size: Option<(u32, u32)>,
    /// A sound's loudness, a byte per bar, and how long a sound or video lasts.
    pub waveform: Option<Vec<u8>>,
    pub duration_ms: Option<u64>,
}

fn inner_attachment(attachment: &Attachment) -> Option<InnerAttachment> {
    let sealed = attachment.sealed.as_ref()?;
    Some(InnerAttachment {
        id: attachment.id,
        filename: attachment.filename.clone(),
        content_type: attachment.content_type.clone(),
        size: attachment.size,
        width: attachment.width,
        height: attachment.height,
        waveform: attachment.waveform.clone(),
        duration_ms: attachment.duration_ms,
        key: sealed.key.clone(),
        nonce: sealed.nonce.clone(),
        compression: sealed.compression,
    })
}

/// Replaces sealed reactions by their emoji; ones this message's key did not seal are dropped.
fn open_reactions(message: &mut Message, key: &str) {
    message.reactions.retain_mut(|reaction| {
        match e2ee::open_reaction(key, &reaction.emoji) {
            Some(emoji) => {
                reaction.emoji = emoji;
                true
            }
            // From before reactions were sealed.
            None => !reaction.emoji.starts_with(e2ee::REACTION_PREFIX),
        }
    });
}

fn refuse(message: &mut Message, lock: MessageLock, text: &str) {
    message.lock = lock;
    message.content = text.to_owned();
    message.attachments.clear();
    if let Some(reply) = &mut message.reply_to {
        reply.excerpt.clear();
    }
    message.kind = MessageKind::Default;
}

/// Fills a message in from what its envelope says. Only attachments named inside it stay: the
/// server could add others, but it cannot have made them belong.
fn apply_inner(message: &mut Message, inner: Inner) {
    message.lock = MessageLock::Sealed;
    message.content = inner.content;
    let stored = std::mem::take(&mut message.attachments);
    for attachment in inner.attachments {
        let Some(on_server) = stored.iter().find(|a| a.id == attachment.id) else {
            continue;
        };
        message.attachments.push(Attachment {
            id: attachment.id,
            filename: attachment.filename,
            content_type: attachment.content_type,
            size: attachment.size,
            width: attachment.width,
            height: attachment.height,
            has_thumb: false,
            waveform: attachment.waveform,
            duration_ms: attachment.duration_ms,
            sealed: Some(SealedFile {
                key: attachment.key,
                nonce: attachment.nonce,
                stored_name: on_server.filename.clone(),
                compression: attachment.compression,
            }),
        });
    }
    match (&mut message.reply_to, inner.reply_to) {
        (Some(reply), Some(id)) if reply.id == id => {
            reply.excerpt = inner.reply_excerpt.unwrap_or_default();
        }
        // The server pointed it at something the author did not answer.
        (reply @ Some(_), _) => *reply = None,
        _ => {}
    }
}
