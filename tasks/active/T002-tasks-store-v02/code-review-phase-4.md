# Code Review — Phase 4 (cycle 1)

**Reviewer:** code-reviewer agent
**Date:** 2026-04-26
**Cycle:** 1 of max 3
**Gate:** REVISE
**Status next:** EXECUTING_PHASE_4

---

## Scope reviewed

- Commits `68764f9`, `4aba048`, `821afe2`, `40cb862` (4 commits, +866 / −13 LOC across 8 files).
- Files audited:
  - `src/handlers/next_action.rs` (new, 434 lines incl. tests)
  - `src/handlers/brief.rs` (new, 273 lines incl. tests)
  - `src/handlers/mod.rs` (+2)
  - `src/cli/dispatch.rs` (+6)
  - `src/cli/dynamic.rs` (+36; verb gating logic)
  - `src/render/context.rs` (+45 / −13; reserved-column inclusion deviation)
  - `tests/fixtures/workflow_minimal/schema.yaml` (+12 / −8; status removal + blocked state)
  - `tasks/active/T002-tasks-store-v02/main.md` (executor log + status updates)

---

## Functional verification — runtime behavior is correct

I built and exercised the binary directly against a fresh `.stores/` with the workflow_minimal fixture installed. Each AC produces the expected on-the-wire output:

| AC | Manual probe | Outcome |
|----|--------------|---------|
| AC4.1 | `stores wf_tasks next-action WF001` on planning row | 9-key text response: `id`, `status`, `current_phase`, `current_cycle`, `next_agent: planner`, `blocked: false`, `blocked_reason: null`, `claimed_by: null`, `claimed_at: null`. Correct. |
| AC4.2 | `stores --json wf_tasks next-action WF001` | 9-key JSON object, all keys present. Repeated with `update --claimed-by agent-foo --claimed-at 2026-04-26T10:00:00Z` → `claimed_by`/`claimed_at` populated correctly. |
| AC4.3 | `stores wf_tasks brief WF001` | Planner-brief markdown rendered to stdout. |
| AC4.4 | `stores wf_tasks brief WF001 --for executor` | Executor-brief markdown rendered (override works). |
| AC4.5 | `stores wf_tasks brief WF001 --for nonexistent_agent` | `Error: unknown agent role 'nonexistent_agent'; available roles: executor, planner` — fixture has only 2 roles; the message format demonstrably lists every role declared in `workflow.agent_roles`. |
| AC4.6 | `start-execute` then `block` WF002, then `next-action` | `blocked: true`, `next_agent: null`. Correct. |
| AC4.7 | `stores observations next-action OBS001` | clap rejects with `unrecognized subcommand 'next-action'` — gating is at the CLI layer, not the handler. Plan-review explicitly allowed both gating styles ("Both can be correct"). |

237 unit tests pass; e2e all 13 steps green.

---

## Findings

### M1 — Handler `run()` functions have ZERO direct test coverage (MEDIUM)

**Where:** `src/handlers/next_action.rs:191-433`, `src/handlers/brief.rs:149-273`.

**Symptom:** Of the 7 ACs, none are exercised by a test that actually invokes `next_action::run` or `brief::run`. The tests in both modules:

- `next_action::tests::compute_next_action(...)` (next_action.rs:386-433) — a private helper that re-implements the handler's logic outside the handler. Three tests (`next_action_executing_returns_executor`, `next_action_planning_returns_planner`, `next_action_blocked_returns_null_agent`) exercise this helper, NOT the actual `run()` function.
- `next_action::tests::next_action_no_workflow_errors` — only asserts `schema.workflow.is_none()`; never calls `run()` against a non-workflow schema.
- `brief::tests::brief_unknown_agent_error_lists_all_roles` and `brief::tests::brief_unknown_agent_error_with_all_four_roles` — both reconstruct the bail! template inline (`format!("unknown agent role '{}'; available roles: {}", ...)`) and assert against that copy. They do not exercise the actual `bail!` site in brief.rs:66.
- `brief::tests::find_next_agent_returns_first_dispatch` — exercises the imported helper, not the handler.

