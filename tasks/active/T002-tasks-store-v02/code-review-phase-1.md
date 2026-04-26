# Code Review — Phase 1 (cycle 2)

## Gate Decision

**Gate:** PASS
**Reviewed:** 2026-04-26
**Reviewer:** code-reviewer agent
**Cycle:** 2 of max 3
**Status next:** EXECUTING_PHASE_2

Cycle 1's critical AC1.4 gap is closed properly. The parser accepts `current_phase < plan.phases.length` directly, the eval logic compares dynamic values on both sides, and the test names now match what they assert. The two majors (M1 — single-AST unification; M2 — ListRecord validator walker) are explicitly deferred to Phase 5 with accurate deviation notes, a `TODO(phase-5)` comment at the gap, and a self-naming pinning test that will FAIL when Phase 5 closes the gap. The deferrals are acceptable on Phase-1 cohesion grounds but Phase 5's plan MUST surface them as concrete tasks before that phase begins — see Learnings.

184 unit tests pass; e2e all 13 steps pass.

---

## Git Reality Check

**Cycle 1 head:** `6bee3ed`
**Cycle 2 commits (atop cycle 1):**

```
ec66f46 T002 P1.cycle2: update main.md — status CODE_REVIEW, cycle-2 execution log
a758662 T002 P1.cycle2: C1 parser+eval — Rhs::Path/PathLength; m1 rename tests; M2 ListRecord TODO + pinning test; m2 update round-trip
```

Two commits, matches the executor's claim. `a758662` is the substantive code+test diff (5 files, +323/-13). `ec66f46` is metadata-only (main.md +18/-1). Commit message naming uses `T002 P1.cycle2:` — slight inconsistency with cycle-1's `T002 P1.X:` style, but per cycle-1 m3 the executor declined to amend; not a blocker.

