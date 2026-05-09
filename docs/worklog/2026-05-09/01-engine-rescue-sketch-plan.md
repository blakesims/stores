# Engine Rescue Sketch Plan

**Date:** 2026-05-09
**Type:** note

## Next Loop Plan

Continue with the focused meta-substrate rescue loop in `docs/worklog/2026-05-09/02-meta-substrate-rescue-plan.md`. That plan is the handoff for the next context: clear/preserve T122 without another blind code-reviewer retry, add minimal failed-role observability, document a manual review escape hatch, and address generated projection hygiene enough to unblock rebases.

## Sketch TODO

1. **Stabilize the scene** — keep daemon/auto-spawn paused; do not resume blocked rows blindly; let useful child drives continue only when lifecycle-clean. ✅ initial audit done; stale-binary watchdog now catches old drive binaries.
2. **Audit active rows** — track `T117`, `T120`, `T121`, `T122` by lifecycle state, branch diff, review output, and whether work is salvageable. ✅ T120/T122 audited; T121/T117 remain blocked decisions.
3. **Salvage/manual-finish useful work** — patch reviewer findings directly in task worktrees where code exists; run tests; commit task-branch fixes. ✅ T120 shipped; T122 useful work checkpointed at `62519e7` and is back in `code_review`.
4. **Reproduce lifecycle bug with tests** — encode the `T118` failure: non-empty but rejected plan + planning-block resume must not enter executor. ✅ tests added in `src/handlers/submit.rs`.
5. **Install guardrails** — executor start requires non-empty plan plus latest relevant plan review `READY`; phase advancement requires prior phase execution/review success. ✅ guardrail/test commits on main: `bca5fd7`, `86b0614`, `e9ba39e`, plus fixture fix `8dfdad1`.
6. **Clear queue deliberately** — merge/accept/close shipped rows; abandon contaminated/superseded rows; resolve duplicate observations and dangling locks. ⏳ next operational focus: T122 review/merge, T121 disposition, T117 disposition, duplicate ready observation cleanup.
7. **Follow-up observability** — separately redesign/fix `stores watch` so it shows operator-actionable state instead of noisy internal buckets. ⏳ still pending; should be a dedicated follow-up after queue is safe.

## Current Snapshot

- Engine/daemon auto-spawn was paused by Blake during the first audit; engine-op later resumed only selected rows.
- Confirmed contaminated historical row: `T118` resumed `blocked → ready → executing` after NEEDS_WORK plan reviews; this is now covered by guardrail tests/fixes.
- Confirmed planner revision-context fix is present in recent planner briefs: revision runs include both rejected plan and prior reviews.
- Current task focus as of 04:26Z:
  - `T117` — T1 `blocked`, `drive_failed:silent_zombie_pid_dead`; no plan-review path because T1 skip-plan. Needs decision: inspect branch/useful work, then resume or abandon/remint.
  - `T120` — ✅ shipped/cleared to `schema_migrated`; merge commit `b707e1a` plus T120 commits `8193050`, `40370ac`.
  - `T121` — T2 `blocked`, `drive_failed:stale_binary_inode`; latest plan review is NEEDS_WORK after repeated planning loops, no code committed. With fixed resume semantics it should route to planning, but likely needs manual contract/plan disposition rather than another blind cycle.
  - `T122` — T2 currently `blocked` again after repeated code-reviewer drive silent-zombies. Lifecycle remains clean and useful work is checkpointed/rebased as commit `48c07ac` on top of main `aa5ec45`, but code-reviewer has not produced a verdict (`code_review_log_len=0`). Manual validation found the branch diff itself builds/lints after rebase; the remaining issue is runner/code-reviewer instability plus one mainline guardrail fixture that was fixed in `8dfdad1`.

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

### 2026-05-09 T122 investigation update

T122 state after engine-op retry:

- Row returned to `blocked`, `blocked_reason=drive_failed:silent_zombie_pid_dead`, `drive_pid=1449456` dead.
- Transition history shows repeated pattern: resume → executing → submit-execute → code_review → watchdog silent_zombie. No code-reviewer verdict landed.
- `agent_runs` has planner/plan-reviewer/executor rows only; no successful code-reviewer row for T122. `cycles[]` has two executor submissions for commit `62519e7`/rebased `48c07ac` and no review entries.
- T122 branch was rebased by engine-op: `HEAD=48c07ac T122 manual-rescue: re-fire observation subscriber edges`, base `aa5ec45`.

Manual branch validation:

