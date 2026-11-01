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
