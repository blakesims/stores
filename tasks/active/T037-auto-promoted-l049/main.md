# T037: Auto-resolve-observation: close linked obs when task hits schema_migrated

## Meta
- **Status:** in_review
- **Created:** 2026-05-06T03:57:35Z
- **Last Updated:** 2026-05-06T04:23:29Z
- **Current Phase:** 1
- **Current Cycle:** 2
- **Blocked Reason:** —
- **Branch:** feat/T037-auto-promoted-l049

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - agents.yaml: declare auto-resolve-observation builtin (subscribes to tasks: → schema_migrated)
- src/flow/builtins/auto_resolve_observation.rs: reads tasks.linked_observations; updates observations.status + observations.resolution per entry
- Idempotency: skip entries already resolved
- Failure tolerance: orphan entries log+ntfy, don&#x27;t crash subscriber
- Predicate guard: no-op when linked_observations is empty
- Integration test: full 10-step chain end-to-end
- **Out:** - Schema changes (none required — observations already has status&#x3D;resolved + resolution field)
- Custom resolution strings per observation (mechanical commit_sha for all)
- Re-opening logic if a downstream task touches a previously-resolved observation
- Cross-store cascading beyond observations (e.g. tickets in another store)

### Done When
Ship auto-resolve-observation: a daemon subscriber that marks every entry in tasks.linked_observations as resolved (with resolution&#x3D;task.commit_sha) when the task hits schema_migrated, closing step 10 of the 10-step pipeline (worklog 08). The mirror image of auto-promote — when the upstream link sets observation.task_id forward, this closes the back-link by setting observations.status&#x3D;resolved.

Acceptance:
- On task transition cargo_installed→schema_migrated (or equivalent terminal-success state), every entry in linked_observations has its observation row updated to status&#x3D;resolved, resolution&#x3D;&lt;task.commit_sha&gt;, within ~5s (one poll cycle)
- Multi-observation tasks (e.g. T020 with L046+L047) resolve all linked entries with the same commit_sha
- Already-resolved observations skipped silently (idempotent on daemon restart or re-run)
- Orphan linked_observations entries (no matching observation row) log warning + ntfy without failing the subscriber
- Existing tests pass; new E2E test asserts ratify→promote→scaffold→drive→accept→deploy→resolve chain landed observation as resolved

### Phases

_Plan not yet submitted._

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** REVISE
- **Summary:** Implemented builtin:auto-resolve-observation subscriber satisfying T037 contract (T1-tier, contract-is-plan; brief shows Phase 1 of 0 with empty phase body). On task entry to schema_migrated, subscriber resolves every linked_observation with resolution&#x3D;task.commit_sha; skips already-resolved (idempotent); orphan entries log warning + ntfy without failing. 6 unit tests pass (resolves_all_linked_observations_with_executor_commit, already_resolved_is_not_overwritten, orphan_warns_and_notifies_without_failing, dispatch_keyword_resolves, string_encoded_linked_observations_and_commit_sha_parse, fixture_yaml_includes_auto_resolve_observation); E2E coverage extended in flow_promote_scaffold_drive_e2e.rs for the resolve edge. cargo build clean.
- **Commit:** `a2bd27f29bc3f666911a0b7615d7a57f84f6ae64`
- **Files:**
  - `src/flow/builtins/auto_resolve_observation.rs`
  - `src/flow/builtins/mod.rs`
  - `src/flow/agents_yaml.rs`
  - `docs/agents-yaml-example.yaml`
  - `tests/fixtures/agents-yaml/post-accept-chain.yaml`
  - `tests/fixtures/agents.yaml`
  - `tests/flow_promote_scaffold_drive_e2e.rs`
  - `tests/flow_starting_line_e2e.rs`
- **At:** 2026-05-06T04:08:39Z
### Phase 1 / Cycle 2
- **Status:** PASS
- **Summary:** REVISE cycle 2 addresses all 5 code review findings. Added 6 declared auto_resolve transitions ({open|investigating|confirmed|ready|needs_info|in_progress} → resolved, actor&#x3D;framework) to stores/observations/schema.yaml. Extracted fire_framework_transition_for(schema, ...) + load_store_schema(name) helpers in flow/builtins/mod.rs; legacy fire_framework_transition delegates. Refactored auto_resolve_observation::resolve_one to fire the schema-declared auto_resolve verb — no more raw UPDATE, no more fabricated history verb, source-state gating now schema-enforced (wont_fix/resolved rejected fail-loud). Tests updated: insert_obs now sets captured_week; dispatch_keyword_resolves asserts outcome; added auto_resolve_writes_declared_transition_history_row + auto_resolve_rejects_terminal_source_state. All 8 auto_resolve_observation tests pass; cargo build clean. Pre-existing sidecar_handoff failures present on parent commit, unrelated to this phase.
- **Commit:** `713638a0f4a820eb51d33a8562eb5512663f897b`
- **Files:**
  - `stores/observations/schema.yaml`
  - `src/flow/builtins/mod.rs`
  - `src/flow/builtins/auto_resolve_observation.rs`
- **At:** 2026-05-06T04:15:36Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** REVISE
- **Summary:** REVISE. Implementation works in tests but bypasses the substrate&#x27;s transition lifecycle by raw-UPDATE&#x27;ing observations and fabricating a non-declared verb in transition_history. The dominant production case (auto-promoted observations resolve from status&#x3D;&#x27;ready&#x27;) has no declared transition in the observations schema — the test passes only because the raw UPDATE skips lifecycle validation. All other builtins (auto_promote, accept_merge, cargo_install, schema_migrate) go through fire_framework_transition; this one diverges. 1 critical, 1 major, 3 minor.
- **Findings:** 1 critical, 1 major, 3 minor
**Details:**
[CRITICAL] Subscriber bypasses observations lifecycle via raw UPDATE.
File: src/flow/builtins/auto_resolve_observation.rs:148-173
Evidence: &#x60;resolve_one&#x60; issues a direct &#x60;UPDATE observations SET status&#x3D;&#x27;resolved&#x27; ...&#x60; and then manually inserts a &#x60;transition_history&#x60; row with &#x60;verb&#x3D;&#x27;auto_resolve_observation&#x27;&#x60;, an undeclared verb. &#x60;stores/observations/schema.yaml&#x60; declares only two &#x60;→ resolved&#x60; transitions: &#x60;open → resolved&#x60; (verb close_as_addressed) and &#x60;in_progress → resolved&#x60; (verb resolve). There is NO &#x60;ready → resolved&#x60; transition. Auto-promoted observations land at &#x60;ready&#x60; (per auto_promote subscriber + per contract: &#x27;mirror image of auto-promote ... when upstream link sets observation.task_id forward, this closes the back-link&#x27;). Production T020 case (L046+L047) is exactly the &#x60;ready → resolved&#x60; jump that the schema does not declare.
Expected: Subscribers go through &#x60;fire_framework_transition&#x60; to gate via validators / actor / on_entry hooks (mirroring auto_promote::run, accept_merge, cargo_install, schema_migrate). The unit test &#x60;resolves_all_linked_observations_with_executor_commit&#x60; only passes because the raw UPDATE skips the missing-transition error.
Suggestion: Add a &#x60;ready → resolved&#x60; transition (and any other relevant source state — &#x60;confirmed&#x60;, &#x60;investigating&#x60;, &#x60;needs_info&#x60;?) to stores/observations/schema.yaml with verb&#x3D;&#x60;auto_resolve&#x60; actor&#x3D;&#x60;framework&#x60;, then refactor &#x60;resolve_one&#x60; to call a small helper analogous to &#x60;fire_framework_transition&#x60; against the observations schema. The from_status read becomes safe (validated by select_transition), and the transition_history row is written by execute_transition_write with the correct declared verb. This is the dogfood-correct fix per CLAUDE.md and matches every other builtin in src/flow/builtins/.

[MAJOR] No source-state gate; will force-resolve from any state including wont_fix/needs_info.
File: src/flow/builtins/auto_resolve_observation.rs:131-155
Evidence: &#x60;resolve_one&#x60; checks only &#x60;status &#x3D;&#x3D; &#x27;resolved&#x27;&#x60; and otherwise issues UPDATE. If a linked observation is at &#x60;wont_fix&#x60; or &#x60;needs_info&#x60;, it gets silently flipped to resolved.
Expected: Resolve only from states where it is semantically valid (open, in_progress, ready). Other source states should at minimum log + skip.
Suggestion: Once the lifecycle-driven path is in place (critical above), the schema&#x27;s transitions enumerate the legal source states and select_transition will reject the rest. Until then, add an explicit allowlist: &#x60;if !matches!(status.as_str(), &quot;open&quot; | &quot;in_progress&quot; | &quot;ready&quot;) { warn + skip }&#x60;.

[MINOR] Pre-transaction status read is racy; from_status in transition_history may not reflect actual UPDATE state.
File: src/flow/builtins/auto_resolve_observation.rs:132-167
Evidence: SELECT status runs outside the transaction; the UPDATE uses &#x60;WHERE status !&#x3D; &#x27;resolved&#x27;&#x60; for safety, but transition_history is inserted with the pre-tx status regardless of whether UPDATE actually changed any row.
Suggestion: Move SELECT inside the transaction OR detect zero-rows-updated from &#x60;tx.execute(...)&#x60; return value and skip the history insert (returning AlreadyResolved) in that case. The schema-driven path (see CRITICAL) makes this moot.

[MINOR] dispatch_keyword_resolves test asserts only registration, not outcome.
File: src/flow/builtins/auto_resolve_observation.rs:380-394
Evidence: Test passes empty linked_observations and asserts only &#x60;res.is_some()&#x60;. Doesn&#x27;t verify the dispatch returns Ok / does the work.
Suggestion: Insert a real observation, dispatch with linked&#x3D;[L###], assert observation flipped + dispatch_builtin returned Ok(0).

[MINOR] Scope creep: T037 commit modifies tests/flow_starting_line_e2e.rs to update seed_starting_line() to 3-arg signature.
File: tests/flow_starting_line_e2e.rs:118-124, 188-194
Evidence: &#x60;git log -- tests/flow_starting_line_e2e.rs&#x60; shows the file was last modified by c877cbb (T026 P2). The seed_starting_line signature changed in 7703608 (L116 fix); apparently that commit didn&#x27;t update this test, and T037 is sweeping up the breakage.
Expected: T037 changes only auto-resolve-observation surface area.
Suggestion: Note the carry-fix in the commit message; alternatively pull it into a follow-up commit on the L116 series. Not blocking but worth flagging.
- **At:** 2026-05-06T04:11:16Z

### Phase 1 / Cycle 2
- **Gate:** PASS
- **Summary:** Re-applying PASS after L123 schema fix (T1 PASS guard); cycle 2 code_reviewer issued PASS but submit-review crashed under stale binary.
- **Findings:** 0 critical, 0 major, 0 minor
- **At:** 2026-05-06T04:22:42Z

---

## Completion
- **In Review:** 2026-05-06T04:23:29Z — awaiting human GO/NO_GO

