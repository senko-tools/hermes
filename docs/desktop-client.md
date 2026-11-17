# The desktop client

Start here in a new session. Reasons and history are in `docs/DECISIONS.md` (2026-11-07 entries), encryption in
`docs/e2ee.md`.

## The crate
`crates/hermes-gui`, binary `hermes`, on gpui through `gpui-kit =0.9.0`. It replaced the iced client in 1.2.0.

Testing on the developer's PC needs a scratch server and its own config dir; see "How the window was tested".

## How it is built
- **One root view**, `Shell` in `src/shell.rs`, owns the state. Each feature's module adds `impl Shell` blocks.
- **Async:** `main` owns a tokio runtime and keeps it entered on the UI thread, because `Core` calls
  `tokio::spawn`. `Shell::run(cx, work, then)` runs `work` on tokio and `then` on the UI thread, with the window.
- **Plain logic without UI:** `tree`, `rows`, `markup`, `menu_entries`, `collapsed`,
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
  JetBrainsMono Nerd Font and Material icons.
- **Title bar (changed 2026-11-07):** the native Windows one, showing the page and space (`general | Space - Hermes`).
  The bar over the main panel only holds the page's buttons and fields.
- **Left column:** rail and sidebar share one background with a border between them; the voice and user panels span
  both at the bottom. A highlighted voice channel's block includes the people in it (from the developer's screenshot).
- **Rail, like Discord's:** a column of icons on the far left. The top icon is the app icon (feather,
  maybe custom later) and opens Friends. The others are spaces; selecting one shows its channels in the
  sidebar and opens the channel last open in it, else its first text channel. Spaces are not a dropdown.
- **Friends:** the sidebar lists DMs and group DMs; the main panel has Online, All, Pending and People tabs.
  Opening someone opens the DM.
- **Instances:** hidden from the sidebar unless none is saved or one needs attention. Their actions are in the menu
  of the rail's plus, the empty rail space, Friends' empty sidebar space, and the user panel.
- **Network graph:** the voice panel's connection line opens live graphs of latency, loss and bandwidth, over 1, 5 or 15 minutes.
- **Responsibility:** the developer left share video and the tray to Claude's judgement.
- **Words (2026-11-08):** spaces are called "servers" in everything the user reads (code, config keys and
  the protocol keep `space`). Instances stay "instances".
- **Hover (2026-11-08):** most things turn the accent colour under the pointer (`icon_button`, chips, nav
  items, channel rows, people under voice channels, who also get a background since 2026-11-10). Inside a
  highlighted voice block the rows are inset pills that leave its bar alone (2026-11-11). People in member lists, messages, search results and
  emoji keep a background hover. `group_hover` only recolours icons (svg): gpui ignores it for text in
  plain divs, so text takes a direct `.hover` on its row.
- **Voice buttons:** `voice::voice_button` with a `Look`: mute and deafen grey while off and red while
  on, leave the muted-mic red, share accent while sharing; toggles animate with a gpui spring. Share sits
  next to leave in the voice panel, and the room I am in has a floating call bar (share, mute|deafen,
  leave). The header has no Leave button.
- **One client per config directory:** a lock file (`hermes.lock`) plus the link pipe; a second launch
  hands over and exits, also while the first is still starting.
- **Voice while alone:** no audio is sent while nobody else is in the room (`voice_has_listeners`).
- **Mentions:** typed and completed as `@name`, `@name#1234`, `@role`, `#channel`;
  `view::resolve_mentions` turns them into markup on send, `readable_mentions` back for editing.
- **Menus:** the rail's empty space and plus list only the instances; the empty space under a server's
  channels offers Create Channel, Create Category and Invite to Server. A person's menu (member lists,
  voice rows, tiles) has Profile, Volume…, Mute for me, and for moderators server mute/deafen,
  Move to and Disconnect, checked like the server checks them. Negative entries are red
  (`Action::is_negative`); only `is_destructive` ones are confirmed.
- **Motion (2026-11-10):** one rail pill springs between servers. Pages, settings tabs and the sidebar's
  list fade in where they are, without sliding (`theme::enter`, `theme::FADE` = 120 ms). The main page
  crossfades: the page that left is drawn under the new one while it fades out (`Shell::leaving`), except
  voice pages (they hold share video) and one result page to another (they share a search field).
  Popups opened by a click (share, network, volume, profile) go through `modal::popup`. Right-click menus
  keep their own placement.
