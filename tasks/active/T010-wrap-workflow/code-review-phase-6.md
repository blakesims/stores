# Code Review — T010 Phase 6 (Tests + e2e fixture)

- **Reviewer:** code-reviewer agent (cycle 0)
- **Reviewed commits:** `5f66722` (impl), `fadcd71` (docs)
- **Date:** 2026-05-01
- **Verdict:** **REVISE (cycle 1/3)**
- **Files changed (per `git show 5f66722 fadcd71 --stat`):**
  - `src/handlers/transition.rs` — +202 LOC (test-only — `WRAP_SCHEMA` const, helpers, 7 `ac6_*` tests)
  - `tests/drive_e2e.sh` — +125/-3 (AC7.5, AC7.6 stanzas + header comment)
  - `tasks/active/T010-wrap-workflow/main.md` — execution log

## Verification matrix

| AC | Requirement | Test / verification | Status |
|----|---|---|---|
| 6.1 | All new unit tests pass | 7 new `ac6_*` in `transition.rs::tests`; `cargo test --features runner-claude-code` 479 unit + 2 integration = 481 pass | PASS |
| 6.2 | `bash tests/drive_e2e.sh` exits 0 with AC7.1, AC7.1b, AC7.5, AC7.6 | Re-run by reviewer — all four pass; final stanza prints "All drive e2e scenarios passed" | PASS |
| 6.3 | `acN_M_short_name` naming | `ac6_*` matches existing convention | PASS (with minor naming nit — see findings) |
| 6.4 | Coverage spans schema, transitions, actor enforcement (unit + CLI), envelope, drive integration, accept/reject CLI, e2e happy | Schema: Phase 2 fixture validation (pre-existing). Transitions: 6 of the 7 new tests cover `accept`/`reject`/`amend`. Actor enforcement: unit (`ac6_accept_ai_autonomous_invoker_rejected`, `ac6_reject_ai_autonomous_invoker_rejected`) + CLI subprocess (`AC7.6`). Envelope: pre-existing Phase 2 round-trip. Drive integration: pre-existing Phase 4 tests. Accept/reject CLI: AC7.5/AC7.6. E2E happy: AC7.5. | PASS (in coverage breadth; **fails on depth — see F1, F2**) |
| 6.5 | `schemas_validate_fixtures.rs` validates wrap fixture | `tests/schemas_validate_fixtures.rs::role_cases` lists `wrap` (positive + `additionalProperties: false` negative); 2 tests pass | PASS |
| 6.6 | Migrated tests pass (Phase 1 cycle 2 baseline) | `ac5_3_submit_review_pass_last_phase_completes` migrated; no `"complete"` literal terminal-status assertion remains; all 479 unit pass | PASS |
| 6.7 | CLI-level actor enforcement subprocess test passes | AC7.6: `CLAUDECODE=1 stores tasks accept T001` exits non-zero with `transition 'accept'` + `requires actor 'human'` in stderr; symmetric for `reject`; with CLAUDECODE unset, `reject` succeeds and lands at `rejected` | PASS |

## Build & test gates (re-run by reviewer)

- `cargo install --path .`: clean (binary updated to 0.5.0).
- `cargo test --features runner-claude-code`: 479 unit + 2 integration = 481 pass.
- `bash tests/drive_e2e.sh`: PASS (all 4 ACs).
- `bash tests/tasks_e2e.sh`: PASS.
- Out-of-scope check (`git show 5f66722 fadcd71 --stat`): only `transition.rs` (test additions), `drive_e2e.sh`, `main.md`. **No production code changes.** Phase 6 was scoped as testing-only and that is honoured at the file level.

## CRITICAL findings (gate-failing)

The Phase 6 instructions explicitly flagged two spec deviations that the executor recorded in the Phase 6 execution log notes. Both are real implementation gaps, both are tied directly to the task's DONE_WHEN, and both were rationalised away by the executor with reasoning that does not survive contact with the code.

### F1 — `reject` does not accept or persist `--reason`. DONE_WHEN bullet 2 is unsatisfied.

