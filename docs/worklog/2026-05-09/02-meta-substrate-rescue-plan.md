# Meta Substrate Rescue Plan

**Date:** 2026-05-09
**Type:** adhoc plan

## One-Loop Objective

Clear all stale/stuck active tasks so the substrate can safely resume normal operation. Use manual/meta-substrate rescue where needed after the T118/T122 incidents: preserve useful work, avoid blind retry loops, add or document only the smallest observability/escape hatches needed for runner failure, and leave the queue in a legible state before restarting broad dogfood automation.

## Loop TODO

1. **Re-establish current state**
   - Read `docs/worklog/2026-05-09/01-engine-rescue-sketch-plan.md` first.
   - Check main status: `git status --short`, `git log --oneline --max-count=12`.
   - Check active rows: `stores tasks status T122`, `stores tasks status T121`, `stores tasks status T117`.
   - Confirm latest important main commits exist: `bca5fd7`, `86b0614`, `e9ba39e`, `8dfdad1`, `597f15d`, `b19bdf0`.

2. **Clear T122 from `in_review` without another blind code-reviewer loop**
   - Current verified state as of this plan revision:
     - `stores tasks status T122` reports `status=in_review phase=1/1 cycle=1 next=wrap blocked=false`.
     - `stores tasks show T122 --json` shows a manual code-review `PASS` in `cycles[2].review` and a wrap run has completed.
     - `agent_runs` now includes `code_reviewer` and `wrap` rows for T122, so the earlier "no verdict" state is stale.
     - worktree: `/home/blake/repos/experiments/stores-T122-auto-promoted-l523`; current branch has generated `tasks/...` projection noise, so do **not** `git add -A` or broad-reset generated files.
   - Next action: route T122 through the human-grounded in-review decision path (`accept`/`reject` as appropriate, or close-out-of-band only if that is the explicit chosen rescue path). Do not send it through another autonomous code-reviewer retry unless a fresh source-code/CLI check proves the manual PASS/wrap state is invalid.
   - If additional validation is needed before acceptance, run targeted checks in the T122 worktree without staging generated projection noise:
     - `cargo clippy --all-targets -- -D warnings`
     - `cargo test --lib`
     - `cargo test auto_promote -- --nocapture`
     - `cargo test auto_resolve -- --nocapture`
     - `cargo test subscriber_edges -- --nocapture`

3. **Audit the remaining failed-role observability gap before adding code**
   - Source check found `src/handlers/drive.rs` already writes synthetic `agent_runs` telemetry for runner spawn/launch failures (`LAUNCH_ERROR_EXIT_CODE = -1`, `write_spawn_error_transcript`, `derive_spawn_fail_model_id`, and `spawn_failure_creates_synthetic_agent_runs_row`). Normal runner returns are also persisted before exit/error handling.
   - Therefore do **not** implement a duplicate synthetic-row path without narrowing the gap first.
   - Remaining possible gap to confirm: watchdog-level silent zombies where the detached drive process dies before `drive.rs` reaches a role spawn / `insert_agent_run` call, or dies between pre-spawn announcement and durable telemetry. If this remains unobservable, add the smallest testable evidence at the auto-drive/watchdog boundary: attempted `display_id`, inferred/current `role`, `phase`, `cycle`, drive pid, start time, failure time/reason, and log/transcript path if known.
   - Prefer extending existing `agent_runs`, `transition_history.actor_note`, or dispatch-lock telemetry over schema changes unless unavoidable.

4. **Add/document manual review escape hatch**
   - Problem: blocked `code_review` rows with useful code and dead reviewer have no clean audited path except repeated resume or close-out-of-band.
   - First inspect existing verbs: `stores tasks submit-review --help`, `close-out-of-band --help`, transition constraints.
   - If existing `submit-review` can be used from `code_review` only, document the safe manual path.
   - If row is `blocked`, decide whether to add a small recovery verb or document `resume → submit-review` only when the new guardrails make it safe.
   - Keep the first pass minimal: documentation/worklog + maybe a focused helper error message is enough if code change is risky.

5. **Generated projection hygiene**
   - Problem: T122 rebase was blocked by generated `tasks/...` projection noise.
   - Identify which files are projections vs user-authored work.
   - Prefer a read-only/status helper or documentation note over broad ignore changes.
   - Possible minimal fix: document a targeted cleanup checklist in this note and/or engine docs; do not delete/restore files unless Blake explicitly authorizes.

6. **Update operator-facing docs/notes**
   - Keep `01-engine-rescue-sketch-plan.md` concise; link to this plan.
   - Add final status: what shipped, what remains blocked, what commands passed, which commits matter.
   - If code changed, commit only relevant files explicitly.

## Known Current Facts

- `T120` shipped successfully to `schema_migrated` via merge commit `b707e1a`.
- T118 contamination root was fixed with guardrail commits:
  - `bca5fd7 repair: guard resume against rejected plans`
  - `86b0614 repair: require executable plan before start`
  - `e9ba39e repair: require executor result before review advance`
  - `8dfdad1 test: align resume recovery fixture with plan approval guard`
- T122 useful work is checkpointed on its branch as `48c07ac T122 manual-rescue: re-fire observation subscriber edges`.
- T122 repeatedly silent-zombied during code-reviewer dispatch; no code-reviewer verdict landed.
- Manual validation already showed T122 clippy and targeted tests passing after rebase to `aa5ec45`, but full `cargo test --lib` required the mainline `8dfdad1` fixture fix.
- L529 exists for the larger `stores watch`/flowtop redesign; do not solve the whole UI here.

## Guardrails

- Do not raw-SQL write `.stores/db.sqlite`; read-only SQL is fine.
- Do not `git add -A`, `git add .`, `reset --hard`, or force anything.
- Do not blindly resume T121; latest plan review is NEEDS_WORK and no code exists.
- Do not blindly retry T122 code-reviewer if manual review/close-out is available; repeated silent-zombie is already evidence.
- Keep changes small and commit finished units explicitly.

## Done When

- All stale/stuck active tasks are either safely cleared, accepted/rejected/abandoned/closed, or preserved with a precise next action that does not require blind retry loops.
- T122 is cleared from `in_review` through the human-grounded review decision path or explicitly documented as the remaining blocker.
- Operators can inspect failed drive/role dispatch evidence instead of guessing from silent_zombie state, or the remaining observability gap is precisely filed if implementation is too large for this loop.
- The manual review/close-out path for blocked reviewer rows is documented enough for engine-op to use.
- Worklog notes (`01` and this `02`) accurately reflect final state and next steps.
