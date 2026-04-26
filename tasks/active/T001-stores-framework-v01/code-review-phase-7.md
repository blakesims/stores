# Phase 7 Code Review — Bundled `gate` store + human-only actor enforcement demo

- **Gate:** PASS
- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Commit:** `e0f1dac`
- **Issues:** 0 critical / 1 major / 3 minor

## Summary

All 6 Phase 7 ACs verified end-to-end against a fresh tmp dir with both bundled stores installed. `cargo test` 83/83 pass (matches executor claim — no new tests added in this phase, which is itself worth noting; see m2 below). DONE_WHEN #3 (multi-store coexistence), #9 (gate add returns G001), #10 (human can answer), #11 (CLAUDECODE-detected ai_autonomous rejected with documented error format) all close.

The two framework fixes the executor flagged are both real bugs and the fixes are correct in scope. However, a **regression-class parallel of fix #2 exists in `src/handlers/update.rs` and is reachable today** (live repro below) — same root cause, not fixed in this commit. Not gate-blocking for Phase 7 because the demo path doesn't traverse it; flagging for Phase 8 or v0.2 carry-forward.

## Live E2E walk

Fresh `mktemp -d`, fresh `cargo build --release`, fresh `stores init`:

- **AC1 (#3):** `stores install ./stores/observations` then `stores install ./stores/gate` — both registered in `manifest.yaml`; `sqlite3 .stores/db.sqlite ".tables"` → `gate observations`. Multi-store coexistence in one DB confirmed.
- **AC2 (#9):** `stores gate add --type decision --question "Soft or hard delete?" --options "soft|hard" --task-ref L001` → `G001` (printed, exit 0). Row state: `display_id=G001 status=pending type=decision question="Soft or hard delete?" options=["soft","hard"] task_ref=L001 answer=NULL`.
- **AC3 (#10):** `stores gate answer G001 --answer hard --invoker human` → `Transitioned G001: pending → answered`; row reads back `status=answered answer=hard`.
- **AC4 (#11):** `CLAUDECODE=1 stores gate answer G002 --answer a` (no `--invoker`) → exit 1 with **two** errors aggregated (defense in depth — both layers fire correctly):
  - `<transition:answer>: transition 'answer' requires actor 'human'; invoker is 'ai_autonomous' (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)`
  - `answer: field 'answer' requires actor 'human'; invoker is 'ai_autonomous' (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)`
  Both messages cite the field/transition `answer` and the required actor `human`, both name `$CLAUDECODE` as the auto-detection source, both suggest `--invoker human` as the override. Exact format the prompt called for.
- **AC4b:** `CLAUDECODE=1 stores gate answer G002 --answer a --invoker human` → `Transitioned G002: pending → answered`. Override path works.
- **AC5:** `CLAUDECODE=1 stores gate cancel G003` (no `--invoker`) → `Transitioned G003: pending → cancelled`. The `cancel` transition's declared `actor: ai_autonomous` matches the env-detected invoker; the `answer` field carries `actor: human` but is not in the diff (cancel only mutates `status`), so the field actor check correctly does not fire on it. **This is exactly what fix #2 was supposed to enable** — see Framework fix #2 verification below.
- **AC6 (cargo test):** 83/83 pass.
- **Phase 8 forward-compat:** Re-ran the JOIN that DONE_WHEN #12 demands — `SELECT o.display_id, o.status, g.display_id, g.status, g.answer FROM observations o LEFT JOIN gate g ON g.task_ref = o.display_id WHERE o.display_id = 'L001';` returns `L001|triaged|G001|pending|` (real JOIN match, non-NULL gate display_id). Phase 8 AC #2 (`≥1 row with non-NULL gate display_id`) is reachable as-is.

## Framework fix #1: `Value::Null` treated as absent in `check_actor`

`src/validate/actor.rs` lines 23–27:
```rust
match lookup(entry, field_path) {
    None | Some(serde_json::Value::Null) => return,
    _ => {}
}
```

**Defensible.** Symmetric with `src/validate/required.rs:43` which has had `is_absent = field_value.is_none() || field_value == Some(&Value::Null)` since Phase 5. Without this symmetry, an unset optional field in a row read back via `read_row` (which injects `Value::Null` for absent columns at `row.rs:201, 215`) trips actor enforcement spuriously on operations like `cancel` that don't write the field. The fix is the right shape; the reasoning is correct.

**Localization check:** `check_actor` has only one call site (`mod.rs:105`); `check_transition_actor` (the other public function in `actor.rs`) takes a verb name, not a field-path lookup, so the Null question doesn't apply there. No other call sites need parallel treatment.

**Regression check on tests:** `actor.rs::absent_field_no_actor_error` already covered the `None` case; this fix doesn't break it. There's no existing test asserting actor enforcement on a field whose value is `Value::Null` (which would have been a wrong assertion anyway). The 83-test suite still passes.

**Fix is the minimal correct change.** Not a major.

## Framework fix #2: `Op::TransitionWithDiff` scoping actor checks to the diff

`src/validate/mod.rs` adds `Op::TransitionWithDiff(String, EntryMap)`; `validate()` separates the entry passed to `validate_field` into `entry` (used for required/enum/pattern — correctly evaluated against the merged final state, since `required_when` cross-Record paths need the merged shape) vs `actor_entry` (used for actor — correctly the diff for transitions, the full entry for Add/Update). `validate_field` takes both and passes `actor_entry` to `check_actor`.

`src/handlers/transition.rs:89` switches the call from `Op::Transition(verb)` to `Op::TransitionWithDiff(verb, diff.clone())`.

**Defensible — and necessary.** The pre-fix logic forced a `human` invoker calling `gate answer G001 --answer hard` to be re-validated against `type`, `question`, `options`, `task_ref` — fields the `ai_autonomous` add-time invoker wrote — and rightly produces no error there because none of those fields have a `human` actor constraint. The actual previously-reported failure (the executor's reverse case: AI cancelling a gate where a human had already written `answer`) is what the fix unblocks. Live repro confirms the fix works: AC5 above runs `CLAUDECODE=1 stores gate cancel G003` against a gate whose `answer` field is `Null`-shaped in the merged row, and the actor check correctly does not fire on `answer` (because the diff is just `status`-shaped).

**Op semantics verification:**
- `Op::Add` (handlers/add.rs:37) — passes `&entry` as the entry; `actor_entry` resolves to the same `entry`. Correct: in `Add`, the entry IS the diff (nothing existed prior). All fields the user writes get actor-checked.
- `Op::Update` (handlers/update.rs:65) — passes `&merged` as the entry; `actor_entry` resolves to `merged`. **This is the same bug the executor just fixed for transitions.** See M1 finding below.
- `Op::Transition(verb)` (no diff) — match arm at `mod.rs:48` falls back to `entry` for actor scoping. Currently no caller emits this variant; reachable only via direct unit test (`mod.rs:350-351` does this, but the fixture has no field-level actor constraint that would make the difference visible). Correct as-is for tests; semantically equivalent to old behaviour.
- `Op::TransitionWithDiff(verb, diff)` — `actor_entry = diff`. The intent.

**Op::Add path spot-check:** `gate add --type decision --question "..."` as `ai_autonomous` writes `type` and `question` (no actor constraint at field level; `default_actor: ai_autonomous` matches). `answer` is absent from the diff, so even though it carries `actor: human`, the check correctly does not fire on it. Verified empirically — `gate add` succeeds without `--invoker`.

**Naming nit (minor):** `Op::TransitionWithDiff` is fine but `Op::Transition` becomes vestigial — only used by direct unit tests. Phase 8 or v0.2 could fold them into `Op::Transition(String, Option<EntryMap>)` or `Op::Transition { verb, diff }`. Not gate-blocking; named clearly enough.

## Major: Op::Update has the same merged-row bug — unfixed

The executor's own diagnosis ("actor checks should apply to writes, not to reads of pre-existing data") logically applies to `Op::Update` too. `handlers/update.rs:65` passes `&merged` to the validator with `Op::Update`, and `validate()` then uses that same merged entry for actor scoping at `mod.rs:50`.

**Live repro:**
```bash
$ stores init
$ stores install ./stores/gate
$ stores gate add --type decision --question "Q?" --options "a|b"   # G001 by ai_autonomous
$ stores gate answer G001 --answer a --invoker human                # human writes answer=a
$ CLAUDECODE=1 stores gate update G001 --question "Q?-updated"      # AI tries to fix the question
Error: validation failed:
- answer: field 'answer' requires actor 'human'; invoker is 'ai_autonomous' (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)
exit=1
```

The AI is updating only `question` (no actor constraint on that field); the validator rejects because the merged entry contains the human-written `answer`. Same shape as the bug Phase 7 fixed, identical root cause, identical fix pattern (pass `Op::UpdateWithDiff(diff)` or refactor the variants).

**Not gate-blocking for Phase 7** because:
1. Phase 7's six ACs don't traverse this path.
2. Phase 8's e2e (per the plan, lines 244–255) walks the README demo: install → add → triage → gate add → gate answer → cancel → JOIN. No `update` of an `answer`-bearing gate row by a different invoker. The bug is latent in the e2e path.
3. The fix is small (~10 LOC mirroring the transition fix) but adding it inside Phase 7's review window risks scope creep when Phase 8 looms.

**Recommendation:** Address in Phase 8 alongside the e2e + README — single-PR cleanup. If Phase 8 keeps moving and this slips, capture as v0.2 and add a regression test in v0.1 that asserts the current (buggy) behaviour so we know what we're carrying.

## Schema correctness check (`stores/gate/schema.yaml`)

- `id_format: "G{:03d}"` — yes, line 2.
- `default_actor: ai_autonomous` — yes, line 3.
- 3 lifecycle states `[pending, answered, cancelled]`, 2 transitions with declared actors — yes, `answer→human`, `cancel→ai_autonomous`. Correct.
- `type` enum (`decision|script`), required — yes.
- `question` text, required — yes.
- `options` `list: text`, optional — yes.
- `answer` text, optional, **`actor: human`** — yes, line 38.
- `task_ref` text, optional — **NOTE:** plan line 236 declared `task_ref` as `display_id` ("display_id, optional, no FK constraint at SQL level — cross-store reference by convention; accepts any display_id from any installed store"). Executor downgraded to `text`. Per the prompt this is acceptable (`display_id` permissive vs `text` are both fine for the demo) and the field's description string in the schema documents the intent. **However, this deviation is not called out in main.md's Phase 7 execution log.** Minor — see m1 below.

DDL probe confirms columns `type / question / answer / task_ref` are all `TEXT` with no `NOT NULL` — matching how the framework treats `required` (validator-enforced, not DB-enforced). Consistent with observations store. Enum CHECK on `type` present (`CHECK (type IN ('decision', 'script'))`).

## Minor findings

- **m1 (deviation not documented):** `task_ref` declared as `text` in the gate schema, not `display_id` as the plan called for. The schema description string acknowledges the cross-store-ref intent, but the Execution Log block in main.md doesn't list this as a deviation. Add a "Deviations" sub-bullet to the Phase 7 entry. Functionally harmless — `display_id` and `text` both write as TEXT and DDL is identical.
- **m2 (no new tests):** Phase 7 added two genuine framework changes but no unit test for either. The Null-as-absent fix has no regression test (a `Value::Null` case in `actor.rs::tests` would be ~5 LOC). The `Op::TransitionWithDiff` scoping has no test that asserts the actor check is correctly suppressed on a non-diff field. The e2e walk above proves the behaviour, but a unit-level regression test would be cheaper to maintain. Recommend adding both in Phase 8 or as a follow-up — they're trivial and would pin the behaviour.
- **m3 (carried, reserved-column-name leaf collision; cycle-2 fresh-eye m1c2):** Still not caught at install time. Phase 7 didn't address; deferrable to v0.2 per the plan's stated risk acceptance. v0.1 bundled stores don't trigger it.

## Forward-compat for Phase 8

- **DONE_WHEN #12 (cross-store JOIN):** Verified live above — the JOIN query from the prompt returns a real match. Phase 8 e2e can rely on this.
- **`tests/e2e.sh` correspondence:** Phase 8 will write the script and the README. Nothing in Phase 7's surface conflicts.
- **`--json` polish:** Phase 6 already verified nested round-trip on `show`/`list` for observations; gate's Records (none) and List (`options`) will go through the same code path. No risk.
- **Op::Update bug (M1 above):** Will not bite Phase 8 e2e per the README demo path, but if anyone adds an "update an existing gate's question" step it will surface. Worth Phase 8 awareness.

## Status

PASS — advance to Phase 8 (`EXECUTING_PHASE_8`).