- Initial `cargo clippy --all-targets -- -D warnings` showed the old `src/handlers/drive.rs:4474` single-element loop warning, indicating the branch needed the T120 clippy fix / rebase. After re-checking post-rebase, the file had the fixed block and clippy passed.
- `cargo clippy --all-targets -- -D warnings` now passes in `/home/blake/repos/experiments/stores-T122-auto-promoted-l523`.
- Targeted tests pass: `cargo test auto_promote -- --nocapture`, `cargo test auto_resolve -- --nocapture`, `cargo test subscriber_edges -- --nocapture`.
- Full `cargo test --lib` in the T122 worktree then exposed a mainline guardrail-test fixture bug: `ac5_14_blocked_to_ready_recovery` expected execution without seeding a latest READY plan review. Fixed on main in `8dfdad1 test: align resume recovery fixture with plan approval guard`; `cargo test --lib` passes on main after that fix.
- Attempted to rebase T122 onto the `8dfdad1` mainline fix, but the worktree has unstaged/generated task projection noise (`tasks/active/*`, `tasks/planning/*`) so `git rebase main` refused. Do not `git add -A`; cleanup must be targeted or done by engine-op with awareness of generated artifacts.

Conclusion: T122 code is not currently known-bad. The blocking symptom is the code-reviewer/drive repeatedly silent-zombieing before verdict, while manual compile/lint/targeted tests pass. Before another substrate retry, rebase T122 onto `8dfdad1` (or manually apply the fixture fix), then prefer manual/Codex review or close-out-of-band if the substrate code-reviewer keeps dying.

### 2026-05-09 meta-substrate manual work candidates

These are small, direct repairs/docs that help the substrate recover without starting another full dogfood cycle while the engine is unstable:

1. **Code-reviewer silent-zombie observability.** File/implement a narrow diagnostic for drive deaths during a role dispatch: persist attempted role (`next_agent`/role), child command/session id, last transcript path, and exit/signal if known. Current evidence can only infer code-reviewer died because status was `code_review`; no `agent_runs` row is written for the failed code-reviewer attempt.
2. **Manual review escape hatch.** Add or document a supported path for "human/Codex reviewed this blocked code_review row" that appends a review entry and advances safely, instead of requiring `close-out-of-band` or a fragile resume loop. This would preserve audit while handling runner failure.
3. **Generated projection hygiene.** Add a cleanup/status command or ignore policy for generated `tasks/...` projection noise in task worktrees. T122 could not rebase because generated task markdown/dirs were dirty; this is operational friction for rescue.
4. **Maintenance mode semantics.** Define a command/state that pauses new auto-spawns/watchdog escalation while allowing selected child drives or manual verbs. Today's "paused" state was ambiguous: existing drives kept mutating rows.
5. **Watch/flowtop follow-up.** L529 now captures the bigger observability redesign. Short-term, fix `stores watch` to stop mixing resolved observations into RATIFY buckets and expose true daemon/drive liveness before the full flowtop project.

## Next Actions

1. **Clear T122 manually or with a lighter review path.** It is blocked again after repeated code-reviewer silent-zombies, but manual clippy/targeted tests pass and no code-review verdict exists. Next concrete move: rebase T122 onto `8dfdad1`, run `cargo test --lib` plus targeted tests, then use manual/Codex review or close-out-of-band if the substrate code-reviewer continues to die.
2. **Decide T121 deliberately.** It is blocked with latest plan review NEEDS_WORK and no code. Fixed resume should send it to planning, but repeated NEEDS_WORK suggests manual contract/plan tightening or abandon/remint may be better than another autonomous loop.
3. **Decide T117.** It is T1 blocked by silent_zombie. Inspect branch/worktree first; if useful, resume after fixed binary is active; if not, abandon/remint.
4. **Install/propagate latest guardrail commits.** Main now has `bca5fd7`, `86b0614`, `e9ba39e`, `8dfdad1`; ensure the active `stores` binary includes all four before broad resume/restart.
5. **Queue cleanup after active rows settle.** Resolve duplicate ready observations from re-mints (`L489/L518`, `L513/L520`, `L515/L523` shape), abandon contaminated/superseded tasks, inspect dangling locks.

## Follow-ups

- Design explicit maintenance/freeze mode semantics: pause daemon spawns/watchdog without necessarily killing useful child drives.
- Fix `stores watch` UX: hide resolved rows from ratification buckets, collapse duplicate observations, expose true daemon/drive status, and show only actionable groups by default.
- Inspect dangling locks before restart; current watch reported many stale/dangling locks.
