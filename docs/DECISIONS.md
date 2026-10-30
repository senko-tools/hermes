# Decisions

Log of deviations from `PLAN.md` and of choices the plan left open.
Newest entries at the bottom. Each entry: date, what changed, why.

---

## 2026-10-30: Opus binding is `opus`, not `audiopus`

- **What:** PLAN.md section 6 lists "`audiopus` (or `opus`)". The workspace uses `opus` 0.4.0.
- **Why:** `audiopus` has had no release since 2021 (0.2.0); `opus` is actively maintained (0.4.0, August 2026).

## 2026-10-30: Minimum supported Rust version is 1.99

- **What:** `rust-version = "1.99"` in the workspace; CI builds with the latest stable toolchain.
- **Why:** 1.99 was the current stable release at project start; the plan does not name an MSRV.

## 2026-10-30: Top-level `spikes/` folder for Phase 0 spikes

- **What:** Phase 0 spikes live in `spikes/<name>/` as standalone binary crates (workspace members,
  `publish = false`). Real crates never depend on them. This folder is not in the section 5.2 layout.
  `.github/workflows/spikes.yml` (manual `workflow_dispatch`) builds them for `x86_64-pc-windows-msvc`
  and `x86_64-unknown-linux-musl` and uploads them as artifacts.
- **Why:** spikes are throwaway experiments that must be testable on the developer's Windows machine
  without a Rust toolchain. Keeping them in the workspace means they share pinned versions and
  CI checks (fmt, clippy, test).
- **Note:** the musl build links a static alsa-lib without `dlopen`, so ALSA plugins (PulseAudio and
  PipeWire bridges) are not available to it. On such desktops it may only see raw `hw:` devices.
  The Windows build is the one that matters for Spike A.
