# T020: auto-promote + auto-scaffold subscribers (upstream-autonomy unlock)

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T14:35:57Z
- **Last Updated:** 2026-05-03T15:00:16Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
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

#### Phase 1: Phase 1: observations &#x27;ready&#x27; state + framework ratify transition
- **Objective:** Add a &#x27;ready&#x27; state to observations.lifecycle plus a framework-actor confirmed→ready transition (verb&#x3D;ratify), guarded by intent_contract.contract_state&#x3D;&#x3D;&#x27;ready&#x27; AND approved_by/approved_at being set. This creates the substrate-level &#x27;contract ratified&#x27; event that auto-promote will subscribe to.
- **Tasks:**
  - Task 1.1: In stores/observations/schema.yaml, add &#x27;ready&#x27; to lifecycle.states (append after &#x27;confirmed&#x27;).
  - Task 1.2: In stores/observations/schema.yaml, add transition {from: confirmed, to: ready, verb: ratify, actor: framework, guard: &quot;intent_contract.contract_state &#x3D;&#x3D; &#x27;ready&#x27; &amp;&amp; intent_contract.approved_by !&#x3D; null &amp;&amp; intent_contract.approved_at !&#x3D; null&quot;}.
  - Task 1.3: Add a follow-on hook in src/handlers/transition.rs (or a new helper) so that immediately after a successful &#x27;confirm&#x27; transition on observations, if the row&#x27;s intent_contract is ready+approved, the framework synchronously fires &#x27;ratify&#x27; (confirmed→ready) inside the same outer call. Use the existing fire_framework_transition pattern from src/flow/builtins/mod.rs as the template, but generalize/copy it to operate on the observations schema (load via BUNDLED_STORE_SCHEMAS).
  - Task 1.4: Add unit tests in src/handlers/transition.rs (or a sibling tests module) covering: (a) confirm with contract ready+approved fires ratify and lands at &#x27;ready&#x27;, writing TWO transition_history rows (investigating→confirmed and confirmed→ready); (b) confirm with contract ready but missing approved_at does NOT auto-ratify (row stays at confirmed); (c) ratify is rejected when invoked directly by a non-framework actor.
- **Acceptance Criteria:**
  - [ ] AC1.1: cargo build succeeds.
  - [ ] AC1.2: schemas_validate_fixtures test passes (observations schema validates with the new state and transition).
  - [ ] AC1.3: New unit test &#x27;confirm_with_ready_contract_auto_ratifies&#x27; passes; transition_history contains both rows in order (investigating→confirmed then confirmed→ready, both with verb&#x3D;&#x27;confirm&#x27; for the first, &#x27;ratify&#x27; for the second, framework invoker on the second).
  - [ ] AC1.4: New unit test &#x27;confirm_without_approval_does_not_auto_ratify&#x27; passes (row stays at &#x27;confirmed&#x27;, only one transition_history row).
  - [ ] AC1.5: cargo test --workspace passes (no regressions in existing 647 tests).
- **Files:** `stores/observations/schema.yaml`, `src/handlers/transition.rs`, `src/flow/builtins/mod.rs`
#### Phase 2: Phase 2: lock-contract direct ratification + tasks planning-arrival synthetic transition
- **Objective:** Make &#x27;observations add --lock-contract&#x27; land the row directly at &#x27;ready&#x27; with both confirmed→ready and (open→investigating→confirmed) transitions recorded in transition_history. Also emit a synthetic from_status&#x3D;&#x27;&#x27; to_status&#x3D;initial_state row in transition_history on every successful add for any store, so subscribers can subscribe to row-creation events (this is the &#x27;planning-arrival&#x27; convention for tasks).
- **Tasks:**
  - Task 2.1: In src/handlers/add.rs, after the row INSERT and display_id resolution, when --lock-contract is set on observations: invoke a helper that walks the row from &#x27;open&#x27; through &#x27;investigating&#x27; → &#x27;confirmed&#x27; → &#x27;ready&#x27; by writing three transition_history rows (verbs: investigate, confirm, ratify; invoker propagated for confirm; framework for the synthetic walk + ratify) and updates the observations.status column to &#x27;ready&#x27; inside the same transaction.
  - Task 2.2: Justify-and-implement choice (Decision Matrix Q1): &#x27;insert at confirmed then framework auto-fires ratify&#x27; — concretely: lock-contract sets the row&#x27;s status to &#x27;confirmed&#x27; at INSERT (override initial_status for this code path only), then fires the same auto-ratify hook from Phase 1 (Task 1.3). Both &#x27;open→...→confirmed&#x27; synthetic markers and the &#x27;confirmed→ready&#x27; framework row land in transition_history. Document the reason in a 1-line code comment that points at decision matrix row Q1.
  - Task 2.3: In src/handlers/add.rs, after the INSERT (for ALL stores, not just observations), insert a synthetic transition_history row with from_status&#x3D;&#x27;&#x27; (empty string), to_status&#x3D;&lt;initial_status&gt;, verb&#x3D;&#x27;create&#x27;, invoker&#x3D;invoker.actor.to_string(). Use crate::db::insert_transition_history; do NOT use NULL — Decision Matrix Q2 chose &#x27;&#x27; so the daemon&#x27;s WHERE from_status &#x3D; ?2 SQL matches it.
  - Task 2.4: Add unit tests in src/handlers/add.rs covering: (a) tasks add inserts exactly one transition_history row with from_status&#x3D;&#x27;&#x27; and to_status&#x3D;&#x27;planning&#x27; and verb&#x3D;&#x27;create&#x27;; (b) observations add (no lock-contract) inserts one row from_status&#x3D;&#x27;&#x27; to_status&#x3D;&#x27;open&#x27;; (c) observations add --lock-contract --invoker human inserts the synthetic create row PLUS confirmed→ready transition rows (final status&#x3D;&#x27;ready&#x27;); (d) observations add --lock-contract --invoker ai_autonomous still rejects (existing behavior preserved).
- **Acceptance Criteria:**
  - [ ] AC2.1: cargo build succeeds.
  - [ ] AC2.2: &#x27;tasks_add_emits_planning_arrival&#x27; unit test passes; transition_history row exists with from_status&#x3D;&#x27;&#x27; (empty string, NOT NULL), to_status&#x3D;&#x27;planning&#x27;, verb&#x3D;&#x27;create&#x27;, store&#x3D;&#x27;tasks&#x27;.
  - [ ] AC2.3: &#x27;lock_contract_lands_at_ready&#x27; unit test passes; observations row final status&#x3D;&#x27;ready&#x27;; transition_history contains the create row PLUS a confirmed→ready row with verb&#x3D;&#x27;ratify&#x27; and invoker&#x3D;&#x27;framework&#x27;.
  - [ ] AC2.4: AC1.5 still passes (existing 647 tests green). Specifically, the existing observations_e2e.sh and tasks_e2e.sh continue to pass; any tests that count transition_history rows must be updated to expect the new synthetic create row, with the change called out in the executor&#x27;s commit message.
  - [ ] AC2.5: SQL grep proves consistency: rg &quot;from_status\s*&#x3D;\s*\?2&quot; src/handlers/agents_run.rs and the synthetic insert in add.rs both use empty-string convention (no NULL). Documented in a comment above the synthetic insert.
- **Files:** `src/handlers/add.rs`, `src/db.rs`, `tests/observations_e2e.sh`, `tests/tasks_e2e.sh`
- **Dependencies:** Phase 1 must be complete (the auto-ratify hook from Task 1.3 is reused here).
#### Phase 3: Phase 3: builtin:auto-promote — observations confirmed→ready creates tasks row
- **Objective:** Implement src/flow/builtins/auto_promote.rs as a builtin subscriber. On observations confirmed→ready, read the observation&#x27;s intent_contract, create a tasks row at &#x27;planning&#x27; with linked_observations populated and contract.{done_when,scope_in,scope_out} mapped from the observation&#x27;s intent_contract, and back-link observation.task_id to the new task. Idempotent: if observation.task_id is already set to an existing tasks row, skip.
- **Tasks:**
  - Task 3.1: Create src/flow/builtins/auto_promote.rs with a &#x60;pub fn run(row: &amp;Value, ctx: &amp;DispatchCtx) -&gt; BuiltinResult&#x60; entry point matching the accept_merge.rs / cargo_install.rs pattern. Read display_id, intent_contract sub-fields (objective, type, in_scope[], out_of_scope[], acceptance[], tier_hint), and existing task_id from the observation row JSON.
  - Task 3.2: Idempotency guard: if observation.task_id is non-empty AND a tasks row with that display_id exists in the same DB, log &#x27;[auto-promote] {obs}: already promoted to {task_id}; skipping&#x27; and return Ok(0).
  - Task 3.3: Mapping (Decision Matrix Q3): tasks.contract.done_when &#x3D; obs.intent_contract.objective + (if acceptance non-empty: &#x27;\n\nAcceptance:\n- &#x27; + join(acceptance, &#x27;\n- &#x27;)); tasks.contract.scope_in &#x3D; join(in_scope, &#x27;\n- &#x27;) with leading &#x27;- &#x27;; tasks.contract.scope_out &#x3D; join(out_of_scope, &#x27;\n- &#x27;) with leading &#x27;- &#x27;; tasks.tier_hint &#x3D; obs.intent_contract.tier_hint; tasks.title &#x3D; obs.summary; tasks.slug &#x3D; derived from obs.display_id (e.g. &#x27;auto-promoted-L042&#x27;) unless obs.intent_contract has a known_solution-like slug hint — for Phase 3 use &#x27;L042-auto-promoted&#x27; template (see Q4); tasks.linked_observations &#x3D; [obs.display_id].
  - Task 3.4: Create the tasks row by calling crate::handlers::add::run_programmatic (extract a programmatic helper if one doesn&#x27;t exist; otherwise build the entry map and call validate + INSERT directly mirroring add.rs:200-310). Invoker for this write is ai_autonomous (the substrate is autonomously promoting an already-ratified contract; no U-moment is created here — the U-moment was the human&#x27;s ratify in Phase 1). Capture the new task display_id from the INSERT.
  - Task 3.5: After successful tasks insert, UPDATE observations SET task_id &#x3D; &lt;new_T###&gt;, updated_at &#x3D; now WHERE display_id &#x3D; &lt;obs_display_id&gt;. Use a direct UPDATE (not a transition) since task_id is a non-status field with no actor restriction. Wrap the tasks-insert + obs-update in a single rusqlite transaction so partial-failure leaves nothing behind.
  - Task 3.6: On failure (validation error, INSERT error, mapping error): return Ok(1) AND emit a stderr log with the obs display_id and root cause. Do NOT flip the observation to wont_fix or any other state — recovery from auto-promote failure is out of scope (see contract scope_out); operator intervention is the path forward.
  - Task 3.7: Wire the builtin into src/flow/builtins/mod.rs::dispatch_builtin (add &#x27;auto-promote&#x27; &#x3D;&gt; Some(auto_promote::run(row, ctx)) match arm and pub mod auto_promote;).
  - Task 3.8: Unit tests in src/flow/builtins/auto_promote.rs covering: (a) successful promote creates tasks row with linked_observations&#x3D;[&#x27;L001&#x27;] and contract fields populated from obs.intent_contract; (b) idempotent re-run is a no-op (no second tasks row created, log line emitted); (c) obs without ready+approved contract is rejected (mapper returns Ok(1) — but in practice the daemon won&#x27;t dispatch since the transition hasn&#x27;t fired).
- **Acceptance Criteria:**
  - [ ] AC3.1: cargo build succeeds.
  - [ ] AC3.2: auto_promote unit test &#x27;promote_creates_task&#x27; passes — new tasks row exists with status&#x3D;&#x27;planning&#x27;, linked_observations contains the source obs id, contract.done_when contains the obs.intent_contract.objective text, observation.task_id is back-linked.
  - [ ] AC3.3: auto_promote unit test &#x27;promote_is_idempotent&#x27; passes — second invocation creates no new tasks row; total tasks count stays at 1.
  - [ ] AC3.4: dispatch_builtin returns Some for keyword &#x27;auto-promote&#x27; (asserted via existing dispatch test pattern).
  - [ ] AC3.5: AC1.5 + AC2.4 still pass.
- **Files:** `src/flow/builtins/auto_promote.rs`, `src/flow/builtins/mod.rs`, `src/handlers/add.rs`
- **Dependencies:** Phase 1 (the confirmed→ready transition exists and fires) and Phase 2 (the daemon must see the transition in transition_history) must be complete.
#### Phase 4: Phase 4: builtin:auto-scaffold — tasks planning-arrival creates worktree + writes workspace_path
- **Objective:** Implement src/flow/builtins/auto_scaffold.rs subscribing to the synthetic planning-arrival edge (tasks: &#x27;&#x27;→planning). It reads .stores/config.yaml&#x27;s scaffold.command template, runs it with {display_id}/{slug}/{branch} substitutions, parses the last stdout line as the absolute worktree path, and updates tasks.workspace_path. Idempotent: if workspace_path is already set and the path exists as a directory, skip.
- **Tasks:**
  - Task 4.1: Extend src/flow/config.rs StoresConfig with an optional scaffold field: &#x60;pub scaffold: Option&lt;ScaffoldCfg&gt;&#x60; where &#x60;ScaffoldCfg { command: String }&#x60;. Add a unit test that parses &#x27;scaffold:\n  command: &quot;./dev scaffold {display_id}&quot;\n&#x27; correctly.
  - Task 4.2: Create src/flow/builtins/auto_scaffold.rs with &#x60;pub fn run(row, ctx)&#x60;. Read display_id, slug, branch, workspace_path from the tasks row JSON.
  - Task 4.3: Idempotency guard: if workspace_path is non-empty AND PathBuf::from(workspace_path).is_dir(), log &#x27;[auto-scaffold] {T###}: workspace_path already set and exists; skipping&#x27; and return Ok(0).
  - Task 4.4: Load StoresConfig via crate::flow::config::load(ctx.config_path). If scaffold is None, log &#x27;[auto-scaffold] {T###}: no scaffold.command configured; skipping&#x27; and return Ok(0) (this is not a failure — projects without scaffolding stay manual).
  - Task 4.5: Substitute {display_id}, {slug}, {branch} placeholders in the configured command. Run via &#x60;Command::new(&quot;sh&quot;).arg(&quot;-c&quot;).arg(&amp;substituted).output()&#x60;. On non-zero exit, log full stderr tail and write the failure note (NOT into blocked_reason — leave the row at planning; Decision Matrix Q5: scaffold failures surface via stderr only, recovery is out of scope per contract).
  - Task 4.6: Parse the worktree path: take the LAST non-empty line of stdout (stripped). Verify it canonicalizes to an existing directory; if not, log error and return Ok(1).
  - Task 4.7: Update tasks SET workspace_path &#x3D; &lt;path&gt;, updated_at &#x3D; now WHERE display_id &#x3D; &lt;T###&gt;. Use a direct UPDATE (workspace_path has no actor restriction in stores/tasks/schema.yaml).
  - Task 4.8: Wire &#x27;auto-scaffold&#x27; &#x3D;&gt; auto_scaffold::run into src/flow/builtins/mod.rs::dispatch_builtin.
  - Task 4.9: Unit tests using a tempdir + a stub scaffold command (e.g. &#x60;mkdir -p /tmp/X &amp;&amp; echo /tmp/X&#x60;): (a) successful scaffold writes workspace_path; (b) idempotent re-run skips when the dir exists; (c) missing scaffold.command returns Ok(0) with no row mutation; (d) scaffold command failure returns Ok(1) and leaves workspace_path unset.
- **Acceptance Criteria:**
  - [ ] AC4.1: cargo build succeeds.
  - [ ] AC4.2: auto_scaffold::tests::scaffold_writes_workspace_path passes.
  - [ ] AC4.3: auto_scaffold::tests::scaffold_is_idempotent passes (running twice produces a single workspace_path write, second run logs the skip line).
  - [ ] AC4.4: dispatch_builtin(&#x27;auto-scaffold&#x27;, ...) returns Some.
  - [ ] AC4.5: AC3.5 still holds.
- **Files:** `src/flow/builtins/auto_scaffold.rs`, `src/flow/builtins/mod.rs`, `src/flow/config.rs`
- **Dependencies:** Phase 2 (the synthetic &#x27;&#x27;→planning row must be in transition_history for the daemon to dispatch) and Phase 1 (chain start) must be complete.
#### Phase 5: Phase 5: agents.yaml subscribers + bundled fixture + ntfy on failure
- **Objective:** Add the auto-promote and auto-scaffold subscribers to the bundled agents.yaml fixture used by the daemon and tests, with correct subscribes_to entries and retry policies. Update tests/fixtures/agents.yaml as the example.
- **Tasks:**
  - Task 5.1: Update tests/fixtures/agents.yaml: append two AgentEntry blocks — name&#x3D;auto-promote subscribing to {store: observations, transition: {from: confirmed, to: ready}}, command &#x27;builtin:auto-promote&#x27;, claim_window_secs: 300, retry_policy: {max_attempts: 1, backoff: linear}; name&#x3D;auto-scaffold subscribing to {store: tasks, transition: {from: &#x27;&#x27;, to: planning}}, command &#x27;builtin:auto-scaffold&#x27;, claim_window_secs: 300, retry_policy: {max_attempts: 1, backoff: linear}.
  - Task 5.2: Verify the empty-string from is accepted by AgentsYaml::validate (in src/flow/agents_yaml.rs) — the existing validator bails on &#x60;sub.transition.from.is_empty()&#x60;. Relax that guard to accept empty-string from (representing row-creation arrival) but keep the &#x27;to&#x27; must-be-non-empty check. Update the validator&#x27;s unit tests accordingly.
  - Task 5.3: Add a parse-test in src/flow/agents_yaml.rs that loads the new fixture and asserts both new agents are present and resolve correctly.
  - Task 5.4: Document the new builtins and the from&#x3D;&#x27;&#x27; convention in docs/philosophy.md or a dedicated subsystem README. (1-2 paragraphs only — doc is not the deliverable; substrate behavior is.)
- **Acceptance Criteria:**
  - [ ] AC5.1: tests/fixtures/agents.yaml parses cleanly via AgentsYaml::from_yaml; new agentry parse-test passes.
  - [ ] AC5.2: validator unit test &#x27;empty_from_status_is_allowed&#x27; passes (was previously rejected).
  - [ ] AC5.3: validator unit test &#x27;empty_to_status_still_rejected&#x27; passes (the &#x27;to&#x27; invariant is preserved).
  - [ ] AC5.4: AC4.5 still holds.
- **Files:** `tests/fixtures/agents.yaml`, `src/flow/agents_yaml.rs`, `docs/philosophy.md`
- **Dependencies:** Phases 3 and 4 must define the builtins this YAML references.
#### Phase 6: Phase 6: end-to-end ratify→promote→scaffold integration test
- **Objective:** Add tests/flow_promote_scaffold_e2e.rs (Rust integration test, mirroring tests/flow_chain_isolation.rs structure) that drives the full chain on a tempdir repo: create observation with --lock-contract → assert ready landing → run poll_once with the new agents.yaml → assert tasks row exists at planning with linked_observations + back-link → run poll_once again → assert workspace_path is populated and the worktree exists. Also assert idempotency: second poll_once is a no-op.
- **Tasks:**
  - Task 6.1: Create tests/flow_promote_scaffold_e2e.rs. Set up a tempdir + git init + cargo crate fixture (reuse tests/flow_chain_isolation.rs::copy_dir + setup helper).
  - Task 6.2: Write a stub scaffold command (a shell one-liner stored in .stores/config.yaml inside the tempdir): &#x60;command: &quot;mkdir -p $REPO/wt-{display_id} &amp;&amp; cd $REPO &amp;&amp; git worktree add -b feat/{display_id}-{slug} wt-{display_id} &amp;&amp; echo $REPO/wt-{display_id}&quot;&#x60; (the {display_id}/{slug} substitutions and stdout-final-line parsing are exercised here).
  - Task 6.3: Programmatically (rust, not shell) invoke &#x60;stores observations add --lock-contract --invoker human&#x60; with a complete intent_contract via crate::handlers::add::run on the in-tempdir DB. Assert the obs row lands at status&#x3D;&#x27;ready&#x27; and transition_history has the confirmed→ready row.
  - Task 6.4: Invoke crate::handlers::agents_run::poll_once with the new agents.yaml (loaded from tests/fixtures/agents.yaml). Assert dispatched count &gt;&#x3D; 1 (auto-promote fired).
  - Task 6.5: Re-read DB: assert tasks row exists with display_id starting &#x27;T&#x27;, status&#x3D;&#x27;planning&#x27;, linked_observations contains the obs id, contract.done_when contains the obs.intent_contract.objective text. Assert observation.task_id back-link is set.
  - Task 6.6: Invoke poll_once a second time (auto-scaffold subscribes to the synthetic &#x27;&#x27;→planning row created by add). Assert tasks.workspace_path is populated, points at an existing directory inside the tempdir, and the directory contains a .git file (worktree marker).
  - Task 6.7: Invoke poll_once a third time. Assert NO additional tasks rows are created (auto-promote idempotent) and NO additional workspace_path mutations occur (auto-scaffold idempotent — log line about skip should be printable but not asserted on for portability).
  - Task 6.8: Use tempfile::tempdir() so cleanup is automatic. Use a unique git config user to avoid system-level pollution.
- **Acceptance Criteria:**
  - [ ] AC6.1: cargo test --test flow_promote_scaffold_e2e passes.
  - [ ] AC6.2: The test asserts within ~5s wall-clock that ratification → tasks row at planning happens (assert via Instant::now() before poll_once and after, max 5s).
  - [ ] AC6.3: The test asserts workspace_path points at an existing directory inside the tempdir.
  - [ ] AC6.4: cargo test --workspace passes (647 + new tests, all green).
  - [ ] AC6.5: The test cleanly tears down the tempdir on both pass and panic paths (tempfile::tempdir() handles the latter).
- **Files:** `tests/flow_promote_scaffold_e2e.rs`
- **Dependencies:** Phases 1-5 must all be complete and merged.

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