- **Motion, round 18 (2026-11-11):** things that change under the pointer note it in `Shell::hovered`
  (`hover.rs`: `track_hover`, `is_hovered`, `HOVER_SPRING`) and spring towards it, since gpui's `.hover`
  switches at once. Springs start at their target when they mount, so a row scrolled back into the
  virtual chat list never replays one; fades keyed by an id do replay, so in the chat they only play for
  what is new (a message less than 1.5 s old, a thumbnail that just arrived, a chip new on a message
  already drawn). Dialogs, popups, menus, the status line and the voice panel also play their way out:
  close them with `Shell::close_later(id, clear, cx)` (`modal.rs`), never by setting the field to `None`,
  and call `cancel_closing(id)` where they open. Durations: `theme::FADE` (120 ms) for fades,
  `modal::POP` (150 ms) for dialogs and popups. Voice buttons (`Shell::voice_button`, `OnHover`):
  sharing turns into a red stop button under the pointer, muted and deafened stay red and crossed out (the pointer only deepens the red; changed 2026-11-18),
  Leave grows into a red "Leave" pill. Divs cannot be scaled in gpui: grow their size instead (tiles, the
  rail's pills, badges), and draw rings as an overlay (`theme::border_width`) so the content never moves.
  Hard-coded corner radii are only for circles and pills; cards and popups use the theme's shape.
- **People (2026-11-10):** a left click on someone (voice rows, member lists, a name or picture in the chat)
  opens their profile beside it (`Shell::open_profile`); the right click opens their menu. Voice rows turn
  the accent colour with a hover background; names in the chat underline under the pointer.
- **Popups hug what was clicked (2026-11-10):** elements with a menu (`with_menu`) and the buttons that open
  popups (`hug_target`, the network line) note their painted bounds (`popup_trigger`). A popup sits beside the
  smallest of them under the click, 12 px away, on the side towards the window's middle (above or below
  when there is no room beside it), and follows the click only within that element (`modal::hug`).
- **Connection icon (2026-11-10):** a fixed 20 px slot between my name and the voice buttons: grey-green when
  connected, amber and pulsing after 3 s of reconnecting, red when the connection gave up or the session
  ended. Its tooltip lists each instance; a click reconnects. The sidebar only shows instances that need the
  person (sign in, failed), and the chat's "reconnecting" banner waits 15 s.