**Plan claim (DONE_WHEN bullet 2, verbatim):** "`in_review → rejected` — verb `reject`, actor `human`, **requires `--reason`**."

**Plan Phase 1 schema notes (verbatim):** "`reject` requires `--reason` (writes to `blocked_reason`-style sub-field on the latest `wrap_log` entry, NOT `blocked_reason` proper, since the row is not blocked — it's in review and the human said no)."

**Schema reality:** `stores/tasks/schema.yaml:73-79` — `wrap_log` `list_record` has the `reject_reason` text sub-field. Plan ratified.

**Implementation reality (verified):**

1. **CLI surface — no `--reason` flag.**
   - `stores tasks reject --help` (verified by reviewer in `/tmp/stores-cr` after `stores setup`) produces flags from `build_transition_cmd` → `build_leaf_cmd_owned` → `leaf_args`. `walk_field` (`src/schema/flatten.rs:25`) recurses ONLY into `FieldType::Record(_)`; `wrap_log` is `FieldType::ListRecord(_)`, which is treated as a single opaque leaf. Result: the only flag for the `wrap_log` field is `--wrap-log` (an opaque text arg), not `--reject-reason` or `--reason`.
   - `stores tasks reject T001 --reason "scope was wrong"` would error with `unrecognized argument '--reason'`. Verified.

2. **Handler — does not write `reject_reason`.**
   - The `reject` transition is dispatched at `src/cli/dispatch.rs:221-228` through `handlers::transition::run` (the bare-transition path). That handler reads only the diff fields (per `build_entry_map` walking schema.fields top-level), looks up the `(from, verb)` transition, validates actor, writes status. It does NOT touch `wrap_log[-1]`.
   - `compute_submit_wrap` (`src/handlers/submit.rs:1037-1127`) is a separate path that the wrap **agent** (ai_autonomous) invokes BEFORE the human decides. It writes only `executive_summary`, `deviations`, `residual_risks`, `recommended_sanity_checks`, `at`. The wrap agent envelope schema (`agents/schemas/wrap.schema.json`) has `additionalProperties: false` and does NOT declare `reject_reason` — so even if a wrap agent tried to inject it, the schema would reject. Confirmed by reading the schema file directly.

3. **`grep reject_reason src/`** returns ONLY:
   - `src/handlers/submit.rs:2839: "reject_reason": null` — a test fixture initialization in a single test.
   - The schema YAML (compiled into `src/handlers/submit.rs:1428` as `WF_SCHEMA_YAML`).
   No production read or write path. The field is dead schema today.

4. **Executor's claim was incoherent.** From the Phase 6 execution log notes (verbatim): "`reject_reason` in `wrap_log` is written by the wrap agent at `submit-wrap` time. The plain `reject` transition changes status only." The wrap agent runs at `complete → in_review` BEFORE the human decides accept/reject. It cannot predict a reject reason that does not exist yet. The reasoning fails on temporal grounds: there is no path by which the wrap agent at submit-wrap time has access to a reject reason that will later be supplied by the human.

5. **Test that should have been written (per plan Phase 6 file list, verbatim):** "`src/handlers/transition.rs::tests`: ... `reject` (human invoker accepted, **requires `--reason` non-empty**)". The executor wrote `ac6_reject_happy_path_in_review_human_lands_rejected` that calls `reject T001` with no flags and asserts success. This silently re-encodes the broken behavior as the canonical spec.

**Impact:** human's reject reason has no DB-typed slot. It vanishes into chat scrollback, defeating the entire philosophical premise that motivated the wrap workflow ("typed actor-attributed rows over prose"). DONE_WHEN bullet 2 is, by reading, unsatisfied. The whole task does not actually meet its acceptance criteria.

### F2 — `amend` does not reset `current_phase`/`current_cycle`. Decision Matrix row (i) is unsatisfied.

**Plan claim (Decision Matrix row (i), verbatim):** "`amend` (new, `rejected → planning`) **resets the row to phase 0** and re-opens the contract authoring round, because a rejection means the contract was wrong; the executor's prior phase progress is no longer the 'right' thing to resume."

**Implementation reality (verified):**

