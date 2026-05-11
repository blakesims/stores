# T148 Autopsy Er Daemon Engine

**Date:** 2026-05-11
**Type:** note

## Summary

T148's external-review churn was not one single daemon bug. It was a stack of three unsafe surfaces:

1. `external_reviews create-pending` is an operator recovery escape hatch with no active-review/fresh-head guard. It can mint a second pending ER while one for the same task/head is already running or already current.
2. Engine-runner Layer 1 only treats `pending/running/passed/revise/tooling_held` rows as active, so when REVISE moves the task back to `executing`, future `complete -> in_review` cycles legitimately mint a new ER; manual `create-pending` during those cycles can double-mint.
3. Task runner liveness metadata is best-effort. `drive_pid` and current-run markers can be stale after death (or left on terminal rows); watchdog/engine-runner decisions rely on `kill(pid, 0)`, dispatch locks, and fresh marker timestamps, with known PID-reuse limitations.

The accept guard did its job for stale passed reviews: it requires a non-superseded PASS whose `head_sha` equals the current task-worktree HEAD. Manual `import-pass` also checks supplied `base_sha/head_sha` against the task workspace before inserting. The manual recovery path that spawned/blocked more reviews was therefore mostly `create-pending`, not `import-pass`.

## Files Retrieved

1. `src/flow/engine_runner.rs` (lines 303-666) - Layer 1 ER backfill and Layer 2 pending-dispatch reconciler.
2. `src/flow/engine_runner.rs` (lines 840-1200) - task scanner liveness/dispatch-lock/current-run-marker logic and auto-drive capacity.
3. `src/flow/builtins/external_review.rs` (lines 1-360) - external-review `run()` CAS, cap check, pending→running transition, preflight, terminal recording entrypoint.
4. `src/flow/builtins/external_review.rs` (lines 360-880) - retry promotion, terminal PASS/REVISE/tooling-held recording, REVISE task transition freshness guard.
5. `src/handlers/external_reviews.rs` (lines 1-360) - git preflight/rebase/head/base collection and input bundle.
6. `src/handlers/external_reviews.rs` (lines 330-590) - `import_manual_pass` head/base validation and PASS row insertion.
7. `src/handlers/external_review_run.rs` (lines 1-35) - manual `external_reviews run` wrapper.
8. `src/cli/dispatch.rs` (lines 309-403) - `run`, `create-pending`, and `import-pass` CLI handlers.
9. `src/cli/dynamic.rs` (lines 1136-1184) - command definitions/help for recovery verbs.
10. `src/handlers/agents_run.rs` (lines 1271-1850) - daemon `poll_once_with_guard`, transition subscriber dispatch, external-review cap gate, watchdog, and engine-runner tick wiring.
11. `src/handlers/agents_run.rs` (lines 1851-2143) - starting-line seeder and `try_claim` dispatch-lock insertion.
12. `src/tui/daemon.rs` (lines 1-150) - daemon pidfile/project liveness probe.
13. `src/handlers/transition.rs` (lines 1088-1220) - T2/T3 accept precheck requiring current-head external-review PASS.
14. `tests/external_review_daemon.rs` (lines 370-1110) - Layer 2, retry, cap, and concurrent `run()` CAS tests.
15. `tests/external_review_acceptance.rs` (lines 1-260) - acceptance precheck and manual import-pass tests.
16. `src/flow/freshness.rs` (lines 1-130) - generic freshness model (review/test/base/head), relevant as contrast to ER accept checks.
17. `src/flow/builtins/integrate.rs` (lines 963-990) - integration lane selecting latest passed ER and superseding stale review.

## Key Code

### Engine Layer 1 creates ER rows whenever T2/T3 task is `in_review` and no active ER exists

`src/flow/engine_runner.rs` lines 315-333 selects T2/T3 in-review tasks, then counts active rows:

```rust
"SELECT COUNT(*) FROM external_reviews \
 WHERE task_id=?1 \
   AND status IN ('pending','running','passed','revise','tooling_held')"
```

If count is zero, lines 338-377 insert a new `pending` ER and a `create-external-review` transition-history row.

Implication: the active set includes `revise`, so Layer 1 should not mint another ER while old REVISE rows remain non-superseded. But manual recovery can bypass this, and any supersede/recovery flow that removes active rows makes the next in-review pass mint again.

### Engine Layer 2 dispatches all pending ERs with no live dispatch lock

`src/flow/engine_runner.rs` lines 512-524 enumerate candidates:

