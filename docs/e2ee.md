# End-to-end encryption

Direct messages, group DMs, their files, and all voice (DM calls and space voice channels, with
screen share video and sound) are encrypted on the sender's device and only readable on the devices of
the people they are for. The server stores and forwards ciphertext. Nobody types a password or a code:
it all happens by itself. Text channels in spaces are not encrypted; they stay searchable on the server.

The code: `crates/hermes-client-core/src/e2ee.rs` (the cryptography), `secure.rs` (keys and messages in
the client's flow), `voice.rs` (sender keys in a call), `crates/hermes-proto/src/e2ee.rs` (the wire format).

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

The server refuses plain DMs (`privacy.require_dm_encryption`, on by default). Clients refuse to send in clear,
and mark a DM message that is not encrypted with an open lock: an old one, or one the server made up.

## Voice

Every participant has a random sender key, sent to each other participant's devices sealed as above and
signed over the room, the sender and the recipient. It is replaced whenever who is in the room or anyone's devices
change, so someone who leaves cannot follow what comes after and someone who joins cannot read what came
before. Only people the gateway also shows in the room get it.

Voice, stream sound and stream video each use a key derived from it, with ChaCha20-Poly1305 and a counter that
never repeats under a key. The clear header of an audio datagram keeps only its type, sequence and timestamp, which
are authenticated; loudness moved inside, and the content is padded to 32 bytes. Packet version 2 keeps
clients from before this out, in both directions, so nothing is ever sent or played in clear.

## What this does not protect

Said plainly, because promising more would be worse than saying nothing:

- **What the server can still see:** who talks to whom and when, how large messages and files are, who is in
  which call and when they talk, reactions (emoji are not encrypted), who is typing, profiles and names.
- **Trust on first use:** a server that is already malicious the first time two people meet can give each its
  own keys. Comparing safety numbers is what catches that; the warning catches every later attempt.
- **A device added later cannot read older messages.** That is the price of having no password and of not letting
  the server add a device to get the history. Losing every device's key loses the history.
- **No forward secrecy for messages:** messages stay on the server to be read later on any device, so whoever steals
  a device's key can open the messages sealed for it, given a copy of the server's data. Voice keys are not stored.
- **Inside a call**, everyone holding a sender key could forge that person's audio. They are all in the room anyway.
- **Search** cannot look inside encrypted DMs, and pins and replies work only from what the device can open.
- **A compromised device or client binary** defeats all of it, as with any such system.

This is our own protocol from reviewed primitives (`x25519-dalek`, `ed25519-dalek`, `chacha20poly1305`, `hkdf`), and it
has not been audited by anyone outside.
