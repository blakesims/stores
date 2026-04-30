# T006 Phase 2 — Code Review

- **Reviewer:** code-reviewer
- **Date:** 2026-04-30
- **Commit reviewed:** `e079400` (feat) + `70a401d` (exec-log update)
- **Gate:** **REVISE (substantial)**
- **Counts:** 1 critical / 0 major / 1 minor

---

## TL;DR

Write-path fix is correct and the user-facing round-trip works: `show --json` emits `evidence.external_refs` as a JSON `array`, not a quoted string. Read path was already correct (deserialises all four JSON types in `row.rs::read_row`). Out-of-scope clean: no Phase 1 / T005 / drive / next_action / submit touches. All 387 cargo tests pass.

**Blocker:** the Decision-Matrix-blessed contract ("fail-silent coerce → validator surfaces field name via required-rule") only holds when the list_record/list_fk field is REQUIRED. For OPTIONAL fields — including the canonical L275 POC field `evidence.external_refs` — bad JSON is silently converted to `Value::Null` with **no error message at all**. This is the exact UX regression cycle-2 revision 4 was added to prevent.

---

## AC verification

| AC | Verdict | Evidence |
|----|---------|----------|
| `coerce_value` ListRecord/ListFk arm parses JSON / fails-silent | PASS | `row.rs:109-114`: `serde_json::from_str → Ok(Array(arr)) ⇒ Array; _ ⇒ Null`. Unit tests for valid/bad/non-array shape pass. |
| `add.rs`, `update.rs`, `transition::execute_transition_write` storage match arms cover both new types | PASS | `add.rs:84` extended to 4-way; `update.rs:97`, `transition.rs:154` extended `List(_)` arm to `List \| ListRecord \| ListFk`. |
| Round-trip test asserts `read_row(...)` returns a JSON array, not a string blob | PASS | `add.rs::list_record_cli_round_trips_as_array` asserts `Value::Array` and inspects `arr[0]["system"]`. |
| Bad-JSON test asserts validator error message contains the field name | PARTIAL | Unit test passes for **required** field (`external_refs: required` mentions the field). Optional-field path silently corrupts (no error). See Critical 1. |
| `tests/tasks_e2e.sh` (ListFk canary) passes | PARTIAL | Step 16 SIGPIPE failure verified pre-existing on master (`33fac5a`). Live `tasks` canary via CLI: `--linked-observations '["L001","L002"]'` round-trips as array. |
| `cargo test --all`, `tests/e2e.sh`, `tests/drive_e2e.sh`, `tests/tasks_e2e.sh` green | PARTIAL | cargo: PASS (387/0). drive_e2e.sh: PASS. e2e.sh + tasks_e2e.sh failures verified pre-existing pre-T006 (CLAUDECODE inheritance / SIGPIPE under pipefail). |

---

## Round-trip via `show --json` (the marquee Finding-B test)

Run from `/tmp/t006-p2-rt`:

```
$ stores observations_1006 add --summary "rt test" --source dev --priority normal \
    --captured-at 2026-04-30 \
    --external-refs '[{"system":"docker","kind":"container","id":"foo"}]'
L001

$ stores observations_1006 show L001 --json | jq '.evidence.external_refs'
[
  {
    "id": "foo",
    "kind": "container",
    "system": "docker"
  }
]

$ stores observations_1006 show L001 --json | jq -r '.evidence.external_refs | type'
array
```

The user-facing round-trip emits a structured JSON array. Finding-B substrate gap is closed for the happy path.

---

## Out-of-scope check

```
$ git show e079400 --stat
 Cargo.lock                  | 2 +-
 src/handlers/add.rs         | 112 ++++++++++++++++++++-
 src/handlers/row.rs         | 69 ++++++++++++-
 src/handlers/transition.rs  | 2 +-
 src/handlers/update.rs      | 2 +-
 tasks/active/.../main.md    | 21 +++-
```

