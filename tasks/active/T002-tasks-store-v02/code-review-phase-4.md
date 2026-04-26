# Code Review — Phase 4 (cycle 2)

**Reviewer:** code-reviewer agent
**Date:** 2026-04-26
**Cycle:** 2 of max 3
**Gate:** PASS
**Status next:** EXECUTING_PHASE_5

---

## Scope reviewed

Cycle 2 commits since cycle-1 baseline `8980bfb`:

- `47f0b96` T002 P4.cycle2: compute/run split — direct handler-level tests for all 7 ACs
- `7d5cc67` T002 P4.cycle2: update execution log + set status CODE_REVIEW

Diffstat:

```
src/handlers/brief.rs                     | 212 +++++++++++--------
src/handlers/next_action.rs               | 288 +++++++++++--------------
tasks/active/T002-tasks-store-v02/main.md |  33 ++-
```

Tightly scoped — no drift. The only files changed are the two new handlers and the task log.

---

## Cycle-1 finding verification

### M1 — handler `run()` had ZERO direct test coverage → RESOLVED (with caveat)

**The compute/run split is real, not cosmetic.**

`src/handlers/next_action.rs:69-115` defines `pub(crate) fn compute(schema, conn, display_id) -> Result<NextActionOutput>`. It performs all I/O against the DB and produces a `NextActionOutput` struct with all 9 AC4.1/AC4.2 keys (`id`, `status`, `current_phase`, `current_cycle`, `next_agent`, `blocked`, `blocked_reason`, `claimed_by`, `claimed_at`). The struct derives `Serialize` + `Deserialize`. `compute()` does no `println!`.

`run()` (next_action.rs:117-163) is now thin: parses `display_id` + `--json`, calls `compute()`, formats either JSON (via the `json!` macro re-emitting all 9 keys) or text mode (9 `println!` lines).

`src/handlers/brief.rs:37-147` defines `pub(crate) fn compute(schema, conn, matches, invoker) -> Result<BriefOutput>` returning `{agent, brief_markdown}`. `run()` (brief.rs:149-170) is thin: calls `compute`, prints either pretty JSON or `brief_markdown` text.

**Tests exercise the actual handler logic now:**

| Test (file:line) | What it asserts | Path through real code |
|--|--|--|
| `next_action_executing_returns_executor` (next_action.rs:300-317) | AC4.1: 9 keys present, executor on executing row | calls `compute()`, then `serde_json::to_value(&out)` — exercises real serialization |
| `next_action_planning_returns_planner` (next_action.rs:320-329) | AC4.1: planning → planner | calls `compute()` |
| `next_action_blocked_returns_null_agent` (next_action.rs:332-345) | AC4.6: blocked: true, next_agent: null in JSON | calls `compute()` then `serde_json::to_value` |
| `next_action_no_workflow_errors` (next_action.rs:348-373) | AC4.7: error names store + "no workflow declaration" | calls `compute()`, asserts on `unwrap_err().to_string()` |
| `brief_compute_unknown_agent_error_lists_all_roles` (brief.rs:257-283) | AC4.5: real `bail!` lists all 4 roles + bad name | calls `compute()` against a 4-role schema, real DB row inserted, `--for nonexistent_agent` |
| `brief_compute_no_workflow_errors` (brief.rs:286-310) | AC4.7: error path | calls `compute()` |

**The previous private `compute_next_action` helper is gone** — `run()` now uses the same `compute()` path the tests exercise. There is no longer a duplicated re-implementation of handler logic.

**Caveat (sub-finding, NOT a gate condition):** there is no `compute()`-level test that asserts on a successful `BriefOutput.brief_markdown` for AC4.3 (default agent → planner) or AC4.4 (`--for executor` happy path). Both ACs require a template file on disk + a manifest entry matching the schema name; the test infrastructure exists in cycle 2 (tempdir + DB + ddl) but stops at the error paths. AC4.3 and AC4.4 are still verified end-to-end via direct CLI probe (cycle-1 functional table) and via e2e — I re-ran both cases against `/tmp/cycle2-probe` (`stores wf_tasks brief WF001` produced the planner template; `--for executor` switched it). The behavior is correct; the gap is an additive nice-to-have for Phase 5 to inherit when it writes submit-handler tests with template inputs.