**Impact:**
- The 9-key JSON output's literal key set, ordering invariants, and null-on-unlocked behavior have no direct assertion. If `run()` were silently changed to omit a key (e.g. dropped `claimed_at` from the json! macro), every test still passes. The contract for AC4.1/AC4.2's "nine fields" lives only in code comments.
- The 9-key text output (the `println!("id: ...")` block at next_action.rs:161-169) has no test. `cargo test` cannot detect a regression where a key is dropped from text-mode output.
- The `--for unknown` error path (brief.rs:66-70 — the actual `bail!`) is dead code from the test suite's perspective. The test asserts a duplicate of the message, not the message produced by `bail!`. If someone changes `available.join(", ")` to `available.join(" ")` or rewords the error, AC4.5 silently regresses while tests stay green.
- `Manifest::load()` and `std::fs::read_to_string(template_path)` failure paths in brief.rs are entirely untested.

**Required action (before PASS):** Add at least one integration-style test per handler that:
1. Builds an in-memory `ArgMatches` (mirroring the dispatcher).
2. Calls `next_action::run` / `brief::run` directly, capturing stdout via the standard pattern (e.g. `gag` crate, or refactor `run()` to return a `String`/`Value` and let a thin `run()` wrapper print it).
3. Asserts on the actual on-the-wire output: every one of the 9 keys, ordering of text-mode lines, the literal AC4.5 error string from the real bail.

The cleaner refactor is to split each handler into a pure `compute(...) -> Result<Output>` and a thin `run()` that calls `println!`. Tests then call `compute` and assert on the structured `Output`. This is also what Phase 5's submit handlers will need.

The brief test file even has unused `crate::db` and `tempfile::tempdir` imports (warnings on compile) — confirming the executor *intended* to write a DB-backed handler test and never finished.

---

### M2 — AC4.5 contract test asserts a re-implementation, not the actual handler (MEDIUM)

**Where:** `src/handlers/brief.rs:167-198` and `:202-256`.

This is a sub-finding of M1 but called out separately because AC4.5 is the only AC with a *literal-string* requirement ("test asserts the strings `planner`, `plan_reviewer`, `executor`, `code_reviewer` all appear"). The current test:

```rust
let available: Vec<&str> = workflow.agent_roles.keys().map(|k| k.as_str()).collect();
let msg = format!(
    "unknown agent role '{}'; available roles: {}",
    bad_role,
    available.join(", ")
);
assert!(msg.contains("planner"), ...);
```

The test reconstructs the format string the handler *also* uses, then checks that string. This is a contract test that doesn't test the contract — it tests a copy.

**Required action:** Replace with a test that calls `brief::run(...)` against an in-memory schema with all four roles declared, captures the returned `Err`, and asserts on `err.to_string()`.

---

### m1 — `next_action::run` calls `stores_dir_for` and discards the result (MINOR)

**Where:** `src/handlers/next_action.rs:67-72`.

```rust
// AC4.5 (task 4.5): validate scope-aware path resolution is consistent.
// We call stores_dir_for to satisfy the "both verbs call paths::stores_dir_for(scope)"
// requirement.  The result is used only to confirm the path is resolvable; the
// caller has already opened `conn` against the correct DB.
let _ = stores_dir_for(schema.scope)?;
```

The comment is candid: this line exists *only* to satisfy the literal task wording. The DB connection has already been opened via `db_path()` (which itself uses scope-aware resolution); throwing away the resolved path adds no behavioral value. Two options:
1. Remove the line. Task 4.5's intent ("a workflow store installed under `scope: repo` works from any worktree") is already satisfied by `db_path()`'s scope-awareness in dispatch.rs.
2. Keep the line and use the resolved path for something — e.g., `brief.rs` uses it as a fallback when the manifest entry is missing (next_action.rs has no analogous use).

