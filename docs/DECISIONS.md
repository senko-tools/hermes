# Decisions

Log of deviations from `PLAN.md` and of choices the plan left open.
Newest entries at the bottom. Each entry: date, what changed, why.

---

## 2026-10-30: Opus binding is `opus`, not `audiopus`

- **What:** PLAN.md section 6 lists "`audiopus` (or `opus`)". The workspace uses `opus` 0.4.0.
- **Why:** `audiopus` has had no release since 2021 (0.2.0); `opus` is actively maintained (0.4.0, August 2026).

## 2026-10-30: Minimum supported Rust version is 1.99

- **What:** `rust-version = "1.99"` in the workspace; CI builds with the latest stable toolchain.
- **Why:** 1.99 was the current stable release at project start; the plan does not name an MSRV.

## 2026-10-30: Top-level `spikes/` folder for Phase 0 spikes

- **What:** Phase 0 spikes live in `spikes/<name>/` as standalone binary crates (workspace members,
  `publish = false`). Real crates never depend on them. This folder is not in the section 5.2 layout.
  `.github/workflows/spikes.yml` (manual `workflow_dispatch`) builds them for `x86_64-pc-windows-msvc`
  and `x86_64-unknown-linux-musl` and uploads them as artifacts.
- **Why:** spikes are throwaway experiments that must be testable on the developer's Windows machine
  without a Rust toolchain. Keeping them in the workspace means they share pinned versions and
  CI checks (fmt, clippy, test).
- **Note:** the musl build links a static alsa-lib without `dlopen`, so ALSA plugins (PulseAudio and
  PipeWire bridges) are not available to it. On such desktops it may only see raw `hw:` devices.
  The Windows build is the one that matters for Spike A.

## 2026-10-30: rustls uses the `ring` crypto provider

- **What:** the workspace `rustls` and `quinn` entries disable default features and enable `ring`
  (rustls: `std`, `ring`, `logging`, `tls12`; quinn: `runtime-tokio`, `rustls-ring`, `log`).
  `tokio-rustls` got the same treatment (`logging`, `tls12`, `ring`) when the server started using
  it (2026-10-31), because its default features also enable `aws-lc-rs`.
- **Why:** rustls 0.23 defaults to `aws-lc-rs`, while quinn and rcgen default to `ring`. With both
  compiled in, rustls cannot pick a provider automatically, and `aws-lc-sys` is a heavy C/CMake
  build on Windows and musl. `ring` alone covers TLS 1.3 for QUIC and HTTPS.

## 2026-10-30: Spike A (audio loopback) passed; "added latency" means our pipeline only

- **Result** (developer's Windows PC, WASAPI shared mode, Yeti X in, Realtek out, both 48 kHz f32,
  480-frame periods): the full chain cpal → nnnoiseless → Opus 20 ms/64 kbps/FEC/DTX → decode → cpal
  works with libopus built statically. Estimated end-to-end latency 77–82 ms, of which:
  - devices (WASAPI engine + drivers): ~14 ms input + ~32 ms output ≈ 46 ms;
  - Hermes pipeline (ring buffers, frame buffering, resampler/Opus/denoiser delays): ~31–36 ms.
  DTX works (6.9–11.9 kbps in silence, ~53 kbps while talking); RNNoise voice probability 0.84 while
  talking; no sustained underruns.
- **What:** the Phase 0 criterion "< 60 ms added latency" is interpreted as latency added by Hermes,
  excluding the OS audio devices, which every voice app pays. The spike counts as passed.
- **Why:** cpal only offers WASAPI shared mode with fixed 10 ms periods, so the device part cannot be
  reduced from our side; the part we control is well under 60 ms.
- **Follow-up for `hermes-audio`:** playback sounded quiet on the developer's setup; the spike now has
  a dBFS level meter and `--gain` to investigate. Consider the AGC from section 12.5 (step 4) if quiet
  microphones turn out to be common.

## 2026-10-30: Spike B (QUIC datagrams) passed

- **Result:** `quic-echo server` (static musl binary) on hermes.senko.tools, bound to `[::]:7700`
  (dual-stack, `net.ipv6.bindv6only = 0`), client on the developer's Windows PC at home, 50 datagrams/s
  of 200 bytes for 60 s, certificate pinned by SHA-256:
  - IPv4: 3000/3000 received, 0 % loss, no reordering, RTT min 5.3 / avg 5.6 / p95 5.7 / max 6.3 ms,
    jitter 0.08 ms, max datagram size 1414 bytes.
  - IPv6: same results (RTT avg 5.6 / p95 5.8 / max 8.2 ms, jitter 0.10 ms) after the developer fixed
    IPv6 on the home network (the first failure was the client network, not the server).
- **What:** no change to the plan; QUIC datagrams over one UDP port are confirmed for voice. A 20 ms
  Opus frame (~160 bytes at 64 kbps) plus the 11-byte header fits well inside 1414 bytes.
- **Notes for the real server:** on a dual-stack socket IPv4 peers appear as `::ffff:a.b.c.d`; convert
  them to plain IPv4 (`to_canonical()`) before per-IP rate limits and logging.

## 2026-10-30: `windows` crate for the push-to-talk polling fallback; iced-ui spike is Windows-only

- **What:** added `windows = "=0.62.2"` (feature `Win32_UI_Input_KeyboardAndMouse`) to the workspace,
  used only on Windows by `spikes/iced-ui --poll` to read `GetAsyncKeyState` every 10 ms. The `spikes`
  workflow does not build `iced-ui` for `x86_64-unknown-linux-musl`.
- **Why:** PLAN.md section 21 names this fallback (`windows` crate) but section 6 does not list the crate;
  approved by the developer. The version is the one cpal already pulls in, so nothing new is downloaded.
  `global-hotkey` 0.9 on Windows uses `RegisterHotKey` (the key is taken away from every other app while
  registered, keyboard keys only) and detects release by polling `GetAsyncKeyState` every 50 ms; the
  spike compares both. A static musl build cannot load the system GUI libraries, so it would be useless.

## 2026-10-30: Spike C (iced UI risks) passed