### M2 — AC4.5 contract test asserted a re-implementation → RESOLVED

`brief_compute_unknown_agent_error_lists_all_roles` (brief.rs:257-283):

1. Inserts a real row into a real SQLite table built from `four_role_schema()` DDL (brief.rs:263-267).
2. Calls `compute(&schema, &conn, &matches, Actor::AiAutonomous)` with `--for nonexistent_agent`.
3. Asserts `err.to_string()` contains literal substrings `"planner"`, `"plan_reviewer"`, `"executor"`, `"code_reviewer"`, AND `"nonexistent_agent"` (5 assertions).

This is the actual `bail!` at brief.rs:74-78 producing the error string, not a copy of the format template. I confirmed by inspection: the format string `"unknown agent role '{}'; available roles: {}"` exists in exactly one place (the `bail!` site); the test does not reconstruct it. If someone changes the wording or the join separator, the test fails.

The new test also makes the join order deterministic: brief.rs:68-73 now sorts `available` before joining (cycle 1 was unsorted, relying on hashmap iteration order). Live CLI probe at `/tmp/cycle2-probe` shows: `Error: unknown agent role 'nonexistent'; available roles: executor, planner` — alphabetical sort works.

### m1 — discarded `stores_dir_for` in next_action.rs → RESOLVED

next_action.rs has zero call to `stores_dir_for` (verified by grep: `grep stores_dir_for src/handlers/next_action.rs` returns nothing; the import is also gone). Dispatcher's `db_path()` is the actual scope-aware resolver.

In brief.rs:53 it remains, used functionally as the fallback `store_root` when the manifest lookup misses (brief.rs:128: `.unwrap_or_else(|| stores_dir.clone())`). Functional, not ceremonial.

### m2 — bundled-store gap → DOCUMENTED

TODO comment lands at brief.rs:116-121 naming the Phase 6 gap concretely:

```rust
// TODO (Phase 6): when schema_path starts with "bundled:" (e.g. the `tasks`
// store), joining it with template_path produces a nonsensical filesystem path
// ("bundled:tasks/templates/planner-brief.md.tpl").  Fix: detect the sentinel
// and route to the in-memory BUNDLED_STORE_TEMPLATES map instead.  No bundled
// store has a workflow today so this is latent; Phase 6 plan-review should
// verify the fix is in place before the `tasks` schema is wired up.
```

Plan-review for Phase 6 has a clear hook to catch this.

### m3 — duplicated `find_next_agent` logic → RESOLVED

next_action.rs:101 in `compute()`: `find_next_agent(workflow, &status)`. The inline `on_state.get(&status).and_then(...)` loop from cycle 1 is gone. `find_next_agent` is now the single implementation, used by both `compute()` and (via re-export) by `brief::compute`.

### m4 — unused test imports in brief.rs → RESOLVED

brief.rs:179-183 — all imports (`crate::db`, `crate::schema::Schema`, `clap::{Arg, ArgAction, Command}`, `rusqlite::Connection`, `tempfile::tempdir`) are now used by the new tests. `cargo build --tests` shows zero warnings in `brief.rs` or `next_action.rs`. Three `unused import: crate::db` warnings remain — they're in unrelated handler files (`add.rs:153`, `transition.rs:181`, `update.rs:148`) and predate Phase 4. Not in scope.

### m5 — AC4.7 bail! unreachable from CLI → COVERED BY NEW COMPUTE TESTS

`next_action_no_workflow_errors` and `brief_compute_no_workflow_errors` both call `compute()` directly against an `obs`-shaped schema with no `workflow` block. Both assert `err.to_string()` contains the exact bail wording. The bail sites are no longer dead code from the test suite's perspective.

