# Code Review — T003 Phase 3: `stores tasks drive` orchestrator

- **Cycle:** 1 / 3
- **Reviewer:** code-reviewer (Opus 4.7 1M)
- **Date:** 2026-04-27
- **Commits reviewed:** `f461787` (impl), `e5af599` (log/status)
- **Base:** `8661e60`

## Gate: PASS

All 10 ACs (3.1–3.10) verified. AC3.8 deviation accepted as equivalent for bundled stores (the only configuration drive supports in v0.3, aligned with DONE_WHEN). 0c / 0M / 3m findings.

## Issues found: 0c / 0M / 3m

### Critical
_None._

### Major
_None._

### Minor

**m1. `LOCK_WINDOW_SECS` is a redefinition, not a shared constant** — `src/handlers/drive.rs:65` defines `const LOCK_WINDOW_SECS: u64 = 300;` while `src/handlers/submit.rs:78` (inside `acquire_lock`) hardcodes the literal `300` via `iso_subtract_seconds(300)`. The values match; the doc comment claims it "matches" `submit.rs`. The phase spec asks for "reuse existing constant" — there is no shared constant to reuse, only a duplicated literal. Risk: if one place is bumped to 600 and the other isn't, drive could pick a row that submit then refuses to lock. Cheap fix in a follow-up: lift to a single `pub(crate) const LOCK_WINDOW_SECS` in `submit.rs` (or a new `lock` module) and import from both. Not a blocker for v0.3 — the values are identical and the test `auto_selection_skips_live_claimed` empirically confirms the lock-window semantics agree.

