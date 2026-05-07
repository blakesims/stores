# T020: auto-promote + auto-scaffold subscribers (upstream-autonomy unlock)

## Meta
- **Status:** blocked
- **Created:** 2026-05-03T14:35:57Z
- **Last Updated:** 2026-05-03T14:42:09Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** plan-review NEEDS_WORK cycle limit exceeded (plan_review_log.length &gt;&#x3D; 3): Plan is mostly executable with strong contract traceability and a thorough decision matrix, but Phase 2 Task 2.4 directly contradicts the decision matrix&#x27;s chosen &#x60;from_status&#x3D;&#x27;&#x27;&#x60; answer, Phase 1 Task 1.4 punts a sub-decision into a code comment instead of resolving it, and Phase 3 Task 3.4 assumes a &#x60;blocked_reason&#x60; column exists on tasks without verifying. Fix these three and ship.
- **Branch:** feat/T020-upstream-autonomy-unlock

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** schema.yaml: observations status &#x27;ready&#x27; + framework auto-transition confirmed→ready when contract_state writes ready+approved fields. agents.yaml: auto-promote (subscribes observations confirmed→ready) + auto-scaffold (subscribes planning-arrival transition; planner picks shape). src/flow/builtins/auto_promote.rs: contract→task with linked_observations + done_when/scope_in/scope_out propagation + observation.task_id back-link. src/flow/builtins/auto_scaffold.rs: project-configurable scaffold command from .stores/config.yaml; parses worktree path; writes workspace_path. Both idempotent. End-to-end test for the full chain.
- **Out:** auto-drive subscriber (planner spawn on planning, separate task). investigator subagent L043 (separate). Recovery from scaffold failure (planner notes only). Schema-enforced cross-agent ref types L035 (T3, separate).

### Done When
Ratifying a contract creates task at planning within ~5s with linked_observations populated and workspace_path pointing at a real feat-branch worktree; daemon idempotent on re-run; 647/647 tests pass + new E2E ratify-promote-scaffold test passes

### Phases

#### Phase 1: Phase 1: Schema — observations &#x27;ready&#x27; state + framework auto-transition on ratify
- **Objective:** Add a new lifecycle state and transition to observations so a ratified contract (contract_state&#x3D;ready + approved_by/at populated) flows confirmed→ready under framework actor; wire an in-handler hook so this fires whenever the ratify-write commits.
- **Tasks:**
  - Task 1.1: Edit stores/observations/schema.yaml — add &#x27;ready&#x27; to lifecycle.states; add transition {from: confirmed, to: ready, verb: ratify, actor: framework, guard: &quot;intent_contract.contract_state &#x3D;&#x3D; &#x27;ready&#x27; &amp;&amp; intent_contract.approved_by !&#x3D; null&quot;}.
  - Task 1.2: Add observations &#x27;workflow&#x27; block (mirroring tasks pattern) with on_state.confirmed: [transition_to: ready] so the existing fire_on_entry_follow_ons machinery fires the framework hop after entering &#x27;confirmed&#x27; (when the guard is satisfied). Ensure existing transitions out of &#x27;confirmed&#x27; (park, claim, wont_fix) still parse and pass schema validation.
  - Task 1.3: Wire a post-update ratify hook in src/handlers/update.rs (and the lock-contract path in src/handlers/add.rs where invoked on existing rows): after a successful update that writes intent_contract.{contract_state&#x3D;ready, approved_by, approved_at}, if the row is in &#x27;confirmed&#x27;, call fire_framework_transition(verb&#x3D;&#x27;ratify&#x27;) so the same auto-fire path runs from the update path (on_state runs only on transition; ratify-via-update needs explicit firing).
  - Task 1.4: Update src/handlers/add.rs lock-contract code path so when --lock-contract is passed on observations add, the row&#x27;s initial_status is rewritten to &#x27;ready&#x27; (or the row is inserted at &#x27;confirmed&#x27; and the framework transition immediately fires) — choose a single canonical path documented in code comment.
  - Task 1.5: Add 4 unit tests in stores/observations/schema.yaml validation + handlers/update.rs/add.rs covering: (a) ratify auto-fires when update writes ready+approved fields, (b) does not fire when only contract_state&#x3D;ready (no approval), (c) does not fire from any state other than &#x27;confirmed&#x27;, (d) fires from --lock-contract path on add.
- **Acceptance Criteria:**
  - [ ] AC1.1: cargo test passes for new schema + handler tests.
  - [ ] AC1.2: stores observations schema-show prints &#x27;ready&#x27; as a lifecycle state and the new ratify transition.
  - [ ] AC1.3: SQL: after ratifying a confirmed observation via update, SELECT status FROM observations WHERE display_id&#x3D;? returns &#x27;ready&#x27; AND a row exists in transition_history with from_status&#x3D;&#x27;confirmed&#x27; to_status&#x3D;&#x27;ready&#x27; verb&#x3D;&#x27;ratify&#x27; invoker&#x3D;&#x27;framework&#x27;.
  - [ ] AC1.4: Repeat-ratify (calling update twice with the same write) is a no-op on the second call (status remains &#x27;ready&#x27;, no second transition_history row inserted).
- **Files:** `stores/observations/schema.yaml`, `src/handlers/add.rs`, `src/handlers/update.rs`, `src/handlers/transition.rs`, `src/handlers/submit.rs`
#### Phase 2: Phase 2: builtin:auto-promote — observations ready → tasks (planning, linked back)
- **Objective:** Implement the subscriber that observes a ratified observation and creates a task at &#x27;planning&#x27; with linked_observations, executive_intent/done_when/scope_in/scope_out propagated from the observation&#x27;s intent_contract, and writes the observation.task_id back-link. Idempotent.
- **Tasks:**
  - Task 2.1: Create src/flow/builtins/auto_promote.rs following the cargo_install.rs / user_escalation.rs pattern: take (row, ctx); read intent_contract.{objective, in_scope, out_of_scope, acceptance, tier_hint, type} from observation row JSON; build done_when/scope_in/scope_out strings (done_when &#x3D; first acceptance criterion or full list joined; scope_in/out &#x3D; newline-joined lists); insert tasks row with status&#x3D;&#x27;planning&#x27;, linked_observations&#x3D;[obs_display_id], tier_hint, contract.{executive_intent, done_when, scope_in, scope_out}.
  - Task 2.2: Mint the next T### display id by reading MAX(id) from tasks (consistent with user_escalation.rs::file_observation L-id minting).
  - Task 2.3: After successful tasks INSERT, write observations.task_id &#x3D; T### back-link via direct UPDATE (single transaction with the tasks INSERT).
  - Task 2.4: Insert a synthetic transition_history row {store&#x3D;&#x27;tasks&#x27;, from_status&#x3D;NULL, to_status&#x3D;&#x27;planning&#x27;, verb&#x3D;&#x27;create&#x27;, invoker&#x3D;&#x27;framework&#x27;, occurred_at&#x3D;now} so the daemon can deliver a &#x27;planning-arrival&#x27; edge to subscribers (Phase 3 auto-scaffold). Document this as the planning-arrival convention.
  - Task 2.5: Idempotency: before INSERT, query observations.task_id; if non-NULL and the referenced task row exists at status&#x3D;&#x27;planning&#x27; or beyond, skip (return Ok(0)) and log [auto-promote] {obs}: already promoted to {T###}, skipping.
  - Task 2.6: Register the builtin in src/flow/builtins/mod.rs::dispatch_builtin and pub mod auto_promote.
  - Task 2.7: Add agents.yaml entry to tests/fixtures/agents.yaml: {name: auto-promote, subscribes_to: [{store: observations, transition: {from: confirmed, to: ready}}], command: &quot;builtin:auto-promote&quot;}.
  - Task 2.8: Unit tests in src/flow/builtins/auto_promote.rs (mirroring cargo_install pattern with fresh_db_with_tasks): (i) ratified obs → tasks row created with linked_observations populated and contract fields propagated; (ii) observations.task_id back-link written; (iii) re-running the builtin twice is a no-op (one task, not two).
- **Acceptance Criteria:**
  - [ ] AC2.1: cargo test flow::builtins::auto_promote passes (3 tests).
  - [ ] AC2.2: Programmatic: after auto_promote::run on a ready observation row, SELECT COUNT(*) FROM tasks WHERE status&#x3D;&#x27;planning&#x27; &#x3D; 1; SELECT linked_observations FROM tasks ... &#x3D; JSON array containing the obs display_id; SELECT task_id FROM observations WHERE display_id&#x3D;? &#x3D; the new T###.
  - [ ] AC2.3: Programmatic: contract.done_when, scope_in, scope_out on the new tasks row are derived from the observation&#x27;s intent_contract.acceptance / in_scope / out_of_scope.
  - [ ] AC2.4: A second invocation produces no additional tasks rows (idempotent).
- **Files:** `src/flow/builtins/auto_promote.rs`, `src/flow/builtins/mod.rs`, `tests/fixtures/agents.yaml`
- **Dependencies:** Phase 1 complete (ready state + ratify transition exist so the daemon has an edge to subscribe to)
#### Phase 3: Phase 3: builtin:auto-scaffold — tasks planning-arrival → real feat-branch worktree
- **Objective:** Implement the subscriber that observes a freshly-created planning task and runs a project-configurable scaffold command, parses its stdout for a worktree path, and writes that path to tasks.workspace_path. Idempotent.
- **Tasks:**
  - Task 3.1: Extend src/flow/config.rs StoresConfig with an optional &#x60;scaffold&#x60; block: { scaffold: { command: &quot;&lt;shell template, e.g. ./dev/new-worktree.sh {{display_id}} {{slug}}&quot;&gt;, worktree_path_regex: &quot;&lt;regex to extract path from stdout, default &#x27;^WORKTREE&#x3D;(.+)$&#x27;&gt;&quot; } }. Add deserialize support; missing block means scaffold builtin no-ops with status&#x3D;1 + clear log.
  - Task 3.2: Create src/flow/builtins/auto_scaffold.rs: read tasks row, render scaffold command template substituting {{display_id}}, {{slug}}, {{branch}}; spawn via sh -c; capture stdout; apply regex to extract worktree path; UPDATE tasks SET workspace_path&#x3D;? WHERE display_id&#x3D;?.
  - Task 3.3: Idempotency: before running, if tasks.workspace_path is non-empty AND the path exists on disk AND &#x60;git -C &lt;path&gt; rev-parse --git-dir&#x60; succeeds, log [auto-scaffold] {T###}: workspace_path already valid ({path}), skipping; return Ok(0).
  - Task 3.4: Failure handling: scaffold command non-zero exit OR regex no-match → log + write blocked_reason via a one-off direct UPDATE (do NOT flip status — scope_out forbids recovery; planner notes only). Test that this surfaces but doesn&#x27;t transition.
  - Task 3.5: Register builtin in dispatch_builtin; agents.yaml entry: {name: auto-scaffold, subscribes_to: [{store: tasks, transition: {from: &#x27;&#x27;, to: planning}}], command: &quot;builtin:auto-scaffold&quot;}. (Empty-string from_status matches the synthetic &#x27;create&#x27; transition_history row from Phase 2.4.)
  - Task 3.6: Add a tests/fixtures scaffold script (tests/fixtures/scaffold-noop.sh) that: takes display_id + slug, creates a tempdir or git worktree, prints WORKTREE&#x3D;&lt;path&gt; on stdout, exits 0. Used by unit + e2e tests.
  - Task 3.7: Unit tests in src/flow/builtins/auto_scaffold.rs: (i) successful scaffold writes workspace_path; (ii) scaffold command absent in config → no-op with logged warning; (iii) re-running on a row with valid workspace_path is a no-op; (iv) scaffold command failure does not flip status but records blocked_reason.
- **Acceptance Criteria:**
  - [ ] AC3.1: cargo test flow::builtins::auto_scaffold passes (4 tests).
  - [ ] AC3.2: Programmatic: after auto_scaffold::run on a planning task with config.scaffold.command set, tasks.workspace_path is populated and points at an existing directory that &#x60;git -C &lt;path&gt; rev-parse --git-common-dir&#x60; resolves successfully.
  - [ ] AC3.3: Programmatic: a second invocation does not change workspace_path (idempotent).
  - [ ] AC3.4: Programmatic: when config.yaml has no scaffold block, auto_scaffold::run returns Ok(1) and tasks.workspace_path remains empty (no panic, no status change).
- **Files:** `src/flow/builtins/auto_scaffold.rs`, `src/flow/builtins/mod.rs`, `src/flow/config.rs`, `tests/fixtures/agents.yaml`, `tests/fixtures/config.yaml`, `tests/fixtures/scaffold-noop.sh`
- **Dependencies:** Phase 2 complete (synthetic planning-arrival transition_history rows are emitted so the daemon can dispatch this builtin)
#### Phase 4: Phase 4: End-to-end ratify→promote→scaffold integration test + idempotency + full suite
- **Objective:** Land a single end-to-end integration test (Rust integration test, mirroring tests/flow_chain_isolation.rs) that exercises: confirmed observation → ratify update → auto-promote fires → tasks row at planning → auto-scaffold fires → workspace_path on disk. Verify daemon idempotency on re-run. Confirm the full project test suite (existing 647 + new) passes.
- **Tasks:**
  - Task 4.1: Create tests/flow_ratify_promote_scaffold.rs: bootstrap a fresh in-memory db with bundled schemas (pattern from tests/flow_chain_isolation.rs); insert a confirmed observation with a ready+approved intent_contract; load tests/fixtures/agents.yaml + config.yaml referencing a tempdir scaffold script; call poll_once twice (or call the builtins directly in sequence) and assert: tasks row created, status&#x3D;&#x27;planning&#x27;, linked_observations contains obs id, observations.task_id back-link set, tasks.workspace_path is a valid git worktree.
  - Task 4.2: Add a second test case in the same file: re-run poll_once a third time → no new tasks row, no second workspace_path mutation, no duplicate transition_history rows for the create or scaffold edges.
  - Task 4.3: Add a third test case: ratify-update path — start with an observation in &#x27;confirmed&#x27;, call the update handler with --invoker human writing intent_contract.contract_state&#x3D;ready + approved_by/at, assert the framework transition fires (status&#x3D;&#x27;ready&#x27;, transition_history row present) — this is the integration of Phase 1 with Phases 2-3.
  - Task 4.4: Run the full suite: cargo test --all-features. Triage any pre-existing test that depends on observations lifecycle states (e.g. observations_e2e.sh, schemas_validate_fixtures.rs) for collateral damage from adding the &#x27;ready&#x27; state; fix call sites that hard-coded the state list.
  - Task 4.5: Update CLAUDE.md and stores/observations/CLAUDE.md to document the new &#x27;ready&#x27; state and the auto-promote/auto-scaffold subscribers (single short paragraph each — no documentation sprawl).
- **Acceptance Criteria:**
  - [ ] AC4.1: cargo test --all-features passes; total count is 647 + N new tests where N &#x3D;&#x3D; sum of new tests added in Phases 1-4 (record N in PR description).
  - [ ] AC4.2: cargo test --test flow_ratify_promote_scaffold passes (3 cases).
  - [ ] AC4.3: Manual smoke (documented in PR test plan): in a scratch repo, &#x60;stores observations add --invoker human --lock-contract ...&#x60; followed by daemon poll → within 5s, &#x60;stores tasks list&#x60; shows a planning task with linked_observations and workspace_path populated; &#x60;git worktree list&#x60; shows the new feat branch.
  - [ ] AC4.4: shell e2e tests/observations_e2e.sh and tests/agents_e2e.sh still pass.
- **Files:** `tests/flow_ratify_promote_scaffold.rs`, `tests/observations_e2e.sh`, `tests/agents_e2e.sh`, `CLAUDE.md`, `stores/observations/CLAUDE.md`
- **Dependencies:** Phases 1-3 complete

---

## Plan Review

### Review 1
- **Gate:** NEEDS_WORK
- **Summary:** Plan is mostly executable with strong contract traceability and a thorough decision matrix, but Phase 2 Task 2.4 directly contradicts the decision matrix&#x27;s chosen &#x60;from_status&#x3D;&#x27;&#x27;&#x60; answer, Phase 1 Task 1.4 punts a sub-decision into a code comment instead of resolving it, and Phase 3 Task 3.4 assumes a &#x60;blocked_reason&#x60; column exists on tasks without verifying. Fix these three and ship.
- **Open Questions:**
  - Phase 2 Task 2.4 says the synthetic planning-arrival row uses &#x60;from_status&#x3D;NULL&#x60;, but Decision Matrix Q2 chose option B (empty string &#x27;&#x27;). The daemon SQL is &#x60;WHERE from_status &#x3D; ?2&#x60; so NULL would never match. Rewrite Task 2.4 to use &#x60;from_status&#x3D;&#x27;&#x27;&#x60; consistent with the matrix and AC2.x (and the agents.yaml subscription in Task 3.5 which already uses &#x60;from: &#x27;&#x27;&#x60;).
  - Phase 1 Task 1.4 says &#x27;choose a single canonical path documented in code comment&#x27; for the --lock-contract path (rewrite initial_status to &#x27;ready&#x27; vs insert at &#x27;confirmed&#x27; then auto-fire). This is a real implementation decision the planner should commit to in the decision matrix, not punt to the executor. Pick one and justify; the choice affects whether transition_history records a confirmed→ready hop on the lock-contract path (AC1.3 implies it should).
  - Phase 3 Task 3.4 plans to &#x27;write blocked_reason via a one-off direct UPDATE&#x27; on tasks. Confirm &#x60;tasks.blocked_reason&#x60; column exists in stores/tasks/schema.yaml; if not, either add a phase to introduce it or pick an existing column (e.g. note in contract.assumptions / a notes field) — otherwise the UPDATE will fail at runtime and AC3.x has no way to assert the failure-handling behavior.
- **At:** 2026-05-03T14:42:09Z

---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