- **Voice on the rail (2026-11-10):** a server's icon gets a round chip with a speaker inside its top-right
  corner (so the rail's edge never cuts it) while people I can see are in its voice channels; the
  tooltip says how many. It is accent when I am there too (on the selected, accent icon only the speaker
  is), and pops in on a spring.
- **Scrolling:** `smooth_scroll.rs` catches wheel notches at the root and replays them as eased pixel
  scrolls, 1.25x the old distance. Touchpad pixel scrolls and Ctrl+wheel pass through.
- **Pictures:** server icons and avatars fill their shape (`ObjectFit::Cover`). Picking one opens the
  crop view (`crop.rs`), which uploads a square PNG of at most 512 px. Avatars, banners and server
  icons may be at most 10 MB (`PICTURE_MAX_BYTES`): the picked file, what is uploaded after
  cropping, and on the server. Other images keep `max_upload_mb`. The crop view opens on the first
  frame; an animation joins it once decoded.
- **A deleted channel** reopens the most recent remaining channel of its server (a voice channel only
  shows its room); Friends opens only when the server is gone.
- **Media (2026-11-08):** sound files get a waveform card and videos (mp4, mkv, mov, webm and more)
  an inline player with full view; nothing loads before play is clicked, and videos keep playing
  while the window is unfocused. `src/media/` runs `ffmpeg`/`ffprobe` as child processes (beside
  `hermes.exe` first, then the PATH) and hands them the file over a loopback-only HTTP
  address with range requests, so decrypted DM media never touches disk. Since 2026-11-11 a file that
  can be read in parts (plain, or sealed in pieces, see `docs/e2ee.md`) streams: each request FFmpeg
  makes is read from the server as it goes (`serve::Source::Remote`, parts of 256 KiB doubling to
  4 MiB, the next fetched while the last is written; up to 64 MB of parts kept per file). Others are
  still fetched whole first. The log says how long opening, the first sound and the first picture
  took. The sender's client
  measures a sound's waveform and a file's length (`Attachment.waveform`, `duration_ms`); without
  them the waveform grows in while the file plays. The release bundles BtbN's LGPL FFmpeg build;
  on Linux FFmpeg must be installed (`apt install ffmpeg`). Installers built by hand need it in
  `target/ffmpeg` (see `release.yml`).
  A paused file runs no FFmpeg process (they start again where it was), all files share one sound
  output, and a paused file is let go after 15 s off screen or when paused files pass 64 MB; it
  plays on from where it was. The whole waveform, and a video's bar, is a seek bar: a press starts
  scrubbing, followed by window-level mouse listeners (`media_scrub_catcher`, also over the full
  view), and the seek happens on release; moving the bar never starts a file. Ctrl+wheel over a
  file, the slider that opens from its speaker, or the plain wheel in the full view set one volume
  for all media (`voice.media_volume`, `voice.media_muted`); the plain wheel over a card scrolls the
  chat. The speaker and slider are one hover area, so moving between them cannot close it; they share
  a pill, the slider has a round knob, opens and closes on a spring, and its fill follows the volume on a
  quick spring. Downloading and
  the like are in each file's right-click menu. Video controls sit over the picture and fade
  2 s after the mouse rests while playing. Uploads larger than the server's `max_upload_mb` are
  refused before sending (the server checks too).
- **Selecting text (2026-11-10):** chat text (names, times, messages, code, file names) is selected by dragging, across
  messages too, in `selection.rs`. Like a browser: a double click selects a word (`word_at`: letters, digits and `_`,
  with `'` and `.` inside a word; a run of spaces; or one other character), a triple click the message's whole
  text, and a drag after either grows by words or whole messages. The line cursor shows over the whole message
  row; a press in its empty space starts from the message's nearest text. Names, links and files keep their
  own cursor. A drag or a multi-click never counts as a click. Ctrl+C or the message's menu copies it.
- **Message spacing (2026-11-10):** inside a group, the space between two messages is split evenly between
  them (`rows::padding`), so a hovered message's highlight has as much room under it as over it while what is
  shown keeps its spacing: a file that starts a message is as far from the file above as from text. While the
  pointer is over a file its message is not highlighted (`Shell::hovered_file`, set in `with_file_menu`).
  Do not use `block_mouse_except_scroll` for this: it makes the root's `on_mouse_up_out` fire and breaks menus.
- **Compact messages (2026-11-10):** `buffer.compact` lays a message out like Vencord's compact mode: the time
  in a column of its own, an 18 px picture, the name and the text on one line, wrapped lines under the text,
  files and reactions under the picture. Follow-ups show only their text, their time on hover.
- **Files:** every file has its own right-click menu (`Target::File`, `Shell::with_file_menu`). gpui calls an element's
  listeners last added first, so what stops an event is added before the menu (`with_own_menu`).
- **Encryption settings:** a server's settings have an Encryption tab (`key_settings.rs`) with its rules, each
  channel's, and who is waiting to be approved. See `docs/e2ee.md`.
- **Last online:** on the profile card, from `GET /users/{id}`; "Online now" while they look Online or DND.
- **Animated pictures:** GIFs, animated WebP and APNG move in the chat, as avatars, server icons and
  profile banners, only while the window has the focus. An `img` needs an `.id(...)` for gpui to
  step through its frames.
- **Level meter:** up to 60 fps, only while the bar moves; it holds `FineTimers` (1 ms Windows timers)
  while the Voice tab shows, since 15.6 ms timer ticks made it stutter.

- **Soft sounds (2026-11-09):** every sound stays at or under 660 Hz with a slow attack; a test holds them to it. Toasts
  are silent and the app plays its own ping.
- **Sizes:** `assets::icon` and the shell go through `theme::px`, so icons follow the UI scale.
- **Local data:** drafts, queued messages, notes and names given to people live sealed in the key ring file.

## Updates

The app updates itself (`hermes-client-core/src/updater.rs`, `hermes-gui/src/updater_window.rs`).
Each release carries `update-manifest.json`, signed with Ed25519, listing every file of the app
(path, size, SHA-256). Next to it are each file compressed (`<name>.zst`) and patches from the
last three releases' files (`<name>.from-<hash>.patch`, zstd with the old file as the
reference). The app hashes its own files and downloads nothing for unchanged ones (FFmpeg, usually
the TUI), a patch where it has a file a patch starts from, and the whole file otherwise. Everything
is checked against the signed hashes before use. Installing renames files in use to `*.old`.
A watcher (the old program, `--update-watch`) starts the new version (`--after-update`) and moves
the old files back if it does not come up within a minute.

