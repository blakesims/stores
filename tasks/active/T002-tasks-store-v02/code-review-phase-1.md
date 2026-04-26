# Code Review — Phase 1 (cycle 1)

## Gate Decision

**Gate:** REVISE
**Reviewed:** 2026-04-26
**Reviewer:** code-reviewer agent
**Cycle:** 1 of max 3
**Status next:** EXECUTING_PHASE_1 (back to executor with specific revision scope)

The implementation lands the bulk of Phase 1 cleanly. 11 of the 12 ACs are genuinely verified by passing unit tests. Test count, e2e, file list, depth-3 walker, ambiguity validation, framework-actor DDL, scope/manifest plumbing, `--invoker framework` rejection — all check out.

However, **AC1.4 has a load-bearing gap that the executor's test substituted around** rather than implementing. The plan's worked-example transcript and AC1.4 explicitly require parsing/evaluating `current_phase < plan.phases.length` (path-vs-path-length comparison), but the parser's RHS only accepts single-quoted literals or bare integers. The executor's test renames the AC to a different (parseable) form — `plan.phases.length > 1` — without flagging the substitution. This will block Phase 5/Phase 7 because the planner specified those exact guards in the schema worked example.

A REVISE cycle resolves this with a small parser extension (or, alternatively, a planner-side decision to flip the guards into a parseable form before Phase 7).

---

## Git Reality Check

**Base commit:** `b8856b8` (handoff doc; just before Phase 1 began)
**HEAD:** `6bee3ed` (master; code committed directly)

### Commits — claimed 9 with `T002 P1.X:` prefix; verified

```
6bee3ed T002 P1: update main.md — status CODE_REVIEW, execution log     ← metadata, no .X
2e8202c T002 P1: manifest + install record scope per InstalledStore     ← no .X
e93eff6 T002 P1.11: framework-actor DDL test
0d12485 T002 P1.10: requires_gate on Transition + ambiguity validation
f5cbf5b T002 P1.9: lift depth limits in read_row / build_entry_map for depth-3 nests
435084c T002 P1.6: paths.rs — StoreScope-aware stores_dir_for + git_common_dir
4ef518e T002 P1.4: expr_eval.rs — evaluate guard Exprs against EntryMap
ae3aef0 T002 P1.2+1.3+1.5+1.7+1.8: schema features — auto_increment, expr.rs, scope, ListRecord, ListFk
bcd5b84 T002 P1.1: actor: framework enum value + invoker rejection
```

9 total commits match. Strictly only **7** carry the `T002 P1.X:` (numbered) prefix; the manifest commit and metadata commit use `T002 P1:`. Minor labeling slop, not a blocker.

### Files — claimed 18 modified; verified

`git diff --stat b8856b8..HEAD` shows **21** files. Subtracting `Cargo.lock` (transitive deps for `tempfile`), `Cargo.toml` (1-line dev-dep add), and `tasks/active/T002-tasks-store-v02/main.md` (task doc) leaves **18 source files**, matching the executor's claim:

| File | Plan-listed | Status |
|---|---|---|
| `src/schema/actor.rs` | yes | extended ✓ |
| `src/schema/mod.rs` | yes | extended ✓ |
| `src/schema/expr.rs` | yes (new) | new ✓ |
| `src/schema/required_when.rs` | yes | minimal extension (5 lines) ✓ |
| `src/schema/lifecycle.rs` | yes | extended ✓ |
| `src/validate/expr_eval.rs` | yes (new) | new ✓ |
| `src/validate/actor.rs` | yes | extended ✓ |
| `src/validate/mod.rs` | implied | 1-line `pub mod expr_eval;` ✓ |
| `src/validate/required.rs` | not in plan | mechanical Field-init update ✓ |
| `src/validate/enum_check.rs` | not in plan | mechanical Field-init update ✓ |
| `src/validate/regex_check.rs` | not in plan | mechanical Field-init update ✓ |
| `src/handlers/row.rs` | yes | extended (depth-agnostic) ✓ |
| `src/handlers/schema_show.rs` | not in plan | 2-line addition (ListRecord/ListFk arms) ✓ |
| `src/cli/dispatch.rs` | yes | extended ✓ |
| `src/paths.rs` | yes | extended ✓ |
| `src/manifest.rs` | yes | extended ✓ |
| `src/install.rs` | not in plan | 2-line additions (pass scope) ✓ |
| `src/codegen/ddl.rs` | implied via 1.11 | extended ✓ |

