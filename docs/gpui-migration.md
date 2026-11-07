# gpui migration: where things stand

Start here in a new session. Reasons and history are in `docs/DECISIONS.md` (2026-11-07 entries).

## The goal
Replace the iced client (`crates/hermes-gui`) with a gpui one (`crates/hermes-gpui`, binary `hermes-gpui`,
using `gpui-kit =0.9.0`). `hermes-gui` stays untouched until the new client replaces it, then it and
`spikes/iced-ui` are removed and PLAN.md section 15 is rewritten.

Today the new client is a layout shell on sample data (`src/sample.rs`), with no server connection.
Run it with `cargo run -p hermes-gpui`, or add `-- --no-instances` for the state with no saved instance.

## Decided by the developer
- **Clicks:** every click, left or right, acts on mouse up, never mouse down. Do not use the kit's
  `context_menu` (it opens on press); use `Shell::with_menu` in `src/menu.rs`. Left clicks: `on_click`.
  Exceptions: drags and placing the text cursor start on press, and the kit's menus close on press
  when clicking outside them.
- **Look:** sizes like Discord's (16 px text, 34 px rows, 40 px avatars, 240 px sidebar, 72 px rail; see
  `src/theme.rs`). JetBrainsMono Nerd Font and Material icons, as in `hermes-gui`.
- **Title bar:** the native Windows one, titled with the current page ("Friends", "#general", "Dave").
  There is no in-app header, so channel topics and header buttons have no place yet.
- **Rail, like Discord's:** a column of icons on the far left. The top icon is the app icon (feather,
  maybe custom later) and opens Friends. The others are spaces; selecting one shows its channels in the
  sidebar and opens its first text channel. Spaces are not a dropdown.
- **Friends:** the sidebar lists DMs and group DMs, and the main panel lists requests and people. Opening
  someone opens the DM.
- **Instances:** hidden from the sidebar unless none is saved. Added from the menu of the empty rail or
  sidebar space, or from settings.
- **Friend system:** a real one, with requests stored on the server. Not built yet.
- **From `hermes-gui`:** sounds (`hermes-audio`), the feather, and logic where it fits. The sidebar tree,
  message rows and collapsed state come over with real data. Check `hermes-gui` before writing logic anew.
- **Priorities:** the Join voice spam (`self.joining` is set but never checked,
  `crates/hermes-gui/src/voice.rs:117`) gets fixed in the new voice code, not in iced.

## Next steps (each gets a short plan first)
1. Real `Core` data and login. The port must own a tokio runtime (iced provides it today).
2. Message list and composer on real data.
3. Screen-share video.
4. Voice and settings.
5. Friend system: proto, server + tests, core, UI.
6. Move the tests over, rename the binary to `hermes`, remove the iced crate.

## Known risks
- **Share video:** gpui's Windows `draw_surfaces` is an empty stub and `surface()` is macOS/iOS only.
  Per-frame images are the only built-in way, and they were dropped once for exhausting GPU memory. It
  probably needs a patched renderer. Only `hermes-gui/src/screen/view.rs` has to be replaced, since
  decoded frames arrive as CPU bytes.
- **No software fallback:** gpui needs a D3D11 adapter. Test on the developer's laptop and over remote desktop.
- **Unstable API:** GPUI is pre-1.0, so keep exact pins.
- **Performance:** the expected gain has not been measured against iced.

## State of the working tree
- Nothing from this work is committed. The developer's own uncommitted work is in the tree too.
  **Commit theirs first:** `Cargo.lock` mixes both, CI uses `--locked`, and `hermes-gpui`'s icons were
  copied from the uncommitted `hermes-gui/assets/icons/mdi/`.
- That uncommitted work fails `cargo clippy --workspace` (`hermes-gui/src/screen/capture.rs:444`, enum size,
  after `dxgi.rs` grew) and `cargo fmt --all --check` (6 files). Left for the developer; do not run
  `cargo fmt --all` over their work without asking. Check `hermes-gpui` with `cargo fmt -p hermes-gpui`.
- Full workspace builds can run Windows out of memory (os error 1455): use `-j 4`.

## Not yet checked by eye or ear
Join and leave sounds, real keyboard typing and Shift+Enter, the taskbar icon, and how the sizes compare
to Discord. The Linux CI packages added to `.github/workflows/ci.yml` are untested.

## How the window was tested
Start the debug binary, send mouse messages to its window with `PostMessage` (the real cursor does not
move) and screenshot the window rectangle. Use `Start-Process`, and never run a command that can wait on
stdin (a stray `python -` hung a session for 20 minutes).
