# Engine Rescue Sketch Plan

**Date:** 2026-05-09
**Type:** note

## Sketch TODO

1. **Stabilize the scene** — keep daemon/auto-spawn paused; do not resume blocked rows; let currently useful child drives finish only if they remain lifecycle-clean.
2. **Audit active rows** — track `T117`, `T120`, `T121`, `T122` by lifecycle state, branch diff, review output, and whether work is salvageable.
3. **Salvage/manual-finish useful work** — patch reviewer findings directly in task worktrees where code exists; run tests; commit task-branch fixes.
4. **Reproduce lifecycle bug with tests** — encode the `T118` failure: non-empty but rejected plan + planning-block resume must not enter executor.
5. **Install guardrails** — executor start requires non-empty plan plus latest relevant plan review `READY`; phase advancement requires prior phase execution/review success.
6. **Clear queue deliberately** — merge/accept/close shipped rows; abandon contaminated/superseded rows; resolve duplicate observations and dangling locks.
7. **Follow-up observability** — separately redesign/fix `stores watch` so it shows operator-actionable state instead of noisy internal buckets.

## Current Snapshot

- Engine/daemon auto-spawn is paused by Blake, but existing child drives were still alive during audit.
- Confirmed contaminated historical row: `T118` resumed `blocked → ready → executing` after NEEDS_WORK plan reviews.
- Confirmed planner revision-context fix is present in recent planner briefs: revision runs include both rejected plan and prior reviews.
- Current task focus:
  - `T117` — T1, now `blocked`, watchdog `silent_zombie_pid_dead`; no plan-review path because T1 skip-plan.
  - `T120` — T2, lifecycle-clean so far: NEEDS_WORK → revised plan → `READY` → execution → code_review → executor clippy fix → `in_review`; branch has commits `8193050` and `40370ac`, likely first merge/accept candidate after manual review.
  - `T121` — T2, planning loop; three NEEDS_WORK reviews on mechanically-tightened Test 6 replacement; no code committed yet, likely needs manual plan/contract decision rather than more autonomous cycling.
  - `T122` — T2, lifecycle-clean so far: NEEDS_WORK → revised plan → `READY` → executing; branch has uncommitted work in auto-promote/auto-resolve areas and should be inspected before it advances further.

### 2026-05-09 active-row audit update

- `T117`: T1 `blocked`, `drive_failed:silent_zombie_pid_dead`; drive pid no longer alive.
- `T120`: T2 `in_review`, phase 1 cycle 2; worktree diff vs main touches `src/flow/builtins/auto_drive.rs`, `src/handlers/agents_run.rs`, `src/handlers/drive.rs`, `tests/flow_chain_isolation.rs`; executor reported cargo build, lib tests, stale-exe tests, silent-zombie E2E, and clippy all passing.
- `T121`: T2 `planning`; latest plan review says revised Test 6 plan still contradicts contract by moving source-agent provenance out of `gatekeeper_decision_json` and relying on boot-only startup_sweep for event-driven reprocessing.
- `T122`: T2 was `executing`, then T120's newly installed stale-binary watchdog correctly blocked the old drive with `drive_failed:stale_binary_inode`. Manual rescue inspected the useful branch diff, ran targeted `auto_promote`, `auto_resolve`, and `subscriber_edges` tests successfully, and checkpointed relevant code as commit `62519e7 T122 manual-rescue: re-fire observation subscriber edges`. Worktree still has framework-generated task projection noise not staged.

## Working Principles

- Judge branch work and substrate lifecycle separately: useful code can exist on a dirty row, and a clean row can still have bad code.
- Do not resume blocked rows until resume semantics are fixed.
- Do not treat “plan exists” as sufficient for execution; executor eligibility must be tied to review approval of the current plan.
- Prefer manual rescue of good work over abandoning/re-minting everything, to avoid wasting already-produced code/reviews.

## Detailed Notes

### Lifecycle bug to reproduce

`T118` transition history showed:

```text
planning → blocked      mark_drive_failed silent_zombie_pid_dead
blocked  → ready        resume
ready    → executing    start
```

This is wrong because the task had prior NEEDS_WORK plan reviews. A non-empty old plan is not proof of an executable approved plan.

### Guardrail shape

- `ready → executing` should require:
  - plan is non-empty; and
  - latest relevant plan-review verdict for that plan is `READY`; and
  - no newer `NEEDS_WORK` invalidates that plan.
