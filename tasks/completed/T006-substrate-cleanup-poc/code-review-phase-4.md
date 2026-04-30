# T006 Phase 4 — Code Review

- **Reviewer:** code-reviewer
- **Date:** 2026-04-30
- **Commit reviewed:** `00bb065` (`feat(T006-P4): repeatable list flags via ArgAction::Append; pipe-separated still works`)
- **Gate:** **PASS**
- **Counts:** 0 critical / 0 major / 2 minor (non-blocking, deferred)

---

## TL;DR

Phase 4 closes Finding D. `--in-scope X --in-scope Y` now parses equivalently to `--in-scope "X|Y"`, both produce `["X", "Y"]` in storage. `ArgAction::Append` is correctly confined to `FieldType::List(_)` — ListRecord, ListFk, and all scalar types are unaffected. Three new unit tests pin the three forms (pipe-free / pipe / mixed). Live integration on the L275 POC schema confirms all three forms round-trip, scalars still work as strings, ListRecord still rejects duplicate flags, and all four L275 enforcement moments (ratify-rejects-draft, evidence.external_refs JSON-array, hyphenated install, repeatable list flags) fire. Out-of-scope clean. T005 + earlier-phase fences hold.

---

## AC verification

| AC | Verdict | Evidence |
|----|---------|----------|
| `cli/dynamic.rs` adds `ArgAction::Append` ONLY for `FieldType::List(_)` (not ListRecord, ListFk, scalars) | PASS | `dynamic.rs:635` (`build_leaf_cmd_owned`) and `dynamic.rs:717` (`build_leaf_cmd`) both use `let is_list = matches!(leaf.field.ty, FieldType::List(_));` then `if is_list { arg = arg.action(ArgAction::Append); }`. Live: `--external-refs '[…]' --external-refs '[…]'` errors with "cannot be used multiple times" (ListRecord is not Append). |
| All 3 consumers (`add.rs` / `update.rs` / `transition.rs`) handle `try_get_many<String>` and join with `|` | PASS | `add.rs:38`, `update.rs:43`, `transition.rs:68` — all three `build_entry_map` closures use `match matches.try_get_many::<String>(cli_name) { Ok(Some(vals)) => …join("|"), _ => None }`. No `unwrap()`; defensive against `Err(_)` and `Ok(None)`. |
| Three new tests pass: `list_field_repeatable_form`, `list_field_pipe_form`, `list_field_mixed_form` | PASS | All three present in `add.rs:622-661`; `cargo test --all` shows 396 passed (393 → 396, +3 as claimed). The test helper `build_add_cmd_with_append` faithfully mirrors the production wiring (`ArgAction::Append` only when `FieldType::List(_)`). |
| `cargo test --all` passes | PASS | Re-ran: `396 passed; 0 failed; 0 ignored` (lib) + `2 passed` (schemas_validate_fixtures). |
| No NEW failures in `tests/e2e.sh`, `tests/drive_e2e.sh`, `tests/tasks_e2e.sh` (pre-existing OK) | PASS | `drive_e2e.sh`: PASS (T005 smoke un-regressed). `e2e.sh` Step 6 fails under `CLAUDE_CODE_*` env autodetection — pre-existing (PASS in clean env via `env -i`). `tasks_e2e.sh` Step 16 grep-pattern false-negative on `ac5_11b` — pre-existing harness quirk (test passes when run directly via `cargo test ac5_11b`). Both already documented as pre-existing in Phase 2/3 review logs. |

---

## Live integration trace (the marquee AC)

Installed `stores 0.4.1` from this branch into `/tmp/t006-p4-live`, ran `stores install observations_1006`, exercised all three forms.

### Form 1: Repeatable (pipe-free)

```
stores observations_1006 add --invoker human \
  --summary "p4 repeatable test" --source dev --priority normal --captured-at 2026-04-30 \
  --in-scope "main.py" --in-scope "scripts/" \
  --contract-state draft --type work --drafted-by reviewer --drafted-at 2026-04-30T12:00:00Z
→ L001
stores observations_1006 show L001 --json | jq '.intent_contract.in_scope'
→ ["main.py", "scripts/"]
```
PASS — exactly the fix Finding D demanded.

### Form 2: Pipe (backwards compat)

