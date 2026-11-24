# Patched crates

Copies of crates.io crates with small changes, used through `[patch.crates-io]` in the root
`Cargo.toml`. They are not workspace members, so `cargo clippy --workspace` and `cargo fmt --all`
leave them alone.

| Crate | Version | Change |
|---|---|---|
| `gpui-pre-windows` | 0.3.12 | `src/shaders.hlsl`, `polychrome_sprite_fragment`; `src/platform.rs`, `begin_vsync_thread` + `hermes_covered` |
| `gpui-pre-wgpu` | 0.3.12 | `src/shaders.wgsl`, `fs_poly_sprite` |
| `gpui-pre-linux` | 0.3.12 | `src/linux/wayland/window.rs`, `hermes_hideable` + `hermes_set_hidden`; `src/linux.rs`, `set_wayland_window_hidden` and `activate_wayland_window` (a second launch's activation token) |

**Why:** gpui draws every picture through one bilinear sampler, so a zoomed picture is blurred
instead of showing its pixels. The patch makes the picture (polychrome sprite) shader read the
texel under each pixel, clamped to the sprite's own atlas tile, whenever one texel covers more
than one screen pixel. Pictures drawn at their size or smaller sample as before. Text and icons
(monochrome sprites) are untouched. Search for `Hermes patch` to find the change.

**Covered windows (#66):** Hermes opens its windows with `inactive_frame_interval: None`, so a
window beside the focused one scrolls and animates at the display's rate instead of gpui's 30 fps
for inactive windows. So that a window nobody can see doesn't draw at that rate, the vsync thread
in `platform.rs` does not invalidate a minimized window, and invalidates a window that is not in
front and lies wholly under one other opaque window only every 250 ms (`hermes_covered`). Being
covered by several windows together is not detected; such a window draws as usual.

**Hiding to the tray on Wayland:** gpui has no way to hide a window, and on Wayland only the
app's own toolkit can. `set_wayland_window_hidden` (called by `desktop/window.rs` with the
window's `wl_surface`) destroys the toplevel's decoration, `xdg_toplevel` and `xdg_surface` and
attaches no buffer: the window leaves the screen and the taskbar, while the surface, the renderer
and the app stay. Its frame loop goes back to unconfigured, so nothing draws while hidden. Showing
makes a new `xdg_surface` and `xdg_toplevel` with the same title, app id, sizes, maximized state
and decoration mode; the first configure draws it again, as when it opened. The visibility change
reaches the window later on the executor (the app is mid-update when it hides). Search for
`Hermes patch`.

**On a gpui upgrade:** copy the new versions from `~/.cargo/registry/src/*/gpui-pre-windows-<v>`,
`gpui-pre-wgpu-<v>` and `gpui-pre-linux-<v>` here (drop `.cargo-ok` and `.cargo_vcs_info.json`), apply the same changes
to both shaders and the vsync thread again, and update the versions above. If the versions don't
match what `Cargo.lock` wants, cargo warns that the patch is unused and the plain crates.io versions are
used instead.
