# Security review (Phase 8, 2026-11-01)

A pass over the checklist of PLAN.md section 13 before v1.0. "Checked in" names where the
property lives and what tests it.

| Item | Status | Checked in |
|---|---|---|
| Passwords are argon2id, hashed off the async threads | ok | `auth::hash_password` (`spawn_blocking`); a dummy hash runs for unknown users so login timing does not reveal accounts |
| Tokens: 32 random bytes, `hms_` prefix, only SHA-256 stored, never logged | ok | `auth::new_token`, `auth::token_hash`; no `tracing` call logs a token (grep) |
| Every REST request checks its session in the database | ok | `Auth` extractor: a deleted session (logout, password change, admin disable, `user reset-password`) stops working at once; gateway connections are closed (directly, or within 10 s for CLI changes) |
| Client tokens in the OS credential store | ok | `client-core::tokens` (keyring, file fallback with user-only permissions) |
| TLS pinning for self-signed servers, refusal on a changed fingerprint, voice pinned to `cert_sha256` | ok | `client-core::tls`, `known_hosts`; voice uses `tls::voice_config` |
| Rate limits: logins per IP, REST per session/IP, messages per channel, gateway ops | ok | `ratelimit.rs`; gateway closes with 4008 above the op budget |
| Input validation: lengths, usernames, control characters | ok | `api::validate`; new in this pass: status text must be one line, search queries are at most 256 characters and every word is quoted for FTS5 (no syntax errors from user input) |
| Uploads: generated names on disk, `nosniff`, non-images downloaded | ok | `files/` (sharded `files/ab/cd/<id>`), `api::attachments` |
| Authorization through one place, every permission tested | ok | `api::perms::require_space` / `require_channel`; new endpoints: search and mentions only cover channels with `VIEW_CHANNEL` + `READ_HISTORY` (DMs: recipients), notification settings only for spaces/channels you can see, `GET /users/{id}` only for people you share a space or DM with, admin endpoints only for instance admins, who cannot disable or demote themselves |
| No telemetry, no server-side external requests | ok | the only outgoing request of a client is the optional update check to the GitHub API (`general.check_for_updates`) |
| Audit log for moderation | ok | `api::audit`, shown in Space settings → Audit log |
| Parsers survive hostile input | ok | `hermes-proto/tests/fuzz.rs` (20 000 random/mutated inputs per parser per test run: voice datagrams, gateway frames, markup); `fuzz/` for coverage-guided runs; deep JSON nesting is rejected by `serde_json`'s recursion limit |

## Notes for later

- The installer and binaries are not code-signed (section 21: post-v1).
- Run `cargo +nightly fuzz run datagram` / `gateway` / `markup` for a few hours before big
  releases; add any crash input to the test above.

# Security review (after 1.0.0, 2026-11-02)

A second pass, aimed at code execution on the server and the clients and at users acting on
other people's things. Every REST handler, the gateway ops, the voice control stream and
datagram forwarding, and every place a client hands data from the network to the operating
system were read again.

## Fixed in this pass

| Problem | Impact | Fix |
|---|---|---|
| An invite link's `#fp=` replaced the pinned fingerprint of a host, and a pin replaces CA validation | A crafted `hermes://<real server>/invite/x#fp=<attacker>` link let an attacker on the network impersonate a server the user already uses, also a Let's Encrypt one (credentials, messages) | `Core::trust_invite_fingerprint`: an invite pins only a host this client has never trusted; existing pins and already added instances are never changed by a link (`client-core/tests/live.rs` `invite_links_never_replace_a_pin`) |
| The GUI started `rundll32` and `explorer` by name on Windows | Windows looks for a program in the application's folder first: a portable `hermes.exe` run from Downloads would start a downloaded `rundll32.exe` (attachments are saved to Downloads) on the next link click | `ShellExecuteW` instead (`gui::open::launch`); URLs must be plain `http(s)` without quotes, backslashes, whitespace or control characters (`is_web_link`) |
| The TUI opened downloads unless the extension was on a list of program types | The list missed types that run code when opened (`.chm`, `.iso`, `.py`, `.xll`, `.library-ms`, …) | Only viewer types open (pictures, sound, video, PDF, plain text: `files::opens_in_viewer`); everything else is only saved |
| Downloads carried no Mark of the Web | Opening a saved file by hand skipped SmartScreen and Office Protected View | `files::mark_downloaded` writes `Zone.Identifier` (ZoneId=3) on Windows, in the GUI and the TUI |
| Bidirectional overrides in filenames and names | `invoice<U+202E>fdp.exe` displays as a PDF; display names could imitate someone else's | The server refuses U+202A–202E and U+2066–2069 in filenames and all names; clients replace them (and zero-width marks) when saving |
| More Windows device names | `CON .txt`, `CONOUT$`, `COM¹` were not caught | `files::safe_filename` |
| A `hermes://` command line was parsed for flags | If a launcher ever passed a crafted link unescaped, it could add `--config-dir \\host\share` | Started with a `hermes:` argument, the GUI ignores every other argument |
| Voice moderation ignored the role hierarchy | A member with `MUTE_MEMBERS`/`DEAFEN_MEMBERS`/`MOVE_MEMBERS` could mute, deafen, move or disconnect the owner and higher roles | Only members below the moderator's highest role (`server/tests/voice.rs` `moderators_cannot_act_on_higher_members`) |
| Channel overrides ignored the role hierarchy | A channel manager could deny a higher role access to a channel | Only `@everyone`, lower roles and the caller's own roles (`server/tests/channels.rs` `overrides_only_for_lower_roles`) |

## Checked and fine

- **Deleting and editing other people's things:** messages: edit author only, delete author or
  `MANAGE_MESSAGES` in that space (audited); DMs never grant `MANAGE_MESSAGES`. Reactions:
  others' only with `MANAGE_MESSAGES`. Pins: `MANAGE_MESSAGES` (DMs: recipients). Invites:
  creator, `MANAGE_INVITES`, or instance admins for instance invites. Members, bans, roles and
  nicknames: permission bit plus hierarchy. Spaces: delete and transfer owner only. Group DMs:
  the owner removes others, anyone removes themselves. Sessions: only your own.
  Attachments: attach only your own unattached uploads; avatars and icons only your own images.
- **Server code execution:** no shell or child processes; files are stored under generated
  names (`files/ab/cd/<id>`), never the user's filename; SQL is parameterised (`sqlx::query!`);
  images are decoded with a 256 MiB limit, at most two at a time, and only probed raster images
  are served inline (everything else as a download with `nosniff`); `Content-Disposition` and
  `Content-Type` are built from cleaned values; the HTML pages escape everything.
- **Client code execution:** toasts are built with `SetInnerText` (no XML injection); the TUI
  never writes server text to the terminal outside ratatui, which drops control characters;
  links open only as `http(s)`; nothing goes through a shell; downloads never overwrite.
- **Native decoders fed by other users:** images (Rust `image` crate, limits), Opus (libopus,
  packets ≤ 1000 bytes, sender overwritten by the server). Keep both up to date.
