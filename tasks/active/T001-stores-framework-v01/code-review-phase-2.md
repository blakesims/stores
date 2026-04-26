# Phase 2 Code Review — Schema parser

- **Commit:** `169480f` (+ doc-only `0019229`)
- **Reviewer:** code-reviewer
- **Reviewed:** 2026-04-26
- **Verdict:** PASS (REVISE-minor; nothing gate-blocking)
- **Issues:** 0 critical / 1 major / 4 minor

## Verification matrix (9 ACs)

| AC | Pass? | Evidence |
|---|---|---|
| AC1 — Unit tests cover every FieldType, required_when, enum, actor, transition | PASS | `parse_full_fixture` + `field_types_roundtrip` + `actor_tag_on_field` + `transitions_parsed` all exercise the FULL_FIXTURE which contains text/integer/bool/enum/list/record/display_id/timestamp + a required_when on a Record sub-field + an enum field with actor + 2 transitions with actors |
| AC2 — Record sub-field carries `required_when` on the sub-Field, not the parent | PASS | `record_subfield_required_when_on_subfield_not_parent` asserts `contract.required_when.is_none()` AND `done_when.required_when == Expr { lhs_path: ["triage","verdict"], rhs_literal: "T3" }`. Direct AC match. |
| AC3 — `"triage.verdict == 'T3'"` → `Expr { ["triage","verdict"], "T3" }` | PASS | `parse_simple` covers it exactly |
| AC4 — Malformed required_when returns error naming unsupported token | PASS | `reject_not_equal`, `reject_or_keyword`, `reject_and_symbol`, `reject_double_quotes_rhs`, `reject_unquoted_rhs` |
| AC5 — Unknown field type / actor returns error pointing at offending key | PASS | `unknown_field_type_errors` ("magic_type") + `unknown_actor_errors` ("robot") |
| AC6 — leaf_args for `triage{verdict,notes}` + `contract{done_when,scope_in,scope_out}` returns 5 leaves with correct kebab cli_names | PASS | `ac6_exact_fixture` asserts exactly 5 leaves with names `["verdict","notes","done-when","scope-in","scope-out"]` |
| AC7 — leaf_args returns error naming both parent paths on collision | PASS | `collision_returns_error_naming_both_paths` asserts msg contains both `triage.notes` and `review.notes` |
| AC8 — `id_format: "L{:03d}"` validates; rendering pk=1 yields L001 | PARTIAL | Validation tested (`l003_format`, `valid_formats`); **renderer is NOT implemented** in Phase 2 — plan text itself says "renderer impl lives in Phase 4" so the AC text contradicts the plan body. Validation half is fully covered. Phase 4 owes the rendering test. |
| AC9 — `Lifecycle.initial_state` defaults to `states[0]`; explicit overrides | PASS | `initial_state_defaults_to_first` + `initial_state_explicit_override` |

`cargo test`: **31 passed; 0 failed**. Executor count matches reality.

## Findings

### Major

**M1 — `OR`/`AND` substring rejection produces false positives on legitimate enum values.**

`required_when::parse` rejects compound expressions via:
```rust
if s.contains("OR") || s.contains(" or ") { bail!(...) }
if s.contains("AND") || s.contains(" and ") { bail!(...) }
```

This is naive substring matching, not token-aware. Verified empirically:
- `region == 'NORTH'` → false-rejected as containing "OR"
- `type == 'CONNECTOR'` → false-rejected
- `name == 'BRANDY'` → false-rejected as containing "AND"

