# Patched crates

Copies of crates.io crates with small changes, used through `[patch.crates-io]` in the root
`Cargo.toml`. They are not workspace members, so `cargo clippy --workspace` and `cargo fmt --all`
leave them alone.

| Crate | Version | Change |
|---|---|---|
| `gpui-pre-windows` | 0.3.12 | `src/shaders.hlsl`, `polychrome_sprite_fragment`; `src/platform.rs`, `begin_vsync_thread` + `hermes_covered` |
| `gpui-pre-wgpu` | 0.3.12 | `src/shaders.wgsl`, `fs_poly_sprite` |

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

**On a gpui upgrade:** copy the new versions from `~/.cargo/registry/src/*/gpui-pre-windows-<v>`
and `gpui-pre-wgpu-<v>` here (drop `.cargo-ok` and `.cargo_vcs_info.json`), apply the same changes
to both shaders and the vsync thread again, and update the versions above. If the versions don't
match what `Cargo.lock` wants, cargo warns that the patch is unused and the plain crates.io versions are
used instead.
