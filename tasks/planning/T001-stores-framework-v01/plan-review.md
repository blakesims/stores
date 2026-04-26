# Plan Review — T001 Stores Framework v0.1

**Reviewer:** `plan-reviewer`
**Date:** 2026-04-26
**Gate:** `NEEDS_WORK` (cycle 1 of 3)

---

## Verdict

The plan is structurally sound: 8 phases in the right order, every locked decision honored, every numbered DONE_WHEN item nominally mapped to at least one phase AC. But there are three places where a technically-correct executor could ship code that passes every phase AC while failing the literal 13-step demo, plus a handful of smaller gaps where implementation grammar is left to guesswork. None of the findings require user input — they are all addressable by the planner inside the Decision Matrix and by tightening a few ACs.

---

## Critical findings

### C1. The `sqlite3` JOIN demo (DONE_WHEN #12) returns zero rows as written

**Where:** Phase 8 AC #2; DONE_WHEN steps 9 + 12.

**Problem:** The literal demo script does this:

- step 4: `stores observations add ...` → produces `L001`
- step 9: `stores gate add --task-ref T042` → produces `G001` with `task_ref="T042"`
- step 12: `select ... from observations o left join gate g on g.task_ref = o.display_id` → joins `g.task_ref` against `o.display_id`. The only observation has `display_id="L001"`. The only gate row has `task_ref="T042"`. **The JOIN produces zero matched rows.** A `LEFT JOIN` returns the observation row with NULL gate columns — which is technically "rows" but does not "demonstrate the JOIN works" in any meaningful sense. The Phase 8 AC ("returns at least one row joining an observation with a gate record") is even stricter and would fail.

**Fix options for the planner:**
1. Change DONE_WHEN #9's `--task-ref` to `L001` so the JOIN matches. (Cleanest; preserves the 13-step path verbatim.)
2. Add a 9b step: a second `gate add --task-ref L001 ...` so a real match exists. (Keeps T042 as a "dangling ref" demo of cross-store-by-convention.)
3. Re-state Phase 8 AC #2 to require an INNER JOIN that returns ≥1 row, and have the e2e script seed a matching gate row outside the README's demo path (worst option — README-vs-test divergence).

Pick one and update **both** the DONE_WHEN list (in `## Task` — coordinate with orchestrator) and Phase 8 AC.

**Severity:** Critical — this is the marquee multi-store-coexistence demo and it's broken on paper.

---

### C2. Record-typed fields must expose sub-fields as flat CLI args — not stated anywhere

**Where:** Phases 2, 4, 6.

**Problem:** The demo invokes `--verdict T3` and `--done-when "..." --scope-in "..." --scope-out "..."`. But `verdict` is a sub-field of the `triage` Record; `done_when`, `scope_in`, `scope_out` are sub-fields of the `contract` Record. Phase 2's `Field` enum lists `Record(Vec<Field>)` but says nothing about how Record sub-fields surface to clap. Phase 4 says one `clap::Arg::new(field.name)` per **schema field** — that produces `--triage` and `--contract`, not `--verdict`/`--done-when`.

**This is the single most likely place an executor will build something that doesn't match the demo.** Two reasonable readings:

- **(a) Flatten Record sub-fields to top-level args.** `--verdict`, `--done-when`, etc. Risk: name collisions across Records. Resolution: namespacing or a stated "sub-field names are unique within a store" rule.
- **(b) Dotted args.** `--triage.verdict`, `--contract.done-when`. Risk: contradicts the literal demo path verbatim.

The locked decisions don't cover this. It belongs in the Decision Matrix with an explicit choice (almost certainly (a) flatten + assert uniqueness) and Phase 4's AC must include "Record sub-field names appear as top-level `--<name>` args".

**Severity:** Critical — without this, the executor either guesses or builds (b) and the demo fails.

---

### C3. `required_when` declared on Record sub-fields — Phase 2 model doesn't support it

**Where:** Phases 2, 5, 6.

**Problem:** Phase 6's observations schema says `contract` is a Record with `required_when: triage.verdict == 'T3'` "applied to each of the three sub-fields". Phase 2's `Field { name, ty, required, required_when, pattern, actor, ... }` is one struct per field — but for the `contract` field whose `ty = Record(Vec<Field>)`, `required_when` could mean either:

- "the whole contract Record is required when triage.verdict == T3" — implies all sub-fields with `required: true` then become required, OR
- "each named sub-field of contract carries this `required_when` independently".

The plan's wording ("applied to each of the three sub-fields") implies the second, but Phase 2's model doesn't show whether the YAML grammar lets you write `required_when` on each leaf inside a Record, or whether it propagates from the parent. This is the **load-bearing rule** for DONE_WHEN #5.

