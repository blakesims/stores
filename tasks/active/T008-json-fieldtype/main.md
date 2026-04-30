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
- **Commit SHA:** (see below)
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

_Executor agent fills this section per phase._

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

---

## Completion
_Final summary when task is complete._
