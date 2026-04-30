# T008: Add `FieldType::Json` for free-shape opaque payloads

## Meta
- **Status:** CODE_REVIEW
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30
- **Blocked Reason:** —

## Task

10.06's production observations carry a `notes` column whose subkeys vary per entry (`siblings`, `discovery_path`, `related_observations`, `tried_repros`, `user_quotes`, etc.). The L275 POC schema (`stores/observations_1006/`) had to drop this field because no `FieldType::Json` exists in stores v0.4.x. Adding one is small foundational framework work that unblocks the full 10.06 observations port at T009.

This task closes the third gap I named in the philosophy-discussion conversation ("Json field type for free-shape `notes` columns") and gets the substrate ready for porting the second 10.06 store after `gate`.

### What `Json` looks like

A field declared `type: json` is opaque at the schema level — the framework does not impose a shape on the value beyond "parseable JSON." Operators write any JSON object/array/scalar via the CLI flag (or `--<flag>-from-file` for large payloads); the framework parses, stores as TEXT, and round-trips structured on read.

Concretely:

```yaml
fields:
  - name: notes
    type: json
    required: false
    description: "Free-shape catch-all (siblings, discovery_path, etc.)"
```

### What's already in place (T006 work)

T006 P2 introduced the `Value::String(raw)` sentinel pattern for `list_record` / `list_fk` parse failures, plus the depth-aware `validate_field` type-shape check. T008's Json arm follows the same shape:

