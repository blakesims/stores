# T008: Add `FieldType::Json` for free-shape opaque payloads

## Meta
- **Status:** PLANNING
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
_Planner agent fills this section._

### Objective
_What we're trying to achieve._

### Scope
- **In Scope:**
  - `src/schema/mod.rs` — `FieldType::Json` variant + parser
  - `src/codegen/ddl.rs` — Json column gets SQL `TEXT` (no special handling beyond that)
  - `src/handlers/row.rs::coerce_value` — Json arm parses `serde_json::from_str`; sentinel `Value::String(raw)` on failure
  - `src/handlers/add.rs / update.rs / transition.rs::execute_transition_write` — write path includes Json in the JSON-serialise match (already covers `Record | List | ListRecord | ListFk` post-T006; add `Json`)
  - `src/handlers/show.rs / list.rs` — read path deserialises Json columns same as Record
  - `src/validate/mod.rs` — Json type-shape check (sentinel detection → error)
  - `stores/observations_1006/schema.yaml` — re-add the `notes` field
  - Tests at each layer (parser, coerce, write, read, validator, round-trip integration)
- **Out of Scope:**
  - Anything listed in `## Task` / "What's NOT in this task"
  - T009 observations port (separate task)

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
