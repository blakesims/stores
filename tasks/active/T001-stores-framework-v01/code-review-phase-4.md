# Code Review — Phase 4: Dynamic CLI codegen + add/show/list/update verbs

- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Commit under review:** `8beeb67d6f437286e6affb40ff524c078d62dc74` ("feat(T001 phase 4): dynamic CLI codegen + add/show/list/update verbs")
- **Verdict:** **REVISE (minor)** — one major data-loss bug + five minors. Fix is small (~10 LOC + a test).
- **Issues:** 0 critical / 1 major / 5 minor

---

## Verification matrix (10 ACs)

| # | AC | Verified? | How |
|---|----|-----------|-----|
| 1 | `add --<field> value` writes a row, returns rendered display_id | YES | `stores kitchen_sink add --title "first thing" --priority low ...` → `K001`, exit 0 |
| 2 | Flat leaf args (no `--<record>` JSON arg) | YES | `stores kitchen_sink add --help` shows `--notes` and `--severity` (Record sub-fields) as flat flags; no `--details` |
| 3 | `--<list> "a\|b\|c"` → JSON array | YES | `--tags "alpha\|beta\|gamma"` round-trips to `["alpha","beta","gamma"]` in DB and `--json` |
| 4 | `show <id>` text + `--json`; Records preserved as nested | YES | `--json` emits `"details": {"notes": "...", "severity": "..."}` (real nested object, not escaped string); `tags` is a real JSON array |
| 5 | `list` text + `--json`; JSON is array | YES | text shows one entry per line with scalar summary; `--json` emits a JSON array of nested objects |
| 6 | `--<field>-from-file path` and stdin via `-` | YES | both `update K001 --title-from-file /tmp/blob.txt` and `echo X \| update K001 --title-from-file -` populate the field |
| 7 | `update <id> --<field> value` mutates row, bumps `updated_*`, leaves `created_*` | YES | direct `sqlite3` query: `created_at` unchanged across two updates; `updated_at` advances on each |
| 8 | `add` writes `status = lifecycle.initial_state` (defaults to `states[0]`) | YES | unit test `add_sets_initial_status_to_first_state` + live `sqlite3 "select status from kitchen_sink"` returns `open` for every row |
| 9 | Reserved cols populated on `add`/`update` | YES | unit test `add_populates_created_and_updated_fields` + live verification |
| 10 | `stores --help` shows installed stores; `stores <store> --help` shows verbs | YES | both confirmed; also probed empty-manifest case (`stores --help` before any install lists only `init`/`install`/`help`) |

