# Where things are, and the rules that keep them in one place

A one-page map. The desktop client's own guide is `desktop-client.md`; encryption is `e2ee.md`.

## Crates
- `hermes-proto`: the wire types and the logic both sides need (permissions, markup, sealed-reaction prefix).
- `hermes-server`: the server. `api/` handlers, `db/` SQL (checked `query!`, offline data in `.sqlx/`),
  `state/` the cache, `voice/` QUIC voice. Feature `testing`: `hermes_server::testing::spawn`, a real
  server for the clients' tests.
- `hermes-client-core`: everything a client does without drawing it: `Core`, the store, REST, gateway,
  voice, encryption, config. Also what both clients show the same way:
  - `menus`: which menu entries a target offers (the GUI draws them; the TUI asks `allowed_actions`).
  - `view`: member lists (per channel), mention names, role colours, typing lines, sizes.
  - `os_open`: handing links, folders and files to the system.
  - `net`: which address to try first (IPv4 for 10 minutes after a failed connect).
  - Feature `testing`: `testing::fixtures`, a store for tests without a server.
- `hermes-gui`: the desktop client; `hermes-tui`: the terminal client; `hermes-audio`; `hermes-voicebot`.

## Rules
- **Settings are changed in code:** `Shell::set_config(|c| ..., cx)` / `Core::update_config`. A switch
  is `flag!(self, cx, "Name", "Hint", section.field)`, which names the field once. Settings with a few
  named values are enums (`config::choice!`), read leniently, written as before.
- **Controls come from `hermes-gui/src/ui/`:** `chip`, `nav_item`, `list_row`, `icon_button`/`tip_button`,
  `error_text`, `chevron`. They follow the pointer through `Shell::track_hover` and one spring. A
  text-colour `.hover(..)` inside `.when(..)` keeps a stale hover in gpui; a test refuses it.
- **Overlays are listed once** in `hermes-gui/src/overlay.rs`: what Escape closes first, what takes
  the keys, how each closes.
- **Menu actions:** offered by `menus::entries_with`, done by `run_action` and the `run_*_action`
  functions; one nothing handles fails a debug build.
- **Errors:** `CoreError` says what kind (`Encryption`, `NotSignedIn`, `Refused`, `Voice`, `Io`, ...);
  front ends turn it into text with `?`. Server: `Error::bad_request`, `Error::forbidden`.
- **Locks** in client-core never panic on a poisoned lock (`unwrap_or_else(PoisonError::into_inner)`).
- **Big parts are folders:** `hermes-gui/src/channel/` and `settings/` split one `impl Shell` into a
  file per job; a method used by a sibling file is `pub(super)`. A `macro_rules!` in a `mod.rs` must
  come before the `mod` lines that use it.
- **Tests:** server integration tests are one binary (`tests/it/`), helpers in `tests/it/common`.
