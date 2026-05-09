# Manual Engine Control North Star

**Date:** 2026-05-09
**Type:** iteration-3 worklog
**Status:** active

## Seed Goal

Manually drive the stores engine with Blake in the loop, while preventing the engine from picking up random tasks or spawning uncontrolled work.

## North Star

The engine can safely sustain up to 5 concurrent tasks with high operator trust: no surprise work selection, no stale/zombie ambiguity, no duplicate/remint cascades, and enough observability that Blake can tell at a glance what is running, why, what is blocked, and what action is safe next.

Manual control is the recovery mode, not the destination. We use it now to stop entropy, clean the queue, identify missing control primitives, and then restart automation only when constrained lanes and observability make high-throughput operation boring.

## Operating Mode

- **Daemon stays stopped** unless explicitly restarted for a narrow, predeclared action.
- **No broad `stores agents run`** while queue shape is unstable.
- **No blind `stores tasks drive` loops** on arbitrary rows.
- **One row at a time** unless Blake explicitly batches a mechanically-identical action.
- **Prefer read/inspect → propose → execute** for any task lifecycle move.
- **Keep useful running work only when already started by Blake/engine-op; do not spawn new work opportunistically.**
- **No raw-SQL writes.** Read-only SQL is allowed for diagnosis.

## Phased Approach

### Phase 0 — Freeze + manual command center

Goal: stop entropy while regaining trust.

- Keep daemon stopped.
- Do not run `stores agents run --once` while queue shape is unstable; startup sweeps can mutate many rows.
- Use only selected direct verbs or selected single-row drives after inspection.
- Maintain this worklog as the live queue board.
- Classify every active row into: accept-ready, review-needed, abandon/remint candidate, hold due to known bug, or safe-to-drive manually.

Exit criteria: no random pickup; every transition is intentional.

### Phase 1 — Queue cleanup to trusted baseline

Goal: reduce WIP to a small, legible set.

Current first-pass targets:

- Accept-ready: `T125` (ER357 PASS), `T127` (ER358 PASS), pending Blake/U3 grounding.
- Review-needed: `T124` (in_review with wrap; no ER row found), needs manual codex/review or narrow ER path.
- Decide: `T126` (substantive FAIL) — salvage revise vs abandon+sharpen.
- Hold: `T123` (I033 contamination risk), `T116` (T1 silent_zombie; wait until WIP low), `T108` (contract drift / repeated NEEDS_WORK).
- Queue-curation candidates: `T134`–`T137` (startup-sweep remints with zero cycles).
- Unknown/stale status: `T128` was executing, drives were reaped; inspect before action.

Exit criteria: no ambiguous active rows; no zombie/remint clutter.

### Phase 2 — Safe manual-drive primitives

Goal: make manual engine driving first-class.

Needed procedures or verbs:

- `in_review + ER PASS` → accept path.
- `in_review + no ER` → manual codex or narrow external-review dispatch path.
- `blocked + useful code` → safe resume/manual submit-review path.
- `planning + duplicate/remint` → abandon path.
- A narrow external-review dispatch path that does not run daemon startup sweeps.
- A single-row drive path that cannot select unrelated rows.

Exit criteria: Blake can advance one selected row without waking the whole machine.

### Phase 3 — Observability before throughput

Goal: make state obvious without raw SQL/grep.

Priority from `docs/engine-health.md`: watch/actionability truth. `stores watch` should show pipeline-shaped buckets and distinguish internal `code_review`, final `in_review`, and external review. It should expose daemon state, drive pids, stale binary state, ER queue, cap-held rows, held reasons, and safe next action.

Exit criteria: operator can answer "what is running, why, and what should I do next?" at a glance.

### Phase 4 — Front-door/remint guardrails

Goal: prevent startup sweeps and front-door gaps from manufacturing surprise WIP.

Priority from `docs/engine-health.md`: native intake triage / draft drain, auto-resolve cleanup, auto-promote re-fire safety, and maintenance-mode lane controls such as `--no-startup-sweeps`, `--review-only`, `--no-auto-drive`, or explicit `stores engine pause/resume --lane ...`.

Exit criteria: daemon can run constrained lanes without surprise queue mutation.

### Phase 5 — Reliability hardening for 5-wide throughput

