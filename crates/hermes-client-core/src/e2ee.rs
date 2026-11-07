//! End-to-end encryption (docs/e2ee.md): what the server must not be able to read is sealed here,
//! before it leaves this device, and opened here when it arrives.
//!
//! - Every device has an Ed25519 key (signs) and an X25519 key (is encrypted to), created by
//!   itself, kept in the OS credential store and never sent anywhere. The server only hands out
//!   the public parts, so the first key seen for someone is pinned ([`Directory`]) and a new device
//!   afterwards raises a notice.
//! - A message is encrypted with a random key, which is sealed separately for each device of each
//!   recipient with a one-time X25519 key (HKDF-SHA256 and ChaCha20-Poly1305), and the whole is
//!   signed by the sending device. No ratchet: messages live on the server and any of a person's
//!   devices must be able to read them later, so a stolen device key opens its past messages.
//! - Voice: every participant has a random sender key, sent to the others sealed the same way and
//!   replaced whenever someone joins or leaves. Voice, stream sound and stream video each use a key
//!   derived from it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, XChaCha20Poly1305};
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use hermes_proto::e2ee::{
    DeviceKeys, Envelope, RegisterDeviceRequest, VERSION, VoiceKey, WrappedKey,
};
use hermes_proto::id::Id;
use hermes_proto::voice::{AudioHeader, AudioKind, HEADER_LEN, MAX_DATAGRAM, VIDEO_HEADER_LEN};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

use crate::config::write_atomic;

const WRAP_INFO: &[u8] = b"hermes-e2ee/v1/wrap";
const MESSAGE_SIG: &[u8] = b"hermes-e2ee/v1/message";
const MESSAGE_AAD: &[u8] = b"hermes-e2ee/v1/message-aad";
const VOICE_SIG: &[u8] = b"hermes-e2ee/v1/voice-key";
const DEVICE_SIG: &[u8] = b"hermes-e2ee/v1/device";
const SAFETY: &[u8] = b"hermes-e2ee/v1/safety-number";
const FILE_AAD: &[u8] = b"hermes-e2ee/v1/file";
/// Message plaintext is padded to a multiple of this, so its length says little.
const MESSAGE_PADDING: usize = 256;
/// Voice plaintext is padded to a multiple of this: the size of an Opus frame follows what is said.
const VOICE_PADDING: usize = 32;
/// Which of a participant's old keys are kept, for packets still on their way.
const KEPT_GENERATIONS: u8 = 3;
/// The generation and packet counter in front of sealed voice.
const SEAL_PREFIX: usize = 7;
const TAG_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eeError {
    Malformed,
    /// Sealed, but not for this device.
    NotForThisDevice,
    /// The signature or the data does not hold up.
    Tampered,
    /// Nobody to send to: the named person has no device yet.
    NoDevices(Id),
}

impl std::fmt::Display for E2eeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            E2eeError::Malformed => f.write_str("malformed encrypted data"),
            E2eeError::NotForThisDevice => f.write_str("not encrypted for this device"),
            E2eeError::Tampered => f.write_str("the encrypted data was changed"),
            E2eeError::NoDevices(_) => f.write_str("this person has no encryption keys yet"),
        }
    }
}

fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

fn unb64(text: &str) -> Result<Vec<u8>, E2eeError> {
    B64.decode(text).map_err(|_| E2eeError::Malformed)
}