`required.rs`, `enum_check.rs`, `regex_check.rs`, `schema_show.rs`, `install.rs`, `validate/mod.rs` are mechanical follow-on edits of the form "add the new struct fields to test fixtures / add the new variants to a match arm". All defensible. None of these constitute hidden refactoring — the diffs are 1-5 lines each.

### Working tree

`git status -s` shows only:
- `tasks/global-task-manager.md` (M) — orchestrator updated externally
- `tasks/active/T002-tasks-store-v02/plan-review.md` (??) — phase-2 artefact

No code changes left uncommitted.

---

## AC Verification Table

| AC | Claim | Verified | Test location |
|---|---|---|---|
| 1.1 | `framework` parses; `--invoker framework` rejected; `Op::Add` at framework field by human fails | YES | `src/schema/actor.rs:56-66`, `src/cli/dispatch.rs:115-127`, `src/validate/actor.rs:217-233` |
| 1.2 | non-integer `auto_increment` fails; missing target fails; self-cycle fails | YES | `src/schema/mod.rs:609-693` (`auto_increment_*`) |
| 1.3 | `parse_guard("phases.length < 4")`, `parse_guard("current_cycle <= 4")` parse; `OR` rejected | YES | `src/schema/expr.rs:245-291` |
| 1.4 | `eval(parse_guard("current_cycle <= 4"), …4)` → true; `…5` → false; `eval(parse_guard("current_phase < plan.phases.length"), …)` → true (depth-3) | **PARTIAL — see C1** | `src/validate/expr_eval.rs:147-183` |
| 1.5 | `scope: repo` parses; missing key → Worktree; unknown errors | YES | `src/schema/mod.rs:697-749` |
| 1.6 | `stores_dir_for(Repo)` resolves to `.git/`'s parent + `.stores`; outside-git errors | YES | `src/paths.rs:163-235` (incl. tmp git repo creation) |
| 1.7 | `cycles: list_record` with nested record/list_text parses; DDL is single TEXT; round-trip add/show/update | YES (parse + DDL); show verified, update not exercised in this phase | `src/schema/mod.rs:751-789`; `src/handlers/row.rs:341-420` |
| 1.8 | `depends_on: list_fk, ref: tasks` parses; DDL TEXT; round-trip Vec<String>; no referential check | YES | `src/handlers/row.rs:422-446`, `src/schema/mod.rs:791-828` |
| 1.9 | `read_row` round-trips `plan.phases[2].name` and `cycles[1].executor.summary` | YES | `src/handlers/row.rs:351-420` |
| 1.10 | `requires_gate: PASS` parses; two `(from, verb, requires_gate=None)` errors | YES | `src/schema/lifecycle.rs:108-192` |
| 1.11 | `actor: framework` field of type text/integer/timestamp produces same DDL as non-framework | YES | `src/codegen/ddl.rs:194-235` |
| 1.12 | All 110 existing tests still pass | YES (174 total, 0 failed) | `cargo test` output: 174 passed |

---

## Issues by Severity

### Critical (1)

**C1. AC1.4 path-vs-path-length form is not implementable by the parser; the executor's test renamed the AC.**

- **Where:** `src/schema/expr.rs:212-237` (parse_rhs); `src/validate/expr_eval.rs:162-183` (test renaming).
- **What the plan/AC says:** AC1.4 line 163 reads:

  > `eval(parse_guard("current_phase < plan.phases.length"), entry_with_current_phase_1_and_2_phases)` is `true` (depth-3 path lookup).

  The Phase 5/Phase 7 worked-example transcript (main.md lines 982-987 and 1001-1005) **literally calls** these two guards on the schema:

  ```yaml
  guard: current_phase < plan.phases.length
  guard: current_phase >= plan.phases.length
  ```

  Both have `path` on the LHS and `path.length` on the RHS.
