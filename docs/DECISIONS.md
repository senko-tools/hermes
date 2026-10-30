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

## 2026-10-30: rustls uses the `ring` crypto provider

- **What:** the workspace `rustls` and `quinn` entries disable default features and enable `ring`
  (rustls: `std`, `ring`, `logging`, `tls12`; quinn: `runtime-tokio`, `rustls-ring`, `log`).
- **Why:** rustls 0.23 defaults to `aws-lc-rs`, while quinn and rcgen default to `ring`. With both
  compiled in, rustls cannot pick a provider automatically, and `aws-lc-sys` is a heavy C/CMake
  build on Windows and musl. `ring` alone covers TLS 1.3 for QUIC and HTTPS.

## 2026-10-30: Spike A (audio loopback) passed; "added latency" means our pipeline only

- **Result** (developer's Windows PC, WASAPI shared mode, Yeti X in, Realtek out, both 48 kHz f32,
  480-frame periods): the full chain cpal → nnnoiseless → Opus 20 ms/64 kbps/FEC/DTX → decode → cpal
  works with libopus built statically. Estimated end-to-end latency 77–82 ms, of which:
  - devices (WASAPI engine + drivers): ~14 ms input + ~32 ms output ≈ 46 ms;
  - Hermes pipeline (ring buffers, frame buffering, resampler/Opus/denoiser delays): ~31–36 ms.
  DTX works (6.9–11.9 kbps in silence, ~53 kbps while talking); RNNoise voice probability 0.84 while
  talking; no sustained underruns.
- **What:** the Phase 0 criterion "< 60 ms added latency" is interpreted as latency added by Hermes,
  excluding the OS audio devices, which every voice app pays. The spike counts as passed.
- **Why:** cpal only offers WASAPI shared mode with fixed 10 ms periods, so the device part cannot be
  reduced from our side; the part we control is well under 60 ms.
- **Follow-up for `hermes-audio`:** playback sounded quiet on the developer's setup; the spike now has
  a dBFS level meter and `--gain` to investigate. Consider the AGC from section 12.5 (step 4) if quiet
  microphones turn out to be common.

## 2026-10-30: Spike B (QUIC datagrams) passed

- **Result:** `quic-echo server` (static musl binary) on hermes.senko.tools, bound to `[::]:7700`
  (dual-stack, `net.ipv6.bindv6only = 0`), client on the developer's Windows PC at home, 50 datagrams/s
  of 200 bytes for 60 s, certificate pinned by SHA-256:
  - IPv4: 3000/3000 received, 0 % loss, no reordering, RTT min 5.3 / avg 5.6 / p95 5.7 / max 6.3 ms,
    jitter 0.08 ms, max datagram size 1414 bytes.
  - IPv6: same results (RTT avg 5.6 / p95 5.8 / max 8.2 ms, jitter 0.10 ms) after the developer fixed
    IPv6 on the home network (the first failure was the client network, not the server).
- **What:** no change to the plan; QUIC datagrams over one UDP port are confirmed for voice. A 20 ms
  Opus frame (~160 bytes at 64 kbps) plus the 11-byte header fits well inside 1414 bytes.
- **Notes for the real server:** on a dual-stack socket IPv4 peers appear as `::ffff:a.b.c.d`; convert
  them to plain IPv4 (`to_canonical()`) before per-IP rate limits and logging.

## 2026-10-30: `windows` crate for the push-to-talk polling fallback; iced-ui spike is Windows-only

- **What:** added `windows = "=0.62.2"` (feature `Win32_UI_Input_KeyboardAndMouse`) to the workspace,
  used only on Windows by `spikes/iced-ui --poll` to read `GetAsyncKeyState` every 10 ms. The `spikes`
  workflow does not build `iced-ui` for `x86_64-unknown-linux-musl`.
- **Why:** PLAN.md section 21 names this fallback (`windows` crate) but section 6 does not list the crate;
  approved by the developer. The version is the one cpal already pulls in, so nothing new is downloaded.
  `global-hotkey` 0.9 on Windows uses `RegisterHotKey` (the key is taken away from every other app while
  registered, keyboard keys only) and detects release by polling `GetAsyncKeyState` every 50 ms; the
  spike compares both. A static musl build cannot load the system GUI libraries, so it would be useless.
