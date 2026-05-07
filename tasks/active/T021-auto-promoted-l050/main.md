# T021: topology_dot_snapshot stale: missing T019 states (cargo_installed/schema_migrated/deploy_blocked)

## Meta
- **Status:** in_review
- **Created:** 2026-05-04T01:38:03Z
- **Last Updated:** 2026-05-04T14:50:00Z
- **Current Phase:** 1
- **Current Cycle:** 1
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

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable. Single phase with mechanical ACs (cargo test exits 0, specific substrings present in fixture, git diff bounded to one file). Decision matrix correctly chooses the wired UPDATE_TOPOLOGY_FIXTURES rewrite path over hand-editing and respects scope_in by not touching expected.md. done_when fully traceable through AC1.1/AC1.2/AC1.4.
- **At:** 2026-05-04T14:47:10Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** No-op refresh: tests/fixtures/topology/expected.dot already contains T019 states (cargo_installed, schema_migrated, deploy_blocked) and mark_* edges. Ran UPDATE_TOPOLOGY_FIXTURES&#x3D;1 cargo test --test topology_dot_snapshot ac2_4_dot_snapshot_matches; rewrite produced byte-identical output (git diff empty). All 5 topology_dot_snapshot tests pass; full suite cargo test green (667 bin tests + integration). Brief premise (fixture stale) did not match reality — fixture was already current, likely refreshed in prior work; no commit needed.
- **Commit:** `none`
- **At:** 2026-05-04T14:48:48Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. Verified executor&#x27;s no-op claim: fixture already contains all 6 T019 substrings (cargo_installed, schema_migrated, deploy_blocked + mark_* edges); UPDATE_TOPOLOGY_FIXTURES rerun would produce byte-identical output (git status clean). All 5 topology_dot_snapshot tests pass; full cargo test suite green (667 unit + integration). commit&#x3D;&#x27;none&#x27; is appropriate because there are no changes to commit. 0 critical, 0 major, 1 minor.
- **Findings:** 0 critical, 0 major, 1 minor
**Details:**
Git reality check:
- &#x60;git status --porcelain&#x60; → empty (clean tree)
- &#x60;git log --oneline -5&#x60; → tip is 4a87148 (worklog 02); no new commit, consistent with executor&#x27;s commit&#x3D;&#x27;none&#x27;.

Acceptance criteria verification:
- AC1.1 PASS: &#x60;cargo test --test topology_dot_snapshot ac2_4_dot_snapshot_matches&#x60; → ok.
- AC1.2 PASS: grep on tests/fixtures/topology/expected.dot returns all 6 required substrings (cargo_installed, schema_migrated, deploy_blocked, mark_cargo_installed, mark_schema_migrated, mark_deploy_blocked) — confirmed via 8 matching lines including state nodes and mark_* edges.
- AC1.3 PASS (vacuously): git diff is empty since fixture was already current. No unrelated drift possible when there is no diff. The brief&#x27;s premise (fixture stale) was incorrect — fixture was already updated in prior work (likely T019 or T020).
- AC1.4 PASS: all 5 tests green (ac2_1, ac2_2, ac2_4, ac2_6, ac2_8); ac2_2 did NOT skip — graphviz appears installed and the dot syntax test ran.
- AC1.5 PASS: full &#x60;cargo test&#x60; suite green; 667 unit tests + integration suites all 0 failed / 0 ignored anomalies.

Findings:
[MINOR] Brief premise (fixture stale, missing T019 states) was incorrect at task-creation time. The fixture was already refreshed by prior work (likely as part of T019/T020 merges visible in recent log: bfa7a72 merge of T020). This is not an executor defect — the executor correctly diagnosed the no-op condition rather than fabricating a diff. Worth recording so the planner/observer system notices: a diagnostic step (&#x60;cargo test --test topology_dot_snapshot&#x60; before scaffolding) would have surfaced the no-op state at observation/promotion time and avoided minting T021 entirely. Suggestion: nothing to fix in code; consider filing a low-priority observation about pre-promotion diagnostic checks for fixture-staleness tasks.

Note on commit&#x3D;none: appropriate. Per executor protocol, committing a byte-identical refresh would create empty noise in history. The drive harness should accept this as a valid no-op completion. If the substrate enforces commit-required, that is a separate substrate friction worth filing — but the AC contract here is the test passing, which it does.
- **At:** 2026-05-04T14:49:40Z

---

## Completion
- **In Review:** 2026-05-04T14:50:00Z — awaiting human GO/NO_GO

