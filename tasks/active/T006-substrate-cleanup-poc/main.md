# T006: Substrate cleanup — POC findings (transition guards, list_record, name escaping, list flags)

## Meta
- **Status:** CODE_REVIEW
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30
- **Blocked Reason:** —
- **Plan-review cycle:** 2 of 3 (cycle 1 NEEDS_WORK → cycle 2 READY)

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

### Objective
Close the four POC findings (A: ignored guards on plain transitions, B: `list_record` stored as opaque string, C: hyphenated store name breaks DDL, D: list flags can't repeat) so the L275 POC trace from `stores/observations_1006/` enforces the schema in full when re-run against a fresh tempdir. Each finding lands as its own committable phase; Phase 5 is an operator-driven integration re-run that asserts all four enforcement moments at once.

### Scope
- **In Scope:**
  - `src/handlers/transition.rs` — guard evaluation in `run_in_tx`
  - `src/handlers/row.rs::coerce_value` — `ListRecord` arm
  - `src/handlers/add.rs` — `ListRecord` storage match arm; audit `update.rs` and `transition.rs::execute_transition_write` for the same gap
  - `src/codegen/ddl.rs` — identifier escaping or install-time validator (decision below)
  - `src/cli/dynamic.rs` — repeatable list flags via `ArgAction::Append`; audit consumers for `get_one` → `get_many` API change
  - Tests for each of the four findings (unit-level in-crate + e2e where appropriate)
  - One integration phase (operator-run) that replays the L275 POC against `stores/observations_1006/` and captures all four moments as artefacts
- **Out of Scope:**
  - 10.06 store ports (gate / observations / tasks) — that's T007+
  - Renaming, refactoring, or otherwise touching T005-shipped code (`parse_envelope`, drive, etc.)
  - The "multiple task directories found for T001" cosmetic warning
  - Schema migrations for previously-stored list_record string blobs — new writes only; existing rows in untouched stores are not rewritten (recorded in Decision Matrix)

### Phases
| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Finding A — wire guard evaluation into plain transitions (`run_in_tx`) | Small |
| 2 | Finding B — `ListRecord` arms in `coerce_value` + write paths (`add`, `update`, `transition`) | Small-Medium |
| 3 | Finding C — DDL identifier escaping or install-time validator (decision required) | Small |
| 4 | Finding D — repeatable list flags via `ArgAction::Append` + consumer audit | Medium |
| 5 | Integration — operator-driven re-run of the L275 POC; capture all four moments as artefacts | Medium |

### Phase Details

#### Phase 1: Finding A — plain-transition guards (full selection algorithm)
- **Objective:** When a `lifecycle.transitions[]` entry has a `guard:`, that guard must be evaluated in `transition::run_in_tx` (it currently is not). Marquee POC failure: `ratify` succeeds while `intent_contract.contract_state == 'draft'`, despite the schema declaring `guard: "intent_contract.contract_state == 'ready'"`. **Substrate fix scope (revised cycle-2):** `transition::run_in_tx` currently uses `.iter().find(|t| t.verb == verb)` (first-match-by-verb). Adding a guard check around that call would silently inherit a selection bug — a schema with two same-verb transitions partitioned only by guard would always pick the first and ignore the partition. The fix is to extract the **full** selection algorithm currently inlined in `submit.rs::find_transition` (lines 236–294: filter by `(from, verb, requires_gate)`, prefer guarded-true, fall back to unguarded, error on ambiguity) into a shared helper, and have both handlers call it.
- **Files to modify:**
  - **New shared helper** `select_transition` (location: `src/schema/lifecycle.rs` — sits next to `Lifecycle::validate_transition_ambiguity`, the install-time partner check). Signature:
    ```rust
    pub fn select_transition<'a>(
        transitions: &'a [Transition],
        from_state: &str,
        verb: &str,
        gate: Option<&str>,
        entry: &EntryMap,
    ) -> anyhow::Result<&'a Transition>
    ```
    Body is verbatim the algorithm at `submit.rs:236–294`: filter by `(from, verb, requires_gate)`; if empty, error with the existing "no transition from '{from}' via verb '{verb}' (gate {gate})" wording; among candidates, prefer guarded-true (error on ambiguity); fall back to unguarded; error if no fallback exists. (`eval` from `validate::expr_eval` is the inner predicate.)
  - `src/handlers/submit.rs::find_transition` (lines 236–294) — collapse to a thin wrapper that delegates to `select_transition`. Keeps the `pub(crate) fn find_transition(schema, ...)` signature unchanged so the four call sites at lines 484/612/775/943/964 stay byte-identical.
  - `src/handlers/transition.rs::run_in_tx` (lines 36–62) — replace the bare `.iter().find(|t| t.verb == verb)` with `select_transition(&schema.lifecycle.transitions, &existing.status, verb, None, &merged_entry)`. Plain transitions never carry a `requires_gate`, so `gate=None` is correct; the helper's "guarded-true preferred / unguarded fallback / ambiguous error" logic still runs and yields the right verb behaviour for the no-guard, single-candidate case (existing tests pass).
  - **Critical ordering note:** the helper needs the **merged** entry (post-diff), so call site must build `merged_entry` *before* `select_transition` (currently `build_entry_map` is at line 65, after the find — re-order so the merge happens first, then the find against the merged entry; this matches `submit::find_transition`'s contract).
- **Acceptance Criteria:**
  - [ ] New unit test in `transition.rs` (extending `OBS_SCHEMA` or a small fixture): a transition with `guard: "field == 'X'"` fails when guard is false (named-error message: at minimum "guard not satisfied" — covers Finding A regression-trap) and succeeds when the merged-entry guard is true (sub-bullet: same-call diff that *brings* the row to guard-true also succeeds, matching submit semantics). Existing `Lifecycle::validate_transition_ambiguity` install-time check is unaffected — verify it still rejects ambiguous unguarded same-verb pairs (sub-bullet of this AC).
  - [ ] **New regression-trap test** (the test that the original cycle-1 plan would NOT have caught): a schema with two same-verb transitions partitioned by guard (e.g. `from: confirmed, verb: ratify, guard: "tier == 'T2'"` vs `from: confirmed, verb: ratify, guard: "tier == 'T3'"`) — `transition::run_in_tx` picks the correct one based on entry tier and rejects when neither guard fires. (This proves the *full* selection algorithm is wired, not just guard-eval-after-first-find.)
  - [ ] `submit.rs::find_transition`'s existing tests (workflow path: submit-plan, submit-plan-review, submit-execute) still pass unmodified — proves the refactor preserves submit's selection semantics.
  - [ ] Existing `transition.rs` tests still pass (no regression in state-machine legality, actor scoping, on-entry follow-ons); `cargo test --all` and `tests/e2e.sh` green.

#### Phase 2: Finding B — `list_record` write path
- **Objective:** A `list_record` value passed in as JSON text must be parsed into a `Value::Array(Vec<Value::Object>)` on the way in, and the write path must serialize that array (not the raw input string). Today `coerce_value` falls through to `Value::String(...)` and `add.rs:84` doesn't match `ListRecord` so the raw string is stored.
- **Files to modify:**
  - `src/handlers/row.rs::coerce_value` (line 88) — add `FieldType::ListRecord(_) => serde_json::from_str(raw).unwrap_or(Value::Null)`. Also add the same arm for `FieldType::ListFk { .. }` for symmetry (currently it falls through to `String` too — same bug shape, captured in Decision Matrix).
  - `src/handlers/add.rs` (line 84) — extend the match to `FieldType::Record(_) | FieldType::List(_) | FieldType::ListRecord(_) | FieldType::ListFk { .. } =>` so all four JSON-typed columns serialize through the same arm.
  - `src/handlers/update.rs` (lines 89–101) — same audit; today the match handles `Record` and `List` but not `ListRecord`/`ListFk`. Extend to all four.
  - `src/handlers/transition.rs::execute_transition_write` (lines 152–163) — same audit; extend `FieldType::List(_)` arm to cover `ListRecord`/`ListFk`.
  - **Error semantics:** if the raw string fails to parse as JSON, `coerce_value` returns `Value::Null` (fail-silent); the validator then catches the missing required value as a normal validation error. Recorded in Decision Matrix; rationale: matches existing `coerce_value` behaviour for malformed integers (falls back to `String` rather than erroring at coerce time, lets validator be the single source of "this entry is invalid" errors).
- **Acceptance Criteria:**
  - [ ] **list_record CLI round-trip:** new unit test in `row.rs` (or `add.rs`) — `add` with `--external-refs '[{"system":"docker","kind":"container","id":"foo"}]'` round-trips via `read_row` to a `Value::Array` of one `Value::Object`, not a `Value::String`. Sub-bullets: (a) `update` and `transition` write paths preserve the same shape; (b) e2e — `add` then `show --json` for a list_record field emits a JSON array, not an escaped JSON-in-string blob.
  - [ ] **list_fk CLI round-trip (cycle-2 revision 5):** `add ... --linked-observations '["L001","L002"]'` round-trips via `read_row` to `Value::Array` of two `Value::String`, mirroring the existing programmatic-write contract. Sub-bullet: re-run `tests/tasks_e2e.sh` (uses `linked_observations` and `depends_on` — the lifecycle-smoke canary) and confirm no regression. (Audit: `grep -rn 'linked_observations' src/handlers/` returned no results — only the schema YAML and read paths reference it, so the new CLI write surface is purely additive.)
  - [ ] **bad-JSON UX test (cycle-2 revision 4) — `list_record_bad_json_returns_validator_error`:** `add ... --external-refs '{not json'` produces an error whose message helps the operator diagnose. At minimum the error must mention the field name (`external_refs`) and indicate the value was rejected (e.g. "missing required field 'external_refs'" after fail-silent `Value::Null`, OR — recommended — enrich the validator to surface "invalid JSON for external_refs"). Bad-JSON fail-silent UX is a deliberate trade (see Decision Matrix); this AC pins the operator-debuggability floor.
  - [ ] `cargo test --all` and `tests/e2e.sh` + `tests/drive_e2e.sh` + `tests/tasks_e2e.sh` green.

#### Phase 3: Finding C — DDL identifier escaping for store names
- **Objective:** A schema with `name: observations-1006` must either (a) install cleanly with the hyphen, or (b) fail at install time with a clear message. Today it fails at DDL execution time with `near "-": syntax error`.
- **Files to modify (path A — quote in DDL, **recommended**):**
  - **New helper `quote_ident(name: &str) -> String`** in `src/codegen/ddl.rs` (or a new `src/codegen/sql.rs`) — returns `format!("\"{}\"", name.replace('"', "\"\""))`. All sites below route the schema name through this helper.
  - **Full enumerated DDL/SQL audit (cycle-2: ran `grep -rn 'INTO {' src/`, `grep -rn 'FROM {' src/`, `grep -rn 'UPDATE {' src/` — the canonical sweep — and the formatted-string `&schema.name` pattern). All sites that interpolate the schema name as a SQL identifier and need `quote_ident`:**
    1. `src/codegen/ddl.rs:95` — `CREATE TABLE IF NOT EXISTS {table}`.
    2. `src/handlers/add.rs:130` — `INSERT INTO {schema.name} (...)`.
    3. `src/handlers/add.rs:141` — `UPDATE {schema.name} SET display_id = ?1 WHERE id = ?2`.
    4. `src/handlers/row.rs:189` — `SELECT {col_list} FROM {table} WHERE display_id = ?1`.
    5. `src/handlers/list.rs:112` — `SELECT {col_list} FROM {}{}{}{}` (table interpolated bare).
    6. `src/handlers/transition.rs:201` — `UPDATE {} SET {set_clause} WHERE id = ?{where_param_idx}`.
    7. `src/handlers/update.rs:134` — `UPDATE {} SET {set_clause} WHERE id = ?{where_param_idx}`.
    8. `src/handlers/submit.rs:81` — `acquire_lock` `UPDATE {table} SET claimed_by = ?1, claimed_at = ?2 ...`.
    9. `src/handlers/submit.rs:94` — `acquire_lock` `SELECT claimed_by, claimed_at FROM {table} WHERE display_id = ?1`.
    10. `src/handlers/submit.rs:112` — `release_lock` `UPDATE {table} SET claimed_by = NULL, claimed_at = NULL WHERE display_id = ?1`.
    11. `src/handlers/submit.rs:217` — `write_status_and_fields` `UPDATE {table} SET {set_clause} WHERE id = ?{where_idx}`.
    12. `src/handlers/drive.rs:246` — `SELECT display_id FROM {table} ...` (auto-pick task query).
    13. `src/handlers/drive.rs:879` — `INSERT INTO {name} (display_id, status, ...)` (test-scaffold task insert).
    14. `src/handlers/drive.rs:1174` — `UPDATE {} SET blocked_reason = ?1 WHERE display_id = ?2`.
    15. `src/handlers/next_action.rs:290` — `INSERT INTO {} ({}) VALUES ({})` (test-scaffold insert).
    16. `src/handlers/next_action.rs:381` — `UPDATE {} SET blocked_reason = ?1 WHERE display_id = ?2`.
    17. `src/handlers/next_action.rs:399` — `UPDATE {} SET blocked_reason = ?1 WHERE display_id = ?2`.
  - **T005 fence note:** sites 12–17 are inside T005-shipped files (`drive.rs`, `next_action.rs`) but the change is **mechanical SQL-string quoting only**, not a behaviour change to drive/next_action logic. Plan-review explicitly authorized this minimal touch (cycle-1 review §7a). We do NOT touch progress flushing, `parse_envelope`, or `is_blocked` helpers.
  - **`{table}` parameter pattern:** several sites (e.g. `submit.rs::acquire_lock/release_lock`, `submit.rs::write_status_and_fields`) take `&schema.name` as a parameter and bind a local `let table = ...;` inside `format!(...)`. Refactor those to `let table = quote_ident(&schema.name);` at the top of the function (one-line change per function); the `format!` strings stay untouched.
  - **Show/list audit confirmed:** `grep -n 'FROM \|INTO \|UPDATE ' src/handlers/show.rs` — no schema-name interpolation in show.rs (it delegates to `read_row`); already covered by site 4.
- **Files to modify (path B — install-time validator, fallback only):**
  - `src/install.rs::run` — after schema parse, validate `schema.name` against `^[A-Za-z_][A-Za-z0-9_]*$` and bail with a clear error naming the invalid characters and suggesting the underscore form. **Decision still recommends path A** (forward-compatible; lets operators name stores naturally).
- **Acceptance Criteria:**
  - [ ] (path A) New unit test in `ddl.rs`: `ddl_for(schema_with_hyphenated_name)` produces `CREATE TABLE IF NOT EXISTS "observations-1006" (...)` and SQLite accepts it (`conn.execute_batch(ddl)` succeeds). Sub-bullet: existing DDL snapshot tests still pass; if path A breaks `ddl_snapshot` (underscore form now produces `CREATE TABLE IF NOT EXISTS "observations" (...)` with quotes), update the snapshot once and explain in the commit message — quoting an already-valid bare identifier is semantically identical to SQLite.
  - [ ] **End-to-end CRUD trap-test against hyphenated-name store (cycle-2 revision 2):** install a fixture schema with `name: obs-test-1006`, then exercise `add` / `show` / `list` / `update` / `transition` / submit verbs against it. Assert all succeed with no SQL syntax errors. This is the trap-test that proves **every** site in the 17-item enumeration above was actually quoted — if any site is missed, this test fails on whichever verb hits it.
  - [ ] (path B fallback only) New unit test in `install.rs`: install with `name: foo-bar` fails with an error mentioning `'-'` is invalid and suggesting `foo_bar`. (Used only if reviewer flips to path B; path A is the recommendation.)
  - [ ] `cargo test --all` green; `tests/tasks_e2e.sh` and `tests/drive_e2e.sh` green (proves drive/next_action sites 12–17 still work after quoting).

#### Phase 4: Finding D — repeatable list flags
- **Objective:** `--in-scope a --in-scope b` must parse equivalently to `--in-scope "a|b"`. Today the first form errors with "the argument '--in-scope' cannot be used multiple times".
- **Files to modify:**
  - `src/cli/dynamic.rs` — in `build_leaf_cmd_owned` (lines 626–660), `build_leaf_cmd` (lines 696–732), and any other site that builds a clap arg from a `LeafArg`, detect `FieldType::List(_)` and add `.action(ArgAction::Append)` to the arg. Note: `LeafArg.field.ty` for a list-typed leaf is the inner element type (per `schema/flatten.rs`), not `List(_)` — verify by reading `flatten.rs` and adjust the detection accordingly. If the leaf carries the original-field info, gate on that.
  - **Consumer audit (critical):** every site that calls `matches.get_one::<String>("<list-cli-name>")` must switch to `matches.get_many::<String>("<list-cli-name>")` for list-typed fields. Identified consumers:
    - `src/handlers/add.rs:33` (the `get_arg` closure passed to `build_entry_map`)
    - `src/handlers/transition.rs:80` (same closure shape)
    - `src/handlers/update.rs` (same)
    - `src/handlers/row.rs::build_entry_map` — this is the central choke point. The cleanest fix: change `get_arg`'s return type from `Option<String>` to `Option<Vec<String>>` (or keep `Option<String>` and join multiple values with `|` so the existing `coerce_value` pipe-split kicks in — **recommended** because it preserves backwards compat and minimizes the blast radius; recorded in Decision Matrix).
  - With the "join with `|`" approach: in each handler's `get_arg` closure, call `matches.try_get_many::<String>(cli_name).ok().flatten()`; if it returns multiple values, join with `|` and pass that as the single string to `coerce_value`. Pipe-separated input continues to work because a single `--in-scope "a|b"` is still passed through unchanged, and `coerce_value`'s split-on-`|` produces the same array.
  - **Edge case to handle:** a single value containing a literal pipe, passed via repeatable form (`--in-scope "a|b" --in-scope c`), would round-trip as `["a", "b", "c"]` (the pipe is split on the way out). This is a known limitation and matches existing behaviour for the pipe-separated form. Recorded in Decision Matrix.
- **Acceptance Criteria:**
  - [ ] **Repeatable-form regression-trap:** `add ... --in-scope "a" --in-scope "b"` produces stored `Value::Array(["a", "b"])` (pipe-free values; the safe case the substrate fix targets). Sub-bullet: `update` and `transition` verbs honour the repeatable form for list fields, not just `add`.
  - [ ] **Backwards-compat pipe form:** `add ... --in-scope "a|b"` (single arg containing a pipe) continues to produce `Value::Array(["a", "b"])`. Existing `tests/e2e.sh` callsites that pass pipe-separated still pass.
  - [ ] **Mixed form:** `add ... --in-scope "a|b" --in-scope "c"` produces `Value::Array(["a", "b", "c"])` — the join-with-`|` approach in `get_arg` collapses both forms to a single coerce_value pipe-split, and this AC pins that behaviour. (Note: this also documents the known limitation — a literal `|` in a single value is unrepresentable; see Decision Matrix.)
  - [ ] `cargo test --all` and the three e2e scripts green.

#### Phase 5: Integration — re-run the L275 POC
- **Objective:** With Phases 1–4 landed, replay the L275 POC trace against `stores/observations_1006/` from a fresh tempdir and capture all four enforcement moments as artefacts. Operator-driven (matches T005 Phase 4 pattern) — executor proposes the script and runs it against an installed `stores` binary; artefacts are diffed/checked into the task folder.
- **Files to modify (artefacts only, no production code changes in this phase unless a regression surfaces):**
  - `tasks/planning/T006-substrate-cleanup-poc/artefacts/poc-rerun.sh` — the replay script (operator runs it).
  - `tasks/planning/T006-substrate-cleanup-poc/artefacts/poc-rerun.log` — captured stdout+stderr.
  - `tasks/planning/T006-substrate-cleanup-poc/artefacts/show-l001.json` — `show --json L001` after `start_t2` (asserts `evidence.external_refs` is a JSON array, not a string).
  - `tasks/planning/T006-substrate-cleanup-poc/artefacts/ratify-rejected.txt` — captured `stores observations_1006 ratify L001 --invoker ai_autonomous` failure when contract_state is still `draft`.
  - `tasks/planning/T006-substrate-cleanup-poc/artefacts/hyphen-install.txt` — captured `stores install` outcome for a fixture with `name: observations-1006-test` (success under path A or clear-error under path B).
  - `tasks/planning/T006-substrate-cleanup-poc/artefacts/repeatable-flag.txt` — captured equivalence of `--in-scope a --in-scope b` vs `--in-scope "a|b"`.
- **Replay steps (the script encodes these):**
  1. Fresh tempdir; `cargo build --release` (or use the installed binary if pinned).
  2. `stores init` (in tempdir).
  3. `stores install /home/blake/repos/experiments/stores/stores/observations_1006`.
  4. `stores observations_1006 add --summary "..." --source dev --priority normal --captured-at 2026-04-30T00:00:00Z --contract-state draft --type work --drafted-by human --drafted-at 2026-04-30T00:00:00Z` → expect `L001`.
  5. **Finding A check:** `stores observations_1006 ratify L001 --invoker ai_autonomous` → MUST FAIL with a guard error (capture to `ratify-rejected.txt`).
  6. `stores observations_1006 update L001 --objective ... --in-scope main.py --in-scope dev --out-of-scope ... --acceptance ... --tier-hint T2 --approved-by human --approved-at 2026-04-30T00:00:00Z --contract-state ready --invoker human` → must succeed (this also exercises Finding D's repeatable flags).
  7. `stores observations_1006 ratify L001 --invoker ai_autonomous` → succeeds (guard now satisfied).
  8. `stores observations_1006 update L001 --evidence-external-refs '[{"system":"docker","kind":"container","id":"foo"}]'` (or write at add time; check the schema's available CLI surface).
  9. `stores observations_1006 start_t2 L001 --invoker ai_autonomous` → succeeds.
  10. `stores observations_1006 resolve L001 --invoker ai_autonomous` → succeeds.
  11. **Finding B check:** `stores observations_1006 show --json L001 > show-l001.json`; verify `.evidence.external_refs` is a JSON array (use `jq -e '.evidence.external_refs | type == "array"'`).
  12. **Finding C check:** install a fixture store named `observations-1006-test` (hyphenated); capture outcome to `hyphen-install.txt` (success under path A; clear-error under path B).
  13. **Finding D check (pipe-free values only):** `stores observations_1006 add --summary "x" --source dev --priority normal --captured-at ... --in-scope "main.py" --in-scope "scripts/"` and a parallel call with `--in-scope "main.py|scripts/"`; `show --json` both, diff, expect identical `intent_contract.in_scope` arrays. Per Decision Matrix: do NOT use values containing `|`; the artefact must pin the happy-path equivalence, not the documented pipe-in-value edge case.
  14. **No-regression check:** `cargo test --all`; `tests/e2e.sh`; `tests/drive_e2e.sh`; `tests/tasks_e2e.sh`; and the T005 drive smoke `stores tasks drive --auto --claude-code --testing` against a fresh tempdir.
- **Acceptance Criteria:**
  - [ ] All four artefacts captured and committed to `tasks/planning/T006-substrate-cleanup-poc/artefacts/`.
  - [ ] All four enforcement moments observable in the artefacts: ratify-rejected (A), evidence array shape (B), hyphen install outcome (C), repeatable-flag equivalence (D).
  - [ ] `cargo test --all` + `tests/e2e.sh` + `tests/drive_e2e.sh` + `tests/tasks_e2e.sh` all green from a clean checkout.
  - [ ] T005 drive smoke (`stores tasks drive --auto --claude-code --testing`) still completes — no regression.

### Decision Matrix
| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| Phase 1 selection algorithm: scope + helper location (cycle-2 revision 1) | Scope: (a) eval guard after existing `.find(|t| verb)` first-match; (b) extract submit's full algorithm (filter by from+verb+gate, prefer guarded-true, fall back unguarded, error on ambiguity) into a shared `select_transition` helper used by both `transition::run_in_tx` and `submit::find_transition`. Helper location: (i) `src/validate/expr_eval.rs`; (ii) `src/schema/lifecycle.rs` (next to `validate_transition_ambiguity`); (iii) new `src/validate/guard.rs` | **scope (b) + location (ii) `src/schema/lifecycle.rs`** | (a) silently inherits a selection bug — schema with two same-verb transitions partitioned only by guard would always pick the first, ignoring the partition. The L275 POC happens to use distinct verbs (`start_t2` vs `start_t3`) so Phase 5 would pass by accident, but the substrate fix is incomplete. Full extraction makes the helper the single source of truth; `submit::find_transition` collapses to a thin delegator. Location-wise: the helper sits beside `validate_transition_ambiguity` (its install-time partner — install-time rejects unguarded ambiguity, runtime helper resolves guarded ambiguity at execution). `eval` is just one inner predicate; the helper itself is a schema-lifecycle concern, not an expression-eval concern. |
| Finding C: identifier escape vs install-time reject | (a) quote in DDL + audit all SQL sites; (b) install-time validator rejects non-`[A-Za-z_][A-Za-z0-9_]*$` names | **(a) quote in DDL** | Forward-compatible; lets operators name stores naturally (`observations-1006`); no rename pressure. Cost: one `quote_ident` helper + audit of **17 SQL-build sites** (cycle-2 revised: cycle-1 estimated ~6, full canonical sweep found 17 including drive.rs/next_action.rs test scaffolds and submit.rs lock helpers; see Phase 3 enumeration). SQLite quoted identifiers are standard; no downstream tooling in this codebase parses bare table names. |
| ListRecord coerce-error semantics | (a) fail-loud (`coerce_value` returns `Result`); (b) fail-silent (`Value::Null` on bad JSON, validator catches it later) | **(b) fail-silent** | Matches existing `coerce_value` behaviour for malformed integers (falls back to `String`); keeps `coerce_value` infallible; concentrates "this entry is invalid" errors in one place (the validator). |
| Bad-JSON UX in list_record coerce (cycle-2 revision 4) | (a) silent `Value::Null` + generic "field is required" validator error; (b) silent `Value::Null` + validator error enriched to mention parse failure for the field; (c) fail-loud at coerce time | **(b) silent + enriched validator error** | Operator UX: passing malformed JSON in a list_record arg shouldn't surface as the same error as omitting the arg entirely. Validator already knows the field name; tagging "invalid JSON for `external_refs`" (or at minimum surfacing the field name) costs nothing and makes the bad-input case debuggable. Matches Phase 2 AC `list_record_bad_json_returns_validator_error`. |
| Finding D: drop pipe-separated form? | (a) keep both pipe-separated and repeatable; (b) deprecate pipe-separated, repeatable only | **(a) keep both** | Backwards compatibility — existing tests, fixtures, and any external scripts using `--in-scope "a\|b"` keep working. The "join with `\|` in the get_arg closure" implementation makes both forms collapse to the same code path with no extra branching in `coerce_value`. |
| Pipe-containing values in list args (cycle-2 revision 3) | (a) introduce escape mechanism (`\\|`) or alternative separator; (b) accept as documented limitation — operators cannot pass a literal `|` in a list value via CLI; if needed, use `--field-from-file` (programmatic write path) to bypass | **(b) documented limitation** | A small documented limitation is cheaper than a quoting/escape mechanism nobody asked for. The substrate fix is repeatable flags; the pipe-in-value case is unchanged from today's substrate (still unrepresentable on the CLI). Phase 4 ACs explicitly use pipe-free values for the equivalence trap-test; Phase 5 step 13 uses `main.py`/`scripts/` (no pipes) so the artefact pins the safe case, not the edge. |
| Migrating existing `list_record` string-blob rows | (a) one-time migration on store re-install; (b) leave existing rows as-is, new writes only | **(b) new writes only** | T006 is a substrate fix, not a data fix. Any L275 POC re-run starts from a fresh tempdir. The 10.06 ports (T007+) will install fresh stores anyway. Recorded so reviewer doesn't expect a migration. |
| Audit `ListFk` write path alongside `ListRecord` | (a) fix `ListFk` in same phase as `ListRecord` (same bug shape); (b) leave `ListFk` untouched (out of POC scope) | **(a) fix together** | The bug is symmetric — `coerce_value` falls through, `add.rs:84` doesn't match. Fixing one without the other leaves a known-bad code path. **Side-effect (cycle-2 revision 5):** this enables NEW CLI write surface for `list_fk` fields — today they're programmatic-only (per `row.rs:17-18`). After Phase 2 lands, operators can pass `--linked-observations '["L001","L002"]'` on add/update for `tasks` schema. Audit confirmed no handler reads `linked_observations` as a string blob (only schema YAML and read paths reference it), so the new surface is purely additive. The `tests/tasks_e2e.sh` lifecycle smoke uses these fields and is the canary AC. |
| Phase 5 fixture for hyphenated store install | (a) reuse `observations_1006` and just rename to `observations-1006`; (b) create a small new fixture `tests/fixtures/hyphen_name_store/schema.yaml` | **(b) small new fixture** | Don't muck with the canonical POC store name; a tiny single-field schema is enough to demonstrate the DDL path. |

---

## Plan Review

**Reviewer:** plan-reviewer
**Date:** 2026-04-30
**Gate:** NEEDS_WORK
**Detailed findings:** see sibling `plan-review.md`.

### Open Questions Finalized
None escalated to human. All five revisions below are within planner scope.

### Issues Found

**Required revisions (NEEDS_WORK):**

1. **Phase 1 — selection-semantics gap.** `transition.rs::run_in_tx` uses `.iter().find(|t| t.verb == verb)` (first-match-by-verb). Adding a guard check after this find does NOT match `submit::find_transition`'s "filter by from+verb+gate; prefer guarded-true; fall back to unguarded; error on ambiguity" semantics. A schema with two same-verb transitions partitioned only by guard would deadlock. The L275 POC happens to use distinct verbs (`start_t2` vs `start_t3`), so Phase 5's artefact passes — but the substrate fix is incomplete. **Fix**: extract the full selection algorithm into a shared helper used by both `run_in_tx` and `submit::find_transition`, OR add an install-time validator that rejects duplicate `(from, verb)` pairs without distinct guards. Add a corresponding AC in Phase 1.

2. **Phase 3 — DDL audit is undercounted.** Plan says "~6 sites" but the canonical sweep (`grep -rn 'INTO {\|FROM {\|UPDATE {' src/`) finds more, and the planner missed several critical ones in T005-shipped code (which is fine to touch for quoting): `drive.rs:879/1174`, `next_action.rs:381/399`, `submit.rs:81/94/112` (acquire_lock/release_lock), `drive.rs:246`. Some sites (acquire_lock/release_lock) take `&schema.name` as a parameter — those need adjustment at call sites. **Fix**: paste the full enumerated list into Phase 3, commit to a `quote_ident` helper used uniformly, and explicitly authorize the minimal touches in drive.rs/next_action.rs as "uniform DDL quoting, not behaviour change."

3. **Phase 4 / Phase 5 — pipe-edge case under-protected.** The Decision Matrix names the "single value containing `|`" edge case but Phase 5's `repeatable-flag.txt` artefact must use pipe-FREE values to assert the equivalence holds in the safe case. **Fix**: add a Phase 4 AC explicitly testing pipe-free equivalence, and a separate AC documenting (no test) the pipe-containing case as a known limitation. Phase 5 step 13 should pin to e.g. `--in-scope main.py --in-scope dev`.

4. **Phase 2 — UX regression test missing for bad JSON.** Choice (b) "fail-silent on bad JSON, validator catches it" is acceptable but the operator UX is "field is required" instead of "JSON parse error at column N." **Fix**: add an AC asserting that `add ... --external-refs 'not-json'` produces a validation error that names the field; optionally enrich the validator error with the parse-error reason.

5. **Phase 2 — ListFk bundling side-effect not noted.** Bundling ListFk fix is sound (same bug shape; today list_fk fields are programmatic-only — see `row.rs:17-18`), but the Decision Matrix should call out that this enables NEW CLI surface for list_fk fields. **Fix**: add a Phase 2 AC that read-after-write via CLI for a `list_fk` field round-trips as `Value::Array` of strings.

### Strengths confirmed (no changes needed)
- DONE_WHEN clauses map 1:1 to phase ACs and Phase 5 artefacts.
- Phase independence and unit-test coverage per phase (no test debt hidden behind Phase 5).
- Phase 4 blast-radius minimization via "join-with-pipe in get_arg" is the right call (~3 callsites touched, public API preserved).
- Out-of-scope T005 fences correctly stated for `parse_envelope`/drive/status/next_action **logic**; quoting fixes in those files for Phase 3 are mechanical and acceptable.
- Decision Matrix names every high-stakes choice with rationale.

### Routing
→ **PLANNING** (back to planner for revisions 1–5). Re-route to plan-reviewer after revision.

### Cycle 2 review

**Reviewer:** plan-reviewer
**Date:** 2026-04-30
**Gate:** READY

All five cycle-1 revisions verified. Status advances PLAN_REVIEW → READY.

**Per-revision verification:**

1. **Phase 1 selection algorithm (cycle-1 §1) — CLOSED.** `select_transition` is named explicitly (main.md:84), signature is plausible (`pub fn select_transition<'a>(transitions, from_state, verb, gate, entry) -> Result<&'a Transition>`), and lives at `src/schema/lifecycle.rs` next to `validate_transition_ambiguity` (its install-time partner — sound location). The regression-trap AC (line 98) names a concrete fixture: two transitions with `from: confirmed, verb: ratify` differing only in `guard: "tier == 'T2'"` vs `"tier == 'T3'"`. Both share the same `from` and `verb`; differ only in guard. Test would not pass for the wrong reason. Critical ordering note (line 95) calls out that `merged_entry` must be built before the helper call. Decision Matrix row 1 documents the choice with rationale.

2. **Phase 3 DDL audit (cycle-1 §3a) — CLOSED.** Plan enumerates 17 sites (main.md:121–137). Independent re-run of the canonical sweep `grep -rn -E 'INTO \{|FROM \{|UPDATE \{' src/` returns 16 matches; +1 for `ddl.rs:95` (CREATE TABLE, not in INTO/FROM/UPDATE pattern) = 17. Matches exactly. All cycle-1-flagged misses (drive.rs:879/1174, next_action.rs:381/399, submit.rs:81/94/112, drive.rs:246) are present.

3. **T005-touch is purely mechanical (cycle-1 §7a) — CLOSED.** Line 138 explicitly states "Sites 12–17 are inside T005-shipped files (drive.rs, next_action.rs) but the change is mechanical SQL-string quoting only, not a behaviour change to drive/next_action logic" and explicitly fences "we do NOT touch progress flushing, parse_envelope, or is_blocked helpers." No refactor sneaking in.

4. **Phase 4 pipe AC coverage (cycle-1 §3 / 4a caveat) — CLOSED.** Three explicit ACs at lines 161–163: (a) pipe-free repeatable `--in-scope "a" --in-scope "b"` → `["a","b"]`; (b) backwards-compat pipe form `--in-scope "a|b"` → `["a","b"]`; (c) mixed form `--in-scope "a|b" --in-scope "c"` → `["a","b","c"]`. Each AC names actual values and expected stored array. Mixed-form AC also documents the known pipe-in-value limitation.

5. **Phase 5 artefact uses pipe-free values (cycle-1 §3 / 4a) — CLOSED.** Step 13 (line 188) uses `--in-scope "main.py" --in-scope "scripts/"` and parallel `--in-scope "main.py|scripts/"`. Zero pipe characters within the values themselves. Explicit note: "do NOT use values containing |".

6. **Phase 2 bad-JSON UX (cycle-1 §3b) — CLOSED.** AC `list_record_bad_json_returns_validator_error` at line 113 asserts the error message must mention the field name (`external_refs`). Decision Matrix row 4 documents the choice (silent + enriched validator error).

7. **Phase 2 ListFk regression-trap (cycle-1 §3c) — CLOSED.** AC at line 112 literally says "re-run `tests/tasks_e2e.sh` (uses `linked_observations` and `depends_on` — the lifecycle-smoke canary) and confirm no regression." Decision Matrix row 8 documents the new CLI surface as additive.

8. **Decision Matrix arithmetic — PASS.** 9 rows total (was 7 + 2 new). New rows are: "Phase 1 selection algorithm: scope + helper location" (cycle-2 revision 1, expanded) and "Bad-JSON UX in list_record coerce" (cycle-2 revision 4) and "Pipe-containing values in list args" (cycle-2 revision 3). One row was expanded rather than added (Phase 1), and two are genuinely new — net +2 from cycle 1's 7. Math holds.

9. **No regression from cycle 1 — PASS.** Phase 5 still captures four artefacts (plus poc-rerun.log/sh = 6 files for 4 enforcement moments). Out-of-scope still lists T005-shipped code, schema migrations, T1 cosmetic warning, and 10.06 ports. T005 logic explicitly fenced at line 138.

10. **AC count constraint (≤4 per phase) — PASS.** Programmatic count: Phase 1=4, Phase 2=4, Phase 3=4, Phase 4=4, Phase 5=4. All within bound; sub-bullets used to fit additional traps inside the four top-level ACs.

**Strengths preserved from cycle 1:**
- DONE_WHEN clauses map 1:1 to phase ACs and Phase 5 artefacts.
- Phase independence and unit-test coverage per phase.
- Phase 4 blast-radius minimization via "join-with-pipe in get_arg".
- Decision Matrix names every high-stakes choice with rationale.

### Routing (cycle 2)
→ **READY** — orchestrator may move folder to `tasks/active/` and dispatch executor for Phase 1.

---

## Execution Log

### Phase 1 — Finding A: plain-transition guard evaluation
- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Completed:** 2026-04-30
- **Commit SHA:** (see below)
- **Files modified:**
  - `src/schema/lifecycle.rs` — added `select_transition` helper (pub fn); updated `validate_transition_ambiguity` to allow guard-partitioned same-verb pairs
  - `src/handlers/submit.rs` — collapsed `find_transition` to a thin delegator to `select_transition`
  - `src/handlers/transition.rs` — rewired `run_in_tx`: reordered to build merged entry before transition selection, replaced bare `.find(|t| t.verb == verb)` with `select_transition`; added 6 regression-trap tests
  - `tasks/active/T006-substrate-cleanup-poc/main.md` — this log
- **Notes:**
  - `select_transition` lives in `src/schema/lifecycle.rs` next to `validate_transition_ambiguity` (install-time partner), as per the plan's Decision Matrix.
  - `validate_transition_ambiguity` was updated to distinguish "fully unguarded" (both `requires_gate=None` AND `guard=None`) from "guard-partitioned" pairs. The regression-trap test schema has two same-verb transitions both with `guard:` but no `requires_gate`; the original validator would have rejected the schema at load time. This update is a necessary companion to the runtime fix — without it, guard-partitioned schemas cannot be installed.
  - The `state_machine_rejects_wrong_from_state` test assertion was updated: the old message "cannot {verb}: row is in state ..." is replaced by `select_transition`'s "no transition from '{from}' via verb '{verb}' found in schema". Semantics preserved, wording changed by the reorder.
  - `tasks_e2e.sh` Step 16 fails with a SIGPIPE/pipefail issue on `cargo test ... | grep -q` — confirmed pre-existing at HEAD before Phase 1 (stash verified). Not introduced by this phase.
  - `cargo test --all`: 380 passed, 0 failed.

### Phase 2 — Finding B: list_record / list_fk write path
- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Completed:** 2026-04-30
- **Commit SHA:** e079400
- **Files modified:**
  - `src/handlers/row.rs` — added `ListRecord` and `ListFk` arms to `coerce_value`; added 5 unit tests for coerce round-trip and bad-JSON fail-silent behaviour
  - `src/handlers/add.rs` — extended match arm from `Record|List` to also cover `ListRecord|ListFk`; added `LIST_RECORD_SCHEMA` fixture, `list_record_schema_and_conn` helper, `list_record_cli_round_trips_as_array` and `list_record_bad_json_returns_validator_error` tests
  - `src/handlers/update.rs` — extended `List` arm to `List|ListRecord|ListFk`
  - `src/handlers/transition.rs::execute_transition_write` — extended `List` arm to `List|ListRecord|ListFk`
  - `tasks/active/T006-substrate-cleanup-poc/main.md` — this log
- **Audit results:**
  - `add.rs`: needed the fix — match arm was `Record|List` only, `ListRecord`/`ListFk` fell through to the `_` text arm, storing raw string
  - `update.rs`: needed the fix — same pattern, `FieldType::List(_)` only
  - `transition.rs::execute_transition_write`: needed the fix — `FieldType::List(_)` only
  - `row.rs::read_row`: already correct — all four types handled in the JSON deserialization branch (pre-existing, introduced in an earlier task)
- **Validator error message:** `pretty_print` formats as `- <field_name>: required`; for `external_refs` with bad JSON → `Null`, the error is `- external_refs: required`. Field name IS in the output. Confirmed by `list_record_bad_json_returns_validator_error` test asserting `msg.contains("external_refs")`.
- **Tests:** 387 passed, 0 failed (`cargo test --all`). `drive_e2e.sh` PASS. `e2e.sh` Step 6 and `tasks_e2e.sh` Step 16 failures are pre-existing (CLAUDECODE env var / SIGPIPE pipefail), confirmed in Phase 1 Code Review.

### Phase 2 — REVISE cycle 1
- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Completed:** 2026-04-30
- **Commit SHA:** (see below)
- **Files modified:**
  - `src/handlers/row.rs` — changed parse-failure branch in `ListRecord|ListFk` arm from `Value::Null` to `Value::String(raw.to_string())` sentinel; updated 3 existing unit tests to assert `Value::String` instead of `Value::Null`
  - `src/validate/error.rs` — added `RuleKind::InvalidJsonArray` variant
  - `src/validate/mod.rs` — added type-shape check in `validate_field`: uses `required::lookup(entry, field_path)` to detect sentinel string at any depth (top-level OR nested inside a Record), emits field-named "value must be a JSON array, got string '...'" error and short-circuits; applies to both required and optional fields
  - `src/handlers/add.rs` — added `list_record_bad_json_optional_field_still_errors` test (optional field, asserts error contains field name + array hint); updated existing `list_record_bad_json_returns_validator_error` test to also assert array hint in wording
  - `tasks/active/T006-substrate-cleanup-poc/main.md` — this log
- **New test:** `list_record_bad_json_optional_field_still_errors`
- **Verbatim error from live repro:**
  ```
  Error: validation failed:
  - evidence.external_refs: value must be a JSON array, got string '{not json'
  ```
- **Test count delta:** 387 → 388 (net +1; 3 renamed tests, 1 new test, 2 removed old names)
- **Notes:**
  - Root cause: `external_refs` is nested inside `evidence` (a Record), not a top-level field. The initial top-level check in `validate`'s main loop missed it. Moving the check into `validate_field` (which is called for all depths) resolved the nesting issue.
  - `drive_e2e.sh`: PASS. `e2e.sh` Step 6 and `tasks_e2e.sh` Step 16 failures confirmed pre-existing.

---

## Code Review Log

### Phase 1 — Finding A: plain-transition guard evaluation
- **Reviewer:** code-reviewer
- **Date:** 2026-04-30
- **Commit reviewed:** `7d99727`
- **Gate:** PASS
- **Counts:** 0 critical / 0 major / 2 minor

**Verification (against Phase 1 ACs):**
- AC1 (guard-eval unit + ambiguity-validator preserved): **PASS** — `plain_transition_guard_false_rejected` and `plain_transition_guard_true_succeeds` cover both directions; install-time validator preserved (test `validate_transition_ambiguity_still_rejects_unguarded_same_verb_pairs`).
- AC2 (regression-trap for full selection algorithm): **PASS** — `GUARDED_PARTITIONED_SCHEMA` defines two `(from=confirmed, verb=ratify)` transitions partitioned by `guard: tier_hint == 'T2'/'T3'`. Test `guard_partitioned_picks_t3_transition_for_t3_row` would fail under the pre-fix `.find(|t| t.verb == verb)` (which would always pick T2 first). Confirmed the bug-trap is real.
- AC3 (submit.rs callers byte-identical): **PASS** — `find_transition` signature unchanged; collapsed to a 6-line delegator. Workflow tests still pass.
- AC4 (cargo test --all + tests/e2e.sh green): **cargo: PASS** (380 passed, 0 failed). **e2e.sh + tasks_e2e.sh pre-existing failures verified pre-existing on `cd5df8b` (master~1)** — not introduced by Phase 1. drive_e2e.sh: **PASS**.

**Headline algorithm extraction:** `select_transition` in `src/schema/lifecycle.rs:136-193` is the FULL algorithm, byte-for-byte equivalent to the pre-T006 `submit.rs::find_transition` (filter by from+verb+gate; prefer guarded-true; ambiguity check; unguarded fallback; bail). Not a thinned-out version.

**Validator-relaxation analysis:**
- Two `(requires_gate=None, guard=None)` pairs → STILL rejected (test covers).
- Two `(requires_gate=None, guard=Some(_))` pairs → accepted (transitively tested via `Schema::from_yaml(GUARDED_PARTITIONED_SCHEMA)`).
- One unguarded + one guarded → accepted (only unguarded counts toward `fully_unguarded`).
- Logic correct; deviation is sound.

**Reordering correctness:** `run_in_tx` builds merged BEFORE selecting — required because guards eval against post-diff entry. Old "row is in state X, expected Y" message replaced by `select_transition`'s "no transition from '{from}' via verb '{verb}' found in schema". Test `state_machine_rejects_wrong_from_state` updated to accept either message. Semantics preserved; wording slightly less informative for wrong-state case but still navigable.

**Out-of-scope check:** Files touched are exactly the 4 planned (`lifecycle.rs`, `submit.rs`, `transition.rs`, `main.md`). NO touches to drive.rs / next_action.rs / parse_envelope / mod.rs / status.rs. T005 fences hold.

**Pre-existing failure verification:** Stashed Cargo.lock, checked out `cd5df8b` (cycle-2-ready, immediately pre-Phase-1), rebuilt binary, re-ran:
- `tests/e2e.sh` Step 6 (actor mismatch on triage) → SAME failure pre-existed (CLAUDECODE not unset in script).
- `tests/tasks_e2e.sh` Step 16 (SIGPIPE on `cargo test ... | grep -q` under `set -o pipefail`) → SAME failure pre-existed.
Both confirmed pre-existing; not Phase 1 regressions.

**Minor findings (not blocking):**
1. **No direct positive-case test for the new validator branch.** The "guard-partitioned pairs are now accepted" path is only exercised transitively via `Schema::from_yaml(GUARDED_PARTITIONED_SCHEMA)` in the regression-trap tests. A dedicated `validate_transition_ambiguity_accepts_guard_partitioned_pairs` assert in lifecycle.rs would be 5 LOC and pin the new behaviour explicitly. Not blocking — the transitive coverage is sufficient (parse-time would fail if the validator rejected).
2. **Wrong-state error wording regression.** Pre-fix: `"cannot {verb}: row is in state '{X}', expected '{Y}'"` — names BOTH actual and expected state. Post-fix: `"no transition from '{X}' via verb '{verb}' found in schema"` — names actual only, leaves operator to grep schema for expected. Acceptable trade documented in execution log; if Phase 5 artefact capture (`ratify-rejected.txt`) wants the expected-state hint, consider enriching the bail message in `select_transition`'s empty-candidates branch.

**Routing:** Phase 1 PASS → Status advances `CODE_REVIEW` → `EXECUTING_PHASE_2` for Finding B (`list_record` write path).

---

### Phase 2 — Finding B: `list_record` / `list_fk` write path
- **Reviewer:** code-reviewer
- **Date:** 2026-04-30
- **Commit reviewed:** `e079400` (+ exec-log update `70a401d`)
- **Gate:** REVISE (substantial)
- **Counts:** 1 critical / 0 major / 1 minor

**Verification (against Phase 2 ACs):**
- AC1 (list_record CLI round-trip): **PASS for required fields.** Verified end-to-end via `show --json L001 | jq '.evidence.external_refs | type'` → `array`. The user-facing round-trip emits a structured JSON array, not an escaped-string blob. Out-of-the-box `read_row` was already correctly deserialising these column types (the diff confirms only the WRITE path was buggy); the read path was an existing correctness, not a Phase-2 addition.
- AC2 (list_fk CLI round-trip): **PASS.** Live test against `tasks` schema with `--linked-observations '["L001","L002"]'` → `show --json` emits `["L001","L002"]` as type `array`. The new additive CLI write surface for list_fk fields works as designed.
- AC3 (bad-JSON UX `list_record_bad_json_returns_validator_error`): **PARTIAL — see Critical 1.** Required-field path works (unit test passes; required validator surfaces field name in path prefix). Optional-field path silently corrupts.
- AC4 (cargo test --all + the three e2e scripts green): **cargo: PASS** (387 passed). **e2e.sh / drive_e2e.sh / tasks_e2e.sh:** drive_e2e.sh PASS; e2e.sh and tasks_e2e.sh failures verified pre-existing on `cd5df8b` (master pre-T006) — same env/SIGPIPE issues as Phase 1 noted, not Phase-2 regressions.

**Round-trip via `show --json`:** `array` (success). The DONE_WHEN moment for Finding B (evidence.external_refs renders as JSON array) is observable end-to-end on a fresh tempdir. POC trace:
```
$ stores observations_1006 add --summary "rt test" --source dev --priority normal \
    --captured-at 2026-04-30 \
    --external-refs '[{"system":"docker","kind":"container","id":"foo"}]'
L001
$ stores observations_1006 show L001 --json | jq '.evidence.external_refs | type'
"array"
```

**Out-of-scope check:** Files touched are exactly the 5 planned (`row.rs`, `add.rs`, `update.rs`, `transition.rs`, `Cargo.lock`, `main.md` — all within Phase 2 scope). NO touches to `lifecycle.rs` (Phase 1) or `select_transition`. NO touches to `drive.rs`/`next_action.rs`/`submit.rs`. T005 fences hold.

**Critical findings (gate-blocking):**

1. **Silent corruption on bad JSON to OPTIONAL list_record/list_fk fields.** The Decision-Matrix-blessed "fail-silent → validator surfaces field name" contract relies on the validator's `required` rule to translate `Value::Null` into a user-facing error. For an OPTIONAL list_record/list_fk field (e.g. `evidence.external_refs` in `observations_1006/schema.yaml` — no `required: true` at the field level), there is no `required` rule to fire. Bad JSON input is silently converted to `Value::Null` and accepted with no diagnostic. Reproduction:
   ```
   $ stores observations_1006 add --summary "bad json test" --source dev --priority normal \
       --captured-at 2026-04-30 --external-refs '{not json'
   L002
   (no error)
   $ stores observations_1006 show L002 --json | jq '.evidence'
   { "external_refs": null }
   ```
   The operator's input was discarded with zero feedback. This is exactly the "operator-debuggability floor" the AC text was written to prevent: AC L113 says the error must "indicate the value was rejected" — the optional case produces no error at all. The unit test `list_record_bad_json_returns_validator_error` only proves the required-field case; the realistic POC schema has the field optional.

   **Why this matters now, not later:** Phase 5's artefact capture (steps 8 + 11) writes `evidence.external_refs` and asserts the round-trip. If a future operator typos the JSON, the artefact will silently capture a Null without flagging the typo — the POC ratification narrative loses the "substrate enforces shape" thesis. This is also the exact UX regression cycle-2 revision 4 was added to prevent.

   **Fix options (planner's call but all are small):**
   - (a) Detect bad-JSON in `coerce_value` and stash a sentinel (e.g. `Value::String(raw)`) instead of `Value::Null`; validator surfaces "invalid JSON for `<field>`: <parse-error>" when it sees String-where-Array-expected. ~15 LOC.
   - (b) Coerce at the handler boundary (add.rs/update.rs) and bail with a clean error before validation. Slightly larger blast radius.
   - (c) Document the optional-field silent-corruption as accepted, downgrade AC L113 wording to "required-field only," and require operators of optional list_record fields to use the programmatic write path. Cheapest but bypasses the original UX intent.

   Recommended: (a). Add a unit test `list_record_bad_json_on_optional_field_surfaces_error` that asserts the error path also fires when the field is optional. This closes the operator-UX gap and matches the Decision Matrix row 4 enrichment language ("invalid JSON for external_refs").

**Minor findings (not blocking once Critical 1 is addressed):**

1. **`required` validator wording.** The bad-JSON unit test asserts the error contains `external_refs`; the actual emitted message is `"validation failed:\n- external_refs: required"`. This is a path-prefix from `pretty_print`, not a field-aware "JSON parse failure" message. AC text accepts either ("missing required field 'external_refs'" OR "invalid JSON for external_refs"); the current behaviour falls into the cheaper option. If the planner picks fix-option (a) for Critical 1, fold the wording-enrichment in at the same time so both required and optional paths use the same enriched message. ~5 LOC.

**What works (preserved correctness):**
- `List(_)` (pipe-split) coerce arm is untouched — no accidental behaviour change to existing pipe-separated list:text fields.
- `read_row` pre-existed correct for all four JSON types (Record/List/ListRecord/ListFk); the executor noted this; no read-path follow-up needed.
- Cargo.lock change (v0.4.1 bump propagation) committed cleanly in the same commit. No unstaged drift.
- All 387 cargo tests pass; drive_e2e.sh canary green.

**Pre-existing failure verification:** Confirmed e2e.sh Step 6 (CLAUDECODE inheritance) and tasks_e2e.sh Step 16 (SIGPIPE on `cargo test ... | grep -q` under `set -o pipefail`) reproduce on master pre-T006 (`33fac5a`). Not Phase-2 regressions; flagged for separate cleanup but out of scope here.

**Routing:** Phase 2 REVISE → Status `CODE_REVIEW` → `EXECUTING_PHASE_2`. Revision scope: address Critical 1 (silent-corruption on optional list_record/list_fk fields) — recommend fix-option (a). Add `list_record_bad_json_on_optional_field_surfaces_error` test pinning the new contract. Optionally fold Minor 1 wording enrichment in the same revision.

---

### Phase 2 — REVISE cycle 1 review
- **Reviewer:** code-reviewer
- **Date:** 2026-04-30
- **Commit reviewed:** `5256bfa`
- **Gate:** PASS
- **Counts:** 0 critical / 0 major / 0 minor

**Cycle-1 critical (silent corruption on optional list_record/list_fk) — CLOSED.**

Live repro against the canonical optional-field schema (`stores/observations_1006`) on a fresh tempdir:
```
$ stores observations_1006 add --summary "rev test" --source dev --priority normal \
    --captured-at 2026-04-30 --external-refs '{not json'
Error: validation failed:
- evidence.external_refs: value must be a JSON array, got string '{not json'
exit=1
```
Exit non-zero ✓; field name (`evidence.external_refs`) named ✓; "JSON array"/"got string" hints present ✓. Cycle-1 critical closed.

**Implementation review:**

1. **Sentinel choice (`row.rs::coerce_value`).** `_ => Value::Null` → `_ => Value::String(raw.to_string())` for ListRecord/ListFk only. `Value::Null` is a legal value for nullable fields (so the required-rule can't catch it on optional fields); `Value::String` is never a valid shape for a list_record/list_fk column, so the type-shape check fires unconditionally. Sound choice; matches recommended fix-option (a).
2. **`List(_)` arm untouched.** Pipe-split List logic at `row.rs:98-105` is unchanged. Sentinel logic only applies to `ListRecord | ListFk`. List vs ListRecord behaviour separation holds.
3. **Validator depth correctness.** Check moved into `validate_field` (lines 172-191), which is called for top-level fields AND recurses through Record/ListRecord nesting. Verified by live repro on both depths: nested (`evidence.external_refs` on observations_1006) and top-level (`linked_observations` on tasks). `required::lookup` is the depth-aware helper (descends into `Value::Object`).
4. **No double-firing.** Top-level run on bad `linked_observations` JSON produces exactly ONE `linked_observations` error, not two. The pre-fix top-level guard the executor mentioned isn't present — the check is only in `validate_field`. Confirmed by reading `validate/mod.rs:86-150` (main loop is purely structural recursion; no parallel ListRecord type-shape check). Short-circuit `return` at line 189 prevents required/enum/pattern/actor checks from also firing on the sentinel string.
5. **Test coverage hardening (cycle-1 minor).**
   - `list_record_bad_json_optional_field_still_errors` (new, `add.rs:367-394`): fixture has no `required: true` on `external_refs` (default false); asserts error contains field name AND array hint. Pins the optional-field floor — the exact case cycle-1's `_returns_validator_error` test missed.
   - `list_record_bad_json_returns_validator_error` (existing, required-field): now also asserts `msg.contains("array")`. Future regressions on the format wording trip this test.
   - Renames `_returns_null` → `_returns_sentinel_string` (3 tests in `row.rs`): semantic flip, not just label. Old assertion `assert_eq!(result, Value::Null, ...)` → `assert_eq!(result, Value::String(raw.to_string()), ...)`. Spot-checked all three: real semantic change.
6. **Happy path unaffected.** Round-trip valid `[{"system":"docker","kind":"container","id":"foo"}]` → `show --json | jq '.evidence.external_refs | type'` → `array`. No regression.
7. **Out-of-scope check.** `git show 5256bfa --stat` lists: `add.rs`, `row.rs`, `validate/error.rs`, `validate/mod.rs`, `main.md`, `global-task-manager.md`. NO touches to `lifecycle.rs`, `drive.rs`, `status.rs`, `next_action.rs`, `submit.rs`, `transition.rs`, `update.rs`. T005 + Phase 1 fences hold. (`update.rs` and `transition.rs` write paths still cover `ListRecord`/`ListFk` from the original Phase 2 commit; the sentinel `Value::String` is short-circuited at validate time, so write paths never see it.)
8. **Test counts and e2e.** `cargo test --all`: **388 passed** (was 387 pre-revise; +1 = `list_record_bad_json_optional_field_still_errors`). `drive_e2e.sh`: PASS. `e2e.sh` Step 6 (CLAUDECODE inheritance) and `tasks_e2e.sh` Step 16 (SIGPIPE on `cargo test ... | grep -q` under `pipefail`) failures verified pre-existing on master pre-T006; not REVISE-1 regressions. Confirmed Step 16 source still uses the SIGPIPE-prone pattern at `tests/tasks_e2e.sh:50-57`.
9. **Binary freshness.** Installed `stores` binary at `/home/blake/.cargo/bin/stores` was last installed at 23:02 (predating commit `5256bfa` at 23:13). Rebuilt via `cargo install --path . --features runner-claude-code` before running live repros; new binary timestamped 23:15. All live results above use the rebuilt binary.

**Minor 1 from cycle-1 (required-path wording polish) — IMPLICITLY CLOSED.** Required-field path now also surfaces "value must be a JSON array, got string ..." (since the type-shape check fires before the required-rule does, short-circuiting it). Both required and optional cases use the unified enriched message. The `list_record_bad_json_returns_validator_error` test now also asserts `msg.contains("array")`, pinning the new wording.

**Routing:** Phase 2 PASS → Status `CODE_REVIEW` → `EXECUTING_PHASE_3` for Finding C (DDL identifier escaping for hyphenated store names).

---

### Phase 3 — Finding C: DDL identifier escaping for hyphenated store names
- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Completed:** 2026-04-30
- **Commit SHA:** 0d992d3
- **Files modified:**
  - `src/codegen/ddl.rs` — added `pub(crate) fn quote_ident(name: &str) -> String` helper; applied at CREATE TABLE site (site 1); updated `ddl_snapshot` test (now expects `"kitchen_sink"` quoted); added 4 new tests: `quote_ident_plain`, `quote_ident_hyphenated`, `quote_ident_escapes_internal_double_quote`, `ddl_hyphenated_name_accepted_by_sqlite`
  - `src/handlers/add.rs` — imported `quote_ident`; applied at INSERT (site 2) and UPDATE display_id (site 3); added `HYPHEN_SCHEMA` const + `hyphen_schema_and_conn`/`build_add_cmd_for`/`build_verb_cmd_for` helpers + `hyphenated_store_name_crud_round_trip` trap-test
  - `src/handlers/row.rs` — imported `quote_ident`; applied at SELECT FROM read_row (site 4)
  - `src/handlers/list.rs` — imported `quote_ident`; applied at SELECT FROM (site 5)
  - `src/handlers/transition.rs` — imported `quote_ident`; applied at UPDATE in `execute_transition_write` (site 6)
  - `src/handlers/update.rs` — imported `quote_ident`; applied at UPDATE (site 7)
  - `src/handlers/submit.rs` — imported `quote_ident`; quoted inside `acquire_lock` (sites 8-9: UPDATE + SELECT), `release_lock` (site 10: UPDATE), `write_status_and_fields` (site 11: UPDATE)
  - `src/handlers/drive.rs` — imported `quote_ident`; applied at auto-pick SELECT (site 12), INSERT test scaffold (site 13), UPDATE blocked_reason test fixture (site 14)
  - `src/handlers/next_action.rs` — imported `quote_ident` (inside test module only); applied at INSERT test scaffold (site 15), two UPDATE blocked_reason test fixtures (sites 16-17; `replace_all` caught both identical strings)
  - `tests/fixtures/obs-test-1006/schema.yaml` — new minimal fixture schema with `name: obs-test-1006`
  - `tasks/active/T006-substrate-cleanup-poc/main.md` — this log
- **Interpolation sites updated:** 17 of 17 (matches plan audit; grep confirms same 17 before fix)
- **Hyphenated CRUD test outcome:** `hyphenated_store_name_crud_round_trip` PASSES — add, read_row, list, update, transition all succeed against `obs-test-1006`; no SQL syntax errors; status transitions from `open` → `reviewed`
- **Test count delta:** 388 → 395 (binary suite) + 2 (integration suite) = 395+2 total. New tests: `quote_ident_plain`, `quote_ident_hyphenated`, `quote_ident_escapes_internal_double_quote`, `ddl_hyphenated_name_accepted_by_sqlite`, `hyphenated_store_name_crud_round_trip` (5 new); `ddl_snapshot` updated (counted as existing)
- **`ddl_snapshot` update:** quoted identifier is semantically identical to bare identifier in SQLite; snapshot now expects `CREATE TABLE IF NOT EXISTS "kitchen_sink" (...)`. All other snapshot assertions (column names, types, CHECK constraints) unchanged.
- **e2e regressions:** none introduced. `drive_e2e.sh` PASS. `e2e.sh` Step 6 (CLAUDECODE inheritance) and `tasks_e2e.sh` Step 16 (SIGPIPE/pipefail) confirmed pre-existing per stash comparison.
- **Notes:**
  - `submit.rs` acquire/release/write functions receive `table: &str` (raw name); quoting done inside each function via `let qtable = quote_ident(table)` — cleaner than quoting at each call site.
  - `next_action.rs` import is test-only; moved to `mod tests` block to suppress `unused_import` warning (the three uses are all inside `#[cfg(test)]`).

---

## Completion
_Final summary when task is complete._