**Recommendation:** delete and update the task-completion log to note that scope handling is enforced by the dispatcher's `db_path()` call.

---

### m2 — `brief::run` will fail for bundled workflow stores (LOW / latent)

**Where:** `src/handlers/brief.rs:107-126`.

`brief::run` resolves the store root from `manifest.stores[].schema_path`. For non-bundled stores this is the canonical filesystem directory. For bundled stores it's a sentinel like `bundled:observations` (set in `install.rs:131, 157`). If a bundled store ever declares a workflow (Phase 6 adds the `tasks` store, which the plan describes as bundled), `brief` will join the sentinel with the template path and try to `read_to_string("bundled:tasks/templates/planner-brief.md.tpl")` — guaranteed I/O error.

No bundled store has a workflow today, so this doesn't gate Phase 4. But it's a footgun for Phase 6's `tasks` store. The fix path is one of:
- Detect `schema_path` starting with `"bundled:"` and route to `BUNDLED_STORE_TEMPLATES` (Phase 7's lookup map).
- Or, when Phase 5 lands the P2-M1 `WorkflowResolved` threading carry-forward, have `brief` read from the resolved in-memory text instead of the disk path. (Plan already describes this as the eventual design.)

**Recommendation:** add a TODO comment at brief.rs:107 naming this gap so Phase 6 plan-review catches it.

---

### m3 — `next_action::run` duplicates `find_next_agent` logic inline (TRIVIAL)

**Where:** `src/handlers/next_action.rs:126-142`.

The module exports `find_next_agent(workflow, status) -> Option<String>` (used by brief.rs). The same module's `run()` function reimplements the same iter-find loop inline instead of calling its own helper. Five-line refactor; pure smell. Optional fix.

---

### m4 — Test imports unused; warnings on compile (TRIVIAL)

**Where:** `src/handlers/brief.rs:152-154`.

```rust
use crate::db;
use crate::schema::Schema;
use tempfile::tempdir;
```

`crate::db` and `tempfile::tempdir` are imported but never referenced in any test in the module. `cargo build --tests` emits two warnings. This is direct evidence supporting M1 — the executor staged imports for DB-backed handler tests and didn't finish them. Fix: write the missing tests (preferred per M1) or delete the imports.

---

### m5 — AC4.7's exact bail! string is unreachable from CLI (TRIVIAL / informational)

**Where:** `src/handlers/next_action.rs:60-66`, `src/handlers/brief.rs:34-41`.

The plan AC says `next-action` on a non-workflow store errors with `"store '<name>' has no workflow declaration; verb only works on workflow-shaped stores."` Plan-review explicitly allowed CLI-layer gating as an alternative ("Both can be correct"). The executor chose CLI gating; the bail! in the handler is defense-in-depth and is reachable only by direct programmatic call. Acceptable, but no test exercises the bail! — flagged for M1.

---

## Deviation review — `build_context` reserved-column inclusion

The executor's deviation report claims this is a "strictly additive" fix. Verified:

- `RESERVED_ENTRY_KEYS` are inserted **first**, then schema fields overwrite on collision (context.rs:36-51). Code comment says "schema field wins since it's inserted after." The actual ordering in the code matches the claim.
- All 24 render-module tests pass, including `context_top_level_keys_match_schema_plus_engine_key` which now uses a `BTreeSet` to count unique keys (so schema-field/reserved-name collisions don't double-count).
- `planner_brief_fixture_renders_correctly` (the byte-for-byte assertion from Phase 3) still passes.
- The fixture schema removed the `status` schema-field declaration, so collision-on-collision is not actually exercised by any test. If a downstream schema declares a `status` field with a different value than the reserved column, the schema field wins per current ordering — but this is academic given DDL itself prevents such schemas (the duplicate-column SQL error is what motivated the fixture fix in the first place).

Conclusion on the deviation: sound, well-documented in the diff comment, no Phase 3 regressions, and the test coverage for the new behavior is adequate (the assertion that all reserved keys appear in the context is in the existing test).

One note: the **rationale** ("schema field value wins on collision") is technically backwards from a defense-in-depth standpoint — if a schema author accidentally declares `status` as a field with a stale value, it would silently override the live status from the DB. But because DDL would reject such a schema (duplicate column), this code path is unreachable in practice. Worth a one-line code comment clarifying the dead-code nature of the collision branch.

---

## Fixture schema review

Changes:
1. `status` field removed (was duplicating reserved column → DDL error). **Sound.**
2. `auto_increment: true` removed from `current_phase`. Not used anywhere in Phase 4; removing it makes the field a plain framework-managed integer. **Sound for now**, but Phase 5's plan describes auto_increment behavior on `current_phase` (PASS-non-last increments it). If Phase 5 adds tests against this fixture expecting auto_increment, they will need to re-add it. Worth a Phase-5 carry-forward note.
3. New states: `blocked` added; transitions `executing → blocked` (verb: `block`, actor: `human`) and `executing → done` (verb: `finish`, actor: `ai_autonomous`). **Sound** — exercises AC4.6.
4. New fields: `blocked_reason: text`, `claimed_by: text`, `claimed_at: timestamp`. **Sound** — exercises AC4.2 lock semantics and AC4.6's blocked_reason key.
5. `submit_targets[submit-plan]` removed. Phase 4 doesn't use submit verbs; harmless. Phase 5 may need to re-add it. Worth a Phase-5 carry-forward note.

---

## Carry-forward to Phase 5

If REVISE actions land cleanly and Phase 4 closes, the executor of Phase 5 should:

1. Add back `submit_targets[submit-plan]: plan` and `auto_increment: true` on `current_phase` if Phase 5's tests require them. (These were stripped in Phase 4 because the previous fixture had unrelated DDL bugs; Phase 5 will set up the proper testing surface.)
2. Pick up the M1 refactor pattern: split each submit handler into `compute(...) -> Result<Output>` + thin `run(...)` wrapper, so AC tests can assert on structured output.
3. Address m2: when threading `WorkflowResolved` through (P2-M1 carry-forward), have `brief` read the resolved in-memory templates instead of disk.

---

## Required actions (cycle 2)

| Action | Severity | Owner |
|--------|----------|-------|
| Add direct handler-level tests for `next_action::run` and `brief::run` covering all 7 ACs (M1 + M2) | MEDIUM | executor |
| Either remove `let _ = stores_dir_for(schema.scope)?` from `next_action.rs:72` or make it functional (m1) | MINOR | executor |
| Delete unused `crate::db` and `tempfile::tempdir` imports in brief.rs (or write the missing tests; M1 is preferred) | TRIVIAL | executor |
| Optional: refactor `next_action::run` to call `find_next_agent` instead of duplicating (m3) | TRIVIAL | executor |
| Optional: add a TODO comment at brief.rs:107 naming the bundled-store gap (m2) | TRIVIAL | executor |

---

## Gate decision

**REVISE.** Phase 4's runtime behavior is demonstrably correct (verified by direct CLI probe of every AC), but the test suite has a structural gap: the new handlers' `run()` functions are entirely untested. The tests-pass count of 237 includes the new tests, but those tests exercise re-implementations of handler logic, not the handlers themselves. AC4.5 specifically asserts a literal-string contract that is checked against a copy of the bail! template, not the actual error.

Phase 5 will introduce four similar handlers (`submit-plan`, `submit-plan-review`, `submit-execute`, `submit-review`); inheriting this test pattern would compound the gap. Tightening it now — one cycle, additive tests only, no logic changes — is the cheapest fix point.

Cycle 1 of max 3.