```sql
SELECT er.id, er.display_id, er.task_id
FROM external_reviews er
WHERE er.status = 'pending'
  AND NOT EXISTS (
    SELECT 1 FROM dispatch_locks dl
    WHERE dl.store = 'external_reviews'
      AND dl.row_id = er.id
      AND dl.agent_name = 'external-review'
      AND dl.finished_at IS NULL
  )
ORDER BY er.id
```

Then lines 537-603 compute cap budget and call `external_review::run()` for each eligible pending row. This is good for daemon recovery, but it means any manually-created duplicate pending ER is treated as real work.

### `external_review::run()` is safe per row, not per task/head

`src/flow/builtins/external_review.rs` lines 89-162 open `BEGIN IMMEDIATE`, require the row is still `pending`, cap-check in the transaction, and CAS `UPDATE ... WHERE status='pending'`. This prevents two callers from dispatching the same ER row twice.

It does **not** prevent two different pending ER rows for the same `task_id/head_sha` from both running. The cap check counts global `running` rows, not task/head uniqueness.

### `create-pending` has no guard

`src/cli/dispatch.rs` lines 317-363 directly inserts a new pending ER using supplied `--base-sha` and `--head-sha`, computes `attempt = MAX(attempt)+1`, and writes transition history with actor note `manual create-pending recovery`.

Missing checks:

- supplied `head_sha` equals current task workspace `HEAD`;
- supplied `base_sha` equals current `main`;
- no non-superseded `pending/running/passed/revise/tooling_held` row already exists for this task/head;
- no `running` ER already exists for the task;
- invoker grounding: code uses `ai_autonomous` as `created_by` and does not require `ai_with_human`, despite command help calling it operator recovery.

### `import-pass` is guarded

`src/handlers/external_reviews.rs` lines 368-409 validate runner/transcript, load the task workspace, resolve current `HEAD`, and fail if supplied `head_sha` does not match. Lines 410-417 do the same for current `main` and supplied `base_sha`. Lines 421-461 then insert a terminal `passed` row. This is why manual PASS import did not itself create stale-head acceptance risk.

### Accept guard is current-head based

`src/handlers/transition.rs` lines 1088-1184 require T2/T3 accept to find a non-superseded ER for the task whose `head_sha` equals current task-worktree HEAD and whose status/verdict is `passed/PASS`. If the latest ER is for a different head, it bails with `stale external review head`.

This guard rejects stale PASS rows, but it does not prevent duplicate ER creation earlier.

### REVISE transition has a head guard, with a blocked-task exception

`src/flow/builtins/external_review.rs` lines 603-652 only fires task `submit-external-review` when `review_head == current_head`, unless task status is `blocked` and `ensure_blocked_er_reconcile_allowed` passes. Lines 655-697 permit blocked reconciliation only for drive/watchdog blocked reasons and still require reviewed head equals current head.

### Runner liveness is best-effort and known stale

`src/flow/engine_runner.rs` lines 894-904 explicitly document that `drive_pid` liveness uses `kill(pid, 0)` and does not protect against PID reuse. Lines 901-929 hold tasks on `live_drive_owner`, `live_dispatch_lock`, or `live_runner_marker`. Lines 1081-1104 treat unfinished dispatch locks with no/positive live pid as live. Lines 1120-1150 treat `.stores/runs/current-*.json` marker as live if status is `running` and timestamp is within 15 minutes.

Current DB evidence: `tasks.T148` is `integrated` but still has `drive_pid=705579`; `kill -0 705579` is dead. That is stale terminal metadata, even if not currently harmful because integrated tasks are not scanned for redrive.

## Architecture

External review has two dispatch paths:

1. Transition subscriber path: `agents_run::poll_once_with_guard` scans `transition_history` for `external_reviews: '' -> pending`, cap-checks, `try_claim`s a dispatch lock, runs `builtin:external-review`, then marks the claim finished (`src/handlers/agents_run.rs` lines 1271-1510).
2. Engine-runner Layer 2 path: each daemon tick directly scans `external_reviews.status='pending'` rows without a live unfinished external-review dispatch lock and calls `external_review::run()` (`src/flow/engine_runner.rs` lines 445-638). This was added to recover rows missed by seeding/restarts.

`external_review::run()` is the shared per-row execution core. It prepares git state, runs Codex/manual runner, records terminal status, and for REVISE feeds the task back from `in_review` to `executing` via framework transition. This creates normal review/repair loops: complete -> in_review -> ER -> REVISE -> executing -> code_review -> complete -> in_review -> new ER.

