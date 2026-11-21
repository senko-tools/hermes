# plan1.md — improvements.md round

## Context
`improvements.md` (repo root) lists 9 GUI improvements. They are done in a **new chat**, which only
has this file and the repo. Usage left: **14% of the week until 2026-11-21 12:00**, then it resets.
Order is by value per token: cheap, clear items first, then #8; the rest after the reset.
Every item is committed on its own as soon as it passes, and its box below is ticked in this file,
so a usage cut loses nothing and the next chat resumes at the first unticked box.

Decisions made:
- **#2 and #5 are deferred** (stay undone this round). Don't start them.
- #8 removes **only the profile/nickname switch** in profile editing. "Change nickname", `/nick`
  and showing nicknames stay.

## How to spend the budget (rules for the executing chat)
- No subagents, no Explore/Plan agents: they re-read everything cold.
- Grep first, then Read with offset/limit; never read whole 2000-line files. `docs/codebase.md` is the map.
- Per item: `cargo clippy -p hermes-gui --all-targets -- -D warnings` + `cargo test -p hermes-gui`
  (add `-p hermes-client-core` when it changed). Full `cargo fmt --all`, workspace clippy and
  `cargo test --workspace` once per batch, before the last commit of the batch.
- At most one screenshot per visual item, at the end of it (scratch server + debug client, see
  memory gui-testing-setup). Skip screenshots if usage looks tight; report them as "not checked by eye".
- Don't stop to ask (memory keep-going-until-done); if usage runs out, wait for the reset and continue.
- Commit format `gui: ...` / `client-core: ...`; push nothing (the developer pushes).
- Sonnet is fine for #1/#6/#3 if usage runs low (`/model`); use opus for #8 and #4.

## Batch A — today, within the 14% (in this order)

- [x] **#1 Text selection by glyph midpoint** — `crates/hermes-gui/src/selection.rs`.
  `spot_under` / `spot_nearest` (≈l.156–180) use `layout.index_for_position(at)`, which gives the
  glyph *start* when the pointer is anywhere on it. Add one helper `boundary_at(layout, at)`: take
  that index, get the x of it and of the next char boundary (`position_for_index`), and return the
  next boundary when `at.x` is past the midpoint. Use it in both functions so both drag directions
  behave the same. `over_text` stays as is. Unit-test the rounding as a pure fn (pass the two x
  values in).
- [x] **#6 Date separators** — `channel/mod.rs::divider` (l.623) already draws line–date–line with
  `px(16)`. Check in the client what's actually off (likely: not inside the same content
  width/insets as the messages, or lines too close to the edges). Fix it so the lines sit inside the
  chat content area with the date centred; the "New messages" divider (l.643) keeps its own look.
- [x] **#3 Channel reordering** — sidebar drag (`sidebar.rs::with_drag` ≈l.1627, `drop_item`
  l.1671) decides before/after from the *drag direction* and `view::move_channel` makes the dragged
  channel "take the target's place", so some places (after the last channel of a list, right under a
  neighbour) are awkward or unreachable. Change to: the **pointer's half of the hovered row**
  decides (top half = before, bottom half = after; on a category row: middle = into it).
  - `hermes-client-core/src/view.rs`: `move_channel(store, space, moved, onto, place: Place)` with
    `enum Place { Before, After, Into }`; `place_channel` inserts at `at` or `at + 1`. Update the
    existing tests (≈l.1020–1080) and add "after the last" and "into a category".
  - `sidebar.rs`: track the hovered half (`on_drag_move` / `drag_over`, current target + place in a
    `Cell` on the shell), draw one accent insertion line (2 px, rounded ends) at that edge instead of
    tint+border, and pass the place to `drop_item`.
  - `space_settings.rs` Channels tab (≈l.1711–1780): same drag + `Place`; keep the up/down step
    buttons only if they still add something.
- [x] **#8 Profile popup with inline editing** — remove the Edit profile form
  (`form.rs` `Purpose::EditProfile` ≈l.114/774/1492/1539 and the profile/nickname switch ≈l.185–197,
  693–727); `Action::EditProfile` (`actions.rs:99`, `command_bar.rs:425`) opens my profile popup in
  edit mode instead. Find the popup with `grep -rn "fn render_profile" crates/hermes-gui/src`.
  - edit mode = same layout, larger; banner and avatar clickable (reuse `crop.rs`), name and bio
    become inline text inputs on click; small pencil icon + subtle hover tint on editable parts.
  - status row: Online / Idle / Do Not Disturb / Invisible with dots in `presence.online/idle/dnd/
    offline` (theme.rs; missing key → dark theme default), changed with one click.
  - save per field on Enter/blur (one PATCH), no Save/Cancel form, no explanatory text.
  - speed: open from cached store data at once; never block on a fetch; fetch in the background.

  → End of Batch A: fmt + workspace clippy + workspace test, commit.

## Batch B — after the reset (2026-11-21 12:00)

- [ ] **#9 Settings fill the space** — `settings/page.rs` / `settings/mod.rs`: drop the fixed
  max-width that leaves empty space; content column takes the width with sensible padding, capped
  only for very wide windows; rows keep the control pinned right. Before #7, so #7 is designed in
  the final layout.
- [ ] **#7 Settings controls** — replace radio-style choices with a visual control built in `ui/`
  (controls come from `hermes-gui/src/ui/`): segmented control / card picker with icon + label and
  accent selected state. Status shows presence dots in `presence.*` colours; date format shows a live
  example; style choices get small previews where cheap. Switch all `choice!`-enum rows in
  `settings/` in one pass.
- [ ] **#4 Startup speed** — measure first: `tracing` timestamps for process start, window created,
  first frame painted, each account's REST login/ready, gateway READY, first space painted. Then fix
  what the numbers show. Suspects: window shown before its first paint (create hidden or with an
  opaque background, show after the first frame); blocking work on the main thread before
  `open_window` (fonts, themes, image decode, config/keys); accounts connected one after another
  instead of concurrently; REST calls that could run in parallel or later. Report before/after timings.

  → End of Batch B: fmt + workspace clippy + workspace test, commit, release exe build.

## Deferred (not this round)
- #2 Animate images when unfocused (setting).
- #5 Search grouped by category.

## Verification
- The three commands from CLAUDE.md pass at the end of each batch.
- New unit tests: midpoint rounding (#1), `move_channel` with `Place` (#3).
- Visual check on Windows with the scratch server + debug client (memory gui-testing-setup);
  anything not looked at is reported as "not checked by eye".
- Developer test steps at the end: select first/last letters of a word from both sides; drag a
  channel to the very bottom / into a category; open my profile, change name, avatar, banner, status;
  date separators in a long chat; settings at narrow and wide window sizes; time a cold start.
