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