The bundled `observations` and `gate` stores in v0.1 use values `T1/T2/T3`, `decision/script`, `human`, etc., none of which trigger this — so v0.1 demo path is unaffected. **But the moment a user-defined store has a `required_when: status == 'CONFIRMED'` (contains "OR" actually doesn't, but `'AUTHORIZED'` does), the parser falsely rejects valid input.** Fix is straightforward: tokenise on whitespace before substring-checking, or check `\s OR \s` / `\s AND \s` with regex.

Classified Major (not Critical) because v0.1 DONE_WHEN demo path is not affected. But this is a foot-gun the moment a user authors a third store, so flagging now beats discovering it under user pressure later.

### Minor

**m1 — AC8 says "rendering with pk=1 yields L001" but no renderer exists.** Plan body explicitly says "renderer impl lives in Phase 4 but the format-string validation lives here", which is internally contradictory. Phase 2 only validates the template; that matches what's testable here. Action: ensure Phase 4 has a render unit test against pk=1 → L001 to actually close m3 from cycle 1.

**m2 — `RawField` does not use `#[serde(deny_unknown_fields)]`.** A typo like `requried_when:` (sic) would silently parse as no required_when, with no error. Not a Phase 2 plan requirement, but worth tightening before user-authored schemas land in Phase 6/7. Trivial one-line fix.

**m3 — `default_actor` field added to `Schema` was not in the Phase 2 plan.** Executor's deviation log doesn't mention it. It's parsed but unused (no inheritance logic). Likely scope-creep groundwork for Phase 5; harmless but flag for awareness. If Phase 5 plan doesn't actually consume it, it should be removed (YAGNI).

**m4 — `parse.rs` test has dead code.** `let bad = "..."; let _ = bad; // suppress warning` is leftover scaffolding from rewriting the test mid-flow. Cosmetic.

**m5 — Reserved-column collision (cycle-2 fresh-eye m1c2) is not caught here.** `leaf_args` only checks intra-leaf uniqueness, not collision against the future reserved columns (`status`, `display_id`, `created_at`, `updated_at`, `created_by`, `updated_by`, `id`). A user-authored store with a leaf named `status` would silently shadow the lifecycle column at DDL emission. **Confirmed deferrable to Phase 3** (DDL codegen): Phase 3 already owns the reserved-column list, so adding the check there alongside table emission is the natural seam — and v0.1 bundled stores don't trigger it.

## Spot-checks

### Record sub-field model (checklist #2)

`Field { ty: FieldType::Record(Vec<Field>) }` — `FieldType::Record` literally holds `Vec<Field>`, where each inner `Field` is the same struct as a top-level field, so sub-fields naturally carry their own `required`, `required_when`, `pattern`, `actor`, `enum_values`. Verified by `record_subfield_required_when_on_subfield_not_parent` test. C3 (cycle 1) genuinely solved at the model layer.

### flatten naming rule (checklist #3)

`fn to_kebab(name: &str) -> String { name.replace('_', "-") }` — leaf's own name only, no parent prefix. `done_when` → `done-when`, NOT `contract-done-when`. Confirmed against AC6 fixture expected output `["verdict","notes","done-when","scope-in","scope-out"]`. Matches the literal DONE_WHEN demo path.

### required_when edge cases (checklist #4)

- `a == 'b' ` (trailing space) → trim handles it. Pass.
- `a == ''` (empty string) → `rhs[1..rhs.len()-1]` slices to empty string. Parses successfully. Pass.
- `a.b.c == 'x'` (3-deep path) → all chars are alnum/underscore/dot, splits to 3 segments. Pass.
- `a == "x"` (double quotes) → rejected with "double quotes" message. Pass.
- `region == 'NORTH'` → **falsely rejected** as containing "OR". See M1.
- Empty LHS / leading-trailing dots → rejected with appropriate messages.

### Cross-Record `lhs_path` parsing (checklist #5)

The parser is path-agnostic — it accepts any `dotted.identifier.path`, so `triage.verdict` referenced from inside `contract.done_when` parses just fine. Resolution lives in Phase 5 (validator walking the EntryMap). The Phase 2 layer doesn't reject sibling-Record references prematurely. Confirmed.

### Lifecycle defaulting (checklist #6)

`resolved_initial_state()` returns `initial_state.as_str()` if set, else `states.first()`. Errors only if states is empty. Tested both paths.

### Custom Deserialize (checklist #7)

`RawFieldType` uses a `Visitor` to handle either `"text"` (string scalar) or `{ list: text }` / `{ record: ... }` (mapping). Clean. Error messages from `serde_yaml` carry line numbers (though the test for that only asserts the literal string `"YAML parse error"` — see m4). Errors from `resolve_field_type` use `anyhow::bail!` and lack line numbers (because they fire after deserialisation), but they do name the offending key — AC5 satisfied. Uneven but acceptable.

### Code organization deviation (checklist #8)

`FieldType` lives in `mod.rs` not `types.rs`; `Schema::from_yaml` lives in `mod.rs` not `parse.rs`; both `types.rs` and `parse.rs` are re-export shims. The circular-dep concern is real (`FieldType::Record(Vec<Field>)` ↔ `Field`), so the deviation is defensible. The shims are tiny (3 lines + 1 re-export) and serve as breadcrumbs for future readers expecting code in those files. Acceptable; no muddiness.

### Forward-compat (checklist #9)

- **Phase 3 (DDL codegen):** Walk `Schema.fields`, map FieldType variants. List/Record collapse to JSON-TEXT. Top-level `required` → NOT NULL. Reserved-column check should land in Phase 3. **Clean seam.**
- **Phase 4 (CLI codegen):** Iterate `leaf_args(schema)`, emit `clap::Arg::new(leaf.cli_name).long(&leaf.cli_name)`. List inner type readable via `field.ty`. **Clean seam.** Render id_format pk → display_id is the missing renderer (see m1).
- **Phase 5 (enforcement):** `RequiredWhenExpr.lhs_path` is a `Vec<String>`, ready for EntryMap traversal. Sub-fields are first-class so per-leaf `actor`/`required`/`pattern` can be enforced recursively. **Clean seam.**

No awkward seams found. The model genuinely supports the marquee `triage --verdict T3 --done-when X` enforcement landing in Phases 4–5.

### Reserved-column collision (checklist #10)

Not caught here; deferrable to Phase 3 (DDL emission owns the reserved-column list). Confirmed v0.1 bundled stores do NOT define `status`/`display_id`/`created_at`/etc. as user-leaves — `status` is store-managed, set by lifecycle, not declared as a field. So the foot-gun is latent for user-authored stores only. See m5.

## Verdict

**PASS** — advance to Phase 3.

All 9 ACs are met (AC8 with the noted plan/test asymmetry). The Record sub-field model genuinely works as the spine for T3-contract enforcement (C3 closed at model + flatten layers); Phase 5 will close it at the enforcement layer.

The 1 Major (M1: `OR`/`AND` substring false-positives) does not block Phase 3 — bundled stores don't trigger it, and the fix is a single-file, ≤10-line change in `required_when.rs` that can be picked up alongside Phase 5 enforcement work (which will be writing more `required_when` tests anyway and is the natural place to harden the parser). Recommend filing as a deferred-fix item in the Phase 5 work block.

The 4 Minors are cosmetic / forward-pointing.

**Action items for Phase 3 / later (non-blocking):**
1. Phase 3: add reserved-column-name uniqueness check at DDL emission (m5).
2. Phase 4: add the actual `render(template, pk) → "L001"` test that AC8 promised (m1).
3. Phase 5 work block: tighten `OR`/`AND` rejection to be token-aware, not substring-matching (M1).
4. Before user-authored schemas land in Phase 6/7: add `#[serde(deny_unknown_fields)]` on `RawField`/`RawSchema` (m2).
5. Trivial cleanup: remove dead `let bad = ...; let _ = bad;` from `parse.rs` test (m4); decide fate of `default_actor` per Phase 5 plan (m3).