Manual recovery verbs sit outside that invariant:

- `external_reviews create-pending` inserts a new pending row directly; daemon Layer 2 then sees and runs it.
- `tasks recover-stale-base` supersedes stale tooling-held rows and creates a fresh pending row after rebasing.
- `external_reviews import-pass` inserts a passed row after validating current base/head.
- `external_reviews run` runs exactly one existing ER row and avoids daemon sweeps/watchdog.

## T148 DB Evidence

Read-only DB inspection showed T148 accumulated ER409, ER410, ER411, ER412, ER414, ER417-ER429. Notable rows:

- ER409: framework Layer 1 backfill at 09:40:40, tooling-held.
- ER410: manual/CLI `create-external-review` at 09:41:09 with actor note `task_row_id=148`, tooling-held.
- ER411/ER414: `recover-stale-base` rows, superseding earlier tooling-held attempts.
- ER417-ER427: many manual `create-pending recovery` and framework `task_row_id=148` attempts during repeated REVISE/repair cycles.
- ER422 and ER423 were created at the same minute for different heads: ER422 by non-manual `task_row_id=148`, ER423 by `manual create-pending recovery`, both dispatched. This is the clearest duplicate-creation pattern.
- ER424 and ER425 repeated the same shape: framework mint at 12:15:30, manual create-pending at 12:15:35, both dispatched.
- ER426 and ER427 repeated it again at 12:18:44/12:18:48.
- ER428 passed at head `d8a89b421...`; then ER429 was imported as manual PASS at current head `244af103...`; accept followed at 13:11:45.

Current worktree evidence:

- `/home/blake/repos/experiments/stores-T148-auto-promoted-l568` HEAD is `244af1039a02937855b62bd539de40ffb24caf84`.
- `main` there is `d8ff8d01ad79603b69ace3c38ef0120b5852fb11`.
- T148 accepted with ER429, whose `head_sha` equals current HEAD; ER428's PASS was stale by accept time.

## Suspected Fixes

1. Harden `external_reviews create-pending` by reusing `import_manual_pass`-style workspace validation: resolve task workspace, checkout task branch if needed, require supplied/current `head_sha` and `base_sha` match, or compute them internally instead of accepting operator-provided SHAs.
2. Add a task/head uniqueness guard before `create-pending`: fail if any non-superseded `pending/running/passed/revise/tooling_held` row exists for the task and either has the same `head_sha` or is `pending/running` without a head yet. If override is truly needed, make it an explicit `--force-supersede` path that supersedes older active attempts first.
3. Consider moving `create-pending` implementation out of ad hoc `cli/dispatch.rs` SQL into a handler shared with Layer 1/recover-stale-base so active-set and transition-history semantics stay consistent.
4. Make `create-pending` require `ai_with_human`/`human` like `recover-stale-base`, not silently `created_by='ai_autonomous'`.
5. Add tests for duplicate task/head rows: one framework-pending plus immediate manual `create-pending` should fail or supersede, not create ER+1. Existing tests prove per-row CAS, not per-task/head uniqueness.
6. Add terminal cleanup for `drive_pid`/current-run metadata on task terminal transitions (accepted/integrated/abandoned/closed) so stale PIDs are not retained. The current scanner excludes terminal statuses, but stale metadata confused manual recovery and status interpretation.
7. Add a stale running-ER watchdog: if `external_reviews.status='running'` has no live dispatch lock/runner process and exceeds a timeout, move to `tooling_held` with retry or mark dispatch lock terminal error. Current watchdog is task/auto-drive focused, not ER-running focused.
8. In Layer 2, before dispatching a pending ER, optionally skip/hold if another ER for the same task is `running`; this would avoid parallel reviews of the same task even when duplicate pending rows exist.

## Start Here

Start with `src/cli/dispatch.rs` lines 317-363. That is the smallest high-leverage bug surface: manual `create-pending` directly inserts duplicate pending reviews and bypasses the current-head/current-base/active-attempt guards that already exist in `import-pass` and accept precheck.

## Follow-ups

- Add a regression test in `tests/external_review_acceptance.rs` or `tests/external_review_daemon.rs` for framework backfill + manual create-pending duplicate prevention.
- Add a read-only health query or TUI warning for terminal tasks with stale `drive_pid` and for ER rows stuck `running` without a live lock/process.
