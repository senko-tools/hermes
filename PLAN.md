# Hermes — Project Plan

> **Project name:** `Hermes`. Every crate, binary, env var and URL scheme below uses `hermes`.
>
> **Official instance:** `hermes.senko.tools` (a Hetzner VPS running Debian 13 "trixie"). See [18.8](#188-reference-deployment-hermessenkotools-on-hetzner-debian-13).
>
> **Code:** one GitHub repository, `github.com/senko-tools/hermes`, inside the GitHub organization `senko-tools`. See [5.1](#51-repositories-and-versioning). (If the organization name `senko-tools` is unavailable, pick another and search-and-replace `senko-tools` in this file.)
>
> **One-line goal:** A self-hostable, open-source chat platform for friend groups and small communities that has **community "spaces" with text and voice channels, private messages (1:1 and group), and voice calls**, written entirely in **Rust**, with a polished **iced** desktop client (primary, for everyone), a **ratatui** terminal client (secondary, for the author/power users), and a **single-binary server** that can be deployed with a few commands or a Pterodactyl egg.

This document is the full specification and build guide. Someone who has never seen the conversation it came from should be able to read it top to bottom and build the system without needing to make architectural decisions. Every major decision is already made, and the reason for it is written next to it. If a decision really must change during development, record it in `docs/DECISIONS.md` (date, what changed, why) so the plan and the code stay consistent.

---

## Table of contents

1. [Background and goals](#1-background-and-goals)
2. [Options considered and the decision](#2-options-considered-and-the-decision)
3. [Scope: what v1.0 includes and excludes](#3-scope-what-v10-includes-and-excludes)
4. [System architecture overview](#4-system-architecture-overview)
5. [Repository layout](#5-repository-layout)
6. [Technology choices (crates)](#6-technology-choices-crates)
7. [Domain model](#7-domain-model)
8. [Permissions](#8-permissions)
9. [Server specification](#9-server-specification)
10. [REST API](#10-rest-api)
11. [Gateway (real-time WebSocket) protocol](#11-gateway-real-time-websocket-protocol)
12. [Voice system](#12-voice-system)
13. [Security model](#13-security-model)
14. [Client architecture (shared core)](#14-client-architecture-shared-core)
15. [iced GUI client specification](#15-iced-gui-client-specification)
16. [TUI client specification](#16-tui-client-specification)
17. [Client configuration](#17-client-configuration)
18. [Self-hosting and deployment](#18-self-hosting-and-deployment)
19. [Testing strategy](#19-testing-strategy)
20. [Build order: phases and milestones](#20-build-order-phases-and-milestones)
21. [Risks and how to handle them](#21-risks-and-how-to-handle-them)
22. [Post-v1 roadmap](#22-post-v1-roadmap)
23. [Licensing](#23-licensing)
24. [Glossary](#24-glossary)
25. [Working with an AI assistant on this plan](#25-working-with-an-ai-assistant-on-this-plan)

---

## 1. Background and goals

### Why this exists
Existing options each miss something for a small friend group:

- **Discord**: proprietary, not self-hostable, data collection.
- **TeamSpeak (6 beta)**: great voice, but joining a server immediately puts you into voice (you cannot just browse text), and chat is not persisted on the server.
- **IRC (+ clients like Halloy)**: excellent lightweight text and a great client UI, but no voice, and the protocol has no real concept of "community servers" with roles.
- **Matrix/Element, Stoat (Revolt), Fluxer**: closer, but either heavy to self-host, voice is fiddly when self-hosted, or the stack is not Rust (the author wants to build and own the whole thing in Rust).

### Product goals
1. **Spaces** (the equivalent of Discord "servers"): a community with categories, text channels and voice channels, roles and permissions.
2. **Browsing never joins voice.** Users can see every channel and *who is in each voice channel* without being connected. Voice is joined only by explicitly clicking a voice channel or starting a call.
3. **All chat history is stored on the server** and is available from any device, with unread tracking.
4. **Private messages**: 1:1 DMs and group DMs, both with voice calls.
5. **Your own messages appear instantly** (optimistic local echo), then are confirmed by the server.
6. **Self-hosting is trivial**: one binary, one data folder, one port number (TCP + UDP), configured by a TOML file or environment variables. Works with Docker, systemd, or a Pterodactyl/Pelican egg.
7. **Good voice quality** with low latency: Opus, noise suppression, push-to-talk or voice activity, per-user volume.

### Non-goals (for v1.0)
Federation between instances, end-to-end encryption, video, screen sharing, mobile apps, a web client, bots marketplace. Several of these are in the [post-v1 roadmap](#22-post-v1-roadmap) and the design leaves room for them.

### Target scale
Designed for friend groups and small communities: **up to ~200 registered users per instance, ~50 online at once, up to ~25 people in one voice channel**. Everything is chosen to be simple at this scale (SQLite, in-memory state cache, single process). Nothing prevents scaling later, but it is not a v1 concern.

---

## 2. Options considered and the decision

| Option | Summary | Pros | Cons |
|---|---|---|---|
| **1. Custom iced client + custom backend** | Build everything | Full control over protocol, UX and features; one coherent codebase | Most work, especially making the GUI look good |
| **2. TUI client + custom backend** | ratatui front end | Fast to build, looks good easily | Keyboard-centric, limited mouse support, hard for non-technical friends; terminals cannot detect key *release* so push-to-talk is not possible |
| **3. Fork Halloy and add voice** | Reuse an existing polished IRC GUI | GUI largely done | IRC has no spaces, roles, server-side DMs, persistent per-user unread state, or voice. You would need a custom server anyway, then rip IRC out of Halloy's internals. Ends up as option 1 with someone else's architecture |

### Decision
**Option 1 as the primary product, plus option 2 as a secondary client, both on top of one shared client library.**

- The **iced GUI** (`hermes-gui`) is the main client for everyone.
- The **TUI** (`hermes-tui`) is the author's personal client and a fast development/debugging tool. It is built *first* for text features because it is quick to iterate on, which validates the protocol before GUI work starts.
- Both clients are thin UIs over **`hermes-client-core`** (networking, state, audio). Building the second UI costs only the UI itself.
- **Halloy is used as design inspiration**, not forked: its layout system (split panes via iced `pane_grid`, sidebar tree, command bar, TOML config, themes, font settings) is the target look and feel for the GUI. Halloy is GPL-3.0; because the Hermes clients are also GPL-3.0 (see [Licensing](#23-licensing)), individual widgets or patterns *can* be adapted from Halloy's source with attribution.

---

## 3. Scope: what v1.0 includes and excludes

### Included in v1.0
**Accounts**
- Username + password registration and login, per-device sessions, log out other sessions.
- Display name, avatar, custom status text, presence (online / idle / do-not-disturb / invisible).
- Registration modes: `open`, `invite_only` (default), `closed`.

**Spaces**
- Create/rename/delete a space, icon, description.
- Categories, text channels, voice channels, with drag-to-reorder.
- Invites (link with optional max uses and expiry).
- Roles with colors, hoisting, ordering and permission bits; per-channel permission overrides.
- Moderation: kick, ban, delete messages, server-mute/deafen, move users between voice channels, audit log.

**Messaging**
- Markdown subset (bold, italic, underline, strikethrough, inline code, code blocks with language, block quotes, spoilers, links).
- Mentions: user, role, channel, `@everyone`/`@here`.
- Replies, edits (with "edited" marker), deletes, reactions (Unicode emoji), pins.
- Attachments (files, images with previews), paste image from clipboard, drag and drop.
- Typing indicators, read states, unread and mention badges synced across devices.
- Message history with infinite scroll-back, jump to message.
- Full-text search (SQLite FTS5).

**Private messages**
- 1:1 DMs and group DMs (up to 10 people), with the same message features.

**Voice**
- Voice channels in spaces; you can see who is in them without joining.
- Voice calls in DMs and group DMs (with ringing).
- Opus audio, noise suppression, voice activity detection or push-to-talk, self-mute/deafen, per-user volume, speaking indicators, input/output device selection, join/leave sounds.

**Clients**
- Windows is the primary target. Linux and macOS should build and work but are best-effort.
- Desktop notifications, tray icon, configurable themes, fonts and keybinds, split panes, command bar.

**Server/self-hosting**
- Single static binary, SQLite storage, files on disk, built-in TLS (self-signed with fingerprint pinning, or your own certificates), works behind a reverse proxy.
- Docker image + compose file, systemd unit + install script, Pterodactyl/Pelican egg.
- Admin CLI (create invite, reset password, backup).

### Excluded from v1.0 (see roadmap)
Video and screen share, echo cancellation (v1 assumes headphones or relies on noise suppression), custom emoji, link previews, threads/forums, bots API, web/mobile clients, E2E encryption, federation, Postgres support.

---

## 4. System architecture overview

```
                       ┌──────────────────────────── hermes-server (one process) ───────────────────────────┐
                       │                                                                                   │
  hermes-gui (iced)    │   TCP :7700  ── HTTPS ──►  REST API (axum)  ──┐                                    │
  hermes-tui (ratatui) │                                               ├─► Domain services ─► State cache    │
         │             │   TCP :7700  ── WSS  ──►  Gateway (events) ──┘       (auth, spaces,   (RwLock, in   │
         │             │                               ▲                       messages, ...)   memory)      │
  hermes-client-core   │                               │ event bus                  │                       │
   ├─ http + ws client │                               │                            ▼                       │
   ├─ state store      │   UDP :7700  ── QUIC ──►  Voice SFU (quinn) ───────►  SQLite (WAL) + files/        │
   └─ hermes-audio     │                 (datagrams = Opus packets, streams = control)                     │
      (cpal, opus,     │                                                                                   │
       noise supp.)    └───────────────────────────────────────────────────────────────────────────────────┘
```

Key points:
- **One port number, two protocols**: TCP 7700 for HTTPS/WebSocket, UDP 7700 for QUIC voice. Default port is `7700`, configurable. This maps cleanly onto a single Pterodactyl allocation or a single router port-forward rule (TCP+UDP).
- **Mutations go through REST**, **events come through the gateway** WebSocket. This keeps request/response errors simple and the event stream one-directional.
- **Voice is a Selective Forwarding Unit (SFU)**: each client sends one Opus stream to the server, the server forwards it to the other participants in the room. No mixing or transcoding on the server, so CPU usage is tiny.
- **State cache**: all spaces, channels, roles, members, overrides, voice states and presences are loaded into memory at startup and written through to SQLite on change. Permission checks never hit the database. Messages are *not* cached on the server; they are queried from SQLite.

---

## 5. Repository layout

### 5.1 Repositories and versioning
**Decision: one repository (a "monorepo") named `hermes`, owned by a GitHub organization named `senko-tools`.** The server and the clients are separate crates and separate binaries inside it, not separate repositories and not branches.

Why not two repositories (`hermes-server` + `hermes-client`):
- Both sides depend on `hermes-proto`, the crate that defines every message on the wire. With split repos it would need a third repository (`hermes-proto`) and every protocol change would become three coordinated commits, version bumps and releases. In one repo a protocol change is **one commit** that updates the types, the server and the clients together, and CI proves they still agree.
- One CI pipeline, one issue tracker, one place for this plan and the docs, one `cargo test --workspace`.
- License per crate still works fine inside one repo (see [Licensing](#23-licensing)).

Grouping on GitHub: GitHub has no folders for repositories. The grouping mechanism is an **organization** (free): `github.com/senko-tools` holds `hermes` now and can later hold related repositories (for example a `hermes-website` or `hermes-eggs` repo), with shared members, permissions and a pinned-repos overview page. (Git submodules or a "meta repository" exist too, but they add friction and are not needed here.)

Versioning: **lockstep**. Server and clients share one version number and are released together from one git tag `vX.Y.Z` (semver). A release that only changes the client still produces a server build with the same version; this keeps GitHub's `releases/latest` URL meaningful for install scripts and the Pterodactyl egg. Compatibility between different versions is handled by `protocol_version`, not by matching release numbers.

Branches: `main` is always releasable; work happens on short-lived feature branches merged by pull request with CI green.

### 5.2 Workspace layout
A single Cargo workspace (Rust edition 2024).

```
hermes/
├── Cargo.toml                  # [workspace], shared dependency versions, lints, release profile
├── PLAN.md                     # this file
├── README.md
├── LICENSE-GPL-3.0, LICENSE-AGPL-3.0, LICENSE-MIT, LICENSE-APACHE
├── crates/
│   ├── hermes-proto/           # Shared types: IDs, models, REST bodies, gateway events, voice packet format,
│   │                           #   permissions, error codes, markup parser. No I/O. (MIT OR Apache-2.0)
│   ├── hermes-server/          # Server binary + library (AGPL-3.0)
│   │   ├── migrations/         # sqlx SQL migrations (embedded into the binary)
│   │   └── src/{main.rs, config.rs, db/, state/, auth/, api/, gateway/, voice/, files/, cli/}
│   ├── hermes-audio/           # Capture, playback, resampling, noise suppression, Opus, jitter buffer, mixer (GPL-3.0)
│   ├── hermes-client-core/     # Connection management, REST+WS client, state store, voice transport, config (GPL-3.0)
│   ├── hermes-gui/             # iced desktop client (GPL-3.0)
│   ├── hermes-tui/             # ratatui terminal client (GPL-3.0)
│   └── hermes-voicebot/        # Dev tool: headless client that joins voice and plays/records WAV files (GPL-3.0)
├── deploy/
│   ├── docker/{Dockerfile, compose.yaml}
│   ├── pterodactyl/egg-hermes.json
│   ├── systemd/hermes.service
│   └── install.sh
├── docs/
│   ├── DECISIONS.md            # log of deviations from this plan
│   ├── self-hosting.md         # end-user hosting guide (written in Phase 7)
│   └── protocol.md             # human-readable protocol reference (kept in sync with hermes-proto)
└── .github/workflows/{ci.yml, release.yml}
```

Rules:
- `hermes-proto` is the **single source of truth** for everything on the wire. Server and clients both depend on it, so they can never disagree about a field name.
- Clients never depend on `hermes-server`; the server never depends on client crates.
- UI crates contain no networking or audio code; they only call `hermes-client-core`.

---

## 6. Technology choices (crates)

Pin exact versions in the workspace `Cargo.toml` at project start, using the latest stable release of each at that time. Versions verified at the time of writing are noted.

| Area | Crate | Notes |
|---|---|---|
| Async runtime | `tokio` | multi-threaded runtime everywhere |
| HTTP server | `axum` + `tower` + `tower-http` | REST, WebSocket upgrade, static file serving, tracing, compression |
| Rate limiting | `tower_governor` | per-token and per-IP limits |
| TLS | `rustls` + `tokio-rustls`, `rcgen` | `rcgen` generates the self-signed certificates |
| Database | `sqlx` (SQLite feature) | embedded migrations (`sqlx::migrate!`), use offline mode (`cargo sqlx prepare`) for compile-time checked queries |
| QUIC | `quinn` (0.11.x at time of writing) | application datagrams for audio, streams for control |
| Serialization | `serde`, `serde_json` | JSON on the wire for REST and gateway |
| Password hashing | `argon2` | argon2id |
| Randomness/hashing | `rand`, `sha2`, `base64` | tokens, fingerprints |
| Config | `figment` (TOML + env provider) | env override `HERMES__SECTION__KEY` |
| CLI | `clap` (derive) | server subcommands, client flags |
| Logging | `tracing`, `tracing-subscriber` | stdout logs; JSON format option |
| Images (server) | `image` | dimensions + thumbnails for uploads |
| Bitflags | `bitflags` | permissions |
| HTTP client | `reqwest` (rustls) | client REST calls |
| WebSocket client | `tokio-tungstenite` (rustls) | gateway |
| Audio I/O | `cpal` | WASAPI on Windows |
| Opus | `audiopus` (or `opus`) | libopus bindings; build statically |
| Resampling | `rubato` | device rate ↔ 48 kHz |
| Noise suppression | `nnnoiseless` | pure-Rust RNNoise port, works on 48 kHz frames of 480 samples |
| Lock-free buffers | `ringbuf` | between audio callbacks and worker threads |
| GUI | `iced` (0.14 at time of writing; use the newest stable) | `pane_grid`, `text_editor`, `image`, `svg`, wgpu renderer with tiny-skia fallback |
| TUI | `ratatui` (0.30.x at time of writing) + `crossterm` (event-stream feature) | |
| Markdown | `pulldown-cmark` | wrapped by Hermes's own markup layer in `hermes-proto` (adds mentions and spoilers) |
| Emoji | `emojis` | shortcode lookup and picker data |
| Clipboard | `arboard` | text + image paste |
| Notifications | `notify-rust` | Windows toast support |
| Tray icon | `tray-icon` | |
| Global hotkeys | `global-hotkey` | push-to-talk (see [Risks](#21-risks-and-how-to-handle-them)) |
| Secrets storage | `keyring` | session tokens in Windows Credential Manager |
| Directories | `directories` | `%APPDATA%\Hermes` etc. |
| File watching | `notify` | hot-reload of client config and themes |
| Windows packaging | `cargo-packager` | NSIS installer + portable zip |

---

## 7. Domain model

### 7.1 Concepts
- **Instance**: one running `hermes-server`. It has user accounts and hosts any number of spaces. A client can be connected to several instances at once (like Halloy connects to several IRC networks).
- **User**: an account on an instance. Users are instance-wide.
- **Space**: a community (equivalent of a Discord "server"). Has members, roles, categories and channels.
- **Channel**: kinds are `text`, `voice`, `category` (space channels) and `dm`, `group_dm` (private channels, not in a space). Every voice channel also has a text chat (like Discord), so voice channels are also message containers.
- **Message**: belongs to a channel.
- **Voice room**: the live voice session of a voice channel or DM/group DM. A user can be in at most **one** voice room per instance at a time; joining another one leaves the first.

### 7.2 IDs
All IDs are **64-bit snowflakes**, serialized in JSON as **strings** (so a future web client in JavaScript cannot lose precision).

```
bits 63..22 (42 bits): milliseconds since Hermes epoch 2026-01-01T00:00:00Z
bits 21..12 (10 bits): worker id (always 0 in v1)
bits 11..0  (12 bits): per-millisecond sequence
```
IDs are therefore sortable by creation time; message pagination uses ID comparisons. The `@everyone` role of a space has the **same ID as the space**.

### 7.3 Database schema (SQLite)
SQLite opened in WAL mode, `foreign_keys=ON`, `busy_timeout=5000`. All IDs are `INTEGER` (i64). Timestamps are Unix milliseconds (`INTEGER`). Booleans are `INTEGER 0/1`.

```sql
-- 0001_init.sql
CREATE TABLE users (
  id              INTEGER PRIMARY KEY,
  username        TEXT NOT NULL UNIQUE COLLATE NOCASE,  -- 2..32 chars, [a-z0-9_.]
  display_name    TEXT,                                 -- 1..32 chars
  password_hash   TEXT NOT NULL,                        -- argon2id PHC string
  avatar_id       INTEGER REFERENCES attachments(id),
  status_text     TEXT,                                 -- 0..128 chars
  instance_admin  INTEGER NOT NULL DEFAULT 0,
  disabled        INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL
);

CREATE TABLE sessions (
  id           INTEGER PRIMARY KEY,
  user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash   BLOB NOT NULL UNIQUE,                    -- SHA-256 of the bearer token
  device_name  TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  last_ip      TEXT
);

CREATE TABLE spaces (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,                            -- 2..64
  description TEXT,                                     -- 0..512
  icon_id     INTEGER REFERENCES attachments(id),
  owner_id    INTEGER NOT NULL REFERENCES users(id),
  created_at  INTEGER NOT NULL
);

CREATE TABLE space_members (
  space_id  INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  nickname  TEXT,
  joined_at INTEGER NOT NULL,
  PRIMARY KEY (space_id, user_id)
);

CREATE TABLE roles (
  id          INTEGER PRIMARY KEY,                      -- @everyone role id == space id
  space_id    INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  color       INTEGER,                                  -- 0xRRGGBB or NULL
  hoist       INTEGER NOT NULL DEFAULT 0,               -- show separately in member list
  mentionable INTEGER NOT NULL DEFAULT 0,
  position    INTEGER NOT NULL,                         -- higher = more powerful; @everyone = 0
  permissions INTEGER NOT NULL                          -- u64 bitfield stored as i64
);

CREATE TABLE member_roles (
  space_id INTEGER NOT NULL,
  user_id  INTEGER NOT NULL,
  role_id  INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (space_id, user_id, role_id),
  FOREIGN KEY (space_id, user_id) REFERENCES space_members(space_id, user_id) ON DELETE CASCADE
);

CREATE TABLE channels (
  id             INTEGER PRIMARY KEY,
  space_id       INTEGER REFERENCES spaces(id) ON DELETE CASCADE,  -- NULL for dm/group_dm
  kind           TEXT NOT NULL CHECK (kind IN ('text','voice','category','dm','group_dm')),
  parent_id      INTEGER REFERENCES channels(id) ON DELETE SET NULL, -- category
  name           TEXT,                                 -- NULL for dm
  topic          TEXT,
  position       INTEGER NOT NULL DEFAULT 0,
  bitrate        INTEGER,                              -- voice only, bits/s
  user_limit     INTEGER,                              -- voice only, 0/NULL = unlimited
  slowmode_secs  INTEGER NOT NULL DEFAULT 0,
  owner_id       INTEGER REFERENCES users(id),         -- group_dm owner
  last_message_id INTEGER,
  created_at     INTEGER NOT NULL
);

CREATE TABLE channel_overrides (
  channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  role_id    INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  allow      INTEGER NOT NULL DEFAULT 0,
  deny       INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (channel_id, role_id)
);

CREATE TABLE dm_recipients (
  channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (channel_id, user_id)
);

CREATE TABLE messages (
  id          INTEGER PRIMARY KEY,
  channel_id  INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  author_id   INTEGER NOT NULL REFERENCES users(id),
  content     TEXT NOT NULL,                           -- 0..4000 chars (raw markup)
  reply_to_id INTEGER,                                 -- not a FK: replied message may be deleted
  edited_at   INTEGER,
  deleted     INTEGER NOT NULL DEFAULT 0,              -- soft delete; content cleared
  pinned      INTEGER NOT NULL DEFAULT 0,
  kind        TEXT NOT NULL DEFAULT 'default'          -- 'default' | 'system' (join, call started, pin, ...)
);
CREATE INDEX messages_channel_id ON messages(channel_id, id);

CREATE TABLE message_mentions (                         -- for mention counts and "mentions" inbox
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL CHECK (kind IN ('user','role','everyone','here')),
  target_id  INTEGER,
  PRIMARY KEY (message_id, kind, target_id)
);

CREATE TABLE attachments (
  id           INTEGER PRIMARY KEY,
  uploader_id  INTEGER NOT NULL REFERENCES users(id),
  message_id   INTEGER REFERENCES messages(id) ON DELETE SET NULL, -- NULL until attached (or avatar/icon)
  filename     TEXT NOT NULL,
  content_type TEXT NOT NULL,
  size         INTEGER NOT NULL,
  width        INTEGER,
  height       INTEGER,
  has_thumb    INTEGER NOT NULL DEFAULT 0,
  sha256       BLOB NOT NULL,
  created_at   INTEGER NOT NULL
);

CREATE TABLE reactions (
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  emoji      TEXT NOT NULL,                            -- Unicode emoji sequence
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (message_id, emoji, user_id)
);

CREATE TABLE read_states (
  user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  channel_id    INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  last_read_id  INTEGER NOT NULL DEFAULT 0,
  mention_count INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (user_id, channel_id)
);

CREATE TABLE invites (
  code        TEXT PRIMARY KEY,                        -- 10 chars, base62
  space_id    INTEGER REFERENCES spaces(id) ON DELETE CASCADE, -- NULL = instance-only invite
  creator_id  INTEGER NOT NULL REFERENCES users(id),
  max_uses    INTEGER,                                 -- NULL = unlimited
  uses        INTEGER NOT NULL DEFAULT 0,
  expires_at  INTEGER,                                 -- NULL = never
  created_at  INTEGER NOT NULL
);

CREATE TABLE bans (
  space_id   INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  reason     TEXT,
  banned_by  INTEGER NOT NULL REFERENCES users(id),
  created_at INTEGER NOT NULL,
  PRIMARY KEY (space_id, user_id)
);

CREATE TABLE notification_settings (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_id  INTEGER NOT NULL,                         -- space id or channel id
  level      TEXT NOT NULL CHECK (level IN ('all','mentions','none')),
  muted_until INTEGER,                                 -- NULL = not muted, -1 = forever
  PRIMARY KEY (user_id, target_id)
);

CREATE TABLE audit_log (
  id         INTEGER PRIMARY KEY,
  space_id   INTEGER NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
  actor_id   INTEGER NOT NULL REFERENCES users(id),
  action     TEXT NOT NULL,                            -- e.g. 'member_kick', 'role_update', 'channel_delete'
  target_id  INTEGER,
  details    TEXT NOT NULL DEFAULT '{}',               -- JSON
  created_at INTEGER NOT NULL
);
CREATE INDEX audit_log_space ON audit_log(space_id, id);

-- 0002_search.sql
CREATE VIRTUAL TABLE messages_fts USING fts5(content, content='messages', content_rowid='id');
-- plus INSERT/UPDATE/DELETE triggers keeping messages_fts in sync
```

Note: there is no `created_at` on messages; it is derived from the snowflake ID.

### 7.4 Wire models (in `hermes-proto`)
Rust structs with `#[derive(Serialize, Deserialize)]`, `#[serde(rename_all = "snake_case")]`. IDs use a newtype `Id(u64)` that serializes as a string. Main models:

```rust
pub struct User { id: Id, username: String, display_name: Option<String>, avatar: Option<AttachmentRef>, status_text: Option<String> }
pub struct Space { id: Id, name: String, description: Option<String>, icon: Option<AttachmentRef>, owner_id: Id }
pub struct Role { id: Id, space_id: Id, name: String, color: Option<u32>, hoist: bool, mentionable: bool, position: i32, permissions: Permissions }
pub struct Member { space_id: Id, user: User, nickname: Option<String>, role_ids: Vec<Id>, joined_at: i64 }
pub struct Channel { id: Id, space_id: Option<Id>, kind: ChannelKind, parent_id: Option<Id>, name: Option<String>, topic: Option<String>,
                     position: i32, bitrate: Option<u32>, user_limit: Option<u32>, slowmode_secs: u32,
                     overrides: Vec<Override>, recipients: Vec<User> /* dm only */, owner_id: Option<Id>, last_message_id: Option<Id> }
pub struct Override { role_id: Id, allow: Permissions, deny: Permissions }
pub struct Message { id: Id, channel_id: Id, author: User, content: String, reply_to: Option<MessageRef>, edited_at: Option<i64>,
                     deleted: bool, pinned: bool, kind: MessageKind, attachments: Vec<Attachment>, reactions: Vec<ReactionSummary>,
                     mentions: Mentions, nonce: Option<String> /* only echoed to the sender */ }
pub struct MessageRef { id: Id, author_id: Id, excerpt: String /* first 100 chars, empty if deleted */ }
pub struct Attachment { id: Id, filename: String, content_type: String, size: u64, width: Option<u32>, height: Option<u32>, has_thumb: bool }
pub struct ReactionSummary { emoji: String, count: u32, me: bool }
pub struct ReadState { channel_id: Id, last_read_id: Id, mention_count: u32 }
pub struct VoiceState { user_id: Id, channel_id: Id, self_mute: bool, self_deaf: bool, server_mute: bool, server_deaf: bool, joined_at: i64 }
pub struct Presence { user_id: Id, status: Status /* online|idle|dnd|offline */ }
pub struct Invite { code: String, space: Option<SpacePreview>, creator_id: Id, max_uses: Option<u32>, uses: u32, expires_at: Option<i64> }
```

### 7.5 Markup
Raw message text is stored and sent as-is. Both clients render it through one parser in `hermes-proto::markup`, which produces a `Vec<Span>` tree, so GUI and TUI render identically.

Supported syntax: `**bold**`, `*italic*`/`_italic_`, `__underline__`, `~~strike~~`, `` `code` ``, fenced code blocks with optional language, `> quote`, `||spoiler||`, autolinks and `[text](url)`, mentions `<@USER_ID>`, `<@&ROLE_ID>`, `<#CHANNEL_ID>`, `@everyone`, `@here`. Headings, tables and images in markdown are **not** rendered (shown as plain text). The input box converts `@name` / `#channel` autocompletions to the ID forms before sending.

---

## 8. Permissions

### 8.1 Bits (`u64`, defined with `bitflags` in `hermes-proto`)

| Bit | Name | Meaning |
|---|---|---|
| 0 | `ADMINISTRATOR` | all permissions, bypasses overrides |
| 1 | `MANAGE_SPACE` | edit name, icon, description |
| 2 | `MANAGE_ROLES` | create/edit/delete roles below own highest role |
| 3 | `MANAGE_CHANNELS` | create/edit/delete/reorder channels and overrides |
| 4 | `KICK_MEMBERS` | |
| 5 | `BAN_MEMBERS` | |
| 6 | `CREATE_INVITES` | |
| 7 | `MANAGE_INVITES` | view/delete others' invites |
| 8 | `CHANGE_NICKNAME` | own nickname |
| 9 | `MANAGE_NICKNAMES` | others' nicknames |
| 10 | `VIEW_AUDIT_LOG` | |
| 16 | `VIEW_CHANNEL` | see channel and (for voice) who is in it |
| 17 | `SEND_MESSAGES` | |
| 18 | `READ_HISTORY` | read messages sent before you opened the channel |
| 19 | `ATTACH_FILES` | |
| 20 | `ADD_REACTIONS` | add *new* reaction emoji (joining an existing one only needs VIEW) |
| 21 | `MENTION_EVERYONE` | `@everyone`, `@here`, non-mentionable roles |
| 22 | `MANAGE_MESSAGES` | delete others' messages, pin |
| 23 | `BYPASS_SLOWMODE` | |
| 32 | `CONNECT` | join voice |
| 33 | `SPEAK` | send audio |
| 34 | `USE_VOICE_ACTIVITY` | without it, push-to-talk is required |
| 35 | `MUTE_MEMBERS` | server mute |
| 36 | `DEAFEN_MEMBERS` | server deafen |
| 37 | `MOVE_MEMBERS` | move/disconnect users in voice |

Gaps between groups are reserved for future bits.

**Default `@everyone` permissions:** `CHANGE_NICKNAME | CREATE_INVITES | VIEW_CHANNEL | SEND_MESSAGES | READ_HISTORY | ATTACH_FILES | ADD_REACTIONS | CONNECT | SPEAK | USE_VOICE_ACTIVITY`.

### 8.2 Algorithm (`hermes-proto::permissions::compute`)
Pure function, heavily unit-tested.

```
fn compute(space, member, channel) -> Permissions:
    if member.user_id == space.owner_id: return ALL
    base = role(@everyone).permissions
    for r in member.roles: base |= r.permissions
    if base contains ADMINISTRATOR: return ALL
    if channel is None: return base

    p = base
    // 1) @everyone override
    if o = override(channel, @everyone): p = (p & !o.deny) | o.allow
    // 2) all member role overrides combined
    allow = 0; deny = 0
    for r in member.roles: if o = override(channel, r): allow |= o.allow; deny |= o.deny
    p = (p & !deny) | allow
    // 3) implicit rules
    if !(p contains VIEW_CHANNEL): return 0
    if channel.kind == voice && !(p contains CONNECT): p &= !(SPEAK | USE_VOICE_ACTIVITY)
    return p
```
Category overrides are **not** inherited automatically; when a channel is created inside a category, the category's overrides are **copied** to it (simple, predictable).

**Hierarchy rule**: a user can only kick/ban/edit/assign roles for members and roles whose highest position is **strictly lower** than their own highest role (owner exempt).

**DMs**: all recipients have `VIEW_CHANNEL | SEND_MESSAGES | READ_HISTORY | ATTACH_FILES | ADD_REACTIONS | CONNECT | SPEAK | USE_VOICE_ACTIVITY`. Nobody has moderation permissions except: the group DM owner can remove recipients.

**Who can open a DM with whom**: controlled by server config `dm_policy`: `shared_space` (default: you must share at least one space) or `anyone`.

---

## 9. Server specification

### 9.1 Binary and CLI
Binary name: `hermes-server`.

```
hermes-server run        [--config <path>]        # default subcommand; starts the server
hermes-server init       [--config <path>]        # writes a commented default hermes.toml and exits
hermes-server check-config [--config <path>]      # validates config, prints effective values
hermes-server invite create [--space <id>] [--uses N] [--expires 7d]   # prints invite link
hermes-server user reset-password <username>      # prints a new random password
hermes-server user set-admin <username> <true|false>
hermes-server user disable <username>
hermes-server backup <output-file>                # consistent online copy via `VACUUM INTO`
hermes-server version
```
`--config <path>` is a global flag accepted by every subcommand (default `./hermes.toml`, or the `HERMES_CONFIG` env var); admin subcommands read it to find the data directory.
Admin subcommands talk directly to the database file and are safe to run while the server is running (SQLite WAL); the running server picks changes up via a lightweight "reload users" trigger (it re-reads the changed user row on the next request for that user; for `disable`, sessions of that user are revoked in the DB and the server checks `disabled` on each authenticated request through the cached user record, refreshed every 10 seconds).

**First start:** if the database has no users, the server prints, in a clearly framed block in the console:
```
==================================================================
 Hermes first-time setup
 Open this link in the Hermes client to create the OWNER account:
 https://hermes.senko.tools:7700/invite/AbC123xYz9
 (single use, valid for 24 hours; run `hermes-server invite create`
  for a new one)
==================================================================
```
The first account created with this setup invite becomes `instance_admin` and owner of a default space named after `instance.name`, with channels `#general` (text) and `General` (voice).

Once fully started, the server logs exactly: `Hermes is ready on <bind>:<port>` (used by Pterodactyl to detect startup).

**Certificate reload:** on `SIGHUP`, and automatically when the files' modification time changes (checked every 60 s), the server reloads `tls.cert_file`/`tls.key_file` without dropping connections (rustls `ResolvesServerCert` backed by an `ArcSwap`). This makes Let's Encrypt renewals seamless. Other config changes require a restart.

Graceful shutdown on SIGINT/SIGTERM/Ctrl+C: stop accepting connections, send gateway close code 4100 ("server restarting", clients reconnect automatically with backoff), close QUIC connections, flush SQLite, exit within 10 seconds.

### 9.2 Configuration
File `hermes.toml` (path from `--config`, default `./hermes.toml`). Every key can be overridden by an environment variable: `HERMES__<SECTION>__<KEY>` in upper case, for example `HERMES__NETWORK__PORT=7700`. Env vars win over the file. Missing file → defaults + env (so a container can be configured purely through env vars).

```toml
[instance]
name = "Hermes"                     # shown in clients; also the default space name on first start
description = ""
public_host = ""                    # REQUIRED for correct invite links: domain or public IP clients use, e.g. "hermes.senko.tools"
public_port = 0                     # 0 = same as network.port (set this if behind NAT/port-mapping)

[network]
bind = "0.0.0.0"                    # use "::" to accept IPv4 + IPv6 (dual stack); recommended when the domain has an AAAA record
port = 7700                         # TCP (HTTPS/WSS) and UDP (QUIC voice) both use this port
trusted_proxies = []                # IPs whose X-Forwarded-For is trusted (when tls.mode = "off" behind a proxy)

[tls]
mode = "self_signed"                # "self_signed" | "files" | "off"
cert_file = ""                      # mode = "files": PEM certificate chain
key_file = ""                       # mode = "files": PEM private key
# "off" = plain HTTP/WS, only for running behind a TLS-terminating reverse proxy.
# The voice QUIC endpoint ALWAYS uses its own self-signed cert stored in data_dir;
# clients pin its fingerprint received over the authenticated gateway.

[storage]
data_dir = "./data"                 # hermes.db, files/, certs/ live here
max_upload_mb = 25
max_total_storage_gb = 0            # 0 = unlimited; uploads rejected when exceeded
thumbnails = true

[registration]
mode = "invite_only"                # "open" | "invite_only" | "closed"
min_password_length = 8

[limits]
max_spaces_per_user = 10            # spaces a non-admin user may create; 0 = only instance admins
max_message_length = 4000
max_group_dm_size = 10
messages_per_5s = 5                 # per user per channel
requests_per_second = 30            # per session, REST
login_attempts_per_minute = 5       # per IP

[privacy]
dm_policy = "shared_space"          # "shared_space" | "anyone"

[voice]
enabled = true
default_bitrate = 64000             # bits/s, per new voice channel
max_bitrate = 128000
max_users_per_room = 25

[log]
level = "info"                      # trace | debug | info | warn | error, or an EnvFilter string
format = "pretty"                   # "pretty" | "json"
```

### 9.3 Data directory
```
data/
├── hermes.db (+ -wal, -shm)
├── files/ab/cd/<attachment_id>          # sharded by first bytes of sha256 hex
├── files/ab/cd/<attachment_id>.thumb.webp
└── certs/
    ├── https-cert.pem, https-key.pem   # generated when tls.mode = "self_signed"
    └── voice-cert.pem, voice-key.pem   # always generated
```
Self-signed certificates are generated once with `rcgen` (10-year validity, SANs = `public_host` + bind IP) and reused forever so fingerprints stay stable. Deleting them changes the fingerprint and clients will warn (by design).

### 9.4 Internal structure
```
src/
├── main.rs            # clap → cli::run
├── config.rs          # figment load + validation
├── db/                # sqlx pool, migrations, one module per table group, all SQL lives here
├── state/             # AppState: Arc<RwLock<Cache>> with spaces, channels, roles, members, overrides,
│                      #   users (lightweight), presences, voice_states; load_from_db(); write-through helpers
├── auth/              # password hashing, token issue/verify, axum extractor `Auth(user_id, session_id)`
├── api/               # axum routers per resource; validation; error → HTTP mapping
├── gateway/           # ws handler, Session struct, resume buffer, EventBus, recipient resolution
├── voice/             # quinn endpoint, room registry, control stream handler, datagram forwarding
├── files/             # upload/download, hashing, thumbnail generation (spawn_blocking)
├── snowflake.rs
└── cli/               # admin subcommands
```

**EventBus**: services call `bus.publish(Event, Audience)`, where `Audience` is one of `Space(space_id, required_perm, channel_id?)`, `Channel(channel_id)` (DM recipients, or space members with VIEW_CHANNEL on it), `User(user_id)` (all sessions of one user), `Users(Vec<Id>)`. The bus resolves the audience to session handles using the state cache and pushes the serialized event (serialize once, share `Arc<str>`) into each session's bounded queue (capacity 1024; if full, the session is closed with 4009 and the client resumes/re-identifies).

**Concurrency**: one tokio task per gateway session (reader+writer with `select!`), one task per QUIC connection, `RwLock` cache with short critical sections (never hold the lock across `.await` on I/O).

### 9.5 Server performance targets (on a 1 vCPU / 1 GB VPS)
- Idle RAM < 60 MB; 50 online users + 20 in voice < 200 MB.
- Voice forwarding adds < 5 ms latency at the server.
- Message send → delivered to other clients < 100 ms on the same continent.
- Bandwidth reference: one Opus stream at 64 kbit/s ≈ 75 kbit/s with overhead. A room with N speakers-capable users costs the server about N×(N−1)×75 kbit/s upstream in the worst case (all talking); with 10 users ≈ 6.8 Mbit/s, realistically far less because of DTX/silence.

### 9.6 Addresses, invite links and the invite web page
**Addresses.** Users type an instance address as `host` or `host:port`. **If the port is omitted, clients use 7700.** So friends just type `hermes.senko.tools`. The canonical stored form is `https://host:port`.

**Invite links** exist in two equivalent forms; clients accept both (pasted into "Add instance", or opened from the OS):
- **Web form (what people share):** `https://hermes.senko.tools:7700/invite/<code>` — works in any chat app or browser.
- **App form:** `hermes://hermes.senko.tools/invite/<code>` (port omitted when 7700; otherwise `hermes://host:port/invite/<code>`).
- When the instance uses a self-signed certificate (`tls.mode = "self_signed"`), both forms get the fingerprint appended: `#fp=sha256:<hex>` (the fragment is never sent to the server). With a real certificate (`files`, or `off` behind a proxy) no fingerprint is added.

**Invite web page.** `GET /invite/<code>` (outside `/api/v1`, no auth) returns a tiny self-contained HTML page (embedded in the binary, no JavaScript framework, no external resources): instance name, space name and icon, member count, an **"Open in Hermes"** button pointing to the `hermes://` form, and a **"Download Hermes"** link to `https://github.com/senko-tools/hermes/releases/latest`. Invalid/expired codes show a friendly error. `GET /` returns a similar page with the instance name and the download link. This is all the "website" the server provides; there is no web chat client in v1.

---

## 10. REST API

Base path: `/api/v1`. JSON bodies (`Content-Type: application/json`) except uploads. Auth header: `Authorization: Bearer <token>`. All responses UTF-8 JSON.

**Errors** always look like:
```json
{ "error": { "code": "MISSING_PERMISSION", "message": "You need SEND_MESSAGES in this channel", "retry_after_ms": null } }
```
Codes (`hermes-proto::ErrorCode`): `BAD_REQUEST`, `VALIDATION_FAILED` (with `fields` map), `UNAUTHORIZED`, `INVALID_CREDENTIALS`, `ACCOUNT_DISABLED`, `MISSING_PERMISSION`, `NOT_FOUND`, `CONFLICT` (e.g. username taken), `RATE_LIMITED` (HTTP 429, `retry_after_ms` set), `PAYLOAD_TOO_LARGE`, `STORAGE_FULL`, `REGISTRATION_CLOSED`, `INVITE_INVALID`, `BANNED`, `VOICE_FULL`, `VOICE_DISABLED`, `INTERNAL`.

**Pagination**: list endpoints take `limit` (default 50, max 100) and `before`/`after` IDs.

### Endpoints

| Method & path | Auth | Body / query → Response | Permission / notes |
|---|---|---|---|
| `GET /instance` | no | → `{name, description, version, protocol_version, registration_mode, limits:{max_upload_mb, max_message_length}, voice:{enabled}}` | used when adding an instance |
| `POST /auth/register` | no | `{username, password, invite_code?, device_name}` → `{token, user}` | depends on `registration.mode`; invite joins its space |
| `POST /auth/login` | no | `{username, password, device_name}` → `{token, user}` | rate-limited per IP |
| `POST /auth/logout` | yes | → 204 | revokes current session |
| `GET /auth/sessions` | yes | → `[{id, device_name, created_at, last_seen_at, current}]` | |
| `DELETE /auth/sessions/{id}` | yes | → 204 | closes that session's gateway |
| `GET /users/@me` | yes | → `User` + `{instance_admin}` | |
| `PATCH /users/@me` | yes | `{display_name?, avatar_id?, status_text?}` → `User` | `avatar_id` from an upload |
| `POST /users/@me/password` | yes | `{current_password, new_password}` → 204 | revokes all other sessions |
| `GET /users/{id}` | yes | → `User` + `{mutual_space_ids}` | |
| `POST /spaces` | yes | `{name}` → `Space` (full, as in Ready) | `limits.max_spaces_per_user` |
| `PATCH /spaces/{id}` | yes | `{name?, description?, icon_id?}` → `Space` | `MANAGE_SPACE` |
| `DELETE /spaces/{id}` | yes | → 204 | owner only |
| `POST /spaces/{id}/transfer` | yes | `{user_id}` → 204 | owner only |
| `GET /spaces/{id}/members` | yes | `?after&limit` → `[Member]` | member |
| `PATCH /spaces/{id}/members/{uid}` | yes | `{nickname?, role_ids?}` → `Member` | `MANAGE_NICKNAMES`/`CHANGE_NICKNAME` (self), `MANAGE_ROLES` + hierarchy |
| `DELETE /spaces/{id}/members/{uid}` | yes | → 204 | `KICK_MEMBERS`; `@me` = leave space (owner cannot leave) |
| `GET /spaces/{id}/bans` | yes | → `[{user, reason, created_at}]` | `BAN_MEMBERS` |
| `PUT /spaces/{id}/bans/{uid}` | yes | `{reason?, delete_messages_hours?: 0..168}` → 204 | `BAN_MEMBERS` |
| `DELETE /spaces/{id}/bans/{uid}` | yes | → 204 | `BAN_MEMBERS` |
| `GET /spaces/{id}/audit-log` | yes | `?before&limit` → `[AuditEntry]` | `VIEW_AUDIT_LOG` |
| `POST /spaces/{id}/roles` | yes | `{name, color?, hoist?, mentionable?, permissions?}` → `Role` | `MANAGE_ROLES` |
| `PATCH /spaces/{id}/roles/{rid}` | yes | partial `Role` → `Role` | `MANAGE_ROLES` + hierarchy; cannot grant bits you lack |
| `DELETE /spaces/{id}/roles/{rid}` | yes | → 204 | `@everyone` cannot be deleted |
| `PUT /spaces/{id}/roles/order` | yes | `[{id, position}]` → `[Role]` | |
| `POST /spaces/{id}/channels` | yes | `{kind, name, parent_id?, topic?, bitrate?, user_limit?}` → `Channel` | `MANAGE_CHANNELS` |
| `PUT /spaces/{id}/channels/order` | yes | `[{id, position, parent_id}]` → `[Channel]` | `MANAGE_CHANNELS` |
| `PATCH /channels/{id}` | yes | `{name?, topic?, parent_id?, bitrate?, user_limit?, slowmode_secs?}` → `Channel` | `MANAGE_CHANNELS` (group DM: any recipient may rename) |
| `DELETE /channels/{id}` | yes | → 204 | `MANAGE_CHANNELS`; group DM: = leave |
| `PUT /channels/{id}/overrides/{role_id}` | yes | `{allow, deny}` → 204 | `MANAGE_CHANNELS` + `MANAGE_ROLES` |
| `DELETE /channels/{id}/overrides/{role_id}` | yes | → 204 | same |
| `GET /channels/{id}/messages` | yes | `?before|after|around&limit` → `[Message]` (newest last) | `VIEW_CHANNEL` + `READ_HISTORY` |
| `GET /channels/{id}/messages/{mid}` | yes | → `Message` | |
| `POST /channels/{id}/messages` | yes | `{content, nonce, reply_to_id?, attachment_ids?: [Id] (max 10)}` → `Message` | `SEND_MESSAGES`, `ATTACH_FILES`; slowmode; `nonce` = client UUID (max 64 chars) |
| `PATCH /channels/{id}/messages/{mid}` | yes | `{content}` → `Message` | author only |
| `DELETE /channels/{id}/messages/{mid}` | yes | → 204 | author or `MANAGE_MESSAGES` |
| `PUT /channels/{id}/messages/{mid}/pin` / `DELETE` | yes | → 204 | `MANAGE_MESSAGES` (DM: any recipient) |
| `GET /channels/{id}/pins` | yes | → `[Message]` | |
| `PUT /channels/{id}/messages/{mid}/reactions/{emoji}` | yes | → 204 | emoji URL-encoded; max 20 distinct per message |
| `DELETE /channels/{id}/messages/{mid}/reactions/{emoji}` | yes | → 204 | own reaction |
| `DELETE /channels/{id}/messages/{mid}/reactions/{emoji}/{uid}` | yes | → 204 | `MANAGE_MESSAGES` |
| `POST /channels/{id}/typing` | yes | → 204 | broadcasts `typing_start`; client sends at most every 8 s |
| `POST /channels/{id}/ack` | yes | `{message_id}` → 204 | updates read state, resets mention count, syncs to own other sessions |
| `GET /search` | yes | `?q&space_id?&channel_id?&author_id?&before&limit` → `{messages:[Message], total}` | FTS5; filtered by VIEW+READ_HISTORY |
| `POST /attachments` | yes | raw body, headers `Content-Type`, `X-Filename` (URL-encoded) → `Attachment` | size ≤ `max_upload_mb`; unattached uploads deleted after 24 h |
| `GET /attachments/{id}/{filename}` | yes | → file bytes | must be able to view the message's channel (or avatar/icon: any user); `Content-Disposition: attachment` for non-image types; `X-Content-Type-Options: nosniff` |
| `GET /attachments/{id}/thumb` | yes | → WebP ≤ 400×400 | |
| `GET /dms` | yes | → `[Channel]` | |
| `POST /dms` | yes | `{recipient_ids: [Id]}` → `Channel` | 1 recipient → existing 1:1 DM is reused; 2+ → new group DM; `dm_policy` |
| `PUT /dms/{id}/recipients/{uid}` | yes | → 204 | group DM only, any recipient may add |
| `DELETE /dms/{id}/recipients/{uid}` | yes | → 204 | owner removes others; anyone removes self |
| `POST /spaces/{id}/invites` | yes | `{max_uses?, expires_in_secs?}` → `Invite` | `CREATE_INVITES`; default 7 days, unlimited uses |
| `GET /spaces/{id}/invites` | yes | → `[Invite]` | `MANAGE_INVITES` |
| `GET /invites/{code}` | no | → `Invite` preview (space name, icon, member count) | |
| `POST /invites/{code}` | yes | → `Space` | join; banned → `BANNED` |
| `DELETE /invites/{code}` | yes | → 204 | creator or `MANAGE_INVITES` |
| `POST /voice/join` | yes | `{channel_id, self_mute, self_deaf}` → `{token, expires_in_secs: 30, quic: {host, port, cert_sha256}}` | `CONNECT`; user limit (bypassed by `MOVE_MEMBERS`) |
| `POST /voice/leave` | yes | → 204 | also happens when the QUIC connection closes |
| `PATCH /voice/@me` | yes | `{self_mute?, self_deaf?}` → 204 | client also tells the QUIC control stream |
| `PATCH /spaces/{id}/voice-states/{uid}` | yes | `{server_mute?, server_deaf?, channel_id?: Id|null}` → 204 | `MUTE_MEMBERS` / `DEAFEN_MEMBERS` / `MOVE_MEMBERS`; `null` channel = disconnect |
| `POST /channels/{id}/call/ring` | yes | `{user_ids?}` → 204 | DM/group DM only; sends `call_ring` to recipients not in the call |
| `PUT /users/@me/notification-settings/{target_id}` | yes | `{level, muted_until}` → 204 | |
| `GET /admin/users` | yes | `?after&limit` → `[User + {disabled, instance_admin}]` | instance admin |
| `PATCH /admin/users/{id}` | yes | `{disabled?, instance_admin?}` → 204 | instance admin |
| `GET /healthz` | no | → `ok` | for Docker/uptime checks (not under `/api/v1`) |
| `GET /` and `GET /invite/{code}` | no | → HTML | landing and invite pages, see [9.6](#96-addresses-invite-links-and-the-invite-web-page) (not under `/api/v1`) |

---

## 11. Gateway (real-time WebSocket) protocol

Endpoint: `wss://host:port/api/v1/gateway`. Text frames, JSON, one message per frame. `protocol_version = 1`.

### 11.1 Envelope
```json
{ "op": "message_create", "s": 1042, "d": { ... } }
```
- `op`: snake_case name.
- `s`: sequence number for **dispatch events** (server → client data events), `null` for control ops.
- `d`: payload.

### 11.2 Connection lifecycle
1. Client connects. Server sends `hello { heartbeat_interval_ms: 30000 }`.
2. Client sends **either** `identify { token, protocol_version: 1, client: { name: "hermes-gui", version, os } }` **or** `resume { token, session_id, last_seq }`.
3. On identify the server replies `ready` (full snapshot, below). On resume it replays every missed event in order and then sends `resumed {}`. If the resume buffer no longer covers `last_seq`, server sends `invalid_session { resumable: false }` and the client must identify again (and treat its state as stale: replace it with the new `ready`).
4. Client sends `heartbeat { last_seq }` every interval (with ±10% jitter). Server answers `heartbeat_ack {}`. Missing two intervals → server closes with 4009; missing an ack for two intervals → client reconnects.
5. Reconnect backoff in the client: 1 s, 2 s, 4 s, 8 s, … capped at 30 s, with jitter. Always try `resume` first if a `session_id` exists.

Resume buffer per session: last 2000 events or 5 minutes, whichever is smaller. The session object is kept for 5 minutes after disconnect.

### 11.3 `ready` payload
```json
{
  "session_id": "…",
  "user": { …User, "instance_admin": false },
  "spaces": [ { "space": {…}, "roles": [...], "channels": [...], "members": [...], "voice_states": [...] } ],
  "dms": [ …Channel ],
  "read_states": [ …ReadState ],
  "presences": [ …Presence ],              // users sharing a space or DM with you
  "notification_settings": [ … ],
  "voice": { "enabled": true, "quic_port": 7700, "cert_sha256": "3F:A9:…" },
  "instance": { "name": "…", "limits": { … } }
}
```
Channels the user cannot `VIEW_CHANNEL` are omitted. Members are included in full (fine at target scale; capped at 1000 per space, larger spaces lazy-load via REST).

### 11.4 Client → server ops
| op | payload | notes |
|---|---|---|
| `identify` | see above | |
| `resume` | `{token, session_id, last_seq}` | |
| `heartbeat` | `{last_seq}` | |
| `presence_update` | `{status: "online"|"idle"|"dnd"|"invisible"}` | client sets `idle` after 10 min without input |

All other actions use REST.

### 11.5 Server → client dispatch events (all carry `s`)
| op | payload |
|---|---|
| `message_create` | `Message` (`nonce` included only for the author's sessions) |
| `message_update` | `Message` |
| `message_delete` | `{id, channel_id}` |
| `message_delete_bulk` | `{ids, channel_id}` (ban with message deletion) |
| `reaction_add` / `reaction_remove` | `{message_id, channel_id, emoji, user_id}` |
| `typing_start` | `{channel_id, user_id, timestamp}` (client shows for 10 s) |
| `read_state_update` | `ReadState` (own sessions only) |
| `channel_create` / `channel_update` | `Channel` |
| `channel_delete` | `{id, space_id}` |
| `space_create` | full space object as in `ready` (you joined or created a space) |
| `space_update` | `Space` |
| `space_delete` | `{id}` (space deleted, or you left/were kicked/banned) |
| `role_create` / `role_update` | `Role` |
| `role_delete` | `{id, space_id}` |
| `member_add` / `member_update` | `Member` |
| `member_remove` | `{space_id, user_id}` |
| `user_update` | `User` (profile changes, sent to everyone who can see the user) |
| `presence_update` | `Presence` |
| `voice_state_update` | `VoiceState` or `{user_id, channel_id: null}` (left) |
| `call_ring` | `{channel_id, caller_id}` |
| `call_ring_stop` | `{channel_id}` |
| `dm_create` | `Channel` |
| `dm_update` | `Channel` (recipients changed, rename) |
| `dm_delete` | `{id}` (you left or were removed) |
| `notification_settings_update` | one setting (own sessions) |

Permission changes (role/override/member role updates) can make channels appear or disappear for a user; the server then sends `channel_create`/`channel_delete` to that user's sessions accordingly. Implementation: after any permission-affecting change, recompute visible channels for affected members and diff against the previous set.

### 11.6 Server → client control ops (no `s`)
`hello`, `heartbeat_ack`, `ready`, `resumed`, `invalid_session {resumable}`.

### 11.7 Close codes
| code | meaning | client behavior |
|---|---|---|
| 4000 | unknown error | reconnect |
| 4001 | unknown op | reconnect (bug) |
| 4002 | decode error | reconnect (bug) |
| 4003 | not authenticated (sent op before identify) | reconnect |
| 4004 | authentication failed | **do not reconnect**; show login |
| 4005 | already authenticated | reconnect |
| 4007 | invalid seq | identify fresh |
| 4008 | rate limited | reconnect after 5 s |
| 4009 | session timed out / queue overflow | resume |
| 4010 | unsupported protocol version | **do not reconnect**; tell user to update |
| 4011 | session revoked (logged out elsewhere) | **do not reconnect**; show login |
| 4100 | server restarting | reconnect with backoff |

---

## 12. Voice system

### 12.1 Why this design
- **QUIC (quinn) on one UDP port**: built-in TLS 1.3 encryption, connection keep-alive and migration (Wi-Fi ↔ wired without dropping), *unreliable datagrams* for audio (no head-of-line blocking) and *reliable streams* for control, all on one socket. Clients always connect *out* to the server, so NAT is not a problem for clients; only the server needs a reachable port.
- **No WebRTC**: avoids ICE/STUN/TURN/DTLS-SRTP complexity and a separate media server (like LiveKit) that would break the single-binary deployment. The trade-off (no browser client for voice without extra work) is acceptable since v1 is desktop-only.
- **SFU, no server mixing**: tiny CPU cost, per-user volume control on the client, speaking indicators for free.
- **Transport encryption only** (client ↔ server). The server operator can technically access audio. E2E is a post-v1 item.

### 12.2 Join flow
```
Client                                  Server (REST)                 Server (QUIC :7700/udp)
  │ POST /voice/join {channel_id} ───────►│ check CONNECT, limits
  │◄── {token, quic:{host,port,cert_sha256}}│ token valid 30 s, single-use
  │                                                                       │
  │ QUIC connect (ALPN "hermes-voice/1"), verify cert == cert_sha256 ────►│
  │ open bi-stream "control": auth {token, self_mute, self_deaf} ────────►│ validate token
  │◄────────────── ready {participant_id, room_id, bitrate, participants} │ VoiceState created,
  │                                                                       │ gateway voice_state_update
  │ ◄════════ datagrams (Opus) ════════►                                  │
```
- The QUIC connection is per instance and reused: switching voice channels sends `switch {token}` on the existing control stream instead of reconnecting.
- If the client is already in a room on that instance (e.g., another device of the same user), the old room membership is ended (`kicked {reason: "joined_elsewhere"}`).
- On QUIC connection loss the client automatically retries: fresh `/voice/join` for the same channel, up to 5 attempts with backoff, showing "Reconnecting…" in the UI.
- QUIC idle timeout 15 s, keep-alive every 5 s.

### 12.3 Control stream messages
Framing: `u32` big-endian length prefix + JSON body `{ "op": "...", "d": {...} }`.

Client → server: `auth {token, self_mute, self_deaf}`, `switch {token}`, `set_state {self_mute, self_deaf}`, `ping {t}`, `leave {}`.

Server → client: `ready {participant_id: u16, room_id, bitrate, participants: [{participant_id, user_id}]}`, `participant_joined {participant_id, user_id}`, `participant_left {participant_id}`, `server_state {server_mute, server_deaf}`, `bitrate_changed {bitrate}`, `moved {token}` (client must `switch` with this token), `kicked {reason}`, `pong {t}`, `error {code, message}`.

`participant_id` is a small per-room integer assigned by the server so packets only need 2 bytes to identify the sender.

### 12.4 Audio datagram format
All integers big-endian. 11-byte header + Opus payload.

```
offset size field
0      1    version_and_type   high nibble = version (1), low nibble = type (1 = audio)
1      1    flags              bit0 = voice_active (VAD/PTT open), bits1-7 reserved (0)
2      2    sender_id          participant_id (client sets its own; SERVER OVERWRITES it, preventing spoofing)
4      4    timestamp          in 48 kHz samples, starts random, +960 per 20 ms frame
8      2    sequence           +1 per packet, wraps
10     1    audio_level        0 = loud … 127 = silent (dBov, like RFC 6464)
11     …    opus_payload       one Opus frame (20 ms)
```

**Server forwarding rule**: on a datagram from participant P in room R: drop if P is server-muted, self-muted, lacks `SPEAK`, or the packet is malformed/too large (> 1000 bytes); otherwise overwrite `sender_id` and `send_datagram` a cheap clone (`bytes::Bytes`) to every other participant in R that is not deafened (self or server). If a send fails because the peer's buffer is full, drop the packet (never block).

### 12.5 Client audio pipeline (`hermes-audio`)
**Capture** (cpal input callback → lock-free ring buffer → capture worker thread):
1. Convert to f32 mono (average channels).
2. Resample device rate → 48 kHz (`rubato`), skip if already 48 kHz.
3. Noise suppression (`nnnoiseless`, 480-sample frames, toggleable; on by default). Also yields a voice probability used by VAD.
4. Simple AGC (target −18 dBFS, slow attack/release, toggleable; off by default).
5. Gate: **voice activity** (RNNoise probability > threshold *or* RMS above user-set sensitivity, with 300 ms hangover) or **push-to-talk** (gate open while key held, plus 150 ms release delay). Self-mute closes the gate.
6. Opus encode: 48 kHz, mono, 20 ms frames (960 samples), `application = VOIP`, bitrate = room bitrate, in-band FEC on, expected packet loss 10%, DTX on. When the gate is closed: send nothing (after a final frame).
7. Build datagram and send through `hermes-client-core`'s QUIC connection.

**Playback** (per remote participant):
1. **Jitter buffer**: ordered by sequence; adaptive target delay 40–200 ms based on observed jitter (start at 60 ms); handles reordering and drops late packets.
2. Opus decode at playout time; on a missing packet use FEC from the next packet if present, else Opus packet-loss concealment (decode with `None`) for up to 5 frames, then silence.
3. Per-user volume (0–200%) and local mute.
4. Mix all participants (sum, then soft-clip with `tanh`), add UI sounds (join/leave/mute/ping), apply output volume.
5. Resample 48 kHz → device rate; write to a ring buffer consumed by the cpal output callback.

**Speaking indicator**: a participant is "speaking" while packets with `voice_active = 1` arrived in the last 250 ms. Your own indicator comes from the local gate.

**Rules for audio code**: no allocation, locking or logging inside cpal callbacks; callbacks only move samples between ring buffers. Device changes (unplugging a headset) are detected from cpal stream errors → rebuild the stream on the default device and show a toast.

**Settings** exposed to the UI: input device, output device, input volume, output volume, input mode (`voice_activity` | `push_to_talk`), PTT key, PTT release delay, VAD sensitivity, noise suppression on/off, AGC on/off, a live input level meter, and a "mic test" (loop back your processed audio to your output).

**Echo cancellation**: not in v1. Headphones are recommended in the UI the first time a user joins voice with noise suppression off.

### 12.6 DM calls
- Each DM/group DM has an implicit voice room keyed by the DM channel ID.
- "Start call" = `POST /voice/join` for the DM channel, then `POST /channels/{id}/call/ring`. Recipients get `call_ring` → GUI shows an incoming call banner with Join / Dismiss, plays a ring sound for up to 30 s. Ringing stops when they join, dismiss, or the caller leaves (`call_ring_stop`).
- A system message `"<user> started a call"` is posted to the DM; when the room empties, a system message `"Call ended · 12 min"` is posted.

---

## 13. Security model

- **Passwords**: argon2id (default params of the `argon2` crate, ~19 MiB memory), hashed in `spawn_blocking`.
- **Tokens**: 32 random bytes, base64url, prefixed `hms_` for recognizability. Only the SHA-256 is stored. Sent as `Authorization: Bearer`. Never logged.
- **Client token storage**: OS credential store via `keyring` (Windows Credential Manager); fallback to a file with user-only permissions if unavailable.
- **TLS**:
  - `self_signed` (default): clients use **certificate pinning**. Invite links include the fingerprint (`#fp=sha256:…`), so a friend who clicks an invite link never sees a scary warning, and a MITM is still detected. If a user adds an instance without a fingerprint, the client shows the fingerprint and asks to trust it (trust-on-first-use, stored in `known_hosts.toml`). If a pinned fingerprint ever changes, the client refuses to connect and shows a clear warning with an explicit "I re-generated the certificate, trust the new one" action.
  - `files`: normal CA validation (e.g., Let's Encrypt certs from certbot); pinning not needed.
  - `off`: only behind a TLS-terminating reverse proxy (Caddy/nginx); the client still sees HTTPS.
  - Voice QUIC: always pinned to `cert_sha256` received over the already-authenticated HTTPS/WSS channel.
- **Rate limits**: see `[limits]`. Login attempts limited per IP. Message sends per user per channel. Gateway: max 120 client ops per minute.
- **Input validation**: all lengths enforced server-side (see schema comments); usernames `^[a-z0-9_.]{2,32}$`; reject control characters except `\n` and `\t` in messages.
- **Uploads**: stored under generated names (never the user's filename on disk), served with `X-Content-Type-Options: nosniff`, non-image types forced to download; images re-encoded for thumbnails only (originals untouched). Optional global storage quota.
- **Authorization**: every REST handler checks permissions through one function `require(perm, space?, channel?)`; integration tests cover each permission bit.
- **Privacy**: no telemetry, no external requests from the server in v1 (link previews are post-v1 and will be opt-in).
- **Audit log** for moderation actions.

---

## 14. Client architecture (shared core)

`hermes-client-core` is a UI-agnostic library used by both clients.

### 14.1 Responsibilities
- Manage **multiple instances** (connections), each with: REST client, gateway connection (with resume/backoff), optional voice QUIC connection.
- Keep a normalized **Store** of everything received: instances → spaces → channels/roles/members, DMs, users, presences, voice states, read states, notification settings, per-channel message windows.
- Expose an **async command API** (send message, edit, react, join voice, …) and a single **event stream** that UIs subscribe to.
- Own the **audio engine** (`hermes-audio`) and connect it to the voice transport.
- Load/save **client config** and the **known hosts** file; store tokens in the keyring.
- Provide **pure view helpers** used by both UIs: sorted channel tree with unread/mention flags, member list grouped by hoisted roles, message grouping (same author within 7 minutes → grouped), markup parsing, permission checks for greying out UI actions.

### 14.2 API shape
```rust
pub struct Core { /* Arc internals */ }

impl Core {
    pub fn new(config: ClientConfig) -> (Core, CoreEvents);           // CoreEvents: async Stream<Item = CoreEvent>
    pub fn store(&self) -> StoreSnapshot;                               // cheap read access (Arc + RwLock read guard or im-rc style snapshot)

    // instances
    pub async fn probe_instance(&self, url: &InstanceUrl) -> Result<InstanceInfo>;   // GET /instance + cert fingerprint
    pub async fn trust_fingerprint(&self, host: &HostPort, fp: Fingerprint);
    pub async fn login(&self, url: &InstanceUrl, username: &str, password: &str) -> Result<InstanceId>;
    pub async fn register(&self, url: &InstanceUrl, username: &str, password: &str, invite: Option<&str>) -> Result<InstanceId>;
    pub async fn logout(&self, inst: InstanceId);

    // messaging (optimistic)
    pub fn send_message(&self, inst: InstanceId, channel: Id, draft: Draft) -> LocalMessageId;
    pub async fn edit_message(...); pub async fn delete_message(...);
    pub async fn react(...); pub async fn unreact(...);
    pub async fn load_history(&self, inst, channel, anchor: HistoryAnchor) -> Result<()>;
    pub fn mark_read(&self, inst, channel, message: Id);                // debounced 1 s
    pub fn typing(&self, inst, channel);                                 // throttled 8 s
    pub async fn upload(&self, inst, path_or_bytes) -> Result<Attachment>;

    // voice
    pub async fn join_voice(&self, inst, channel: Id) -> Result<()>;
    pub async fn leave_voice(&self);
    pub fn set_self_mute(&self, on: bool); pub fn set_self_deaf(&self, on: bool);
    pub fn set_ptt(&self, pressed: bool);
    pub fn set_user_volume(&self, inst, user: Id, volume: f32);
    pub fn audio_settings(&self) -> AudioSettings; pub fn set_audio_settings(&self, s: AudioSettings);
    pub fn list_audio_devices(&self) -> AudioDevices;

    // everything else in the REST API gets a 1:1 async method (spaces, roles, channels, invites, moderation, …)
}

pub enum CoreEvent {
    InstanceStatus { inst: InstanceId, status: ConnStatus },          // Connecting, Connected, Reconnecting{attempt}, Failed{reason}, LoggedOut
    StoreChanged { inst: InstanceId, what: Changed },                 // fine-grained: Channel(id), Messages(channel), Members(space), VoiceStates(channel), …
    Notification(Notification),                                       // mention / DM → UI decides toast + sound
    IncomingCall { inst, channel: Id, caller: Id },
    VoiceStatus(VoiceStatus),                                         // Disconnected, Connecting, Connected{ping_ms, loss_pct}, Reconnecting
    Speaking { inst, user: Id, speaking: bool },
    InputLevel(f32),                                                  // 20 Hz, only while settings/voice panel needs it
    Error(UserFacingError),
}
```

### 14.3 Optimistic messages
1. `send_message` immediately inserts a `PendingMessage { local_id, nonce = uuid v4, content, attachments, state: Sending }` at the bottom of the channel window and emits `StoreChanged`.
2. POST in the background. On success the HTTP response *or* the `message_create` with the same `nonce` (whichever comes first) replaces the pending message with the real one, in place.
3. On failure: `state: Failed(reason)` → UI shows the message dimmed in red with "Retry" / "Delete".
4. Messages typed while disconnected queue up (max 50) and are sent in order on reconnect.

### 14.4 Message windows
Each channel keeps a contiguous window of up to 500 messages in memory. Opening a channel loads the latest 50; scrolling up loads 50 more (`before`); jumping to a message loads `around`. Windows of channels not viewed for 10 minutes are trimmed to the last 50. No persistent message cache on disk in v1 (the server is the source of truth; startup cost is one `ready` + 50 messages per open channel).

### 14.5 Notifications logic (shared)
A message triggers a notification if: not authored by me, channel not currently focused in a visible pane (or app unfocused), user status is not `dnd`, and notification level allows it (`all`, or `mentions` and the message mentions me/my role/@everyone where permitted). DMs default to `all`, spaces default to `mentions`. The core emits `Notification`; each UI decides how to present it.

---

## 15. iced GUI client specification

Binary: `hermes` (crate `hermes-gui`). Target look and feel: **Halloy's clean, dense, keyboard-friendly layout**, plus the minimum Discord-like elements needed for spaces and voice.

### 15.1 Window layout
```
┌──────────────────────┬────────────────────────────────────────────────────────────────┐
│ SIDEBAR (resizable,  │ PANE GRID (iced pane_grid, any number of splits, drag to move)  │
│ left or right)       │ ┌───────────────────────────────┬─────────────────────────────┐ │
│                      │ │ #general · topic text   [⋮]   │ #memes                [⋮]   │ │
│ ▾ hermes.senko.tools │ │───────────────────────────────│─────────────────────────────│ │
│   ▾ Friends          │ │ 18:02 Alice                    │ ...                         │ │
│     ▾ TEXT           │ │       hello!                   │                             │ │
│       # general  3   │ │       second line (grouped)    │                             │ │
│       # memes   @1   │ │ 18:05 Bob  ↪ replying to Alice │                             │ │
│     ▾ VOICE          │ │       hi  [😀 2] [👍 1]          │                             │ │
│       🔊 Lounge      │ │ ...                            │                             │ │
│          ● Alice     │ │ Bob is typing…                 │                             │ │
│          ○ Bob 🔇    │ │ ┌─────────────────────────────┐│                             │ │
│       🔊 Gaming      │ │ │ Message #general            ││                             │ │
│   ▸ Study Group      │ │ └─────────────────────────────┘│                             │ │
│ ▾ Direct messages    │ └───────────────────────────────┴─────────────────────────────┘ │
│     Carol        1   │                                                                  │
│     Alice, Bob       │                                                                  │
│ ▸ other.host:7700    │                                                                  │
│──────────────────────│                                                                  │
│ VOICE PANEL          │                                                                  │
│ 🟢 Lounge · Friends  │                                                                  │
│ 38 ms  [🎤][🎧][⏏]   │                                                                  │
│──────────────────────│                                                                  │
│ USER PANEL           │                                                                  │
│ (avatar) you  ● [⚙]  │                                                                  │
└──────────────────────┴────────────────────────────────────────────────────────────────┘
```
(Icons above are only illustrative; the client uses an icon font or SVG icons, not emoji, for UI chrome.)

**Sidebar** (one scrollable tree, like Halloy's server list):
- Level 1: instances (with connection status dot; right-click: reconnect, log out, instance settings, copy invite).
- Level 2: spaces (collapsible; unread dot / mention count when collapsed; right-click: space settings, invite people, notification level, mute, leave).
- Level 3: categories (collapsible), channels sorted by position. Text channels: `#` + name, bold if unread, mention badge. Voice channels: speaker icon + name; **connected users listed underneath at all times** (visible without joining), with speaking ring, mute/deaf icons. Clicking a text channel opens it in the focused pane. **Single-clicking a voice channel only selects it and opens its text chat; joining requires double-click, or the "Join voice" button in its pane header** (configurable: `voice.join_on = "double_click" | "single_click"`, default `double_click`). This directly avoids accidental voice joins.
- "Direct messages" section per instance: DMs sorted by last activity, with unread badges and a "+" to start a DM/group DM.
- Collapsed state persisted per node.

**Voice panel** (bottom of sidebar, only while in voice): channel and space name (click = open that channel), connection quality (ping, loss %), buttons: mute, deafen, disconnect, and audio settings. Push-to-talk indicator when PTT is active.

**User panel**: avatar, display name, status selector (online/idle/dnd/invisible), settings button.

**Pane grid**: each pane shows one **buffer**. Buffer types:
- `Channel` (text channel, voice channel's text chat, DM, group DM): header (name, topic, member-list toggle, pins, search, Join-voice button for voice/DM), message list, typing line, input box. Optional member list docked on the right side of the pane (toggle per pane, remembered).
- `VoiceRoom`: grid/list of participants with avatars, speaking rings, per-user volume sliders on right-click.
- `Mentions`: recent mentions across all instances.
- `Search`: results with jump-to.
- `Settings`: tabs (Account, Appearance, Voice & Audio, Notifications, Keybinds, Instances, About).
- `SpaceSettings`: tabs (Overview, Roles, Channels & permissions, Members, Invites, Bans, Audit log).
- `Logs`: client log output (for debugging, like Halloy's logs buffer).

Pane actions (header `⋮` menu and keybinds): split horizontal, split vertical, close, maximize/restore, swap. Layout (pane tree + which buffer in each) is saved on exit and restored on start.

**Command bar** (Ctrl+K): fuzzy search over channels, DMs, spaces, users and actions ("Toggle mute", "Join voice: Lounge", "Open settings", "Split pane right", "Theme: …"). Same concept as Halloy's command bar.

### 15.2 Message list
- Virtualized list (only visible rows built) to stay fast in long histories.
- Grouping: consecutive messages of the same author within 7 minutes show author + timestamp once; hovered grouped lines show their time on the left.
- Per message: author name in their highest colored role's color, optional avatar (setting `buffer.show_avatars`, default true), timestamp (format configurable, default `%H:%M`), "(edited)" marker, reply preview line above (click = jump), attachments (images inline scaled to max 400×300 using the server thumbnail, click = full-size viewer overlay; other files as a card with name/size/download), reaction chips (click toggles your reaction; hover shows who), system messages styled dim/italic.
- Mentions of you: highlighted row background. Spoilers: blurred block until clicked. Code blocks: monospace with background (syntax highlighting post-v1).
- Hover toolbar on a message: react, reply, edit (own), more (copy text, copy link, pin, delete, copy ID).
- "New messages" divider at the first unread message; "Jump to present" button when scrolled up; mark-as-read when the bottom is visible and the window is focused.
- Infinite scroll up triggers `load_history`.
- Failed/pending messages styled as described in [14.3](#143-optimistic-messages).

### 15.3 Input box
- iced `text_editor` (multi-line). Enter sends, Shift+Enter newline (configurable).
- Autocomplete popup: `@` users/roles, `#` channels, `:` emoji shortcodes, `/` slash commands. Tab/Enter accepts, arrow keys navigate.
- Up arrow in an empty box edits your last message.
- Reply mode bar ("Replying to Alice ✕"), edit mode bar.
- Attachments: "+" button (file dialog via `rfd`), drag-and-drop files onto the pane, paste images from the clipboard (`arboard`). Shows upload progress chips before sending.
- Slash commands (client-side, also usable from the TUI): `/me`, `/shrug`, `/join <voice channel>`, `/leave`, `/mute`, `/deafen`, `/nick <name>`, `/dm <user>`, `/invite`, `/status <online|idle|dnd|invisible>`.
- Draft text per channel is kept when switching channels.

### 15.4 Dialogs and flows
- **First run / Add instance**: "Paste an invite link or address" (accepts `hermes.senko.tools`, `host:port`, `https://…/invite/…` and `hermes://…` forms, see [9.6](#96-addresses-invite-links-and-the-invite-web-page)) → probe → (fingerprint prompt if needed) → instance preview (name, space from invite) → Login / Create account tabs → connected. Invite links can also be opened from the OS: the installer registers the `hermes://` URL scheme on Windows, so clicking an invite link in a browser opens the client.
- **Create space**, **Create channel** (kind, name, category, private toggle that creates an `@everyone` deny-VIEW override and lets you pick roles), **Invite people** (create link, copy).
- **Space settings → Roles**: list ordered by position (drag to reorder), editor with name, color picker, hoist, mentionable, permission checkboxes grouped by category with descriptions. **Channel permissions**: per-role tri-state toggles (allow / inherit / deny).
- **Members**: list with search, role assignment, kick, ban (with message deletion window), server mute/deafen/move for voice.
- **Incoming call** banner at top of the window with Join / Dismiss.
- **Confirmations** for destructive actions (delete channel/space/role, ban).
- **Fingerprint changed** warning dialog.

### 15.5 Theming and fonts (Halloy-style)
- Themes are TOML files in `<config_dir>/themes/`, hot-reloaded. Built-in: `dark` (default), `light`, `high-contrast`.
- Theme keys: `general.{background, border, horizontal_rule, unread_indicator, scrollbar}`, `text.{primary, secondary, tertiary, success, error, warning, info, link, mention_bg, code_bg}`, `buttons.{primary, secondary, danger}.{background, background_hover, text}`, `sidebar.{background, selected, hover}`, `pane.{header_bg, focused_border}`, `voice.{speaking_ring, muted}`, `roles.default_color`.
- Fonts: `font.family` (default: bundled Inter for UI; bundled JetBrains Mono for code), `font.size` (default 13), `font.mono_family`. Color emoji: rely on the system emoji font (Segoe UI Emoji on Windows); verify rendering in the Phase 0 spike.
- UI scale factor setting (0.75–2.0) on top of OS DPI.

### 15.6 Default keybinds (all rebindable in config)
| Action | Default |
|---|---|
| Command bar | `Ctrl+K` |
| Next / previous channel with unread | `Alt+Shift+Down` / `Alt+Shift+Up` |
| Next / previous channel in sidebar | `Alt+Down` / `Alt+Up` |
| Focus next / previous pane | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Split pane right / down | `Ctrl+\` / `Ctrl+Shift+\` |
| Close pane | `Ctrl+W` |
| Maximize pane | `Ctrl+Shift+F` |
| Toggle member list | `Ctrl+U` |
| Toggle sidebar | `Ctrl+B` |
| Search | `Ctrl+F` |
| Mark channel read | `Escape` (in message list) |
| Toggle mute | `Ctrl+Shift+M` |
| Toggle deafen | `Ctrl+Shift+D` |
| Push-to-talk | unbound by default (global hotkey; user picks e.g. `Mouse4` or `CapsLock`) |
| Toggle mute (global) | unbound by default |
| Settings | `Ctrl+,` |

### 15.7 Desktop integration
- Notifications via `notify-rust` (toast with author and message preview; click focuses the window and opens the channel).
- Tray icon (`tray-icon`): show/hide, mute, deafen, quit. Closing the window minimizes to tray (setting, default on). Tray icon shows a badge dot on unread mentions.
- Taskbar flash on DM/mention when unfocused (winit `request_user_attention`).
- Single instance: a second launch (e.g., from a `hermes://` link) forwards its argument to the running instance via a local named pipe and exits.
- Start with Windows (setting, off by default).
- Update check: on start, query the GitHub releases API for the client repo; if newer, show a subtle indicator in the user panel with a link (no auto-install in v1). Can be disabled.

### 15.8 iced structure
```
hermes-gui/src/
├── main.rs              # iced::application(...) with title, update, view, subscription, theme
├── app.rs               # App state: Core handle, layout (pane_grid::State<Pane>), sidebar state, modals, settings
├── message.rs           # enum Message { Core(CoreEvent), Sidebar(..), Pane(PaneId, buffer::Message), Modal(..), Key(..), Tick, ... }
├── subscription.rs      # CoreEvents → Message::Core; keyboard; window events (file drop, focus); global hotkeys; timers
├── sidebar/             # tree view, voice panel, user panel
├── buffer/              # channel.rs, voice_room.rs, settings/, space_settings/, search.rs, mentions.rs, logs.rs
├── widget/              # message_row, reaction_chip, avatar, badge, input (text_editor wrapper + autocomplete), context_menu, color_picker, tooltip
├── modal/               # add_instance, create_space, create_channel, invite, confirm, image_viewer, incoming_call, fingerprint
├── theme/               # Theme struct, TOML loading, iced style functions for each widget
├── keybinds.rs
└── assets/              # fonts, icon font/SVGs, sounds (join.ogg, leave.ogg, mute.ogg, unmute.ogg, message.ogg, ring.ogg)
```
Follow the Elm architecture strictly: all side effects go through `Task`s calling `Core` methods; `view` is pure and reads from the store snapshot.

---

## 16. TUI client specification

Binary: `hermes-tui`. Purpose: the author's personal client and a fast development tool. It shares `hermes-client-core` and config with the GUI, so features come almost for free; only rendering and input differ.

### 16.1 Layout
```
┌ Sidebar ─────────────┐┌ #general — Friends @ hermes.senko.tools ───────────┐┌ Members ──────┐
│ hermes.senko.tools   ││ 18:02 alice  hello!                                 ││ Admin         │
│  Friends             ││ 18:02        second line                            ││  alice ●      │
│   # general       3  ││ 18:05 bob    ↪ alice: hello!                        ││ Online        │
│   # memes        @1  ││              hi   [😀 2]                            ││  bob ●        │
│   ◂) Lounge          ││                                                     ││ Offline       │
│       alice *        ││                                                     ││  carol        │
│       bob   m        ││ bob is typing…                                      ││               │
│  DMs                 │├─────────────────────────────────────────────────────┤│               │
│   carol           1  ││ > message #general_                                 ││               │
└──────────────────────┘└─────────────────────────────────────────────────────┘└───────────────┘
 NORMAL │ voice: Lounge 38ms │ m:mute d:deafen │ :help
```
`*` = speaking, `m` = muted, `d` = deafened. Member list toggles with `u`. Status line at the bottom shows mode, voice status and hints.

### 16.2 Input model (vim-like modes)
- **Normal mode**: `j/k` move in sidebar or message list (depending on focus), `h/l` or `Tab` switch focus (sidebar ↔ messages ↔ members), `Enter` open channel / join voice (on a voice channel, `Enter` opens its chat, `J` joins voice — same "no accidental join" principle), `i` insert mode, `r` reply to selected message, `e` edit own, `x` delete own (with confirm), `+` react (opens emoji search), `gg/G` top/bottom, `Ctrl+u/d` page, `m` toggle mute, `D` toggle deafen, `L` leave voice, `u` toggle member list, `/` search, `:` command mode, `q` quit (confirm while in voice).
- **Insert mode**: type message, `Enter` send, `Alt+Enter` newline, `Tab` autocomplete (`@`, `#`, `:`), `Esc` back to normal, `Up` on empty input edits last message.
- **Command mode** `:`: `:join <channel>`, `:dm <user>`, `:voice <channel>`, `:leave`, `:mute`, `:deafen`, `:status <s>`, `:upload <path>`, `:open <attachment#>` (opens in default app/browser), `:instance add <invite-or-url>`, `:login`, `:set <key> <value>`, `:quit`. Slash commands from the GUI also work in insert mode.
- **Mouse** (crossterm mouse capture): click to select channel/message, scroll wheel scrolls; nice-to-have, not required.

### 16.3 Limitations (by design)
- **No push-to-talk**: terminals do not report key release events (except some terminals with the kitty keyboard protocol). The TUI supports voice activity and toggle mute. If the kitty protocol is available (crossterm `PushKeyboardEnhancementFlags` with `REPORT_EVENT_TYPES`), PTT can be enabled as an optional feature later.
- Images are shown as `[image: cat.png 1200×800]` with `:open`; inline terminal images (via `ratatui-image`) are a post-v1 nice-to-have.
- Settings UI is minimal: most settings are edited in `config.toml` (hot-reloaded) or via `:set`.

### 16.4 Structure
```
hermes-tui/src/
├── main.rs          # terminal setup/restore (also on panic), tokio runtime
├── app.rs           # state, mode, focus, selected items
├── event.rs         # select! over crossterm EventStream, CoreEvents, tick (250 ms)
├── ui/              # sidebar.rs, messages.rs, members.rs, input.rs, statusline.rs, popup.rs (autocomplete/emoji/confirm)
├── keymap.rs        # configurable keymap from [tui.keys]
└── commands.rs      # ':' command parser (shared slash-command logic lives in client-core)
```

---

## 17. Client configuration

Location: `%APPDATA%\Hermes\config.toml` on Windows, `~/.config/hermes/config.toml` on Linux, `~/Library/Application Support/Hermes/config.toml` on macOS. **Portable mode**: if `config.toml` exists next to the executable, that directory is used instead (like Halloy). Hot-reloaded on change. Unknown keys produce a warning, not an error. The GUI settings screens write to this same file (preserving comments with `toml_edit`).

```toml
[general]
theme = "dark"
start_minimized = false
close_to_tray = true
check_for_updates = true

[font]
family = "Inter"
mono_family = "JetBrains Mono"
size = 13
ui_scale = 1.0

[sidebar]
position = "left"            # "left" | "right"
width = 240
show_voice_users = true

[buffer]
show_avatars = true
timestamp_format = "%H:%M"
group_minutes = 7
enter_sends = true
image_previews = true
show_typing = true

[voice]
join_on = "double_click"     # "double_click" | "single_click"
input_device = "default"
output_device = "default"
input_volume = 1.0
output_volume = 1.0
mode = "voice_activity"      # "voice_activity" | "push_to_talk"
ptt_key = ""                 # e.g. "Mouse4", "CapsLock", "Ctrl+Space"
ptt_release_delay_ms = 150
vad_sensitivity = -50        # dBFS threshold
noise_suppression = true
auto_gain = false
sounds = true                # join/leave/mute sounds
sounds_volume = 0.6

[notifications]
enabled = true
sound = true
show_preview = true
flash_taskbar = true

[keys]                       # GUI keybinds, action = "key combo"
command_bar = "Ctrl+K"
toggle_mute = "Ctrl+Shift+M"
toggle_deafen = "Ctrl+Shift+D"
# … every action from section 15.6

[tui.keys]                   # TUI normal-mode keys, action = "key"
join_voice = "J"
toggle_mute = "m"
# … every action from section 16.2

[[instances]]                # managed by the client; tokens are NOT stored here (keyring)
id = "hermes.senko.tools"     # host, plus ":port" only when not 7700
url = "https://hermes.senko.tools:7700"
username = "alice"
autoconnect = true

[[layout]]                   # saved pane layout (managed by the GUI)
# serialized pane tree
```
Other files in the config dir: `known_hosts.toml` (pinned fingerprints), `themes/*.toml`, `user_volumes.toml` (per instance/user volumes), `collapsed.toml` (sidebar tree state), `logs/` (rolling client logs, 7 days).

---

## 18. Self-hosting and deployment

Goal: from zero to a running instance in **under 5 minutes** with any of these methods. All methods run the same binary.

### 18.1 Release artifacts (built by `.github/workflows/release.yml` on git tag `vX.Y.Z`)
All artifacts below are attached to one GitHub release (lockstep versioning, see [5.1](#51-repositories-and-versioning)).

- `hermes-server-x86_64-unknown-linux-musl.tar.gz`, `hermes-server-aarch64-unknown-linux-musl.tar.gz` (static, run anywhere), `hermes-server-x86_64-pc-windows-msvc.zip`.
- Docker image `ghcr.io/senko-tools/hermes-server:<version>` and `:latest` (multi-arch).
- `Hermes-Setup-<version>.exe` (NSIS installer, registers `hermes://` scheme) and `Hermes-<version>-portable.zip` for the GUI; `hermes-tui` in the same zip and as standalone archives for Windows/Linux/macOS.
- SHA-256 checksums file.

Server and client versions are independent but both report `protocol_version`; a client refuses (with a clear message) instances whose protocol version it does not support.

### 18.2 Requirements for any host
- A machine with a public IP (VPS, home server with port forwarding, game panel).
- **One port open for both TCP and UDP** (default 7700).
- ~100 MB RAM, a little disk for the database and uploads.
- Set `instance.public_host` to the domain/IP friends will use.

### 18.3 Method A: Docker Compose
`deploy/docker/compose.yaml`:
```yaml
services:
  hermes:
    image: ghcr.io/senko-tools/hermes-server:latest
    restart: unless-stopped
    ports:
      - "7700:7700/tcp"
      - "7700:7700/udp"
    environment:
      HERMES__INSTANCE__NAME: "My Friends"
      HERMES__INSTANCE__PUBLIC_HOST: "hermes.senko.tools"
      HERMES__STORAGE__DATA_DIR: "/data"
    volumes:
      - ./data:/data
```
Commands:
```sh
mkdir hermes && cd hermes
curl -LO https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/docker/compose.yaml
# edit PUBLIC_HOST and NAME
docker compose up -d
docker compose logs hermes      # shows the first-time setup invite link
```
Dockerfile: multi-stage, build with `cargo build --release --target x86_64-unknown-linux-musl`, final stage `gcr.io/distroless/static` (or `scratch` + CA certs), runs as non-root UID 10001, `HEALTHCHECK` hitting `/healthz`, `EXPOSE 7700/tcp 7700/udp`, `VOLUME /data`, entrypoint `["/hermes-server", "run"]`.

### 18.4 Method B: Linux install script + systemd
```sh
curl -fsSL https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/install.sh | sudo sh
sudo nano /etc/hermes/hermes.toml     # set public_host and name
sudo systemctl enable --now hermes
sudo journalctl -u hermes             # shows the setup invite link
```
Supported targets for the script: Debian 12/13, Ubuntu 22.04+, and other systemd distributions (the binary itself is static and distribution-independent). `install.sh`: detects arch, downloads the latest musl release, verifies checksum, installs to `/usr/local/bin/hermes-server`, creates system user `hermes`, `/etc/hermes/hermes.toml` (via `hermes-server init`, only if missing), data dir `/var/lib/hermes`, installs `hermes.service` (with `ProtectSystem=strict`, `ReadWritePaths=/var/lib/hermes`, `ReadOnlyPaths=/etc/hermes`, `NoNewPrivileges=yes`, `Restart=on-failure`, `ExecReload=/bin/kill -HUP $MAINPID`, and `AmbientCapabilities=CAP_NET_BIND_SERVICE` so the port can be set below 1024 if wanted). Re-running the script upgrades the binary and keeps config/data. Reminds the user to open TCP+UDP 7700 in the firewall (prints the `ufw` command).

### 18.5 Method C: Pterodactyl / Pelican egg
`deploy/pterodactyl/egg-hermes.json` (PTDL_v2 format; import in Admin → Nests → Import Egg). The installer downloads the release binary; configuration comes entirely from egg variables mapped to `HERMES__…` environment variables; the panel's allocated port is used for both TCP and UDP.

```json
{
  "_comment": "Hermes chat server egg",
  "meta": { "version": "PTDL_v2", "update_url": null },
  "exported_at": "2026-10-30T00:00:00+00:00",
  "name": "Hermes",
  "author": "you@example.com",
  "description": "Self-hosted chat with spaces, DMs and voice. Uses the allocated port for both TCP (HTTPS/WebSocket) and UDP (voice).",
  "features": null,
  "docker_images": { "Debian": "ghcr.io/parkervcp/yolks:debian" },
  "file_denylist": [],
  "startup": "./hermes-server run --config ./hermes.toml",
  "config": {
    "files": "{}",
    "startup": "{\"done\": \"Hermes is ready\"}",
    "logs": "{}",
    "stop": "^C"
  },
  "scripts": {
    "installation": {
      "script": "#!/bin/bash\nset -e\ncd /mnt/server\nARCH=$( [ \"$(uname -m)\" = \"aarch64\" ] && echo aarch64 || echo x86_64 )\nif [ \"${VERSION}\" = \"latest\" ] || [ -z \"${VERSION}\" ]; then\n  URL=\"https://github.com/senko-tools/hermes/releases/latest/download/hermes-server-${ARCH}-unknown-linux-musl.tar.gz\"\nelse\n  URL=\"https://github.com/senko-tools/hermes/releases/download/${VERSION}/hermes-server-${ARCH}-unknown-linux-musl.tar.gz\"\nfi\ncurl -fsSL \"$URL\" -o hermes.tar.gz\ntar -xzf hermes.tar.gz\nrm hermes.tar.gz\nchmod +x hermes-server\n[ -f hermes.toml ] || ./hermes-server init --config ./hermes.toml\necho 'Hermes installed.'",
      "container": "ghcr.io/parkervcp/installers:debian",
      "entrypoint": "bash"
    }
  },
  "variables": [
    { "name": "Version", "description": "Release tag to install (e.g. v1.0.0) or 'latest'. Reinstall the server to update.", "env_variable": "VERSION", "default_value": "latest", "user_viewable": true, "user_editable": true, "rules": "required|string|max:20", "field_type": "text" },
    { "name": "Instance name", "description": "Name shown in clients.", "env_variable": "HERMES__INSTANCE__NAME", "default_value": "Hermes", "user_viewable": true, "user_editable": true, "rules": "required|string|max:64", "field_type": "text" },
    { "name": "Public host", "description": "Domain or public IP that friends connect to (used in invite links).", "env_variable": "HERMES__INSTANCE__PUBLIC_HOST", "default_value": "", "user_viewable": true, "user_editable": true, "rules": "required|string|max:253", "field_type": "text" },
    { "name": "Registration mode", "description": "open, invite_only or closed.", "env_variable": "HERMES__REGISTRATION__MODE", "default_value": "invite_only", "user_viewable": true, "user_editable": true, "rules": "required|string|in:open,invite_only,closed", "field_type": "text" },
    { "name": "Max upload size (MB)", "description": "Maximum attachment size.", "env_variable": "HERMES__STORAGE__MAX_UPLOAD_MB", "default_value": "25", "user_viewable": true, "user_editable": true, "rules": "required|integer|between:1,1024", "field_type": "text" },
    { "name": "Bind address", "description": "Leave as 0.0.0.0.", "env_variable": "HERMES__NETWORK__BIND", "default_value": "0.0.0.0", "user_viewable": false, "user_editable": false, "rules": "required|string", "field_type": "text" }
  ]
}
```
The port: Pterodactyl injects `SERVER_PORT`; the server reads `SERVER_PORT` as a fallback for `network.port` **when `HERMES__NETWORK__PORT` is not set** (implement this explicitly in `config.rs`; document it). Pterodactyl allocations forward both TCP and UDP for the allocated port. Verify the docker image tags still exist at implementation time (the `parkervcp` yolks/installers images are the ones commonly used by community eggs).

### 18.6 Method D: behind a reverse proxy (optional)
For users who already run Caddy/nginx with a real domain certificate: set `tls.mode = "off"`, proxy HTTPS (including WebSocket upgrade on `/api/v1/gateway`) to `127.0.0.1:7700`, and **still expose UDP 7700 directly** for voice (reverse proxies do not forward QUIC voice). Example Caddy and nginx snippets go in `docs/self-hosting.md`.

### 18.7 Backups and upgrades
- Backup: `hermes-server backup /path/backup.db` (safe while running) + copy `data/files/`. For Docker: `docker compose exec hermes /hermes-server backup /data/backup.db`.
- Upgrades: replace the binary / pull the new image / reinstall the egg; migrations run automatically on start; the server refuses to start on a database created by a *newer* version (clear error message).

### 18.8 Reference deployment: hermes.senko.tools on Hetzner (Debian 13)
This is the official instance and the first real deployment target (used from Phase 1 onward). It uses Method B (systemd, no Docker: lighter and simplest on a dedicated VPS) with a real Let's Encrypt certificate, so no fingerprint pinning is involved for this instance.

**Decisions for this instance**
- Port **7700** TCP+UDP (default; leaves 80/443 free for anything else on the VPS, e.g. a future `senko.tools` website). Friends just type `hermes.senko.tools`.
- `tls.mode = "files"` with a Let's Encrypt certificate from `certbot`.
- `bind = "::"` (IPv4 + IPv6, since Hetzner VPSs have both).
- `registration.mode = "invite_only"`.

**1. DNS** (at the DNS provider of `senko.tools`):
- `A` record `hermes` → the VPS IPv4 address.
- `AAAA` record `hermes` → the VPS IPv6 address (Hetzner shows a /64; use e.g. `<prefix>::1`, which is the address configured on the VPS by default).

**2. Hetzner Cloud Firewall** (Cloud Console → Firewalls, apply to the server). Inbound rules:
| Protocol | Port | Purpose |
|---|---|---|
| TCP | 22 | SSH (ideally restricted to your own IP) |
| TCP | 80 | Let's Encrypt HTTP-01 challenge (certbot) |
| TCP | 7700 | Hermes HTTPS + WebSocket |
| UDP | 7700 | Hermes voice (QUIC) |
Allow ICMP too (useful for diagnostics). If a host firewall is also wanted on the VPS: `apt install ufw && ufw allow 22/tcp && ufw allow 80/tcp && ufw allow 7700/tcp && ufw allow 7700/udp && ufw enable`. (Hetzner's firewall alone is sufficient.)

**3. Base system**
```sh
apt update && apt full-upgrade -y
apt install -y curl ca-certificates certbot unattended-upgrades
dpkg-reconfigure -plow unattended-upgrades     # automatic security updates
```

**4. Certificate**
```sh
certbot certonly --standalone -d hermes.senko.tools --agree-tos -m <your-email> --non-interactive
```
If a web server later occupies port 80, switch to its plugin (`--nginx`, or let Caddy manage the cert) instead of `--standalone`.

Deploy hook so the `hermes` user can read the cert and the server reloads it after each renewal. Create `/etc/letsencrypt/renewal-hooks/deploy/hermes.sh` (mode 755):
```sh
#!/bin/sh
set -e
install -d -o root -g hermes -m 750 /etc/hermes/tls
install -o root -g hermes -m 640 /etc/letsencrypt/live/hermes.senko.tools/fullchain.pem /etc/hermes/tls/fullchain.pem
install -o root -g hermes -m 640 /etc/letsencrypt/live/hermes.senko.tools/privkey.pem   /etc/hermes/tls/privkey.pem
systemctl reload hermes 2>/dev/null || true
```
Run it once manually after step 5 has created the `hermes` user. Debian's certbot package installs a systemd timer that renews automatically.

**5. Install Hermes**
```sh
curl -fsSL https://raw.githubusercontent.com/senko-tools/hermes/main/deploy/install.sh | sh
/etc/letsencrypt/renewal-hooks/deploy/hermes.sh      # copy the cert for the hermes user
```

**6. Configure** `/etc/hermes/hermes.toml` (only the changed keys shown):
```toml
[instance]
name = "Hermes"
public_host = "hermes.senko.tools"

[network]
bind = "::"
port = 7700

[tls]
mode = "files"
cert_file = "/etc/hermes/tls/fullchain.pem"
key_file = "/etc/hermes/tls/privkey.pem"

[storage]
data_dir = "/var/lib/hermes"

[registration]
mode = "invite_only"
```

**7. Start and get the owner invite**
```sh
systemctl enable --now hermes
journalctl -u hermes -n 50      # shows: https://hermes.senko.tools:7700/invite/<code>
```
Open that link (or paste it into the client) and create the owner account.

**8. Verify**
- `curl https://hermes.senko.tools:7700/healthz` → `ok` (from your own PC).
- Join a voice channel from two clients on different networks (e.g., PC + phone hotspot) to confirm UDP 7700 is open.

**9. Backups** (simple, nightly, keep 14): `/etc/cron.daily/hermes-backup`:
```sh
#!/bin/sh
set -e
d=/var/backups/hermes; mkdir -p "$d"
runuser -u hermes -- /usr/local/bin/hermes-server backup --config /etc/hermes/hermes.toml "$d/hermes-$(date +%F).db"
tar -C /var/lib/hermes -czf "$d/files-$(date +%F).tar.gz" files
find "$d" -type f -mtime +14 -delete
```
Optionally enable Hetzner's own server backups (paid add-on) or copy `/var/backups/hermes` off the machine (e.g. a Hetzner Storage Box via `rsync`).

**10. Updating**: re-run the install script from step 5 (keeps config and data), then `systemctl restart hermes`.

---

## 19. Testing strategy

| Level | What | How |
|---|---|---|
| Unit | snowflake, permissions `compute` (table-driven, every bit and override combo), markup parser, jitter buffer (reorder, loss, late packets), datagram encode/decode, config loading/env overrides, message grouping | `cargo test` |
| Server integration | every REST endpoint incl. permission denial cases; gateway identify/ready/resume/invalid_session; event audiences (user without VIEW_CHANNEL must not receive events); rate limits; first-start setup invite | spawn server on a random port with a temp data dir inside the test; `reqwest` + `tokio-tungstenite` test client in `hermes-server/tests/` |
| Voice | QUIC auth with valid/expired token; forwarding rules (muted/deaf/no SPEAK); two `hermes-voicebot` instances exchanging a WAV and measuring loss/latency | `hermes-voicebot --join <channel> --play test.wav --record out.wav` |
| Client core | store updates from recorded event sequences; optimistic message reconciliation; reconnect/resume logic with a fake server | unit + integration against the real server |
| GUI | key screens rendered headlessly and snapshot-compared; manual test checklist per release | iced 0.14 headless testing support |
| Load (manual, before v1.0) | 50 bot clients chatting, 20 voicebots in one room on a 1 vCPU VPS | `hermes-voicebot` + a small chat-bot mode |
| CI | `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`, build on Windows + Linux | GitHub Actions |

---

## 20. Build order: phases and milestones

The order is designed so that **risky parts are proven early**, **something usable exists as soon as possible**, and the **protocol is validated by a cheap client (TUI) before investing in the GUI**. Time estimates assume part-time solo work (~10–15 h/week) and are rough.

### Phase 0: Foundations and spikes (1–2 weeks)
Goal: remove the biggest unknowns before writing real code.
- [x] Create the GitHub organization `senko-tools` and the repository `hermes` (see [5.1](#51-repositories-and-versioning)); create the workspace, crates (empty), CI, licenses, `docs/DECISIONS.md`.
- [x] Point DNS `hermes.senko.tools` at the Hetzner VPS and open the firewall ports from [18.8](#188-reference-deployment-hermessenkotools-on-hetzner-debian-13) (needed for Spike B).
- [x] **Spike A: audio loopback on Windows**: cpal capture → nnnoiseless → Opus encode → decode → playback, with < 60 ms added latency. Proves the audio stack builds (libopus static) and works.
- [x] **Spike B: QUIC datagrams**: quinn server on the Hetzner VPS (UDP 7700), client at home, send 50 packets/s of 200 bytes, measure RTT and loss. Proves the network approach.
- [x] **Spike C: iced UI risks**: `pane_grid` with splits, `text_editor` input, a 10,000-row virtualized list scrolling smoothly, color emoji rendering, file drop event, and a `global-hotkey` push-to-talk key that reports press **and release** while the window is unfocused.
- [x] Write findings into `docs/DECISIONS.md`; if a spike fails, apply the fallback from [Risks](#21-risks-and-how-to-handle-them).

**Done when:** all three spikes work on the author's Windows machine.

### Phase 1: Server core, text only (3–4 weeks)
- [x] `hermes-proto`: IDs, models, error codes, permissions + `compute` with tests, gateway envelope/events, markup parser.
- [x] Config loading (TOML + env + `SERVER_PORT` fallback), `init`, `check-config`.
- [x] SQLite + migrations, snowflake generator, state cache load.
- [x] TLS modes (`self_signed` with persisted cert, `files`, `off`).
- [x] Auth: register/login/logout/sessions, first-start setup invite, invites.
- [x] Spaces, channels, roles, members, overrides REST + audit log.
- [x] Messages REST (send, edit, delete, history, reactions, pins, typing, ack/read states, mentions).
- [x] Gateway: hello/identify/ready/heartbeat/resume, EventBus with audiences, all dispatch events for the above.
- [x] DMs and group DMs.
- [x] Attachments upload/download/thumbnails.
- [x] Rate limits, validation, graceful shutdown, `/healthz`, the "Hermes is ready" log line.
- [x] TLS certificate reload on SIGHUP / file change.
- [x] Landing page `/` and invite page `/invite/<code>`.
- [ ] **Minimal deployability early**: `install.sh` + systemd unit (and the Dockerfile + compose file), and deploy the server to `hermes.senko.tools` following [18.8](#188-reference-deployment-hermessenkotools-on-hetzner-debian-13), so a real test instance runs from now on.
- [x] Integration tests for all of the above.

**Done when:** the integration test suite passes and the server runs on `hermes.senko.tools` with a valid Let's Encrypt certificate.

### Phase 2: Client core + TUI text client (2–3 weeks)
- [x] `hermes-client-core`: instance management, probing, fingerprint pinning/known hosts, keyring tokens, REST client, gateway with resume/backoff, Store, optimistic sending, history windows, read-state handling, notifications logic, config loading + hot reload, slash commands.
- [x] `hermes-tui`: layout, modes, sidebar tree, message view, input with autocomplete, member list, commands, add instance/login flow.

**Done when:** the author can use the TUI daily for text chat with a friend on the VPS instance, across reconnects, without bugs that lose messages.

### Phase 3: iced GUI, text features to parity (4–6 weeks)
- [x] App skeleton, theme system (dark/light), fonts, settings persistence.
- [x] Add instance / login / register / fingerprint flows, `hermes://` link handling, single-instance forwarding.
- [ ] Sidebar tree with unread/mention badges, DMs section, context menus.
- [ ] Pane grid with save/restore, command bar, keybinds.
- [ ] Channel buffer: virtualized message list, grouping, replies, edits, reactions, attachments with previews and image viewer, typing, new-messages divider, jump to present.
- [ ] Input box: multi-line, autocomplete, reply/edit modes, uploads (dialog, drag and drop, paste).
- [ ] Notifications, tray icon, taskbar flash.

**Done when:** a non-technical friend can install the GUI, click an invite link, create an account and chat comfortably.

### Phase 4: Voice backend and audio engine (3–4 weeks)
- [ ] Server: quinn endpoint on the same port number (UDP), voice cert, `/voice/join` tokens, control stream protocol, room registry, forwarding rules, voice states over the gateway, server mute/deafen/move, user limits, DM rooms and ringing.
- [ ] `hermes-audio`: full capture and playback pipelines, jitter buffer, mixer, device enumeration, device-loss recovery, UI sounds.
- [ ] Client core: voice connection, join/switch/leave/reconnect, speaking detection, per-user volume, events.
- [ ] `hermes-voicebot` and voice integration tests.

**Done when:** two voicebots and two real users talk in one room for 30 minutes with no audible glitches on normal connections and audio recovers automatically after a 5-second network drop.

### Phase 5: Voice UI (1–2 weeks)
- [ ] GUI: voice users in the sidebar (visible without joining), double-click to join, voice panel, voice room buffer, speaking rings, per-user volume menu, mute/deafen keybinds, global push-to-talk and global mute, incoming-call banner, audio settings page with level meter and mic test.
- [ ] TUI: voice users in sidebar, `J`/`L`/`m`/`D`, status line.

**Done when:** voice is usable daily from both clients.

### Phase 6: Community and moderation features (2–3 weeks)
- [ ] GUI space settings: overview, roles editor (permissions with descriptions, drag-reorder), channel permission overrides (tri-state), members (roles, kick, ban), invites management, bans, audit log viewer.
- [ ] Create/edit/delete/reorder channels and categories by drag and drop, private channel creation.
- [ ] Notification settings per space/channel, mute.
- [ ] Search (FTS5) buffer, mentions inbox, pins.
- [ ] Profile editing (avatar upload, display name, status text), sessions management.

**Done when:** a friend group can run their community without anyone touching the server CLI.

### Phase 7: Packaging and self-hosting polish (1–2 weeks)
- [ ] Release workflow: all artifacts from [18.1](#181-release-artifacts-built-by-githubworkflowsreleaseyml-on-git-tag-vxyz).
- [ ] Windows installer (cargo-packager/NSIS) with `hermes://` registration, Start-menu shortcut, uninstaller; portable zip.
- [ ] `install.sh` + systemd unit; Pterodactyl egg tested on a real panel; reverse proxy docs.
- [ ] `docs/self-hosting.md` written for non-experts (with screenshots).
- [ ] Admin CLI complete (`backup`, `user …`, `invite create`).

**Done when:** a person who has never seen the project can host an instance by following `docs/self-hosting.md` with any of the three methods in under 15 minutes.

### Phase 8: Hardening and v1.0 (2 weeks)
- [ ] Load test (see [Testing](#19-testing-strategy)) and fix bottlenecks.
- [ ] Security review pass over the checklist in [Security](#13-security-model); fuzz the datagram parser and gateway decoder (`cargo fuzz`).
- [ ] Accessibility basics: full keyboard navigation in the GUI, high-contrast theme, scalable UI.
- [ ] Update check, crash log file, "copy debug info" in About.
- [ ] Bug bash with friends for a week; fix.

**v1.0 is done when all of these are true:**
1. Text chat, DMs, group DMs, reactions, replies, edits, attachments, search all work across reconnects.
2. Browsing a space never joins voice; voice channel occupants are always visible.
3. Voice works in channels and DM calls with noise suppression and PTT/VAD.
4. Roles and permissions are enforced server-side and editable in the GUI.
5. Server deploys via Docker, systemd script, and Pterodactyl egg.
6. All tests pass in CI on Windows and Linux.

**Rough total:** 25–35 weeks part-time.

### Practical tips for the order of work inside each phase
- Write the `hermes-proto` types for a feature first, then the server endpoint + test, then the client-core method, then the UI. Never the other way around.
- Keep the TUI working as features land; it is the quickest way to manually test protocol changes.
- Commit `docs/protocol.md` updates together with protocol changes.

---

## 21. Risks and how to handle them

| Risk | Likelihood | Mitigation / fallback |
|---|---|---|
| Global push-to-talk key release is unreliable with `global-hotkey` on Windows (unfocused window) | Medium | Fallback: a dedicated thread polling key state with `GetAsyncKeyState` via the `windows` crate (or the `device_query` crate) every 10 ms for the configured PTT key. Decide in Spike C. |
| libopus build problems on Windows | Low–Medium | Use the crate's bundled/static build feature; fallback: prebuilt libopus via vcpkg in CI. |
| QUIC/UDP blocked on some networks (strict corporate/school Wi-Fi) | Low for friend groups | v1: show a clear "voice connection failed (UDP blocked?)" error. Post-v1: TCP fallback carrying voice frames over a WebSocket (higher latency but works). |
| iced text input / list performance or missing widgets | Medium | Virtualize lists; borrow patterns from Halloy (GPL-compatible); keep widgets in `widget/` so they can be swapped. Track iced releases; Halloy is a good reference for upgrade paths. |
| Color emoji rendering in iced | Medium | Verify in Spike C; fallback: render emoji as bundled Twemoji/Noto SVG images via a custom rich-text widget. |
| Echo without AEC when users use speakers | Medium | Noise suppression + VAD reduce it; recommend headphones in UI; AEC is the first post-v1 voice item. |
| Scope creep | High | Anything not listed in [Scope](#3-scope-what-v10-includes-and-excludes) goes to the roadmap, not into the current phase. |
| Windows SmartScreen warns on the unsigned installer | High | Document "More info → Run anyway" in the README; code signing is post-v1. |
| SQLite write contention | Low at target scale | WAL + single writer pool connection for writes; messages are small. |

---

## 22. Post-v1 roadmap
Roughly in priority order:
1. Echo cancellation (`webrtc-audio-processing` crate) and stereo/music mode for voice channels.
2. Screen sharing and video (VP8/AV1 over QUIC streams/datagrams, simulcast-lite), voice channel "stage" layout.
3. Custom emoji and stickers per space.
4. Link previews (opt-in, server-side fetch with SSRF protection).
5. Threads.
6. Bot accounts + webhook API (bots use `hermes-proto` directly, it is MIT/Apache).
7. TCP fallback for voice.
8. Syntax highlighting in code blocks, inline images in the TUI.
9. Postgres storage backend (behind the same `db` module interface).
10. Web client (would need WebTransport for voice, which maps well onto QUIC datagrams).
11. Android client.
12. End-to-end encrypted DMs (MLS).
13. Federation between instances.

---

## 23. Licensing
- `hermes-proto`: **MIT OR Apache-2.0** (so anyone can build bots/tools/clients).
- `hermes-server`: **AGPL-3.0-or-later** (modified hosted versions must share source).
- Clients and `hermes-audio`, `hermes-client-core`, `hermes-voicebot`: **GPL-3.0-or-later**, which is compatible with Halloy's GPL-3.0 and allows adapting Halloy code with attribution in file headers and the README.
- Bundled fonts (Inter, JetBrains Mono) are under the SIL Open Font License; include their license files. Sounds must be self-made or CC0.

---

## 24. Glossary
- **Instance**: one running Hermes server.
- **Space**: a community inside an instance (Discord calls these "servers").
- **Buffer**: anything that can be shown in a GUI pane (a channel, settings, search…). Term borrowed from Halloy/IRC clients.
- **Gateway**: the WebSocket connection that delivers real-time events.
- **Dispatch event**: a gateway event carrying data, with a sequence number.
- **SFU**: Selective Forwarding Unit; the server forwards each user's audio to others without mixing.
- **Opus**: the audio codec used for voice.
- **QUIC**: encrypted UDP-based transport; used here for voice.
- **Datagram**: an unreliable, unordered QUIC message; one per 20 ms audio frame.
- **Jitter buffer**: a short client-side delay that smooths out uneven packet arrival.
- **VAD**: voice activity detection; transmit only while speaking.
- **PTT**: push-to-talk.
- **DTX**: Opus discontinuous transmission; sends almost nothing during silence.
- **TOFU / pinning**: trusting a self-signed certificate's fingerprint the first time (or from an invite link) and refusing changes later.
- **Egg**: a Pterodactyl/Pelican panel template describing how to install and run a server.
- **Monorepo**: one git repository containing several related projects (here: server, clients and shared crates).
- **Lockstep versioning**: all parts are released together under one version number.

---

## 25. Working with an AI assistant on this plan
This plan is written so an assistant (or a new contributor) can pick up any phase:
1. Point it to this file and the current phase's checklist in [section 20](#20-build-order-phases-and-milestones).
2. Ask for one checklist item at a time, in the order *proto types → server + tests → client-core → UI*.
3. Require that every change keeps `cargo fmt`, `cargo clippy -D warnings` and `cargo test --workspace` green.
4. Any deviation from this plan must be added to `docs/DECISIONS.md` in the same change.
5. Tick checklist items in this file as they are completed so progress is visible.