Goal: make `drive.max_parallel=5` mean five useful, non-conflicting tasks.

- stale/zombie handling boring and visible
- external-review stale-base recovery clear/automatic
- duplicate/remint detection fail-loud
- file-overlap scheduling before concurrent execution
- priority scheduler after the queue is curated

Exit criteria: five-wide work does not create rebase/stale/duplicate debt.

### Phase 6 — Metrics and empirical tuning

Goal: choose runners/models by evidence.

Capture and expose role, runner/model, prompt/template hash, duration, exit, tokens/cost where available, and pass/revise/fail outcomes across `agent_runs` and `external_reviews`.

Exit criteria: throughput tuning becomes empirical, not vibes.

## Immediate Control Objective

Complete Phase 0 and begin Phase 1: keep the engine frozen, verify the live queue, then make deliberate row-by-row progress with Blake.

Candidate patterns to verify from source/CLI before use:

1. Direct task verbs only (`accept`, `reject`, `abandon`, `submit-review`, `recover-stale-base`, etc.) with daemon stopped.
2. Direct single-row `stores tasks drive <TID> --max-iters N` only after confirming the row is safe to drive.
3. Manual review path for in-review rows when ER/codex lane would otherwise require daemon.
4. If ER must run, prefer a narrow external-review dispatch path or manual codex invocation over daemon restart.

## Current Known Queue Shape

Last verified snapshot after manual-control work:

- Daemon: stopped.
- Drives: reaped / no live `stores tasks drive` processes.
- External reviews: no `pending` / `running` / `tooling_held` rows.
- Dispatch locks: DB still has stale unfinished `auto-drive` locks (16 observed), but no matching live processes.
- `T125`: accepted after rebase + fresh ER359 PASS at current head.
- `T127`: accepted after rebase + fresh ER360 PASS at current head.
- `T124`: `in_review`, has executor commit + wrap; no ER row found yet; next likely manual review or create/run a narrow ER attempt.
- `T126`: `blocked`, substantive code-review FAIL; needs salvage-vs-abandon decision.
- `T123`: `blocked`, stale-binary/I033 contamination risk; hold.
- `T116`: `blocked`, T1 silent_zombie; hold until WIP low.
- `T108`: `blocked`, contract drift / repeated NEEDS_WORK; needs revise or abandon/remint decision.
- `T128`: now `blocked` (`drive_failed:silent_zombie_pid_dead`) after reaped drive; inspect before action.
- `T134`–`T137`: startup-sweep remints now `blocked` (`drive_failed:silent_zombie_pid_dead`) after killed/reaped drives; likely queue-curation candidates, not automatic drive candidates.
- Observations: 43 open, 13 ready, 1 investigating, 473 resolved, 6 wont_fix. New drive-failed opens include `L532`–`L536` for `T128`/`T134`–`T137`.

## Guardrails

- Do not restart daemon broadly.
- Do not run `stores agents run --once` as a convenience; it can fire startup sweeps and remint/scaffold/drive multiple rows.
- Do not abandon/accept/reject without Blake grounding where required.
- Do not resume I033-shaped rows with non-empty rejected plans until the manual-main I033 fix is confirmed landed.
- Do not use `git add -A` / `git add .`; stage explicit files only.

## First Verification Steps

1. Confirm no daemon/drive processes are running.
2. Confirm live task statuses from CLI.
3. Inspect config/source for any supported pause/maintenance/narrow-dispatch knobs.
4. Decide the first safe manual move with Blake.

## Work Completed This Iteration

- Confirmed `stores agents run --once` is a broad daemon tick, not a narrow subscriber dispatch. Even with a reduced `.stores/agents.yaml`, it still runs hardcoded startup sweeps and engine-runner/watchdog logic.
- Used Blake's token to accept `T125` and `T127`, but only after rebasing each branch onto current `main` and refreshing external review at the new branch head.
- Added a narrow control primitive directly on `main`: `stores external_reviews run <ERID>`.
  - Commit: `06403b5 repair: add narrow external review run verb`.
  - Purpose: run exactly one external review row without daemon startup sweeps, auto-promote, auto-drive, watchdog, or engine-runner.
  - Validation: `cargo test --lib external_review -- --nocapture`, `cargo clippy --lib --tests -- -D warnings`, `cargo install --path . --features runner-claude-code,runner-pi --locked --force`.