`cargo test`: 41/41 passing (matches executor's claim).

---

## Issues

### M1 (MAJOR) — Record sub-field update silently destroys sibling sub-fields

**Where:** `src/handlers/update.rs` lines 41–46.

```rust
// Merge diff into existing
let mut merged = existing.clone();
for (k, v) in &diff {
    merged.insert(k.clone(), v.clone());
}
```

**Symptom (reproduced live):**
```
$ stores kitchen_sink update K001 --notes "X" --severity Y
$ sqlite3 .stores/db.sqlite "select details from kitchen_sink where display_id='K001'"
{"notes":"X","severity":"Y"}                       # OK so far

$ stores kitchen_sink update K001 --severity warning
$ sqlite3 .stores/db.sqlite "select details from kitchen_sink where display_id='K001'"
{"severity":"warning"}                             # `notes` is GONE
```

**Root cause:** `build_entry_map(schema, get_arg)` constructs the diff by walking `leaf_args` and only emitting paths the user supplied. For a Record-typed field, the diff therefore contains a partial Object (only the sub-keys the user mentioned). The merge step then `insert`s that partial Object on top of `details` in the existing entry — replacing it wholesale instead of deep-merging sub-keys. The `update.rs` SQL writer then serialises the partial Object as the new JSON value of the `details` column.

**Why this is major, not critical:**
- It IS data-loss on every partial Record update by every user.
- It does NOT block the Phase 4 ACs (each AC tests a single round of partial update or a full-field update of a non-Record field).
- It DOES jeopardise DONE_WHEN #6 the moment Phase 6 lands, because the demo flow plus any subsequent refinement (`update L001 --done-when "revised"`) on the `contract` Record will silently lose `scope_in` / `scope_out`.
- The transition handler in Phase 6 will likely share `update`'s diff-merge code path; the fix here also fixes that.

**Fix sketch:** in `update.rs`, after computing `diff`, when a key in `diff` corresponds to a Record-typed field in the schema, merge into the existing Object instead of replacing it. ~10 LOC. Suggested test: a `handlers::update::tests::record_subfield_update_preserves_siblings` unit test that does `add → update --severity X → assert notes still present`.

**Note:** the bug is local to `update`, not `add`. `add` writes the full Record as serialized at insert time so the partial-write problem doesn't apply.

---

### m1 (MINOR) — Update silently coerces unparseable Integer to `0`

**Where:** `src/handlers/update.rs` lines 84–90.

```rust
FieldType::Integer => {
    let i = match new_val {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        _ => 0,                                     // <-- silent zero
    };
    sql_values.push(rusqlite::types::Value::Integer(i));
}
```

`coerce_value` falls through to `Value::String(raw)` when an integer can't be parsed; the update handler then sees `Value::String(...)` and writes `0`. The `add` handler at least preserves NULL in the analogous position. Reproduced: `update K004 --count "alsogarbage"` writes `0` to the `count` column.

Phase 5's validator should reject the unparseable value before this branch fires; deferrable but worth a comment in the meantime.

---

### m2 (MINOR) — Text formatter parent-line for Records has no separator

**Where:** `src/output.rs` `print_map_text`.

A Record key prints as `details:` with sub-fields indented two spaces below. Acceptable; mention if README starts asserting layout.

---

### m3 (MINOR) — ISO-8601 timestamp math duplicated

`install.rs::chrono_now` and `handlers/row.rs::now_iso8601` are byte-identical implementations. Phase 3's m4 already flagged this. Recommend extracting to a single `paths::now_iso8601()` or pulling in the `time` crate.

---

### m4 (MINOR) — `dispatch::detect_invoker` ignores `--invoker` flag

The function comment acknowledges this:
```rust
// --invoker override (not a clap arg yet; Phase 6+ adds it; read from env for now)
```

Ensure Phase 6 wires it before the `gate answer` actor-mismatch demo (DONE_WHEN #10/#11) runs.

---

### m5 (MINOR) — `coerce_value` List splitter has no escape

Cycle-2 m2c2 already noted this; deferrable. A literal `|` inside a value can't be escaped.

---

### Reserved-column-name collision (cycle-2 m1c2 carry)

A schema that declares a leaf named `status` / `id` / `display_id` / `created_at` etc. will fail at install time with SQLite's own `Error code 1: SQL error or missing database: duplicate column name: status`. Reproduced. So this is **not** silent corruption — it's caught — but the error is SQLite's, not ours, and the `is_reserved` list in `dynamic.rs` already knows the right answer. Recommend mirroring `is_reserved` into install-time check (3 LOC). Not gate-blocking.

---

## Forward-compat notes

### Phase 5 (enforcement engine)

- **Validator signature** `fn validate(&Schema, &EntryMap, Actor) -> Result<()>` is in place and stable.
- **EntryMap is genuinely nested.** Verified empirically by reading what Phase 4's reassembly produces from flat CLI args: `entry["details"]["severity"] = "warning"` shape, not `entry["severity"] = "warning"`. So Phase 5's `required_when` evaluator can resolve `triage.verdict == 'T3'` from anywhere in the tree.
- **Caveat:** the plan called for the validator to take `Op` (Add | Update | Transition(verb)) as well; the current stub elides this. Phase 5 should add `Op` at the same time it lands the body.
- **`Schema.default_actor` is still unused** (Phase 2 m3 carry-forward, also Phase 3 m3). Phase 5 should either consume it or drop it.

### Phase 6 (lifecycle transitions)

- `dynamic.rs::build_store_command` hardcodes `add`/`show`/`list`/`update`. Adding per-transition verbs is a clean extension — another loop iterating `schema.lifecycle.transitions` and calling `build_leaf_cmd(verb, &leaves, true)` (each transition needs `<display_id>` positional). No rework.
- The transition handler (Phase 6's `handlers/transition.rs`) will read existing → merge diff → validate → write — same shape as `update`. **It MUST inherit the M1 fix** or the contract Record will be partially overwritten on every transition that supplies a subset of Record sub-fields.

---

## Action items (REVISE)

1. **Fix M1.** In `src/handlers/update.rs`, when merging the diff into the existing entry, deep-merge Record-typed sub-fields rather than replacing the parent Object.
2. **Add a regression test.** `handlers::update::tests::record_subfield_update_preserves_siblings`: add a row with `details = {notes: "X", severity: "Y"}`; update with only `--severity Z`; assert post-update `notes` is still `"X"`.
3. **Recommit** as `feat(T001 phase 4 fix): preserve sibling sub-fields on partial Record update` (or similar).
4. **Re-review.** Re-run `cargo test` and the live partial-Record update reproduction.

The five minors are all deferrable — none blocks moving to Phase 5 once M1 is fixed. m1 (silent integer→0 coercion) and m4 (`--invoker` flag wiring) should be carried into Phase 5/6 explicitly.
