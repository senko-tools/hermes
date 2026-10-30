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
