# Phase 5 Code Review — Enforcement engine

- **Reviewer:** code-reviewer (Opus 4.7 1M)
- **Date:** 2026-04-26
- **Commit:** `ebd667b` (+ `cd59e53` log update)
- **Verdict:** PASS — advance to Phase 6
- **Issues:** 0 critical / 0 major / 4 minor

## Git reality

```
$ git show ebd667b --stat
14 files changed, 1195 insertions(+), 20 deletions(-)
src/cli/dispatch.rs                            |  15 +-
src/cli/dynamic.rs                             |   6 +
src/codegen/ddl.rs                             |   3 +
src/handlers/add.rs                            |   8 +-
src/handlers/update.rs                         |   8 +-
src/schema/required_when.rs                    |  45 ++-
src/validate/actor.rs                          | 218 +++++++++++++++
src/validate/enum_check.rs                     |  91 +++++++
src/validate/error.rs                          |  35 +++
src/validate/mod.rs                            | 364 ++++++++++++++++++++++++-
src/validate/regex_check.rs                    | 125 +++++++++
src/validate/required.rs                       | 220 +++++++++++++++
tasks/active/T001-stores-framework-v01/main.md |  44 ++-
tests/fixtures/all_types_store/schema.yaml     |  33 +++
```

Touches only validate/, the two write handlers, the dispatch + dynamic CLI seam, the fixture, and the DDL snapshot test (column list expansion). No unrelated files. Scope discipline: clean.

## ACs verified

| AC | Description | Method | Result |
|---|---|---|---|
| 1 | Unit tests cover required, required_when, enum, pattern, actor (passing + failing) | `cargo test` 79/79 + spot-read `validate/*.rs::tests` modules | PASS |
| 2 | Cross-Record `required_when` test (`contract.done_when` ← `triage.verdict == 'T3'`); fires on T3, silent on non-T3 | `validate::required::tests::cross_record_required_when_fires_for_t3` + `…_silent_for_non_t3`; also `validate::tests::cross_record_required_when_fires_for_t3` at the top-level integration; **plus** live E2E in tmp dir | PASS |
| 3 | E2E: `add ... --verdict T3` (no contract) errors out citing the three missing fields and the `required_when` rule | Live in tmp dir — output matches; 3 errors with rule named in message | PASS |
| 4 | Pattern-mismatch rejected | Live: `add --slug "Bad Slug!"` → `value 'Bad Slug!' does not match pattern '^[a-z0-9-]+$'` | PASS |
| 5 | Invoker mismatch error format documented in plan | Live with `probe_store` (kitchen_sink has no human-actor field): `CLAUDECODE=1 stores probe add --question ok? --answer yes` → exact spec error message | PASS |
| 6 | Errors aggregate (multiple violations in one pass) | Live: `add` with missing title + bad slug + bad priority + verdict=T3 → 6 errors aggregated, sorted | PASS |
| 7 | Phase 2 M1 fix (NORTH/BAND no longer trip the OR/AND substring detector) | Spot-read `required_when::contains_keyword` (proper word-boundary check); regression tests `parse_accepts_quoted_or_in_literal` + `parse_accepts_quoted_and_in_literal` pass | PASS |
| 8 | All 79 tests pass | `cargo test` | PASS |

## Live E2E session (representative)

```
$ stores init && stores install /…/all_types_store
Installed store 'kitchen_sink' (table: kitchen_sink)

$ stores kitchen_sink add --title hi --verdict T3
Error: validation failed:
- contract.done_when: required (because triage.verdict == 'T3')
- contract.scope_in: required (because triage.verdict == 'T3')
- contract.scope_out: required (because triage.verdict == 'T3')

$ stores kitchen_sink add --title T3demo --verdict T3 --done-when X --scope-in Y --scope-out Z
K001

$ stores kitchen_sink add --slug "Bad Slug!"
Error: validation failed:
- slug: value 'Bad Slug!' does not match pattern '^[a-z0-9-]+$'

$ stores kitchen_sink add --slug "BAD SLUG" --priority "critical" --verdict T3
Error: validation failed:
- contract.done_when: required (because triage.verdict == 'T3')
- contract.scope_in: required (because triage.verdict == 'T3')
- contract.scope_out: required (because triage.verdict == 'T3')
- priority: value 'critical' is not one of the allowed values: [low, medium, high]
- slug: value 'BAD SLUG' does not match pattern '^[a-z0-9-]+$'
- title: required

$ CLAUDECODE=1 stores probe add --question "ok?" --answer "yes"
Error: validation failed:
- answer: field 'answer' requires actor 'human'; invoker is 'ai_autonomous' (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)

$ CLAUDECODE=1 stores probe add --question "ok?" --answer "yes" --invoker human
P001
```