**Fix:** Phase 2 must explicitly state: Record sub-fields are themselves `Field` structs and may carry their own `required_when`. The YAML grammar example in Phase 2's unit test should include a Record with a sub-field bearing `required_when`. Phase 5's required_when unit test must include a Record-sub-field case.

**Severity:** Critical — the entire T3-contract enforcement (the motivating story) hangs on this.

---

## Major findings

### M1. List-typed CLI argument parsing convention is unspecified

**Where:** Phase 4; DONE_WHEN #9.

`stores gate add ... --options "soft|hard"` passes a single string for a `list<text>` field. Phase 4 says nothing about how a `List(_)` field receives values: `|`-separated? Comma-separated? Repeated `--options soft --options hard`? Each is reasonable; none is in the plan.

**Fix:** Add a Decision Matrix row + a Phase 4 AC: `--<list-field>` accepts `|`-separated input (matches DONE_WHEN literally) and the parser splits on `|` for `List(Text)` fields.

---

### M2. `show`/`list` handlers must round-trip JSON sub-columns back to nested structure

**Where:** Phase 4 (handlers), Phase 6 (DONE_WHEN #7).

Records and Lists are stored as JSON in TEXT columns (per Decision Matrix). DONE_WHEN #7 says `show L001` "prints the entry, contract embedded" — meaning the nested `contract.{done_when, scope_in, scope_out}` shape comes back. The `show.rs` handler must deserialize JSON columns back to typed values for printing (and especially for `--json` output to be valid). Phase 4 doesn't mention this round-trip.

**Fix:** Phase 4 AC: "for fields whose `ty` is `Record` or `List`, the column value is parsed as JSON before formatting. `--json` output preserves nesting." Phase 6 AC: "show output for `L001` includes `triage.verdict='T3'` and the three contract sub-fields under their parent keys."

---

### M3. Initial `status` value on `add` is not specified

**Where:** Phase 4 (`add` handler), Phase 6 (lifecycle).

The reserved column `status TEXT NOT NULL` must get a value on `add`. Where does it come from? Reasonable convention: the first state in `lifecycle.states`. This is fine but unstated, and the schema YAML for observations doesn't have an explicit `initial_state` field.

**Fix:** Either (a) Phase 2 schema model adds `Lifecycle.initial_state: Option<String>` defaulting to `states[0]` with an explicit AC; or (b) state in Phase 4 AC: "`add` writes `status = lifecycle.states[0]`" and assert it.

---

### M4. `update` verb is shipped with no AC exercising it

**Where:** Phase 4.

The plan generates `add`, `show`, `list`, `update` per store. None of the 13 DONE_WHEN items uses `update`, and Phase 4 has no AC that calls it. So either it ships untested, or it ships unfinished. Two options:

- Drop `update` from v0.1 scope (it's not in DONE_WHEN; transitions handle real status changes).
- Keep it but add a Phase 4 AC: "`stores <store> update <display_id> --<field> value` mutates the row through the validator."

---

### M5. Risk register underweights three real risks

The plan lists 5 assumptions/risks; missing from the risk discussion (not just the matrix):

- **Dynamic clap construction surface area.** Building `Command` trees at runtime from a typed schema model is well-supported but every runtime code path bypasses derive macros' compile-time checks. Argument-name collisions across stores or across Record sub-fields are runtime-discoverable only.
- **JSON-vs-column tradeoff for nested types** (already chosen but not risk-flagged): `where contract.scope_in like '%backend%'` requires `json_extract` everywhere; future structured queries get awkward. This is a known cost; flag it as accepted technical debt.
- **`required_when` evaluator straddles typed in-memory entries and the JSON-nested storage form.** The validator runs on the in-memory entry **before** JSON serialization (correct), but the dotted-path lookup must therefore traverse the typed entry-map representation, not the SQLite row. Phase 5's "walks nested Record and JSON-as-Value structures" wording conflates the two; clarify it walks the in-memory `EntryMap`.

**Fix:** Add a paragraph or three bullets to the Risks section.

---

## Minor findings

### m1. Re-running `cargo install --path .` is not in any AC
DONE_WHEN starts with it; trivial to assume it works, but worth a one-line Phase 1 note that subsequent `cargo install --path .` invocations replace the binary cleanly.

### m2. `created_at` / `updated_at` / `created_by` / `updated_by` reserved columns are listed but never explicitly populated
Phase 4 handlers should set `created_at = updated_at = now()` and `created_by = invoker.to_string()`. Add to Phase 4 AC.

### m3. `id_format` template parsing is not unit-tested in Phase 2
Phase 2 lists `Schema { id_format }` but no AC parses or renders it. Phase 4 mentions rendering. Add a small Phase 2 AC: "`id_format: \"L{:03d}\"` parses; rendering with `pk=1` yields `L001`."

### m4. `stores install` second-time idempotency on a *different* store path with the same `name:` is undefined
The Decision Matrix covers re-installing the same store. What about installing a different folder whose `schema.yaml` declares the same `name` as an installed one? Probably the same "reject" error. State it.

---

## Cross-cutting concerns

1. **The Record/sub-field treatment is the spine of the whole framework** (CLI args, validator's required_when, JSON storage round-trip, `show` output). Critical findings C2 + C3 + major M2 are all aspects of the same design choice. Fixing them coherently in one Decision Matrix entry — "Record sub-field treatment" — is probably cleaner than three separate edits.

2. **README-as-test integrity.** Phase 8 promises "copy-paste from README into a fresh shell reproduces e2e success." Critical finding C1 means the README and the e2e script either diverge or the JOIN demo is hollow. This deserves an explicit Phase 8 AC: "the e2e script is a literal copy of the README's command list — no extra setup steps."

3. **Phase 3 fixture stores.** Phase 3 ACs use "tests/fixtures/" stores because the real ones ship in P6/P7. The fixtures must exercise enough type variety (Record, List, Enum, required_when) to prove DDL codegen end-to-end. The plan implies but doesn't promise a fixture covers all field types. Consider adding to Phase 3 AC.

---

## Recommended Revise Feedback (numbered, addressable)

1. **Resolve the JOIN-returns-zero-rows problem (C1).** Pick one of the three fix options; update DONE_WHEN #9 and Phase 8 AC #2 in lockstep.
2. **Add a Decision Matrix entry "Record sub-field treatment" (C2 + C3 + M2).** State: (a) Record sub-fields are flat top-level CLI args; (b) sub-field names are asserted unique within a store at install-time; (c) `Field` instances inside `Record(Vec<Field>)` may carry their own `required_when`/`pattern`/`actor`; (d) `show` and `--json` output reconstruct the nested Record shape from the JSON-stored column.
3. **Add a Decision Matrix entry "List CLI input format" (M1).** Choose `|`-separated for `list<text>` to match DONE_WHEN #9 literally; add Phase 4 AC.
4. **Specify initial-status convention (M3).** Either add `Lifecycle.initial_state` to Phase 2 with default `states[0]`, or add a Phase 4 AC asserting `status` defaults to `lifecycle.states[0]` on `add`.
5. **Decide `update` verb's fate (M4).** Drop or add an AC.
6. **Tighten Phase 4 AC for reserved-column population (m2).** `created_at`, `updated_at`, `created_by`, `updated_by` written on every insert/update.
7. **Tighten Phase 5 AC to include a `required_when` unit test against a Record-sub-field LHS path** to lock in C3's fix.
8. **Add Phase 2 AC for `id_format` round-trip (m3).**
9. **Add Phase 3 AC: re-install with different path but duplicate `name` is rejected (m4).**
10. **Expand Risks section with the three risks named in M5.**
11. **Phase 8 AC: e2e script is a literal copy of README's command list — no extra setup.** This pins README-test integrity.

---

## Final verdict

`NEEDS_WORK` — back to the planner for one revision pass. The structural plan is good; the gaps are concrete and tightly scoped. After cycle 1's edits, expect a quick second pass to confirm. If a fix to C1 changes the user-facing demo path (option 1 changes `--task-ref T042` to `L001`), the orchestrator should also update the `## Task` section's DONE_WHEN list to keep main.md and DONE_WHEN aligned.

---

# Cycle 2 Review

**Reviewer:** `plan-reviewer`
**Date:** 2026-04-26
**Gate:** `READY` (cycle 2 of 3)

---

## Verdict

The planner addressed all 11 numbered cycle-1 items with real edits to the Plan body, not just claims in the change-log. The most load-bearing new work — `src/schema/flatten.rs` and the Record-sub-field-as-first-class-Field model — is coherently designed and threads cleanly through Phases 2 → 4 → 5 → 6. Fresh-eye pass on the revised Plan turned up only minor edge cases that do not block executor work. Advance to executor.

---

## Per-item verification (cycle 1 → cycle 2)

For each cycle-1 item, verified against the actual revised Plan body (not just the change-log claim):

### Item 1 — JOIN-zero-rows (C1) — PASS
- `## Task` line 34: DONE_WHEN #9 uses `--task-ref L001`. Surrounding parenthetical explains why (so the JOIN in #12 returns matched rows).
- Phase 7 AC #2 (line 239): uses `--task-ref L001`.
- Phase 8 AC #2 (line 251): explicitly requires "≥1 row matching `L001` with non-NULL gate `display_id` (`G001`) — i.e. a real JOIN match exists, not just a LEFT-JOIN row with NULL gate columns".
- Phase 8 AC #3 (line 252): pins README↔script literal correspondence.
- The hollow-LEFT-JOIN gotcha that motivated this item is explicitly forbidden.

### Item 2 — Record sub-field treatment (C2 + C3 + M2) — PASS
The single most important cycle-1 item, addressed cohesively across model, codegen, validator, storage round-trip:
- Decision Matrix new row (line 262) explicitly chooses (a) flatten + first-class sub-fields + nested round-trip; states the kebab-case naming rule (parent NOT prefixed) and the within-store uniqueness rule.
- Phase 2 model (line 143): "when `ty == Record(Vec<Field>)`, **each inner `Field` is a full `Field` struct** that may carry its own `required`, `required_when`, `pattern`, and `actor`".
- Phase 2 new file `src/schema/flatten.rs` (line 149): `leaf_args` walks Records, returns `LeafArg { cli_name, path, field }`, asserts uniqueness at install-time.
- Phase 2 ACs (lines 153, 157, 158): YAML fixture exercises sub-field `required_when`; `leaf_args` returns the expected 5 leaves; collision detection error names both parent paths.
- Phase 4 (line 181): clap codegen iterates `leaf_args` to emit `--<cli_name>` per leaf.
- Phase 4 reassembly (line 182): "as args are read off `ArgMatches`, leaf values are nested back into their parent Record paths to build the in-memory `EntryMap`".
- Phase 4 ACs (lines 188, 190): flat flag emission verified; round-trip on `show`/`list` verified.
- The model + codegen + reassembly + validator + read-side round-trip form a single cohesive design. The flat CLI surface and the nested EntryMap are explicitly bridged by Phase 4's reassembly step — without that bridge, Phase 5's cross-Record `required_when` evaluator would fail. The bridge exists.

### Item 3 — List CLI input format (M1) — PASS
- Decision Matrix row (line 263) chooses `|`-separated.
- Phase 4 AC line 189: `--options "soft|hard"` deserializes to `["soft","hard"]`.

### Item 4 — Initial-status convention (M3) — PASS
- Phase 2 (line 147): `Lifecycle.initial_state: Option<String>`, defaults to `states[0]`.
- Phase 2 AC line 160: explicit + default both verified.
- Phase 4 handler text (line 183): `add` writes `status = lifecycle.initial_state`.
- Phase 4 AC line 194 + Phase 6 AC line 225: end-to-end check that `add` produces `status='open'`.

### Item 5 — `update` verb fate (M4) — PASS
- Kept (Decision Matrix line 265).
- Phase 4 AC line 193: `update <display_id> --<field> value` mutates row through validator + bumps `updated_*`.

### Item 6 — Reserved-column population (m2) — PASS
- Phase 4 handler text (line 183): `add` populates all four reserved columns; `update` only touches `updated_*`.
- Phase 4 AC line 195: explicit verification.

### Item 7 — Phase 5 cross-Record `required_when` test (C3 enforcement) — PASS
- Phase 5 AC line 209: dedicated unit test where `contract.done_when` carries `required_when: triage.verdict == 'T3'`; asserts the rule fires when `triage.verdict='T3'` and is silent otherwise.
- Phase 5 mod text (line 200): validator runs on in-memory typed `EntryMap` (nested), with dotted-path lookup traversing typed entry-map representation including ascent into sibling Records — explicitly clarified per cycle-1 M5 third bullet.
- Phase 5 `required.rs` text (line 203) explicitly states the cross-Record case is supported.

### Item 8 — `id_format` round-trip (m3) — PASS
- Phase 2 AC line 159: `"L{:03d}"` parses; rendering with `pk=1` yields `L001`. (Renderer impl in Phase 4, format-string validation in Phase 2 — clean split.)

### Item 9 — Same-name-different-path rejection (m4) — PASS
- Phase 3 AC line 174: explicit rejection with name-collision error in the same error class.
- Decision Matrix Re-install row (line 269) updated to cover both same-path and same-name-different-path.

### Item 10 — Risks expansion (M5) — PASS
- Risks/Assumptions section (lines 276–282) expanded with three named risks:
  - Dynamic clap construction surface area + leaf_args uniqueness as backstop.
  - JSON-in-TEXT vs structured columns as accepted technical debt for v0.1.
  - `required_when` evaluator on `EntryMap` not stored row, including update re-validation flow.

### Item 11 — README-as-test pinning — PASS
- Phase 8 AC line 252: `tests/e2e.sh` is a literal copy of README's numbered command list; top-of-file correspondence comment for auditability.

### Adjacent: m1 (cargo install replace) — PASS
- Phase 1 AC line 135.

### Adjacent: cross-cutting #3 (all-types fixture) — PASS
- Phase 3 `tests/fixtures/all_types_store/schema.yaml` (line 169) with snapshot AC covering every `FieldType` variant.

---

## Fresh-eye check (independent, post-revision)

Read the revised Plan as if for the first time, looking for new gaps the revisions might have introduced:

### Cohesion check on the Record-sub-field design

The new design has many moving parts (flatten.rs at parse time, dynamic clap codegen at startup, reassembly at dispatch time, nested-EntryMap walk at validate time, JSON-column round-trip at read time). I traced the data flow for the marquee case — `stores observations triage L001 --verdict T3 --done-when X --scope-in Y --scope-out Z`:

1. Parse: `flatten.rs` produces leaves `verdict, notes, done-when, scope-in, scope-out` — no collisions (`notes` is the only Record sub-field repeated in observations: it appears in `triage` only).
2. CLI: `--verdict`, `--done-when`, etc. emitted as flat flags.
3. Reassemble: dispatch builds `EntryMap` with `triage.verdict='T3'`, `contract.done_when='X'`, `contract.scope_in='Y'`, `contract.scope_out='Z'`.
4. Validate: walks the nested EntryMap. `contract.done_when`'s `required_when` evaluates `triage.verdict == 'T3'` by descending into the `triage` sibling Record. Result: rule satisfied; entry valid.
5. Write: Records and Lists serialize to JSON-in-TEXT.
6. Read (`show`): Record/List columns deserialize back to nested form for `--json`.

The chain holds. The two reasonably-tricky steps (3 and 4) are explicitly called out in Phase 4 line 182 and Phase 5 line 203.

### Edge cases that are NOT addressed (deferrable nits)

These are all minor; none gate the executor:

#### m1c2 (cycle-2 minor 1) — Reserved-column-name collision with leaf names
`leaf_args` checks uniqueness across leaves (Phase 2 AC line 158), but doesn't check against reserved columns (`id`, `display_id`, `status`, `created_at`, `updated_at`, `created_by`, `updated_by`). A schema with a top-level field or Record sub-field named `status` would silently shadow the reserved column at the CLI surface even though the DDL would reject it later. Trivial fix the executor can add to `flatten.rs`'s uniqueness check; not gating.

#### m2c2 — `|`-character escape in List<Text> values
The `|`-split parser has no escape mechanism. A legitimate `--options "a|b|with literal | inside"` parses as 4 elements. v0.1-acceptable; document if it bites.

#### m3c2 — `update` doesn't explicitly forbid status mutation
Phase 4 AC for `update` says "mutates the row through the validator and bumps `updated_at`/`updated_by`" — doesn't say `update` rejects `--status` (status changes should go through transitions). Likely covered implicitly because `status` isn't in `leaf_args` (it's a reserved column, not a schema field), so there'd be no `--status` flag emitted. But not explicit. Minor.

### What I checked and found OK

- The `--<cli-name>-from-file` companion is gated to `Text` leaves (Phase 4 line 181) — consistent with Record-sub-field-text being a Text leaf.
- The `id_format` validation/render split (Phase 2 parses, Phase 4 renders) is clean.
- Phase 3's all-types fixture is created early enough to be reused by Phase 5 unit tests (line 169) — good separation from the real bundled stores in P6/P7.
- Phase 8 AC #4 explicitly ties off M2 by re-checking `--json` output for nested Records on `show` and `list`.
- The `update` keep-vs-drop decision (Decision Matrix line 265) explicitly justifies the cost-benefit.
- Iteration limit math: this is cycle 2 of 3 NEEDS_WORK budget; if I had returned NEEDS_WORK here, one more cycle would remain. READY ends the budget consumption.

---

## Final verdict (cycle 2)

`READY` — advance to executor.

All 11 cycle-1 items are genuinely addressed in the Plan body. The load-bearing redesign (Record sub-fields as first-class Fields, flat CLI flags via `flatten.rs`, reassembly-into-EntryMap, cross-Record `required_when` evaluation) is coherent end-to-end. Fresh-eye pass found three minor nits, all deferrable and addressable inside executor judgement. The plan is implementable as written.

Status flipped to `READY` in `## Meta`. Orchestrator should `git mv tasks/planning/T001-stores-framework-v01 tasks/active/` and spawn the executor for Phase 1.
