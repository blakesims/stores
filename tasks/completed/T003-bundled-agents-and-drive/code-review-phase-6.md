# Code Review — Phase 6: Guide handlers (`gate <id> guide` full + `tasks <id> guide` stub)

- **Cycle:** 1 of 3
- **Reviewer:** code-reviewer
- **Date:** 2026-04-27
- **Commits under review:** 44a58bd (impl), 47845a3 (log-only)
- **Diff scope:** `git diff 181505b..HEAD -- src/` → 4 files, +1098 / -2

## Gate: PASS

Counts: **0 Critical / 0 Major / 4 minor (cosmetic / coverage / DRY)**

## AC verification table

| AC   | Statement                                                                                                                                                                              | Status | Evidence                                                                                                                                                                                                                                                                              |
|------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 6.1  | `stores gate <id> guide --mock <fixture>` builds bundle with gate row + linked task row + authorized verbs.                                                                            | PASS   | `build_gate_brief` (guide.rs:338-393) emits Gate Row, Linked Task Row, Recent Plan Reviews, Last 2 Code Review Cycles, and verbs list. `gate_bundle_contains_gate_row_and_verbs` and `gate_bundle_without_linked_task` assert all 5 AUTHORIZED_VERBS substrings + FORBIDDEN clause.   |
| 6.2  | `stores tasks <id> guide --mock <fixture>` builds bundle with task row + last next-action + last review.                                                                              | PASS   | `build_tasks_brief` (guide.rs:398-437) emits Task Row, Last Next-Action Output, Last Review Cycle. `tasks_bundle_contains_task_and_next_action` + `tasks_bundle_with_review_cycle` cover both empty + populated review cycle paths.                                                  |
| 6.3  | `cargo test handlers::guide` covers both bundle shapes with fixture rows.                                                                                                              | PASS   | 16/16 pass; both `gate_bundle_*` and `tasks_bundle_*` use real schemas + DDL + INSERT fixtures.                                                                                                                                                                                       |
| 6.4  | Parser-level test that prompt body contains 5 authorized verbs + forbid clause + no-direct-edit instruction.                                                                          | PASS   | 3 dedicated tests: `guide_prompt_contains_authorized_verbs` (5/5 substrings), `guide_prompt_contains_forbid_clause` (4-way OR — robust to variants), `guide_prompt_contains_no_direct_edit_instruction` (`stores gate answer` substring or "NOT edit"). `agents/guide.md` contains "FORBIDDEN" 1×, "MUST NOT" 2×, all 5 verbs in the Authorized section. |
| 6.5  | `gate guide` exits 0 if status `pending → answered`; otherwise 1.                                                                                                                      | PASS   | `check_gate_transition` pure function (guide.rs:174-188) tested 4 ways: pending→answered (Ok), pending→pending (Err), answered→answered (Err), pending→cancelled (Err). Handler `run_gate_guide_with_runner` (guide.rs:209-274) wires status_before / spawn / status_after / check_gate_transition. Plus runner-error path tested. See verdict below.       |
| 6.6  | `tasks guide` documented as v0.3 stub with v0.4 expansion noted.                                                                                                                       | PASS   | Doc-comment on `run_tasks_guide` (guide.rs:108-110) and `run_tasks_guide_with_runner` (guide.rs:281-283) says "v0.3 stub-quality form: dumps context bundle, no specialized tooling. Full form expected in v0.4." Module-level doc-comment also classifies the two forms. agents/guide.md §"Note on v0.3 Stub Quality" reinforces. |

Full suite: **354/354** passed (was 338 → +16 new). Re-run twice: no flakes. `cargo build` clean (default + `runner-claude-code` feature + `--no-default-features`). Live `--help` confirms `--mock` hidden, `--claude-code` arg absent without feature compiled in.

## AC6.5 test coverage verdict — ACCEPTED