Output is alphabetically sorted by `field_path`, deterministic, and contains the precise rule context (which `required_when` triggered, which pattern was violated, what the invoker was and how it was detected).

## Spot-checks of load-bearing logic

### `validate::required::lookup` (cross-Record path resolution)

The dotted-path walker descends through nested `Value::Object`s for each path segment after the first (which keys into the EntryMap directly). On any missing segment or non-Object intermediate, it returns `None` — no panic surface. This is what makes `lhs_path = ["triage","verdict"]` resolvable from inside a `contract.done_when` evaluation: the validator hands `lookup` the **top-level** `entry`, not the local sub-tree, so the path can climb back out of `contract` into `triage`.

Verified that a `required_when` referencing a non-existent field is silent (`triggered = false` because `None.as_str() == None != Some(rhs)`). No crash; rule simply doesn't fire. This matches the cynical "what if the schema is misconfigured" probe.

### `actor::check_actor` + `check_transition_actor`

- `effective_actor` falls back from `field.actor` to `default_actor` (store-level). Verified by `default_actor_applied_when_field_has_none` test and via `validate_field` passing `&schema.default_actor` (resolves the `Schema.default_actor` carried-YAGNI minor from Phase 2/3 reviews).
- `actor_allowed` is correct: `Human` requires `Human`; `AiAutonomous` requires `AiAutonomous`; `AiWithHuman` accepts both. The `ai_with_human` semantic is "either is fine".
- Error message format matches the spec verbatim: `"field 'X' requires actor 'human'; invoker is 'ai_autonomous' (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)"`. Verified live.
- `check_transition_actor` uses the same `actor_allowed` predicate; produces an error with `field_path = vec!["<transition:VERB>"]` so it sorts cleanly into the pretty-printed list.

### `Op::Transition(verb)` plumbing

`mod.rs:43-49` looks up the transition by verb in `schema.lifecycle.transitions`, then if the transition has a declared actor, runs `check_transition_actor`. Forward-compat for Phase 6: the handler will pass `Op::Transition(verb_string)` along with the merged entry-diff and the resolved invoker; everything downstream (per-field rules, transition-actor check) fires as a single pass with all errors aggregated.

### Aggregation + pretty_print

`validate()` collects into `Vec<ValidationError>` and only `Err`s if non-empty. `pretty_print` clones the slice, sorts by `field_path`, joins as bullets. Sort is deterministic. Verified via the 6-error live aggregation test above.

## Deviations (executor's notes — assessed)

| ID | Deviation | Verdict |
|---|---|---|
| D1 | `RuleKind::Pattern` promoted from unit variant to `Pattern { pattern: String }` so the offending pattern lands in the error | **Improvement.** Caller-friendly; no other consumers. |
| D2 | `--invoker` global flag pulled into Phase 5 (plan said Phase 6+) | **Defensible.** AC5 demands a live demo of actor-mismatch under CLAUDECODE, which requires the override path to exist. The flag is `global(true)` in clap — Phase 6's transition subcommands will inherit it for free. Doesn't preempt anything. |
| D3 | Fixture rename `triage.notes` → `triage_notes` | **Defensible.** Forced by the leaf-arg uniqueness check `leaf_args(schema)` in `install.rs:25-27` (collision with `details.notes`). The framework correctly rejected the original fixture; the rename keeps the test alive. **Note:** the framework still doesn't catch reserved-column-name collisions at install time — see m3 below. |
| D4 | `pretty_print_sorted_by_field_path` test fixup (alphabetical: `contract.*` < `summary`) | **Trivial.** Sort logic itself is correct. |

