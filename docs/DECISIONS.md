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
