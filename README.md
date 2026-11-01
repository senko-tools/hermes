# Hermes

A self-hostable, open-source chat platform for friend groups and small communities: spaces with text and
voice channels, private messages (1:1 and group) and voice calls. Written entirely in Rust, with a
single-binary server, an [iced](https://github.com/iced-rs/iced) desktop client and a
[ratatui](https://github.com/ratatui/ratatui) terminal client.

> **Status:** 1.0. Text chat, DMs and group DMs, voice channels and calls, roles and permissions,
> search, and self-hosting with Docker, a Linux install script or a game server panel.

## Getting started

- **Use it:** download `Hermes-Setup-<version>.exe` (or the portable zip) from the
  [releases](https://github.com/senko-tools/hermes/releases), open an invite link from a friend
  or paste a server address. Windows SmartScreen warns about the unsigned installer: choose
  *More info → Run anyway*. The terminal client `hermes-tui` comes with it.
- **Host it:** [`docs/self-hosting.md`](docs/self-hosting.md) walks through it step by step.
- **New to the app?** Right-click almost anything (channels, spaces, messages, people) for its
  actions, press `Ctrl+K` to jump anywhere, and `Ctrl+F` to search.

The full specification and build plan is [`PLAN.md`](PLAN.md). Deviations from it are logged in
[`docs/DECISIONS.md`](docs/DECISIONS.md).

## Crates

| Crate | What it is | License |
|---|---|---|
| [`hermes-proto`](crates/hermes-proto) | Shared wire types, permissions, markup parser. No I/O. | MIT OR Apache-2.0 |
| [`hermes-server`](crates/hermes-server) | Server binary and library | AGPL-3.0-or-later |
| [`hermes-audio`](crates/hermes-audio) | Audio engine (capture, playback, Opus, jitter buffer, mixer) | GPL-3.0-or-later |
| [`hermes-client-core`](crates/hermes-client-core) | Connection, REST and gateway client, state store, voice transport | GPL-3.0-or-later |
| [`hermes-gui`](crates/hermes-gui) | iced desktop client | GPL-3.0-or-later |
| [`hermes-tui`](crates/hermes-tui) | ratatui terminal client | GPL-3.0-or-later |
| [`hermes-voicebot`](crates/hermes-voicebot) | Dev tools: headless voice client, `hermes-loadtest` | GPL-3.0-or-later |

Phase 0 experiments live in [`spikes/`](spikes) (standalone binaries, not used by the real crates).

## Building

Requires Rust 1.99 or newer.

```bash
cargo build --workspace
```

Checks that must pass before a change is merged (CI runs them on Linux and Windows):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

Each crate carries its own license (see the table above). The full texts are in
[`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE), [`LICENSE-GPL-3.0`](LICENSE-GPL-3.0) and
[`LICENSE-AGPL-3.0`](LICENSE-AGPL-3.0).
