# T010 Phase 2 — Code Review

**Reviewer:** code-reviewer
**Phase:** 2 (Wrap envelope schema)
**Commits reviewed:** `da8d38c` (production schema + fixture + tests + variant patch), `73901a2` (execution log)
**Date:** 2026-05-01
**Gate:** **PASS**
**Revision count:** 0/3

---

## Summary

Small, well-scoped phase. Production wrap schema is shipped (Draft 2020-12, `additionalProperties: false`, `reasoning` slot first per planner convention, `$id` set, all descriptions present, no `gate` field). `AgentEnvelope::Wrap` gained `reasoning: Option<String>` with `#[serde(default)]` so the Phase 1 inline stub fixture still parses. Schema validation tests cover wrap (positive + negative). Two new unit tests (AC2.3, AC2.4) pass. All 5 ACs satisfied. Test count claim verified: 455 unit + 2 integration = 457 (Phase 1 baseline 455). Build clean — no new warnings. Stub markers correctly retained on `agents/wrap.md` and `stores/tasks/templates/wrap-brief.md.tpl` (Phase 4's job). Strict-envelope ratification (decision matrix row h) confirmed.

Three findings below — none blocking. All are notes/follow-ups, not revisions.

---

## What landed correctly

### Schema shape (AC2.5, AC2.2)

- `$schema` is Draft 2020-12 URL.
- `$id`: `https://stores.local/schemas/wrap` matches the planner convention (`https://stores.local/schemas/planner`).
- `role` is `const "wrap"` with description; the redundant `type: string` from the Phase 1 stub correctly dropped (`const` implies type).
- `executive_summary` is required and described.
- Optional fields: `reasoning` (string), `deviations`, `residual_risks`, `recommended_sanity_checks` (all arrays of strings). Descriptions present on every field.
- `additionalProperties: false` literally present (line 8).
- **No `gate` field** — confirms decision matrix row (h): wrap is synthesis, not decision authority.
- `default: []` correctly dropped from the array fields (not needed; `#[serde(default)]` on the Rust side covers absence).

### Style consistency with `planner.schema.json`

- `reasoning` slot is the **first property** in both schemas (recovery pattern preserved).
- Same field-description style.
- Same top-level structure (`$schema`, `$id`, `title`, `description`, `type`, `required`, `additionalProperties`, `properties`).

### Rust variant (`AgentEnvelope::Wrap`)

- `src/handlers/drive.rs:111-121` — fields in same order as schema properties: `reasoning`, `executive_summary`, `deviations`, `residual_risks`, `recommended_sanity_checks` (role is implicit via `#[serde(rename = "wrap")]` tag).
- `reasoning: Option<String>` with `#[serde(default)]` — verified at `drive.rs:112-113`. The Phase 1 inline `wrap_fixture_json()` (which omits `reasoning`) still parses (used by `happy_path_one_phase_mock` and other Phase 1 tests).
- The arrays already had `#[serde(default)]` from Phase 1 — preserved.

### Fixture quality (`tests/fixtures/agent_outputs/wrap.json`)

- All fields populated: `role`, `reasoning`, `executive_summary`, 2 deviations, 1 residual_risk, 4 recommended_sanity_checks.
- Pretty-printed (multi-line) — only fixture in the directory that is multi-line; see Finding 1.
- Content is on-topic (T010 Phase 2 self-referential — fine for representativeness).

### Validation tests (`tests/schemas_validate_fixtures.rs`)

- `RoleCase { role: "wrap", stray_key: "unexpected_wrap_field" }` added.
- Both positive (`all_fixtures_validate_against_schemas`) and negative (`fixtures_with_stray_field_rejected_by_schema`) cover wrap.
- Negative coverage gives `additionalProperties: false` real meaning.

### `parse_envelope_from_wrap_fixture` (AC2.3)

- Test passes (`cargo test parse_envelope_from_wrap_fixture` — ok).
- Asserts source is `"sdk"` (Layer 1).
- Verifies `reasoning.is_some()`, `executive_summary` non-empty, all three arrays non-empty.
- Choice to use `structured_output` injection (rather than `make_run_output`) is justified — see Finding 1.

### `role_mismatch_wrap_envelope_while_executing` (AC2.4)

- Follows the existing per-role role-mismatch pattern.
- Inserts task at `executing` status, queues a wrap envelope from the runner, drives, asserts Err.
- Asserts the error string contains `executor` (expected role), `wrap` (received), and the session_id (`wrap-mismatch-session`) — all three required substrings match the existing `envelope role mismatch: expected {expected}, received {received}, session_id {sid}` format at `drive.rs:651`.

### Bundled schema registry (AC2.1)

- `cargo test bundled_schemas_count_matches_agents` — ok.
- `BUNDLED_AGENT_SCHEMAS.len() == 6` — entries: planner, plan-reviewer, executor, code-reviewer, guide, wrap.
- (No diff for `cli/agents.rs` in `da8d38c` — registry was already correct from Phase 1.)

### Out-of-scope hygiene

`git show da8d38c --stat` — files changed: `agents/schemas/wrap.schema.json`, `tests/fixtures/agent_outputs/wrap.json`, `src/handlers/drive.rs`, `tests/schemas_validate_fixtures.rs`. Nothing in `agents/wrap.md`, `submit.rs`, `agents/guide.md`, `compute_submit_wrap`, etc. Phase 4's `STUB` marker on `agents/wrap.md` and `stores/tasks/templates/wrap-brief.md.tpl` retained.

### Tests + build

- `cargo test --features runner-claude-code` — 455 unit pass, 2 integration pass. Total 457.
- `cargo build --features runner-claude-code` — Finished, 1 warning (pre-existing dead_code on `AgentEnvelope::Wrap` fields; baseline at `8e8e635` already had this warning, only `reasoning` newly added to the list of unread fields). Phase 3's `compute_submit_wrap` will read these fields; warning is expected to clear then.

---

## Findings (non-blocking)

### Finding 1 — divergent test pattern for `parse_envelope_from_wrap_fixture` (MINOR / informational)

**Verified claim.** The executor's note in main.md is correct: `make_run_output` (`drive.rs:996-1008`) sets `final_message` to the **last non-empty stdout line**, which for the pretty-printed wrap fixture is `}`. With `final_message = "}"`:

- Layer 2 (SAP) — `extract_envelope_from_text("}", None)` finds no JSON-object candidate. Miss.
- Layer 3 (legacy direct parse) — `serde_json::from_str::<AgentEnvelope>("}")` errors. Miss.
- Last-resort stdout last-line scan — also `}`. Miss.

So if `parse_envelope_from_wrap_fixture` had used `make_run_output(wrap_full_fixture_json(), 0)` like its peers (`parse_envelope_from_planner_fixture` etc., all at `drive.rs:1487-1513`), the test would fail. The executor's chosen workaround — inject the parsed JSON directly into `structured_output` — exercises Layer 1 (sdk) and is what real claude-code runs deliver. That's defensible and arguably the more production-relevant path.

**Cost:** the wrap envelope's Layer 2/3 parse paths are now untested at the per-role-fixture level. Other roles all hit Layer 3 ("legacy" source) via `make_run_output`. There is no positive-coverage test that the wrap envelope can be recovered from prose `final_message` (SAP) or last-line stdout (legacy). For wrap specifically this is lower-risk because real runs go through `structured_output`, but the asymmetry is worth flagging.

**Two acceptable follow-ups, neither required for this phase:**

1. Compact `tests/fixtures/agent_outputs/wrap.json` to a single line (matches the other 5 fixtures), then switch the test to use `make_run_output` like its peers — uniformity, also gets Layer 3 coverage for wrap. Cosmetic + 1-line code change.
2. Or: file an enhancement to `make_run_output` so `final_message` is set to the **full stdout** rather than the last line. That would let multi-line fixtures work without special-casing; would also align with how the real runner aggregates SDK output. Slightly bigger blast radius — touches every test that uses `make_run_output`.

Neither is in scope for Phase 2. Calling it out so it's not lost.

### Finding 2 — pre-existing dead_code warning grew by one field (TRIVIAL)

`cargo build --features runner-claude-code` emits:

```
warning: fields `reasoning`, `executive_summary`, `deviations`, `residual_risks`, and `recommended_sanity_checks` are never read
```

Phase 1's baseline (`8e8e635`) already had the same warning sans `reasoning`. The dispatcher arm at `drive.rs:852` `AgentEnvelope::Wrap { .. } => { ... }` discards all fields (Phase 1 stub: just emits the "in_review" sentinel). Phase 3 (`compute_submit_wrap`) is owed the read sites — that will clear the warning. No action this phase.

### Finding 3 — main.md execution-log test count uses different baseline than cycle-2 reviewer (TRIVIAL / reconciliation)

The Phase 1 cycle-2 review note at main.md:523 records: "Execution log test count '457' doesn't match `cargo test --release` reading of 437 (435 unit + 2 integration). Pre-existing drift; reconcile in Phase 6." The Phase 2 execution log records 455 unit + 2 integration = 457, and I confirm 455+2=457 with `cargo test --features runner-claude-code` here. The discrepancy comes from `cargo test` (debug, with feature) vs `cargo test --release` (without the `runner-claude-code` feature, possibly). Not a Phase 2 issue. Already flagged for Phase 6.

---

## ACs verified

| AC | Test/Artefact | Status |
|----|---------------|--------|
| 2.1 | `cli::agents::tests::bundled_schemas_count_matches_agents` (`len() == 6`) | PASS |
| 2.2 | `tests/schemas_validate_fixtures.rs` — both positive and negative wrap cases | PASS |
| 2.3 | `handlers::drive::tests::parse_envelope_from_wrap_fixture` | PASS |
| 2.4 | `handlers::drive::tests::role_mismatch_wrap_envelope_while_executing` | PASS |
| 2.5 | `additionalProperties: false` literal present in `wrap.schema.json:8` | PASS |

---

## Decision

**PASS.** No revisions required. Findings 1–3 are informational/follow-ups.

**Status update:** EXECUTING_PHASE_3 (orchestrator advances; Phase 3 — `compute_submit_wrap` + drive auto-fire `request_review` on PASS-on-last-phase via state-local flag — is unblocked).