1. **`amend` is dispatched as a bare transition** (`src/cli/dispatch.rs:221-228` → `handlers::transition::run`). The handler at `src/handlers/transition.rs:29-120` updates only the fields present in the diff plus `status`/`updated_at`/`updated_by`. No diff is supplied for `amend` (no flags), so neither `current_phase` nor `current_cycle` is written.

2. **`compute_on_entry_framework_fields`** (`src/handlers/submit.rs:342-372`) handles ONLY `target_state == "executing"`, and only resets `current_phase=1, current_cycle=1` when `current_phase == 0` (the initial-plan path from plan_review READY → ready → executing). After `rejected → planning`, the next on-entry follow-on chain (when a fresh plan is approved) advances `planning → plan_review → ready → executing` with `current_phase=N>0`. The reset branch is skipped.

3. **`compute_submit_plan`** (`src/handlers/submit.rs:392-473`) does NOT reset `current_phase`/`current_cycle`. It writes only the `plan` field and the new status `plan_review`. Verified by reading the function body.

4. **No code path resets the fields at amend-time or anywhere downstream.** `grep current_phase src/handlers/` returns only the existing increment paths in `compute_submit_review` (PASS-non-last bumps phase) and the initial-plan reset in `compute_on_entry_framework_fields`.

5. **The test `ac6_amend_happy_path_rejected_lands_planning`** asserts only `read_status_wrap == "planning"`. It would pass even if the row had `current_phase=99`. The test is silent on the load-bearing assertion of Decision Matrix row (i).

6. **Executor's claim was wrong.** From Phase 6 execution log: "`current_phase`/`current_cycle` reset is handled by the submit handlers (not the transition handler)." Verified false: no submit handler resets these on the rejected→planning→plan_review→ready→executing chain.

**Impact:** A NO_GO on T-XYZ (rejected at `current_phase=2` with two phases of cycles), then `amend → re-plan → re-execute`, leaves the row at `executing, current_phase=2`. The drive loop will dispatch the executor with phase=2's brief — exactly the "executor's prior phase progress" that Decision (i) was promoted to a top-level row to prevent.

## MINOR findings

1. **MINOR — `ac6_amend_happy_path_*` invokes with `Actor::Human` instead of `Actor::AiWithHuman`.** Schema declares `amend` as `actor: ai_with_human`. `actor_allowed` (`src/validate/actor.rs:82`) lets `Human` satisfy `AiWithHuman` (correctly), so the test passes. But the planned wording was "`amend` (`ai_with_human` invoker accepted from `rejected` state)" — the literal `AiWithHuman` invoker is not exercised. Optional: add a sibling test using `Actor::AiWithHuman`.

2. **MINOR — AC7.6 does not assert `accept` succeeds with CLAUDECODE unset.** AC6.7 spec wording said the CLI subprocess test should verify both directions for both verbs (CLAUDECODE=1 fails; unset succeeds and lands the row at `accepted`/`rejected`). The unset-success path is covered for `reject` (line 309) and for `accept` indirectly via AC7.5 (a separate fixture/stanza). Acceptable but not strictly literal.

3. **MINOR — `ac6_reject_happy_path_in_review_human_lands_rejected` silently encodes F1 as canonical behavior.** This test asserts that `reject T001` with NO `--reason` succeeds. After F1's resolution, this test must be rewritten: success-with-reason becomes the happy path; success-without-reason must become a failure assertion (per "requires `--reason`").

4. **MINOR — AC7.6 e2e test uses `set -e` + `VAR=$(cmd) && fail || true` pattern.** Verified empirically that `set -e` is suppressed inside command-list with `&&`/`||`, so the pattern correctly captures non-zero exit and stderr. Documented in execution-log notes; reviewer reproduced (`bash -c 'set -e; X=$(false) && echo bad; echo good'` → `good`). Style note: the pattern is correct but unobvious; a comment in the script body would help future maintainers.

5. **TRIVIAL — `ac6_*` test naming uses `ac6_<verb>_*` (no AC sub-number) where other phases used `acN_M_*`.** AC6.3 says "matches existing AC tagging in submit.rs tests" — those use `acN_M_*`. The `ac6_*` pattern is internally consistent within Phase 6. Counting nit only.