fn unb64_array<const N: usize>(text: &str) -> Result<[u8; N], E2eeError> {
    unb64(text)?.try_into().map_err(|_| E2eeError::Malformed)
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    rand::fill(&mut bytes);
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A device's ID: a hash of its signing key, so an ID cannot be claimed for another key.
pub fn device_id(sign_key: &[u8; 32]) -> String {
    hex(&Sha256::digest(sign_key)[..16])
}

/// Length-prefixed fields, so that no two different lists of fields sign alike.
fn transcript(domain: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut hash = Sha256::new();
    for field in std::iter::once(domain).chain(fields.iter().copied()) {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    hash.finalize().to_vec()
}

/// This device's secret keys.
pub struct Identity {
    sign: SigningKey,
    dh: StaticSecret,
    device_id: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

impl Identity {
    pub fn generate() -> Identity {
        Identity::from_seeds(random(), random())
    }

    fn from_seeds(mut sign: [u8; 32], mut dh: [u8; 32]) -> Identity {
        let signing = SigningKey::from_bytes(&sign);
        let secret = StaticSecret::from(dh);
        sign.zeroize();
        dh.zeroize();
        let device_id = device_id(signing.verifying_key().as_bytes());
        Identity {
            sign: signing,
            dh: secret,
            device_id,
        }
    }

    /// What is kept in the credential store.
    pub fn to_secret(&self) -> Zeroizing<String> {
        let mut seeds = Zeroizing::new([0u8; 64]);
        seeds[..32].copy_from_slice(&self.sign.to_bytes());
        seeds[32..].copy_from_slice(&self.dh.to_bytes());
        Zeroizing::new(b64(&*seeds))
    }

    pub fn from_secret(secret: &str) -> Option<Identity> {
        let bytes = Zeroizing::new(B64.decode(secret).ok()?);
        if bytes.len() != 64 {
            return None;
        }
        let mut sign = [0u8; 32];
        let mut dh = [0u8; 32];
        sign.copy_from_slice(&bytes[..32]);
        dh.copy_from_slice(&bytes[32..]);
        Some(Identity::from_seeds(sign, dh))
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// My public keys, signed for my account.
    pub fn public(&self, user: Id) -> DeviceKeys {
        let sign_key = self.sign.verifying_key().to_bytes();
        let dh_key = PublicKey::from(&self.dh).to_bytes();
        let signature = self
            .sign
            .sign(&transcript(DEVICE_SIG, &[&user.0.to_be_bytes(), &dh_key]));
        DeviceKeys {
            device_id: self.device_id.clone(),
            sign_key: b64(&sign_key),
            dh_key: b64(&dh_key),
            signature: b64(&signature.to_bytes()),
            revoked: false,
        }
    }

    pub fn registration(&self, user: Id) -> RegisterDeviceRequest {
        let keys = self.public(user);
        RegisterDeviceRequest {
            device_id: keys.device_id,
            sign_key: keys.sign_key,
            dh_key: keys.dh_key,
            signature: keys.signature,
        }
    }

    /// Opens what was sealed for this device with [`wrap_for`].
    fn unwrap(
        &self,
        eph: &[u8; 32],
        keys: &[WrappedKey],
    ) -> Result<Zeroizing<[u8; 32]>, E2eeError> {
        let wrapped = keys
            .iter()
            .find(|k| k.device == self.device_id)
            .ok_or(E2eeError::NotForThisDevice)?;
        let wrapped = unb64(&wrapped.key)?;
        let my_public = PublicKey::from(&self.dh);
        let shared = self.dh.diffie_hellman(&PublicKey::from(*eph));
        if !shared.was_contributory() {
            return Err(E2eeError::Tampered);
        }
        let wrap_key = wrap_key(shared.as_bytes(), eph, my_public.as_bytes());
        let opened = wrap_cipher(&wrap_key)
            .decrypt(
                &[0u8; 12].into(),
                Payload {
                    msg: &wrapped,
                    aad: self.device_id.as_bytes(),
                },
            )
            .map_err(|_| E2eeError::Tampered)?;
        let opened = Zeroizing::new(opened);
        let key = <[u8; 32]>::try_from(opened.as_slice()).map_err(|_| E2eeError::Tampered)?;
        Ok(Zeroizing::new(key))
    }
}

fn wrap_key(shared: &[u8; 32], eph: &[u8; 32], recipient: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(eph);
    salt[32..].copy_from_slice(recipient);
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared)
        .expand(WRAP_INFO, key.as_mut())
        .expect("32 bytes is a valid length");
    key
}

fn wrap_cipher(key: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(&(*key).into())
}

/// Seals `key` for each device with one one-time key. Each device's wrapping key is unique to that
/// one-time key, so the fixed nonce is never used twice with a key.
fn wrap_for(
    key: &[u8; 32],
    devices: &[DeviceKeys],
) -> Result<([u8; 32], Vec<WrappedKey>), E2eeError> {
    // Fresh for every call and dropped (and wiped) at its end: one-time, whatever its type's name.
    let eph_secret = StaticSecret::from(random::<32>());
    let eph = PublicKey::from(&eph_secret).to_bytes();
    let mut wrapped = Vec::with_capacity(devices.len());
    for device in devices {
        let recipient: [u8; 32] = unb64_array(&device.dh_key)?;
        let shared = eph_secret.diffie_hellman(&PublicKey::from(recipient));
        if !shared.was_contributory() {
            return Err(E2eeError::Malformed);
        }
        let wrap = wrap_key(shared.as_bytes(), &eph, &recipient);
        let sealed = wrap_cipher(&wrap)
            .encrypt(
                &[0u8; 12].into(),
                Payload {
                    msg: key.as_slice(),
                    aad: device.device_id.as_bytes(),
                },
            )
            .map_err(|_| E2eeError::Malformed)?;
        wrapped.push(WrappedKey {
            device: device.device_id.clone(),
            key: b64(&sealed),
        });
    }
    Ok((eph, wrapped))
}

/// An attachment as its message describes it: the real name and the key are only in here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InnerAttachment {
    pub id: Id,
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub key: String,
    pub nonce: String,
}

/// What a message says, inside its envelope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inner {
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<InnerAttachment>,
    /// The message this answers, and a line of it: the server cannot make that line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_excerpt: Option<String>,
}

fn message_transcript(
    channel: Id,
    author: Id,
    device: &str,
    eph: &str,
    nonce: &str,
    ciphertext: &str,
    keys: &[WrappedKey],
) -> Vec<u8> {
    let mut list = Vec::new();
    for key in keys {
        list.extend_from_slice(key.device.as_bytes());
        list.push(0);
        list.extend_from_slice(key.key.as_bytes());
        list.push(0);
    }
    transcript(
        MESSAGE_SIG,
        &[
            &[VERSION],
            &channel.0.to_be_bytes(),
            &author.0.to_be_bytes(),
            device.as_bytes(),
            eph.as_bytes(),
            nonce.as_bytes(),
            ciphertext.as_bytes(),
            &list,
        ],
    )
}

fn message_aad(channel: Id, author: Id, device: &str) -> Vec<u8> {
    transcript(
        MESSAGE_AAD,
        &[
            &channel.0.to_be_bytes(),
            &author.0.to_be_bytes(),
            device.as_bytes(),
        ],
    )
}

/// Encrypts a message for `devices` and signs it. `devices` must include my own, or I cannot read
/// it back.
pub fn seal_message(
    identity: &Identity,
    author: Id,
    channel: Id,
    devices: &[DeviceKeys],
    inner: &Inner,
) -> Result<Envelope, E2eeError> {
    let json = Zeroizing::new(serde_json::to_vec(inner).map_err(|_| E2eeError::Malformed)?);
    let mut plain = Zeroizing::new(Vec::with_capacity(json.len() + 4 + MESSAGE_PADDING));
    plain.extend_from_slice(&(json.len() as u32).to_be_bytes());
    plain.extend_from_slice(&json);
    let padded = plain.len().next_multiple_of(MESSAGE_PADDING);
    plain.resize(padded, 0);

    let key = Zeroizing::new(random::<32>());
    let nonce: [u8; 24] = random();
    let aad = message_aad(channel, author, &identity.device_id);
    let ciphertext = XChaCha20Poly1305::new(&(*key).into())
        .encrypt(
            &nonce.into(),
            Payload {
                msg: &plain,
                aad: &aad,
            },
        )
        .map_err(|_| E2eeError::Malformed)?;
    let (eph, keys) = wrap_for(&key, devices)?;
    let (eph, nonce, ciphertext) = (b64(&eph), b64(&nonce), b64(&ciphertext));
    let signature = identity.sign.sign(&message_transcript(
        channel,
        author,
        &identity.device_id,
        &eph,
        &nonce,
        &ciphertext,
        &keys,
    ));
    Ok(Envelope {
        v: VERSION,
        device: identity.device_id.clone(),
        eph,
        nonce,
        ciphertext,
        keys,
        sig: b64(&signature.to_bytes()),
    })
}

fn verify(device: &DeviceKeys, message: &[u8], signature: &str) -> Result<(), E2eeError> {
    let key: [u8; 32] = unb64_array(&device.sign_key)?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| E2eeError::Malformed)?;
    let signature: [u8; 64] = unb64_array(signature)?;
    key.verify(message, &Signature::from_bytes(&signature))
        .map_err(|_| E2eeError::Tampered)
}