`git diff --stat HEAD~2..HEAD` confirms no unrelated files touched. Working tree shows only `tasks/global-task-manager.md` (M, orchestrator-managed) and `tasks/active/T002-tasks-store-v02/plan-review.md` (?? — Phase-2 artefact, not this review's concern).

---

## Per-fix Verification

### C1 (critical → fixed) — `parse_guard("current_phase < plan.phases.length")` parses and evaluates

**Parser side (`src/schema/expr.rs`):**

- New `Rhs::Path(Vec<String>)` and `Rhs::PathLength(Vec<String>)` variants added.
- `parse_rhs` now falls through (after literal + integer) to a dotted-identifier branch that mirrors `parse_lhs`: detects optional `.length` suffix, validates `[A-Za-z0-9_.]` and rejects leading/trailing dots, then constructs `Rhs::PathLength` or `Rhs::Path`.
- The error message updated to "single-quoted literal, an integer, or a dotted path (optionally ending in .length)" — accurate.
- Doc comment on `parse_guard` updated to reflect the wider grammar.
- `Rhs::Literal` paired with `Lhs::PathLength` is still rejected at parse time with a clear "length comparison requires an integer RHS" error — the path-RHS extension didn't loosen this safety check.

**Parser tests (in `src/schema/expr.rs`):**

- `parse_current_phase_lt_plan_phases_length` calls `parse_guard("current_phase < plan.phases.length")` — the EXACT AC1.4 string — and asserts `Lhs::Path(["current_phase"])`, `Op::Lt`, `Rhs::PathLength(["plan", "phases"])`. This is the marquee test cycle 1 demanded. Verified.
- `parse_current_phase_ge_plan_phases_length` covers the M9-companion form for the dual PASS-transitions described in Phase 7's schema.
- `parse_path_eq_path` covers the `a == b` form (path-vs-path consistency check).

**Eval side (`src/validate/expr_eval.rs`):**

- New `rhs_as_i64` helper resolves `Rhs::Integer` and `Rhs::PathLength` to `Option<i64>` (returns `None` on missing path or non-collection value). Centralises numeric-RHS resolution; clean.
- `eval_path` now has two new arms:
  - `Rhs::PathLength`: requires LHS to be a JSON `Number(i64)`; resolves RHS via `rhs_as_i64`; calls `compare_i64`. Missing/non-numeric → `false`. Correct behaviour for `current_phase < plan.phases.length`.
  - `Rhs::Path`: looks up RHS path; if both sides are `Number(i64)` compare numerically, if both are `String` compare lexicographically with all six ops, otherwise `false`. The mixed-type-and-missing-path safety is consistent with the rest of the evaluator's "missing → false, never panic" philosophy.
- `eval_length` extended symmetrically — `Rhs::PathLength` and `Rhs::Path` arms both resolve to `i64` (latter requires the path to be a `Number`; otherwise `false`).

**Eval tests (in `src/validate/expr_eval.rs`):**

- `current_phase_lt_plan_phases_length_true`: `{current_phase: 1, plan: {phases: [{}, {}]}}` → `true`. **Exact AC1.4 form, exact AC1.4 entry shape.** Verified.
- `current_phase_lt_plan_phases_length_false_equal`: `{current_phase: 2, plan: {phases: [{}, {}]}}` → `false`. The "boundary equals length" case that selects the `→ complete` transition in Phase 5. Verified.
- `current_phase_lt_plan_phases_length_missing_phase_returns_false`: omits `current_phase` from the entry; eval returns `false` rather than panicking. Defensive.
- `path_eq_path_true` / `path_eq_path_false`: symmetric coverage of the `a == b` path-vs-path form.

**Phase 5/Phase 7 unblock check:** With these changes, the Phase 7 schema lines

```yaml
guard: current_phase < plan.phases.length
guard: current_phase >= plan.phases.length
```

now load through `parse_guard` without bailing. Phase 5's submit-review handler can call `eval(guard, &merged_entry)` against the bumped `current_phase` and the schema's `plan.phases` list, and get the right boolean to drive transition selection. The C1 critical is fully resolved.

**Side-effect check:** None of the 174 prior tests broke; cycle-1's `plan_phases_length_gt_constant_*` tests (the renamed-from-misleading ones) still pass. The `Rhs::Literal` + `Lhs::PathLength` rejection still fires at parse time. No regressions.

### m1 (minor → fixed) — test names accurate

- `current_phase_lt_plan_phases_length_depth3` → `plan_phases_length_gt_constant_depth3` (with doc comment explaining the rename and what it actually tests: `plan.phases.length > 1`).
- `current_phase_lt_plan_phases_length_false` → `plan_phases_length_gt_constant_false` (same).

The doc comments specifically reference the rename so future readers can trace history. The new C1 tests use the previously-misleading names that match the AC verbatim — names now load-bear correctly. Verified.

### M2 (major → DEFERRED, accepted) — ListRecord validator walker

**What was done:**

- Block comment added at `src/validate/mod.rs:76-84`:
  > `TODO(phase-5): recurse into ListRecord sub-fields ... Phase 5's submit handlers write into ListRecord cells (e.g. cycles[].executor.summary) and will need this walk to enforce required/actor rules on those writes. The current behaviour (Phase 1): required fields inside a ListRecord element do NOT produce validation errors — this is intentional for now because Phase 1 has no submit path that targets individual list elements.`
- Pinning test `list_record_required_sub_field_not_validated_phase1` (note the literal "phase1" in the name): builds a schema with a `list_record entries` whose elements have a `required: true note` field; validates an entry whose `entries: [{}]` (element missing `note`); asserts validation PASSES (because the walker doesn't descend). Doc comment: "When Phase 5 adds the ListRecord walker, this test's expectation will INVERT from `unwrap()` to `unwrap_err()`. The change will be visible here, making the Phase-5 diff obvious."

**Reviewer judgment — accept the deferral.** Three reasons:

1. **The deferral is visible and self-defeating.** The pinning test will FAIL the moment Phase 5's executor adds the recursive walker (because `validate(...)` will start returning `Err(...)` instead of `Ok(())`). The Phase 5 executor cannot silently slip past this — they will see the failure, read the test's doc comment, and either flip `unwrap()` → `unwrap_err()` or delete the test. Either way the deferral is intentional, not lost.
2. **The TODO is at the right anatomical site.** Block comment sits inside the `for sub_field in fields` loop where the missing `ListRecord` arm would go. A grep for `TODO(phase-5)` will find it. The comment names the consumer (Phase 5 submit handlers writing `cycles[].executor.summary`).
3. **Phase-1 cohesion argument is correct.** Phase 1 has no path that writes into a ListRecord cell — the `add` op writes the whole JSON value as TEXT. There's nothing for the walker to validate at Phase-1 runtime; adding it now would be writing infrastructure with no caller, which the executor rightly resisted.

**Caveat (propagated to Learnings):** Phase 5's plan must explicitly enumerate the ListRecord walker as a task. Currently task 5.2 says "evaluate the guard against the merged entry" and 5.3 says "validator pass with the appropriate `Op::Submit*` variant against the (locked, diffed) entry" — neither names the walker extension. The Phase 5 plan-review must catch this.

### M1 (major → DEFERRED, accepted) — single AST type

**What was done:**

- Deviation note in main.md (line 1092) replaced with an accurate description: two ASTs coexist, the alias re-export does NOT unify them, Phase 5 must bridge via either (a) widen `required_when::Expr` to an alias of `expr::Expr` + update 8 call sites, or (b) `impl From<required_when::Expr> for expr::Expr`.
- No code change in this cycle.

**Reviewer judgment — accept the deferral, with the same caveat.** Reasons:

1. **The plan's "single AST type" intent (task 1.3 line 150) was aspirational shorthand.** The cycle-1 reviewer flagged that the alias didn't actually unify the types — that's still true, but the cost of unifying NOW is touching 8 existing `validate/required.rs` and `handlers/schema_show.rs` call sites that already work, just to satisfy a nominal "single type" claim. The user's stated preference (per Q-NEW-1 closure in cycle-2 plan-review) was to keep cycle 1 small.
2. **Phase 5's transition handler is where the bridge actually pays.** Task 5.2 ("evaluate the guard against the merged entry") and the eventual `required_when` evaluation against the same merged entry inside `submit-*` handlers are the call sites that need both ASTs unified. Doing the bridge as part of Phase 5's diff puts the unification next to its first user.
3. **The deviation note is now honest.** Cycle 1's "satisfied at module level" was misleading; cycle 2's "two ASTs coexist; Phase 5 must bridge" is accurate. Future reviewers won't be fooled.

**Caveat (propagated to Learnings):** Phase 5's plan task 5.2 must explicitly add a sub-step "bridge `required_when::Expr` and `expr::Expr` (option a or b)" before guard evaluation can land cleanly. The Phase 5 plan-review must catch this.

### m2 (minor → fixed, partially) — `cycles` update round-trip

**What was done:** `cycles_update_round_trips` test in `src/handlers/row.rs:447-494`:
1. INSERTs a `tasks` row with `cycles = [{phase:1, cycle:1, executor:{summary:"first draft", commit:"abc"}}]`.
2. `read_row` returns 1 element with summary `"first draft"`.
3. UPDATE replaces `cycles` with two elements: first one modified (`"revised draft"`, commit `"def"`) and a second one appended (`"final"`, commit `"ghi"`).
4. `read_row` returns 2 elements with the modified first and the new second.

**Coverage:** element-modify (cycle[0] changed) and element-add (cycle[1] new) are both exercised. Element-remove is NOT tested. The test name suggests "round-trip" rather than "all CRUD" so the omission is acceptable for AC1.7's "update replaces a list element correctly" wording — replacement of element values is verified; pure shrinking (`[A,B] → [A]`) is not. Reviewer note: shrinking is a single SQL UPDATE that overwrites the JSON-as-TEXT cell; the same code path that handles modify+add will trivially handle remove. Phase 5's submit handlers exercise append-only (`cycles.push(...)`), not remove, so this gap doesn't bite. Accepted.

### m3 (minor → not actioned, accepted) — commit prefix

The cycle-1 reviewer's m3 was advisory ("future commits should use the actual task number in the prefix"). Cycle 2 commits use `T002 P1.cycle2:` — a different inconsistency (no `.X` task number on either commit, but a `cycle2` qualifier instead). The executor's note "don't churn history if the diff is settled" is reasonable. Both `a758662` and `ec66f46` are clearly attributable to Phase 1 cycle 2; the searchability isn't materially harmed. Accepted.

---

## Test Verification

```
cargo test → test result: ok. 184 passed; 0 failed; 0 ignored
```

184 = 174 (cycle-1) + 10 (cycle-2 additions). The 10 new tests:
1. `parse_current_phase_lt_plan_phases_length` (parser, `expr.rs`)
2. `parse_current_phase_ge_plan_phases_length` (parser, `expr.rs`)
3. `parse_path_eq_path` (parser, `expr.rs`)
4. `current_phase_lt_plan_phases_length_true` (eval, `expr_eval.rs`)
5. `current_phase_lt_plan_phases_length_false_equal` (eval, `expr_eval.rs`)
6. `current_phase_lt_plan_phases_length_missing_phase_returns_false` (eval, `expr_eval.rs`)
7. `path_eq_path_true` (eval, `expr_eval.rs`)
8. `path_eq_path_false` (eval, `expr_eval.rs`)
9. `list_record_required_sub_field_not_validated_phase1` (M2 pinning, `validate/mod.rs`)
10. `cycles_update_round_trips` (m2 round-trip, `handlers/row.rs`)

All 10 verified by name in the `cargo test` output.

`env -u CLAUDECODE bash tests/e2e.sh` → all 13 DONE_WHEN steps pass. No e2e regression.

---

## AC Verification (re-confirmed for cycle 2)

| AC | Cycle 1 | Cycle 2 | Notes |
|---|---|---|---|
| 1.1 | YES | YES | unchanged |
| 1.2 | YES | YES | unchanged |
| 1.3 | YES | YES | parser tests for path-RHS forms add depth |
| 1.4 | **PARTIAL** | **YES** | C1 fix lands the AC1.4-exact form; tests at parse + eval level |
| 1.5 | YES | YES | unchanged |
| 1.6 | YES | YES | unchanged |
| 1.7 | YES | YES | m2 update round-trip strengthens coverage |
| 1.8 | YES | YES | unchanged |
| 1.9 | YES | YES | unchanged |
| 1.10 | YES | YES | unchanged |
| 1.11 | YES | YES | unchanged |
| 1.12 | YES (174) | YES (184) | +10, all pass |

All 12 ACs verified end-to-end.

---

## What's Good (cycle 2)

- **C1 fix is surgical.** ~30 LOC of parser extension + ~50 LOC of eval extension + ~80 LOC of tests. No unrelated refactoring; no API churn at existing call sites.
- **Eval semantics on path-vs-path are consistent with the schema philosophy.** Both sides resolved from the entry; numeric-numeric and string-string both work; mixed types and missing paths return `false` rather than panicking. This is the same "errors as boolean false, never crash" rule the rest of the evaluator follows.
- **Pinning test is the right pattern for deferred work.** `list_record_required_sub_field_not_validated_phase1` self-narrates. A test name that ends in `_phase1` and a body that asserts a known gap is the correct way to lock a contract that will INVERT in a future phase.
- **Deviation note quality improved.** Cycle 1's "satisfied at module level" was wishy-washy. Cycle 2's "two ASTs coexist; Phase 5 must bridge (option a or b)" is the precise statement Phase 5's plan-reviewer needs.
- **Test cost discipline.** 10 new tests, each tightly scoped (one function, one assertion path). No catch-all integration tests that would obscure regressions.
- **No churn on cycle-1's good work.** The `Rhs::Literal` + `Lhs::PathLength` parse-time rejection still fires; the existing `current_cycle <= 4` AC1.4-marquee test still passes; the `framework`-actor DDL test, ambiguity validation, depth-3 walker, scope/manifest plumbing — all untouched.

---

## Issues Found in Cycle 2 — none new

No new findings. The two deferrals (M1, M2) are documented appropriately and propagated to Phase 5 via the Learnings section below.

---

## Learnings (Cycle 2 → Phase 5)

**Phase 5 plan must address the M1 (AST unification) and M2 (ListRecord validator walker) deferrals — verify before executing Phase 5.**

Specifically, the Phase 5 planner should add (at minimum):

1. **A sub-task under 5.2** that bridges `required_when::Expr` and `expr::Expr`. Two options — pick one explicitly:
   - (a) Widen `required_when::Expr` to a type alias of `expr::Expr`. Requires updating ~8 call sites in `validate/required.rs` and `handlers/schema_show.rs` that currently access `.lhs_path` and `.rhs_literal` on the narrower struct.
   - (b) Add `impl From<required_when::Expr> for expr::Expr` (or a `to_guard_expr()` method) and call it at the bridge point in `submit-*` handlers. Existing call sites unchanged.
2. **A sub-task under 5.3** (or as a new 5.3a) that extends `validate/mod.rs::validate_field` to recurse into `FieldType::ListRecord(sub_fields)`. The current walker has a `TODO(phase-5)` block comment at the gap. The pinning test `list_record_required_sub_field_not_validated_phase1` will FAIL when this is implemented — the executor must flip its expectation from `unwrap()` to `unwrap_err()` (or delete the test, which is also acceptable since the contract has changed).

Both are visible in the Phase-1 deviation notes (main.md lines 1092-1093) and reinforced by code-level TODOs. The Phase 5 plan-review's job is to confirm both made it into the task list before execution.

**Cycle 2 process learning:**

- **The pinning-test-with-self-naming pattern works.** `list_record_required_sub_field_not_validated_phase1` literally tells a future reader "this test belongs to phase 1's contract; phase 5 will invert it." Cleaner than a comment-only TODO. Recommend adopting this pattern for any future deferred-to-next-phase work.
- **"Single AST" claims should not be checked at the alias level.** The cycle-1 deviation note ("re-export satisfies single-AST intent") was technically true (one of the names IS shared) but semantically false (the structs are different shapes). Future deviation notes should describe the type-level reality, not the surface-level symbolic indirection.
- **The C1 fix arrived in ~80 LOC of code + 80 LOC of tests, exactly the scope cycle 1 estimated** ("~30 LOC + 3 tests" was the lower-bound estimate; the real total is larger because both sides — parser AND eval — needed extending, and the executor wisely added eval-side support for `Rhs::Path` symmetrically with `Rhs::PathLength`). Cycle-1 estimates should explicitly call out "both parse and eval" when a grammar feature is added, to avoid undersell.
