# T006: Substrate cleanup — POC findings (transition guards, list_record, name escaping, list flags)

## Meta
- **Status:** PLANNING
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30
- **Blocked Reason:** —

## Task

The L275 proof-of-concept (`stores/observations_1006/schema.yaml`, ran against a tempdir at `/tmp/poc-1006`) demonstrated four enforcement-substrate gaps in `stores` v0.4. Three of the four are bugs that bypass the framework's own philosophy thesis (rules that exist in the schema but do not fire at runtime, or values that round-trip through the substrate without their declared structure). The fourth is a cosmetic CLI defect.

This task closes all four as a themed batch so the next 10.06 migration step can lean on a substrate that enforces the schema in full.

### The four findings (from POC commit `e502b98` body and the audit conversation)

**Finding A — Plain-transition guards silently ignored.** `src/handlers/transition.rs::run_in_tx` checks state legality (`current_status == transition.from`) but never calls `eval(guard, &merged)`. Only the workflow `submit-*` verbs in `submit.rs::find_transition` honour `guard:` clauses. Result: the L275 POC's `ratify` verb (with `guard: "intent_contract.contract_state == 'ready'"`) succeeded even though the contract was still `draft`. **This is the marquee philosophy violation** — a rule that exists in the schema but does not fire at runtime.

**Finding B — `list_record` values stored as opaque string, not parsed JSON.** In `coerce_value` (`src/handlers/row.rs:88`) there is no `ListRecord` arm; values fall through to the `_ => Value::String(...)` branch. In `add.rs` storage logic (`src/handlers/add.rs:84`) the match is `FieldType::Record(_) | FieldType::List(_) =>` — `ListRecord` does not match, so the value is stored as the raw input string. Concrete consequence: `evidence.external_refs` from L275 round-tripped as a literal JSON-string blob (`"[{\"system\":\"docker\",...}]"`), not as a structured array. The schema declares the inner shape but the substrate doesn't parse it.

**Finding C — Store names containing `-` break DDL.** The DDL codegen (`src/codegen/ddl.rs`) interpolates the store name as a bare SQL identifier. A name like `observations-1006` produces `CREATE TABLE observations-1006 (...)` which fails with `near "-": syntax error`. Workaround used in the POC: rename to underscore. Real fix: either quote the identifier (`"observations-1006"`) in DDL, or validate at install time and reject non-identifier-safe names with a clear error.

**Finding D — `list: text` flags can't repeat at the CLI.** Currently `--in-scope "X" --in-scope "Y"` errors with `"the argument '--in-scope' cannot be used multiple times"`. The convention is pipe-separated (`"X|Y|Z"`), per `coerce_value` at `row.rs:98-104`. This works but is awkward for any value containing a pipe and is not how operators expect repeatable flags to behave. Fix: use `clap::ArgAction::Append` for `List(...)` fields so both forms work (pipe-separated for legacy, repeatable for ergonomics).

### Validation: the L275 POC re-run

Once all four are fixed, re-running the L275 POC trace from `stores/observations_1006/` against a fresh tempdir should:

1. Install successfully even with a hyphenated store name (after Finding C — though we keep the `_` form since it's already shipped, just confirm hyphenated names don't break).
2. Accept `--in-scope "main.py" --in-scope "dev"` as repeatable flags (Finding D).
3. Round-trip `evidence.external_refs` as a structured JSON array on `show --json`, not a string blob (Finding B).
4. Reject `ratify L001` with `--invoker ai_autonomous` BEFORE the contract is `ready`, because the transition guard now fires (Finding A). Today, ratify succeeds despite the guard.

### What's NOT in this task

- Renaming `parse_envelope` or any T005-shipped code. T005 is closed; don't relitigate.
- Porting an actual 10.06 store (gate / observations / tasks). That's T007+, after this substrate is clean.
- Diagnosing why the haiku executor agent emitted a `guide` envelope in the t003-smoke (model-discipline issue, separate task).
- The "multiple task directories found for T001" cosmetic warning. Either becomes a follow-up cleanup or fits into one of these phases as a side-quest.

### DONE_WHEN

A repeat of the L275 POC trace from `/tmp/poc-1006` (using `stores/observations_1006/` as installed) demonstrates **all four enforcement moments firing**: (1) `ratify` rejects the transition when the contract is still `draft` (Finding A), (2) `evidence.external_refs` round-trips as a parsed JSON array via `show --json` (Finding B), (3) a hyphenated store name installs cleanly OR is rejected at install time with a clear error (Finding C), and (4) `--in-scope X --in-scope Y` accepts both forms equivalently to `--in-scope "X|Y"` (Finding D). All four CLI demonstrations captured as artefacts; `cargo test --all` and `tests/e2e.sh` + `tests/drive_e2e.sh` + `tests/tasks_e2e.sh` all green. No regressions in T005's drive smoke (`stores tasks drive --auto --claude-code --testing` still completes).

---

## Plan
_Planner agent fills this section._

### Objective
_What we're trying to achieve._

### Scope
- **In Scope:**
  - `src/handlers/transition.rs` — guard evaluation in `run_in_tx`
  - `src/handlers/row.rs::coerce_value` — `ListRecord` arm
  - `src/handlers/add.rs` — `ListRecord` storage match arm
  - `src/codegen/ddl.rs` — identifier escaping or install-time validator
  - `src/cli/dynamic.rs` — repeatable list flags via `ArgAction::Append`
  - Tests for each of the four findings
  - One integration test that re-runs the L275 POC end-to-end and asserts all four moments
- **Out of Scope:**
  - 10.06 store ports
  - Renaming, refactoring, or otherwise touching T005-shipped code
  - The "multiple task directories" cosmetic warning (separate)
  - Schema migrations for existing list_record values (decide approach in Decision Matrix)

### Phases
| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | _planner fills_ | _planner sets_ |

### Phase Details
#### Phase 1: [Title]
- **Objective:** ...
- **Files to modify:** ...
- **Acceptance Criteria:**
  - [ ] ...

### Decision Matrix
| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| ... | ... | ... | ... |

---

## Plan Review
_Plan-reviewer agent fills this section._

- **Gate:** READY | NEEDS_WORK | NOT_READY
- **Open Questions Finalized:** —
- **Issues Found:** —

---

## Execution Log
_Executor agent fills this section per phase._

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

---

## Completion
_Final summary when task is complete._