The executor flagged that `MockRunner::spawn` cannot synchronously update the DB the way a real `claude -p` session would (by shelling `stores gate answer` mid-spawn). Their workaround: extract `check_gate_transition(display_id, status_before, status_after)` as a pure function and unit-test the policy directly with all 4 transition cases, plus a separate handler-level test of `run_gate_guide_with_runner` that exercises the wiring (status_before read → spawn → status_after read → policy check) in the failure path.

This composition genuinely covers AC6.5's intent for v0.3:

1. **Policy correctness** — `check_gate_transition` exhaustively unit-tests the single-bit transition (pending→answered = Ok; everything else = Err). All 3 error paths the AC implies are hit: stayed-pending, transitioned-to-other-state (cancelled), and pre-answered (answered→answered, which guards against false-positive "guide ran on already-resolved gate").
2. **Wiring correctness** — `gate_guide_exits_one_when_gate_not_answered` and `gate_guide_exits_one_on_runner_error` exercise the full handler with a mock runner; the first verifies the post-spawn status read sees an unchanged DB (correct Err), the second verifies non-zero runner exit short-circuits before the transition check.
3. **Read-back correctness** — `gate_guide_exits_zero_when_gate_answered_by_db_update` is honestly a degenerate test (its own comment block admits the integration path is uncovered by the mock runner); it ends up only verifying `read_gate_status` reads a freshly-updated DB row. **Acceptable as v0.3 coverage** but the test name oversells what it does — recorded as **m4**. The real integration smoke is deferred to manual e2e against a live `claude -p` runner (out of scope for unit tests).

For v0.3, this is the right cut. The full integration loop (mock runner that can call back into the DB) would require either threading a callback through the `Runner` trait or running the real `claude` CLI in CI — both disproportionate. Pure-function + handler-wiring + read-back is the correct decomposition for the constraint.

## `gate`-store special-case verdict — CLEAN ENOUGH SEAM

The executor reports three coordinated changes to register `guide` on `gate`:

1. `WORKFLOW_VERBS` list in `dynamic.rs:202-205` includes `guide` (alongside `next-action`, `brief`, `drive`, `status`, etc.).
2. Workflow-bearing schemas auto-register `guide` via `if schema.workflow.is_some()` block (line 216-228 ends with `.subcommand(build_guide_cmd())`).
3. **An explicit `if schema.name == "gate"` branch** at line 232-234 also registers `guide` for the `gate` store, which has **no `workflow:` declaration**.

**Why a generic mechanism would be over-engineering for v0.3:** `gate` is the only non-workflow store that needs `guide`. The other bundled non-workflow store (`observations`) does not need it. Introducing a schema-level `extra_verbs:` field or a `guide_eligible: true` flag to drive registration generically would (a) widen the schema surface for one consumer, (b) require migration logic for legacy schemas, (c) duplicate the per-verb registration plumbing already established by the workflow-verbs list. The 3-line `if schema.name == "gate"` branch is honest about being a hard-coded special case for v0.3 and is documented as such by the function-doc on `build_guide_cmd` (lines 497-500: "Registered on both `gate` (full form) and `tasks` (stub form)").

**Subtle bug surface — recorded as m1:** the dedup logic at line 253 (`if schema.workflow.is_some() && WORKFLOW_VERBS.contains(&verb.as_str())`) only suppresses duplicate `guide` transition-verb registrations on workflow schemas. If someone adds a `guide` lifecycle transition to a non-workflow schema (e.g. `gate`) in the future, the explicit `gate`-branch registration plus the transition-verb registration would both fire and produce a clap "subcommand exists" panic. Today this is unreachable (gate has no `guide` transition), but the dedup invariant is asymmetric. Defensible for v0.3; flag for v0.4 cleanup if the schema model grows.

**Conclusion:** clean enough seam for v0.3. The honesty of "`gate` is special — these 3 lines wire it" beats premature abstraction.

## Public-API delta