- **Result** (developer's Windows PC, iced 0.14): pane_grid split/close/drag/resize, `text_editor`
  input (Enter sends, Shift+Enter newline), a virtualized list of 10,000 rows with color emoji
  scrolling smoothly, file-drop events, and global push-to-talk reporting press **and** release while
  the window is unfocused all worked, and the developer reported it as fast. Memory use ~125 MB on
  Windows. The app crashed once and the crash could not be reproduced (no message was captured);
  watch for it in Phase 3 and capture the panic output if it happens again.
- **Virtualized list:** fixed row height; the scroll offset from `scrollable::on_scroll` selects the
  visible range plus 10 rows of overscan, with spacers for the rest. Only ~30–60 rows are built at any
  time. Variable-height rows (wrapping, attachments, replies) are still open for Phase 3.
- **Emoji** (observed in the Linux VM with Noto Color Emoji; not checked row by row on Windows):
  ordinary emoji, skin tones and ZWJ sequences render in color; flags (regional indicator pairs) render
  as letters in boxes, and characters with VS16 (`❤️ ☕ ⚠️`) render as monochrome symbols. Windows'
  Segoe UI Emoji has no flag glyphs at all. Keep the section 21 fallback (bundled Twemoji/Noto images in
  a custom rich-text widget) in mind for Phase 3, at least for flags.
- **Push-to-talk decision** (section 21 says "decide in Spike C"): use the `GetAsyncKeyState` polling
  thread (every 10 ms, `windows` crate) for push-to-talk on Windows. `global-hotkey` also reports release
  (it polls `GetAsyncKeyState` itself every 50 ms), but it registers the key with `RegisterHotKey`, which
  takes the key away from every other app (a PTT key like `V` or `CapsLock` would stop working
  everywhere else) and it cannot use mouse buttons, which section 15.6 suggests (Mouse4). Polling does
  neither and has lower release latency. `global-hotkey` stays the choice for one-shot global actions
  where taking the key is fine (global toggle-mute). Other platforms are not targeted in v1.

## 2026-10-30: Phase 0 complete

- **What:** all three spikes passed on the developer's machine (see the entries above); no fallback from
  section 21 was needed. The spike code stays in `spikes/` as reference and is not used by real crates.
- **Carry-over notes for later phases:**
  - Phase 1 (server): canonicalize `::ffff:a.b.c.d` peers before per-IP rate limits and logs (Spike B).
  - Phase 3 (GUI): variable-height virtualized rows; emoji fallback for flags/VS16; investigate the
    one-off crash; the developer has UI changes in mind.
  - Phase 4 (audio): playback sounded quiet in Spike A, so check levels and consider the AGC from
    section 12.5; PTT uses the polling approach above.

## 2026-10-30: Hand-written markup parser instead of wrapping `pulldown-cmark`

- **What:** `hermes-proto::markup` is a small hand-written parser for exactly the syntax in section 7.5
  (Discord-style rules); `pulldown-cmark` (section 6) is not used. Additions the plan did not spell out:
  backslash escapes of ASCII punctuation (`\*`), `<https://…>` links, quotes need `> ` (so `>_<` stays
  text) and do not nest, `_`-italics only at word boundaries (`snake_case` stays text), link targets must
  be `http://` or `https://`, and formatting nests at most 12 levels deep.
- **Why:** approved by the developer. CommonMark disagrees with chat syntax in many places: `__x__` is
  bold there (we want underline), single newlines are merged, 4-space indents become code blocks,
  `- x` becomes a list, bare URLs are not links, and `||spoiler||` / `<@id>` would need a second pass
  anyway. A parser that only knows our syntax is more predictable and keeps GUI and TUI identical.

## 2026-10-30: `Permissions` is a decimal string on the wire

- **What:** `Permissions` serializes as a decimal string (`"196608"`), like `Id`. Unknown bits from a
  newer peer are dropped when decoding.
- **Why:** approved by the developer. Bits currently go up to 37, but the reserved gaps go up to bit 63,
  which a JavaScript number cannot hold exactly; using strings now avoids a breaking change later.

## 2026-10-30: `hermes-proto` details the plan left open

- **HTTP statuses for `ErrorCode`:** 400 `BAD_REQUEST`, `VALIDATION_FAILED`; 401 `UNAUTHORIZED`,
  `INVALID_CREDENTIALS`; 403 `ACCOUNT_DISABLED`, `MISSING_PERMISSION`, `REGISTRATION_CLOSED`, `BANNED`,
  `VOICE_DISABLED`; 404 `NOT_FOUND`, `INVITE_INVALID`; 409 `CONFLICT`, `VOICE_FULL`; 413
  `PAYLOAD_TOO_LARGE`; 429 `RATE_LIMITED`; 500 `INTERNAL`; 507 `STORAGE_FULL`. `fields` is omitted
  unless the code is `VALIDATION_FAILED`.
- **Models:** `AttachmentRef = {id, filename}` (enough to build the download URL); `Mentions =
  {users, roles, everyone, here}` (the `message_mentions` kinds); `SpacePreview = {id, name, icon,
  member_count}`; `Member.role_ids` never lists `@everyone` (it is implicit); `Message.nonce` is left
  out of the JSON when absent.
- **Gateway:** client ops are sent as `{op, d}` without `s` (the server ignores `s` if present);
  `heartbeat.last_seq` is `0` before any dispatch was received (sequences start at 1);
  `ready.voice.quic_port` / `cert_sha256` are `null` when voice is disabled;
  `ready.instance.limits = {max_upload_mb, max_message_length}` (same as `GET /instance`).
- **Permissions helpers:** besides `compute`, `highest_position`, `can_manage_member` and
  `can_manage_role` implement the hierarchy rule (owner ranks above everyone, nobody acts on the owner).

## 2026-10-30: Server config loading details

- **`toml` crate:** added `toml = "=0.8.23"` (the version `figment` already depends on, so nothing new is
  downloaded) to print the effective config as TOML in `check-config`. Approved by the developer;
  section 6 does not list it. `clap` gets its `env` feature for `HERMES_CONFIG`.
- **Precedence:** defaults < `hermes.toml` < `SERVER_PORT` < `HERMES__SECTION__KEY`. Section 18.5 only says
  `SERVER_PORT` is a fallback when `HERMES__NETWORK__PORT` is unset; it also wins over the file because the
  Pterodactyl egg runs `init`, which writes `port = 7700`, and the panel's allocation must still apply.
  An empty `SERVER_PORT` is ignored; an invalid one is an error.
- **Strictness:** unknown keys (in the file or as `HERMES__…` variables) are errors, so typos are not
  silently ignored. String keys also accept env values that look like numbers or booleans
  (`HERMES__INSTANCE__NAME=1337`).
- **Validation:** errors for port 0, `tls.mode = "files"` without both paths, empty `instance.name` or
  `storage.data_dir`, zero sizes/rate limits, `limits.max_group_dm_size < 2`, bitrates outside Opus's
  6000–510000 range or default > max, and an invalid `log.level` filter. An empty `instance.public_host`
  is only a warning, so a container configured purely through env vars still starts. Whether the TLS files
  exist is checked by the TLS item, not here.
- **`init`** refuses to overwrite an existing file (exit code 1) and creates missing parent directories.
- **`run`** only loads the config and sets up logging for now; it exits with code 1 until the server exists.

## 2026-10-31: SQLite, snowflakes and the state cache

- **`synchronous = NORMAL`:** set on every connection besides the pragmas from section 7.3. It is the
  usual setting with WAL: a power loss can lose the last commits but cannot corrupt the database.
- **Two pools:** a read pool (4 connections) and a write pool with a single connection, as the
  "SQLite write contention" row in section 21 says. Migrations run on the write pool.
- **Offline query data:** `crates/hermes-server/.sqlx/` is committed so builds need no database. After
  changing a query, regenerate it with `cargo sqlx prepare` (`sqlx-cli` 0.9.0, a dev tool, not a crate
  dependency); the exact commands are in `src/db/mod.rs`. `build.rs` makes the crate rebuild when a
  migration changes.
- **Snowflake overflow:** when all 4096 sequence numbers of a millisecond are used, the generator moves
  on to the next millisecond instead of sleeping, so it never blocks while holding its lock. A clock
  going backwards reuses the last timestamp, so IDs stay unique and increasing within one process.
- **Cache shape:** users are cached once (`Cache::users`); channels keep DM recipients as IDs
  (`recipient_ids`) and the wire `Channel::recipients` stays empty in the cache. Presences and voice
  states are runtime-only maps. `Cache::space_permissions`/`channel_permissions` wrap
  `hermes_proto::permissions::compute` (DMs use `Permissions::DM_RECIPIENT`).
- **`0002_search.sql`:** an external-content FTS5 table plus insert/delete/update-of-content triggers.
  A soft delete clears `content`, which removes the message from the index.

## 2026-10-31: Auth, sessions, setup invite and invites

- **`invites.creator_id` is nullable** (approved by the developer): migration `0003_invite_creator.sql`
  rebuilds the table with `creator_id INTEGER REFERENCES users(id) ON DELETE SET NULL` (NULL = created
  by the server) and `code TEXT NOT NULL PRIMARY KEY` (SQLite otherwise allows NULL text keys).
  `Invite.creator_id` in `hermes-proto` is `Option<Id>`. The setup invite is created before any user
  exists, and `hermes-server invite create` has no user either.
- **`/users/@me`** (approved): `GET /users/@me` and `POST /users/@me/password` (revokes all other
  sessions) are included here although no checklist item names them; `PATCH /users/@me` and
  `GET /users/{id}` are left for later.
- **`run` serves HTTP(S)** now (approved): the API router on the TLS listener (or plain HTTP with
  `tls.mode = "off"`). Graceful shutdown, `/healthz` and the "Hermes is ready" line stay with their own item.
- **`reqwest` uses `rustls-no-provider`:** the workspace entry changed from `rustls` to
  `rustls-no-provider`, because `rustls` pulls in `aws-lc-rs` (see the `ring` entry above). Code that
  builds a `reqwest::Client` must install the `ring` provider first
  (`rustls::crypto::ring::default_provider().install_default()`); the server tests do.
- **Registration rules the plan left open:**
  - The very first account always needs an invite (the setup invite), in every mode, so a random
    visitor cannot become the instance admin. `closed` refuses everything else.
  - `invite_only` without an invite and `closed` answer `REGISTRATION_CLOSED`; an unknown, expired or
    used-up invite answers `INVITE_INVALID`.
  - Usernames are trimmed and lowercased before the `^[a-z0-9_.]{2,32}$` check (login lowercases too).
    Passwords: `registration.min_password_length` to 256 characters. Device names: 1 to 64
    characters, no control characters.
  - Login runs a dummy argon2 check for unknown usernames (no timing hint), and answers
    `ACCOUNT_DISABLED` only after the right password.
- **Sessions:** `last_seen_at`/`last_ip` are written at most once a minute per session, not on every
  request. `last_ip` is the TCP peer, or the `X-Forwarded-For` client when the peer is in
  `network.trusted_proxies`; IPv4-mapped IPv6 peers are stored as plain IPv4. A disabled user's
  requests are refused through the cached `disabled` flag. `DELETE /auth/sessions/{id}` only finds the
  caller's own sessions (`NOT_FOUND` otherwise); closing the gateway connection comes with the gateway.
- **Setup invite:** single use, 24 hours, instance-only. A restart before anyone registered reuses the
  newest unused server-created instance invite instead of printing a new code each time. The link uses
  `instance.public_host` (or the bind address, or `localhost`) and the public port, with
  `#fp=sha256:…` when `tls.mode = "self_signed"`. The banner goes to stdout, not the log, so it is
  visible in every log format. The first account becomes `instance_admin` and owns a space named
  `instance.name` with `@everyone` (`DEFAULT_EVERYONE`), `#general` and `General` (voice,
  `voice.default_bitrate`).
- **Invites:**
  - `expires_in_secs` absent = 7 days, `0` = never, at most 30 days. `max_uses` absent or `0` = unlimited.
  - A use is taken with a single conditional `UPDATE … RETURNING`, so concurrent joins cannot go over
    `max_uses`. Already being a member returns the space without using up the invite.
  - `POST /invites/{code}` returns a `SpaceSnapshot` ("Space (full, as in Ready)"). An instance-only
    invite there answers `INVITE_INVALID`.
  - Space endpoints answer `NOT_FOUND` to non-members, so they cannot probe which spaces exist.
  - `DELETE /invites/{code}`: the creator, members with `MANAGE_INVITES`, or (instance-only invites)
    instance admins.
  - `hermes-server invite create [--space ID] [--uses N] [--expires 30m|24h|7d|never]` defaults to
    7 days and unlimited uses, and prints both link forms.
- **Deferred:** the per-IP login rate limit goes with the "Rate limits" item, and gateway events for
  joins with the gateway item.
- **Test fix:** the config tests used `figment::Jail::clear_env`, which removes every variable of the
  process, including `SystemRoot`. On Windows, other tests running in parallel then fail to create
  sockets (WSA error 10106). The jail now only clears the environment when a `HERMES*`/`SERVER_PORT`
  variable is set, and puts all other variables back right away.

## 2026-10-31: Spaces, channels, roles, members, overrides REST and the audit log

Approved by the developer as a plan before implementation.

- **Gateway events** for these changes come with the gateway item ("all dispatch events for the
  above"); the handlers only update the database and the cache for now.
- **`PATCH` bodies:** a missing field is unchanged; for fields that can be empty (`description`,
  `icon_id`, `topic`, `nickname`, `color`, `parent_id`, `user_limit`) `null` clears them
  (`hermes_proto::api::Patch<T>`). A `PATCH` that changes nothing writes nothing and is not audited.
- **Write lock:** `AppState::writes` (a `tokio::sync::Mutex`) is held by handlers that read the cache,
  write the database and then update the cache, so two changes cannot interleave and leave the cache
  different from the database.
- **One permission path:** `api::perms` (`space`/`require_space`, `channel`/`require_channel`) is used by
  every space and channel handler (section 13). Non-members get `NOT_FOUND` for spaces, and users
  without `VIEW_CHANNEL` get `NOT_FOUND` for channels.
- **Spaces:** `POST /spaces` returns a `SpaceSnapshot` like `ready` and creates the same layout as the
  first-start space (`@everyone`, `#general`, `General`). `limits.max_spaces_per_user` counts owned
  spaces (`BAD_REQUEST` when reached; `0` = `MISSING_PERMISSION` for everyone but instance admins);
  instance admins have no limit. Names 2–64 characters, descriptions up to 512. `icon_id` must be an
  image (`image/*`) uploaded by the caller. Transfer: owner only, to another member.
- **Roles:**
  - Names 1–32 characters; `color` at most `0xFFFFFF`.
  - A new role goes in at position 1 and the other roles move up by one.
  - `PATCH` cannot change `position`; only `PUT …/roles/order` can. A reorder may only move roles
    below the caller's highest role to positions below it; afterwards all positions are renumbered
    1..n in the new order (ties broken by ID). It returns every role of the space.
  - Roles and overrides can only add bits the caller has (owners and administrators have all).
    Removing bits is not restricted.
  - Of `@everyone` only `permissions` can change (`BAD_REQUEST` otherwise).
- **Members:**
  - `{uid}` may be `@me` in `PATCH` and `DELETE /spaces/{id}/members/{uid}`.
  - Nicknames 1–32 characters. Your own needs `CHANGE_NICKNAME` (or `MANAGE_NICKNAMES`); anyone
    else's needs `MANAGE_NICKNAMES` and the hierarchy rule.
  - `role_ids` is the complete new list; every added or removed role must be below the caller's
    highest role, and the target must be below the caller unless it is the caller.
  - Changing your own nickname is not audited; leaving is not audited. The owner cannot leave
    (`BAD_REQUEST`).
  - `GET …/members` is ordered by user ID (`?after&limit`).
- **Bans:** `BAN_MEMBERS` plus the hierarchy rule when the user is a member; users who are not members
  can be banned too. A ban removes the membership. Banning again replaces the reason but keeps the
  date. `delete_messages_hours` (0–168) soft-deletes the user's messages in the space from that
  window: content cleared, unpinned, reactions and mentions removed, attachments unlinked.
  `reason` up to 512 characters.
- **Channels:**
  - Names 1–100 characters (no lowercasing), topics up to 1024, `slowmode_secs` up to 21600 (not
    for categories).
  - Only voice channels take `bitrate` (6000 to `voice.max_bitrate`, default
    `voice.default_bitrate`) and `user_limit` (up to `voice.max_users_per_room`; `0` is stored as
    unlimited).
  - A parent must be a category of the same space; categories have no parent.
  - New channels go last in the space (highest position + 1).
  - Deleting a category moves its children to the top level.
  - `PUT …/channels/order` sets the given positions (no renumbering); `parent_id` absent = unchanged,
    `null` = top level. It returns the space's channels the caller can see.
  - DMs are not handled here: they answer `MISSING_PERMISSION` until the DM item.
- **Overrides:** `MANAGE_CHANNELS` + `MANAGE_ROLES` in the channel. `allow` and `deny` must not share
  bits (`VALIDATION_FAILED`). The bits must be ones the caller has in the channel.
- **Audit log:** actions are `space_update`, `space_transfer`, `member_update`, `member_kick`,
  `member_ban`, `member_unban`, `role_create`, `role_update`, `role_delete`, `roles_reorder`,
  `channel_create`, `channel_update`, `channel_delete`, `channels_reorder`, `override_update`,
  `override_delete` and `message_delete` (used by the messages item). `details` is a JSON object,
  usually the changed fields. Entries are written in the same transaction as the change.
  `GET …/audit-log` is newest first (`?before&limit`). `AuditEntry` and `Ban` are new
  `hermes-proto` models.

## 2026-10-31: Messages REST

Approved by the developer as a plan before implementation.

- **Gateway events** (`message_create`, `reaction_add`, `typing_start`, `read_state_update`, …) come
  with the gateway item; `POST …/typing` only checks `SEND_MESSAGES` for now.
- **Sending:**
  - `nonce` is required (1–64 characters, no control characters) and echoed in the `POST`
    response.
  - `content` is stored exactly as sent (not trimmed): at most `limits.max_message_length`
    characters, no control characters except `\n` and `\t` (so `\r` is rejected). Content that is
    only whitespace needs at least one attachment.
  - `reply_to_id` must be a message of the same channel (a deleted one is allowed; its excerpt is
    empty).
  - `attachment_ids` (max 10, duplicates ignored) must be the sender's own uploads that are not
    attached yet, and need `ATTACH_FILES`. Uploading is the attachments item; tests create the rows
    with SQL.
  - Categories answer `BAD_REQUEST` ("Categories have no messages").
  - `channels.last_message_id` never moves backwards, even if concurrent sends commit out of order.
- **Slowmode** is kept in memory per channel and user (`messages::Slowmode`) and skipped with
  `BYPASS_SLOWMODE`; it answers `RATE_LIMITED` with `retry_after_ms`. The `messages_per_5s` limit
  stays with the "Rate limits" item.
- **History:** `?before|after|around&limit` (at most one anchor, `BAD_REQUEST` otherwise), oldest
  first. `around` returns up to `limit / 2` older messages, then the message itself and newer ones.
  Needs `VIEW_CHANNEL` + `READ_HISTORY`, like `GET …/messages/{mid}` and `GET …/pins`. Deleted
  messages are returned as tombstones (`deleted: true`, empty content, no attachments, reactions or
  mentions).
- **Edit:** author only, not for system messages. Stored mentions are recomputed, but mention counts
  are not raised again. An empty edit is allowed only when the message has attachments.
- **Delete** (author, or `MANAGE_MESSAGES`): soft delete that clears the content, unpins, removes
  reactions and mentions, and unlinks attachments (file cleanup comes with the attachments item).
  Deleting someone else's message in a space is audited as `message_delete`. In DMs nobody can delete
  others' messages.
- **Pins:** `MANAGE_MESSAGES` in spaces, any recipient in DMs; pinning twice is not an error. No pin
  limit. `GET …/pins` is newest first (by message ID; there is no pin time).
- **Reactions:**
  - The emoji must be one Unicode emoji known to the `emojis` crate (listed in section 6;
    added to `hermes-server`).
  - Joining an existing reaction needs only `VIEW_CHANNEL`; a new emoji needs `ADD_REACTIONS`, and
    a message can have at most 20 different emoji (`BAD_REQUEST`).
  - Adding twice or removing a missing own reaction is not an error. Removing others' reactions
    needs `MANAGE_MESSAGES`. Deleted messages answer `NOT_FOUND`.
  - `Message.reactions` lists each emoji in the order its oldest remaining reaction was added.
- **Mentions:** `@everyone`/`@here` count only with `MENTION_EVERYONE`; roles only if mentionable or
  with `MENTION_EVERYONE`; user mentions only for existing users; mentions inside code do not count
  (the markup parser ignores them). `Message.mentions` holds only the mentions that count.
  `read_states.mention_count` goes up by one per message for every mentioned user who can see the
  channel (not the author), unless they already read past it. `@here` counts users whose presence is
  not offline/invisible; presence comes with the gateway, so until then it counts nobody.
- **Ack:** `VIEW_CHANNEL`; the message must be in the channel (deleted is fine). `last_read_id` is set
  to it, even if that is older (marking a channel unread), and `mention_count` is reset to 0.

## 2026-10-31: Gateway and DM details the plan leaves open

- **Group DM ownership:** when the owner leaves (or is removed), the remaining recipient with the
  lowest user ID becomes the owner. When the last recipient leaves, the group DM is deleted.
- **1:1 DMs cannot be left:** `DELETE /channels/{id}` on a 1:1 DM answers `BAD_REQUEST`; `PATCH`
  answers `MISSING_PERMISSION`; `PUT/DELETE /dms/{id}/recipients/{uid}` answer `BAD_REQUEST`.
- **`Channel.recipients` lists everyone,** the requesting user included, sorted by ID.
- **`POST /dms`:** the caller may list themselves; they are ignored. `max_group_dm_size` counts the
  caller. With `dm_policy = "shared_space"` the caller must share a space with every recipient
  (and with anyone they add to a group DM); otherwise `MISSING_PERMISSION`.
- **`ReactionSummary.me` is `false` in broadcast `message_update`:** one payload is serialized for
  all viewers, so it cannot carry per-viewer flags. Clients keep their own `me` from
  `reaction_add` / `reaction_remove`. The REST response to the editor still has the correct `me`.
- **`ready.presences` lists only users who are not offline;** clients treat missing users as
  offline. A user's own sessions also receive their `presence_update` events.
- **`ready.voice.cert_sha256` is `None`** until the voice server exists (Phase 4);
  `quic_port` is the public port when voice is enabled.
- **Ops before identify:** every op except `identify`/`resume` (heartbeat included) closes with
  4003. A connection that neither identifies nor heartbeats for two intervals is closed with 4009.
  After `invalid_session` the client may identify on the same connection.
- **`typing_start`** is not sent back to the typing user; `read_state_update` goes to all of the
  user's sessions, including the one that acked.
- **Resuming a session that still has a connection** closes the old connection with 4009.
- **Test-only dependency:** `futures-util` (already in the lock file through axum) is a
  dev-dependency of `hermes-server` for the gateway test client's `StreamExt`/`SinkExt`.

## 2026-10-31: Attachment details the plan leaves open

- **What counts as an image:** only PNG, JPEG, GIF and WebP, detected from the file's bytes, that
  decode within limits (16384 px per side, 256 MiB per decode, at most 2 decodes at a time). They
  get `width`/`height`, the detected `content_type` and `Content-Disposition: inline`. Everything
  else (SVG included) keeps the declared `Content-Type` (parameters dropped, lowercased; invalid →
  `application/octet-stream`) and is served with `Content-Disposition: attachment`.
- **Thumbnails are lossless WebP:** the `image` crate (section 6) only encodes lossless WebP.
  Images that already fit in 400×400 are re-encoded at their own size, never upscaled. GIFs and
  animated WebPs use their first frame.
- **Filenames:** `X-Filename` is required, percent-decoded as UTF-8; any directory part is dropped,
  whitespace trimmed, control characters rejected, at most 255 bytes. The filename in the download
  URL must match the stored one, or the answer is `NOT_FOUND`. A file literally named `thumb`
  cannot be downloaded by name, because `/attachments/{id}/thumb` is the thumbnail route.
- **Access:** an attachment on a message needs `VIEW_CHANNEL` on its channel (DM: recipient). An
  avatar or icon is visible to every signed-in user. An unattached upload (also after its message
  was deleted) is visible only to its uploader. Every denial answers `NOT_FOUND`.
- **Quota:** `max_total_storage_gb` counts originals only (not thumbnails), in GiB. Uploads over
  `max_upload_mb` (MiB) are refused from `Content-Length` when present, otherwise while streaming.
- **Cleanup:** every hour, uploads created more than 24 h ago that are on no message and are no
  avatar or icon are deleted, rows and files. Unfinished uploads in `files/tmp/` are removed on start.
- **Downloads** support HTTP ranges and conditional requests (`tower-http`'s `ServeFile`) and send
  `Cache-Control: private, max-age=31536000, immutable`.

## 2026-10-31: Rate limit, validation and shutdown details the plan leaves open

- **REST limit** (`requests_per_second`): a `tower_governor` token bucket per bearer token (keyed by
  its SHA-256, so raw tokens are never kept in memory), or per client IP for requests without a
  token. It covers everything under `/api/v1` (the gateway upgrade included), not `/healthz` or
  the HTML pages. `tower_governor` only reports whole seconds, so its 429s say
  `retry_after_ms` ≥ 1000. A `Retry-After` header (seconds) is added to every 429.
- **Login limit** (`login_attempts_per_minute`): per client IP on `POST /auth/login`, successful
  attempts included.
- **Message limit** (`messages_per_5s`): a token bucket per `(channel, user)`, checked after the
  permission checks and before slowmode, with millisecond `retry_after_ms`. A small built-in limiter
  (`ratelimit::KeyedLimiter`), because `tower_governor` works per request, not per channel.
- **Gateway limit:** 120 client ops per connection as a token bucket (burst 120, 2 per second
  refill). The op after that closes the connection with 4008.
- **The `tonic` feature** of `tower_governor` (on by default) is disabled; only `axum` is used.
- **JSON bodies** are limited to 256 KiB (`PAYLOAD_TOO_LARGE`). Uploads stream and have their own limit.
- **Shutdown:** on Ctrl+C/SIGINT (and SIGTERM on unix) the server stops accepting, closes every
  gateway connection (identified or not) with 4100, waits up to 8 s for running requests,
  aborts the rest, then closes the database (WAL checkpoint), so it exits within 10 s.
  "Hermes stopped" is logged at the end.
- **Ready line:** logged as `Hermes is ready on <network.bind>:<port>` once the socket is bound and
  the server runs. With `bind = "::"` it reads `Hermes is ready on :::7700`.

## 2026-10-31: `arc-swap` for the certificate reload; reload details

- **What:** added `arc-swap = "=1.9.3"` to the workspace, used only by `hermes-server`'s
  `tls::CertReloader`, a rustls `ResolvesServerCert` that serves the current `CertifiedKey`.
- **Why:** PLAN.md section 9.1 names the design ("rustls `ResolvesServerCert` backed by an
  `ArcSwap`"), but section 6 does not list the crate. The developer said to follow the plan.
- **Scope:** only `tls.mode = "files"` reloads; a `self_signed` certificate never changes by design.
- **Triggers:** SIGHUP (unix only; Windows has no such signal) reloads unconditionally. Every 60 s the
  modification times of `cert_file` and `key_file` are compared with those of the last attempt, and
  any change triggers a reload.
- **Failures:** a pair that cannot be read, parsed or does not match is logged as an error and the
  old certificate stays in use. A broken pair is reported once per change, not every minute.
- **Open connections** are not touched; only new handshakes get the new certificate.

## 2026-10-31: Landing and invite page details the plan leaves open

- **Space icon:** attachment downloads need a token, so the invite page inlines the icon's
  thumbnail as a `data:image/webp;base64,…` URI. Without an icon (or thumbnail) it shows the
  first letter of the space name.
- **Account-only invites** (the first-start setup invite and `hermes-server invite create` without
  `--space`) show the instance name and "Create your account on this Hermes server."
- **Landing page** shows the instance name, its description, the address to type in the app
  (`host`, or `host:port` when the public port is not 7700) and the download link.
- **Headers:** `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline';
  img-src data:; …`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
  `Referrer-Policy: no-referrer` (the invite code is in the URL), `Cache-Control: no-store` on
  invite pages and `no-cache` on the landing page. Invalid, expired and used-up codes answer 404.
- **The fingerprint for `#fp=`** in the "Open in Hermes" link comes from `AppState::fingerprint`,
  set by `run` after TLS is loaded.

## 2026-10-31: hermes.senko.tools serves a static landing page through nginx on 80/443

- **What:** on the reference VPS, nginx (which already hosts the other `*.senko.tools` sites)
  serves `hermes.senko.tools` on ports 80/443. Port 80 redirects to HTTPS, and 443 serves the static
  page from `deploy/www/hermes.senko.tools/index.html`, installed to `/var/www/hermes`. The site
  config is `deploy/nginx/hermes.senko.tools`. `https://hermes.senko.tools/invite/<code>` (port left
  off) redirects to `https://hermes.senko.tools:7700/invite/<code>`. Hermes itself still runs on
  7700 TCP+UDP with its built-in `/` and `/invite/<code>` pages, and nothing is proxied.
- **Changes to PLAN.md section 18.8:** ports 80/443 are no longer "left free". The certificate is
  renewed by certbot's `nginx` authenticator instead of `--standalone` (18.8 step 4 already names
  this option once a web server uses port 80). The deploy hook from step 4 is unchanged.
  The Hetzner firewall must allow TCP 443 as well as 80.
- **Server cleanup:** earlier certbot runs had added `hermes.senko.tools` blocks to
  `/etc/nginx/sites-available/00-catch-all` that answered 421/404. They were removed (backup
  `00-catch-all.bak-20261031-132144` next to it) in favour of the dedicated site.
- **Why:** requested by the developer, so the bare domain shows something useful in a browser.

## 2026-10-31: Minimal deployability (install.sh, systemd, Docker) details

Approved by the developer as a plan before implementation.

- **`hermes-server backup <output>`** (from section 9.1, listed again in Phase 7) is added now, so the
  reference instance can run the nightly backup from 18.8 step 9 from day one: `VACUUM INTO` on a
  separate connection, refuses an existing output file or a missing database, and reminds the user
  that `files/` must be copied separately.
- **`hermes-server healthcheck`** (not in section 9.1) is new: the distroless image has no shell or
  curl for the `HEALTHCHECK`. It sends `GET /healthz` to the loopback address of the bind address
  (or the bind address itself when it is specific), over TLS without certificate checks when
  `tls.mode` is not `off` (it only asks whether the local server answers), and exits 0 on `ok`.
- **`install.sh`:**
  - It downloads three release files: `hermes-server-<arch>-unknown-linux-musl.tar.gz`,
    `hermes.service` and `SHA256SUMS`. Both other files are checked against `SHA256SUMS`. The unit
    comes from the release, so the installed unit matches the binary.
  - New options: `--version vX.Y.Z` (or `HERMES_VERSION`) and `--from DIR` (or `HERMES_FROM`),
    which installs files that are already downloaded.
  - Why `--from`: the repository is private for now, so the VPS cannot download releases without
    a token. GitHub's `releases/latest` also skips pre-releases.
  - A new config gets `storage.data_dir = "/var/lib/hermes"`. It is owned by `root:hermes`, mode
    640, in `/etc/hermes` (750).
  - It does not restart a running server after an upgrade; it prints the command (18.8 step 10).
- **systemd unit:** the hardening from 18.4 plus `ProtectHome`, `PrivateTmp`, `PrivateDevices`,
  kernel/cgroup protection, `RestrictAddressFamilies`, `CapabilityBoundingSet=CAP_NET_BIND_SERVICE`,
  `UMask=0027` and `TimeoutStopSec=20`.
- **Docker:**
  - The builder is `rust:alpine`, which is musl natively; it builds for `$(uname -m)`, so the same
    Dockerfile works on arm64.
  - The final stage is `gcr.io/distroless/static-debian12`, with `/data` owned by UID 10001 and
    `HERMES_CONFIG=/data/hermes.toml` (optional).
  - The compose file keeps `./data:/data` from 18.3. A bind-mounted directory created by Docker
    belongs to root, so the comments add `chown 10001:10001 data`.
  - No image is published to ghcr.io yet (Phase 7).
- **`release.yml` (early, partial):** a `v*` tag builds only what `install.sh` needs (x86_64 musl
  server, `hermes.service`, `SHA256SUMS`) and creates the GitHub release, as a pre-release when
  the tag contains `-`. aarch64, Windows, the Docker image and the clients come with the Phase 7
  item.
- **Reference instance files:** `deploy/letsencrypt/hermes.sh` (the 18.8 step 4 deploy hook) only
  reacts when `RENEWED_LINEAGE` is its own certificate, because the VPS renews other domains too.
  `deploy/cron/hermes-backup` is step 9; the backup directory is owned by `hermes` so the backup
  can run as that user.
- **`.gitattributes`** forces LF for `deploy/**` and `*.sh`, so scripts work when checked out on
  Windows with `core.autocrlf`.

## 2026-10-31: `GET /instance` and endpoint coverage tests

- **`GET /api/v1/instance`** was in section 10 but not built yet; it is added with the integration
  tests item because the client (Phase 2) needs it to probe instances. The body is
  `hermes_proto::api::InstanceResponse` (`name`, `description`, `version`, `protocol_version`,
  `registration_mode`, `limits`, `voice.enabled`). `RegistrationMode` moved from the server config
  to `hermes-proto` (the server re-exports it) so both sides share the enum.
- **`tests/endpoints.rs`** adds the cross-cutting checks of sections 13 and 19:
  - every authenticated route refuses a missing, unknown or logged-out token with
    `401 UNAUTHORIZED`;
  - each space permission bit (`ADMINISTRATOR` through `MANAGE_MESSAGES`) is tested on one
    endpoint, once without the bit (refused) and once with it (allowed). `MENTION_EVERYONE` and
    `BYPASS_SLOWMODE` change results instead of refusing and stay covered in `messages.rs`.
  
  The voice bits come with Phase 4.
- **Not covered:** endpoints that belong to later phases (`PATCH /users/@me`, `GET /users/{id}`,
  search, voice, notification settings, admin) do not exist yet.

## 2026-10-31: `hermes-client-core` details

Approved by the developer as a plan before implementation (voice and audio are Phase 4).

- **New dependencies:**
  - `rustls-platform-verifier = "=0.7.1"` (already in the lock file through reqwest): CA validation
    uses the OS trust store.
  - `toml_edit = "=0.22.27"` (the version `toml` uses; named in section 17): writes `config.toml`
    while keeping comments.
  - `hermes-server` is a **dev-dependency** of `hermes-client-core`, so its integration tests can run
    a real server (section 19). Section 5.2's rule ("clients never depend on hermes-server") still
    holds for the normal dependencies.
  - `futures-util` is now a normal dependency too (the gateway's `StreamExt`/`SinkExt`).
  - `uuid` is not used: nonces are UUID v4 strings built from `rand`.
- **Addresses:** `HostPort` stands for both `InstanceUrl` and `HostPort` from section 14.2.
  - It accepts `host`, `host:port`, `[v6]:port`, a bare IPv6 address or an `https://`/`hermes://`
    URL. The default port is 7700 and hosts are lowercased.
  - The instance ID is `host`, plus `:port` when it is not 7700. `known_hosts.toml` is keyed by
    `host:port`.
  - Both invite link forms are parsed, and so is `#fp=`. A fingerprint can be written as
    `sha256:<hex>`, as plain hex, or as `AB:CD:…`.
- **Certificate trust:**
  - `probe_instance` reads `GET /instance` with a verifier that records the certificate and
    whether CA validation would accept it. It answers `Trusted` (CA-valid), `Pinned`, `Unknown`
    (ask, trust on first use) or `Changed`.
  - `login`/`register` probe first and refuse `Unknown` (`UntrustedCertificate(fp)`) and
    `Changed` (`CertificateChanged`).
  - All other connections use either the OS verifier or a verifier that accepts exactly the pinned
    fingerprint (host name and issuer are not checked for pinned certificates).
  - A server with another `protocol_version` is refused with a clear message.
- **Tokens:** keyring service `hermes`, account `<username>@<instance id>`. If the keyring fails,
  `tokens.toml` in the config directory is used instead (mode 600 on unix). A successful keyring
  write removes any copy from the file.
- **Config:**
  - Unknown keys are warnings, except in the free-form `[keys]`, `[tui.keys]` and `[[layout]]`.
  - The float settings are `f64`, so they are written as typed (`0.6`).
  - `set_setting("a.b", value)` accepts TOML values; anything that does not parse is taken as a
    string. Unknown keys and wrong types are refused.
  - Saving merges into the existing file (comments and unknown keys stay) and writes a temporary
    file, then renames it.
  - Hot reload watches the directory (debounced 250 ms) and sends `CoreEvent::ConfigReloaded`.
- **Gateway:**
  - Backoff is 1 s, 2 s, 4 s, … capped at 30 s, with ±20 % jitter. After a connection that reached
    `ready`/`resumed`, the next retry starts at attempt 1.
  - `hello`, `ready` and `resumed` each have 15 s to arrive.
  - The client reconnects if no `heartbeat_ack` came for two intervals plus 5 s.
  - Close codes follow `CloseCode::action()`. 4004 and 4011 report `LoggedOut` and delete the
    stored token; 4010 reports `Failed`.
  - `set_online(false)` closes the connection but keeps the session, and `set_online(true)` resumes
    it. Its own `Offline` status makes the offline queue testable and gives the UIs an "offline"
    switch.
  - Frames this client cannot decode (events from newer servers) are skipped.
- **Store:**
  - `ready` replaces everything. Loaded message windows are dropped because they may have gaps, but
    pending messages stay; the visible channels reload their latest page right away.
  - Members come from the snapshots. Only spaces at the 1000-member cap load the full list over REST.
  - A new message is added to a window only when the window is "live" (ends at the newest message).
  - My own new messages mark the channel read locally, and the core acks them. A message that
    mentions me raises `mention_count` locally (the server does not send `read_state_update` for
    mentions).
  - Typing indicators last 10 s, counted with the client clock.
  - Windows not shown for 10 minutes are trimmed to the last 50 messages (checked every minute). A
    window in the middle of history is dropped instead.
- **Optimistic sending:** one outbox task per instance sends in order and only while connected.
  - Network errors put the message back in the queue (`Queued`) and retry after 2 s.
  - `RATE_LIMITED` waits `retry_after_ms`.
  - Any other error is `Failed(reason)`, with `retry_message` / `discard_message`.
  - With 50 messages queued, new ones fail right away.
- **API shape:** instead of 1:1 `Core` methods for every endpoint, `Core::rest(inst)` returns the
  signed-in `Rest` client, which has one typed method per endpoint. Message actions that touch the
  store (send, edit, delete, react, history, read, typing, upload) are on `Core`. Voice methods,
  `InputLevel`, `VoiceStatus` and `Speaking` come with Phase 4.
- **Notifications:**
  - The level is the channel's setting, else its space's, else `all` for DMs and `mentions` for
    spaces.
  - Nothing is notified while muted, in `dnd`, for my own messages, or while the channel is in a
    visible pane of a focused window (`set_visible_channels`, `set_app_focused`).
  - `[notifications] enabled = false` turns them all off.
- **Slash commands:**
  - `/me x` sends `_x_`. `/shrug` appends `¯\_(ツ)_/¯`, escaped so the markup parser keeps it.
  - `//text` sends `/text`. `/status` also accepts `away` and `offline`.
  - `/join`, `/leave`, `/mute` and `/deafen` are parsed now and do something once voice exists.

## 2026-10-31: `hermes-tui` details

Approved by the developer as a plan before implementation (voice keys come with Phase 5, `/`
search with Phase 6; both show "comes in a later version").

- **Local time without a date crate:** section 6 lists none, so the UTC offset is read once at
  startup: `date +%z` on unix, and `GetTimeZoneInformation` on Windows through the `windows` crate
  (already listed; the TUI enables its `Win32_System_Time` feature, `cfg(windows)` only).
  - A DST change while the TUI runs is only picked up after a restart.
  - `[buffer] timestamp_format` supports `%H %M %S %d %m %Y %y %%`.
- **Keys:**
  - Every normal-mode action of section 16.2 has a name in `[tui.keys]` (`move_down`, `join_voice`,
    `top`, …; `:help` lists them). Keys are written `J`, `Ctrl+u`, `Enter`, `Tab`, or two
    characters such as `gg`. Unknown actions and keys that do not parse are warnings.
  - Arrow keys always move. One extra action is not in the plan: `retry` (`R`), which re-sends a
    failed message (Enter on it works too).
- **Commands** beyond section 16.2:
  - `:instance remove [id]`, `:logout`, `:invite` (prints the web invite link, with `#fp=` when
    the instance is pinned), `:retry`, `:offline`/`:online`, `:help`.
  - `:join <channel>` opens a text channel by name, preferring the current space.
- **Add instance / login:**
  - `:instance add <invite link or address>` probes the server. An invite's `#fp=` is trusted
    without asking (section 13); an unknown or changed certificate shows its fingerprint and asks.
  - The account form starts on "create an account" for invite links and "sign in" otherwise;
    Ctrl+R switches.
  - Signing in with an existing account and an invite link then joins the invite's space.
  - `hermes-tui <link>` does the same on start.
- **Notifications** ring the terminal bell and show in the status line.
- **Attachments:** `:open <n>` downloads attachment n of the selected message (or of the newest
  message with attachments) to a temp folder and opens it with the OS default app (`start`,
  `open`, `xdg-open`). `:upload <path>` sends the file as its own message.
- **Logs** go to `<config dir>/logs/hermes-tui.log` (`HERMES_LOG` sets the filter). There is no
  7-day rotation yet (`tracing-appender` is not in section 6).
- **CLI flags:** `--config-dir <dir>` and an optional link argument.
- **Test-only dependency:** `hermes-server` is a dev-dependency of `hermes-tui` for an end-to-end
  test (invite link → account → open `#general` → send → render with ratatui's `TestBackend`).

## 2026-10-31: GUI skeleton, themes, fonts and settings details

Approved by the developer as a plan before implementation.

- **Binary:** the `hermes-gui` crate builds the `hermes` binary (section 15). Release builds on
  Windows use the `windows` subsystem (no console window). Logs go to
  `<config dir>/logs/hermes.log` (`HERMES_LOG` sets the filter). The `Core` is created in iced's
  `boot`, which runs inside iced's tokio runtime (feature `tokio`).
- **Themes:** a user theme in `<config dir>/themes/<name>.toml` wins over a built-in of the same
  name. Keys it leaves out use the `dark` values, and unknown keys are ignored, both with a
  warning. A theme that does not parse falls back to the built-in of that name, else `dark`. Colors
  are `#rrggbb` or `#rrggbbaa`. The themes folder is watched (debounced 200 ms). Widgets without a
  Hermes style function use iced's defaults, driven by a palette built from the theme.
- **Fonts:** Inter (Regular, Medium, Bold) and JetBrains Mono (Regular, Bold) are bundled as static
  TTF files (SIL OFL, license files next to them in `crates/hermes-gui/assets/fonts/`), not the
  variable fonts. Any other `font.family` is looked up among the system fonts by name. Because iced
  only reads its default font and size at startup, every text goes through helpers that set font
  and size, so `[font]` changes apply live. `font.size` is clamped to 8–32 and `font.ui_scale` to
  0.75–2.0 (section 15.5); the scale is applied through iced's scale factor.
- **Settings persistence:** the Settings buffer has only the Appearance tab so far (theme, font size,
  UI scale, sidebar position and width). Each change is written with `Core::set_setting`, so
  `config.toml` keeps its comments; sliders preview live and save on release. Edits to
  `config.toml` from outside apply live through the core's hot reload.

## 2026-10-31: GUI add-instance, sign-in, links and single-instance details

Approved by the developer as a plan before implementation.

- **`Core::invite_preview(host, code)`** (new in `hermes-client-core`): `GET /invites/{code}`
  without a session, so the dialog shows the invite's space and member count before the account
  step. Like `login`, it requires a trusted (CA-valid or pinned) certificate.
- **Dialog:** address or invite link → probe → certificate step (unknown: trust on first use;
  changed: red warning with both fingerprints and "I re-generated the certificate, trust the new
  one") → preview with Sign in / Create account tabs. An invite's `#fp=` is trusted without asking
  (section 13), as in the TUI. Invite links start on Create account. Create account is enabled for
  `open` registration or whenever there is an invite (the server decides for `closed`, where only
  the setup invite works). Signing in with an invite link also joins its space. A logged-out
  instance in the sidebar opens the same dialog with the username filled in.
- **Single instance:** one channel per config directory, so portable copies do not collide:
  the named pipe `\.\pipe\hermes-<FNV-1a of the lowercased config dir path>` on Windows, the unix
  socket `<config dir>/hermes.sock` elsewhere (a stale socket is removed). A message is one line:
  the link, or an empty line meaning "show the window". The running client unminimizes and
  focuses its window and starts the dialog with the link. Local IPC only, in `hermes-gui`.
- **`hermes://` registration** in the Windows registry stays with the installer (Phase 7); the
  client already accepts the link as its argument.
- **Test-only dependency:** `hermes-server` is a dev-dependency of `hermes-gui` for the end-to-end
  dialog tests, as for `hermes-tui`.

## 2026-10-31: GUI sidebar tree, context menus and collapsed state

Approved by the developer as a plan before implementation.

- **Tree:** built as a flat list of rows from the store (`sidebar/tree.rs`), on top of
  `view::channel_tree` and `view::dm_list`. Spaces are listed in ID order (the store does not keep
  the `ready` order). A collapsed space, category or DM section shows the sum of its channels'
  badges (mention count, else an unread dot); expanded ones show the badges on the channels.
  Voice channels always list their occupants from `voice_states` (name, "muted"/"deaf"); the
  speaking ring comes with Phase 5.
- **Icons:** until an icon font is added, text glyphs mark rows (`#` text, `♪` voice, `@` DM,
  `≡` group DM), not emoji.
- **Selecting** a text channel or DM shows its header in the main area (the message list comes
  with the channel buffer item) and calls `open_channel` and `set_visible_channels`. A single click
  on a voice channel only selects it; double-click to join comes with Phase 5.
- **`collapsed.toml`:** `collapsed = ["<instance id>", "<instance id>/space/<id>",
  "<instance id>/category/<id>", "<instance id>/dms"]`, written on every toggle (temporary file +
  rename). A broken file is ignored with a warning.
- **Context menus:** iced has none, so `widget/context_menu.rs` draws one with `stack` + `pin` at
  the pointer, kept inside the window; outside clicks and Escape close it. The pointer position
  comes from a small wrapper widget around the whole window that records it without sending a
  message per mouse move (uses iced's `advanced` feature). Entries: instance (Reconnect, Go
  offline/online, Log out, Remove instance), space (Invite people: creates an invite and copies
  the link, Copy space ID, Leave space), channel/DM (Mark as read, Copy channel ID). Remove instance
  and Leave space are confirmed first. Space settings, notification level and mute come with
  Phase 6.
- **New DM:** the "+" of the Direct messages section takes comma-separated usernames or display
  names (case-insensitive) of users the store knows; the new DM opens right away.

## 2026-10-31: GUI pane grid, layout, command bar and keybinds

Approved by the developer as a plan before implementation.

- **Panes:** buffers are `Empty`, `Channel` and `Settings` for now (the others come with their
  features). Opening a channel or Settings uses the focused pane. Splitting copies the pane's
  buffer into the new pane, which gets the focus; closing the last pane empties it. The header has
  Members / Maximize buttons and a `⋮` menu (split right/down, maximize, member list, close). The
  docked member list uses `view::member_groups` (a DM lists its recipients).
- **`[[layout]]`:** one table per window holding the pane tree: a split is
  `{ split = "vertical" | "horizontal", ratio, a, b }`, a pane is `{ buffer = "channel" |
  "settings" | "empty", instance, channel, members }`. Saved through `Core::set_setting("layout",
  …)` when the window is closed, restored on start. A layout that does not parse gives one empty
  pane; panes of removed instances or left spaces are emptied.
- **Keybinds:** every action of section 15.6 has a config name (`command_bar`, `next_unread`,
  `prev_unread`, `next_channel`, `prev_channel`, `focus_next_pane`, `focus_prev_pane`,
  `split_right`, `split_down`, `close_pane`, `maximize_pane`, `toggle_member_list`,
  `toggle_sidebar`, `search`, `mark_read`, `toggle_mute`, `toggle_deafen`, `settings`) and the
  default from the table; `[keys]` overrides them, an empty value unbinds. Keys a text input used
  only count with Ctrl, Alt or Super. Search, mute and deafen say "comes in a later version".
- **Punctuation keys on non-US layouts** (approved): a punctuation key in a combo also matches the
  key in its place on a US keyboard, so the default `Ctrl+\` splits on layouts where `\` needs
  AltGr (on a Finnish keyboard: Ctrl + the `'` key left of Enter). A key that types the character
  wins over a US position, and letters only match what they type.
- **Command bar:** subsequence fuzzy match (word starts and runs score higher, gaps lower) over
  channels, DMs, "Message @user" (opens or creates the DM), the keybind actions and "Theme: …".

## 2026-10-31: GUI channel buffer, part A (message list)

Approved by the developer as a plan before implementation.

- **`hermes-client-core::time`:** the TUI's local time module moved to the client core so both
  clients share it; the `windows` crate feature `Win32_System_Time` moved with it.
- **Rows:** built from the store (`buffer/channel/rows.rs`): day separators (local time), the
  "New messages" divider before the first newer message by someone else than me (the read marker
  is taken when the channel is opened in a pane, so the divider stays put while reading), grouping
  with `view::is_grouped`, reply previews ("original message deleted" for an empty excerpt) and
  pending messages after the loaded ones (grouped with my previous message). Deleted messages are
  not shown.
- **Virtualization:** rows have different heights. Each row starts with an estimate (lines at the
  pane width, attachments, reactions, reply) and keeps the height iced's `sensor` widget measures
  once it is shown; only rows within 600 px of the viewport are built, with spacers for the rest.
  The scrollable is anchored to the bottom, so offsets count from the newest message, new messages
  stay in view, and older pages load above without moving the view. Heights are forgotten when the
  pane width changes.
- **Loading and reading:** older history loads when the top is within 400 px. "Jump to present"
  shows when scrolled up or when the window is not live (it then loads the latest page). A channel
  is marked read when its newest message is visible, the window is focused and the window is live
  (on scroll, on new messages and on focus).

## 2026-10-31: GUI channel buffer, part B (message content)

Approved by the developer as a plan before implementation.

- **Markup:** `hermes_proto::markup` spans are flattened into styled pieces in blocks (text, code
  block, quote) and drawn with iced `rich_text`. Mentions show names (`@deleted-role`,
  `#unknown` when unknown) with a tinted background; channel mentions open the channel. Spoilers
  are hidden until clicked (per message). Links open in the default browser; only `http(s)` URLs
  are opened, and on Windows through `rundll32 url.dll,FileProtocolHandler`, never a shell.
- **Fonts:** Inter Italic is bundled too (OFL), so italic text is real italic.
- **Messages:** a letter avatar in the author's colour (avatar images come with profile editing in
  Phase 6) on the first message of a group, the time in the gutter of grouped lines on hover.
- **Reactions:** chips with emoji and count, highlighted when mine; a click toggles. The model has
  no per-user list, so there is no "who reacted" tooltip yet.
- **Hover toolbar:** React (8 quick reactions; the full picker comes with `:` autocomplete in the
  input box item) and `⋯` (copy text, copy ID, pin/unpin with MANAGE_MESSAGES, delete for own
  messages or with MANAGE_MESSAGES, confirmed). Reply and Edit come with the input box.
- **Attachments:** images with a thumbnail show inline at most 400×300 (the thumbnail is loaded
  the first time its row is shown and kept in memory); a click opens the original in a full-window
  viewer (Escape or a click closes it). Other files show a card with a Download button. Downloads
  go to the Downloads folder with a sanitized name (no directory parts, no characters Windows
  forbids, no device names) and never overwrite a file (` (1)`, ` (2)`, …).

## 2026-10-31: TUI `:open` no longer goes through a shell

- **Bug:** `:open` saved an attachment under its uploaded name and ran `cmd /C start "" <path>`.
  The server allows `&` in filenames and Rust only quotes arguments with spaces or quotes, so a
  file named like `x&calc.exe` made cmd run a second command on the opener's Windows machine.
- **Fix (approved):** on Windows the TUI calls `ShellExecuteW` directly (`windows` crate features
  `Win32_UI_Shell` and `Win32_UI_WindowsAndMessaging`, `cfg(windows)` only); other platforms
  already ran `open`/`xdg-open` without a shell. Saved names go through
  `hermes_client_core::files::safe_filename` (shared with the GUI), and files that would run code
  when opened (`.exe`, `.bat`, `.lnk`, `.ps1`, …, see `files::is_program`) are saved but not
  opened.

## 2026-10-31: GUI input box

Approved by the developer as a plan before implementation.

- **`rfd = "=0.17.2"`** (approved; named in section 15.3, not listed in section 6): the native
  file dialog behind the "+" button.
- **Composer:** one draft per channel (kept when switching channels, shared by panes showing the
  same channel), an iced `text_editor`. Enter sends and Shift+Enter is a new line; with
  `buffer.enter_sends = false`, Enter is a new line and Ctrl/Shift+Enter sends. Only the focused
  editor reacts to keys (iced asks every editor). Trailing whitespace is not sent. The popup, the
  reply/edit bar and the upload chips have fixed places in the layout so the editor keeps its
  focus when they appear.
- **Autocomplete:** `view::complete` for the token before the cursor: `@` and `#` anywhere, `:`
  with at least two characters after it, `/` only at the start of the message. Up/Down move,
  Tab/Enter accept, Escape closes. Accepted mentions are inserted in their ID form.
- **Modes:** Reply and Edit (own messages) in the hover toolbar; Up in an empty box edits my last
  message; Escape (or ✕) leaves the popup, then the mode, and otherwise marks the channel read.
- **Slash commands** through `hermes_client_core::commands`, as in the TUI; `/join`, `/leave`,
  `/mute` and `/deafen` say voice comes later.
- **Attachments:** the "+" dialog (multi-select), files dropped on the window (for the focused
  pane's channel) and Ctrl+V with an image on the clipboard (`arboard`, saved as PNG through the
  `image` crate into the temp folder, uploaded as `pasted image.png`); Ctrl+V with text pastes the
  text. Each file is a chip ("uploading…", its size when ready, the error when it failed, ✕ to
  remove). The core reports no byte progress, so there is no percentage. Sending waits until all
  uploads finished; failed ones are left out.

## 2026-10-31: GUI notifications, tray icon and taskbar flash

Approved by the developer as a plan before implementation.

- **Toasts:** `notify-rust`, shown for every `CoreEvent::Notification` (the core decides what is
  notified) while `[notifications] enabled`: title `Alice in #general` (or the DM partner),
  the preview capped at 200 characters ("New message" with `show_preview = false`, "Sent an
  attachment" for empty text), the default sound with `sound = true`. A click on a toast while
  Hermes runs shows the window and opens the channel (Windows reports the activation in-process).
  Until the installer registers an app ID (Phase 7), Windows labels the toasts "Windows
  PowerShell".
- **Taskbar flash:** DMs and mentions while the window is unfocused
  (`request_user_attention(Informational)`), unless `flash_taskbar = false`.
- **Tray icon:** created on the main thread before the event loop; the icon (also the window icon)
  is drawn in code: a rounded "H" in the accent colour, with a red dot while a mention or an unread
  DM exists. Left click shows/hides the window; the menu has Show/hide, Mute and Deafen (disabled
  until voice exists) and Quit. If the tray cannot be created, closing the window quits.
- **Close to tray:** with `general.close_to_tray` (default on) the close button hides the window
  (the layout is saved); Quit in the tray saves and exits. `general.start_minimized` starts hidden
  only when the tray exists.

## 2026-11-01: Voice server details the plan leaves open

Approved by the developer as a plan before implementation.

- **New dependency:** `bytes = "=1.12.1"` (already in the lock file through quinn) for the cheap
  datagram clones section 12.4 names (`bytes::Bytes`).
- **Endpoint:** quinn binds `network.bind:network.port` over UDP with `voice-cert.pem` /
  `voice-key.pem` (same generator and SANs as the self-signed HTTPS certificate), TLS 1.3 only,
  ALPN `hermes-voice/1`, idle timeout 15 s, keep-alive 5 s. A failed bind stops the server.
  `ready.voice` and `/voice/join` carry the fingerprint as `"3F:A9:…"` and `public_port`.
- **QUIC host** in `/voice/join`: `instance.public_host`, or the host of the request's `Host`
  header when that is empty (brackets and port removed).
- **Tokens:** `hms_` + 32 random bytes like session tokens, kept in memory only.
- **Control stream:** the client opens one bi-stream; `auth` must come within 10 s. A bad or
  expired token, or any other op first, answers `error {code: "UNAUTHORIZED"}` and closes the
  connection. A bad `switch` token only answers `error`. Error codes are `ErrorCode` names.
  `kicked` reasons: `joined_elsewhere`, `disconnected` (moderator or `POST /voice/leave`),
  `channel_deleted`, `removed`. After `kicked` and `leave` the connection stays open for a
  later `switch`.
- **Self mute/deafen** arrives both as `PATCH /voice/@me` and `set_state`; a change that changes
  nothing publishes nothing, so one gateway event goes out.
- **Leaving rooms:** the registry removes everyone who can no longer `VIEW_CHANNEL` their room
  (channel deleted, left/kicked/banned from the space, left the group DM, permissions changed).
  Losing `CONNECT` alone does not kick; losing `SPEAK` stops forwarding within 1 s (the bit is
  re-checked at most once a second per speaker).
- **Moderation:** `PATCH /spaces/{id}/voice-states/{uid}` checks the bits in the user's current
  channel (and `MOVE_MEMBERS` in the target one, which the user must be able to view and connect
  to). No role hierarchy check. Audit actions `member_voice_update`, `member_move`,
  `member_disconnect`. A move sends `moved {token}`; the token ignores `user_limit`.
- **Limits:** `user_limit` counts users other than the joining one and is bypassed by
  `MOVE_MEMBERS`; `voice.max_users_per_room` always applies. Both are checked at `/voice/join`
  and again at `auth`/`switch`.
- **DM calls:** a DM room that goes from empty to one participant posts the system message
  `"<display name> started a call"` (author: the caller); when it empties, `"Call ended · N min"`
  (at least 1, same author). Ringing (`call_ring`) needs `CONNECT` in the DM, goes to the listed
  recipients (default: all) who are not in the call, and stops (`call_ring_stop`) for a user when
  they join, and for everyone rung when the caller leaves or the room empties. Rings are forgotten
  after 60 s; the 30 s ring timeout is the client's.
- **`ready.dm_voice_states`** (new field, defaults to empty): voice states of DM and group DM
  calls, which have no space snapshot to live in.
- **Shutdown:** the QUIC endpoint closes with code 0 ("server restarting") with the gateway; clients
  reconnect like after a network loss.
- **`docs/protocol.md`** now exists (human-readable reference, kept in sync with `hermes-proto`).

## 2026-11-01: `hermes-audio` details the plan leaves open

Approved by the developer as a plan before implementation.

- **Layout:** pure pipelines (`capture`, `playback`) built from `dsp` (resampler, downmix, RNNoise
  wrapper, AGC, gate), `codec` (Opus), `jitter`, `mixer` and `sounds`; `devices` opens cpal
  streams; `engine` is one worker thread that owns the streams and pipelines. `hermes-audio`
  depends on `hermes-proto` for the datagram header.
- **Virtual I/O:** `Io::Virtual(Box<dyn VirtualIo>)` replaces the devices with a 48 kHz mono frame
  source and sink, driven in real time (20 ms ticks), so the voicebot and tests run the same
  pipelines without hardware.
- **Gate:** voice activity opens on RMS above `vad_sensitivity` *or* RNNoise probability above
  0.9 (fixed), with a 300 ms hangover; push-to-talk has the release delay; self-mute closes it.
  Every frame is encoded (continuous encoder state, timestamps follow real time); a closed gate
  sends one final frame with `voice_active = 0`, then nothing.
- **AGC:** gain 0.1–10 towards −18 dBFS, adapting only on frames louder than −50 dBFS (attack 10 %,
  release 2 % per frame).
- **Jitter buffer:** target = 40 ms + 3 × RFC 3550 jitter, clamped to 40–200 ms, starting at 60 ms
  and following slowly (5 % per packet). Playback starts when the queued span reaches the target.
  More than target + 4 frames queued skips the oldest. A missing frame uses the next packet's FEC
  if it is there, else concealment; after 5 concealed frames with nothing queued, or right away
  when the last packet had `voice_active = 0`, the stream pauses and re-buffers on the next packet.
- **Playback:** per-user volume 0–200 % and local mute; deafen silences participants but not UI
  sounds; output volume after the soft clip. The output ring is topped up to 30 ms.
- **UI sounds** are synthesized sine tones (join, leave, mute, unmute, ping, ring), so there are no
  asset files.
- **Devices:** the input opens only while transmitting, metering or testing the microphone; the
  output while transmitting, testing, or a sound plays, and closes after 3 s idle. cpal errors
  other than xruns, followed route changes and refused real-time priority rebuild the stream: the
  chosen device first, else the default one (`AudioEvent::DeviceChanged`), else
  `AudioEvent::DeviceError` and a retry every 2 s.
- **Mic test** loops your encoded packets back through a local playback stream, so you hear what
  others hear (gate included).
- **`examples/audio-check.rs`** checks devices by hand: `list`, `meter`, `mic-test`, `sounds`.

## 2026-11-01: Client core voice details the plan leaves open

Approved by the developer as a plan before implementation.

- **Dependencies:** `hermes-client-core` now depends on `quinn` and `hermes-audio` (both in
  section 6 / the workspace).
- **One session at a time** (`Core::voice_channel`, `Core::voice_status`). `join_voice` on the
  same instance sends `switch` on the open connection; another instance leaves first. It returns
  after the server's `ready` (10 s timeout) with the REST error (`VOICE_FULL`, …) or
  `CoreError::Voice` (connection, UDP).
- **Reconnect:** a lost connection (QUIC error or closed control stream after a `ready`) retries
  with a fresh `/voice/join` for the current channel up to 5 times, waiting 0.5, 1, 2, 4 and 8 s
  (`VoiceStatus::Reconnecting {attempt}`). A REST answer that is not a network error (no
  permission, full) stops the retries. Giving up and `kicked` end the session with
  `CoreEvent::Error` and `VoiceStatus::Disconnected`; `moved` is followed with `switch`.
  Short network drops (under the 15 s idle timeout) keep the connection and audio resumes.
- **Mute/deafen:** deafen also mutes (the server sees `self_mute = true`); undeafening restores
  your own mute. Changes go to the engine, the control stream (`set_state`) and
  `PATCH /voice/@me`. Server mute/deafen from `server_state` also closes the local gate.
- **Push-to-talk is forced** while the channel lacks `USE_VOICE_ACTIVITY`
  (`Core::voice_requires_ptt`).
- **Events:** `CoreEvent::VoiceStatus` (ping from `ping`/`pong` every 2 s, loss from QUIC path
  stats), `CoreEvent::Speaking {inst, user, speaking}` (your own from the gate) and
  `CoreEvent::InputLevel(dBFS)` while `set_level_meter(true)`. Device fallbacks and failures
  are `CoreEvent::Error` toasts.
- **Volumes:** `user_volumes.toml` (`[instance] "<user id>" = 1.5`); 1.0 is not stored. Local
  per-user mute (`set_user_muted`) is not saved.
- **Audio settings** come from `[voice]` (also on hot reload); `set_audio_settings` writes them
  back. `CoreOptions::audio` chooses real devices (default) or `Io::Virtual`.
- **`Core::start_call`** = `join_voice` + `POST /channels/{id}/call/ring`. `Core::play_sound`
  is for the UIs (ring, ping).
- **The QUIC client** binds `0.0.0.0:0` or `[::]:0` to match the resolved server address
  (Windows sockets are not dual-stack by default).
- **`Core::drop_voice_connection`** (hidden) breaks the connection for the reconnect test.
- **Proto:** the unused `KickReason::ServerShutdown` was removed; a shutdown closes the QUIC
  connection and clients reconnect.

## 2026-11-01: `hermes-voicebot` and voice integration test details

Approved by the developer as a plan before implementation.

- **Library + binary:** `hermes_voicebot::Bot` signs in (or registers with an invite) through
  `hermes-client-core` with `Io::Virtual`, joins a voice channel (by name in any space, or by
  ID, DMs included), plays 48 kHz mono samples and records what it hears. The binary is a thin
  CLI around it:
  `hermes-voicebot --server <addr> --user <u> --password <p> [--register [--invite <code>]]
  [--fingerprint <fp>] --join <channel> [--play in.wav | --tone] [--loop] [--record out.wav]
  [--duration <s>] [--denoise]`. The password can come from `HERMES_BOT_PASSWORD`.
- **No WAV crate:** a small reader (PCM 8/16/24/32-bit, 32-bit float, extensible format, any
  rate and channels, downmixed and resampled to 48 kHz) and a 16-bit PCM writer.
- **Defaults for bots:** UI sounds off (they would be recorded), RNNoise off unless `--denoise`
  (it treats synthetic tones as noise), self-signed certificates trusted on first use unless
  `--fingerprint` is given.
- **Measuring:** test signals are tone bursts; `signal::compare` finds burst onsets in what was
  sent and heard (20 ms frames above −34 dBFS RMS) and reports bursts heard, loss (audible
  frames heard vs sent) and per-burst latency on a shared clock.
- **Tests:** two bots in-process exchange 6 bursts (all heard, loss < 10 %, median latency
  < 250 ms; locally about 5–65 ms with 0 % loss), and the binary is run twice (one plays a
  44.1 kHz stereo WAV, the other records `out.wav`, which must contain all 4 bursts). The
  forwarding-rule and token tests from section 19 are the server's `tests/voice.rs`.

## 2026-11-01: GUI voice UI details

Approved by the developer as a plan before implementation.

- **PTT key lives in `[voice] ptt_key`** (written by Settings → Voice & Audio), not in a
  `[keys] push_to_talk` entry: section 17 had both places. `[keys] toggle_mute_global` (unbound by
  default) is the global mute key; it is registered with `global-hotkey` and is not a window
  keybind, so it never fires twice.
- **Push-to-talk polling** (Spike C decision) is in `hermes-gui` itself: `windows` is a
  `cfg(windows)` dependency of the GUI. Keys: `Mouse4`, `Mouse5`, `MouseMiddle`, `CapsLock`,
  `F1`–`F24`, letters, digits, `Space`, `Tab`, `Ctrl`/`Shift`/`Alt` (left/right variants too) and
  combos like `Ctrl+Space` (all held). On other systems PTT only works while the window is
  focused (iced key press/release); global PTT is not targeted there in v1.
- **Client-core additions:** `CoreEvent::CallRingStopped {inst, channel}` (from `call_ring_stop`;
  before it was dropped) and `Core::is_speaking(inst, user)` (the core keeps the speaking set, so
  the UIs do not each track it). `view::find_voice_channel` resolves `/join <name>` and the TUI's
  `:voice <name>`, preferring the space being viewed.
- **Joining a DM room** nobody else is in starts a call (`start_call`, which rings the others);
  otherwise it just joins. The pane header shows "Start call" or "Join voice" accordingly.
- **Incoming call banner** rings with `Sound::Ring` every 2 s for up to 30 s; it is not shown if
  you are already in that room. Join opens the DM and joins; Dismiss or `call_ring_stop` stops it.
- **Settings → Voice & Audio:** devices are listed in the background when the tab opens (and with
  "Refresh devices"); the level meter runs only while the tab is visible and the mic test stops
  when the tab is left. Sliders apply on release. The headphones hint is shown once per run on
  joining with noise suppression off (not persisted).
- **Per-user volume menu** (right-click a participant in the sidebar or the voice room): a
  0–200 % slider applied live and saved to `user_volumes.toml`, plus "Mute for me" (not saved).

## 2026-11-01: TUI voice details

Approved by the developer as a plan before implementation.

- **`J`** joins the room of the selected sidebar entry (voice channel, DM, or someone listed in a
  room), or of the open channel when the messages have the focus. `L` leaves, `m` toggles mute
  (unmuting while deafened undeafens), `D` toggles deafen. `q` asks for confirmation in voice.
- **`:voice [channel]`** (alias `:v`): without a name it answers the ringing call (opening the
  DM), else joins the open channel's room. `:leave`, `:mute`, `:deafen` and the slash commands
  `/join <voice channel>`, `/leave`, `/mute`, `/deafen` do the same as the keys.
- **Incoming calls** ring the terminal bell and say "<name> is calling: :voice to answer"; the
  status line says when the caller stops.
- **`Core::join_or_call`** (client-core) is the shared "join" of both UIs: an empty DM room starts
  a call, anything else joins.

## 2026-11-01: `install.sh` does the whole setup

Requested by the developer (ahead of the Phase 7 item), after the manual steps of 18.8 proved
error-prone (a binary copied with WinSCP lost its executable bit and the service crash-looped).

- **One script for everything:** besides the binary, unit, user and folders it now writes the
  config (`public_host`, `name`, `port`, `data_dir`, `bind = "::"` when the host has public IPv6),
  gets a Let's Encrypt certificate for a domain with certbot (reusing an existing one; `--nginx` or
  `--apache` when that server holds port 80, else `--standalone`; falls back to self-signed with a
  warning), installs the certbot deploy hook and the nightly backup (both embedded in the script,
  so `deploy/letsencrypt/hermes.sh` and `deploy/cron/hermes-backup` were removed), opens the port
  in ufw when ufw is active, runs `check-config` as `hermes`, (re)starts the server, waits for
  "Hermes is ready" and prints the owner invite link on the first start.
- **Questions** (domain or IP, server name, Let's Encrypt email) only on a first install in a
  terminal; `--domain`, `--name`, `--email`, `--port`, `--self-signed`, `--yes`, `--no-start`.
  An existing config is kept unless `--reconfigure`; an existing Let's Encrypt setup is refreshed
  on every run. Re-running repairs permissions (`chown -R hermes` on the data dir, binary 755).
- **Newest release by default, pre-releases included** (GitHub API), instead of
  `releases/latest`, which skips them. Missing tools are installed with apt when available.
- **Private repository:** the server cannot download from GitHub, so the script uses the release
  files in its own folder automatically. Every release now ships `install.sh` (release.yml), so
  "download the release, copy the folder, run `sh install.sh`" works.

## 2026-11-01: Release workflow (Phase 7, item 1) details

Approved by the developer as a plan before implementation.

- **Artifacts:** `hermes-server-{x86_64,aarch64}-unknown-linux-musl.tar.gz`,
  `hermes-server-x86_64-pc-windows-msvc.zip`, `Hermes-<version>-portable.zip` (`hermes.exe` +
  `hermes-tui.exe`), `hermes-tui-<version>-x86_64-pc-windows-msvc.zip`,
  `hermes-tui-<version>-x86_64-unknown-linux-gnu.tar.gz`, `install.sh`, `hermes.service`,
  `SHA256SUMS`, and the image `ghcr.io/senko-tools/hermes-server:<version>` (amd64 + arm64;
  `:latest` only for releases without a `-`). Server archive names stay unversioned (install.sh
  and the egg use them).
- **Left out:** the Windows installer comes with the installer item (Phase 7 item 2); macOS
  builds are skipped for now (10x runner minutes on a private repository; Windows first, Linux a
  bonus, as the developer asked).
- **aarch64** is cross-compiled with `cargo-zigbuild` (arm runners are not free for private
  repositories). Windows binaries link the C runtime statically. The Linux TUI is built on
  `ubuntu-22.04` (older glibc) and needs the system's `libasound2`.
- **Docker:** `deploy/docker/Dockerfile.release` copies the musl binaries built by the workflow
  (per `TARGETARCH`), so nothing is compiled under QEMU; `deploy/docker/Dockerfile` still builds
  from source for self-builders.
- **Dry run:** anything but a tag only uploads workflow artifacts (no release, no image push). It
  runs when `release.yml` or `Dockerfile.release` changes on `dev`, or by hand.

## 2026-11-01: Phase 6 in the GUI: space settings, channel management, menus

The developer asked for Phase 6 to v1.0 without questions, decisions logged here.

- **Space settings buffer** (`buffer/space_settings/`): tabs Overview, Roles, Channels &
  permissions, Members, Invites, Bans, Audit log. Bans and Audit log only show with
  `BAN_MEMBERS` / `VIEW_AUDIT_LOG`, Invites with `CREATE_INVITES` or `MANAGE_INVITES`. The
  buffer is saved in the layout as `{ buffer = "space_settings", instance, space }`. Every change
  goes through REST and comes back over the gateway; only the invite, ban and audit lists are
  loaded over REST (when their tab opens, with a Refresh button).
- **Roles editor:** name, color (12 swatches plus a `#RRGGBB` field instead of a full color
  picker widget), hoist, mentionable, permission checkboxes grouped General / Text / Voice with a
  description each. Checkboxes for bits I do not have are disabled (the server refuses them
  anyway). The descriptions live in `hermes_proto::permissions::CATALOG` so both UIs share them.
- **Drag and drop** (roles in the settings, channels in the sidebar and the Channels tab): iced
  buttons take the mouse press, so rows report the pointer entering/leaving and a window-wide
  listener reports the left button going down and up (`drag.rs`). Dropping a channel on another
  takes its place; on a category moves it in first. **Move up / Move down** buttons do the same
  without a mouse (keyboard users, accessibility). The order logic is in client-core
  (`view::move_channel`, `view::step_channel`) so the TUI can use it too.
- **Channel overrides:** per role, each channel-relevant permission has Deny / Inherit / Allow
  buttons; setting all to Inherit deletes the override. Text channels hide the voice bits.
- **Private channel** in the create dialog: an `@everyone` deny-`VIEW_CHANNEL` override plus an
  allow-`VIEW_CHANNEL` override for each ticked role. Text channel names are slugged
  (`My Room` → `my-room`), voice channel names are kept as typed.
- **One generic form dialog** (`modal/form.rs`) for create space / channel / category, edit
  channel (name, topic, slow mode, voice user limit and bitrate), ban (reason, delete messages
  from the last hour / day / week), nickname, invite (expiry, max uses) and profile.
- **Context menus everywhere** (muscle memory): right-click on a message (anywhere on it), on a
  message author's name or avatar, on members in the member list, on categories, on spaces
  (also a visible `⋮` button on space rows for people who do not right-click). Menus only offer
  what I may do, have separators, and check marks for the current notification level. New
  actions: create space/channel/category, space settings, mark all read, notification level,
  mute, open in a new pane, join voice, edit channel, permissions, delete channel, rename/leave
  group, reply, edit, quote, message / mention / copy username / nickname / roles / kick / ban,
  transfer ownership, delete space. Destructive ones ask first.
- **Command bar** additions: "Create a space", "Edit profile", "Space settings: X",
  "Create channel in X", "Invite people to X".

## 2026-11-01: Server endpoints for Phase 6 (search, profile, notification settings, admin)

- Implemented as PLAN.md section 10 says: `PATCH /users/@me`, `GET /users/{id}`,
  `PUT /users/@me/notification-settings/{target_id}`, `GET /search` (FTS5), `GET/PATCH
  /admin/users`.
- **Additions:** `DELETE /users/@me/notification-settings/{target_id}` with a new dispatch event
  `notification_settings_delete {target_id}`, because "back to the default" (a channel following
  its space again) cannot be expressed with `PUT`. `GET /users/@me/mentions?before&limit` for
  the Mentions buffer: the `message_mentions` table was meant for a "mentions inbox" (its schema
  comment), but section 10 has no endpoint to read it.
- **Search syntax:** every word must match as a prefix (`"word"*` in FTS5, quotes escaped), so no
  input causes an FTS syntax error; at most 16 words. Deleted messages never match.
- **Admin:** disabling a user deletes all their sessions (closing their gateways); admins cannot
  disable or demote themselves, so an instance never loses its last admin by accident.

## 2026-11-01: Search, mentions, pins and the new Settings tabs (GUI)

- **One results buffer** (`buffer/results.rs`) shows search results, mentions and pins; it opens
  next to the focused channel (a split) so the channel stays visible, and a click on a result
  opens its channel at that message, highlighted. Layout keys: `search` (instance, optional
  space), `mentions`, `pins` (instance, channel); the query is not saved.
- **Jump to message** (also for reply previews, which the plan says are clickable): the page
  around the message is loaded (`HistoryAnchor::Around`), the list scrolls to it, and scrolling
  down from there loads newer pages until the present.
- **Entry points:** `Ctrl+F` searches the focused channel's space (the whole instance from a DM);
  pane headers of channels have "Pins" and "Search"; space menus have "Search in this space";
  the user panel has "Mentions"; the command bar has "Search everywhere", "Search in X" and
  "Mentions". Mentions load the newest 50 per instance, merged newest first.
- **Settings tabs** now: Account (profile, avatar, password, signed-in devices with sign out),
  Appearance, Chat (`[buffer]`), Voice & Audio, Notifications (`[notifications]` and the window
  options of `[general]`), Keybinds (read-only list; editing stays in `config.toml`) and About
  (version, copy debug info, open the logs folder, project page). The Instances tab is not a
  separate tab: the sidebar's instance menu has everything it would (sign in/out, remove).
- **Muted channels** (muted, or level "nothing") show no unread dot and are drawn dimmed; mentions
  still count (`view::TreeChannel::muted`, shared with the TUI).

## 2026-11-01: Admin CLI (Phase 7)

- `hermes-server user list | reset-password <name> | set-admin <name> <true|false> |
  disable <name> | enable <name>` and `hermes-server version`. `list` and `enable` are additions
  to section 9.1 (otherwise a disabled account could only be re-enabled over the API).
- Reset passwords are 16 letters and digits without look-alikes (0/O, 1/l/I); reset and disable
  delete every session of the user.
- **Pick-up by a running server:** a task compares the database with the cache every 10 seconds
  (`users::refresh`): it copies `disabled` / `instance_admin` and closes gateway connections whose
  session no longer exists. REST requests check the session in the database on every request,
  so they stop at once.

## 2026-11-01: Windows installer, egg, self-hosting guide (Phase 7)

- **Installer:** cargo-packager 0.11.8 (a dev tool, installed in the release job) builds an NSIS
  installer from `crates/hermes-gui/Packager.toml`: per-user install (no admin prompt) to
  `%LOCALAPPDATA%\Programs\Hermes`, Start menu and desktop shortcuts, uninstaller, `hermes://`
  registered in `HKCU\Software\Classes\hermes` (`"…\hermes.exe" "%1"`), and `hermes-tui.exe`
  installed next to the GUI. The release names it `Hermes-Setup-<version>.exe`. Its version is
  written in `Packager.toml` (cargo-packager needs it there with `-c`); a test keeps it equal to
  the crate version. Not code-signed (SmartScreen: "More info → Run anyway", as section 21 says).
- **App icon:** the rounded "H" the tray already draws (`desktop::tray::icon_rgba`), rendered
  to `assets/icons/` by an ignored test so the installer, shortcuts and window share it.
- **Egg:** `deploy/pterodactyl/egg-hermes.json` as in section 18.5, plus a fixed
  `HERMES__STORAGE__DATA_DIR=./data` and `HERMES__LOG__FORMAT=pretty`, arm64 detection, and
  `hermes-server version` printed after install. It ships with each release. It still has to be
  tried on a real panel (the checklist item stays open until then).
- **`docs/self-hosting.md`** is written for non-experts (install script, Docker, panel, Caddy and
  nginx, invites, admin commands, backups, troubleshooting). Screenshots are not in it yet: they
  need the real apps on a desktop (the checklist item stays open until then).

## 2026-11-01: Phase 8 tooling: load test, fuzzing, update check, crash logs

- **`hermes-loadtest`** (in `hermes-voicebot`): N chat bots in one text channel at a steady rate,
  M voice bots playing tones in one voice channel, an observer measuring send → arrive latency;
  the first bot creates an invite for the others. Local run (debug server on the developer's
  Windows desktop, 50 chat bots at 6 messages/min + 20 voice bots): 300/300 messages delivered,
  p50 8 ms, p99 13 ms, max 19 ms; server working set ≈ 120 MB. The 1 vCPU VPS run of section 19
  is still to do.
- **Fuzzing:** `crates/hermes-proto/tests/fuzz.rs` throws 20 000 random and mutated inputs at the
  datagram parser, the gateway decoder and the markup parser on every test run (seeded, so
  failures reproduce). `fuzz/` is a separate cargo-fuzz crate (own workspace, `libfuzzer-sys`, a
  dependency only there) for long coverage-guided runs with nightly.
- **Update check** (`client-core::update`): GitHub releases API, newest non-draft release newer
  than the running version (pre-releases only for pre-release users); the GUI shows "Hermes X is
  available" above the user panel (click opens the release page). Off with
  `general.check_for_updates = false`. While the repository is private the API answers 404 and
  nothing is shown.
- **Crash logs** (`client-core::crash`): a panic writes `logs/crash-<unix ms>.log` (version, OS,
  message, location, backtrace) in the config folder; GUI and TUI. Settings → About has "Copy
  debug info" and "Open the logs folder".

## 2026-11-01: Accessibility basics (Phase 8)

- High-contrast theme and the UI scale setting already existed; keyboard navigation was missing
  in menus and dialogs. Now: **Tab / Shift+Tab** move between the fields of dialogs and settings
  (iced only focuses text fields), context menus take **Up/Down + Enter/Space** (Escape closes),
  and **Shift+F10 or the Menu key** opens the focused channel's menu, as on Windows. Together
  with the existing keybinds (Ctrl+K command bar, Alt+Up/Down channels, Ctrl+Tab panes, Ctrl+F
  search) everything in the main flow works without a mouse.
- The security review result is recorded in `docs/security-review.md`.

## 2026-11-01: TUI: action menus, mouse, search and space commands

- **Action menu** (the TUI's right-click): `a` (configurable as `[tui.keys] menu`), `:menu` or a
  right-click opens a list of what can be done with the selection: a sidebar channel (open,
  join voice, mark read, pins, notification level, mute, rename, topic, delete), a space (mark
  read, invite, search, create channels, notifications), an instance (create a space, profile,
  mentions, offline/online, log out, remove), a message (reply, react, edit, pin, open the
  attachment, message/mention the author, delete) or a person (message, mention, roles, kick,
  ban). Entries that need text open the `:` line pre-filled (`:ban bob `). Only what I may do is
  offered.
- **Mouse:** a left click on a sidebar row opens it, on a member selects them, on the messages
  focuses them; a right-click does the same and opens the menu; the wheel scrolls the list under
  the pointer.
- **Search** (`/` or `:search <words>`, `:search!` for the whole instance), `:mentions` and
  `:pins` show a results popup; Enter opens the channel at that message (loading the page
  around it; scrolling past the newest loaded message loads newer pages).
- **Commands** for Phase 6 in the TUI: `:notify all|mentions|none|default|mute|unmute [space]`,
  `:create text|voice|category <name>`, `:space <name>`, `:rename`, `:topic`,
  `:delete-channel`, `:kick`, `:ban <user> [reason]`, `:unban`, `:role <user> +role|-role`,
  `:profile name|status <text>`, `:read`. The role editor and channel overrides stay GUI-only
  (section 16.3: the TUI's settings UI is minimal).

## 2026-11-01: GUI look: own palette, Lucide icons, roomier layout

- **Themes:** the built-in `dark` and `light` themes no longer use Discord's colors. Dark is a
  warm charcoal, light a warm paper, and both use a gold accent (`buttons.primary`, which also
  colors focus rings, selected tabs and the active header buttons). `high-contrast` is unchanged.
  No theme keys were added; new looks derive from the existing keys.
- **Icons:** PLAN.md section 15.1 asks for an icon font for UI chrome. The GUI bundles a subset
  of Lucide (`assets/fonts/Lucide.ttf`, ISC license in `Lucide-ISC.txt`, from lucide-static
  0.468.0), only the glyphs listed in `widget/icon.rs`. Text buttons such as "Pins", "Search",
  "Members", "Mute" and "React" became icon buttons with tooltips.
- **Spacing:** shared measurements in `theme/style.rs` (gaps, radii, 32 px controls, 48 px
  headers, row padding) so rows, buttons and inputs line up; text inputs, drop-downs and
  scrollbars use themed styles instead of iced's defaults.
- **Layout changes:** avatars 40 px with more space between authors; the composer is one
  rounded field with the attach button inside; day dividers say Today / Yesterday / weekday;
  Settings and Space settings use a side navigation instead of a row of tabs (same tabs);
  member list and user panel show avatars with presence dots; the empty pane is titled "Home".
- **Why:** the developer found the GUI cramped, unaligned and too plain, and asked for its own
  style (like Halloy) that still feels familiar to people coming from Discord.

## 2026-11-01: GUI: Nerd Font, Material icons, full-window settings, calm splits

- **Font:** the default `font.family` and `font.mono_family` are now `JetBrainsMono Nerd Font`
  (bundled: Regular, Italic, Medium, Bold; SIL OFL in `JetBrainsMonoNerdFont-OFL.txt`) instead
  of Inter / JetBrains Mono (PLAN.md section 15.5). Inter and JetBrains Mono stay bundled.
  Settings → Appearance has a font and a code-font picker: the bundled families or "Custom…" for
  any installed font. Existing `config.toml` files keep the family they already name.
- **Icons:** the Lucide subset from the previous entry is gone; icons are the Material Design
  glyphs inside the bundled Nerd Font (`widget/icon.rs`), so there is one font less. No emojis
  in UI chrome.
- **Theme keys:** new `[presence] online / idle / dnd / offline` (status dots everywhere,
  including the status menu). Themes that leave them out get the `dark` values with a warning,
  like every other key.
- **Settings and Space settings** open over the whole window instead of in a pane (PLAN.md 15.1
  lists them as pane buffers): navigation on the left, a round close button (Esc also closes).
  Layouts saved with a settings pane still restore and show it in the pane.
- **Split panes ("calm splits"):** 1 px dividers instead of boxed panes with gaps; the focused
  pane is marked by a gold line under its header; unfocused panes show their header buttons only
  on hover; only the focused pane shows its member list.
- **Smaller changes:** right-click menus have a header naming their target, an icon per entry
  and the check on the right, and the right-clicked row, message or person stays outlined while
  its menu is open; the reply/edit bar and upload chips sit inside the composer; settings
  checkboxes are switches; file attachments are cards with a download button; icon buttons are
  34 px; grouped messages always lay out their timestamp (shown on hover) so rows never move.
- **Why:** the developer's review of the previous round (too small, text-heavy, cluttered
  settings and splits, unaligned elements) and their choices: full-window settings, calm
  splits, Material icons via the Nerd Font, JetBrainsMono Nerd Font as the default.

## 2026-11-01: GUI: one type scale and spacing grid, richer theme format

- **Measurements:** every text uses one of six sizes derived from `font.size` (caption −2,
  label, strong, title +2, heading +5, display +11); gaps and paddings sit on a 4 px grid
  (`GAP_XS` 4 … `GAP_XL` 24); text buttons, inputs, drop-downs and rows are 32 px tall and icon
  buttons 32 px square (was 34).
- **Theme format (PLAN.md 15.5):** new sections `[surface]` (deep, main, sidebar, raised,
  overlay, hover), `[accent]` (base, highlight, muted, glow, focus), `[icons]` (default, active),
  `[channel]` (border / background / hover for the selected, unread, mention and connected row
  states) and `[shape]` (radius, radius_large, indicator_width). The older keys stay. A theme
  file that leaves out a key of these sections gets it derived from its own older keys (not from
  `dark`, and without a warning), so themes written before look the same.
- **Why:** the developer found the UI messy (many text sizes, buttons of different heights) and
  wants richer custom themes, e.g. a port of their Vencord theme, which the old keys could not
  express (surface levels, accent-coloured icons, square corners, row state colours).

## 2026-11-01: GUI: settings on one grid, resizable panels, steady UI scale

- **Settings layout:** every settings row (user and space settings) is a name and hint on the
  left and the control in a fixed 280 px column on the right; rows are grouped under small
  titles; every on/off setting is its own row with the switch at the right edge (role
  permissions too, instead of checkboxes). Kick, Ban and Transfer moved to a "Danger zone"
  group; the space settings tab "Channels & permissions" is now "Channels".
- **UI scale** applies when the slider is released (it changed the window scale while dragging,
  so the slider jumped), with presets (90–150 %) and new keybinds `zoom_in` (Ctrl+=),
  `zoom_out` (Ctrl+-) and `zoom_reset` (Ctrl+0).
- **Resizable panels:** the sidebar and the member list are resized by dragging their inner
  edge (double-click resets); new config key `sidebar.member_list_width` (default 232). The
  "Sidebar width" slider became a Reset button.
- **Why:** the developer found settings messy (switches not in one column, controls of
  different widths) and the UI scale slider unusable, and asked for a draggable sidebar.

## 2026-11-01: GUI: settings header, footer colour, row radius, feather icon

- **Theme keys:** `surface.footer` (the voice and user panels; was `surface.deep`, too dark),
  `shape.row_radius` (rows in lists; `shape.radius` is now for things that stand alone, so
  the neko theme rounds buttons, inputs and menus a little while its rows stay square).
  `surface.deep` now dims the window behind dialogs. Old files derive both new keys.
- **Settings pages** have a header like the main window instead of the round close button:
  the brand over the navigation and "Return to <what the focused pane shows>" (Esc) over the
  content.
- **App icon:** a gold feather (the header's `md-feather`) on a charcoal square instead of the
  blue "H", for the window, tray and installer.
- **Why:** the developer's review of the previous round.

## 2026-11-01: GUI: notifications as toasts, no message toolbar, member list in every pane

- **Notifications** ("Created #x", errors) are a card at the top right instead of a bar above
  the panes; info ones close on their own after 5 s.
- **Message hover toolbar removed:** right-click has every action; "Add reaction" moved into
  that menu (it opens the quick reactions on the message).
- **Member list:** every pane shows its own when toggled on (the previous round showed it only
  in the focused pane, which made it disappear when focus moved).
- **Sidebar:** "New message" is a row under Direct messages instead of a small `+`.
- **Sounds:** join, leave, mute and unmute are lower (G4/C5) and softer.
- **Animations:** menus, dialogs, the settings page and notifications animate in (iced
  `Animation`, 140–220 ms).
- **Why:** the developer's review of the previous round.

## 2026-11-02: GUI fixes after 1.0.0

- **No animations:** the open animations of menus, dialogs, the settings page and
  notifications (previous entry) are removed; everything appears at once.
- **Voice channels open on their voice room** (a click in the sidebar, the voice panel, a
  `<#channel>` link); the chat is the "Chat" button in the pane header. Sidebar rows keep the
  same widget tree in every state, so the first click of a double-click is no longer lost.
- **Footer colour:** the user and voice panels default to the sidebar colour
  (`surface.footer` stays a theme key).
- **Message scrollbar** touches the pane's right edge; "Jump to present" sits clear of it.
- **Sidebar tabs (PLAN.md 15.1 deviation):** "Spaces" and "Messages" tabs under the header
  instead of a "Direct messages" section after the spaces, which was easy to mistake for one
  more category. Each tab lists every instance; the other tab shows a dot or the mention count
  when something there is unread, and opening a channel (also from a notification or a call)
  switches to its tab. The `<instance>/dms` collapse key is gone.
- **Why:** the developer's review of 1.0.0.

## 2026-11-02: Closing 1:1 DMs

- **What:** `DELETE /dms/{id}` (an addition to PLAN.md section 10) closes a 1:1 DM for the
  caller; `dm_recipients.closed` (migration 0004) remembers it. A closed DM is left out of
  `GET /dms` and `ready`; a new message or a call ring reopens it for everyone who closed it
  (`dm_create` before the event), and so does `POST /dms` to the same person for the caller.
  Group DMs are still left, not closed. The GUI has "Close conversation" in a DM's menu and an
  X on hover; the TUI has `:close`.
- **Why:** there was no way to remove a 1:1 DM from the list.

## 2026-11-02: Privacy settings and message requests

- **What (additions to PLAN.md sections 7.3, 10 and 11):** each user chooses who may start DMs
  with them: `dm_from` = everyone (default) / people in my spaces / nobody, and whether everyone
  else gets through as a **message request** (`message_requests`, default on). Stored in
  `users.dm_from` / `users.message_requests` and `dm_recipients.request` (migration 0005);
  `GET`/`PATCH /users/@me/privacy`, `ready.privacy`, `privacy_update`, `POST /dms/{id}/accept`,
  and `Channel.request` (set only in the recipient's copy).
- **Rules:** the server's `dm_policy` still applies first. Existing 1:1 DMs are always reused, so
  changing the setting never cuts off a conversation. A request notifies once (when it arrives);
  its messages never notify or count as unread; calls do not ring it; no presence is shared
  either way until it is accepted. Accepting (button, menu, `:accept`) or answering with a
  message turns it into a normal DM. Ignoring closes it for good: its events stop and new
  messages do not reopen it (the sender is not told). Ignoring a group request leaves the group.
- **Clients:** GUI: Settings → Account → Privacy; requests sit under "Requests" in the Messages
  tab with accept and ignore buttons. TUI: a Requests heading, `:accept`, `:close`,
  `:privacy dms everyone|spaces|nobody`, `:privacy requests on|off`.
- **Why:** the developer asked for per-user DM privacy with message requests that only notify
  for the request itself.

## 2026-11-02: Security pass after 1.0.0

- **Invite fingerprints (PLAN.md section 13 deviation):** an invite's `#fp=` is trusted without
  asking only for a host this client has never trusted. A pin replaces CA validation, so a link
  could otherwise move the trust of a known server to an attacker's certificate.
- **Role hierarchy for voice moderation and channel overrides (PLAN.md section 10 deviation):**
  `PATCH /spaces/{id}/voice-states/{uid}` acts only on members below the caller's highest role
  (your own state is always yours); channel overrides are set and removed only for `@everyone`,
  roles below the caller's highest role and the caller's own roles.
- **Names:** filenames and every single-line name refuse text direction overrides
  (U+202A–202E, U+2066–2069).
- **Clients:** links and folders open through `ShellExecuteW` on Windows (never a program by
  name); the TUI opens only viewer file types; downloads get the Mark of the Web; a `hermes:`
  command line ignores every other argument (`--config-dir` needs an `https://` invite link).
- The full list is in `docs/security-review.md`.
- **Why:** the developer asked for a security pass focused on code execution and on users acting
  on other people's things.

## 2026-11-02: Screen sharing and the smart voice room grid

- **What (moved forward from the PLAN.md section 22 roadmap):** screen sharing in voice
  channels and calls. Capture with `xcap` (Windows only for now: on Linux it would need
  PipeWire, XCB and libclang on the build machines, and the GUI ships for Windows only), H.264
  with `openh264` built from its bundled source (OpenH264 2.7.0); both crates were approved by
  the developer (PLAN.md section 6 addition). H.264 rather than VP8/AV1 (section 22): it is the
  one codec with a self-contained, real-time encoder and decoder that builds everywhere with
  only a C++ compiler.
- **Protocol:** control ops `share_start`, `share_stop`, `watch`, `request_keyframe`,
  `share_started`, `share_stopped`, `keyframe_requested`, `ready.sharing`; one QUIC
  unidirectional stream per frame with a 12-byte header (docs/protocol.md "Screen shares").
  Shares are known only inside the room, so only people in the call see them.
- **Permission:** new bit `STREAM` (1 << 38, "Share screen"), in `@everyone`'s and DM
  recipients' defaults; migration 0006 adds it to existing `@everyone` roles. It is removed
  with `CONNECT` in voice channels, like `SPEAK`.
- **Limits:** frames ≤ 512 KiB and ≤ 30 per second; a watcher's queue holds 8 frames; the GUI
  sends at most 1280×720 at 10 fps (1.5 Mbit/s), a keyframe at least every 5 s.
- **GUI:** clicking a voice channel opens its voice room, a grid that picks the column count
  making the 16:9 tiles largest for the pane. Everyone is one tile; a share fills the sharer's
  tile with their name on a badge, and right-click is always that person's menu. Shares are
  received only while the voice room shows in a pane. "Share screen" / "Stop sharing" in the
  voice room's header; which screen in Settings → Voice & Audio (`voice.share_screen`). The TUI
  ignores shares.
- **Why:** the developer asked for screen sharing now, with one smart grid of people and
  shares.

## 2026-11-02: Screen share quality settings, enlarged shares, selecting messages

- **What:** `voice.share_resolution` (`720p` default, `1080p`, `1440p`, `native` = up to the
  protocol's 3840 px) and `voice.share_fps` (10 default; 5, 10, 15 or 30 offered, at most the
  protocol's 30) in Settings → Voice & Audio, read when a share starts. The bitrate follows the
  size and rate (about 0.1 bit per pixel per frame, 1.5 to 8 Mbit/s), which keeps keyframes well
  under the 512 KiB frame limit. This replaces the fixed 1280×720 at 10 fps of the screen sharing
  entry above.
- **Enlarged shares:** clicking a share in the voice room shows it alone in the grid's area;
  clicking it again or Esc goes back to everyone.
- **Selecting messages:** "Select" in a message's menu or Ctrl+click starts selecting, Shift+click
  adds a range, and while selecting, a click toggles. Only messages I may delete can be
  selected. A bar replaces the composer with Delete (confirmed once, one `DELETE` per message:
  no bulk endpoint) and Cancel/Esc. The jumped-to message and the one whose menu is open lost
  their accent border and keep a plain tint.
- **Why:** the developer asked for these.

## 2026-11-02: Faster screen sharing, Direct3D 12 by default, the share menu

- **Renderer:** on Windows the GUI draws with Direct3D 12 unless `WGPU_BACKEND` is set. wgpu
  otherwise picks Vulkan first, and an older Intel driver's Vulkan hung the window ("not
  responding") on the developer's laptop. Without a Direct3D 12 GPU, iced falls back to its
  software renderer.
- **Capture:** DXGI Desktop Duplication (`screen/dxgi.rs`, through the `windows` crate with the
  Direct3D 11/DXGI features xcap already enables: no new dependency) on the GPU the screen is
  connected to; GDI (xcap) only if duplication is unavailable. Not xcap's own duplication
  recorder: its thread never stops once started, hands over frames up to a frame late and only
  looks at the default GPU. Measured on the developer's PC (1440p screen, release build):
  12 ms per picture instead of 43 ms.
- **Pipeline:** capturing + scaling and encoding + sending run on two threads; a picture the
  encoder has no time for is dropped. An unchanged screen sends nothing but a repeated picture
  when a keyframe is due. Scaling is our own area average on up to four threads
  (`screen/scale.rs`: 9–11 ms for 1440p → 1080p, against 46 ms for `image::imageops::thumbnail`);
  RGB → YUV uses OpenH264's SIMD conversion (2 ms instead of the per-pixel path). A line with
  the share's numbers is logged every 10 s.
- **NASM:** OpenH264's build quietly drops its assembly when NASM is not on `PATH` (every build
  so far, 1.1.0 included): encoding a 1080p picture took 66 ms instead of 18 ms. The release
  workflow installs NASM and fails if OpenH264 was built without it. Local builds need NASM on
  `PATH` too.
- **Watching:** shares are drawn with iced's `shader` widget (`screen/view.rs`): one set of Y/U/V
  textures per share, overwritten in place, converted to colors on the GPU. As images, every
  frame was a new GPU texture (and, above 2 MiB, a separate upload on iced's worker), which is
  the prime suspect for the laptop running out of GPU memory, and every pixel was converted on
  the CPU. iced's software renderer does not draw shares.
- **Share menu:** the voice panel's audio settings button became Share screen; it opens a menu
  with the screen, resolution and frame rate (saved as soon as they are picked, so the menu
  offers the last choice) and Start sharing, as does Share screen in the voice room's header.
  While sharing, the button stops the share. The options stay in Settings → Voice & Audio too.
- **Why:** the developer reported the laptop hanging and running out of memory, and shares
  that looked the same speed whatever the settings.

## 2026-11-05: Measuring screen shares, testing on a local network

- **What:** every hop of a screen share logs its numbers every 10 s: the sharer (capture, encode,
  keyframes, frames refused by a full connection queue), both transport directions in
  `hermes-client-core` (rate, bitrate, time on the wire, failures), the server (frames in, over
  the rate limit, queued for and lost to watchers, keyframe requests), the watcher's decoder
  (decode and waiting time, pictures the window had no time for, arrival jitter, delay drift) and
  the drawing (new pictures reaching the GPU). `share_transport_benchmark` (ignored test in
  `hermes-client-core`) pushes share-sized frames through a real local server without any codec.
  `docs/testing-locally.md` describes a server and two clients on a local network and how to read
  the numbers. OpenH264 is optimized in dev builds too, like the audio crates.
- **First results (developer's PC, 1440p screen shared at 1080p, release build):** capture 12 ms
  (reading the desktop back from the GPU), scaling 12 ms, YUV 2 ms, software encode 18 ms per
  delta frame (32 ms per keyframe), software decode 12 ms: about 44 ms of CPU per frame, so the
  current design cannot reach 60 fps, let alone 4K. The transport on one machine delivers frames in
  1–2 ms even at 25 Mbit/s, but the server's fixed one-second rate window drops about 2 % of frames
  at exactly 30 fps (timer jitter puts 31 frames in a window), and each drop breaks the picture
  until the next keyframe. Both are fixed in the following steps.
- **Why:** the developer reported laggy shares that cost about 11 % CPU whether watched or not,
  and asked for a way to tell server problems from client ones.

## 2026-11-05: Watching screen shares on demand

- **What:** a screen share is received only by people who chose to watch it. A sharer's tile
  shows "Sharing their screen" and a **Watch** button (a click anywhere on the tile does the same);
  a watched share has a **Stop watching** button in its top-right corner (and on its tile while
  it loads). The choice is forgotten when the share or the call ends; it is still only received
  while the voice room shows in a pane and the window is open. This replaces "shares are received
  while the voice room shows" of the 2026-11-02 entry.
- **Protocol (addition to docs/protocol.md):** the server tells a sharer how many watch them with
  the new control op `share_watchers {count}`, after `share_start` and whenever it changes
  (watching, stopping, a watcher leaving). While it is 0 the GUI stops capturing and encoding
  (Desktop Duplication stays open so a new watcher gets a picture at once); its own tile says how
  many watch. Older clients skip the unknown op; a newer client with an older server never gets
  it and keeps sending as before.
- **Why:** the developer asked for starting and stopping watching; a share nobody watches cost
  the sharer about 11 % CPU for nothing.

## 2026-11-05: Screen share transport for 60 fps

- **Limits:** frames up to 2 MiB (a 4K keyframe) and 60 per second. The server's rate limit is a
  token bucket (60 per second, 10 frames of burst) instead of a fixed one-second window, which
  dropped about 2 % of the frames of a share at exactly the limit (timer jitter put one frame too
  many in a window). 32 streams (frames) may be open per connection, a watcher's queue holds 16
  frames, the sharer's own 8; both ends allow 4 MiB in flight per stream and 16 MiB per
  connection (the defaults could not hold a large keyframe).
- **No keyframe storms:** after a lost frame (a full queue, a frame over the rate, or a new
  watcher) the server sends that watcher nothing from the sharer until their next keyframe, which
  it asks for (at most every 500 ms). The sharer, when its own connection refuses a frame, sends
  nothing until a keyframe, made at most every 500 ms; the watcher's decoder asks for a keyframe
  at most every 500 ms instead of for every frame it cannot decode. Before, every loss produced
  another keyframe, the largest frames, on a link that was already behind.
- **Measured (transport benchmark, one machine):** 600 of 600 frames at 60 fps and 25 or
  40 Mbit/s, median delay 1 ms (before: 295 of 300 at 30 fps; 60 fps impossible).
- **Not done (planned):** quinn's BBR congestion controller. Cubic kept up in the benchmark, and
  quinn marks BBR as experimental; worth trying only if the LAN or internet numbers show the
  sender's rate collapsing after losses.
- **Why:** the developer asked for at least 4K at 60 fps and for smoothness first.

## 2026-11-05: Hardware screen share encoding

- **What:** on Windows a share is captured, scaled, converted and encoded on the GPU: Desktop
  Duplication into a texture of our own, the Direct3D 11 video processor scales it and converts
  it to NV12 (limited-range BT.601, like OpenH264) in one pass (`screen/gpu_convert.rs`), and
  the GPU's own H.264 encoder (NVENC, AMF or Quick Sync) encodes it through Media Foundation
  (`screen/mf_encode.rs`): low-latency mode, constant bitrate, High profile, no B-frames,
  keyframes only when asked for. The picture never comes back to the CPU. Everything goes through
  the `windows` crate (features `Win32_Media_MediaFoundation`, `Win32_System_Com`,
  `Win32_System_Ole`, `Win32_System_Variant`): no new dependency, Media Foundation is part of
  Windows. The output is the same Annex B H.264 as before, so watchers (and older clients, with
  OpenH264) decode it unchanged.
- **Fallback:** without a GPU with video support or a hardware encoder on the screen's GPU, or
  when it fails, the share continues on the CPU path (readback, our scaler, OpenH264).
- **Bitrate:** about 0.07 bit per pixel per frame, 2 to 40 Mbit/s (1440p at 60 fps: 15 Mbit/s;
  4K at 60 fps: 35 Mbit/s); the rate control buffer is half a second but at most 12 Mbit, so a
  keyframe stays well under the 2 MiB frame limit.
- **Measured (developer's PC, NVIDIA encoder, 1440p screen):** convert + encode per frame,
  median: 2.8 ms at 1080p, 5.3 ms at 1440p, 7.3 ms at 4K (CPU path: about 44 ms at 1080p).
  `real_screen_hardware_is_captured_and_encoded` and `share_end_to_end` (ignored tests) measure
  it on any machine.
- **Why:** the developer asked for at least 4K at 60 fps and for CPU and GPU use like Discord's;
  the software encoder cannot do either.

## 2026-11-05: Hardware screen share decoding, fewer window updates

- **What:** on Windows, watchers decode shares with Windows' own H.264 decoder through Media
  Foundation (`screen/mf_decode.rs`), on the GPU (DXVA) given a Direct3D 11 device, else on the
  CPU; OpenH264 stays the fallback (and the decoder everywhere else). It is kept up to date by
  Windows Update, which matters for a decoder that parses other people's data. Its NV12 pictures
  are drawn as they are: a Y and an interleaved U/V texture, converted in the same shader
  (`screen/view.rs`). A decoded picture is read back through a staging texture of our own;
  Media Foundation's own way stalled about 8 ms per picture.
- **Measured (developer's PC, NVIDIA GPU), per picture:** 2.9 ms at 1080p, 4.7 ms at 1440p,
  10.4 ms at 4K (OpenH264: 4.6, 8.1 and 18.5 ms).
- **Window updates:** decoded pictures no longer each become a message. The decoder keeps the
  newest picture of each share; the window takes them all at once when it is ready
  (`Message::ScreenImages`), so a busy window shows the newest picture instead of working
  through a queue, and these updates skip the voice and level meter bookkeeping that every other
  message runs.
- **Not done (planned as optional):** sharing the decoded texture with the renderer directly
  (Direct3D 11 to wgpu's Direct3D 12) would save the readback and upload; the numbers above do
  not need it yet.
- **Why:** the developer asked for 4K at 60 fps; OpenH264 needs 18 ms or more for a 4K picture.

## 2026-11-05: Smoothness, Mixed or Clarity; 60 fps and 2160p

- **What:** `voice.share_preference` (`smoothness` default, `mixed`, `clarity`) says what a share
  keeps when the connection or the computer falls behind: Smoothness keeps the frame rate and
  lowers the bitrate, then the size (to half); Clarity keeps sharp, full-size pictures and lowers
  the frame rate (to a quarter), and spends twice the bits on every picture; Mixed is in between.
  Falling behind (frames my connection refuses, the server asking for keyframes again and again
  because watchers lose frames, or encoding slower than the frame rate) steps down at most every
  2 s; 8 calm seconds step back up. Chosen as a three-way switch in the share menu, on my own tile
  in the voice room while I share (a running share follows at once) and in Settings → Voice &
  Audio. The CPU fallback path ignores it.
- **Options:** resolution adds `2160p`; frame rates are 15, 30 and 60 (5 and 10 removed). New
  defaults: 1080p at 60 fps (was 720p at 10).
- **Why:** the developer asked for smoothness first, with a quick setting for clarity.

## 2026-11-05: Log files by date; crash reports named by local time

- **What:** the client logs to `logs/latest.log` (TUI: `logs/latest-tui.log`). At the next start,
  or when the local day changes while it runs, that file moves to
  `logs/logs_old/<app>-YYYY-MM-DD.log` (`-2`, `-3`, … for more runs on one day; empty logs are
  dropped). A `hermes.log` from older versions is moved there too. Crash reports are named
  `crash-YYYY-MM-DD-HHMMSS.log` (local time) instead of `crash-<unix ms>.log`
  (`client-core::logs`).
- **Retention:** rotated logs are deleted after 7 days (PLAN.md section 13); crash and freeze
  reports in `logs/` after 30 days, which the plan does not cover.
- **Why:** the developer asked for logs separated by date with a `latest.log`, old ones in a
  `logs_old` folder that gets cleaned up; one ever-growing `hermes.log` mixed every run together.

## 2026-11-05: Freeze watchdog with minidumps

- **What:** a `hermes-watchdog` thread in the GUI (`hermes-gui/src/watchdog.rs`). The UI thread
  beats on every message, and a 2 s timer makes sure messages keep coming while idle. When no
  beat comes for 10 s, the watchdog writes `logs/freeze-YYYY-MM-DD-HHMMSS.log` (version, system,
  how long, whether the UI thread was in `update`, in `view` or elsewhere) and, on Windows, a
  minidump of every thread's stack (`MiniDumpNormal | MiniDumpWithThreadInfo`) next to it. At
  most 3 reports per run; when the UI responds again that goes to `latest.log`. A jump in the
  watchdog's own clock (the computer slept) starts the count over.
- **Dependency:** no new crate; the `windows` crate (PLAN.md section 6) gets the
  `Win32_System_Diagnostics_Debug`, `Win32_System_Threading`, `Win32_Storage_FileSystem`,
  `Win32_System_Kernel` and `Win32_System_Memory` features (the last three only because
  `MiniDumpWriteDump`'s binding requires them). Approved by the developer.
- **Why:** a friend's client froze while voice kept working, and left no crash log and nothing
  in its log: a hang is not a panic. The cause found (a store read lock taken twice on the UI
  thread) is fixed, and debug builds now panic on it (`Core::store`); the watchdog makes the next
  unknown hang show where it is stuck.

## 2026-11-07: The desktop client moves from iced to gpui

- **What:** the developer decided to replace iced with gpui, which changes PLAN.md sections 6 and 15.
  The new client grows in `crates/hermes-gpui` (binary `hermes-gpui`); `crates/hermes-gui` stays
  untouched until the new client replaces it, then it and `spikes/iced-ui` are removed. So far the new
  crate is a layout shell on sample data.
- **Dependency:** `gpui-kit =0.9.0` (Apache-2.0), which pins GPUI as `gpui-pre =0.3.11` and adds the
  widgets gpui lacks (text areas, menus, dialogs, resizable panels). Not the `gpui` crate: its last
  release is 0.2.2 from October 2025. Approved by the developer. CI's Linux job gets the system
  libraries gpui needs.
- **Layout changes** (replacing section 15.1 in the new client): no native title bar, the sidebar
  reaches the top of the window and a 32 px bar over the main panel holds the window buttons and drag
  area. Friends is the sidebar's top item and its page holds message requests, DMs, group DMs and
  people, replacing the Spaces | Messages tabs. Instances are hidden from the sidebar unless none is
  saved, and are added from the menu of the sidebar's empty space or from settings. A real friend
  system (requests stored on the server) was chosen and is still to be built.
- **Why:** the developer expects better performance and a safer long-term bet, with Discord's CTO
  announcing a GPUI desktop client in November 2026.
- **Known gaps:** on Windows, gpui's surface drawing is an empty stub, so screen-share video has no
  built-in path other than per-frame images (see the 2026-11 screen share entries for why those were
  dropped), and probably needs a patched renderer. gpui's Windows renderer has no software fallback at
  runtime, while iced's has. GPUI is pre-1.0 with no stable API, hence the exact pin. The performance
  gain has not been measured.

## 2026-11-07: gpui client layout, second pass after the developer tried it

Replaces the layout part of the entry above; `docs/gpui-migration.md` has the current summary.

- **What:** the native Windows title bar returns, titled with the current page, and the in-app bar is
  gone. A Discord-like icon rail replaces the spaces dropdown: its top icon is the app icon and opens
  Friends, with DMs in the sidebar and requests and people in the main panel. Sizes follow Discord's.
  The font, Material icons, sounds and app icon come from the iced client.
- **Mouse up:** every click acts on release. The kit's `context_menu` opens on press, so `menu.rs` has
  our own. Drags and placing the text cursor still start on press, and the kit's menus close on press.
- **Dependency:** `embed-resource =3.0.12` as a build dependency, because gpui reads the window icon
  from the executable's resources. Approved by the developer. `hermes-gpui` also uses `hermes-audio`.
- **Why:** the shell looked small and compact, the dropdown looked off, mouse down felt too quick, and
  the top sidebar item sat too close to the window's edge without a title bar.

## 2026-11-07: gpui client: a header again, and how share video and the tray work

Developer's decisions while the client gets everything `hermes-gui` has. `hermes-gpui` replaces
`hermes-gui` once it works, taking its crate name. Nothing else about the iced client matters until then.

- **Header:** a bar over the main panel returns, without a title: it drags the window and holds the
  page's buttons and fields and the window buttons, so the native title bar is gone again. The title
  stays set for the taskbar. The window's minimum size is 720x480: the rail, a narrow sidebar and a
  usable chat fit, and smaller windows only get worse.
- **Share video:** decoded pictures become BGRA images on the decoder thread (scaled down by whole
  steps when the tile is small) and the shell frees each image it replaces with `Window::drop_image`.
  gpui then makes a new atlas texture for most frames, which costs little. Measured in release between
  two clients on one PC, sharing a mostly still screen at the 1080p setting and 60 fps: a steady 60 fps with no
  pictures dropped, 3.4 ms to decode and 7.8 ms to convert when enlarged in a maximized window (1 ms in a small
  tile), about one CPU core, and GPU memory flat. A busy screen and slower PCs are not measured. A patched
  renderer is not needed so far.
- **Tray:** gpui cannot hide a window, so `desktop/window.rs` calls `ShowWindowAsync` with the window's
  own handle. Dependency `raw-window-handle =0.6.2` for that (already in the lockfile through gpui),
  approved by the developer.
- **Voice:** a join in progress blocks further joins (the old Join voice spam). `Core::voice_link`
  exposes the QUIC connection's numbers for a network panel with live graphs, opened from the voice
  panel's connection line.
- **Friend system and split panes:** after everything else. The Friends page lists message requests
  and people from shared spaces until then. Pane keybinds stay valid config but do nothing.
- **Sizes and fonts:** `font.size` draws 3 px larger than its value, so the iced default of 13 means the 16 px
  the developer asked for. The UI scale goes through `theme::px` and the window's rem size, since gpui has no scale
  of its own.
- **Choices as chips:** settings with a few choices are rows of buttons instead of dropdowns.
- **Header search:** a "Find anything" field in the header opens the command bar, so it can be found without the key.
- **Member list:** hides by itself when the chat would get narrower than 480 px. The channel topic sits at its top.
- **Freeze dumps:** `MiniDumpWriteDump` on the own process froze about 1 in 4 test runs (4 of 15, against 0 of 15
  without that test), so the test that writes a real dump only runs on request. The watchdog itself still dumps
  in-process, as in the iced client; Microsoft advises a separate process for this. Not changed yet.

## 2026-11-07: window tests for hermes-gpui
**What changed:** `hermes-gpui` has a new dev-dependency, `gpui = { package = "gpui-pre", version = "=0.3.11", features = ["test-support"] }`, the gpui the kit already pins with its test platform on. `the_mouse_wheel_over_a_chat_does_not_panic` runs the real `Shell` in a headless window against a real server.
**Why:** the wheel crash came from a borrow only a real event dispatch shows. The test fails with the same panic when the fix is taken out. The kit's own `test-support` feature would be simpler, but it mentions the optional `gpui-fast`, which makes Cargo resolve it and would force `taffy` from 0.15.1 down to 0.15.0 in the app. Tests that use real I/O need `cx.executor().allow_parking()`.

## 2026-11-07: smaller default text and named sizes
**What changed:** `font.size` now defaults to 11, which `hermes-gpui` draws at 14 px, with secondary text at 13 and captions at 12. Before, it was 13 (16 px, chosen to be close to Discord). Text uses four roles (`heading_size`, `body_size`, `secondary_size`, `caption_size`) instead of three sizes with one-off additions.
**Why:** the developer tried the client, found the default too big and the sizes inconsistent, and chose 14 px. A saved `font.size` is not touched. The default lives in `hermes-client-core`, so a new iced install would start at 11 px; that client is on its way out.