- **What the parser does:** `parse_rhs` (`expr.rs:212-237`) only accepts `'literal'` (single-quoted) or a bare integer (`raw.parse::<i64>()`). A path expression like `plan.phases.length` on the RHS fails both branches and falls through to `bail!("guard RHS must be a single-quoted literal or an integer; got: {raw}")`.
- **What the test does:** the test named `current_phase_lt_plan_phases_length_depth3` (`expr_eval.rs:161-174`) does NOT test `parse_guard("current_phase < plan.phases.length")`. It tests `parse_guard("plan.phases.length > 1")` — putting `.length` on the LHS and an integer on the RHS, which the parser does support. The test name is misleading; the AC's specific expression form is not exercised.
- **Why this blocks Phase 5/7:** Phase 7's tasks schema (per the worked-example transcript) declares `guard: current_phase < plan.phases.length` directly in YAML. When that schema is loaded, `Schema::from_yaml` will eventually call `parse_guard` on that string and bail. Phase 5's submit-review handler will never get the chance to evaluate it. The smoke-test won't run.
- **Resolution options (executor picks):**
  1. **Extend the grammar** (smallest change): allow `Rhs::PathLength(Vec<String>)` and `Rhs::Path(Vec<String>)`. Update `parse_rhs` to detect a dotted-identifier RHS and recognise the optional `.length` suffix, mirroring `parse_lhs`. Update `eval_path` and `eval_length` to look up the RHS value the same way they look up the LHS, comparing two dynamic values. Locked subset D6 says "length operators" without specifying which side `.length` lives on — the form chosen here is consistent with D6's intent.
  2. **Planner-side rewrite:** ask the planner to flip the guards into the parseable form `plan.phases.length > current_phase` etc. — but `current_phase` is also a path, not a literal, so this requires the same grammar extension. So option 2 collapses into option 1.
  3. **Schema-author works around it:** require that all guards be of the form `path OP integer-literal` — meaning the schema author hard-codes the phase count (e.g. `current_phase < 7`). Brittle and contradicts D6's intent of declarative length comparisons. NOT recommended.

  Recommend option 1. Adds ~30 LOC to `expr.rs` and a path-vs-path arm to `expr_eval.rs::eval_path` / `eval_length`. Add tests:
  - `parse_guard("current_phase < plan.phases.length")` returns the expected AST.
  - `eval(...)` on `{current_phase: 1, plan: {phases: [_, _]}}` returns `true`.
  - `eval(...)` on `{current_phase: 2, plan: {phases: [_, _]}}` returns `false`.
- **Why I'm flagging this critical, not minor:** Phase 5/7 cannot proceed without this. Phase 1 claims AC1.4 PASS but only verifies half of it. The other half is exactly the form the worked-example transcript depends on. If we let this through, Phase 7's smoke-test schema will fail to load.

### Major (2)

**M1. Phase 1's "single AST type" intent is not literally satisfied; deviation rationale is plausible but understated.**

- **Where:** `src/schema/required_when.rs:32-35` keeps the narrower `Expr { lhs_path, rhs_literal }` while `src/schema/expr.rs:58-62` defines a wider `Expr { lhs: Lhs, op: Op, rhs: Rhs }`. The two are different Rust types. The re-export is `pub use crate::schema::expr::Expr as GuardExpr` — an alias for the *new* one, not unification with the *old* one.
- **What the plan said** (1.3, line 150): "Re-export `Expr` from `expr.rs` and have `required_when.rs` `pub use` it where the AST overlaps so there's a single AST type."
- **What was delivered:** Two distinct AST types. `required_when.rs::Expr` retains its narrower shape so existing callers (`validate/required.rs:55`, `handlers/schema_show.rs:83-84,180-181`) keep working unchanged.
- **Executor's defense:** documented in main.md line 1092. "Existing call sites use `RequiredWhenExpr` alias unchanged. The plan's 'single AST type' intent is satisfied at the module level via re-export without breaking `e.lhs_path` / `e.rhs_literal` access."
- **Reviewer's read:** the alias does NOT unify the types. `validate/required.rs:55` cannot accept a `GuardExpr` value — it pattern-matches on the narrower struct. Phase 5's transition handler will need an explicit conversion or two parallel code paths (one for `required_when`, one for guards). This is workable but worth surfacing as a known cost — the plan's "single AST" intent is genuinely deferred, not "satisfied at module level."
- **Recommendation:** no code change required this cycle. Add a short note in main.md's deviation list clarifying that Phase 5 will need to bridge the two ASTs (either by widening `required_when.rs::Expr` to an alias of `expr::Expr` then, or by using two parallel code paths). The longer this stays "two ASTs," the more likely Phase 5's guard-evaluation refactor surfaces hidden assumptions.