`git diff 181505b..HEAD -- src/` introduces 6 new public items, all in `handlers::guide`:

- `pub mod guide;` (mod.rs)
- `pub struct MockFixtureItem` — duplicated shape from `handlers::drive::MockFixtureItem` (recorded as **m2** — DRY)
- `pub struct GateGuideArgs`, `pub struct TasksGuideArgs` — args structs
- `pub fn run_gate_guide`, `pub fn run_tasks_guide` — entry points called from `dispatch.rs`
- `pub(crate) const AUTHORIZED_VERBS`, `pub(crate) fn check_gate_transition`, `pub(crate) fn read_gate_status`, `pub(crate) fn run_gate_guide_with_runner`, `pub(crate) fn run_tasks_guide_with_runner`, `pub(crate) fn build_gate_brief`, `pub(crate) fn build_tasks_brief` — test-visible helpers, properly crate-scoped.

No widening of pre-existing surface. The duplicated `pub MockFixtureItem` is the only minor concern (binary-internal module, low blast radius — the type is structurally identical to drive's, so a future caller that confuses them gets identical behaviour).

## Findings

### Critical (0)
_None._

### Major (0)
_None._

### Minor (4) — non-blocking

- **m1**: Dedup-list asymmetry in `dynamic.rs:253` — `WORKFLOW_VERBS` skip applies only to `schema.workflow.is_some()` paths. The `if schema.name == "gate"` branch at line 232-234 bypasses any dedup. Today unreachable (`gate` has no `guide` transition); flag for v0.4 if the schema model grows extra_verbs / guide_eligible. (See "gate-store special-case verdict" above.)
- **m2**: `MockFixtureItem` is now a duplicated `pub struct` in both `handlers::drive` (drive.rs:125) and `handlers::guide` (guide.rs:58). Identical shape. Lift to `handlers::common` or `runner::fixture` in v0.4 — DRY hygiene. Not worth a follow-up commit in cycle 1.
- **m3**: `extract_plan_review_log` (guide.rs:467-473) dumps the **entire** `plan_review_log` array, not "recent" as AC6.1 reads ("recent plan-review log"). For v0.3 task fixtures (≤3 cycles in practice), the bundle stays small. Spec wording is loose ("recent" is undefined window) so this is interpretation, not defect. If logs grow long, consider `.iter().rev().take(2)` like `extract_last_n_cycles`.
- **m4**: Test `gate_guide_exits_zero_when_gate_answered_by_db_update` (guide.rs:917-957) is poorly named — its body comment honestly admits it cannot drive the mock runner to update the DB, so the test ends up only verifying `read_gate_status` round-trips a freshly-updated row. Either rename to `read_gate_status_picks_up_db_writes` or delete (the policy is fully covered by the 4 `check_gate_transition_*` tests). Cosmetic — does not weaken the AC6.5 coverage matrix.

## Test-suite snapshot

| Run                                                  | Tests | Result |
|------------------------------------------------------|-------|--------|
| `cargo test handlers::guide`                         | 16    | PASS   |
| `cargo test` (full suite, run 1)                     | 354   | PASS   |
| `cargo test` (full suite, run 2 — flake check)       | 354   | PASS   |
| `cargo build` (default features)                     | —     | clean  |
| `cargo build --features runner-claude-code`          | —     | clean  |
| `cargo build --no-default-features`                  | —     | clean  |
| Live: `stores gate G001 guide --claude-code` (no feature) → unknown-flag rejection | — | confirmed |

## Phase 7 readiness

`run_gate_guide(schema, GateGuideArgs)` and `run_tasks_guide(schema, TasksGuideArgs)` are stable public entry points. Dispatch wiring at `dispatch.rs:137-164` is final. Phase 7's e2e tests can drive `--mock <fixture>` against either form without further wiring changes. No regressions to phases 1-5 (`cli::agents`, `runner::*`, `handlers::drive`, `cli::setup`, `handlers::status` all green).