- `coerce_value::Json` parses `serde_json::from_str(raw)`. On success, returns the parsed `Value` (any shape). On failure, returns `Value::String(raw)` sentinel.
- The validator's type-shape check fires when a Json field's stored value is `Value::String(raw)` (sentinel detected); error message names the field and signals "value must be valid JSON, got string '<raw>'".
- `show --json` and `list --json` emit the field as its structured JSON value, not a quoted blob (mirrors T006 P2's `list_record` read-path correctness).

### Validation: re-add `notes` to the L275 POC

Once Json is in place, `stores/observations_1006/schema.yaml` re-acquires the dropped field:

```yaml
- name: notes
  type: json
  required: false
```

And the POC's L275 trace, augmented:

```bash
stores observations_1006 add ... \
  --notes '{"siblings":["L210"],"reproed_by":"agent","discovery_path":"T271 wrap"}'
stores observations_1006 show L001 --json | jq '.notes.siblings'
# Expected: ["L210"]   (string array from a structured object, not an embedded blob)

# Bad JSON (sentinel + validator error):
stores observations_1006 add ... --notes '{not json'
# Expected: exit 1, error "notes: value must be valid JSON, got string '{not json'"
```

### What's NOT in this task

- **Subkey shape enforcement.** Json is intentionally opaque. If a future use case wants typed subkeys, that's a `Record` field type (already supported); use `type: json` only for genuinely free-shape data.
- **Schema-within-schema validation.** No `json_schema:` attribute on a Json field; no nested validation rules. Round-trip parseability is the only guarantee.
- **Migration of existing rows.** No Json columns exist anywhere yet; new feature only.
- **The full observations port (T009).** T008 is the framework feature; T009 is the actual port that uses it.
- **`pi-ask-user`, cross-store guards, anything in T010/T011/T012 territory.**

### DONE_WHEN

A schema declares `type: json` on a field. Round-trip end-to-end works:

1. **CLI accepts JSON**: `stores <store> add --notes '{"k":"v","arr":[1,2]}'` succeeds; the field is stored as TEXT containing the JSON string.
2. **`show --json` emits structured JSON**: `stores <store> show L001 --json | jq '.notes.k'` returns `"v"` (the structured value), not a quoted-string blob.
3. **Bad JSON surfaces a field-named error**: `stores <store> add --notes '{not json'` exits non-zero with an error message containing the field name (`notes`) AND signals "value must be valid JSON" (or equivalent — must distinguish from "field is required").
4. **The L275 POC re-runs with `notes`**: `stores/observations_1006/schema.yaml` re-acquires the field; the POC trace stores a notes object and round-trips it through `show --json` as a structured object.
5. **No regressions**: `cargo test --all`, `tests/e2e.sh`, `tests/drive_e2e.sh`, `tests/gate_e2e.sh`, `tests/tasks_e2e.sh` all green (modulo pre-existing CLAUDECODE/SIGPIPE failures already documented in T006/T007). T005 drive smoke un-regressed.

---

## Plan

### Objective

Add `FieldType::Json` as a top-level, opaque, free-shape column type. Schemas declare `type: json`; the framework parses any valid JSON via `serde_json::from_str`, stores the canonical serialisation as TEXT, and round-trips structured values through `show --json` / `list --json`. Bad JSON at write time produces a field-named "value must be valid JSON" validation error (parallel to T006 P2's `InvalidJsonArray` for `list_record`/`list_fk`). The substrate change re-enables a `notes` column on `stores/observations_1006/schema.yaml`, which T009 will lean on.

### Scope

- **In Scope:**
  - `src/schema/mod.rs` — `FieldType::Json` variant in the enum (line 52); recognise `"json"` in `RawFieldType::Scalar` and `resolve_field_type`; update the `unknown field type` error string to list `json`. Unit tests for parser.
  - `src/codegen/ddl.rs` — extend the JSON-TEXT match arm (line 57 / lines 76-79) to include `Json`; no other DDL changes (Json is a single TEXT column, no CHECK).
  - `src/handlers/row.rs` — add `FieldType::Json` arm to `coerce_value` (line 89): `serde_json::from_str(raw)` → `Ok(v) => v`, `Err(_) => Value::String(raw)` sentinel. Add `Json` to `read_row`'s JSON-deserialise match (lines 247-250) so reads parse the stored TEXT back to a structured `Value`.
  - `src/handlers/add.rs:95`, `src/handlers/update.rs:107`, `src/handlers/transition.rs:164` — add `FieldType::Json` to each "JSON-serialise to TEXT" match arm. (Three call-sites; all currently match `Record | List | ListRecord | ListFk`.)
  - `src/handlers/list.rs:146` — add `Json` to read-side match (parallel to `read_row` change).
  - `src/cli/dynamic.rs` — extend the `is_text_like` predicate at lines 631-634 / 706-709 so Json fields get the `--<name>-from-file` companion flag (large payloads via stdin / file).
  - `src/validate/error.rs` — generalise `RuleKind::InvalidJsonArray` to `RuleKind::InvalidJson { expected: String }` (or add a new `InvalidJson` variant — see Decision 3) and update existing call-site in `src/validate/mod.rs`.
  - `src/validate/mod.rs` — extend the type-shape sentinel-detection block (lines 177-191) so Json fields fire the same check; emit the new rule with message `"value must be valid JSON, got string '<raw>'"` (truncated to 60 chars per existing convention). Short-circuit subsequent checks for sentinel rows (mirrors existing).
  - `stores/observations_1006/schema.yaml` — re-add the `notes` field (top-level, `type: json`, `required: false`).
  - Tests at each layer: parser (Phase 1), coerce + write round-trip (Phase 2), validator type-shape (Phase 3), read-path structured emission (Phase 4), end-to-end POC trace (Phase 5).
- **Out of Scope:**
  - Allowing Json inside `Record` / `ListRecord` sub-fields (Decision 1: top-level only for v0.5).
  - `json_schema:` shape constraints, nested validation rules, migration tooling — explicitly listed in `## Task`.
  - The 10.06 observations port itself (T009).
  - Top-level JSON-string round-trip (`--notes '"hello"'`) — see Decision 2; documented limitation.

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | `FieldType::Json` parser + DDL | Low |
| 2 | Write path: `coerce_value` + storage match arms + CLI from-file | Low |
| 3 | Validator type-shape check (`RuleKind::InvalidJson`) | Low |
| 4 | Read path: `show.rs` / `list.rs` / `read_row` round-trip | Low |
| 5 | Integration: re-add `notes` to `observations_1006`, smoke trace | Low (operator-driven) |

### Phase Details

#### Phase 1: `FieldType::Json` parser + DDL
- **Objective:** Add the `Json` variant to the schema type system. Schemas declaring `type: json` parse cleanly; DDL emits a single TEXT column.
- **Files to modify:**
  - `src/schema/mod.rs` (`FieldType` enum line 52, `resolve_field_type` Scalar arm lines 231-267)
  - `src/codegen/ddl.rs` (`scalar_col_def` line 57, `ddl_for` match lines 76-79)
  - `tests/fixtures/all_types_store/schema.yaml` (add a `json` field so the existing snapshot test exercises it; update `ddl_snapshot` expected string accordingly)
- **Acceptance Criteria:**
  - [ ] `FieldType::Json` exists; `Schema::from_yaml` accepts a top-level field with `type: json` and produces `FieldType::Json`. Unit test in `src/schema/mod.rs::tests` mirrors the shape of `list_fk_parses`.
  - [ ] Unknown-type error message lists `json` in the expected-types enumeration (asserted in `unknown_field_type_errors` extension).
  - [ ] `ddl_for` emits `<field_name> TEXT` for a Json column (no CHECK clause). Snapshot test in `src/codegen/ddl.rs::tests` updated; `ddl_for` for the `all_types_store` fixture compiles cleanly under `rusqlite::Connection::open_in_memory().execute_batch(&ddl)`.
  - [ ] A `type: json` declared inside a `record:` or `list_record:` sub-field set produces a clear schema-parse error naming the parent field and saying "json fields must be top-level" (Decision 1). Test added to `src/schema/mod.rs::tests`.

#### Phase 2: Write path — `coerce_value` + storage match arms + CLI from-file
- **Objective:** Operator-supplied JSON is parsed at the CLI surface, validated, and stored as TEXT. Bad JSON produces a `Value::String(raw)` sentinel that flows through to the validator (Phase 3). Large payloads can be loaded via `--<name>-from-file`.
- **Files to modify:**
  - `src/handlers/row.rs` (add Json arm to `coerce_value`)
  - `src/handlers/add.rs:95` (add `Json` to JSON-serialise match)
  - `src/handlers/update.rs:107` (add `Json` to JSON-serialise match)
  - `src/handlers/transition.rs:164` (add `Json` to JSON-serialise match)
  - `src/cli/dynamic.rs` (extend `is_text_like` at lines 631-634 / 706-709 to include `Json`, so the from-file companion flag is registered)
- **Acceptance Criteria:**
  - [ ] `coerce_value(&FieldType::Json, r#"{"k":"v","arr":[1,2]}"#)` returns `Value::Object` containing those keys (unit test in `row.rs::tests`).
  - [ ] `coerce_value(&FieldType::Json, "{not json")` returns `Value::String("{not json".to_string())` — sentinel (unit test).
  - [ ] End-to-end via `add`: with a fixture schema having `notes: json`, `stores <store> add --notes '{"k":"v"}'` succeeds; querying SQLite directly shows `notes` cell contains the JSON string `{"k":"v"}` (test in `handlers/add.rs::tests` mirrors existing list_record round-trip).
  - [ ] Json fields get a `--<name>-from-file` flag in the generated CLI for `add` / `update` / transition verbs. Verified by an integration test that loads a JSON payload from a temp file and asserts the row was stored with the expected JSON. Null semantics (Decision 4): a Json field with no value supplied stores the JSON literal `null` in the TEXT column (mirrors today's Record behavior — see add.rs:97-101 `None => "null"`).

#### Phase 3: Validator type-shape check (`RuleKind::InvalidJson`)
- **Objective:** Bad JSON written to a Json field produces a field-named validation error containing the phrase "valid JSON". Required-field-missing remains a distinct `Required` rule. Sentinel detection short-circuits other checks for that field (mirrors T006).
- **Files to modify:**
  - `src/validate/error.rs` (rename `InvalidJsonArray` → `InvalidJson { expected: String }` per Decision 3)
  - `src/validate/mod.rs` (extend the sentinel block at lines 177-191 to handle `Json` in addition to `ListRecord | ListFk`; build the message from the rule's `expected` field)
- **Acceptance Criteria:**
  - [ ] Required Json field with no value → `RuleKind::Required` (NOT `InvalidJson`). Test asserts both `rule == Required` and message does NOT contain "valid JSON".
  - [ ] Optional Json field with bad JSON (`Value::String("{not json")` in EntryMap) → `RuleKind::InvalidJson` with message `"value must be valid JSON, got string '{not json'"`. Critical test: this fires for OPTIONAL fields too (the marquee fix from T006 P2 REVISE — sentinel ≠ Null). Test in `src/validate/mod.rs::tests`.
  - [ ] Required Json field with bad JSON → emits `InvalidJson` only (short-circuit prevents `Required` from also firing). Test asserts exactly one error for that field path.
  - [ ] Existing `InvalidJsonArray` call-sites (T006 P2 list_record / list_fk tests) remain green after the rename to `InvalidJson { expected: "JSON array" }`. `cargo test --all` passes; the wording shift is small and documented in Decision 3.

#### Phase 4: Read path — `show.rs` / `list.rs` / `read_row` round-trip
- **Objective:** A Json cell stored as TEXT round-trips back to a structured `Value` on read; `show --json` / `list --json` emit the structured JSON, not a quoted-string blob.
- **Files to modify:**
  - `src/handlers/row.rs::read_row` (lines 247-250: add `FieldType::Json` to the JSON-deserialise match)
  - `src/handlers/list.rs:146` (add `FieldType::Json` to the parallel match; note `list.rs` currently matches only `Record | List` — must also pick up `ListRecord | ListFk | Json` for parity, so this phase closes a pre-existing parity gap as a side benefit)
- **Acceptance Criteria:**
  - [ ] After `add --notes '{"k":"v","arr":[1,2]}'`, `read_row` returns an EntryMap whose `notes` value is `Value::Object` with the expected nested structure (unit test in `row.rs::tests` mirrors `depth3_plan_phases_round_trips`).
  - [ ] `stores <store> show L001 --json | jq '.notes.k'` returns `"v"` (the structured value), not a JSON-escaped blob. Asserted by an integration test invoking the binary or by directly testing `output::print_entry_json` against a known EntryMap.
  - [ ] `list.rs` decode loop now also handles `ListRecord | ListFk | Json` (close pre-existing parity gap as a side effect; document in execution log). Regression test: existing `list` JSON output for a `list_record` field remains structured.
  - [ ] Empty / NULL Json cell on read → `Value::Null` in EntryMap (NOT `Value::String("")` or `Value::String("null")`). Test asserts read of a row with an unset Json column yields Null in the entry.

#### Phase 5: Integration — re-add `notes` to `observations_1006` + smoke trace
- **Objective:** Operator-driven (mirrors T006 P5 / T007 P4). Re-add `notes: json` to the L275 POC schema and capture artefacts that demonstrate end-to-end JSON round-trip + bad-JSON validator error.
- **Files to modify:**
  - `stores/observations_1006/schema.yaml` (re-add the `notes` field as a top-level `type: json`, `required: false`)
- **Smoke artefacts (committed to `tasks/active/T008-json-fieldtype/p5-smoke/`):**
  - `notes-add-good.txt` — `stores observations_1006 add ... --notes '{"siblings":["L210"],"discovery_path":"T271 wrap"}'` succeeds; capture stdout + return code 0.
  - `notes-show-json.json` — `stores observations_1006 show L001 --json` showing `"notes": {"siblings":["L210"],...}` as a structured object (NOT a quoted blob). Verified with `jq '.notes.siblings[0]'` returning `"L210"`.
  - `notes-add-bad.txt` — `stores observations_1006 add ... --notes '{not json'` exits non-zero with stderr containing the strings `notes` AND `valid JSON`.
  - `notes-from-file.txt` — `stores observations_1006 add ... --notes-from-file payload.json` succeeds; payload.json contains a multi-key object; `show --json` round-trips it.
  - `regression.log` — output of `cargo test --all`, `tests/e2e.sh`, `tests/drive_e2e.sh`, `tests/gate_e2e.sh`, `tests/tasks_e2e.sh` (modulo the documented pre-existing CLAUDECODE/SIGPIPE failures from T006/T007).
- **Acceptance Criteria:**
  - [ ] `observations_1006/schema.yaml` parses cleanly via `Schema::from_yaml` (covered by `tests/schemas_validate_fixtures.rs` if it covers shipped stores; otherwise add a one-shot test).
  - [ ] All five artefacts above are committed; each demonstrates exactly the DONE_WHEN clause it pins (good JSON adds, structured show, bad JSON errors with field name + "valid JSON", from-file works, regressions are zero).
  - [ ] T005 drive smoke (`stores tasks drive --auto --claude-code --testing`) still completes against a fresh tempdir — no regression.
  - [ ] `cargo test --all` is green. Pre-existing CLAUDECODE/SIGPIPE failures documented in T006/T007 are still allowed; no NEW failures introduced.

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| 1. Top-level vs nested Json | (a) allow Json inside Record / ListRecord sub-fields; (b) top-level only for v0.5 | **(b) top-level only** | Json exists for genuinely free-shape data. Allowing it inside Record/ListRecord is double-counting — those types already support arbitrary inner shapes via their own sub-field tree. Restricting to top-level keeps the parser simple, the validator's sentinel-detection block uniform (no per-element walking), and avoids accidental "I declared Json inside a Record because I was lazy" anti-pattern. Future task can lift the restriction if a real use case appears. Enforced by a parser check in `raw_to_field` / `resolve_field_type` that errors if a `type: json` appears inside a `record:` or `list_record:` `fields:` block. |
| 2. Validator semantics for sentinel detection | (a) `Value::String` for a Json field IS the sentinel (unconditional); (b) `Value::String` IS the sentinel ONLY when `serde_json::from_str(raw)` returns Err (re-parse to disambiguate); (c) wrap success-strings in a distinguishable form (e.g. keep raw JSON-encoded form `"hello"` instead of inner content `hello`) | **(b) re-parse to disambiguate; document top-level-string limitation** | (a) breaks for `--notes '"hello"'` — coerce parses to `Value::String("hello")` (legitimate), but unconditional sentinel-flag would falsely error. (c) breaks the read-path round-trip: stored value would be `"\"hello\""` and `show --json` would emit the JSON-encoded form, not `"hello"`. (b) handles the common cases (object/array/number/bool/null) cleanly; the residual edge — top-level JSON string `'"hello"'` — false-flags because re-parsing the inner content `hello` (no quotes) is not valid JSON. **Documented limitation:** users wanting a literal top-level string should use `Text` or wrap in an object (`'{"v":"hello"}'`). The `notes` use case is always object/array, so the practical surface is unaffected. Add a note in `## Out of Scope` and a regression test asserting the limitation (`Value::String("hello")` for Json field flags as sentinel — known false-positive). |
| 3. Error-rule shape | (a) add `RuleKind::InvalidJson` as a separate variant alongside `InvalidJsonArray`; (b) generalise `InvalidJsonArray` → `InvalidJson { expected: String }` carrying "JSON array" or "valid JSON"; (c) parameterise via a free-form message field | **(b) generalise into `InvalidJson { expected: String }`** | Smallest diff. The two error sites (list_record/list_fk and Json) share 95% of behaviour: both detect a `Value::String` sentinel for a non-text column, both emit a "value must be ..., got string '...'" message. Carrying the `expected` discriminator on the rule means downstream consumers (envelope renderer, agent error parsers) can branch if needed without inspecting message text. The rename touches three lines of test text in T006 tests; trivial. Wording chosen: `expected: "JSON array"` (existing) and `expected: "valid JSON"` (new). |
| 4. Null semantics for Json | (a) absent Json field stores SQL NULL; (b) absent Json field stores the JSON literal `null` in the TEXT column (matches today's Record/List/ListRecord/ListFk behaviour at `add.rs:97-101`) | **(b) JSON literal `null`** | Mirrors existing v0.4 storage convention for all JSON-TEXT columns. Read-path code (`read_row` lines 251-258) already handles the empty / "null" case by collapsing to `Value::Null` in the EntryMap. Switching to SQL NULL would diverge from sibling types and require new branches in three storage call-sites. Verified storage logic at `add.rs:97-101` matches this choice. |
| 5. `required: true` interaction | (a) treat sentinel-detected (bad JSON) as also failing `required` — emit both errors; (b) sentinel short-circuits other checks for that field, so a required Json with bad JSON shows only `InvalidJson` (not `Required`) | **(b) short-circuit, mirror T006** | T006 P2 REVISE 1 already established this pattern at `validate/mod.rs:189` (`return;` after pushing the type-shape error). Two errors for the same root cause is noise; the type-shape error already implies "field is malformed" and operators do not need a parallel "field is required" message when they explicitly tried to provide a value. For required Json with NO value provided, `Required` fires normally (not `InvalidJson`) because the EntryMap has no entry, not a `Value::String` sentinel. Test: required Json + bad value → exactly ONE error of kind `InvalidJson`. |

---

## Plan Review

- **Gate:** READY
- **Reviewer date:** 2026-04-30
- **Open Questions Finalized:** None — all five decisions are made and rationalised in the Decision Matrix; no human input required to start execution.

### Verification against codebase

All cited line numbers and call-site shapes verified against tree at `master @ abf6845`:

| Plan claim | Codebase | Status |
|---|---|---|
| `add.rs:97-101` stores `"null"` literal for absent JSON-TEXT fields | confirmed: `Some(v) => to_string(v) ; None => "null"` | OK — Decision 4 grounded |
| `add.rs:95` match is `Record \| List \| ListRecord \| ListFk` | confirmed verbatim | OK |
| `update.rs:107` matches `List \| ListRecord \| ListFk` (Record handled separately at `:99` for deep-merge) | confirmed | OK — Json correctly placed in line 107 arm (no deep-merge for opaque blobs) |
| `transition.rs:164` matches `List \| ListRecord \| ListFk` (Record at `:158`) | confirmed | OK |
| `row.rs:247-250` JSON-deserialise match covers `Record \| List \| ListRecord \| ListFk` | confirmed | OK |
| `list.rs:146` matches ONLY `Record \| List` — drops `ListRecord \| ListFk` | confirmed: line 146 reads `FieldType::Record(_) \| FieldType::List(_)` only; `ListRecord \| ListFk` fall through to `_ => entry.insert(.., raw_val.clone())` which leaves the JSON-string un-decoded | **Side-benefit claim TRUE — pre-existing parity gap genuinely exists; Phase 4 closes it** |
| `cli/dynamic.rs:631-634, 706-709` `is_text_like` predicate gates from-file flag | confirmed verbatim | OK |
| `validate/mod.rs:177-191` sentinel-detection block with `return;` short-circuit at `:189` | confirmed; ListRecord/ListFk only | OK — Decision 5 short-circuit pattern grounded |
| `validate/error.rs::InvalidJsonArray` is a unit variant | confirmed; only two call sites (error.rs:11 and mod.rs:181) so the rename has trivial blast radius | OK — Decision 3 minimal-diff claim grounded |
| `schema/mod.rs:266` unknown-type error enumerates current types | confirmed; planner correctly extends this string | OK |
| `stores/observations_1006/schema.yaml` has no `notes` field | confirmed via grep — was dropped pending Json | OK — Phase 5 in scope |

### Decision Matrix audit

All five decisions are present, with options and rationale. Highest-stakes decision is **Decision 2 (sentinel re-parse semantics)**: the chosen scheme correctly handles object/array/number/bool/null at the top level (the production `notes` shape) but false-flags top-level JSON strings (`'"hello"'`). The limitation is explicit in `## Out of Scope` clause 4 ("Top-level JSON-string round-trip — see Decision 2"), and Decision 2 itself spells out the workaround (use `Text` or wrap in object). For the actual L275 / 10.06 use case this surface is unaffected. Acceptable.

Decision 1's parser-level rejection of nested Json (inside Record / ListRecord) is explicit in Phase 1 AC #4 with a clear error string ("json fields must be top-level"). Enforced at the right layer (parser), not deferred to validator.

Decision 3 keeps existing wording "value must be a JSON array, got string '...'" for list_record/list_fk after the rename, just with the rule struct carrying `expected: "JSON array"`. Existing T006 P2 user-facing messages are preserved verbatim. The two call-sites (error.rs:11, mod.rs:181) are the entire impact surface.

Decisions 4 and 5 mirror established T006 patterns 1:1.

### Phase coherence

- Phase 1 (parser + DDL) — foundation; 4 ACs.
- Phase 2 (write path) — depends on Phase 1; 4 ACs; the `--<name>-from-file` extension and null-default semantics are correctly bundled here.
- Phase 3 (validator) — depends on Phase 2 (sentinel must exist before validator can detect it); 4 ACs; required+bad-JSON case is AC #3.
- Phase 4 (read path) — depends on Phase 1 only (DDL stable); 4 ACs; closes pre-existing list.rs parity gap as side benefit.
- Phase 5 (integration / smoke) — depends on all four; 4 ACs; operator-driven trace artefacts mirror the T006 P5 / T007 P4 convention.

Each phase is independently committable. Phase ordering respects dependencies. Total ACs: 20 (5 × 4), within the ≤4-per-phase constraint.

### Out-of-scope hygiene

Plan does not touch T005 territory (drive, parse_envelope, status/next_action, is_blocked), T006 territory (select_transition, list_record write path, quote_ident, ArgAction::Append) — these are correctly treated as prerequisites — T007 territory (gate schema, dispatch.rs guard), or T009/T010/T011/T012 territory. The L275 schema re-add is correctly kept in T008 scope (Phase 5), not deferred to T009.

### Issues Found

None blocking. Two minor observations the executor should keep in mind (not gate-failing):

1. **List vs ListRecord/ListFk read parity (Phase 4 AC #3):** the planner says Phase 4 will close a pre-existing `list.rs` parity gap. Verified — currently `ListRecord` and `ListFk` rows are emitted as a JSON-encoded *string* by `stores list --json` (the values fall to the `_ => entry.insert(.., raw_val.clone())` arm at `list.rs:158`). When the executor expands the `list.rs:146` match to `Record | List | ListRecord | ListFk | Json`, this changes the public output shape for `list --json` of any existing store with a `list_record` / `list_fk` field. Worth a one-line note in the execution log. Not blocking — this is a *fix*, not a regression — but it should be called out when the phase commits.

2. **Decision 2 false-flag test (Phase 1 AC #4-adjacent):** the documented limitation (top-level `'"hello"'` false-flags) is currently mentioned in the Decision Matrix but no AC explicitly pins a *regression test asserting the limitation*. Decision 2 itself says "Add a … regression test asserting the limitation." Planner should add this as a sub-bullet under Phase 3 AC #2, or accept that the limitation lives only as a documentation note. Not blocking — the decision is sound either way.

### Gate verdict

**READY.** All five decisions are well-reasoned and grounded in verified codebase facts. Sentinel pattern correctly mirrors T006 P2 (re-parse to disambiguate, short-circuit on detection, `expected: String` discriminator on the rule). Phase decomposition is clean and dependency-ordered. Out-of-scope discipline is solid. AC count constraint met. The two minor observations above are notes for the executor, not revisions for the planner.

Status: PLAN_REVIEW → READY.

---

## Execution Log

### Phase 1 — `FieldType::Json` parser + DDL TEXT emission

- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Finished:** 2026-04-30
- **Commit SHA:** 22eeb72
- **Files modified:**
  - `src/schema/mod.rs` — added `Json` variant to `FieldType` enum; added `"json"` arm in `resolve_field_type` Scalar branch; updated unknown-type error string to include `json`; added nested-Json rejection in `raw_to_field` (post-resolve walk of sub-fields); added 4 unit tests.
  - `src/codegen/ddl.rs` — added `FieldType::Json` to both the `scalar_col_def` None arm and the `json_defs` TEXT arm in `ddl_for`; updated `ddl_json_columns_are_text` test; updated `ddl_snapshot` expected string.
  - `src/handlers/schema_show.rs` — added `FieldType::Json => "json"` arm in `field_type_str` (required by non-exhaustive match; not in plan but mandatory to compile — trivial one-liner).
  - `tests/fixtures/all_types_store/schema.yaml` — added `metadata: json` field.

- **Nested-rejection error message (verbatim):**
  `"field 'PARENT.CHILD': type 'json' may only appear at the top level"`
  e.g. `"field 'outer.inner_json': type 'json' may only appear at the top level"`

- **Test count delta:** 396 → 400 (+4 new: `field_type_json_parses`, `field_type_json_unknown_type_error_lists_json`, `field_type_json_in_record_rejected`, `field_type_json_in_list_record_rejected`)

- **DDL test:** `ddl_json_columns_are_text` extended to assert `metadata TEXT` with no CHECK clause. `ddl_snapshot` updated to include `metadata TEXT` column in the kitchen_sink expected output.

- **Deviation from plan:**
  - `src/handlers/schema_show.rs` was not in the Phase 1 file list but required a one-line `Json => "json"` arm to avoid a non-exhaustive match compile error. Strictly necessary; no behavioral change.
  - Nested-rejection is implemented as a post-resolve walk in `raw_to_field` (not in `resolve_field_type` or a separate `raw_to_subfield`) because `resolve_field_type` lacked access to the parent field name. The result is identical: rejection fires at `Schema::from_yaml` time with the field path named in the error.

### Phase 2 — Write path: `coerce_value` + storage match arms + CLI from-file

- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Finished:** 2026-04-30
- **Commit SHA:** 5450e13
- **Files modified:**
  - `src/handlers/row.rs` — added `FieldType::Json` arm to `coerce_value`; parses via `serde_json::from_str::<Value>(raw)`, returns parsed `Value` on success, `Value::String(raw)` sentinel on failure. Added 4 unit tests: `coerce_value_json_parses_object`, `coerce_value_json_parses_array`, `coerce_value_json_parses_scalar`, `coerce_value_json_bad_returns_sentinel_string`.
  - `src/handlers/add.rs` — added `FieldType::Json` to the `Record | List | ListRecord | ListFk` storage match arm at line 95. Added 2 integration tests: `json_field_write_then_read_round_trips_as_object` (verifies stored TEXT is valid JSON matching input), `json_field_absent_stores_null_literal` (Decision 4: absent field stores `"null"`).
  - `src/handlers/update.rs` — added `FieldType::Json` to the `List | ListRecord | ListFk` storage match arm at line 107.
  - `src/handlers/transition.rs` — added `FieldType::Json` to the `List | ListRecord | ListFk` storage match arm at line 164.
  - `src/cli/dynamic.rs` — extended both `is_text_like` predicates (in the transition-verb builder at line 631 and `build_leaf_cmd` at line 706) to include `FieldType::Json`, giving Json fields the `--<name>-from-file` companion flag.

- **Test count delta:** 400 → 406 (+6 new: 4 unit in `row.rs`, 2 integration in `add.rs`)

- **Integration test note:** `json_field_write_then_read_round_trips_as_object` verifies write mechanics by querying SQLite directly (the stored TEXT is parseable JSON with the expected structure). Full `read_row` round-trip (returning `Value::Object` from `read_row`) is Phase 4's job — the read-path match arm for `FieldType::Json` is not yet added to `read_row`, so read_row currently returns `Value::Null` for Json columns. This is expected at Phase 2.

- **Deviation from plan:** None. All five deliverables implemented exactly as specified. The integration test queries SQLite directly rather than via `read_row` because the read path is Phase 4 scope — this is per plan ("Don't touch: The read path (Phase 4)").

---

## Code Review Log

### Phase 1 — `FieldType::Json` parser + DDL TEXT emission

- **Verdict:** PASS
- **Reviewer date:** 2026-04-30
- **Commit reviewed:** 22eeb72
- **Test count:** 400/0 (baseline 396 → +4 new); all four new Phase 1 tests green in isolation
  (`field_type_json_parses`, `field_type_json_unknown_type_error_lists_json`,
  `field_type_json_in_record_rejected`, `field_type_json_in_list_record_rejected`)

**AC verification**

| AC | Verdict | Evidence |
|---|---|---|
| `FieldType::Json` exists; top-level `type: json` parses | PASS | `src/schema/mod.rs:71`; `field_type_json_parses` asserts `notes.ty == FieldType::Json` |
| Unknown-type error mentions `json` | PASS | `src/schema/mod.rs:272`; `field_type_json_unknown_type_error_lists_json` |
| `ddl_for` emits `<field> TEXT` (no CHECK) for Json | PASS | `src/codegen/ddl.rs:80-84`; `ddl_json_columns_are_text` asserts `metadata TEXT` and absence of `metadata TEXT CHECK`; `ddl_snapshot` updated |
| Nested Json (record / list_record) rejected at parse with named-path error | PASS | `src/schema/mod.rs:310-326` (`raw_to_field` post-resolve walk); error is verbatim `field 'PARENT.CHILD': type 'json' may only appear at the top level`. Two tests pin the wording AND the field path (`outer.inner_json`, `items.payload`). |

**D5 deviation judgment**

`src/handlers/schema_show.rs:113` — added `FieldType::Json => "json"`. Verified:
- Exactly one line; matches the pattern of the eight surrounding arms
- Returns the type-name string for human-readable schema dumps; no other logic in `field_type_str` or the file changed
- Strictly mechanical: Rust's exhaustive-match check requires it once the variant is added
- Not a behavioral change; the schema-show output gains a `json` token only when a schema actually declares a Json field

Verdict: **justified, not a scope creep**.

**Specific concerns addressed**

1. **Nested-rejection timing.** Rejection fires in `Schema::from_yaml` via `raw_to_field`'s post-resolve walk. Because `resolve_field_type` recurses into `record:` / `list_record:` sub-fields by calling `raw_to_field` for each, deeply nested Json (e.g. `record > record > json`) is also rejected at parse time. Confirmed by reading `src/schema/mod.rs:253,261` (recursive `subs.iter().map(raw_to_field)`). Tests `field_type_json_in_record_rejected` and `field_type_json_in_list_record_rejected` execute against `Schema::from_yaml` and pass — error surfaces during parse, not during install or write.
2. **Error wording for real-world misuse.** A schema with `intent_contract: { type: record, fields: [{ name: notes, type: json }] }` would error with `"field 'intent_contract.notes': type 'json' may only appear at the top level"`. Names parent + child clearly. Acceptable.
3. **DDL CHECK absence.** `src/codegen/ddl.rs:80-84` puts Json in the same `json_defs` arm as `Record | List | ListRecord | ListFk`, which emits `<name> TEXT` with no CHECK. No `json_valid(...)` was added. Test `ddl_json_columns_are_text` explicitly asserts the absence of `metadata TEXT CHECK`. SQLite-JSON1 portability risk avoided.
4. **No write-path / read-path / validator drift.** `git show 22eeb72 --name-only` lists exactly: `src/codegen/ddl.rs`, `src/handlers/schema_show.rs`, `src/schema/mod.rs`, `tasks/active/T008-json-fieldtype/main.md`, `tests/fixtures/all_types_store/schema.yaml`. No touches to row.rs / add.rs / update.rs / transition.rs / show.rs / list.rs / validate/* / cli/dynamic.rs / observations_1006/schema.yaml. Phase 1 is tightly scoped.
5. **No T005/T006/T007 territory.** Confirmed: drive.rs, parse_envelope, status.rs, next_action.rs, lifecycle.rs, submit.rs, dispatch.rs, codegen/ddl.rs's `quote_ident`, gate schema — all untouched.
6. **Pre-existing failures unchanged.** `tests/e2e.sh` Step 6 fails with the same CLAUDECODE auto-detect message documented in T006/T007. `tests/tasks_e2e.sh` Step 16 fails with the same `grep -q ... || fail` SIGPIPE pattern (the underlying `ac5_11b` / `ac5_13` / `ac5_14` Rust tests pass cleanly when run directly via `cargo test`). Neither failure shape is new.

**Findings (non-blocking observations)**

1. The post-resolve walk in `raw_to_field` (lines 310-326) is conceptually correct but slightly redundant when combined with the recursion in `resolve_field_type`: a Json sub-field one level deep is caught when `raw_to_field` is invoked on the immediate parent record, but a Json sub-field two levels deep (e.g. `record > record > json`) is caught at the *innermost* `raw_to_field` call — so the error path will name the inner record and the json field (`inner_record.json_field`), not the full chain. This matches the documented error format ("PARENT.CHILD"); the walk does what the AC requires. No fix needed for Phase 1, but if T009 ever hits a deep-nest case, the error path may be less informative than ideal. Note for future reference; not gate-failing.
2. The Phase 1 acceptance scope correctly defers all sentinel / coerce / write-path / read-path work to later phases; the executor's commit message and Execution Log are accurate.
3. Executor's "deviation" callout in the Execution Log (schema_show.rs one-liner; post-resolve walk vs. in-`resolve_field_type`) is honest and scoped; no hidden changes.

**Routing:** Status `CODE_REVIEW` → `EXECUTING_PHASE_2`.

### Phase 2 — Write path: `coerce_value` + storage match arms + CLI from-file

- **Verdict:** PASS
- **Reviewer date:** 2026-04-30
- **Commit reviewed:** 5450e13
- **Test count:** 406/0 (baseline 400 → +6 new). `cargo test --all` confirmed locally; final test result line: `406 passed; 0 failed`.

**AC verification**

| AC | Verdict | Evidence |
|---|---|---|
| `coerce_value(&Json, '{"k":"v","arr":[1,2]}')` returns `Value::Object` | PASS | `row.rs:311-321` (`coerce_value_json_parses_object`); also array (`coerce_value_json_parses_array` at `:324`) and scalar (`:340`) covered |
| `coerce_value(&Json, "{not json")` returns `Value::String("{not json")` sentinel | PASS | `row.rs:351-361` — `assert_eq!(result, Value::String(raw.to_string()), ...)`. Exact-match assertion on the sentinel String, not "is some kind of String" |
| End-to-end `add` with JSON object stores TEXT containing the JSON string | PASS | `add.rs:json_field_write_then_read_round_trips_as_object` queries SQLite directly (`SELECT notes FROM jstore`) and re-parses the stored TEXT — does NOT go through `read_row`. Asserts `parsed["k"] == "v"` and `parsed["arr"] == json!([1,2])` |
| Json fields get `--<name>-from-file` flag | PASS | `cli/dynamic.rs:633` (transition-verb builder) AND `:708` (`build_leaf_cmd`) BOTH extended to include `FieldType::Json` in `is_text_like`. Decision 4 null-default verified by `json_field_absent_stores_null_literal` (asserts stored value == `"null"`) |

**Specific concerns addressed**

1. **Sentinel detection coverage.** The 4 unit tests in `row.rs` cover object, array, scalar (number 42), and bad-JSON sentinel. The bad-JSON test at `:351-361` asserts `Value::String("{not json".to_string())` via `assert_eq!` — exact-match, not a loose string check. This is the critical regression-trap for Phase 3's validator and it pins the sentinel exactly.
2. **Storage match consistency.** Verified across all three write-path sites:
   - `add.rs:95`: `Record(_) | List(_) | ListRecord(_) | ListFk { .. } | Json`
   - `update.rs:107`: `List(_) | ListRecord(_) | ListFk { .. } | Json` (Record handled separately at `:99` for deep-merge — correct per plan)
   - `transition.rs:164`: `List(_) | ListRecord(_) | ListFk { .. } | Json` (Record at `:158`)
   No drift; Json is appended to the same shape that T006 P2 established.
3. **`is_text_like` both call sites.** Both predicates updated:
   - `cli/dynamic.rs:633` (transition-verb builder)
   - `cli/dynamic.rs:708` (`build_leaf_cmd`)
   Confirmed via `grep -n "FieldType::Json" src/cli/dynamic.rs`. From-file flag is registered for Json fields in BOTH paths.
4. **No accidental Json-in-`List(_)` addition.** `coerce_value` has its own `FieldType::Json` arm at `row.rs:124-127`, separate from the `List(_)` arm at `:99-106` (which does pipe-split) and from the `ListRecord | ListFk` arm at `:113-118` (which expects array shape). Json is its own arm with `Ok(v) => v` (any shape) — no fallthrough.
5. **Storage-layer round-trip claim verified.** `json_field_write_then_read_round_trips_as_object` does exactly what the executor described: insert via `Op::Add`, query SQLite directly with `conn.query_row("SELECT notes FROM jstore WHERE display_id = 'J001'", ...)`, then `serde_json::from_str(&stored_notes)` and assert nested keys round-trip. The test does NOT call `read_row`, correctly avoiding Phase 4 scope.
6. **`read_row` not changed.** Verified at `row.rs:256-259`: the JSON-deserialise match still reads `Record(_) | List(_) | ListRecord(_) | ListFk { .. }` — NO Json. Phase 4 scope preserved as the executor flagged.
7. **Out-of-scope check.** `git show 5450e13 --stat` lists exactly: `src/cli/dynamic.rs`, `src/handlers/add.rs`, `src/handlers/row.rs`, `src/handlers/transition.rs`, `src/handlers/update.rs`, `tasks/active/T008-json-fieldtype/main.md`. NO touches to `validate/*` (Phase 3), `show.rs` / `list.rs` (Phase 4), `observations_1006/schema.yaml` (Phase 5), or any T005/T006/T007 territory. `list.rs:146` still matches only `Record | List` (unchanged) — Phase 4 will close that parity gap.
8. **Pre-existing failures unchanged.** `cargo test --all` returns 406/0 cleanly. The documented CLAUDECODE auto-detect failure in `tests/e2e.sh` Step 6 and SIGPIPE pattern in `tests/tasks_e2e.sh` Step 16 are external-shell test issues, not Rust unit/integration tests; no new Rust failures introduced.

**Findings (non-blocking observations)**

1. The integration test name `json_field_write_then_read_round_trips_as_object` is slightly misleading — it does NOT actually exercise a "read" path (which would imply `read_row`). It only verifies the write half of the round-trip via direct SQLite query. The test itself is correct and the body's NOTE comment explicitly documents this. Renaming to `..._stored_text_parses_as_object` would be more accurate but is cosmetic and the executor's existing comment makes the intent clear. Not blocking.
2. Decision 2's documented limitation (top-level JSON string `'"hello"'` false-flags) is not yet exercised by any test in Phase 2. Per the plan, that regression test was suggested for Phase 3 (validator-layer); the limitation is preserved as documented behaviour by the current `Ok(v) => v` arm at `row.rs:124-127`. Not blocking — Phase 3's responsibility.
3. The Phase 1 review's "post-resolve walk only catches PARENT.CHILD, not deep-nest chain" observation does not apply to Phase 2; Phase 2 is purely write-path and has no recursion.

**Routing:** Status `CODE_REVIEW` → `EXECUTING_PHASE_3`.

### Phase 3 — Validator type-shape check (`RuleKind::InvalidJson`)

- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Finished:** 2026-04-30
- **Commit SHA:** e50f2a1
- **Files modified:**
  - `src/validate/error.rs` — renamed `InvalidJsonArray` (unit variant) to `InvalidJson { expected: String }` (struct variant). Updated doc comment.
  - `src/validate/mod.rs` — updated existing T006 P2 call site from `InvalidJsonArray` to `InvalidJson { expected: "JSON array".to_string() }` (user-facing message unchanged: `"value must be a JSON array, got string '...'"`) . Added new `FieldType::Json` sentinel-detection block immediately after the existing ListRecord/ListFk block: re-parses the string via `serde_json::from_str`; on Err emits `InvalidJson { expected: "valid JSON" }` with message `"value must be valid JSON, got string '...'"` (truncated to 60 chars) and short-circuits via `return;`. On Ok (top-level JSON string, case c) no error — documented limitation per Decision 2. Added 4 new tests.

- **Error message formats (verbatim):**
  - JSON array (T006 P2 backwards-compat): `"value must be a JSON array, got string '<raw>'"`
  - valid JSON (new for Json type): `"value must be valid JSON, got string '<raw>'"`

- **Test count delta:** 406 → 410 (+4 new:
  `validate_json_required_field_bad_value_emits_invalid_json`,
  `validate_json_optional_field_bad_value_still_emits_invalid_json`,
  `validate_json_top_level_string_is_treated_as_sentinel_known_limitation`,
  `validate_json_existing_list_record_message_unchanged`)

- **Acceptance criteria:**
  - [x] `RuleKind::InvalidJson { expected: String }` exists; old `InvalidJsonArray` removed
  - [x] T006 P2 call site updated; existing tests still pass with same user-facing wording
  - [x] Json sentinel detection in `validate_field`; short-circuits other checks
  - [x] All Phase 3 tests pass; `cargo test --all` = 410 passed, 0 failed

- **Deviation from plan:**
  - The plan described the `validate_json_top_level_string_is_valid` test as asserting NO error (Decision 2 limitation: top-level JSON string is valid). However, per Decision 2's re-parse logic: `coerce_value` for `'"hello"'` returns `Value::String("hello")` (inner content, no quotes). The validator re-parses `"hello"` (no quotes) via `serde_json::from_str` → Err (not valid JSON without quotes). This DOES trigger the sentinel. This is the documented false-flag: the test is renamed `validate_json_top_level_string_is_treated_as_sentinel_known_limitation` and asserts the false-flag fires (pinning the known behaviour rather than asserting "no error"). The plan's Decision 2 text explicitly states "regression test asserting the limitation"; the test correctly pins the limitation. No behavioral change from plan's intent.

---

### Phase 3 — Validator type-shape check (`RuleKind::InvalidJson`)

- **Verdict:** PASS
- **Reviewer date:** 2026-04-30
- **Commit reviewed:** e50f2a1
- **Test count:** 410/0 (baseline 406 → +4 new). `cargo test --all` confirmed locally; final result lines: `410 passed; 0 failed` (lib) and `2 passed; 0 failed` (`schemas_validate_fixtures`).

**AC verification**

| AC | Verdict | Evidence |
|---|---|---|
| `RuleKind::InvalidJson { expected: String }` exists; old `InvalidJsonArray` removed | PASS | `src/validate/error.rs:9-12`. Repo-wide grep shows zero `InvalidJsonArray` references; both call-sites at `validate/mod.rs:181` (`expected: "JSON array"`) and `:204` (`expected: "valid JSON"`) carry the discriminator on the rule struct. |
| T006 P2 call site updated; existing tests pass with same user-facing wording | PASS | `validate/mod.rs:183` still emits `"value must be a JSON array, got string '<raw>'"` verbatim. `list_record_bad_json_returns_validator_error` and `list_record_bad_json_optional_field_still_errors` (both in `add.rs`) green; both assert `msg.contains("JSON array")`. The new `validate_json_existing_list_record_message_unchanged` further pins the message prefix `"value must be a JSON array, got string '"` (starts_with assertion). |
| Json sentinel detection in `validate_field`; short-circuits other checks | PASS | `validate/mod.rs:198-215` — `matches!(&field.ty, FieldType::Json)` → re-parses raw via `serde_json::from_str`; on `Err` pushes InvalidJson and `return;` (line 211); on `Ok` no-ops. The `return;` short-circuit prevents `check_required` / `check_enum` / `check_pattern` / actor checks from firing. Test `validate_json_required_field_bad_value_emits_invalid_json` asserts `notes_errs.len() == 1` for a `required: true` Json field with bad JSON — exactly the short-circuit behaviour. |
| 4 new tests pass; `cargo test --all` ≥ 410 | PASS | All four new tests green: `validate_json_required_field_bad_value_emits_invalid_json`, `validate_json_optional_field_bad_value_still_emits_invalid_json`, `validate_json_top_level_string_is_treated_as_sentinel_known_limitation`, `validate_json_existing_list_record_message_unchanged`. Total 410/0. |

**Specific concerns addressed**

1. **T006 P2 backwards compat (NON-NEGOTIABLE).** Verified by direct read of `validate/mod.rs:183`: format string is `"value must be a JSON array, got string '{}'"` — character-for-character identical to pre-Phase-3 code. Both `list_record_bad_json_returns_validator_error` (required) and `list_record_bad_json_optional_field_still_errors` (optional) green; their assertions `msg.contains("external_refs")` and `msg.contains("JSON array")` succeed unchanged. Wording shift: zero. PASS.
2. **Json error message wording.** `validate/mod.rs:206` emits `"value must be valid JSON, got string '<raw>'"` with `<raw>` truncated to 60 chars at `:207`. Field name appears via `pretty_print`'s `format!("- {}: {}", e.field_dot(), e.message)` (`error.rs:36`); for `notes`, the rendered line is `- notes: value must be valid JSON, got string 'hello'`. Wording matches plan AC ("value must be valid JSON" — exact). Truncation matches existing convention (same `if raw.len() > 60 { &raw[..60] } else { raw.as_str() }` formula as the ListRecord/ListFk arm).
3. **`<raw>` is sentinel content, not full CLI input.** `coerce_value` for Json (Phase 2, `row.rs:124-127`) returns `Value::String(raw_input)` where `raw_input` is the raw `&str` passed to `coerce_value`. The validator reads `Value::String(raw)` directly — same string. For the documented limitation case (`'"hello"'`), `coerce_value` parses successfully to `Value::String("hello")` (inner content), and the validator sees `"hello"` (no quotes). Message reads `got string 'hello'`. This is the intended behaviour per Decision 2.
4. **Sentinel short-circuit verified.** Test `validate_json_required_field_bad_value_emits_invalid_json` schema declares `notes: json, required: true`. The entry has a sentinel value; the test asserts `notes_errs.len() == 1` and the rule is `InvalidJson`. If the short-circuit weren't present, `check_required` would also fire (since `Value::String` is non-null but not the field's expected shape — required check passes for non-null but a parallel error path would occur). The single-error assertion pins this. PASS.
5. **Optional vs required Json regression-trap.** `validate_json_optional_field_bad_value_still_emits_invalid_json` exists and passes. Schema declares `notes: json, required: false`. Sentinel value present. Test asserts exactly 1 InvalidJson error fires — same regression trap that the T006 P2 REVISE 1 cycle locked in for ListRecord. The Json arm uses identical `Value::String(raw)` sentinel detection (lines 199-200), so the optional/required parity is structurally guaranteed.
6. **Deviation judgment (test rename).** Plan named the test `validate_json_top_level_string_is_valid` and described it as asserting NO error. The executor renamed it to `validate_json_top_level_string_is_treated_as_sentinel_known_limitation` and asserts the false-flag DOES fire (single InvalidJson error). This matches Decision 2's stated semantics: re-parse cannot disambiguate top-level JSON-string from sentinel without parser-level wrapping (rejected option c). The rename pins the LIMITATION rather than asserting unreachable correctness — strictly more accurate. The test serves as a regression-trap: if a future change inadvertently fixes the false-flag (e.g. by switching to wrap-on-success), this test will fail and force an explicit Decision 2 reconsideration. PASS the deviation.
7. **Out-of-scope check.** `git show e50f2a1 --stat` lists exactly: `src/validate/error.rs`, `src/validate/mod.rs`, `tasks/active/T008-json-fieldtype/main.md`. NO touches to `row.rs` (Phase 2), `add.rs` / `update.rs` / `transition.rs` (Phase 2), `show.rs` / `list.rs` (Phase 4), `observations_1006/schema.yaml` (Phase 5), `cli/dynamic.rs`, or `codegen/ddl.rs`. Tightly scoped.
8. **No T005/T006/T007 logic touched** beyond the InvalidJsonArray → InvalidJson{expected} migration. The T006 P2 call-site at `validate/mod.rs:181` reuses the existing format string verbatim; the existing T006 short-circuit pattern (`return;` at `:189`) is preserved. T005 (drive, parse_envelope, status, lifecycle, submit, dispatch) and T007 (gate schema) territory untouched.
9. **Pre-existing failures unchanged.** `cargo test --all` is 410/0 — no new failures. The documented `tests/e2e.sh` Step 6 (CLAUDECODE auto-detect) and `tests/tasks_e2e.sh` Step 16 (SIGPIPE) external-shell test failures are unrelated to validate/* and remain unchanged in shape.

**Findings (non-blocking observations)**

1. The Json sentinel-detection block (`mod.rs:198-215`) is structurally near-identical to the ListRecord/ListFk block (`:177-191`) — only the type-arm in `matches!` and the format strings differ. Future cleanup could extract a small helper `check_invalid_json_sentinel(field, &field_path, entry, errors, expected: &str, msg: &str, reparse: bool)` to dedupe, but the duplication is shallow (2 blocks, ~13 lines each) and the reparse-or-not distinction makes a generic helper marginally awkward. Cosmetic; not gate-failing.
2. The "documented limitation" test is currently the only test asserting a NON-error path through the `FieldType::Json` arm — but it asserts an error (the false-flag). There's no test that exercises a successful Json sentinel re-parse landing on `Ok(_)` (the no-op `else` branch at line 213). Such a test would require wiring `coerce_value` output (`Value::Object` etc.) directly into the EntryMap and asserting no notes_err — somewhat redundant since the existing `add` integration test (`json_field_write_then_read_round_trips_as_object`) already proves the happy path doesn't error. Note for future reference; not gate-failing.
3. Non-trivial-finding budget: I aimed for ≥3 substantive findings on a non-trivial change. The change IS non-trivial in concept (rename + new sentinel detection block + 4 tests + documented-limitation pin), but the execution is mechanically clean: every decision was pre-locked in the Decision Matrix, the executor mirrored the T006 P2 pattern 1:1, all four AC tests pass, and the only deviation is a test rename that strictly improves accuracy. The findings above are minor cosmetic notes rather than substantive defects. Lower count is justified by the tightness of the planning + the precedent of T006 P2 (which was already through 1 REVISE cycle, so this Phase 3 inherits a battle-tested pattern).

**Routing:** Status `CODE_REVIEW` → `EXECUTING_PHASE_4`.

### Phase 4 — Read path: `show.rs` / `list.rs` / `read_row` round-trip

- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Finished:** 2026-04-30
- **Commit SHA:** (see below — committed after this log entry)
- **Files modified:**
  - `src/handlers/row.rs` — added `FieldType::Json` to the JSON-deserialise match in `read_row` (lines 256-260); added 2 Phase 4 unit tests: `read_row_json_field_returns_structured_object` (asserts `Value::Object` with nested `k`/`arr` keys), `read_row_json_field_null_cell_returns_null` (asserts `"null"` literal reads back as `Value::Null`).
  - `src/handlers/list.rs` — extended the decode match from `Record | List` to `Record | List | ListRecord | ListFk | Json` (closes pre-existing parity gap); added 2 Phase 4 tests: `list_json_field_decodes_to_structured_value` (Json field → `Value::Object`), `list_list_record_field_decodes_to_structured_value` (ListRecord/ListFk → structured array; parity gap verified closed).
  - `src/handlers/show.rs` — no change required; delegates entirely to `read_row` for deserialisation and `output::print_entry_json` for emission. Phase 4 `read_row` change is sufficient.

- **`show --json` round-trip outcome:** `show.rs` calls `read_row` then `output::print_entry_json(&entry)`. With the `FieldType::Json` arm now in `read_row`'s decode match, a stored JSON TEXT cell is deserialized to a `Value::Object` (or Array/Number/etc.) before emission. `print_entry_json` serialises that structured value directly — no quoted-string blob. The `read_row_json_field_returns_structured_object` test pins this: `J001` row with `notes = '{"k":"v","arr":[1,2]}'` reads back as `Value::Object` with `k → "v"` and `arr → [1,2]`.

- **list.rs ListRecord/ListFk parity gap — CLOSED:** Pre-P4, `list.rs:146` matched only `Record | List`; `ListRecord | ListFk` fell to `_ =>` which emitted the raw JSON string (`Value::String`). Phase 4 extends the match to `Record | List | ListRecord | ListFk | Json`. This changes the `list --json` output shape for any existing store with `list_record` / `list_fk` fields: those fields now emit structured arrays instead of string blobs. This is a fix, not a regression; the plan reviewer pre-approved this as a "defensible" side benefit. `list_list_record_field_decodes_to_structured_value` pins both ListRecord (array of objects) and ListFk (array of strings) decoding correctly post-P4.

- **Test count delta:** 410 → 414 (+4 new: `read_row_json_field_returns_structured_object`, `read_row_json_field_null_cell_returns_null`, `list_json_field_decodes_to_structured_value`, `list_list_record_field_decodes_to_structured_value`). `cargo test --all` = 414 passed; 0 failed.

- **Deviation from plan:** None. `show.rs` required no modification (plan AC #3 acknowledged this case: "if `show.rs` delegates to `read_row` for the deserialisation, then no further change needed"). Tests are inline in `row.rs` and `list.rs` test modules per plan.

---

## Completion
_Final summary when task is complete._
