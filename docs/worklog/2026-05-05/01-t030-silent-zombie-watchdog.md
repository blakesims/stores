# T030 Silent Zombie Watchdog

**Date:** 2026-05-05
**Type:** note

## Summary

T030 closes L062: the daemon watchdog now catches drive subprocesses that die silently after the dispatch lock is closed. Previously the watchdog's `WHERE finished_at IS NULL` filter on `dispatch_locks` meant any post-spawn crash left the row stuck in `executing`/`planning` indefinitely. Fix is a tasks-table scan with a grace window plus a structured suffix on `blocked_reason`, end-to-end verified against the real `stores` binary.

## Details

### L062 reproduction shape

The L062 demonstration: spawn `stores tasks drive <id>`, kill the PID immediately. The auto-drive subscriber had already called `mark_claim_finished` on the dispatch_lock when handing off to the subprocess, so the existing open-lock sweep filter (`finished_at IS NULL`) didn't see the row. Result: row sat in `executing` forever, no transition_history entry, no operator signal.

Two sub-shapes both needed coverage:
- **`silent_zombie_pid_dead`** — `drive_pid` recorded, but `kill -0 <pid>` fails (process gone).
- **`pid_never_recorded`** — claimed_at set, drive_pid still NULL past the grace window (subprocess died before recording PID).

### Fix shape (tasks-table scan + grace window)

`scan_zombie_tasks()` (Phase 2) inspects the tasks table directly for in-cycle rows (`planning..code_review`) — independent of dispatch_lock state. For each candidate:
1. If `drive_pid` is set: `kill(pid, 0)` to liveness-check.
2. If `drive_pid` is NULL: check `claimed_at + ZOMBIE_GRACE_SECS < now`.
3. Both paths apply `drive_started_at + ZOMBIE_GRACE_SECS < now` so a warming-up drive is not flipped.

Dead/timed-out rows are flipped via `mark_drive_failed` to `blocked` with a structured `blocked_reason`.

### Structured reason suffix convention

Phase 3 plumbed `detection_reason: Option<&str>` through `fire_mark_drive_failed`. The suffix appears after a colon: `drive_failed:<reason>`. Current values:
- `drive_failed:silent_zombie_pid_dead`
- `drive_failed:pid_never_recorded`
- `drive_failed` (bare — open-lock dead-PID path; backwards compatible with pre-T030 callers)

This makes the failure mode mechanically distinguishable in transition_history without parsing free-form text. Future watchdog reasons (e.g. L071's cooperative-abort path) should follow the same `drive_failed:<snake_case_reason>` convention.

### Real-binary e2e (Phase 4)

`tests/drive_silent_zombie_e2e.rs` uses `CARGO_BIN_EXE_stores` — not the `STORES_DRIVE_CMD` shell stubs that masked L062 in T022. The test seeds the closed-lock + dead-PID shape via SQL, runs `stores agents run --once` (new flag, sets `max_iters=1`, no daemonize), and asserts the row is now `blocked` with `drive_failed:silent_zombie_pid_dead` and a single transition_history row. The `--once` flag is the orthogonal substrate primitive for any test or operator who wants a single watchdog tick without spawning a daemon.

## Follow-ups

- **L071** is the paired observation — drive aborts gracefully on runner exit=1 (rate limit) but doesn't notify substrate. T030's watchdog doesn't catch it because the drive process is still alive (just exiting cleanly with the row left in `executing`). Different fix: cooperative abort needs to flip the row itself before exiting, or write a sentinel the watchdog can detect. Now top of the highest-leverage list.
- Worklog file is `01-t030-silent-zombie-watchdog.md` (lowercase, script-enforced kebab-case) — AC5.2's `*T030*silent-zombie*.md` glob pattern was illustrative; the `./new-note.sh` slug rule wins per `docs/worklog/CLAUDE.md`.