---

## Test verification

```
$ cargo test 2>&1 | tail -3
test handlers::next_action::tests::next_action_planning_returns_planner ... ok
test handlers::row::tests::cycles_update_round_trips ... ok
test result: ok. 237 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

```
$ env -u CLAUDECODE bash tests/e2e.sh 2>&1 | tail -2
=== All 13 DONE_WHEN steps verified ===
  ... all 13 PASS
```

237 tests pass; e2e all 13 steps green. Test count unchanged from cycle 1 — old private-helper tests were replaced by equivalent `compute()`-level tests rather than added on top.

---

## Live CLI re-probe (regression sanity)

Fresh `.stores/` install at `/tmp/cycle2-probe`, fixture `workflow_minimal`:

| Probe | Outcome |
|--|--|
| `stores wf_tasks add --title "test" --invoker human` | `WF001` |
| `stores wf_tasks next-action WF001` (text) | 9 lines, all keys present, `next_agent: planner`, `blocked: false`, NULL fields render as `null` |
| `stores --json wf_tasks next-action WF001` | 9-key JSON, `next_agent: "planner"`, `blocked: false`, `claimed_by: null` etc. |
| `stores wf_tasks brief WF001` | Planner brief markdown |
| `stores wf_tasks brief WF001 --for nonexistent` | `Error: unknown agent role 'nonexistent'; available roles: executor, planner` (sorted) |

No regression from cycle 1.

---

## Drift check

`git diff 8980bfb..HEAD --stat` is clean: only `src/handlers/brief.rs`, `src/handlers/next_action.rs`, `tasks/active/T002-tasks-store-v02/main.md`. No incidental changes elsewhere — no `Cargo.toml`, `dispatch.rs`, `dynamic.rs`, `context.rs`, fixtures, or other handlers touched. Commit hygiene clean (two commits, no amends).

---

## New findings (cycle 2)

None gating. One sub-finding tracked above (no `compute()`-level happy-path test for `BriefOutput.brief_markdown`); recorded as carry-forward for Phase 5 since the test infrastructure pattern (DB + tempdir + manifest stub) is exactly what submit-handler tests will need to set up.

---

## Carry-forward to Phase 5

1. **Apply the same compute/run split to all four submit verbs** (`submit-plan`, `submit-plan-review`, `submit-execute`, `submit-review`). The pattern from this cycle — `pub(crate) fn compute(...) -> Result<HandlerOutput>` + thin `run()` printer + `#[derive(Serialize, Deserialize)]` on the output struct — is now the established shape.
2. **Cover the brief happy paths** (AC4.3 / AC4.4) at compute level when Phase 5 sets up template-on-disk test infrastructure for submit-render flows. The fixtures and tempdir helpers are already in place.
3. **P2-M1 carry-forward (still owed):** Thread `WorkflowResolved` into `main.rs` so brief.rs no longer needs to read templates from disk per-call. This also closes the bundled-store gap (m2) since `WorkflowResolved::resolve_from_strings` already handles the in-memory case.
4. **Re-add fixture fields if Phase 5 tests need them:** `submit_targets[submit-plan]: plan` and `auto_increment: true` on `current_phase` were stripped in Phase 4's fixture cleanup.

---

## Gate decision

**PASS.** Cycle 1's medium findings (M1 and M2) are structurally fixed: the compute/run split is real, all `run()` paths now flow through a tested `compute()` function, AC4.5's `bail!` is exercised by the actual handler call rather than a copy of its format string, and the `Serialize`/`Deserialize` round-trip on `NextActionOutput` provides the 9-key contract assertion the cycle-1 review demanded. Minor and trivial findings (m1-m5) all addressed. 237 tests pass; e2e green; no drift; commit hygiene clean.

The one residual sub-finding (no compute-level happy-path test for brief markdown rendering) is informational and naturally absorbed into Phase 5's submit-handler test infrastructure work — the cycle's test pattern is now the template for all four submit verbs.

Cycle 2 of max 3.
