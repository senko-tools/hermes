# gpui migration: where things stand

Start here in a new session. Reasons and history are in `docs/DECISIONS.md` (2026-11-07 entries).

## The goal
Replace the iced client (`crates/hermes-gui`) with a gpui one (`crates/hermes-gpui`, binary `hermes-gpui`,
using `gpui-kit =0.9.0`). Only `hermes-gpui` matters now: when the developer finds it working, it replaces
`hermes-gui` completely and takes its crate and binary names. Then `hermes-gui` and `spikes/iced-ui` are
removed and PLAN.md section 15 is rewritten.

The client runs on the real `Core`: `cargo run -p hermes-gpui -- --config-dir <dir>`. Testing on the developer's
PC needs a scratch server and its own config dir; see "How the window was tested".

## How it is built
- **One root view**, `Shell` in `src/shell.rs`, owns the state. Each feature's module adds `impl Shell` blocks.
- **Async:** `main` owns a tokio runtime and keeps it entered on the UI thread, because `Core` calls
  `tokio::spawn`. `Shell::run(cx, work, then)` runs `work` on tokio and `then` on the UI thread, with the window.
- **Copied from `hermes-gui` without UI:** `tree`, `rows`, `markup`, `menu_entries`, `collapsed`,
  `single_instance`, `watchdog`, `open`, `desktop/`, `screen/` (capture and codecs), `keybinds`, `ptt`.
- **Sizes:** use `theme::px`, never gpui's `px`: it applies the UI scale. Text sizes are the roles
  `theme::heading_size()`, `body_size()`, `secondary_size()` and `caption_size()`. `font.size` draws 3 px larger than
  its value (the default 11 means 14 px).
- **Colours:** `theme::*()` functions read the active theme, a thread-local, so they work anywhere.
  `theme::apply_to_kit` colours the kit's widgets; kit buttons read `button_primary` etc., not `primary`.
- **Dialogs** are the shell's own (`modal::dialog`), since the kit's rebuild from a closure without our state.

## Decided by the developer
- **Clicks:** every click, left or right, acts on mouse up, never mouse down. Do not use the kit's
  `context_menu` (it opens on press); use `Shell::with_menu` in `src/menu.rs`. Left clicks: `on_click`.
  Exceptions: drags and placing the text cursor start on press, and the kit's menus close on press
  when clicking outside them.
- **Look:** sizes like Discord's (34 px rows, 40 px avatars, 240 px sidebar, 72 px rail).
  JetBrainsMono Nerd Font and Material icons, as in `hermes-gui`.
- **Header:** a bar over the main panel, without a title: it drags the window and holds the page's buttons and
  fields and the window buttons. The native title bar is off; the window title stays set for the taskbar.
- **Rail, like Discord's:** a column of icons on the far left. The top icon is the app icon (feather,
  maybe custom later) and opens Friends. The others are spaces; selecting one shows its channels in the
  sidebar and opens the channel last open in it, else its first text channel. Spaces are not a dropdown.
- **Friends:** the sidebar lists DMs and group DMs, and the main panel lists requests and people from shared
  spaces. Opening someone opens the DM.
- **Instances:** hidden from the sidebar unless none is saved or one needs attention. Their actions are in the menu
  of the rail's plus, the empty rail or sidebar space, and the user panel.
- **Network graph:** the voice panel's connection line opens live graphs of latency, loss and bandwidth.
- **Responsibility:** the developer left share video and the tray to Claude's judgement.

## Not done yet, on purpose
1. **Friend system** (requests stored on the server: proto, server + tests, core, UI): after this port.
2. **Split panes:** after everything else. Their keybinds stay valid config but do nothing.
3. **Replacing `hermes-gui`:** when the developer says so. Rename the crate and binary to `hermes`, remove the
   iced crate and spike, update CI, docs and PLAN.md section 15.

## Known risks
- **Share video:** gpui's Windows `draw_surfaces` is an empty stub, so pictures are per-frame BGRA images freed
  with `drop_image`. See DECISIONS for the numbers. A debug build converts far too slowly to judge: measure release.
  Only a mostly still screen between two clients on one PC has been measured.
- **No software fallback:** gpui needs a D3D11 adapter. Test on the developer's laptop and over remote desktop.
- **Unstable API:** GPUI is pre-1.0, so keep exact pins.
- **Performance:** the expected gain has not been measured against iced.

## State of the working tree
- The developer's earlier iced work fails `cargo clippy --workspace` (`hermes-gui/src/screen/capture.rs:444`) and
  `cargo fmt --all --check` (6 files). It does not matter while iced is on its way out, so check this crate with
  `cargo fmt -p hermes-gpui` and `cargo clippy -p hermes-gpui --all-targets -- -D warnings`.
- Full workspace builds can run Windows out of memory (os error 1455): use `-j 4`.
- `cargo clean` only once the app compiles and the build is not needed any more (developer's request).

## Not yet checked by ear or by a person
Real mic and speaker quality in a call, push-to-talk and the global mute key, the tray's menu, toasts and the taskbar
flash, real keyboard modifiers (Ctrl+K, Shift+Enter; the test helper can only send plain keys), file dialogs,
drag and drop, and how the sizes compare to Discord. Linux CI packages in `.github/workflows/ci.yml` are untested.

## How the window was tested
Run a scratch `hermes-server` copied elsewhere (port 7799, `registration.mode = "open"`), seed it over REST, start the
debug binary with `--config-dir <scratch>`, send mouse messages to its window with `PostMessage` (the real cursor does
not move), and screenshot with `PrintWindow` flag 2. Check both a large and a small window. Use `Start-Process`, and
never run a command that can wait on stdin (a stray `python -` hung a session for 20 minutes). Never screenshot a share
of the real screen: judge it from the logs and from pixel statistics of the tile.
