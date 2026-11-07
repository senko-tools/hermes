//! End-to-end encryption on the wire (docs/e2ee.md): device keys, sealed messages and sealed voice
//! keys. Everything here is opaque to the server, which only stores and forwards it. Binary values
//! are base64 strings. No cryptography happens here; `hermes-client-core` does it.

use serde::{Deserialize, Serialize};

use crate::id::Id;

/// The version of the format below.
pub const VERSION: u8 = 1;
/// Largest [`Envelope`], serialized, that the server stores: a message at the longest length, with
/// room for the keys of a large group's devices.
pub const MAX_ENVELOPE_BYTES: usize = 256 * 1024;
/// Largest sealed voice key the server relays.
pub const MAX_VOICE_KEY_BYTES: usize = 16 * 1024;
/// Devices one account can have at once.
pub const MAX_DEVICES: usize = 32;
/// Users asked about in one `POST /users/devices`.
pub const MAX_DEVICE_QUERY: usize = 100;

/// A device's public keys. The private ones never leave the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeys {
    /// Hex of the first 16 bytes of the SHA-256 of `sign_key`, so it cannot be claimed for
    /// another key.
    pub device_id: String,
    /// Ed25519 public key: signs what the device sends.
    pub sign_key: String,
    /// X25519 public key: what is encrypted for the device.
    pub dh_key: String,
    /// `sign_key`'s signature over the user ID and `dh_key`.
    pub signature: String,
    /// No longer gets anything new, but its old messages can still be checked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub revoked: bool,
}

/// `PUT /users/@me/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterDeviceRequest {
    pub device_id: String,
    pub sign_key: String,
    pub dh_key: String,
    pub signature: String,
}

/// `POST /users/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceQuery {
    pub user_ids: Vec<Id>,
}

/// A user's devices, revoked ones included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDevices {
    pub user_id: Id,
    pub devices: Vec<DeviceKeys>,
}

/// A symmetric key sealed for one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedKey {
    pub device: String,
    pub key: String,
}

/// An encrypted message, as the server stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    /// The device that sealed and signed it.
    pub device: String,
    /// One-time X25519 public key the [`WrappedKey`]s are sealed with.
    pub eph: String,
    pub nonce: String,
    pub ciphertext: String,
    pub keys: Vec<WrappedKey>,
    /// `device`'s signature over all of the above, the channel and the author.
    pub sig: String,
}

/// A voice participant's sender key, sealed for the devices of one other participant. Relayed by
/// the server as a string, as JSON, in `voice_key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceKey {
    pub v: u8,
    /// Which of the sender's keys this is; datagrams name it.
    pub generation: u8,
    pub device: String,
    pub eph: String,
    pub keys: Vec<WrappedKey>,
    /// `device`'s signature over all of the above, the channel, the sender and the recipient.
    pub sig: String,
}

/// How an opened message stands. Not on the wire: the client fills it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageLock {
    /// Not encrypted: everything in a space, and DMs from before encryption.
    #[default]
    Plain,
    /// Decrypted and its signature checked.
    Sealed,
    /// Encrypted, but not for this device (sent before it existed).
    NotForThisDevice,
    /// Encrypted, and the signature or the data is wrong.
    Tampered,
}

/// The key of an encrypted attachment, from inside its message. Not on the wire as such: the client
/// keeps it with the attachment to decrypt the download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedFile {
    pub key: String,
    pub nonce: String,
    /// The name the file is stored under on the server.
    pub stored_name: String,
}