/// Checks and decrypts a message. `sender` is the pinned device `envelope.device` names.
pub fn open_message(
    identity: &Identity,
    author: Id,
    channel: Id,
    sender: &DeviceKeys,
    envelope: &Envelope,
) -> Result<Inner, E2eeError> {
    if envelope.v != VERSION || envelope.device != sender.device_id {
        return Err(E2eeError::Malformed);
    }
    let digest = message_transcript(
        channel,
        author,
        &envelope.device,
        &envelope.eph,
        &envelope.nonce,
        &envelope.ciphertext,
        &envelope.keys,
    );
    verify(sender, &digest, &envelope.sig)?;
    let eph: [u8; 32] = unb64_array(&envelope.eph)?;
    let key = identity.unwrap(&eph, &envelope.keys)?;
    let nonce: [u8; 24] = unb64_array(&envelope.nonce)?;
    let ciphertext = unb64(&envelope.ciphertext)?;
    let aad = message_aad(channel, author, &envelope.device);
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new(&(*key).into())
            .decrypt(
                &nonce.into(),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eeError::Tampered)?,
    );
    if plain.len() < 4 {
        return Err(E2eeError::Malformed);
    }
    let len = u32::from_be_bytes([plain[0], plain[1], plain[2], plain[3]]) as usize;
    let json = plain.get(4..4 + len).ok_or(E2eeError::Malformed)?;
    serde_json::from_slice(json).map_err(|_| E2eeError::Malformed)
}

/// Encrypts a file with a key of its own: `(sealed bytes, key, nonce)`.
pub fn seal_file(bytes: &[u8]) -> Result<(Vec<u8>, String, String), E2eeError> {
    let key = Zeroizing::new(random::<32>());
    let nonce: [u8; 24] = random();
    let sealed = XChaCha20Poly1305::new(&(*key).into())
        .encrypt(
            &nonce.into(),
            Payload {
                msg: bytes,
                aad: FILE_AAD,
            },
        )
        .map_err(|_| E2eeError::Malformed)?;
    Ok((sealed, b64(&*key), b64(&nonce)))
}

pub fn open_file(sealed: &[u8], key: &str, nonce: &str) -> Result<Vec<u8>, E2eeError> {
    let key = Zeroizing::new(unb64_array::<32>(key)?);
    let nonce: [u8; 24] = unb64_array(nonce)?;
    XChaCha20Poly1305::new(&(*key).into())
        .decrypt(
            &nonce.into(),
            Payload {
                msg: sealed,
                aad: FILE_AAD,
            },
        )
        .map_err(|_| E2eeError::Tampered)
}

