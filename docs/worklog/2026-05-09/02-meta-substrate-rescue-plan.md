# Meta Substrate Rescue Plan

**Date:** 2026-05-09
**Type:** adhoc plan

## One-Loop Objective

Complete one manual/meta-substrate rescue loop that makes the engine safer to operate after the T118/T122 incidents: preserve useful T122 work, add the smallest observability/escape hatches needed for runner failure, and leave the queue in a legible state without starting another full dogfood cycle while the substrate is unstable.

## Loop TODO

1. **Re-establish current state**
   - Read `docs/worklog/2026-05-09/01-engine-rescue-sketch-plan.md` first.
   - Check main status: `git status --short`, `git log --oneline --max-count=12`.
   - Check active rows: `stores tasks status T122`, `stores tasks status T121`, `stores tasks status T117`.
   - Confirm latest important main commits exist: `bca5fd7`, `86b0614`, `e9ba39e`, `8dfdad1`, `597f15d`, `b19bdf0`.

2. **Clear or preserve T122 without another blind code-reviewer loop**
   - Inspect T122 row and worktree:
     - row: `stores tasks status T122`; transition history for T122; `cycles[]` for executor submissions.
     - worktree: `/home/blake/repos/experiments/stores-T122-auto-promoted-l523`.
   - Clean/rebase carefully:
     - Do **not** `git add -A` or reset generated task projection noise.
     - If needed, ask Blake/engine-op before deleting generated `tasks/...` files.
     - Goal: rebase T122 onto main containing `8dfdad1` or manually apply the fixture fix.
   - Validate T122 code manually:
     - `cargo clippy --all-targets -- -D warnings`
     - `cargo test --lib`
     - `cargo test auto_promote -- --nocapture`
     - `cargo test auto_resolve -- --nocapture`
     - `cargo test subscriber_edges -- --nocapture`
   - If validation passes, prefer audited manual/Codex review or close-out-of-band over another substrate code-reviewer retry, because T122 has repeated code-reviewer silent-zombies and no verdict.

3. **Add minimal failed-role observability**
   - Problem: T122 code-reviewer died before an `agent_runs` row existed, so operators infer failure from task state rather than seeing attempted role/session/transcript.
   - Find dispatch/run insertion points in `src/handlers/drive.rs` and runner boundary code.
   - Add the smallest testable behavior: when a role dispatch starts or fails before structured submission, persist enough evidence to identify `display_id`, `role`, `phase`, `cycle`, `started_at`, `ended_at`, nonzero/synthetic exit, and transcript/session path if known.
   - Prefer a synthetic `agent_runs` row or equivalent existing telemetry path; avoid schema changes unless unavoidable.
   - Test with existing mock/drive tests or add a focused unit test proving failed code-reviewer dispatch leaves an inspectable run record.

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

- T122 is either safely cleared/closed or preserved with a precise next action that does not require another blind reviewer retry.
- Operators can inspect failed role dispatch evidence instead of guessing from silent_zombie state, or a concrete observation/task is filed if implementation is too large for this loop.
- The manual review/close-out path for blocked reviewer rows is documented enough for engine-op to use.
- Worklog notes (`01` and this `02`) accurately reflect final state and next steps.