## Issues

### Minors (4)

**m1 — `is_absent` treats empty string as present.** `check_required` at `required.rs:43` defines `is_absent = field_value.is_none() || field_value == Some(&Value::Null)`. An empty string `--done-when ""` is `Value::String("")`, which counts as present. Verified live: `add --title hi --verdict T3 --done-when "" --scope-in "" --scope-out ""` succeeds. The v0.1 demo path doesn't trigger this and the plan doesn't specify empty-string semantics; tighten in a future phase if user-facing surveys show it matters. Defer.

**m2 — `mod.rs::transition_actor_mismatch_caught` is misleadingly named.** The test (lines 332-338) actually verifies that the `triage` transition with `actor: ai_with_human` accepts both `Human` and `AiAutonomous` — it's a positive-path test, not a mismatch test. The actual negative case (`actor: human` rejecting `AiAutonomous`) lives in `actor.rs::transition_actor_mismatch_fires`. Rename for clarity (e.g. `transition_with_ai_with_human_accepts_both`). Cosmetic; not gate-blocking.

**m3 — Reserved-column-name leaf collision still not caught at install.** Carried from Phase 2 m5 / Phase 3 m2 / Phase 4 m5. Phase 4's `dynamic.rs::is_reserved` list (status, display_id, created_at, etc.) is not mirrored at install time. A user leaf named `status` would surface as SQLite's own `duplicate column name` error, not a clean stores-layer message. Phase 5 didn't pick it up. Bundled v0.1 stores don't trigger it. Recommend mirroring `is_reserved` into `install::run`'s pre-flight checks; ≤ 10 LOC.

**m4 — `Schema.default_actor` carried-minor closes (positive note).** Phase 2 m3 / Phase 3 m3 flagged `Schema.default_actor` as unused-YAGNI. Phase 5 wires it correctly: `mod.rs:53/58` passes `&schema.default_actor` to `validate_field`, which forwards it to `check_actor`, which uses `effective_actor` to fall back. The carried minor is **resolved** by this phase.

## Forward-compat notes

**Phase 6 (lifecycle transitions + observations store):**
- The validator API `validate(schema, &EntryMap, Op, Actor) -> Result<(), Vec<ValidationError>>` is stable. Phase 6's transition handler should follow the `update.rs` shape: read existing row → deep-merge diff (Records preserved, Lists/scalars replaced) → call `validate(schema, &merged, Op::Transition(verb), invoker)` → if Ok, write `status = transition.to` plus the diff fields → COMMIT.
- The transition handler must additionally check current `status == transition.from`. This is **state-machine legality**, distinct from the validator's per-field/per-actor rules. The validator does NOT do this check; Phase 6 owns it.
- The `--invoker` flag is already global; transition subcommands inherit it. No CLI rework needed.
- The `observations` schema's three `contract.*` sub-fields with `required_when: triage.verdict == 'T3'` will resolve identically to the kitchen_sink fixture's matching shape. The cross-Record path resolution is verified.

**Phase 7 (gate store + human-only actor demo):**
- The `gate.answer` field with `actor: human` is exactly what the live `probe_store` test exercised. No validator changes needed.
- The `transition: answer` with `actor: human` will be enforced by `check_transition_actor` automatically.

## Verdict

**PASS — advance to Phase 6.** Status: `EXECUTING_PHASE_6`.

The load-bearing correctness work — cross-Record `required_when` resolution against the in-memory typed EntryMap, actor enforcement with the documented error format, multi-error aggregation, and the Op::Transition wiring for Phase 6 — is genuine, tested, and verified live in a fresh tmp dir. Four executor deviations are all defensible. Four minors are all deferrable; one carried minor (Schema.default_actor YAGNI) is resolved by this phase. No critical or major issues.
