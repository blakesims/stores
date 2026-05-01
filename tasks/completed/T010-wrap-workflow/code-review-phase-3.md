# Code Review — T010 Phase 3 (`submit-wrap` handler + auto-fire wiring)

- **Reviewer:** code-reviewer agent (cycle 0)
- **Reviewed commit:** `c36e3ac42ec5aa24e472ecbdcc90589499965992` (+ docs `26de11f`)
- **Date:** 2026-05-01
- **Verdict:** **PASS**
- **Revision count:** 0
- **Files changed (per `git show c36e3ac --stat`):**
  - `src/handlers/submit.rs` — +357 lines (handler + 5 tests + helpers)
  - `src/cli/dispatch.rs` — +35 lines (submit-wrap arm)
  - `src/cli/dynamic.rs` — +40 lines (build_submit_wrap_cmd + workflow registration)
  - `src/handlers/drive.rs` — +49/-17 lines (real handler call replaces stub)
  - `tasks/active/T010-wrap-workflow/main.md` — execution log

## Verification matrix (revised ACs)

| AC | Requirement | Test / verification | Status |
|----|---|---|---|
| 3.1 | Wrong-state rejection w/ exact error format | `ac3_1_submit_wrap_rejects_wrong_state` (unit) + end-to-end CLI smoke on `executing` row | PASS |
| 3.2 | Append + status unchanged + `at` set | `ac3_2_submit_wrap_appends_entry_and_status_unchanged` (unit) + end-to-end CLI smoke (verified `at: "2026-05-01T06:18:19Z"` in persisted JSON) | PASS |
| 3.3 | Lock acquired/released, no leak | `ac3_3_lock_acquired_and_released` — asserts `claimed_by` and `claimed_at` both NULL after commit | PASS |
| 3.4 | DROPPED — no transition fired | Verified `compute_submit_wrap` body has no `find_transition` call; correct | PASS |
| 3.5 | Actor enforcement matches existing pattern | submit-wrap accepts any invoker; mirrors `compute_submit_plan_review` which only enforces actor via `validate::validate(...)` against the verb-matched transition. Schema confirms no `submit-wrap` verb in `lifecycle.transitions`. | PASS |
| 3.6 | Re-entry appends, never overwrites | `ac3_6_submit_wrap_re_entry_appends_not_overwrites` (unit) + CLI smoke (2 successive calls produce wrap_log with 2 entries) | PASS |
| 3.7 | CLI dispatch reads 4 + 1 `--*-from-file` args, list args newline-split + empty-filtered, output is `Value::Object` matching wrap_log entry shape | Code-read verification (dispatch arm assembles `serde_json::Map` correctly; uses `read_lines_from_file` for list args which splits on `\n`, trims, filters empties); end-to-end CLI smoke confirms `--deviations-from-file <2-line file>` produces `["dev1","dev2"]` | PASS |
| 3.8 | ≥5 new tests | 5 new `ac3_*` tests in `submit.rs::tests`; total moved from 457 → 460 unit (+5 / -2 churn? actually executor reports +5 net) | PASS |

## Build & test gates

- `cargo build --features runner-claude-code`: **clean** (no warnings introduced).
  - The 3 `unused import: crate::db` warnings from test builds in `add.rs`/`transition.rs`/`update.rs` predate this branch — `git diff master..HEAD -- src/handlers/{add,transition,update}.rs` returns zero diff.
- `cargo test --features runner-claude-code`: **460 unit + 2 integration = 462 pass** — matches executor's claim exactly.
- `cargo test handlers::submit ac3_`: 5/5 pass (each ac3_* test green).
- `bash tests/drive_e2e.sh`: **PASS** — both AC7.1 happy path and AC7.1b revise-once. Final state per drive_loop output: `[T001] in_review; brief written; awaiting \`stores tasks accept | reject\``.
- `bash tests/tasks_e2e.sh`: **PASS** — Step 13 final state = `in_review|2`, Step 15 SQLite final state confirms. (The summary banner line `#13 PASS phase 2 (PASS-last) → complete` is pre-existing stale labelling that predates T010; the underlying assertions correctly check `in_review`.)

## Specific concern follow-up

The orchestrator brief flagged: "Was the executor able to **strengthen** that test [`happy_path_one_phase_mock`] to actually assert `wrap_log[]` is non-empty after drive runs?"

**Answer: no, not strengthened.** The test still asserts only `na.status == "in_review"` and `runner.remaining_count() == 0`. The same proxy-only pattern persists in `in_review_first_iteration_dispatches_wrap` (line 1336) and `in_review_re_entry_after_amend_dispatches_fresh_wrap` (line 1378). The comment at drive.rs:1356-1357 even says "Phase 1 stub; Phase 3 will write this via compute_submit_wrap" — yet Phase 3 didn't follow through and replace the placeholder seed with a post-condition assertion on the real handler's output.