**M2. `validate/mod.rs::validate_field` does NOT recurse into `FieldType::ListRecord(_)` sub-fields.**

- **Where:** `src/validate/mod.rs:67-76`. The walk recurses into `FieldType::Record(_)` only. `FieldType::ListRecord(_)` element fields are skipped at runtime entry validation.
- **Why it matters:** Phase 1 added `ListRecord` storage (DDL TEXT, JSON round-trip). The runtime validator does not exercise required/required_when/enum/pattern/actor checks on element fields inside a `ListRecord`. As long as Phase 1 only ships the type without flow that writes into ListRecord cells, this is benign — there is no path that fires a write that needs validating.
- **Phase 5 consequence:** when `submit-execute` writes into `cycles[].executor.summary`, the validator will silently pass because it never walks into ListRecord element fields. Phase 5 must extend `validate_field` (or use a separate code path for `Op::SubmitExecute`) to recurse into the new variant.
- **Why major, not critical:** this does not break Phase 1 ACs. AC1.7's round-trip test exercises read/write at the storage layer, which works. But since this is a known foundation issue that Phase 5 will encounter, surfacing it now in the deviation list (and possibly adding a passing-but-marked TODO test) lowers the cycle-2/3 risk for Phase 5.
- **Recommendation:** add a `// TODO(phase-5): recurse into ListRecord sub-fields` note at `validate/mod.rs:70`, and consider one defensive test asserting current behaviour (a required field inside a ListRecord element does NOT fire a runtime validation error in Phase 1) — pinning the contract so Phase 5's change is visible.

### Minor (3)

**m1. AC1.4 test name is misleading.**

- **Where:** `src/validate/expr_eval.rs:161-174` and `:177-183`.
- **What:** `current_phase_lt_plan_phases_length_depth3` and `current_phase_lt_plan_phases_length_false` test `parse_guard("plan.phases.length > 1")`, not `parse_guard("current_phase < plan.phases.length")`. Future readers will be confused.
- **Fix:** rename to e.g. `plan_phases_length_gt_constant_depth3` once C1's grammar extension lands. Keep these tests AND add the AC1.4-form tests separately.

**m2. AC1.7's "update replaces a list element correctly" round-trip is not exercised.**

- **Where:** AC1.7 says: "`update` replaces a list element correctly." The implemented tests cover `read_row` (deserialise) and DDL emission. There is no test in `src/handlers/row.rs` (or `update.rs`) that covers writing `cycles[N]` with a different element value and reading it back.
- **Why minor not major:** AC1.7's round-trip narrative was about stable storage, which works (the JSON column round-trips identity). The `update` semantics for ListRecord is naturally a Phase 5 concern (the submit handlers append to the list). For Phase 1, `add` + `show` round-trip is sufficient evidence the storage is correct.
- **Fix:** clarify in main.md that the Phase-1 scope is JSON-as-TEXT round-trip (not list-element mutation), or add a tiny `update`-style test to `handlers/update.rs` that overwrites a `cycles` JSON value end-to-end.

**m3. Commit-prefix labelling drift.**

- **Where:** commits `2e8202c` ("T002 P1: manifest + install record scope per InstalledStore") and `6bee3ed` ("T002 P1: update main.md — status CODE_REVIEW, execution log") use `T002 P1:` (no `.X`).
- **Why:** the executor's "9 commits with `T002 P1.X:` prefix" claim is technically wrong — only 7 are numbered. The manifest commit corresponds to plan task 1.6's manifest follow-on (not a new task) and the metadata commit is housekeeping. Both fine; just don't claim `T002 P1.X:` if the commit is `T002 P1:`.
- **Fix:** none required. Future cycles, label commits accurately.

---

## What's Good

