# Hermes

A self-hostable, open-source chat platform for friend groups and small communities: spaces with text and
voice channels, private messages (1:1 and group) and voice calls. Written entirely in Rust, with a
single-binary server, an [iced](https://github.com/iced-rs/iced) desktop client and a
[ratatui](https://github.com/ratatui/ratatui) terminal client.

> **Status:** early development (Phase 0). Nothing is usable yet.

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
| [`hermes-voicebot`](crates/hermes-voicebot) | Dev tool: headless voice client for testing | GPL-3.0-or-later |

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