- resume from a planning/revision failure should return to `planning`, not `ready`, unless the row has a currently approved executable plan.
- phase `N → N+1` should require phase `N` executor output plus code-review PASS/approval, not merely populated phase/cycle fields.

### 2026-05-09 guardrail TDD update

Work path: `src/handlers/submit.rs`.

Tests added near existing resume tests:

- `resume_with_non_empty_rejected_plan_routes_to_planning_for_non_t1` — reproduces the exact T118 shape: T2 row, non-empty plan, latest `plan_review_log.gate = NEEDS_WORK`, blocked by drive failure. Before the fix this failed with `left: "executing" / right: "planning"`.
- `resume_with_non_empty_ready_plan_keeps_ready_path_for_non_t1` — preserves valid T122-like recovery: T2 row, non-empty plan, latest `plan_review_log.gate = READY`, blocked by transient/stale drive failure; resume may still return to execution.

Implementation changed in `compute_resume`:

- `plan` empty/null/empty object → resume to `planning`.
- T2/T3 with populated plan → resume to `ready` only when the latest plan review log entry has `gate == "READY"`; otherwise resume to `planning`.
- T1 behavior preserved: null-plan T1 resumes through planning/skip-plan to synthesize a contract-derived plan; populated-plan T1 may resume through ready/executing.
- Updated existing stale-bookkeeping resume test to seed a `READY` plan review when it expects non-T1 resume to execution.

Validation run:

- `cargo test --lib resume_ -- --nocapture` → 13 passed.
- `cargo test --lib follow_on -- --nocapture` → 6 passed.

### 2026-05-09 ready→executing guard update

Work path: `src/handlers/submit.rs`.

Tests added near existing `fire_on_entry_follow_ons_*` tests:

- `fire_on_entry_follow_ons_ready_refuses_rejected_plan_for_non_t1` — constructs a T2 row already in `ready` with a non-empty plan but latest `plan_review_log.gate = NEEDS_WORK`; directly fires `fire_on_entry_follow_ons(..., "ready")` and asserts it errors with the executable-plan requirement and leaves status at `ready`.
- `fire_on_entry_follow_ons_ready_allows_latest_ready_plan_for_non_t1` — constructs the same shape but with latest gate `READY`; asserts ready on-entry advances to `executing` and sets phase/cycle to `1/1`.

Implementation changed:

- Added `entry_has_executable_plan(entry)` helper. It requires a non-empty plan plus either T1 `plan_source == "contract_synthesized"` or latest `plan_review_log.gate == "READY"`.
- `fire_on_entry_follow_ons` now refuses the framework `start` transition (`ready → executing`) when `entry_has_executable_plan` is false. This makes executor-start itself enforce the invariant, not just `resume`.

Validation run:

- `cargo test --lib fire_on_entry_follow_ons_ready -- --nocapture` → 2 passed.
- `cargo test --lib resume_ -- --nocapture` → 13 passed.
- `cargo test --lib follow_on -- --nocapture` → 8 passed.

### 2026-05-09 phase-advance guard update

Work path: `src/handlers/submit.rs`.

Test added near submit-review PASS tests:

- `submit_review_refuses_phase_advance_without_executor_result` — constructs a malformed `code_review` row with a matching phase/cycle entry but no `executor` object, then asserts `compute_submit_review(..., "PASS", ...)` errors and leaves status/phase/cycle unchanged.

Implementation changed:

- `compute_submit_review` now checks the matching `cycles[]` entry has an `executor` object before patching review output or selecting PASS/REVISE/FAIL transitions. This makes phase advancement require a real `submit-execute` result, not merely a forged phase/cycle shell.

Validation run:

- `cargo test --lib submit_review -- --nocapture` → 4 passed.
- Regression bundle: `cargo test --lib resume_ -- --nocapture`, `cargo test --lib follow_on -- --nocapture`, and `cargo test --lib submit_review -- --nocapture` all pass.

## Follow-ups

- Design explicit maintenance/freeze mode semantics: pause daemon spawns/watchdog without necessarily killing useful child drives.
- Fix `stores watch` UX: hide resolved rows from ratification buckets, collapse duplicate observations, expose true daemon/drive status, and show only actionable groups by default.
- Inspect dangling locks before restart; current watch reported many stale/dangling locks.
