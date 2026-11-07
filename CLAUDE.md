# Hermes

Hermes is a self-hostable chat platform (spaces with text + voice channels, DMs, voice calls) written in Rust:
a single-binary server, a gpui desktop client (primary, see `docs/desktop-client.md`) and a ratatui terminal client (secondary).

`PLAN.md` and `docs/DECISIONS.md` are historical: they need not be read, followed or updated.
The developer's requests are what to build.

## How to work in this repo
- Before writing code for anything that touches more than two or three files, post a short plan
  (files to create/change and why) and wait for approval.
- Order inside a feature: `hermes-proto` types -> server endpoint + tests -> `hermes-client-core` -> UI.
- If a request is impossible, ambiguous or clearly wrong, stop and ask.
- Keep changes small and reviewable. Do not add new dependencies without asking.

## Commands (must all pass before a task is considered done)
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

## Environment
- Sessions run in a Linux cloud VM. The developer's own machine is Windows, and the production server is a
  Debian 13 VPS (hermes.senko.tools). Claude has access to neither.
- Things that need real hardware or the real server (microphone/speaker tests, GUI look and feel on Windows,
  QUIC tests over the internet, deployment) cannot be verified here. Write the code, make it compile and pass
  the tests that can run here, then give the developer exact step-by-step instructions to test it on Windows
  or on the server.
- Windows-specific code must still compile on Linux (use `cfg(windows)` where needed); CI on GitHub Actions
  builds on both Windows and Linux.
- Do not connect to remote servers, with one exception: when the session runs on the developer's own PC,
  Claude may connect to the production server with `ssh senko.tools` or `hetzner` (from that PC only,
  never from a cloud VM).
- Never read, print or commit secrets, private keys or tokens.

## Git
- Commit messages: `area: what changed` (e.g. `server: add login endpoint`, `proto: add Message model`).
- Push to the `dev` branch only; the developer reviews and merges the pull request from `dev` into `main`.
