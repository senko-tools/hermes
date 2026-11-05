# Hermes protocol reference

Human-readable reference for everything on the wire. The source of truth is the `hermes-proto`
crate; this file is kept in sync with it. Protocol version: **1**.

- [Conventions](#conventions)
- [REST](#rest)
- [Gateway (WebSocket)](#gateway-websocket)
- [Voice](#voice)

---

## Conventions

- **IDs** are 64-bit snowflakes, sent as **decimal strings** (`"1234567890"`) because JavaScript
  numbers cannot hold them.
- **Timestamps** are Unix milliseconds (numbers).
- **Permissions** are a `u64` bit set sent as a decimal string (see PLAN.md section 8.1 for the
  bits).
- JSON field names are `snake_case`; enum values are `snake_case` strings.
- Default port **7700**: TCP for HTTPS/WSS, UDP for QUIC voice.

## REST

Base path `/api/v1`, JSON bodies, `Authorization: Bearer hms_…`. The endpoint list is PLAN.md
section 10.

Errors always look like:

```json
{ "error": { "code": "MISSING_PERMISSION", "message": "You need SPEAK in this channel", "retry_after_ms": null } }
```

`VALIDATION_FAILED` adds `"fields": { "name": "what is wrong" }`; `RATE_LIMITED` (HTTP 429) sets
`retry_after_ms`.

| Code | HTTP |
|---|---|
| `BAD_REQUEST`, `VALIDATION_FAILED` | 400 |
| `UNAUTHORIZED`, `INVALID_CREDENTIALS` | 401 |
| `ACCOUNT_DISABLED`, `MISSING_PERMISSION`, `REGISTRATION_CLOSED`, `BANNED`, `VOICE_DISABLED` | 403 |
| `NOT_FOUND`, `INVITE_INVALID` | 404 |
| `CONFLICT`, `VOICE_FULL` | 409 |
| `PAYLOAD_TOO_LARGE` | 413 |
| `RATE_LIMITED` | 429 |
| `STORAGE_FULL` | 507 |
| `INTERNAL` | 500 |

Spaces and channels you cannot see answer `NOT_FOUND`; missing permissions on visible ones answer
`MISSING_PERMISSION`.

### User, search and admin endpoints

| Request | Response | Notes |
|---|---|---|
| `PATCH /users/@me` `{display_name?, avatar_id?, status_text?}` | `CurrentUser` | `null` clears a field; `display_name` 1–32, `status_text` ≤ 128 chars; `avatar_id` must be an image you uploaded; sends `user_update` to everyone sharing a space or DM |
| `GET /users/{id}` | `User` + `{mutual_space_ids}` | yourself, or someone you share a space or DM with; else `NOT_FOUND` |
| `GET /users/@me/mentions` `?before&limit` | `[Message]`, newest first | messages in channels you can read that mention you, one of your roles, `@everyone` or `@here`; your own are left out (an addition to PLAN.md section 10) |
| `PUT /users/@me/notification-settings/{target_id}` `{level, muted_until?}` | 204 | a space you are in or a channel you can see; `muted_until`: `null`, `-1` (forever) or Unix ms; sends `notification_settings_update` |
| `DELETE /users/@me/notification-settings/{target_id}` | 204 | back to the default; sends `notification_settings_delete {target_id}` (an addition to PLAN.md section 10) |
| `GET /search` `?q&space_id?&channel_id?&author_id?&before?&limit?` | `{messages, total}`, newest first | every word must match as a prefix; only channels with `VIEW_CHANNEL` + `READ_HISTORY` (DMs you are in); deleted messages never match; an empty `q` is `VALIDATION_FAILED` |
| `GET /users/@me/privacy` | `{dm_from, message_requests}` | `dm_from`: `everyone` (default), `spaces` (people sharing a space with you) or `nobody`; `message_requests` (default `true`) (an addition to PLAN.md section 10) |
| `PATCH /users/@me/privacy` `{dm_from?, message_requests?}` | `{dm_from, message_requests}` | sends `privacy_update` to your sessions; existing DMs are unchanged |
| `GET /admin/users` `?after&limit` | `[User + {disabled, instance_admin}]` | instance admins only |
| `PATCH /admin/users/{id}` `{disabled?, instance_admin?}` | 204 | instance admins only; disabling signs the user out everywhere; admins cannot disable or demote themselves |

### DM endpoints

| Request | Response | Notes |
|---|---|---|
| `POST /dms` `{recipient_ids}` | `Channel` | a new DM to someone whose `dm_from` leaves you out is a message request for them (their copy has `request: true`), or `MISSING_PERMISSION` if their `message_requests` is off; an existing 1:1 DM is always reused; the server's `dm_policy` applies first |
| `POST /dms/{id}/accept` | `Channel` | accepts your message request (`dm_update`, or `dm_create` if you had ignored it); sending a message in it accepts it too; no-op for other DMs |
| `DELETE /dms/{id}` | 204 | closes a 1:1 DM for you: `dm_delete {id}` to your sessions, and it leaves `GET /dms` and `ready`; a message or call ring in it, or `POST /dms` to the same person, reopens it with `dm_create` first; group DMs answer `BAD_REQUEST` (leave them with `DELETE /channels/{id}`) (an addition to PLAN.md section 10) |

### Voice endpoints

| Request | Response | Notes |
|---|---|---|
| `POST /voice/join` `{channel_id, self_mute?, self_deaf?}` | `{token, expires_in_secs: 30, quic: {host, port, cert_sha256}}` | voice channel, DM or group DM; `CONNECT`; `user_limit` unless `MOVE_MEMBERS`; `voice.max_users_per_room` always |
| `POST /voice/leave` | 204 | the QUIC connection gets `kicked {reason: "disconnected"}` |
| `PATCH /voice/@me` `{self_mute?, self_deaf?}` | 204 | no-op when not in voice |
| `PATCH /spaces/{id}/voice-states/{uid}` `{server_mute?, server_deaf?, channel_id?}` | 204 | `MUTE_MEMBERS` / `DEAFEN_MEMBERS` / `MOVE_MEMBERS` in the user's channel; `channel_id: null` disconnects |
| `POST /channels/{id}/call/ring` `{user_ids?}` | 204 | DMs only; rings recipients not in the call |

`cert_sha256` is the voice certificate's SHA-256 as upper-case hex pairs joined with `:`.

## Gateway (WebSocket)

`wss://host:port/api/v1/gateway`, one JSON text frame per message:

```json
{ "op": "message_create", "s": 1042, "d": { } }
```

`s` is the sequence number of dispatch events and `null` for control ops.

### Lifecycle

1. Server: `hello {heartbeat_interval_ms: 30000}`.
2. Client: `identify {token, protocol_version: 1, client: {name, version, os}}` or
   `resume {token, session_id, last_seq}`.
3. Server: `ready {…}` after identify; after resume, every missed event and then `resumed {}`, or
   `invalid_session {resumable: false}` (identify again and replace your state).
4. Client: `heartbeat {last_seq}` every interval (±10 %); server: `heartbeat_ack {}`.

Client ops: `identify`, `resume`, `heartbeat`, `presence_update {status}`. Everything else is REST.

### `ready`

```json
{
  "session_id": "…",
  "user": { "id": "1", "username": "alice", "instance_admin": false, "…": "…" },
  "spaces": [ { "space": {}, "roles": [], "channels": [], "members": [], "voice_states": [] } ],
  "dms": [],
  "read_states": [],
  "presences": [],
  "notification_settings": [],
  "dm_voice_states": [],
  "voice": { "enabled": true, "quic_port": 7700, "cert_sha256": "3F:A9:…" },
  "instance": { "name": "…", "limits": { "max_upload_mb": 25, "max_message_length": 4000 } }
}
```

`presences` lists only users who are not offline. `dm_voice_states` holds the voice states of DM
and group DM calls you are a recipient of.

### Dispatch events

`message_create`, `message_update`, `message_delete {id, channel_id}`,
`message_delete_bulk {ids, channel_id}`, `reaction_add` / `reaction_remove`
`{message_id, channel_id, emoji, user_id}`, `typing_start {channel_id, user_id, timestamp}`,
`read_state_update`, `channel_create` / `channel_update`, `channel_delete {id, space_id}`,
`space_create` (full snapshot), `space_update`, `space_delete {id}`, `role_create` /
`role_update`, `role_delete {id, space_id}`, `member_add` / `member_update`,
`member_remove {space_id, user_id}`, `user_update`, `presence_update`, `voice_state_update`,
`call_ring {channel_id, caller_id}`, `call_ring_stop {channel_id}`, `dm_create`, `dm_update`,
`dm_delete {id}`, `notification_settings_update`, `notification_settings_delete {target_id}`,
`privacy_update {dm_from, message_requests}` (own sessions).

A DM in `ready.dms`, `GET /dms`, `dm_create` and `dm_update` carries `request: true` when it is
a message request for you (the field is left out otherwise). A request is never rung and shares
no presence until accepted; ignoring one (`DELETE /dms/{id}`) closes it for good: its events
stop and new messages do not reopen it. `ready.privacy` holds your privacy settings.

`voice_state_update` carries a full `VoiceState`
`{user_id, channel_id, self_mute, self_deaf, server_mute, server_deaf, joined_at}`, or
`{user_id, channel_id: null}` when the user left voice. Moving between rooms sends the "left"
update for the old channel's viewers, then the new state.

### Close codes

| Code | Meaning | Client |
|---|---|---|
| 4000 | unknown error | reconnect |
| 4001 | unknown op | reconnect |
| 4002 | decode error | reconnect |
| 4003 | not authenticated | reconnect |
| 4004 | authentication failed | stop, show login |
| 4005 | already authenticated | reconnect |
| 4007 | invalid seq | identify fresh |
| 4008 | rate limited | reconnect after 5 s |
| 4009 | session timed out / queue overflow | resume |
| 4010 | unsupported protocol version | stop, ask to update |
| 4011 | session revoked | stop, show login |
| 4100 | server restarting | reconnect with backoff |

## Voice

QUIC on UDP, same port number as HTTPS. ALPN `hermes-voice/1`, TLS 1.3, the certificate pinned to
`cert_sha256` (it is always self-signed; host names are not checked). Idle timeout 15 s,
keep-alive 5 s. One connection per instance, reused across rooms.

### Joining

1. `POST /voice/join` → token (single use, 30 s).
2. Connect and open **one bidirectional stream** (the control stream).
3. Send `auth {token, self_mute, self_deaf}` within 10 s → `ready`.
4. To change rooms on the same connection: `POST /voice/join` for the new channel, then
   `switch {token}` → `ready` for the new room.

### Control stream

Each frame is a `u32` big-endian length (at most 65536) followed by a JSON body
`{ "op": "…", "d": { … } }`.

Client → server:

| op | d |
|---|---|
| `auth` | `{token, self_mute, self_deaf}` (first message) |
| `switch` | `{token}` |
| `set_state` | `{self_mute, self_deaf}` |
| `ping` | `{t}` (echoed in `pong`) |
| `leave` | `{}` (leave the room, keep the connection) |
| `share_start` | `{}`: share my screen (needs `STREAM`; frames follow, see below) |
| `share_stop` | `{}` |
| `watch` | `{participant_id, watching}`: start or stop receiving someone's share |
| `request_keyframe` | `{participant_id}`: a frame of their share was lost (only while watching) |

Server → client:

| op | d |
|---|---|
| `ready` | `{participant_id, room_id, bitrate, participants: [{participant_id, user_id}], sharing?: [participant_id]}` (the others in the room; `sharing`: those of them sharing their screen, left out when empty) |
| `participant_joined` | `{participant_id, user_id}` |
| `participant_left` | `{participant_id}` |
| `server_state` | `{server_mute, server_deaf}` |
| `bitrate_changed` | `{bitrate}` |
| `moved` | `{token}`: a moderator moved you; `switch` with it |
| `kicked` | `{reason}`: `joined_elsewhere`, `disconnected`, `channel_deleted`, `removed` |
| `pong` | `{t}` |
| `error` | `{code, message}` (`ErrorCode` names) |
| `share_started` | `{participant_id}` |
| `share_stopped` | `{participant_id}` (also implied by `participant_left`; watchers must `watch` again when it restarts) |
| `keyframe_requested` | `{}`: send your next screen share frame as a keyframe |
| `share_watchers` | `{count}`: how many members watch your share; sent after `share_start` and whenever it changes. While it is 0 frames go nowhere, and clients stop capturing (clients that never receive it, from an older server, keep sending) |

An invalid or expired `auth` token, or any op before `auth`, answers `error {code: "UNAUTHORIZED"}`
and the server closes the connection. After `kicked` or `leave` the connection stays open.

### Audio datagrams

QUIC datagrams, all integers big-endian, 11-byte header + one 20 ms Opus frame (48 kHz, mono):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | version (high nibble, 1) and type (low nibble, 1 = audio) |
| 1 | 1 | flags: bit 0 = voice active (gate open); other bits 0 |
| 2 | 2 | sender ID: the participant ID (the server overwrites it) |
| 4 | 4 | timestamp in 48 kHz samples, random start, +960 per frame |
| 8 | 2 | sequence, +1 per packet, wraps |
| 10 | 1 | audio level, 0 = loud … 127 = silent (−dBov) |
| 11 | … | Opus payload |

The server drops datagrams that are malformed, larger than 1000 bytes, or from senders who are
muted (self or server) or lack `SPEAK`, and forwards the rest to everyone else in the room who is
not deafened. It never waits for a slow receiver.

### Screen shares

Who shares is told only on the control stream, never on the gateway: only people in the call
know about a share or receive it. Each frame is one H.264 access unit (Annex B), sent on its own
**unidirectional stream**: a 12-byte header, the bitstream, then the end of the stream. Peers may
have at most 8 such streams open at once, and both sides read them one at a time in the order
they were opened, so frames stay in order.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | version (high nibble, 1) and type (low nibble, 2 = video) |
| 1 | 1 | flags: bit 0 = keyframe |
| 2 | 2 | sender ID (the server overwrites it) |
| 4 | 2 | width, even, 2–3840 |
| 6 | 2 | height, even, 2–3840 |
| 8 | 4 | milliseconds since the share started |

The server drops frames over 512 KiB (header included), arriving slower than 5 s, with a bad
header, from someone who does not share or lacks `STREAM` (rechecked every second), or beyond 30
per second, and sends the rest to the members watching the sharer. A watcher whose queue (8
frames) is full loses the frame, and the sharer gets `keyframe_requested` (at most every
500 ms; also when someone starts watching). The GUI sends at most 1280×720 at 10 frames per
second, a keyframe at least every 5 s.

### DM calls

Every DM and group DM has a room keyed by its channel ID. "Start call" = `POST /voice/join` +
`POST /channels/{id}/call/ring`. Recipients get `call_ring`; it stops with `call_ring_stop` when
they join, or for everyone when the caller leaves or the room empties. Clients stop ringing after
30 s on their own. A system message `"<name> started a call"` is posted when the room fills, and
`"Call ended · N min"` when it empties.