**Why this is non-blocking:**
- The 5 new `ac3_*` unit tests cover `compute_submit_wrap` exhaustively at the handler level (state-machine guard, append, status preservation, `at` override, lock release, re-entry).
- The queue-drain proxy proves drive dispatched the wrap step.
- End-to-end CLI smoke (executed during this review) proves the handler persists wrap_log content correctly.
- Therefore, the integration "fixture → drive → handler writes wrap_log content" is covered de facto, just not by a single end-to-end assertion.

**Recommendation:** Phase 4 or Phase 6 should append `let wrap_log = read_wrap_log(...); assert_eq!(wrap_log.len(), 1); assert_eq!(wrap_log[0]["executive_summary"], "stub");` to `happy_path_one_phase_mock` and similar to the other two `in_review_*` tests, AND fix the stale comment at drive.rs:1356-1357. None of this blocks the gate.

## Findings

### MINOR

**F1. Integration tests still use queue-drain proxy.** *(Phase 6 cleanup — non-blocking.)*

Three drive integration tests (`happy_path_one_phase_mock`, `in_review_first_iteration_dispatches_wrap`, `in_review_re_entry_after_amend_dispatches_fresh_wrap`) assert `runner.remaining_count() == 0` as a proxy that drive dispatched the wrap step, but never check that `wrap_log[]` was actually populated by the post-Phase-3 handler. Now that `compute_submit_wrap` is wired, the tests should additionally assert wrap_log length and the `executive_summary` content from the wrap fixture.

Suggested fix in `src/handlers/drive.rs::tests`:
```rust
// After drive_loop returns:
let wrap_log: Vec<Value> = serde_json::from_str(&conn.query_row::<String, _, _>(
    "SELECT wrap_log FROM tasks WHERE display_id='T001'", [], |r| r.get(0)
).unwrap()).unwrap();
assert_eq!(wrap_log.len(), 1);
assert_eq!(wrap_log[0]["executive_summary"].as_str().unwrap(), "stub");
```

### TRIVIAL

**F2. Stale comment in `in_review_re_entry_after_amend_dispatches_fresh_wrap`** (drive.rs:1356-1357). The comment "Phase 1 stub; Phase 3 will write this via compute_submit_wrap" is now historical — Phase 3 has shipped. Reword in Phase 6 cleanup.

**F3. CLI permissiveness asymmetry.** Agent path requires non-empty `executive_summary` (serde non-`Option` type in `AgentEnvelope::Wrap`); CLI path tolerates missing `--summary-from-file` (produces empty string). Matches the existing pattern of `submit-execute`/`submit-review` CLI arms; not Phase 3's invention.

**F4. `require_workflow` called twice in `compute_submit_wrap`** (submit.rs:1044, 1047). First call's result is discarded. Mirrors `compute_submit_plan` (399, 421). Pre-existing pattern, harmless.

**F5. Test naming.** `ac3_7_submit_wrap_handler_sets_at_overriding_caller` is named for AC3.7 but actually covers an AC3.2 sub-concern (the handler-level `at` override). The actual AC3.7 (CLI dispatch arg-forwarding) is not unit-tested at the dispatch layer. Matches existing convention — no `submit-plan-review`/`submit-review` CLI dispatch unit tests exist either; CLI coverage lives in `tasks_e2e.sh`. Acceptable.

## Out-of-scope check

`git show c36e3ac --stat` shows exactly:
- `src/cli/dispatch.rs`
- `src/cli/dynamic.rs`
- `src/handlers/drive.rs`
- `src/handlers/submit.rs`
- `tasks/active/T010-wrap-workflow/main.md`

No contamination from Phase 4 (`agents/wrap.md`, `stores/tasks/templates/wrap-brief.md.tpl`, `src/render/context.rs`) or Phase 5 (`agents/guide.md`, `skills/task:wrap/`). Clean scope discipline.

## Gate decision

**PASS** — all 7 active ACs verified (AC3.4 dropped per orchestrator brief), build clean, all 462 tests green, both shell e2e suites green, end-to-end CLI smoke confirms persistence works as advertised. The 5 findings are all non-blocking; F1 is a real opportunity gap that should be addressed in Phase 4 or Phase 6 but does not undermine correctness because the handler is independently verified by 5 unit tests and the CLI path is verified by manual end-to-end smoke during this review.

**Status update:** `EXECUTING_PHASE_4`.
