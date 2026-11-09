# End-to-end encryption

Direct messages, group DMs, messages in server channels, their files and reactions, and all voice (DM
calls and space voice channels, with screen share video and sound) are encrypted on the sender's device
and only readable on the devices of the people they are for. The server stores and forwards ciphertext.
Nobody types a password or a code: it all happens by itself.

The code: `crates/hermes-client-core/src/e2ee.rs` (the cryptography), `secure.rs` (keys and messages in
the client's flow), `channel_keys.rs` (server channels), `search_index.rs` (searching), `voice.rs` (sender keys
in a call), `crates/hermes-proto/src/e2ee.rs` (the wire format).

## Keys

Each device makes its own Ed25519 key (signs) and X25519 key (is encrypted to) per account per server. The
secrets stay in the OS credential store (the `tokens.toml` fallback where there is none). The public parts go
to the server (`PUT /users/@me/devices`), signed to the account; a device's ID is a hash of its signing key.

The server hands out those public keys, and could lie about them. So clients:

- check that each key signed itself to that account and fits its ID;
- **pin** the first keys seen for someone (`<config dir>/e2ee/<instance>-<user>.json`, public keys only),
  trusting them on first use;
- **warn** in the conversation when someone gets another device later, and also when I do;
- show a **safety number** (lock button in a DM) and a room code (the voice connection details), the same on
  everyone's screen if nobody put their own keys in between.

Signing out or revoking a session revokes its device. The rows stay, so old messages can still be checked.

## Messages and files

A message's content, attachment names and keys, and reply excerpt are sealed: JSON, padded to 256 bytes,
XChaCha20-Poly1305 under a random key. That key is sealed for each device of each recipient and for my own
with a one-time X25519 key, HKDF-SHA256 and ChaCha20-Poly1305. The sending device signs everything, including
the channel and the author, so a message cannot be moved, edited or passed off as someone else's. Files get a key of their
own, kept inside the message; on the server they are `encrypted.bin` of type `application/octet-stream`.
Before encrypting, the sender shrinks what compresses well (`compress.rs`: zstd, and PNGs that hold only pixels are
re-encoded), keeping the result only if it saves at least 5 %. The server can see the smaller size.

The server refuses plain DMs (`privacy.require_dm_encryption`, on by default). Clients refuse to send in clear,
and mark a DM message that is not encrypted with an open lock: an old one, or one the server made up.

A picture also gets a small preview (at most 400 px), encrypted and stored beside it, so that showing it in the
chat does not mean downloading all of it. A reaction is sealed with a key derived from its message's key. The same
emoji always seals the same way on the same message, so the server can count it without knowing which it is.

## Server channels

Sealing a message to every device of every member would not scale and would leave new members with nothing to
read, so a channel has shared, numbered keys. A message is encrypted with a random key, that key is sealed under
the channel's current key, and the sending device signs everything as in a DM.

Whoever writes first makes key 1. When someone who holds the current key can no longer read the channel (they left,
were removed, lost the permission, or a device was revoked), the next person to write makes a new one for those who
remain. A device that lacks a channel's keys asks for them; each key handed over is sealed for that one device and
signed by the device handing it over. They are kept in a key ring on disk, encrypted with a key from the OS
credential store.

Two rules decide who gets keys. A server sets them (only its owner); a channel may differ (the owner or an administrator):

- **Who shares keys.** *Automatic:* any member's client does it by itself. *Approved:* someone with Manage Server
  lets each new device in, in the server's Encryption settings.
- **New members can read earlier messages.** When off, a new member's arrival starts a new key and they get only
  that one. This is enforced by the keys, not by the server hiding anything.
  A member asks for keys when they first open the channel, so what was written between joining and that moment stays
  closed to them too.

Changing a rule never takes back keys someone already has. Messages from before channels were encrypted stay readable,
with an open lock. A server may turn the requirement off with `privacy.require_channel_encryption`.