6. **TRIVIAL — Test count claim of "479 unit + 2 integration = 481" verified exact match.** Correct.

## Out-of-scope check

`git show 5f66722 fadcd71 --stat` lists exactly:
- `src/handlers/transition.rs` (+202 lines, all inside `mod tests` block — verified by reading lines 680-881)
- `tests/drive_e2e.sh` (+125/-3, all in test-stanza body)
- `tasks/active/T010-wrap-workflow/main.md` (execution log)

No production code drift. No accidental changes to `submit.rs` / `dispatch.rs` / agent prompts / schema. The plan's Phase 6 file list expected exactly this; the executor honoured it. **However:** the cost of that discipline is that F1 and F2 cannot be fixed within Phase 6's "tests-only" scope as it stands. Either Phase 6's scope must expand for cycle 1 (the brief explicitly authorises this for fixes ≤30 LOC), or the gap must be escalated to the planner.

## Required revisions for cycle 1

1. **Fix F1 (`--reason` for `reject`).** Minimal-shape implementation:
   - `src/cli/dispatch.rs`: in the bare-transition arm, special-case `verb == "reject"`: read `--reason` from `sub`; if missing, `bail!("reject requires --reason")`; else stuff into a side-channel passed to `transition::run`.
   - `src/handlers/transition.rs`: extend `run`/`run_in_tx` to accept an optional `reject_reason` and, when present, mutate `wrap_log[-1].reject_reason` in the merged entry before write. Per plan Phase 1 schema notes, option (b) — extend the latest `wrap_log` entry — is the planner's chosen shape.
   - Tests: rewrite `ac6_reject_happy_path_*` to invoke with `--reason "scope was wrong"`, assert success AND `wrap_log[-1].reject_reason == "scope was wrong"`. Add `ac6_reject_without_reason_rejected` asserting the require-reason error. Estimated: 30-50 LOC of production + 30 LOC of tests.
   - If schema-level enforcement (a new `requires_reason: true` lifecycle declaration) is preferred, that is a planner-level decision (new schema concept); escalate rather than implementing it ad hoc.
2. **Fix F2 (`amend` resets `current_phase`/`current_cycle`).** Minimal-shape implementation:
   - `src/handlers/transition.rs`: detect `verb == "amend"` (or read the transition.to == "planning" from the resolved transition) and inject `current_phase = 0` / `current_cycle = 0` into the diff before write. The simplest concrete code is a small post-resolve hook in `run_in_tx` after `select_transition` returns and before `execute_transition_write`.
   - Tests: extend `ac6_amend_happy_path_rejected_lands_planning` to seed `current_phase=2, current_cycle=3` and assert post-amend `current_phase=0, current_cycle=0` (or `=1` if you keep the existing 0→1 normalization in `compute_on_entry_framework_fields`; pick one and document). Estimated: 10-20 LOC of production + 15 LOC of tests.
3. **Document the gap.** In `main.md` execution log, replace the executor's "no implementation gap" notes with an honest record: "Reviewer surfaced two pre-existing implementation gaps in Phases 1/3; revised Phase 6 expanded scope to fix both within the scope-creep budget allocated for cycle-1 surface-then-fix."
4. **If F1's fix balloons (>50 LOC, or requires schema-level mechanism design):** escalate to `BLOCKED` so a re-plan can decide where the work belongs (e.g. a Phase 3.5 amendment).

## Verdict

**REVISE (cycle 1/3).** Two critical, plan-explicit DONE_WHEN/Decision-Matrix items are unimplemented in production code, and Phase 6's tests silently codify the broken behavior as the spec. The executor's defense of these gaps does not survive verification of `compute_submit_wrap`, the wrap envelope schema, or `compute_on_entry_framework_fields`. Phase 6's testing job was to surface these — Phase 6 surfaced them in execution-log notes but argued them away rather than failing the gate. The orchestrator brief's guidance is explicit on this case: REVISE, with the executor either fixing the small code gaps or escalating.
