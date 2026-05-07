# T021: topology_dot_snapshot stale: missing T019 states (cargo_installed/schema_migrated/deploy_blocked)

## Meta
- **Status:** plan_review
- **Created:** 2026-05-04T01:38:03Z
- **Last Updated:** 2026-05-04T14:47:03Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T021-auto-promoted-l050

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - tests/topology_dot_snapshot.rs::ac2_4_dot_snapshot_matches: update expected fixture to current emit_dot output
- regen via test framework&#x27;s snapshot mechanism if available, else manual write
- verify run after regen (test passes)
- **Out:** - rethinking snapshot strategy (golden-file vs property tests)
- broader topology refactor
- adding non-T019 states
- cleaning up similar drift in other snapshot tests if they exist (separate observations)

### Done When
Regenerate tests/topology_dot_snapshot.rs&#x27;s expected fixture to include T019 states (cargo_installed, schema_migrated, deploy_blocked) + mark_* edges; restore green test.

Acceptance:
- cargo test --test topology_dot_snapshot passes
- the new snapshot includes cargo_installed, schema_migrated, deploy_blocked + mark_cargo_installed/mark_schema_migrated/mark_deploy_blocked transitions
- no other states added or removed (diff-bounded)
- existing tests pass

### Phases

#### Phase 1: Phase 1: Regenerate expected.dot fixture and verify
- **Objective:** Regenerate tests/fixtures/topology/expected.dot to current emit_dot output (which now includes T019 states cargo_installed/schema_migrated/deploy_blocked and their mark_* transitions), then verify the snapshot test passes and no unrelated drift entered the fixture.
- **Tasks:**
  - Task 1.1: Run &#x60;UPDATE_TOPOLOGY_FIXTURES&#x3D;1 cargo test --test topology_dot_snapshot ac2_4_dot_snapshot_matches&#x60; to overwrite tests/fixtures/topology/expected.dot with the current emit_dot output (the rewrite path is wired at tests/topology_dot_snapshot.rs:59-61).
  - Task 1.2: Inspect &#x60;git diff tests/fixtures/topology/expected.dot&#x60; and confirm the diff is bounded to (a) three new state nodes (cargo_installed, schema_migrated, deploy_blocked) and (b) their incoming/outgoing mark_cargo_installed / mark_schema_migrated / mark_deploy_blocked transition edges. No other states added or removed; no unrelated edge churn.
  - Task 1.3: Re-run &#x60;cargo test --test topology_dot_snapshot&#x60; (all four tests) without UPDATE_TOPOLOGY_FIXTURES to confirm ac2_4_dot_snapshot_matches now passes and ac2_1 / ac2_6 / ac2_8 still pass.
  - Task 1.4: Run &#x60;cargo test&#x60; to confirm the broader suite stays green (no incidental regressions from the fixture refresh).
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo test --test topology_dot_snapshot ac2_4_dot_snapshot_matches&#x60; exits 0.
  - [ ] AC1.2: tests/fixtures/topology/expected.dot contains the substrings &#x60;cargo_installed&#x60;, &#x60;schema_migrated&#x60;, &#x60;deploy_blocked&#x60;, &#x60;mark_cargo_installed&#x60;, &#x60;mark_schema_migrated&#x60;, &#x60;mark_deploy_blocked&#x60;.
  - [ ] AC1.3: &#x60;git diff --stat tests/fixtures/topology/expected.dot&#x60; shows only that one file changed; the diff body is limited to the three new state nodes and their mark_* edges (no other state IDs added or removed).
  - [ ] AC1.4: &#x60;cargo test --test topology_dot_snapshot&#x60; runs all 4+ tests green (ac2_1, ac2_4, ac2_6, ac2_8; ac2_2 may skip if graphviz absent).
  - [ ] AC1.5: &#x60;cargo test&#x60; (full suite) exits 0.
- **Files:** `tests/fixtures/topology/expected.dot`

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

