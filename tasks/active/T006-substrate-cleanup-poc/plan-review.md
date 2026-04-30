# T006 Plan Review — Detailed Findings

**Reviewer:** plan-reviewer
**Gate:** NEEDS_WORK
**Date:** 2026-04-30

---

## Summary

The plan is structurally sound: each of the four POC findings maps cleanly to a phase, Phase 5 captures all four enforcement moments as artefacts, and the Decision Matrix names the high-stakes choices (DDL escape vs reject, fail-silent coerce, ListFk bundling, keep pipe-separated). The DONE_WHEN clauses align 1:1 with Phase ACs. Out-of-scope creep into T005 code (drive.rs, parse_envelope, status.rs, next_action.rs) is correctly fenced.

Two concrete gaps need to be closed before READY:

1. **Phase 1 silently inherits an existing selection-semantics gap** that submit.rs already solved. The planner says "do not change submit's selection semantics," but `transition.rs::run_in_tx` uses `.iter().find(|t| t.verb == verb)` (first-match-by-verb), which is **already inconsistent** with submit's `find_transition` (filter by from+verb+gate, prefer guarded-true, fall back to unguarded). The L275 schema has two `from: confirmed, verb: start_t2/start_t3` transitions distinguished only by guard. Phase 1's "find first then check guard" will eval the first guard, fail it, and bail — instead of falling back to the second. The L275 POC re-run uses distinct verbs (`start_t2` vs `start_t3`) so this won't trip Phase 5's artefact, but the substrate fix is incomplete: a future schema with two same-verb transitions partitioned by guard would deadlock.

2. **DDL audit (Phase 3, path A) is undercounted.** The plan claims "~6 sites" but a full grep finds substantially more, including sites the planner missed entirely.

3. **Phase 4's "join with `|`" implementation has a documented edge case** that is named in the Decision Matrix but is not protected by any acceptance criterion. The Phase 5 artefact (`repeatable-flag.txt`) needs to use a pipe-free value to assert equivalence holds in the safe case.

Plus several smaller risks worth flagging but not blocking.

---

## Per-question findings

### 1. DONE_WHEN alignment

**PASS.** Each of the four DONE_WHEN clauses maps to a specific phase:
- (1) ratify rejects when contract is draft → Phase 1 AC + Phase 5 step 5 (`ratify-rejected.txt`).
- (2) `evidence.external_refs` round-trips as JSON array via `show --json` → Phase 2 AC + Phase 5 step 11 (`show-l001.json`).
- (3) hyphenated store name installs cleanly OR rejected with clear error → Phase 3 AC + Phase 5 step 12 (`hyphen-install.txt`).
- (4) `--in-scope X --in-scope Y` ≡ `--in-scope "X|Y"` → Phase 4 AC + Phase 5 step 13 (`repeatable-flag.txt`).

Phase 5 literally restates the four moments and captures each as a named artefact file. Test gates (`cargo test --all` + three e2e scripts + T005 drive smoke) match the DONE_WHEN exactly.

### 2. Bug-coverage completeness

**PASS.** Findings A/B/C/D map to Phases 1/2/3/4 respectively. Phase 5 closes the integration loop. No dangling items from `## Task`'s four findings.

### 3. Decision Matrix soundness — high-stakes spot-checks

#### 3a. DDL escape vs install-time-reject (Finding C) — **NEEDS_WORK**

Choice (path A — quote in DDL) is sound in principle, but the **audit is incomplete**. The planner names ~6 sites; the actual interpolation surface is larger:

| Site | Already listed? |
|------|-----------------|
| `src/codegen/ddl.rs:95` `CREATE TABLE IF NOT EXISTS {table}` | ✓ |
| `src/handlers/add.rs:130` `INSERT INTO {} (...)` | ✓ |
| `src/handlers/add.rs:141` `UPDATE {} SET display_id` | ✓ |
| `src/handlers/transition.rs:201` `UPDATE {} SET ...` | ✓ |
| `src/handlers/update.rs:134` `UPDATE {} SET ...` | ✓ (under "update.rs UPDATE SQL") |
| `src/handlers/row.rs:189` `SELECT ... FROM {table}` | ✓ |
| `src/handlers/list.rs:112` `SELECT ... FROM {}{}{}{}` | ✓ |
| `src/handlers/show.rs` | ✓ (named generically) |
| `src/handlers/submit.rs::write_status_and_fields` (`submit.rs:217` UPDATE) | ✓ |
| `src/handlers/submit.rs:81/94/112` (`acquire_lock`/`release_lock` SQL all use `{table}` = `&schema.name`) | ✗ **MISSED** |
| `src/handlers/drive.rs:246` `SELECT display_id FROM {table}` | ✗ **MISSED** |
| `src/handlers/drive.rs:879/883` `INSERT INTO {name} ...` (drive's task-row insert) | ✗ **MISSED** |
| `src/handlers/drive.rs:1174` `UPDATE {} SET blocked_reason ...` | ✗ **MISSED** |
| `src/handlers/next_action.rs:381/399` `UPDATE {} SET blocked_reason` | ✗ **MISSED** |
| `src/handlers/next_action.rs:288–294` test-helper `INSERT INTO {} (...)` | ✗ **MISSED** (test code, but still needs quoting if test schemas have hyphens) |

The drive.rs and next_action.rs sites are critical — they touch tasks-store rows during workflow execution. If path A misses any one of them, a hyphenated tasks-store name would crash at runtime in a non-obvious code path. **Required**: planner must explicitly enumerate every interpolation site reached via `quote_ident` and assert the audit is exhaustive (`grep -rn 'INTO {' src/ ; grep -rn 'FROM {' src/ ; grep -rn 'UPDATE {' src/` is the canonical sweep — paste the count into the plan).

Note also that `submit.rs` uses a `let table = &schema.name;` pattern with `format!("... {table} ...")` interpolation. If the helper is `quote_ident(&schema.name)`, all those sites need a one-line refactor (`let table = quote_ident(&schema.name)`). Manageable, but the plan should call out the pattern.

#### 3b. Fail-silent coerce errors on bad list_record JSON (Finding B) — **PASS with caveat**

Choice (b) — `Value::Null` on parse failure, validator catches it — is consistent with existing `coerce_value` behaviour for malformed integers (which falls back to `Value::String(raw)`). Operator UX risk: the validator error will read "field is required" or "expected array, got null," not "JSON parse error at column 17." That's strictly worse than the current pipe-separated form's clear "field is required" because the operator passed something but it disappeared.

This is a minor UX regression in the bad-input case, but consistent with the current substrate philosophy. Acceptable as decided. Recommend Phase 2 add a unit test that asserts the validator error is informative enough that an operator can debug ("missing required field 'external_refs'" — not silent success). Currently AC2 only asserts the happy path round-trip.

#### 3c. ListFk bundled with ListRecord in Phase 2 — **PASS, with one note**

The bundling is sound: same bug shape (coerce_value falls through, write-path match doesn't include the variant). Today `list_fk` fields (`tasks.depends_on`, `tasks.linked_observations`, `observations_1006.depends_on`/`linked_observations`) are written **programmatically only** — see comment at `row.rs:17-18`: "ListRecord and ListFk fields cannot be set via flat CLI args; they are written programmatically." Submit handlers pass JSON `Value`s directly, bypassing coerce_value. So adding `ListFk` to coerce_value is purely additive (creates new CLI surface) — no behaviour change for existing programmatic writers.

However, the planner did not call this out as **enabling new CLI surface** for `list_fk` fields. That's a side-effect of the bundling. Worth a note in the plan that after Phase 2, operators can pass `--depends-on '["T001","T002"]'` on add/update (where today they'd get a String value silently). Not a blocker, but adjacent acceptance criteria should confirm: (a) read_row's existing list_fk handling still works, (b) the new write path produces a JSON array compatible with the read path.

### 4. Phase independence

#### 4a. Phase 4 vs Phase 5 — **PASS**

Phase 4's "keep both forms working" approach (join with `|` in `get_arg`) preserves backwards compat. Phase 5's POC trace already uses the repeatable form (`--in-scope main.py --in-scope dev`), and the `repeatable-flag.txt` artefact verifies both forms agree. Sanity check: existing `tests/e2e.sh` does not currently use any pipe-separated list flag (`grep -n in-scope tests/`), so there's no implicit regression vector. ✓

**Caveat — edge case under-protected**: the Decision Matrix names the "single value containing a literal pipe" edge case (`--in-scope "a|b" --in-scope c` round-trips as `["a","b","c"]`, not `["a|b","c"]`). This is unchanged from current pipe-separated behaviour, but the Phase 5 artefact must use a pipe-FREE value to assert the equivalence ("a", "b") rather than something like ("a|b", "c"). Recommend Phase 5 step 13 use unambiguous values and an AC that says "equivalence holds for pipe-free values" so the artefact isn't accidentally probative of the buggy edge case.

#### 4b. Unit-test coverage per phase, not hidden behind Phase 5 — **PASS**

Each phase has its own ACs that include in-crate unit tests:
- Phase 1: unit test in `transition.rs` extending `OBS_SCHEMA` for guard true/false.
- Phase 2: unit test in `row.rs`/`add.rs` for round-trip; e2e for show --json.
- Phase 3: unit test in `ddl.rs` for hyphenated DDL; unit test or e2e for CRUD against hyphenated store.
- Phase 4: unit test (or e2e) for `--field a --field b` ≡ `--field "a|b"`.

Phase 5 is integration on top of these, not a substitute for them. ✓

### 5. API change blast radius for Phase 4 — **PASS**

Total `get_one::<String>` count in handlers/cli is 44. The plan correctly identifies the central choke point (`get_arg` closure in `add.rs:33`, `transition.rs:80`, `update.rs:39`) and recommends the **minimum-blast-radius approach**: change only the closure shape (or join with `|` before passing to coerce_value), not the public API. The other ~41 `get_one` callsites consume scalar fields (display_id, status, sort, since, gate, etc.) and are unaffected.

The "join with `|`" approach is explicitly the recommendation in the plan. Three callsites get touched: add.rs, update.rs, transition.rs. Surgical and contained. ✓

### 6. Test-scope sanity (regression-trap tests) — **PASS**

Each phase has a regression-trap test that the bug fix specifically catches:
- Phase 1: guard-false case fails with named error → traps the marquee Finding A.
- Phase 2: `show --json` returns array (not string) → traps Finding B.
- Phase 3: hyphenated store install + CRUD → traps Finding C.
- Phase 4: `--in-scope a --in-scope b` produces `["a","b"]` array → traps Finding D.

No "no error" assertions; all are positive shape/value assertions. ✓

### 7. Anti-pattern flags

#### 7a. T005 code creep — **PASS**
Scope explicitly fences `parse_envelope`, drive.rs, status.rs, next_action.rs. **Caveat**: Phase 3's DDL audit MUST quote `schema.name` interpolations in `drive.rs:879/1174` and `next_action.rs:381/399`. This is not "rewriting T005 code" — it's a one-line `quote_ident()` substitution at each callsite. Scope should explicitly authorize this minimal touch with the rationale "DDL fix requires uniform quoting; not a behaviour change to drive/next_action logic."

#### 7b. `submit::find_transition` rewrite scope — **PASS**
Plan limits the change to swapping the inline `eval(...)` for a helper call. Selection semantics preserved (guarded-true preferred, unguarded fallback, ambiguity error). ✓

#### 7c. Manual-verification ACs in Phase 5 — **PASS**
All four enforcement moments have a captured artefact file. No "manual eyeball" steps. ✓

#### 7d. **NEW: Phase 1 selection-semantics gap (NOT FLAGGED IN PLAN)** — **NEEDS_WORK**

`transition.rs:36-41` does:
```rust
let transition = schema.lifecycle.transitions
    .iter()
    .find(|t| t.verb == verb)
    .ok_or_else(...)?;
```

This picks the **first** transition matching the verb. After Phase 1 adds guard evaluation, the flow is: find first → eval guard → bail if false. This does NOT match submit's `find_transition`, which filters by `from + verb + gate`, prefers guarded-true, and falls back to unguarded.

**Concrete risk**: a schema with two same-verb transitions partitioned by guard (e.g. `confirmed -- start_t2 (guard:T2) --> in_progress` and `confirmed -- start_t2 (guard:T3) --> in_progress`) — Phase 1 will always pick the first, eval its guard, and bail when the row is in the other branch. Today the bug is masked because the simple `find` succeeds (no guard check) and the submit path uses different verbs (`start_t2` vs `start_t3`). After Phase 1 lands, this becomes a regression for any schema that uses guard-partitioned transitions on the plain CLI surface.

**Mitigation**: Phase 1 should adopt submit's selection algorithm. The cleanest path:
1. Move `find_transition`'s body into a shared helper in `validate::guard` (or `lifecycle::find_transition`).
2. Have both `transition::run_in_tx` and `submit::find_transition` delegate to it.
3. The plan's existing `eval_transition_guard` helper subsumes into this.

Alternatively, restrict Phase 1 to "if the matched transition has a guard, eval it" with a documented caveat that multi-guard partitioning on plain transitions is not supported in v0.4.x and add a schema-validation rule that errors at install time on duplicate `(from, verb)` pairs without distinct guards. Either approach is acceptable; the plan must pick one and add an AC.

The L275 POC trace itself does NOT trip this — `ratify` is the only `from: open` transition. So Phase 5 will pass even with the gap. This is exactly why the gap is dangerous: the substrate fix declares "guards now fire" but the substrate is still inconsistent under realistic schema patterns. Required to fix before READY.

---

## Required revisions (NEEDS_WORK)

1. **Phase 1: address selection-semantics gap.** Either (a) extract the full selection algorithm (filter by from+verb+gate, guarded-true preferred, unguarded fallback, ambiguity error) into a shared helper used by both `transition::run_in_tx` and `submit::find_transition`, OR (b) document and enforce at install-time that plain transitions cannot have multiple `(from, verb)` entries distinguished only by guards. Add the corresponding AC.

2. **Phase 3: complete the DDL audit.** Run the canonical sweep (`grep -rn 'INTO {\|FROM {\|UPDATE {' src/`) and enumerate every site explicitly. Specifically add: `drive.rs:879/1174`, `next_action.rs:381/399`, `submit.rs:81/94/112` (acquire_lock/release_lock), `submit.rs:217`, `drive.rs:246`. Commit to a `quote_ident` helper used at every site or document a per-site rationale where it's not needed. Note that some sites (e.g. `acquire_lock`) take `&schema.name` as a parameter and interpolate it as `{table}` — those need adjustment at the call sites.

3. **Phase 4 & Phase 5: pin down the equivalence-test value.** Phase 5 step 13 must use pipe-free values (e.g. `--in-scope main.py --in-scope dev`, not `--in-scope "a|b"` mixed with repeatable form). Add an AC to Phase 4 that explicitly tests the pipe-free equivalence and a separate AC documenting (without test) the pipe-containing edge case as a known limitation.

4. **Phase 2: add UX-regression test for bad JSON.** Add an AC that asserts: `add ... --external-refs 'not-json'` produces a validation error mentioning the field name `external_refs` (not a silent succeed). Optional-but-recommended: include the parse-error reason in the validator's error message.

5. **Phase 2: note ListFk side-effect.** Decision Matrix entry on ListFk bundling should call out that this enables CLI write-paths for `list_fk` fields that today are programmatic-only. Add a brief AC: read-after-write via CLI for a `list_fk` field round-trips as `Value::Array` of strings (mirroring the current programmatic write contract).

---

## Open questions (none escalated to BLOCKED)

The five revisions above are all concrete and within planner scope. No human input required.

---

## Routing

→ **PLANNING** (back to planner for revisions 1–5).

After revision, re-route to plan-reviewer for re-gate.

---

# Cycle 2 Review (2026-04-30)

**Reviewer:** plan-reviewer
**Gate:** READY

## Summary

All five cycle-1 revisions verified as actually closing the named gap. The plan is structurally complete, the audits are exhaustive, and the regression-trap fixtures are concrete enough that they cannot pass for the wrong reason. Status advances PLAN_REVIEW → READY.

## Verification methodology

For revision 2 (DDL audit) I ran the canonical sweep myself rather than trust the planner's count:

```bash
grep -rn -E 'INTO \{|FROM \{|UPDATE \{' src/
```

Returned 16 matches (add×2, drive×3, list×1, next_action×3, row×1, submit×4, transition×1, update×1). Adding `ddl.rs:95` (`CREATE TABLE IF NOT EXISTS {table}` — outside the INTO/FROM/UPDATE pattern) yields 17, matching the planner's enumeration exactly. Every site flagged in cycle 1 (drive.rs:879/1174, next_action.rs:381/399, submit.rs:81/94/112, drive.rs:246) is present.

For revision 1 (selection algorithm) I confirmed both regression-trap transitions share `from: confirmed` and `verb: ratify`, differing only in `guard: "tier == 'T2'"` vs `"tier == 'T3'"`. The fixture cannot pass for a reason other than guard-discrimination.

For revision 3/5 (Phase 4/5 pipe coverage) I read each AC text — values are concrete (`"a"`, `"b"`, `"a|b"`, `"c"`, `"main.py"`, `"scripts/"`), expected arrays are spelled out, no pipe characters appear in the trap-test values themselves.

Programmatic AC count per phase: 4/4/4/4/4. All within the ≤4 constraint.

## Per-revision verdict

| Cycle-1 revision | Verdict | Evidence |
|---|---|---|
| 1: Selection algorithm | CLOSED | `select_transition` named, signature given, fixture concrete (lines 84–98) |
| 2: DDL audit completeness | CLOSED | 17 sites, matches independent canonical sweep (lines 121–137) |
| 3: Phase 4/5 pipe coverage | CLOSED | Three Phase 4 ACs with concrete values; Phase 5 step 13 pipe-free |
| 4: Bad-JSON UX | CLOSED | AC `list_record_bad_json_returns_validator_error` (line 113) asserts field name surfaces |
| 5: ListFk regression-trap | CLOSED | tasks_e2e.sh canary AC (line 112); Decision Matrix row 8 calls out new CLI surface |

## Decision Matrix arithmetic

7 cycle-1 rows + 2 new (Bad-JSON UX, Pipe-containing values) + 1 expanded (Phase 1 algorithm) = 9 rows total. Confirmed.

## No new gaps introduced

- Phase 5 still captures four enforcement moments as artefacts.
- Out-of-scope unchanged.
- T005 logic still fenced (line 138 explicit).
- AC count per phase still ≤4.

## Routing

→ **READY** — orchestrator may move folder to `tasks/active/` and dispatch executor for Phase 1.