- Discovered the narrow ER verb currently has no pending row to trial; `T124` is the next likely candidate if/when an ER row is created.
- Confirmed the engine is paused but DB state is still messy: blocked remints, drive-failed observations, stale unfinished dispatch locks, and one in-review task remain.

## Next Priorities

1. **T124 review/accept path** — inspect branch, create/run ER using the new narrow verb or do manual review, then accept/reject deliberately.
2. **T126 disposition** — substantive FAIL; decide salvage revise vs abandon+sharpen.
3. **T134–T137 cleanup** — fresh remints now blocked after reaped drives; likely abandon or explicitly hold, depending on whether their linked observations still represent desired work.
4. **T128 disposition** — blocked silent_zombie after commitless executor summary; inspect and decide close/abandon/remint.
5. **Observation cleanup** — especially L532–L536 new drive-failed observations and stale L524–L531 drive/deploy-failed noise.
6. **Stale dispatch_locks audit** — 16 unfinished `auto-drive` locks with no live processes; understand whether current watchdog/daemon guards safely ignore them before any broad restart.
7. **Do not broad-restart daemon** until Phase 1 queue cleanup and Phase 2 narrow-control procedures are complete.

## Handover Prep

If context is cleared, give the next agent this worklog plus the queue-curator triage report.

Essential state to preserve:

- Manual-control north star: high-trust 5-wide throughput, but current mode is frozen/manual until the queue is clean and control primitives are safe.
- Daemon/drive state at last check: no daemon and no live `stores tasks drive` processes. Do not broad-restart daemon.
- New narrow primitive exists on main and installed binary: `stores external_reviews run <ERID>` from commit `06403b5`. Use this instead of `stores agents run --once` for single ER rows.
- `stores agents run --once` is unsafe in manual-control mode: it runs startup sweeps plus engine-runner/watchdog, not just agents.yaml subscribers.
- `T125` and `T127` are accepted after rebase + fresh ER PASS.
- Remaining priority rows: `T124` in_review, `T126` blocked FAIL, `T128` blocked, `T134`–`T137` blocked remints, holds `T108`/`T116`/`T123`.
- Observation cleanup remains important: source-observation remint risks (`L034`, `L513`, `L514`, `L515`, `L520`) and new drive-failed obs (`L532`–`L536`).
- Stale unfinished `auto-drive` dispatch_locks exist; do not raw-SQL write/delete them. Treat them as a pre-restart audit item.
- Untracked generated projection dirs remain and should not be swept with `git add -A`: `tasks/active/T802-test-task/`, `tasks/active/T803-test-task/`, `tasks/planning/T801-test-task/`.

Suggested next-agent first moves:

1. Read this worklog and the queue-curator triage report.
2. Verify process state: `pgrep -af 'stores agents run|stores tasks drive' || true`.
3. Verify live queue with `stores tasks status` for `T124 T126 T128 T134 T135 T136 T137 T108 T116 T123`.
4. Start with the triage report's fastest safe mechanical cleanup, but do not execute lifecycle verbs without Blake grounding where required.
5. If `T124` needs ER, create/locate the ER row and use `stores external_reviews run <ERID>`, not daemon.

## 2026-05-09 Update — cleanup complete, replacement path

- Queue cleanup shipped: `T014` and `T116` reached `schema_migrated`; `T108`, `T126`, `T128`, `T123`, `T134`–`T137` are abandoned; linked cleanup observations were resolved/wont_fix; remint-risk query returns no rows.
- `T123/L528` are intentionally retired after unsafe resume-to-executing exposed the I033 class again. Do not revive abandoned `T123`.
- Resume safety repaired on main: `9f11fc5 repair: make resume return to planning`; `resume` now only routes `blocked → planning`.
- Architect created replacement draft `L538` (`open`, contract draft, T3): generic integration lane + repo adapters, no auto-promote risk until ratified.
- Recommended next shape: ratify/harden `L538`, let auto-promote create a fresh task, and have engine-controller drive that row in a separate thread with this thread supervising U-moments.

## Done When

- We have a documented manual-control procedure for progressing selected rows without broad daemon pickup.
- Random task pickup is prevented by process state and verified by CLI/process checks.
- The next row action is chosen deliberately with Blake, not by daemon heuristics.