One-time setup:
1. `cargo run -p hermes-client-core --bin make-update -- keygen` prints a key pair.
2. On GitHub, set the repository secret `UPDATE_SIGNING_KEY` to the first value and the repository
   variable `HERMES_UPDATE_PUBLIC_KEY` to the second. Keep the secret nowhere else.
3. The next tagged release builds the key into the app and publishes the update files. Builds
   without the variable only link to the release page, as before.

Testing locally (debug builds only): `make-update build --dir <new folder> --version 9.9.9
--out <served folder> --previous <old folder>` with a test key in `UPDATE_SIGNING_KEY`. Then run
the debug client from the old folder with `HERMES_UPDATE_URL=<url of the served folder>` and
`HERMES_UPDATE_KEY=<the test key's public half>`.

## Updates

The app updates itself (`hermes-client-core/src/updater.rs`, `hermes-gui/src/updater_window.rs`).
Each release carries `update-manifest.json`, signed with Ed25519, listing every file of the app
(path, size, SHA-256). Next to it are each file compressed (`<name>.zst`) and patches from the
last three releases' files (`<name>.from-<hash>.patch`, zstd with the old file as the
reference). The app hashes its own files and downloads nothing for unchanged ones (FFmpeg, usually
the TUI), a patch where it has a file a patch starts from, and the whole file otherwise. Everything
is checked against the signed hashes before use. Installing renames files in use to `*.old`.
A watcher (the old program, `--update-watch`) starts the new version (`--after-update`) and moves
the old files back if it does not come up within a minute.

One-time setup:
1. `cargo run -p hermes-client-core --bin make-update -- keygen` prints a key pair.
2. On GitHub, set the repository secret `UPDATE_SIGNING_KEY` to the first value and the repository
   variable `HERMES_UPDATE_PUBLIC_KEY` to the second. Keep the secret nowhere else.
3. The next tagged release builds the key into the app and publishes the update files. Builds
   without the variable only link to the release page, as before.

Testing locally (debug builds only): `make-update build --dir <new folder> --version 9.9.9
--out <served folder> --previous <old folder>` with a test key in `UPDATE_SIGNING_KEY`. Then run
the debug client from the old folder with `HERMES_UPDATE_URL=<url of the served folder>` and
`HERMES_UPDATE_KEY=<the test key's public half>`.

## Not done yet, on purpose
1. **Split panes:** after everything else. Their keybinds stay valid config but do nothing.

## Known risks
- **Share smearing:** a frame lost after the server (full decoder queue, timed-out stream) holds that share
  back until a keyframe; `read_screen` asks for one. Never hand the decoder frames after a gap.
- **Share video:** gpui's Windows `draw_surfaces` is an empty stub, so pictures are per-frame BGRA images freed
  with `drop_image`. See DECISIONS for the numbers. A debug build converts far too slowly to judge: measure release.
  Only a mostly still screen between two clients on one PC has been measured.
- **No software fallback:** gpui needs a D3D11 adapter. Test on the developer's laptop and over remote desktop.
- **Unstable API:** GPUI is pre-1.0, so keep exact pins.

## Building
Full workspace builds can run Windows out of memory (os error 1455): use `-j 4`.

## Not yet checked by ear or by a person
Real mic and speaker quality in a call, push-to-talk and the global mute key, the tray's menu, toasts and the taskbar
flash, real keyboard modifiers (Ctrl+K, Shift+Enter; the test helper can only send plain keys), file dialogs,
drag and drop, and how the sizes compare to Discord. Linux CI packages in `.github/workflows/ci.yml` are untested.

## How the window was tested
Run a scratch `hermes-server` copied elsewhere (port 7799, `registration.mode = "open"`, and
`privacy.require_channel_encryption = false` so plain REST messages are accepted), seed it over REST, start the
debug binary with `--config-dir <scratch>`, send mouse messages to its window with `PostMessage` (the real cursor does
not move), and screenshot with `PrintWindow` flag 2. Check both a large and a small window. A release client of the developer's may be running: select
windows by the path `target\\debug\\hermes.exe` only. Use `Start-Process`, and
never run a command that can wait on stdin (a stray `python -` hung a session for 20 minutes). gpui counts
clicks itself from plain `WM_LBUTTONDOWN`s and ignores `WM_LBUTTONDBLCLK`, so post plain downs for a double
click. Never screenshot a share
of the real screen: judge it from the logs and from pixel statistics of the tile.