**m2. Drive only works with bundled stores; limitation undocumented in `--help`** — `drive_loop` (drive.rs:382) bails with `"no bundled template '...' for store '{}'; drive requires a bundled store"` if the store is filesystem-installed (i.e. `schema_path` doesn't start with `bundled:`). This is the right v0.3 trade-off (DONE_WHEN scopes drive to the bundled tasks store), but `stores tasks drive --help` does not mention this constraint. The help text already says "Drive a workflow task to a terminal state via a runner" — adding a one-liner like "Currently supports bundled stores only" would prevent surprise. Not a blocker because in practice the only workflow store available is `tasks` (also bundled), and the runtime error is helpful.

**m3. Render-failure silence is technically logged but progress message ordering misleads** — `drive_loop` calls `compute_render_in` after submit; on failure it logs `[T001] render compute failed (non-fatal): ...` to stderr and continues. In the happy-path test there's no manifest, so render fails on every iteration. The next stderr line is the progress message `phase 0 cycle 0: planner → submitted (gate=None)` which uses `na.current_phase`/`na.current_cycle` read **before** submit — so the post-submit phase advance isn't reflected (e.g., after planner submits, `current_phase` becomes 1 but the printed progress still says `phase 0`). Cosmetic only, but in production logs this could mislead the human watching `--follow`. Recommendation: either re-read state after submit for the progress line, or label the line "after submit" semantics clearly. Not blocking for v0.3.

## AC Verification

| AC | Description | Verified | Evidence |
|----|-------------|----------|----------|
| 3.1 | `drive` loops next-action → brief → spawn → submit → render | ✓ | `drive_loop` lines 318–477; `happy_path_one_phase_mock` test reaches `complete` after 4 agents |
| 3.2 | `--auto` selects via SQL with lock window, ASC by created_at | ✓ | `resolve_task_id` lines 237–259 matches spec SQL; `auto_selection_picks_earliest_created_at` and `auto_selection_skips_live_claimed` tests cover both paths; "no candidates" message clear (line 256); `--help` describes selection (dynamic.rs:451–457) |
| 3.3 | `--mock <fixture>` (always built, hidden) and `--claude-code` (feature-gated) | ✓ | `dynamic.rs:475` `.hide(true)` for `--mock`; `dynamic.rs:482-490` `--claude-code` under `#[cfg(feature = "runner-claude-code")]`. Live verified: without feature, `--help` omits both `--mock` and `--claude-code`; with feature, `--claude-code` appears, `--mock` still hidden. Fixture format = JSON array of `{stdout, stderr, exit_code, final_message}` (`MockFixtureItem`). |
| 3.4 | Progress to stderr; nothing on stdout | ✓ | Only `eprintln!` calls in drive_loop; verified empirically — running happy-path test with `--nocapture` shows all progress on stderr; no `println!` anywhere in drive.rs |
| 3.5 | `--max-iters` default 50; on hit non-zero + clear message | ✓ | dispatch.rs:120 default=50; `max_iters_aborts_loop` test asserts non-zero and message contains "max iterations exceeded" (line 911) |
| 3.6 | Runner non-zero exit → no submit, task state unchanged | ✓ | Lines 407–419: bail before parse/submit; `runner_error_mid_loop_does_not_corrupt_state` reads row before/after and asserts `status` and `plan` unchanged byte-equal (lines 960–969) |
| 3.7 | Tests cover happy path / auto-selection / live-claim / max-iters / runner-error / terminal | ✓ | All 6 scenarios present (tests at lines 786, 816, 846, 891, 920, 976, 994) — 12/12 pass |
| 3.8 | Reuses brief/render handlers; no public-API widening | ✓* | See deviation subsection below |
| 3.9 | `blocked` status → exit 0 + helpful hint | ✓ | Lines 329–339: returns `Ok(())` after eprintln including `stores gate <id> guide` hint; `terminal_blocked_exits_zero` test asserts |
| 3.10 | Envelope parser: last non-empty JSON line, role-tagged enum, parse failure surfaces stderr + non-zero, no submit | ✓ | `parse_envelope` lines 488–516 prefers `final_message` then scans `stdout.lines().rev().find(non-empty)`; `AgentEnvelope` enum tagged on `"role"` with kebab-case mapping (planner / plan-reviewer / executor / code-reviewer); commentary tolerated (`parse_envelope_tolerates_commentary` test); on failure: drive_loop:422–432 logs runner stdout/stderr to stderr and bails before submit; all 4 fixture roles parse correctly (4 fixture tests) |

*AC3.8 — accepted with caveat; see below.

## AC3.8 deviation verdict

**ACCEPTED.** The executor's reasoning was directionally correct but factually imprecise:

1. **Public-API surface — actually unchanged.** Executor claims `build_context` and `render_template` are `pub(crate)`. They are not — both are `pub` in `src/render/mod.rs:7-8` (re-exported via `pub use`), and so is the underlying `pub fn` in `context.rs:33` and `engine.rs:149`. They were already part of the public API prior to this phase. So drive.rs adding `use crate::render::{build_context, render_template};` is **not** a widening — the surface was already public. The executor accidentally got the right answer for the wrong reason. (No new `pub fn` / `pub struct` was added in this commit beyond `DriveArgs`, `MockFixtureItem`, and `run_drive` — all of which are entry-point types appropriate for `pub`. `resolve_task_id` and `drive_loop` are correctly `pub(crate)`.)

2. **Semantic equivalence — true for bundled stores only.** Reading `compute_brief` line-by-line vs the inlined block in `drive_loop:355–390`:
   - Workflow-presence check: ✓ both
   - Agent role determination: brief uses `find_next_agent(workflow, status)` directly; drive uses `compute_next_action(...).next_agent` which **also** calls `find_next_agent(workflow, &status)` (next_action.rs:101). Equivalent.
   - Briefing template path lookup via `workflow.briefing_templates`: ✓ both
   - Template content load: brief has a **two-branch** path — bundled (via `BUNDLED_STORE_TEMPLATES` when `schema_path` starts with `bundled:`) **and** filesystem (read from `schema_path/template`). Drive **only** has the bundled branch.
   - `build_context` + `render_template`: ✓ both
   
   For a filesystem-installed `tasks` store, drive will hard-fail at line 382 with "drive requires a bundled store". For the bundled `tasks` store (the only one shipped, and the only one the DONE_WHEN exercises), the two paths produce byte-identical output.

3. **Manifest-invariant skipped: none material.** `compute_brief` calls `Manifest::load()` purely to discover whether the store is bundled-or-filesystem. It doesn't enforce any other manifest invariant (no scope check, no ownership check, no version check). Skipping it for the bundled case is safe.

4. **Why the inlining was necessary for tests.** `Manifest::load()` requires `.stores/manifest.yaml` at cwd. The drive tests use `tempdir()` and run with no manifest — calling `compute_brief` would fail. The inlined path doesn't need a manifest because it commits to the bundled path. This is a hermetic-tests-vs-production-parity tension; the executor chose hermetic tests, which is correct.

**Verdict:** Equivalent for bundled stores in production. v0.3 only ships a bundled tasks store, and DONE_WHEN explicitly scopes drive to that. The limitation should be documented (m2 above) but does not block. Phase 4–7 can proceed.

## Test matrix re-run

| Command | Result | Notes |
|---------|--------|-------|
| `cargo test handlers::drive` | 12/12 pass | 0.05s |
| `cargo test` (full suite) | 324/324 pass | 0.09s; 3 pre-existing `unused import: crate::db` warnings in add/transition/update tests, not from this phase |
| `cargo build` | clean | 0.05s incremental |
| `cargo build --features runner-claude-code` | clean | 0.03s incremental |
| `stores tasks drive --help` (no feature) | shows `--auto`, `--max-iters`; hides `--mock`; no `--claude-code` | as expected |
| `stores tasks drive --help` (with feature) | shows `--claude-code`; still hides `--mock` | as expected |
| `stores tasks drive --auto` (empty DB) | `Error: no non-complete tasks available...` exit 1 | clear |
| End-to-end fixture parse (4 roles) | all parse correctly | inline tests cover each fixture file |

## Public-API surface check

`git diff 8661e60..HEAD -- src/ | grep -E "^\+.*pub "` finds exactly 5 new public items, all in `drive.rs`:

```
+pub struct MockFixtureItem        // fixture deserialization helper, appropriate
+pub struct DriveArgs              // CLI args struct, appropriate
+pub fn run_drive                  // entry point, called from dispatch.rs
+pub(crate) fn resolve_task_id     // crate-private, test access
+pub(crate) fn drive_loop          // crate-private, test access
```

No widening of existing items. `render::build_context` and `render::render_template` were already `pub` before this phase (verified at `src/render/mod.rs:7-8`). AC3.8 "no public API changes" is satisfied.

## Tests inspected for rigor (not just count)

- `runner_error_mid_loop_does_not_corrupt_state` — reads `status` and `plan` columns before AND after the failed runner call, asserts byte-equality. **Pre/post comparison is real, not just exit code check.** ✓
- `auto_selection_skips_live_claimed` — uses `now()` for `claimed_at`, then asserts T002 (unclaimed) is selected over T001 (live-claimed). The lock-window math is exercised end-to-end. ✓
- `parse_envelope_tolerates_commentary` — provides a stdout with two commentary lines + a final JSON line, verifies the `Planner` variant is returned. ✓
- `terminal_blocked_exits_zero` — asserts `Ok(())` on a blocked task with no runner queued. Confirms early exit before runner.spawn is reached. ✓
- `max_iters_aborts_loop` — sets `max_iters=1` and one runner output; asserts error contains "max iterations exceeded". ✓
- `happy_path_one_phase_mock` — feeds 4 fixture files through MockRunner FIFO; asserts final status is `complete`. End-to-end through state machine. ✓

All six AC3.7-required scenarios are present and substantive.

## Phase 4–7 plug-in confirmation

Drive's public surface (`run_drive(schema, DriveArgs)`) is the integration point for Phase 4 (`stores setup` adds it to the bundled flow) and Phase 7 (skill rewrite calls `stores tasks drive --auto --claude-code`). The CLI args are stable: `[display_id]`, `--auto`, `--max-iters`, `--mock`, `--claude-code`. No follow-on rework needed for downstream phases — they can plug in unchanged.

## Recommendation

**PASS — advance to Phase 4.** Three minor findings logged for later cleanup; none gate progression. AC3.8 deviation is acceptable for v0.3.