## Searching

The server cannot search what it cannot read. Each device keeps its own index of the words in the encrypted messages
it can open (word to message IDs, not the messages), compressed and encrypted on disk with a key from the credential
store. It is loaded by the first search and let go when idle, and a slow pass fills it in from the history. It finds
whole words and the starts of words. Older plain messages are still searched on the server.

## Voice

Every participant has a random sender key, sent to each other participant's devices sealed as above and
signed over the room, the sender and the recipient. It is replaced whenever who is in the room or anyone's devices
change, so someone who leaves cannot follow what comes after and someone who joins cannot read what came
before. Only people the gateway also shows in the room get it.

Voice, stream sound and stream video each use a key derived from it, with ChaCha20-Poly1305 and a counter that
never repeats under a key. The clear header of an audio datagram keeps only its type, sequence and timestamp, which
are authenticated; loudness moved inside, and the content is padded to 32 bytes. Packet version 2 keeps
clients from before this out, in both directions, so nothing is ever sent or played in clear.

## What a device holds a server to

A hostile server is the case all of this is for, so a device remembers what it has seen and does not
take the server's word for less:

- **Key rules:** a channel seen to need approval, or to hide its past, stays that way on this device
  until someone changes the rule from this device. A server claiming otherwise gets a warning.
- **Encryption stays on:** an instance seen to encrypt its channels keeps being treated as encrypted.
- **Older keys:** a key this device holds is never replaced, and a server offering an older channel key
  than the newest held gets nothing sent. Restoring a server from a backup older than a key rotation
  therefore stops sending in that channel, on purpose.
- **Messages in clear:** a plain message after a conversation's first sealed one is not shown and does
  not notify, because the server could have written it under any name.
- **Replays:** a sealed message says when it was written and which message it edits. One the server
  moved, or posted again later (over an hour off), or rolled back from an edit, is not shown.
- **Devices:** a device that comes back after being removed counts as new, and what it signs while
  removed does not verify. Marking a conversation as verified holds sending while someone has an
  unseen device.
- **Damage:** a damaged record file is set aside and reported, rather than silently starting over.
- **Removing an instance** deletes this device's keys, channel keys, search index and records for it.
  Signing out keeps them, so signing back in on this device still reads the history.

## What this does not protect

Said plainly, because promising more would be worse than saying nothing:

- **What the server can still see:** who talks to whom and when, how large messages and files are (after
  compression), who is in which call and when they talk, how many reactions a message has, who is typing, who a channel
  message mentions (it has to, to notify them), and profiles, avatars and the names of servers and channels.
- **Automatic key sharing trusts the server about who is a member.** A server that is actively malicious could add a
  member of its own and be handed a channel's keys. It protects against a stolen disk or backup and a curious host.
  Only *Approved* stops that, because a person decides.
- **Trust on first use:** a server that is already malicious the first time two people meet can give each its
  own keys. Comparing safety numbers is what catches that; the warning catches every later attempt.
- **A device added later cannot read older direct messages.** That is the price of having no password and of not
  letting the server add a device to get the history. Losing every device's key loses the history. In a server channel a
  new device gets older keys from another member, if the channel's rules allow it and someone is online to share them.
- **No forward secrecy for messages:** messages stay on the server to be read later on any device, so whoever steals
  a device's key can open the messages sealed for it, given a copy of the server's data. Voice keys are not stored.
- **Inside a call**, everyone holding a sender key could forge that person's audio. They are all in the room anyway.
- **Search** on a device only finds what that device can open, and pins and replies work only from what it can open.
  The index protects against a copied config folder or a stolen disk, not against malware running as the user.
- **A compromised device or client binary** defeats all of it, as with any such system.

This is our own protocol from reviewed primitives (`x25519-dalek`, `ed25519-dalek`, `chacha20poly1305`, `hkdf`), and it
has not been audited by anyone outside.