```
--in-scope "a|b|c"
→ L002
.intent_contract.in_scope == ["a", "b", "c"]
```
PASS — legacy form preserved.

### Form 3: Mixed

```
--in-scope "a|b" --in-scope "c"
→ L003
.intent_contract.in_scope == ["a", "b", "c"]
```
PASS — join-with-`|` strategy collapses both forms to a single coerce_value pipe-split, as designed.

### Scalar fields unaffected

`L001`'s `summary`, `source`, `priority` round-trip as strings (not arrays):

```
{ "summary": "p4 repeatable test", "source": "dev", "priority": "normal" }
```

The single-element join is identity, as advertised. PASS.

### ListRecord still single-value JSON

```
--external-refs '[{"kind":"url","value":"https://example.com"}]'
→ L004
.evidence.external_refs == [{ "kind": "url", "value": "https://example.com" }]
```
Phase 2 semantic preserved. PASS.

```
--external-refs '[…]' --external-refs '[…]'
→ error: the argument '--external-refs <external-refs>' cannot be used multiple times
```
ListRecord correctly rejects duplicate flags (no Append). PASS.

---

## L275 four enforcement moments — all fire

1. **ratify rejects draft** (Phase 1): `stores observations_1006 ratify L001` → `Error: no transition from 'open' via 'ratify' (gate None) had its guard satisfied`. PASS.
2. **evidence.external_refs JSON array** (Phase 2): L004 round-trip — confirmed JSON array, not quoted string. PASS.
3. **hyphenated store install** (Phase 3): `stores install` of `name: hyphen-store` fixture → `Installed store 'hyphen-store' (table: hyphen-store)` cleanly. PASS.
4. **repeatable list flags** (Phase 4): all three forms above. PASS.

---

## Fence audit

### Out-of-scope: clean

`git show 00bb065 --stat`:

```
src/cli/dynamic.rs                              |  54 +++++-----
src/handlers/add.rs                             | 125 +++++++++++++++++++++++-
src/handlers/transition.rs                      |  11 ++-
src/handlers/update.rs                          |  11 ++-
tasks/active/T006-substrate-cleanup-poc/main.md |  25 ++++-
```

NOT touched: `lifecycle.rs`, `validate/`, `row.rs`, `codegen/ddl.rs`, `submit.rs`, `drive.rs`, `next_action.rs`. Matches the planned scope exactly.

### T005 + earlier-phase fences

- `transition.rs`: edits at lines 65-74 (the `get_arg` closure inside `run_in_tx`). The `select_transition` call site, the state-machine logic, and the storage match arms (Phase 2) are untouched.
- `add.rs`: edits at lines 34-44 (`get_arg` closure) plus the new tests in `mod tests` at lines 545-661. The Phase 2 storage match arm at the top of `run` is untouched.
- `update.rs`: edits at lines 39-49 (`get_arg` closure). Storage logic untouched.

All fences hold. Phase 4 is exclusively in the get-arg/CLI-wiring layer.

---

## Findings

### Critical: 0
### Major: 0
### Minor: 2 (non-blocking, deferred)

**Minor 1 — `get_arg` closure duplication.** The `try_get_many` + join-with-`|` block is now duplicated verbatim across `add.rs:34-44`, `update.rs:39-49`, and `transition.rs:65-74`. The Phase 4 plan considered consolidating into `row.rs::build_entry_map` as the central choke point, but chose duplication for blast-radius minimization. Fine for now; raises maintenance cost of any future change to list-arg handling. Could be hoisted in a future hardening pass.

**Minor 2 — `try_get_many::<String>` `Err` arm swallowed.** `try_get_many` returns `Err(_)` only when the registered value type doesn't match `String`. The `_ => None` arm swallows that, so a future schema change registering a non-String list field would silently null the value rather than surfacing the programming error. Not exploitable today (every leaf arg is String). A `debug_assert!` would catch it in test builds. Theoretical; deferred.

---

## Routing

**Phase 4 PASS** → Status `CODE_REVIEW` → `EXECUTING_PHASE_5` (L275 POC re-run integration phase).

After Phase 5's artefact capture, Status advances to `MERGE_REVIEW` (T006 has 5 phases; Phase 5 is the last).
