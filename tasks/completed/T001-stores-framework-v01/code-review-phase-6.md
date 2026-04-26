# Phase 6 Code Review — Lifecycle transitions + bundled `observations` store

- **Gate:** PASS
- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Commit:** `400ab8b`
- **Issues:** 0 critical / 0 major / 4 minor
- **Test count claimed:** 83/83 — verified, all pass

## Summary

All 10 Phase 6 ACs verified end-to-end against the bundled `observations` store
in a fresh tmp dir. The marquee DONE_WHEN flow — `triage L001 --verdict T3
--done-when X --scope-in Y --scope-out Z` — works exactly as specified and is
the load-bearing acceptance: Phase 6 closes DONE_WHEN #2, #4, #5, #6, #7, #8.

## Per-AC verification (live, fresh tmp dir)

- **AC1 (DONE_WHEN #2):** `stores install ./stores/observations` registers the
  store; `sqlite3 .stores/db.sqlite ".schema observations"` shows the expected
  layout — reserved cols (`id`, `display_id`, `status`, `created_at`,
  `updated_at`, `created_by`, `updated_by`) followed by user fields (`summary`,
  `body`, `triage`, `contract`, `tags`). PASS.
- **AC2 (DONE_WHEN #4):** `stores observations add --summary "thing broke"` →
  prints `L001`; row written with `status='open'` (lifecycle.initial_state
  defaults to `states[0]`). PASS.
- **AC3 (DONE_WHEN #5):** `stores observations triage L001 --verdict T3` (no
  contract) → exits 1 with three errors aggregated in one pass:
  ```
  - contract.done_when: required (because triage.verdict == 'T3')
  - contract.scope_in: required (because triage.verdict == 'T3')
  - contract.scope_out: required (because triage.verdict == 'T3')
  ```
  All three contract sub-fields cited; `required_when` rule named in each. PASS.
- **AC4 (DONE_WHEN #6):** `stores observations triage L001 --verdict T3
  --done-when "X works" --scope-in "Y" --scope-out "Z"` → prints
  `Transitioned L001: open → triaged`; status moves to `triaged`. PASS.
- **AC5 (DONE_WHEN #7):** `stores observations show L001` text mode renders
  Records nested under their parent keys (`triage:` / `contract:` blocks with
  sub-keys indented). `--json` output validates with `python3 -m json.tool`
  and contains `triage: {verdict: "T3"}` and `contract: {done_when: "X
  works", scope_in: "Y", scope_out: "Z"}` as **real nested objects, not
  escaped strings** — M2 from cycle 1 ties off here. PASS.
- **AC6 (DONE_WHEN #8):** `stores observations list` shows the row. `--json`
  list emits a JSON array with the same nested Records. Note: text-mode list
  shows scalar fields only on a single line (Records skipped) — this matches
  the existing `print_list_text` design (see Phase 4). Acceptable. PASS.
- **AC7 (state-machine reject):** Re-triage after `triaged` is rejected with:
  `Error: cannot triage: row is in state 'triaged', expected 'open'`.
  Error message includes both states. PASS.
- **AC8 (resolve transition, ai_autonomous):** `CLAUDECODE=1 stores
  observations resolve L001` succeeds; status moves to `resolved`. The
  transition's actor is `ai_autonomous` and CLAUDECODE auto-detects to
  AiAutonomous, satisfying the actor check. Without CLAUDECODE (i.e. invoker
  resolves to Human), the same command is rejected with: `transition
  'resolve' requires actor 'ai_autonomous'; invoker is 'human'` — actor
  enforcement on transitions works. PASS.
- **AC9 (test count):** `cargo test` 83/83 pass — matches executor's claim.
  Four new tests in `handlers::transition::tests`:
  `triage_t3_without_contract_fails`, `triage_t3_with_contract_succeeds`,
  `state_machine_rejects_wrong_from_state`,
  `resolve_transition_from_triaged_succeeds`. PASS.
- **AC10 (CLI surface):** `stores observations --help` lists 7 verbs:
  `add show list update triage resolve wont_fix` (4 base + 3 transitions).
  Each transition takes positional `<display_id>` and the same flat leaf
  args as `add`/`update`. PASS.

## Code quality notes (transition handler)

- **State-machine check (`transition.rs:34-45`):** Reads `current_status`
  from the existing row, compares against `transition.from`. On mismatch,
  bails with `"cannot {verb}: row is in state '{current}', expected
  '{from}'"`. Both states named in the error — matches the AC requirement.
- **Deep-merge diff (`transition.rs:67-86`):** Identical pattern to
  `update.rs:42-62` (the Phase 4 cycle-2 fix). For Record-typed fields,
  existing sub-keys are preserved and diff sub-keys overlaid. Type-mismatch
  guard `(Some(Value::Object(_)), Value::Object(_))` falls through safely
  to wholesale insert if either side isn't an Object — no panic surface.
- **Validator runs before write (`transition.rs:89-91`):** Validation
  failure short-circuits via `?` — no DB write happens, no manual rollback
  needed. Validator gets the merged entry, so cross-Record `required_when`
  paths resolve correctly through the typed nested EntryMap.
- **Single-statement atomicity (`transition.rs:97-169`):** The UPDATE writes
  diff fields + `status = transition.to` + `updated_at` + `updated_by` in
  one SQL statement. SQLite makes single statements implicitly atomic; no
  explicit `BEGIN/COMMIT` needed for a single UPDATE.
- **SQL writer for Records (`transition.rs:115-121`):** Serializes the
  *merged* value (not the partial diff), inheriting the Phase 4 M1 fix
  pattern correctly.

## Schema correctness (`stores/observations/schema.yaml`)

- All fields present: `summary`, `body`, `triage{verdict, notes}`,
  `contract{done_when, scope_in, scope_out}`, `tags`.
- `required_when: "triage.verdict == 'T3'"` declared on each of the three
  contract **sub-fields** (lines 49, 53, 57) — NOT on the contract Record
  level. Matches the C3 model.
- Lifecycle: 4 states (`open`, `triaged`, `resolved`, `wont_fix`), 3
  transitions each with declared actor (`ai_with_human`, `ai_autonomous`,
  `ai_with_human`). `initial_state` omitted, defaults to `open` (states[0]).
- `id_format: "L{:03d}"` — verified to render `L001` for pk=1.
- `default_actor: ai_with_human` — consumed by Phase 5's `effective_actor`,
  used as fallback for fields without explicit actor (allows Human to call
  `add` even though no field-level actor is declared).
- Schema YAML parses cleanly via the Phase 2 parser; no executor-side
  adaptation diverged from the parser's input format.

## Forward-compat for Phase 7 (gate store)

Phase 7 ships `gate/schema.yaml` with `actor: human` on the `answer` field
and on the `answer` transition. Phase 6's changes don't touch the actor
enforcement engine — `validate::actor::check_actor` and
`check_transition_actor` are untouched (last modified in Phase 5). The
existing `probe_store` test in Phase 5 already exercised `actor: human`
field-level enforcement; Phase 7 just needs to ship the schema.
**No regressions from Phase 6 found in the actor enforcement path.**

The dynamic CLI codegen now generates per-transition verbs uniformly, so
`gate answer G001 --answer hard --invoker human` and
`gate cancel G001 --invoker ai_autonomous` will work for free in Phase 7
once the `gate` schema is dropped in. Multi-store coexistence verified by
installing both `observations` and `kitchen_sink` into the same DB; the
table layout and `manifest.yaml` accommodate both cleanly.

## Issues — 4 Minors

**(m1) Base-verb collision detection is parse-time, not install-time.**
`dynamic.rs:75` warns to stderr and skips a colliding transition verb when
`build_root` runs at parse-time. A user schema that declared `verb: update`
or `verb: add` would silently lose the transition (only a stderr warning,
no install-time rejection). v0.1 bundled stores (`triage`/`resolve`/
`wont_fix`) don't trigger this. Recommend mirroring this check into Phase
3's install validation in a future phase. **Not gate-blocking; deferrable.**

**(m2) No de-duplication across transition verbs themselves.** `dynamic.rs:
72-84` iterates `lifecycle.transitions` and calls `subcommand()` for each
verb without checking for duplicates among transitions. If a schema
accidentally declared two transitions with the same verb (e.g. two `from`
states both invoking `triage`), clap would silently overwrite one. The
bundled `observations` schema doesn't trigger this. **Not gate-blocking;
deferrable to install-time validation alongside m1.**

**(m3) `--invoker bogus` falls through silently to env detection.**
`dispatch.rs:67-74` `match` arm with `_ => {}` swallows unknown invoker
values and falls through to `$CLAUDECODE` detection. A user typo like
`--invoker hman` would be silently downgraded. Should error out. Phase 5
m1c2 carried; not introduced in Phase 6. **Deferrable.**

**(m4) Reserved-column-name install-time check (Phase 2 m5 / 3 m2 / 4 m5 /
5 m3 carried).** `is_reserved` list in `dynamic.rs:242-256` is not mirrored
at install time — a user leaf named `status` would surface as SQLite's own
`duplicate column name: status` error rather than a framework-level message.
Phase 6 didn't touch this path. **Carries forward; deferrable.**

## Executor deviations — both defensible

- **`build_leaf_cmd_owned` (`dynamic.rs:97-151`):** owned-`String` variant
  of `build_leaf_cmd` to satisfy clap's `From<String> for clap::builder::Str`
  with the `string` feature. Same pattern as Phase 4's `clap "string"`
  feature opt-in. Avoids `&'static str` constraint when the verb name comes
  from a runtime-loaded YAML schema. Defensible.
- **`wont_fix` accepts underscores natively in clap.** Verified live:
  `stores observations wont_fix L001` parses and routes correctly. No
  kebab conversion needed at the verb level. Defensible.

## DONE_WHEN status

Phase 6 fully closes:
- **#2** install registers store + DDL applied ✓
- **#4** add returns L001 with status=open ✓
- **#5** T3 triage without contract cites all 3 fields + required_when rule ✓
- **#6** full triage moves status to triaged ✓
- **#7** show preserves nested Records (text + JSON) ✓
- **#8** list shows the row ✓

Sets up:
- **#3** multi-store coexistence (verified empirically via observations +
  kitchen_sink in the same DB; final verification with `gate` is Phase 7).

## Verdict: PASS — advance to Phase 7 (`EXECUTING_PHASE_7`).

Action items: none gate-blocking. The 4 minors are all deferrable and
carry forward to Phase 7+ for incremental cleanup.