Files touched are exactly the 5 planned. NO `lifecycle.rs` re-touch (Phase 1 fence holds). NO `drive.rs` / `next_action.rs` / `submit.rs` (T005 fence holds). `Cargo.lock` is the v0.4.1 bump propagation, committed in the same commit (clean — no unstaged drift after the commit; review concern #6 cleared).

`List(_)` pipe-split arm in `coerce_value` is untouched (review concern #5 cleared) — no accidental behaviour change to existing list:text fields.

---

## Critical 1 — Silent corruption on bad JSON to OPTIONAL list_record/list_fk fields

The Decision Matrix row "Bad-JSON UX in list_record coerce (cycle-2 revision 4)" picked option (b) — silent `Value::Null` + validator error enriched to mention parse failure for the field. Phase 2's implementation:
1. Coerce returns `Value::Null` on bad JSON ✓
2. Validator surfaces field name **only when the field is required**.

The chosen test fixture (`REQUIRED_LR_SCHEMA` in `add.rs`) hard-codes `required: true`, which makes the validator's `required` rule fire and mention `external_refs`. But the canonical POC schema has the field OPTIONAL:

```yaml
# stores/observations_1006/schema.yaml
- name: evidence
  type: record
  fields:
    - {name: observed_at, type: timestamp}
    - {name: env, type: text}
    - name: external_refs
      type: list_record
      # NO `required: true` here!
      fields:
        - {name: system, type: text, required: true}  # sub-field required, not the parent
        ...
```

Live reproduction against this schema:

```
$ stores observations_1006 add --summary "bad json" --source dev --priority normal \
    --captured-at 2026-04-30 --external-refs '{not json'
L002          # ← no error!

$ stores observations_1006 show L002 --json | jq '.evidence'
{
  "external_refs": null
}
```

The operator's input is silently discarded with zero diagnostic. This violates the AC L113 "the error must mention the field name AND indicate the value was rejected" contract — there IS no error.

This also matches `tasks/schema.yaml`'s `linked_observations` and `depends_on` (also optional list_fk): bad JSON to those silently writes Null.

### Why this is gate-blocking

1. **Cycle-2 revision 4 explicitly identified this UX gap and added AC L113 to close it.** Phase 2's implementation closes it for required fields only — the AC's intent (operator-debuggability floor) is unmet for the realistic case.
2. **Phase 5 artefact integrity.** The L275 POC re-run captures `evidence.external_refs` round-trip in `show-l001.json`. If an operator typos the JSON during capture, the artefact silently records `null` and the "substrate enforces shape" thesis the POC narrative depends on is undermined — the very enforcement moment the POC was supposed to demonstrate.
3. **Substrate philosophy.** T006 is fundamentally a "rules in the schema must fire at runtime" task. Silent input rejection is the same shape of violation as Finding A (silently-ignored guard) — the substrate accepted invalid input without emitting a runtime signal.

### Fix options (small)

- **(a) RECOMMENDED — sentinel-on-bad-JSON:** make `coerce_value` for ListRecord/ListFk return e.g. `Value::String(raw)` on parse failure (instead of `Value::Null`). Validator detects type-mismatch (String where Array expected for list_record column) and emits `"invalid JSON for <field>: <parse-error>"`. Fires for both required and optional fields. ~15 LOC + 1 test.
- **(b) coerce-at-handler boundary:** in `add.rs::run` / `update.rs::run` / `transition::execute_transition_write`, validate JSON shape before validation step; bail on parse failure with clean error. Larger blast radius (3 sites).
- **(c) downgrade AC L113 to "required field only"** and document optional-field silent-corruption as accepted. Cheapest, but contradicts cycle-2 revision 4's explicit operator-debuggability intent.

**Recommendation: (a).** Add a test `list_record_bad_json_on_optional_field_surfaces_error` that asserts the error fires even when the field is optional. This pins the AC L113 floor without forcing schema authors to mark every list_record field required.

---

## Minor 1 — Required-path message is `field: required`, not `invalid JSON for field`

The Decision Matrix row 4 prefers "invalid JSON for `external_refs`" wording over a generic "missing required field." Today the emitted message is `"validation failed:\n- external_refs: required"` — the field name comes from `pretty_print`'s path-prefix, not from a JSON-aware error. The unit test passes because it only asserts `msg.contains("external_refs")`, which is true.

Acceptable per the AC's "at minimum" floor, but if Critical 1 is addressed via fix option (a), the same enrichment surfaces for the required path automatically — both required and optional cases get a unified `"invalid JSON for external_refs: <reason>"` message. No extra LOC if folded together.

---

## Pre-existing failures (not Phase 2 regressions)

| Test | Failure | Verified pre-existing on |
|------|---------|--------------------------|
| `tests/e2e.sh` Step 6 (triage T3 with full contract) | actor mismatch — `ai_autonomous` invoker auto-detected from inherited `CLAUDECODE=1` | runs clean with `env -u CLAUDECODE` (script doesn't `unset CLAUDECODE`) |
| `tests/tasks_e2e.sh` Step 16 (`cargo test ... \| grep -q "test result: ok"`) | SIGPIPE under `set -o pipefail` — grep matches early, cargo test gets killed, pipefail fires | reproduces on `33fac5a` (master pre-T006) |
| `tests/drive_e2e.sh` | n/a — PASS | — |

Both `e2e.sh` and `tasks_e2e.sh` failures reproduce on master immediately before T006 started; not Phase 2 regressions. Worth a separate cleanup (unset CLAUDECODE in e2e.sh; rewrite the Step 16 grep to `cargo test ... > /tmp/x.log; grep -q ... /tmp/x.log`) but out of T006 scope.

---

## Routing

**Gate:** REVISE (substantial)

**Status:** `CODE_REVIEW` → `EXECUTING_PHASE_2`

**Revision scope:**
1. Address Critical 1 — pick fix-option (a) or justify a different option in writing. Add `list_record_bad_json_on_optional_field_surfaces_error` test (or equivalent name) pinning the operator-error floor for optional fields.
2. (Optional, recommended in same revision) Minor 1 — fold the wording enrichment so both required and optional cases emit `"invalid JSON for <field>"`-style messages.

Estimated LOC: ~15-25 across `row.rs::coerce_value` + 1 validator wiring + 2 tests. Same files Phase 2 already touches.