/// A number two people can compare over another channel: when it is the same on both sides, nobody
/// has put their own keys between them. It covers every device of the people in `keys`.
pub fn safety_number(keys: &[(Id, Vec<DeviceKeys>)]) -> String {
    let mut lines: Vec<String> = keys
        .iter()
        .flat_map(|(user, devices)| {
            devices
                .iter()
                .filter(|d| !d.revoked)
                .map(move |d| format!("{user}:{}", d.sign_key))
        })
        .collect();
    lines.sort();
    let mut digest = Sha256::digest(SAFETY).to_vec();
    for line in &lines {
        digest = Sha256::new()
            .chain_update(&digest)
            .chain_update(line.as_bytes())
            .finalize()
            .to_vec();
    }
    digest
        .chunks_exact(3)
        .take(6)
        .map(|chunk| {
            let n = u32::from(chunk[0]) << 16 | u32::from(chunk[1]) << 8 | u32::from(chunk[2]);
            format!("{:05}", n % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- who has which keys ----------------------------------------------------------------------

/// Whether a device's keys are what they claim: the ID fits the key, and the key signed itself to
/// this account.
pub fn device_is_valid(user: Id, device: &DeviceKeys) -> bool {
    let check = || -> Result<(), E2eeError> {
        let sign: [u8; 32] = unb64_array(&device.sign_key)?;
        let dh: [u8; 32] = unb64_array(&device.dh_key)?;
        if device_id(&sign) != device.device_id {
            return Err(E2eeError::Tampered);
        }
        verify(
            device,
            &transcript(DEVICE_SIG, &[&user.0.to_be_bytes(), &dh]),
            &device.signature,
        )
    };
    check().is_ok()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct KnownUser {
    /// Every device ever seen, for checking old messages.
    devices: BTreeMap<String, DeviceKeys>,
    /// Device IDs added since I was last told, waiting to be shown.
    #[serde(default)]
    unseen: BTreeSet<String>,
}

/// The keys seen for each user on one instance (trust on first use), kept in a file so a server that
/// changes someone's keys later is noticed. Public keys only.
#[derive(Debug, Default)]
pub struct Directory {
    path: Option<PathBuf>,
    users: HashMap<Id, KnownUser>,
    /// When each user's devices were last asked for (not kept).
    fetched: HashMap<Id, i64>,
    /// Goes up whenever anyone's devices change, so voice knows to hand out keys again.
    revision: u64,
}

impl Directory {
    pub fn load(path: Option<PathBuf>) -> Directory {
        let users = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Directory {
            path,
            users,
            fetched: HashMap::new(),
            revision: 0,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn save(&self) {
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match serde_json::to_vec_pretty(&self.users) {
                Ok(bytes) => {
                    if let Err(err) = write_atomic(path, &bytes) {
                        tracing::warn!("known encryption keys not saved: {err}");
                    }
                }
                Err(err) => tracing::warn!("known encryption keys not saved: {err}"),
            }
        }
    }

    /// Takes in what the server says `user`'s devices are. Keys that do not check out are dropped.
    /// The first devices seen for someone are simply pinned; later ones count as unseen.
    pub fn update(&mut self, user: Id, devices: Vec<DeviceKeys>, now: i64) {
        self.fetched.insert(user, now);
        let entry = self.users.entry(user).or_default();
        let first_contact = entry.devices.is_empty();
        let mut changed = false;
        let live: BTreeSet<&str> = devices
            .iter()
            .filter(|d| !d.revoked)
            .map(|d| d.device_id.as_str())
            .collect();
        // A device the server no longer lists is not sent to any more.
        for (id, device) in &mut entry.devices {
            let revoked = !live.contains(id.as_str());
            if device.revoked != revoked {
                device.revoked = revoked;
                changed = true;
            }
        }
        for device in &devices {
            if !device_is_valid(user, device) {
                tracing::warn!("ignoring invalid encryption keys of user {user}");
                continue;
            }
            match entry.devices.get(&device.device_id) {
                // An ID is a hash of the key, so a known ID with another key cannot pass the check
                // above; and a known device coming back is no news.
                Some(_) => {}
                None => {
                    if !first_contact && !device.revoked {
                        entry.unseen.insert(device.device_id.clone());
                    }
                    entry
                        .devices
                        .insert(device.device_id.clone(), device.clone());
                    changed = true;
                }
            }
        }
        if changed {
            self.revision += 1;
            self.save();
        }
    }

    /// When `user`'s devices were last asked for.
    pub fn fetched_at(&self, user: Id) -> Option<i64> {
        self.fetched.get(&user).copied()
    }

    /// Makes the next look at `user` ask the server again.
    pub fn forget_fetch(&mut self, user: Id) {
        self.fetched.remove(&user);
    }

    pub fn device(&self, user: Id, device: &str) -> Option<&DeviceKeys> {
        self.users.get(&user)?.devices.get(device)
    }

    /// Who to encrypt to: the devices in use.
    pub fn live_devices(&self, user: Id) -> Vec<DeviceKeys> {
        self.users.get(&user).map_or_else(Vec::new, |u| {
            u.devices.values().filter(|d| !d.revoked).cloned().collect()
        })
    }

    /// New devices of `user` I have not been told about yet.
    pub fn unseen(&self, user: Id) -> usize {
        self.users.get(&user).map_or(0, |u| u.unseen.len())
    }

    /// I have been told about `user`'s new devices.
    pub fn acknowledge(&mut self, user: Id) {
        if let Some(entry) = self.users.get_mut(&user)
            && !entry.unseen.is_empty()
        {
            entry.unseen.clear();
            self.save();
        }
    }
}

// ---- voice ------------------------------------------------------------------------------------

fn voice_transcript(
    channel: Id,
    from: Id,
    to: Id,
    generation: u8,
    device: &str,
    eph: &str,
    keys: &[WrappedKey],
) -> Vec<u8> {
    let mut list = Vec::new();
    for key in keys {
        list.extend_from_slice(key.device.as_bytes());
        list.push(0);
        list.extend_from_slice(key.key.as_bytes());
        list.push(0);
    }
    transcript(
        VOICE_SIG,
        &[
            &[VERSION, generation],
            &channel.0.to_be_bytes(),
            &from.0.to_be_bytes(),
            &to.0.to_be_bytes(),
            device.as_bytes(),
            eph.as_bytes(),
            &list,
        ],
    )
}

/// Seals my sender key for `to`'s devices.
pub fn seal_voice_key(
    identity: &Identity,
    channel: Id,
    from: Id,
    to: Id,
    devices: &[DeviceKeys],
    generation: u8,
    seed: &[u8; 32],
) -> Result<VoiceKey, E2eeError> {
    let (eph, keys) = wrap_for(seed, devices)?;
    let eph = b64(&eph);
    let signature = identity.sign.sign(&voice_transcript(
        channel,
        from,
        to,
        generation,
        &identity.device_id,
        &eph,
        &keys,
    ));
    Ok(VoiceKey {
        v: VERSION,
        generation,
        device: identity.device_id.clone(),
        eph,
        keys,
        sig: b64(&signature.to_bytes()),
    })
}

/// Checks and opens someone's sender key. `sender` is the pinned device it names.
pub fn open_voice_key(
    identity: &Identity,
    channel: Id,
    from: Id,
    to: Id,
    sender: &DeviceKeys,
    key: &VoiceKey,
) -> Result<Zeroizing<[u8; 32]>, E2eeError> {
    if key.v != VERSION || key.device != sender.device_id {
        return Err(E2eeError::Malformed);
    }
    verify(
        sender,
        &voice_transcript(
            channel,
            from,
            to,
            key.generation,
            &key.device,
            &key.eph,
            &key.keys,
        ),
        &key.sig,
    )?;
    let eph: [u8; 32] = unb64_array(&key.eph)?;
    identity.unwrap(&eph, &key.keys)
}

/// The keys derived from one sender key: nonces count separately for each.
#[derive(Clone)]
struct StreamKeys {
    voice: ChaCha20Poly1305,
    share: ChaCha20Poly1305,
    video: ChaCha20Poly1305,
}

impl StreamKeys {
    fn derive(seed: &[u8; 32]) -> StreamKeys {
        let hk = Hkdf::<Sha256>::new(None, seed);
        let key = |label: &[u8]| {
            let mut out = Zeroizing::new([0u8; 32]);
            hk.expand(label, out.as_mut())
                .expect("32 bytes is a valid length");
            ChaCha20Poly1305::new(&(*out).into())
        };
        StreamKeys {
            voice: key(b"hermes-e2ee/v1/voice"),
            share: key(b"hermes-e2ee/v1/share-audio"),
            video: key(b"hermes-e2ee/v1/video"),
        }
    }

    fn of(&self, stream: Stream) -> &ChaCha20Poly1305 {
        match stream {
            Stream::Voice => &self.voice,
            Stream::Share => &self.share,
            Stream::Video => &self.video,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    Voice,
    Share,
    Video,
}

impl From<AudioKind> for Stream {
    fn from(kind: AudioKind) -> Stream {
        match kind {
            AudioKind::Voice => Stream::Voice,
            AudioKind::Share => Stream::Share,
        }
    }
}

/// My sender key: a counter per stream, so a nonce is never used twice under a key.
struct SendKey {
    generation: u8,
    keys: StreamKeys,
    counters: [u64; 3],
}

impl SendKey {
    /// `[generation][48-bit counter]` and the nonce made from them.
    fn next(&mut self, stream: Stream) -> ([u8; SEAL_PREFIX], [u8; 12]) {
        let slot = &mut self.counters[stream as usize];
        let counter = *slot;
        *slot += 1;
        let mut prefix = [0u8; SEAL_PREFIX];
        prefix[0] = self.generation;
        prefix[1..].copy_from_slice(&counter.to_be_bytes()[2..]);
        (prefix, nonce_of(&prefix))
    }
}

fn nonce_of(prefix: &[u8; SEAL_PREFIX]) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[5..].copy_from_slice(prefix);
    nonce
}

/// The sender keys of one voice session: mine, and those the others sent me.
#[derive(Default)]
pub struct VoiceCrypto {
    send: Option<SendKey>,
    /// By participant, then generation.
    recv: HashMap<u16, BTreeMap<u8, StreamKeys>>,
    next_generation: u8,
}

impl std::fmt::Debug for VoiceCrypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceCrypto")
            .field("keyed", &self.recv.len())
            .finish_non_exhaustive()
    }
}

/// What the server may see of a voice datagram: its type and timing. Who sent it is what it fills
/// in itself, and loudness travels inside.
fn audio_aad(kind: AudioKind, header: &AudioHeader, prefix: &[u8; SEAL_PREFIX]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(16);
    aad.push(match kind {
        AudioKind::Voice => 1,
        AudioKind::Share => 3,
    });
    aad.extend_from_slice(&header.timestamp.to_be_bytes());
    aad.extend_from_slice(&header.sequence.to_be_bytes());
    aad.extend_from_slice(prefix);
    aad
}

impl VoiceCrypto {
    /// A new room or connection: nobody's keys carry over.
    pub fn reset(&mut self) {
        self.send = None;
        self.recv.clear();
    }

    /// A new random sender key: `(generation, seed)`, to send around before [`Self::use_key`].
    pub fn new_key(&mut self) -> (u8, Zeroizing<[u8; 32]>) {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        (generation, Zeroizing::new(random()))
    }

    /// Starts sending with this key.
    pub fn use_key(&mut self, generation: u8, seed: &[u8; 32]) {
        self.send = Some(SendKey {
            generation,
            keys: StreamKeys::derive(seed),
            counters: [0; 3],
        });
    }

    /// Whether I can send yet.
    pub fn has_key(&self) -> bool {
        self.send.is_some()
    }

    pub fn add_received(&mut self, participant: u16, generation: u8, seed: &[u8; 32]) {
        let keys = self.recv.entry(participant).or_default();
        keys.insert(generation, StreamKeys::derive(seed));
        // The newest few, by age: generations wrap, so "older" is counted back from this one.
        keys.retain(|g, _| generation.wrapping_sub(*g) < KEPT_GENERATIONS);
    }

    pub fn remove_participant(&mut self, participant: u16) {
        self.recv.remove(&participant);
    }

    /// Whether `participant`'s key has arrived.
    pub fn has_key_of(&self, participant: u16) -> bool {
        self.recv.contains_key(&participant)
    }

    /// Seals a plain datagram from the audio engine for the wire. Who hears it and how loud are
    /// taken out of the clear header; `None` while I have no key.
    pub fn seal_audio(&mut self, datagram: &[u8]) -> Option<Vec<u8>> {
        let (kind, header, opus) = AudioHeader::decode_any(datagram).ok()?;
        let send = self.send.as_mut()?;
        let (prefix, nonce) = send.next(kind.into());
        let mut plain = Vec::with_capacity(opus.len() + VOICE_PADDING + 3);
        plain.push(u8::from(header.voice_active));
        plain.push(header.audio_level);
        plain.extend_from_slice(&(opus.len() as u16).to_be_bytes());
        plain.extend_from_slice(opus);
        // Never over the size the server takes; the frame goes unpadded in that rare case.
        let padded = plain.len().next_multiple_of(VOICE_PADDING);
        if HEADER_LEN + SEAL_PREFIX + padded + TAG_LEN <= MAX_DATAGRAM {
            plain.resize(padded, 0);
        }
        let aad = audio_aad(kind, &header, &prefix);
        let sealed = send
            .keys
            .of(kind.into())
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: &plain,
                    aad: &aad,
                },
            )
            .ok()?;
        let clear = AudioHeader {
            voice_active: true,
            sender_id: 0,
            timestamp: header.timestamp,
            sequence: header.sequence,
            audio_level: hermes_proto::voice::LEVEL_SILENT,
        };
        let mut body = Vec::with_capacity(SEAL_PREFIX + sealed.len());
        body.extend_from_slice(&prefix);
        body.extend_from_slice(&sealed);
        Some(clear.encode_as(kind, &body))
    }

    /// Opens a datagram from the wire into the plain form the audio engine takes. `None`: no key
    /// yet, or it does not check out; either way it is not played.
    pub fn open_audio(&self, datagram: &[u8]) -> Option<(AudioKind, u16, Vec<u8>)> {
        let (kind, header, body) = AudioHeader::decode_any(datagram).ok()?;
        let prefix: &[u8; SEAL_PREFIX] = body.get(..SEAL_PREFIX)?.try_into().ok()?;
        let keys = self.recv.get(&header.sender_id)?.get(&prefix[0])?;
        let aad = audio_aad(kind, &header, prefix);
        let plain = keys
            .of(kind.into())
            .decrypt(
                &nonce_of(prefix).into(),
                Payload {
                    msg: &body[SEAL_PREFIX..],
                    aad: &aad,
                },
            )
            .ok()?;
        let len = u16::from_be_bytes([*plain.get(2)?, *plain.get(3)?]) as usize;
        let opus = plain.get(4..4 + len)?;
        let open = AudioHeader {
            voice_active: plain[0] != 0,
            sender_id: header.sender_id,
            timestamp: header.timestamp,
            sequence: header.sequence,
            audio_level: plain[1],
        };
        Some((kind, header.sender_id, open.encode_as(kind, opus)))
    }

    /// Seals a screen share frame (`header` in clear, as the server needs it to forward).
    pub fn seal_video(&mut self, header: &[u8; VIDEO_HEADER_LEN], frame: &[u8]) -> Option<Vec<u8>> {
        let send = self.send.as_mut()?;
        let (prefix, nonce) = send.next(Stream::Video);
        let mut aad = video_aad(header);
        aad.extend_from_slice(&prefix);
        let sealed = send
            .keys
            .video
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: frame,
                    aad: &aad,
                },
            )
            .ok()?;
        let mut out = Vec::with_capacity(VIDEO_HEADER_LEN + SEAL_PREFIX + sealed.len());
        out.extend_from_slice(header);
        out.extend_from_slice(&prefix);
        out.extend_from_slice(&sealed);
        Some(out)
    }

    /// Opens a screen share frame: `data` is everything after the header.
    pub fn open_video(
        &self,
        sender: u16,
        header: &[u8; VIDEO_HEADER_LEN],
        data: &[u8],
    ) -> Option<Vec<u8>> {
        let prefix: &[u8; SEAL_PREFIX] = data.get(..SEAL_PREFIX)?.try_into().ok()?;
        let keys = self.recv.get(&sender)?.get(&prefix[0])?;
        let mut aad = video_aad(header);
        aad.extend_from_slice(prefix);
        keys.video
            .decrypt(
                &nonce_of(prefix).into(),
                Payload {
                    msg: &data[SEAL_PREFIX..],
                    aad: &aad,
                },
            )
            .ok()
    }
}

/// The frame header without the sender ID, which the server fills in.
fn video_aad(header: &[u8; VIDEO_HEADER_LEN]) -> Vec<u8> {
    let mut aad = header.to_vec();
    aad[2] = 0;
    aad[3] = 0;
    aad
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_proto::voice::{AudioHeader, set_sender_id, set_video_sender_id};

    const ALICE: Id = Id(1);
    const BOB: Id = Id(2);
    const EVE: Id = Id(3);
    const CHANNEL: Id = Id(100);

    fn inner(text: &str) -> Inner {
        Inner {
            content: text.to_owned(),
            ..Inner::default()
        }
    }

    #[test]
    fn a_message_opens_for_every_device_it_was_sealed_for_and_nobody_else() {
        let (alice, bob, bob2, eve) = (
            Identity::generate(),
            Identity::generate(),
            Identity::generate(),
            Identity::generate(),
        );
        let devices = [alice.public(ALICE), bob.public(BOB), bob2.public(BOB)];
        let env = seal_message(&alice, ALICE, CHANNEL, &devices, &inner("hello")).unwrap();
        assert!(!serde_json::to_string(&env).unwrap().contains("hello"));
        let sender = alice.public(ALICE);
        for reader in [&alice, &bob, &bob2] {
            let open = open_message(reader, ALICE, CHANNEL, &sender, &env).unwrap();
            assert_eq!(open.content, "hello");
        }
        assert_eq!(
            open_message(&eve, ALICE, CHANNEL, &sender, &env),
            Err(E2eeError::NotForThisDevice)
        );
    }

    #[test]
    fn nothing_about_a_message_can_be_changed_unnoticed() {
        let (alice, bob, eve) = (
            Identity::generate(),
            Identity::generate(),
            Identity::generate(),
        );
        let devices = [alice.public(ALICE), bob.public(BOB)];
        let env = seal_message(&alice, ALICE, CHANNEL, &devices, &inner("hello")).unwrap();
        let sender = alice.public(ALICE);
        let open = |env: &Envelope| open_message(&bob, ALICE, CHANNEL, &sender, env);
        assert!(open(&env).is_ok());

        // Moved to another channel, or passed off as someone else's.
        assert_eq!(
            open_message(&bob, ALICE, Id(101), &sender, &env),
            Err(E2eeError::Tampered)
        );
        assert_eq!(
            open_message(&bob, EVE, CHANNEL, &sender, &env),
            Err(E2eeError::Tampered)
        );
        // Every field is covered by the signature.
        let mut bad = env.clone();
        bad.ciphertext = b64(&[0u8; 256]);
        assert_eq!(open(&bad), Err(E2eeError::Tampered));
        let mut bad = env.clone();
        bad.nonce = b64(&[9u8; 24]);
        assert_eq!(open(&bad), Err(E2eeError::Tampered));
        let mut bad = env.clone();
        bad.keys[1].key = b64(&[1u8; 48]);
        assert_eq!(open(&bad), Err(E2eeError::Tampered));
        let mut bad = env.clone();
        bad.keys.remove(0);
        assert_eq!(open(&bad), Err(E2eeError::Tampered));
        // A server that signs with a device of its own is caught by the pin: the sender is looked
        // up by the author's pinned devices, and its signature does not fit another's key.
        let forged = seal_message(&eve, ALICE, CHANNEL, &devices, &inner("forged")).unwrap();
        assert!(open(&forged).is_err());
        let mut relabelled = forged;
        relabelled.device = sender.device_id.clone();
        assert_eq!(open(&relabelled), Err(E2eeError::Tampered));
    }

    #[test]
    fn message_sizes_are_padded() {
        let alice = Identity::generate();
        let devices = [alice.public(ALICE)];
        let size = |text: &str| {
            seal_message(&alice, ALICE, CHANNEL, &devices, &inner(text))
                .unwrap()
                .ciphertext
                .len()
        };
        assert_eq!(size("a"), size("a little longer"));
    }

    #[test]
    fn identities_survive_being_stored() {
        let identity = Identity::generate();
        let again = Identity::from_secret(&identity.to_secret()).unwrap();
        assert_eq!(identity.public(ALICE), again.public(ALICE));
        assert!(Identity::from_secret("nonsense").is_none());
    }

    #[test]
    fn files_need_their_key_and_cannot_be_changed() {
        let (sealed, key, nonce) = seal_file(b"secret file").unwrap();
        assert!(!sealed.windows(6).any(|w| w == b"secret"));
        assert_eq!(open_file(&sealed, &key, &nonce).unwrap(), b"secret file");
        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert_eq!(open_file(&bad, &key, &nonce), Err(E2eeError::Tampered));
        let (_, other, _) = seal_file(b"x").unwrap();
        assert_eq!(open_file(&sealed, &other, &nonce), Err(E2eeError::Tampered));
    }

    #[test]
    fn devices_must_prove_their_keys() {
        let bob = Identity::generate();
        let good = bob.public(BOB);
        assert!(device_is_valid(BOB, &good));
        assert!(!device_is_valid(EVE, &good), "signed for another account");
        let mut swapped = good.clone();
        swapped.dh_key = Identity::generate().public(BOB).dh_key;
        assert!(
            !device_is_valid(BOB, &swapped),
            "someone else's encryption key"
        );
        let mut claimed = Identity::generate().public(BOB);
        claimed.device_id = good.device_id.clone();
        assert!(!device_is_valid(BOB, &claimed), "someone else's device ID");
    }

    #[test]
    fn the_directory_pins_first_keys_and_reports_new_ones() {
        let mut dir = Directory::load(None);
        let (one, two) = (Identity::generate(), Identity::generate());
        dir.update(BOB, vec![one.public(BOB)], 1);
        assert_eq!(dir.unseen(BOB), 0, "the first keys seen are simply trusted");
        assert_eq!(dir.live_devices(BOB).len(), 1);
        let before = dir.revision();

        // Asking again changes nothing.
        dir.update(BOB, vec![one.public(BOB)], 2);
        assert_eq!(dir.revision(), before);

        // A later device is reported, and used.
        dir.update(BOB, vec![one.public(BOB), two.public(BOB)], 3);
        assert_eq!(dir.unseen(BOB), 1);
        assert_eq!(dir.live_devices(BOB).len(), 2);
        assert!(dir.revision() > before);
        dir.acknowledge(BOB);
        assert_eq!(dir.unseen(BOB), 0);

        // Invalid keys never get in.
        let mut forged = Identity::generate().public(BOB);
        forged.signature = two.public(BOB).signature;
        dir.update(BOB, vec![one.public(BOB), two.public(BOB), forged], 4);
        assert_eq!(dir.live_devices(BOB).len(), 2);

        // A device that goes is not sent to, but its key still checks old messages.
        let gone = two.device_id().to_owned();
        dir.update(BOB, vec![one.public(BOB)], 5);
        assert_eq!(dir.live_devices(BOB).len(), 1);
        assert!(dir.device(BOB, &gone).is_some());
        // And coming back is no news.
        dir.update(BOB, vec![one.public(BOB), two.public(BOB)], 6);
        assert_eq!(dir.live_devices(BOB).len(), 2);
        assert_eq!(dir.unseen(BOB), 0);
    }

    #[test]
    fn the_directory_remembers_across_runs() {
        let dir = std::env::temp_dir().join(format!("hermes-e2ee-{}", std::process::id()));
        let path = dir.join("known.json");
        let _ = std::fs::remove_file(&path);
        let (one, two) = (Identity::generate(), Identity::generate());
        let mut known = Directory::load(Some(path.clone()));
        known.update(BOB, vec![one.public(BOB)], 1);
        drop(known);
        // A server that swaps someone's keys while I am away is still noticed.
        let mut known = Directory::load(Some(path));
        known.update(BOB, vec![two.public(BOB)], 2);
        assert_eq!(known.unseen(BOB), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn safety_numbers_agree_and_change_with_the_keys() {
        let (a, b, c) = (
            Identity::generate(),
            Identity::generate(),
            Identity::generate(),
        );
        let mine = safety_number(&[(ALICE, vec![a.public(ALICE)]), (BOB, vec![b.public(BOB)])]);
        let theirs = safety_number(&[(BOB, vec![b.public(BOB)]), (ALICE, vec![a.public(ALICE)])]);
        assert_eq!(mine, theirs);
        assert_eq!(mine.len(), 6 * 5 + 5);
        let swapped = safety_number(&[(ALICE, vec![a.public(ALICE)]), (BOB, vec![c.public(BOB)])]);
        assert_ne!(mine, swapped);
    }

    /// Two participants with each other's keys. `b` is participant 7 on `a`, and the reverse is 3.
    fn keyed_pair() -> (VoiceCrypto, VoiceCrypto) {
        let (alice, bob) = (Identity::generate(), Identity::generate());
        let mut a = VoiceCrypto::default();
        let mut b = VoiceCrypto::default();
        let (generation, seed) = a.new_key();
        let sealed = seal_voice_key(
            &alice,
            CHANNEL,
            ALICE,
            BOB,
            &[bob.public(BOB)],
            generation,
            &seed,
        )
        .unwrap();
        let opened =
            open_voice_key(&bob, CHANNEL, ALICE, BOB, &alice.public(ALICE), &sealed).unwrap();
        assert_eq!(*opened, *seed);
        a.use_key(generation, &seed);
        b.add_received(3, sealed.generation, &opened);
        (a, b)
    }

    fn voice(sequence: u16, opus: &[u8]) -> Vec<u8> {
        AudioHeader {
            voice_active: true,
            sender_id: 0,
            timestamp: u32::from(sequence) * 960,
            sequence,
            audio_level: 12,
        }
        .encode(opus)
    }

    #[test]
    fn voice_keys_are_for_one_person_in_one_room() {
        let (alice, bob, eve) = (
            Identity::generate(),
            Identity::generate(),
            Identity::generate(),
        );
        let sender = alice.public(ALICE);
        let seed = [7u8; 32];
        let sealed =
            seal_voice_key(&alice, CHANNEL, ALICE, BOB, &[bob.public(BOB)], 1, &seed).unwrap();
        assert!(open_voice_key(&bob, CHANNEL, ALICE, BOB, &sender, &sealed).is_ok());
        assert_eq!(
            open_voice_key(&bob, Id(5), ALICE, BOB, &sender, &sealed).err(),
            Some(E2eeError::Tampered),
            "replayed into another room"
        );
        assert_eq!(
            open_voice_key(&eve, CHANNEL, ALICE, BOB, &sender, &sealed).err(),
            Some(E2eeError::NotForThisDevice)
        );
    }

    #[test]
    fn voice_goes_through_sealed_and_comes_out_as_it_went_in() {
        let (mut a, b) = keyed_pair();
        let plain = voice(5, b"opus bytes");
        let mut wire = a.seal_audio(&plain).unwrap();
        assert!(!wire.windows(10).any(|w| w == b"opus bytes"));
        // The server only learns timing: not how loud, not whether it is voice.
        let (_, clear, _) = AudioHeader::decode_any(&wire).unwrap();
        assert_eq!(clear.audio_level, hermes_proto::voice::LEVEL_SILENT);
        // It fills in the sender.
        set_sender_id(&mut wire, 3);
        let (kind, sender, opened) = b.open_audio(&wire).unwrap();
        assert_eq!((kind, sender), (AudioKind::Voice, 3));
        let (header, opus) = AudioHeader::decode(&opened).unwrap();
        assert_eq!(opus, b"opus bytes");
        assert_eq!(header.audio_level, 12);
        assert_eq!(header.sender_id, 3);
    }

    #[test]
    fn voice_cannot_be_changed_or_played_without_the_key() {
        let (mut a, b) = keyed_pair();
        let mut wire = a.seal_audio(&voice(1, b"opus")).unwrap();
        set_sender_id(&mut wire, 3);
        assert!(b.open_audio(&wire).is_some());
        // Someone else's sender ID: another key, or none.
        let mut other = wire.clone();
        set_sender_id(&mut other, 4);
        assert!(b.open_audio(&other).is_none());
        // Changing the clear header or the data.
        let mut bad = wire.clone();
        bad[9] ^= 1;
        assert!(b.open_audio(&bad).is_none());
        let mut bad = wire.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(b.open_audio(&bad).is_none());
        // A plain frame in the same format.
        let mut plain = voice(1, b"not encrypted at all");
        set_sender_id(&mut plain, 3);
        assert!(b.open_audio(&plain).is_none());
        // Nothing goes out without a key.
        assert!(VoiceCrypto::default().seal_audio(&voice(1, b"x")).is_none());
        // And a newcomer without my key hears nothing.
        assert!(VoiceCrypto::default().open_audio(&wire).is_none());
    }

    #[test]
    fn nonces_never_repeat_even_if_the_audio_engine_starts_over() {
        let (mut a, _) = keyed_pair();
        // The same packet twice, as after reopening the microphone: its counters start again.
        let one = a.seal_audio(&voice(1, b"same")).unwrap();
        let two = a.seal_audio(&voice(1, b"same")).unwrap();
        assert_ne!(one, two);
        assert_ne!(
            one[HEADER_LEN..HEADER_LEN + SEAL_PREFIX],
            two[HEADER_LEN..HEADER_LEN + SEAL_PREFIX]
        );
    }

    #[test]
    fn a_new_key_shuts_out_who_only_had_the_old_one() {
        let (mut a, b) = keyed_pair();
        let (generation, seed) = a.new_key();
        a.use_key(generation, &seed);
        let mut wire = a.seal_audio(&voice(1, b"opus")).unwrap();
        set_sender_id(&mut wire, 3);
        assert!(b.open_audio(&wire).is_none(), "b was not given the new key");
    }

    #[test]
    fn old_keys_keep_working_for_a_moment_then_go() {
        let mut b = VoiceCrypto::default();
        let mut a = VoiceCrypto::default();
        let (g0, s0) = a.new_key();
        a.use_key(g0, &s0);
        b.add_received(3, g0, &s0);
        let mut wire = a.seal_audio(&voice(1, b"opus")).unwrap();
        set_sender_id(&mut wire, 3);
        for _ in 0..2 {
            let (g, s) = a.new_key();
            b.add_received(3, g, &s);
        }
        assert!(b.open_audio(&wire).is_some(), "two rotations later");
        for _ in 0..3 {
            let (g, s) = a.new_key();
            b.add_received(3, g, &s);
        }
        assert!(b.open_audio(&wire).is_none(), "long gone");
    }

    #[test]
    fn padding_hides_small_differences_in_size() {
        let (mut a, _) = keyed_pair();
        let one = a.seal_audio(&voice(1, &[0; 40])).unwrap();
        let two = a.seal_audio(&voice(2, &[0; 50])).unwrap();
        assert_eq!(one.len(), two.len());
    }

    #[test]
    fn a_frame_at_the_largest_size_still_fits() {
        let (mut a, b) = keyed_pair();
        let room = MAX_DATAGRAM - HEADER_LEN - SEAL_PREFIX - TAG_LEN - 4;
        let mut wire = a.seal_audio(&voice(1, &vec![1; room])).unwrap();
        assert!(wire.len() <= MAX_DATAGRAM);
        set_sender_id(&mut wire, 3);
        assert!(b.open_audio(&wire).is_some());
    }

    #[test]
    fn video_frames_are_sealed_with_the_header_in_clear() {
        let (mut a, b) = keyed_pair();
        let header = hermes_proto::voice::VideoHeader {
            keyframe: true,
            sender_id: 0,
            width: 1920,
            height: 1080,
            timestamp: 40,
        }
        .encode();
        let mut wire = a.seal_video(&header, b"h264 frame").unwrap();
        assert!(!wire.windows(10).any(|w| w == b"h264 frame"));
        assert!(
            hermes_proto::voice::VideoHeader::decode(&wire)
                .unwrap()
                .keyframe
        );
        set_video_sender_id(&mut wire, 3);
        let on_wire: [u8; VIDEO_HEADER_LEN] = wire[..VIDEO_HEADER_LEN].try_into().unwrap();
        assert_eq!(
            b.open_video(3, &on_wire, &wire[VIDEO_HEADER_LEN..])
                .unwrap(),
            b"h264 frame"
        );
        // A keyframe flag flipped by the server is noticed.
        let mut flipped = on_wire;
        flipped[1] = 0;
        assert!(
            b.open_video(3, &flipped, &wire[VIDEO_HEADER_LEN..])
                .is_none()
        );
    }
}
