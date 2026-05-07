# T002 Plan Review — Cycle 2

**Reviewer:** Plan-reviewer agent
**Date:** 2026-04-26
**Plan version:** main.md as of 2026-04-26 cycle-2 planner submission (1062 lines)
**Cycle:** 2 of max 3

---

## Gate Decision

**READY** → forward to executor.

The planner's cycle-2 revision addresses every cycle-1 critical and major
finding. The architectural picture is now coherent: Phase 1 owns the field-shape
foundation (list_record/list_fk/requires_gate), Phase 5 owns the transaction
boundary with explicit `run_in_tx` plumbing, the 4th-REVISE guard math is
algebraically correct, and the worked-example transcript exposes (and corrects)
the most likely off-by-one error a careful executor would still have made.

Three minor inconsistencies remain — see "Cycle-2 propagation hygiene" below —
but they are textually small, self-evident under test execution, and the
executor will catch them through AC failure rather than design rework. A third
cycle for these would be wasteful relative to the bias guidance ("don't
manufacture issues to justify a third cycle").

---

## Cycle-1 Critical Verification

### C1 — Phase 1 owns `list_record`, `list_fk`, depth-3 walk

**RESOLVED.**

- 1.7: `FieldType::ListRecord(Vec<Field>)` added with TEXT-as-JSON storage,
  RawFieldType deserialiser extension, recursive validator walk for
  list_record/record/list_text/scalar inner fields. Explicit depth note
  ("depth 3" for cycles[].executor and plan.phases[].name).
- 1.8: `FieldType::ListFk { ref_store: String }` added with the soft-FK
  semantics from the v0.1 `task_ref` precedent: TEXT JSON column of
  display_ids, no insert-time enforcement, lazy resolution at render time
  with graceful missing-row handling.
- 1.9: `read_row` / `build_entry_map` extension for depth-3 nests called
  out specifically — "audit every `path.len() <= 2` check; either lift the
  limit or branch on type." This was the exact gap cycle-1 flagged at
  `src/handlers/row.rs:188`. AC1.9 covers the round-trip:
  `plan.phases[2].name` and `cycles[1].executor.summary`.
- AC1.7, AC1.8, AC1.9, AC1.10, AC1.11 are all individually testable.

The "Files" list now correctly enumerates `src/handlers/row.rs` ("lift
`path.len() <= 2` depth limits") and `src/schema/mod.rs` ("FieldType::ListRecord
and FieldType::ListFk variants").

### C2 — 4th-REVISE guard, post-increment, `current_cycle <= 4`

**RESOLVED — and the planner caught their own off-by-one in the worked-example transcript.**

This is the single most important fix in the cycle. Verifying the math
end-to-end:

- Initial value: `current_cycle = 1` after `add` (A28; 5.4 table; 5.5 narrative).
- 1st REVISE: post-bump 1→2; guard `2 <= 4` TRUE; cycle 2 begins.
- 2nd REVISE: post-bump 2→3; guard `3 <= 4` TRUE; cycle 3 begins.
- 3rd REVISE: post-bump 3→4; guard `4 <= 4` TRUE; cycle 4 begins.
- 4th REVISE attempt: post-bump (working-copy) 4→5; guard `5 <= 4` FALSE;
  the engine routes to the unguarded `code_review → blocked` fallback;
  the working-copy bump rolls back so persisted `current_cycle = 4`.

This gives exactly the DONE_WHEN-required "3 REVISE cycles allowed; 4th
rejected" semantics.

Where the math is asserted in the plan:
- Phase 5.5 table — correct.
- Phase 7 schema YAML line 526 (`guard: "current_cycle <= 4"` on
  REVISE→executing) — correct.
- AC5.4 — correct ("3 REVISEs succeed; 4th rejected; would-be value 5").
- AC5.4b cross-phase isolation — explicitly tests the cumulative-counter
  bug from cycle 1 doesn't reappear.
- A24 in Decision Matrix — correct.
- Worked-example transcript — first traces it with `<= 3`, catches the
  off-by-one, reverts to `<= 4`, and explicitly notes the cycle-2
  self-audit moment. This is a useful pedagogical artifact for the
  executor (and for any future review).

The planner's worked-example self-audit IS the kind of thing the
recommendation at the end of cycle-1 was meant to surface. It worked.

### C3 — Transaction boundary with `run_in_tx` split

**RESOLVED.**

- 5.7 is explicit: `transition::run` keeps its existing entry-point shape
  and opens its own transaction; `pub(crate) fn run_in_tx(tx: &Transaction,
  ...)` is the re-entrant core called by submit handlers (which hold the
  outer tx) and by engine-fired follow-on transitions inside the same tx.
- 5.3 enumerates 13 ordered steps, beginning with `let tx =
  conn.unchecked_transaction()?;` and ending with commit. Every read,
  validator pass, user-write, follow-on, and lock release is inside `tx`.
- 5.3 step 10 specifies lock release as the FINAL action inside `tx`,
  resolving M11.
- AC5.11 covers panic injection between the user-write (5.3 step 8) and
  the follow-on (5.3 step 9), asserting either both writes apply or
  neither — the atomicity contract C3 demanded.
- AC5.13 asserts the lock is held across follow-on transitions inside `tx`.

### C4 — Drop "next-action validates submit"

**RESOLVED.**

- 5.6 explicitly removes the dependency: "submit handlers do NOT call
  `next-action`; the validator's actor model is the invariant enforcement.
  The original Phase 5 dependency was incoherent."
- Phase 5's "Dependencies" line now reads "Phase 1 (Expr/eval, list_record,
  requires_gate), Phase 2 (Workflow + submit_targets). Phase 4 dependency
  removed (C4)."
- 4.2 step describing `next-action` reiterates: "This verb is purely a
  read primitive. It returns which agent SHOULD be spawned next; it is
  NOT used to validate submission writes."

The boundary is now clean.

---

## Cycle-1 Major Verification

| # | Item | Status | Notes |
|---|------|--------|-------|
| M1 | Slug immutability acknowledged; render dir-move handles renames | RESOLVED | 6.3 documents the "if user does mutate slug" behavior explicitly. |
| M2 | submit-plan + actor model interaction | NOT EXPLICITLY ADDRESSED | The plan inherits "no actors on plan sub-fields" by virtue of the schema YAML not declaring them; this is acceptable for v0.2 but the cycle-1 finding's request for a comment was not landed. Minor. |
| M3 | submit_targets pulled into Phase 2.1 | RESOLVED | Lines 196-197; AC2.6 covers the validation. |
| M4 | requires_gate pulled into Phase 1.10 | RESOLVED | Phase 1.10 + AC1.10. Phase 7.2 explicitly removed ("now done in Phase 1.10"). |
| M5 | ready→executing on-entry firing tested | RESOLVED | AC5.7 covers the synchronous follow-on inside the same tx; 5.4 table row "submit-plan-review --gate READY" enumerates the post-actions. |
| M6 | framework-actor DDL test | RESOLVED | Phase 1.11 + AC1.11. |
| M7 | claimed_by/claimed_at in next-action JSON | RESOLVED | 4.2 step 4 includes both fields; AC4.2 explicitly covers the locked-row case. |
| M8 | Engine post-action table for current_phase / current_cycle bumps & resets | RESOLVED | 5.4's table is comprehensive: every (status, verb, gate) combination has framework-field-writes and follow-on columns. |
| M9 | Two PASS transitions disambiguated by guard | RESOLVED | 5.5b explicit; Phase 7 schema YAML lines 522-528 declare both transitions with `current_phase < plan.phases.length` and `current_phase >= plan.phases.length`. The planner chose option (a) "guard-disambiguated", consistent across the plan. |
| M10 | BLOCKED→READY recovery semantics | RESOLVED | 5.4 row "resume" + AC5.14 + Phase 9 step 12a-12d. current_cycle resets to 1, current_phase preserved, cycles list preserved as audit trail. |
| M11 | Lock semantics across follow-on | RESOLVED | 5.4 lock-semantics paragraph + AC5.13. Lock held throughout tx; released as final action before commit. |

**M2** is the only major not fully addressed. It's a minor risk (no current
schema field declares `actor: ai_with_human` on plan sub-fields, so the
problem doesn't manifest in v0.2). Folding into Phase 7 documentation
during execution is appropriate — not a blocker.

---

## Decision Matrix Consolidation

Cycle-1 recommended closing five open questions (Q2/Q4/Q5/Q6/Q8) and
surfacing one new (Q-NEW-1). All landed:

- **A19 (Q2):** Render dir-move synchronous in `render`. Rationale lands at
  line 733; matches the cycle-1 ruling.
- **A20 (Q4):** Self-contained per-agent templates. Rationale: locality
  beats DRY for ~50-100 line templates; revisit at ~150 lines. Sound.
- **A21 (Q5):** Engine writes context-rich `blocked_reason`. The exact
  format is enumerated in 5.4 ("4th revise rejected by guard
  current_cycle <= 4 on phase {N} cycle {M}: <last-review summary>").
  Verifies the human reading a blocked task gets actionable context.
- **A22 (Q6):** Auto-cycle to planning with the
  `plan_review_log.length < 3` guard for 4th-attempt block. Closes the
  self-contradiction cycle-1 noted (Q6 contradicted AC5.8).
- **A23 (Q8):** Bash for the e2e walk; Rust integration test for the
  panic-injection atomicity tests (AC5.11/5.13/5.14). Sensible split —
  bash can't easily simulate panics.

**Q-NEW-1 (legacy T001/T002 vs. DB-backed `tasks list` UX):** Surfaced
correctly. The planner recommends C (clean break in v0.2; document
boundary). The question is well-formed: a single-paragraph user
decision with three concrete options and downstream implications named
(affects how Phase 8's `task:next` skill handles "next ready task"
queries).

**Net open questions for user input:** Q1 (priority field scope), Q3
(`requires_gate` shape), Q7 (`scope: repo` outside git: fall back vs.
error), Q-NEW-1 (legacy task discovery). All four are genuinely
user-level decisions; none can be planner-decided without overruling a
written source or making a UX call that's not the planner's to make.

---

## Worked-Example Transcript Assessment

This was cycle-1's specific request. The planner delivered a 240-line
transcript (lines 781-1017) that walks one full task lifecycle:
add → plan → plan-review (READY) → execute → 3 REVISE cycles → 4th
REVISE attempt → BLOCKED → resume → re-execute → PASS → phase 2 →
PASS-last → complete → render.

**The transcript's most useful artifact is the cycle-2 self-audit at
lines 892-916.** The planner's first pass through the transcript used
`current_cycle <= 3`, then traced through and caught their own
off-by-one (only 2 REVISEs would be allowed instead of 3), then
documented the correction inline and amended A24 / Phase 5.5 / Phase 7
schema accordingly. This is exactly the implicit-decision-forcing
exercise cycle-1 asked for.

**Coverage check:**
- Initial state assertions match A28 (current_cycle=1, current_phase=0). ✓
- submit-plan flow inside one tx with lock acquire/release. ✓
- submit-plan-review --gate READY firing the on-entry
  ready→executing follow-on inside the same tx. ✓
- 3 REVISEs proceed; 4th rejected; cycles[].review for the failing
  attempt is written as audit trail; status routes to blocked;
  blocked_reason populated. ✓
- resume verb resets current_cycle to 1, preserves current_phase and
  cycles list. ✓
- PASS-non-last selects → executing; PASS-last selects → complete via
  the M9 / 5.5b guard partition. ✓
- Render is a separate post-commit command (not inside the submit tx). ✓
- The directory move on status_dir change happens inside render. ✓

**No contradictions found between transcript and schema YAML / 5.4
post-action table / ACs (modulo the propagation hygiene flags below).**

---

## Cycle-2 propagation hygiene (NOT critical, surfaced for executor)

The C2 guard fix correctly propagated to: A24, Phase 5.5 narrative,
Phase 5.5 table, Phase 7 schema YAML line 526, AC5.4, AC5.4b, the
worked-example transcript final state, R12. **It missed three
locations:**

### Hygiene-1 — AC7.2 says initial `current_cycle: 0`

`AC7.2: stores tasks add ... returns T001; row reads back with
status: planning, current_phase: 0, **current_cycle: 0**.`

This contradicts:
- A28: "Initializes to 1 on add"
- 5.4 row "Initial add": "current_cycle = 1 (initial values)"
- Worked-example transcript line 790: "current_cycle = 1 (initial; per A28)"
- The whole `current_cycle <= 4` post-increment math, which assumes
  initial value 1.

**Fix:** AC7.2's tail should read `current_cycle: 1`.

### Hygiene-2 — Phase 9.3 step 9 says `current_cycle: 1` after 1st REVISE

`9. ... submit-review T001 --gate REVISE ... → assert status: executing,
**current_cycle: 1**.`

Per the corrected semantics: 1st REVISE bumps 1→2, so the assertion
should be `current_cycle: 2`.

**Fix:** Phase 9.3 step 9's tail should read `current_cycle: 2`.

### Hygiene-3 — Phase 9.3 step 10 says "to hit cycle 3"

`10. Repeat steps 8+9 two more times to hit **cycle 3**.`

After three REVISEs from initial value 1, current_cycle = 4 (1→2→3→4),
not 3. The "hit cycle 3" framing is the pre-correction cycle-2 first-pass
math. Step 11's "4th submit-review with REVISE" is consistent with
4→5-and-fail; step 10's setup math is what's stale.

**Fix:** Phase 9.3 step 10 should read "Repeat steps 8+9 two more times
to hit cycle 4 (so that the next REVISE attempt would be the 4th and
should fail)."

### Why these are hygiene, not critical

- The ACs are inline assertions in tests; an executor implementing to
  AC5.4 / 5.4 table / A28 will see Phase 9 step 9-10 fail and trace back
  to the inconsistency. The fix is mechanical (three numbers).
- AC5.4 is the marquee assertion; AC7.2 is the "initial state" assertion
  for one task in one phase. The executor will follow AC5.4 for the
  marquee feature and adjust AC7.2 / Phase 9 to match.
- Sending the whole plan back through cycle 3 for three numbers is
  disproportionate.

The executor is instructed in the cycle-2 review summary to apply these
three fixes during Phase 7 / Phase 9 implementation and call them out
in the Execution Log.

---

## Other observations (executor-relevant, non-blocking)

### Phase 1 size

Cycle-1 minor m4 noted Phase 5 alone might be 1500+ LOC. Cycle 2
correctly diagnosed that Phase 1 ALSO grew (~700-900 LOC by absorbing
list_record/list_fk/requires_gate per C1/M4). A29 documents the
size-growth and defers split-vs-don't to the executor or a third plan
review. Acceptable: splitting now risks the same cross-phase-dependency
hazards cycle-1 caught; deferring keeps phase boundaries clean and
gives the executor judgment over the actual implementation experience.

### M2 fold-in

The "submit-plan + actor model interaction" cycle-1 flag suggests adding
a comment to Phase 7 schema explaining that plan sub-fields don't
declare actors and thus inherit ai_autonomous-implicit. This was not
landed in cycle 2. Acceptable: the issue doesn't manifest because no
plan sub-field declares an explicit actor in the cycle-2 schema. If a
future schema change adds one, that change will surface the issue
naturally. Mark for executor awareness, not blocking.

### "engine-bumped entry" wording in 5.5b vs. 5.5

5.5 uses "post-increment ordering" for REVISE working-copy bump.
5.5b uses "(engine-bumped) entry" for PASS guard evaluation. PASS
doesn't actually bump anything before transition selection (the
current_phase += 1 happens as a post-action of the selected
transition). The wording in 5.5b is misleading but the semantics work
out: for REVISE, current_cycle is bumped against a working copy; for
PASS, no bump is needed for guard evaluation because the partition
guards reference current_phase pre-bump. Minor wording cleanup.

### Files coverage

The Files list in each phase is more complete than cycle-1. Phase 5
explicitly lists `src/handlers/transition.rs — guard evaluation;
auto-block on guard fail; **split into run and run_in_tx per C3 / 5.7**`,
matching the C3 fix. Phase 1 lists `src/handlers/row.rs` for the
depth-3 walk extension. Phase 6 splits `render::path.rs` out for
testability. Good practice.

---

## What this plan would benefit from (if cycle 3 happens)

If for any reason a cycle 3 occurs, the only items worth surfacing are:

1. The three hygiene-1/2/3 fixes (AC7.2, Phase 9 step 9, Phase 9 step 10).
2. M2 comment in Phase 7 schema YAML.
3. 5.5b wording cleanup ("engine-bumped entry" — clarify or remove).

None justify cycle 3 on their own. They fold into normal execution.

---

## Plan Strengths (cycle 2 additions)

- **Worked-example transcript caught its own bug.** This is the highest-value
  artifact in the cycle-2 revision. Future plan reviewers should consider
  recommending this kind of "force the implicit decisions explicit" exercise
  earlier in the planning loop.
- **Decision Matrix grew from A18 to A29 with all rationale named.** The
  cycle-1 → cycle-2 closures (A19-A23) and the C1/C2/C3/C4 rationales
  (A24-A28) are well-cited and minimize executor re-litigation.
- **Cross-references inside the plan are tight.** "M9 / 5.5b" and "C2 / 5.5"
  anchors throughout AC text mean the executor can navigate the plan
  without re-reading 1062 lines.
- **AC5.11/5.13/5.14 are non-trivially testable.** AC5.11 in particular
  (panic injection between submit-write and follow-on) is the kind of
  test that surfaces real atomicity bugs and is now explicit.

---

**End of plan-review cycle 2.**