- **Test count is real.** `cargo test` reports 174 passing, 0 failed. Test additions are dense (64 new tests) and exercise both happy-path and error paths for each new feature.
- **Depth-3 walker correctness.** The `read_row` rewrite is depth-agnostic — it round-trips arbitrary nested JSON via `serde_json::from_str::<Value>`, no `path.len() <= 2` guards remain anywhere in `src/handlers/row.rs`. Tests exercise depth-3 specifically (`plan.phases[2].name` and `cycles[1].executor.summary`).
- **Ambiguity validation.** `validate_transition_ambiguity` is the right shape — it groups by `(from, verb)` and counts unguarded transitions, allows N-1 of them. Test coverage includes the ambiguous case AND the "one unguarded is allowed" non-pathological case (`lifecycle.rs:177-192`).
- **`stores_dir_for(Repo)` outside-git errors hard.** Per Q7 user decision (B = hard error), the implementation does NOT fall back to cwd. Test `stores_dir_for_repo_errors_outside_git` (`paths.rs:188-202`) verifies this. Lock-protected against test races.
- **`from_env` never returns Framework.** Test `from_env_never_returns_framework` (`actor.rs:82-87`) is the right shape — env can never resolve to the engine-internal actor.
- **`auto_increment_within: <self>` cycle check.** Tested at `src/schema/mod.rs:673-693` with a clear error message ("auto_increment_within cannot reference itself").
- **`--invoker framework` rejection error message.** Cites "internal actor" and "framework" — matches AC1.1 wording closely.
- **e2e passes.** All 13 DONE_WHEN steps green when `CLAUDECODE` is unset. Pre-existing inheritance issue is correctly noted, not introduced by this phase.
- **Minimal collateral damage.** Mechanical Field-init updates in `validate/{enum_check,regex_check,required}.rs` and the 2-line `schema_show.rs` arm are exactly what the new struct fields require. No surprise refactoring.

---

## Required Actions for REVISE

In priority order:

1. **(Critical, C1)** Extend `parse_guard` and `eval` to support a `.length` (or path) RHS. Concretely:
   - Add `Rhs::Path(Vec<String>)` and `Rhs::PathLength(Vec<String>)` variants in `src/schema/expr.rs`.
   - In `parse_rhs`: after the integer/literal branches, attempt to parse the RHS as a dotted identifier (with optional `.length` suffix) — mirror `parse_lhs`.
   - In `src/validate/expr_eval.rs::eval_path` and `eval_length`: when RHS is a path, look it up and compare dynamic values. Both sides may be missing → `false`. Mixed types (string-vs-int) → `false`.
   - Add tests asserting AC1.4 specifically:
     - `parse_guard("current_phase < plan.phases.length")` returns the expected AST (Lhs::Path + Rhs::PathLength).
     - `eval` on `{current_phase: 1, plan: {phases: [{}, {}]}}` returns `true`.
     - `eval` on `{current_phase: 2, plan: {phases: [{}, {}]}}` returns `false` (`2 < 2` is false).
     - `eval` on entry missing `current_phase` returns `false`.
2. **(Major, M1)** Update main.md's Phase 1 deviation note to clarify that the "single AST type" intent is genuinely deferred to Phase 5. The current note ("intent is satisfied at module level") understates the cost. One paragraph.
3. **(Major, M2)** Add a `// TODO(phase-5)` comment at `src/validate/mod.rs:70` and (optionally) one test pinning the current "ListRecord sub-fields not validated at runtime" behaviour. This makes Phase 5's required change visible.
4. **(Minor, m1)** Rename the misleading test or add the AC1.4-form-specific tests as in (1).
5. **(Minor, m2)** Either clarify AC1.7's `update` scope in main.md or add a thin test exercising `update` on a `cycles` JSON cell.
6. **(Minor, m3)** No action needed; future commits should use the actual task number in the prefix.

After fixes, re-run `cargo test` (expect 174 + ~3 new) and confirm AC1.4 tests pass. No e2e change expected.

---

## Learnings (Cycle 1 → Cycle 2 input)

- **Test names that match an AC are load-bearing documentation.** The test `current_phase_lt_plan_phases_length_depth3` was named after the AC but its body tested a different expression form. A reviewer scanning test names would assume coverage where there is none. Recommendation for future cycles: when an AC text is verbatim a parse_guard input, the test body must call `parse_guard` with that exact string.
- **D6 (locked grammar) was under-specified about which side `.length` lives on.** D6 reads "Equality (`==`, `!=`) + `.length <`, `.length <=`, ..." which an honest reader could parse as either "LHS has `.length`" or "operator family includes `.length`." The plan's worked-example transcript clarifies both sides, but the executor's parser interpreted it narrowly. Cycle-2 plan revisions should make grammar shape explicit in D6.
- **The `required_when` ↔ `expr` AST split surfaced as expected.** The plan flagged this would happen (m1 deviation in plan-review cycle 2). It did. Phase 5 will need an explicit unification or bridging step. Carrying this as a TODO is fine; pretending it is "satisfied at module level" is not.
