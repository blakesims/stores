# T022: Auto-drive subscriber: spawn drive cycle when task lands at planning

## Meta
- **Status:** plan_review
- **Created:** 2026-05-04T15:08:52Z
- **Last Updated:** 2026-05-04T15:17:32Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T022-auto-promoted-l048

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - agents.yaml: declare auto-drive builtin (subscribes tasks: → planning with workspace_path IS NOT NULL predicate)
- src/flow/builtins/auto_drive.rs: spawns &#x60;stores tasks drive TXXX --claude-code --invoker ai_autonomous&#x60; as detached subprocess; consumes existing drive plumbing
- .stores/config.yaml: drive.max_parallel default + override; document in agents-yaml-example.yaml
- Failure handling: drive nonzero exit OR missing wrap envelope → blocked with reason; ntfy via user-escalation
- Idempotency: dispatch_locks UNIQUE-claim respected on daemon restart
- Integration test: ratify mock observation, watch full pipeline land at in_review with wrap envelope
- **Out:** - Plan-review NEEDS_WORK loop policy (handled inside drive cycle today; revisit if friction surfaces)
- depends_on enforcement (L038&#x27;s territory; auto-drive may passive-check but not own the rule)
- Multi-host parallelism (single-daemon assumed)
- Live-tail of drive output via daemon (today&#x27;s /tmp/claude-... stream is enough)

### Done When
Ship auto-drive: a daemon subscriber that invokes &#x60;stores tasks drive&#x60; autonomously when a task lands at planning with workspace_path set, closing step 6 of the 10-step pipeline (worklog 08). After this ships, ratifying a contract triggers the planner without orchestrator round-trip — alongside T020 (auto-promote/auto-scaffold) and the auto-resolve-observation subscriber, this closes the upstream pipeline.

Acceptance:
- Ratifying any observation contract → auto-promote → auto-scaffold → auto-drive fires within ~5s of scaffold completion (one poll cycle), spawning the planner subagent without orchestrator round-trip
- Drive subprocess runs detached from the daemon polling loop; daemon continues polling other transitions while drive runs
- If drive exits non-zero or wrap envelope never lands, task transitions to blocked with blocked_reason&#x3D;drive_failed; ntfy fires via user-escalation
- Daemon restart mid-drive does NOT re-spawn (dispatch_locks UNIQUE-claim respected); idempotent
- .stores/config.yaml drive.max_parallel setting (default 1) gates concurrent drives
- Existing tests pass with --features runner-claude-code; new E2E test asserts ratify→promote→scaffold→drive→wrap chain

### Phases

#### Phase 1: Phase 1: Schema — mark_drive_failed transition + drive lifecycle columns
- **Objective:** Extend tasks schema so a framework-actor verb can collapse any pre-in_review state to &#x60;blocked&#x60; with &#x60;blocked_reason&#x60;, and reserve columns the daemon will use to track drive subprocess lifecycle.
- **Tasks:**
  - Task 1.1: In stores/tasks/schema.yaml, add framework transitions &#x60;mark_drive_failed&#x60; from each of {planning, plan_review, ready, executing, code_review} to &#x60;blocked&#x60; (actor: framework). Acceptance gate is &#x60;blocked_reason&#x60; non-empty (mirror &#x60;mark_deploy_blocked&#x60; shape).
  - Task 1.2: Add tasks fields &#x60;drive_pid&#x60; (integer, nullable) and &#x60;drive_started_at&#x60; (timestamp, nullable) to schema.yaml, alongside existing optional bookkeeping fields like &#x60;blocked_reason&#x60;. Reserved-field metadata follows existing patterns.
  - Task 1.3: Add a &#x60;fire_mark_drive_failed(conn, display_id, blocked_reason, policies_hash)&#x60; helper in src/flow/builtins/mod.rs analogous to &#x60;fire_mark_deploy_blocked&#x60;.
  - Task 1.4: Update bundled tasks DDL fixtures / golden snapshots (whatever &#x60;cargo test&#x60; currently regenerates from the schema) so existing tests continue to pass.
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo test -p stores --features runner-claude-code&#x60; passes after schema change (no test regressions).
  - [ ] AC1.2: A unit test in src/flow/builtins/mod.rs (e.g., &#x60;m_drive_failed_transition_mechanics&#x60;) inserts a task at &#x60;planning&#x60; with &#x60;workspace_path&#x60; set, calls &#x60;fire_mark_drive_failed(.., &#x27;drive_failed&#x27;, &#x27;&#x27;)&#x60;, and asserts &#x60;status&#x3D;&#x27;blocked&#x27;&#x60;, &#x60;blocked_reason&#x3D;&#x27;drive_failed&#x27;&#x60;, and &#x60;transition_history.verb&#x3D;&#x27;mark_drive_failed&#x27;&#x60; with &#x60;invoker&#x3D;&#x27;framework&#x27;&#x60;.
  - [ ] AC1.3: The same helper succeeds from each of &#x60;plan_review&#x60;, &#x60;ready&#x60;, &#x60;executing&#x60;, &#x60;code_review&#x60; source states (parameterised test).
  - [ ] AC1.4: A row already at &#x60;in_review&#x60; rejects &#x60;mark_drive_failed&#x60; (no transition declared); validation error surfaces.
- **Files:** `stores/tasks/schema.yaml`, `src/flow/builtins/mod.rs`, `src/flow/builtins/mod.rs (tests)`
#### Phase 2: Phase 2: Per-subscription predicate gate in agents.yaml
- **Objective:** Let an agent subscription declare a row-state predicate that must hold for the daemon to claim and dispatch (closes the &#x60;workspace_path IS NOT NULL&#x60; gate without polluting the policy layer or forcing a synthetic transition).
- **Tasks:**
  - Task 2.1: Extend &#x60;flow::agents_yaml::Subscription&#x60; with &#x60;#[serde(default)] predicate: Option&lt;flow::predicate::Predicate&gt;&#x60; reusing the existing predicate type from src/flow/predicate.rs.
  - Task 2.2: Update &#x60;AgentsYaml::validate()&#x60; to accept &#x60;predicate&#x60; (no extra rules — the predicate type already validates its own shape).
  - Task 2.3: In src/handlers/agents_run.rs::poll_once, after reading &#x60;row_json&#x60; and BEFORE &#x60;try_claim&#x60;, evaluate &#x60;sub.predicate&#x60; against the row. If false, &#x60;continue&#x60; (no claim, no ntfy). Order: predicate gate runs after policy &#x60;decide()&#x60; halt-check (preserves existing halt semantics).
  - Task 2.4: Add unit tests covering: predicate true → claim+dispatch; predicate false → no claim, no dispatch, no ntfy; missing predicate → existing behavior unchanged.
  - Task 2.5: Update tests/fixtures/agents.yaml schema parse test to confirm predicate-bearing entries round-trip.
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;cargo test flow::agents_yaml&#x60; passes including a new &#x60;subscription_with_predicate_parses&#x60; test that loads &#x60;predicate: { op: &#x27;!&#x3D;&#x27;, left: &#x27;$workspace_path&#x27;, right: &#x27;&#x27; }&#x60;.
  - [ ] AC2.2: &#x60;cargo test handlers::agents_run&#x60; passes including a new &#x60;predicate_false_skips_claim&#x60; test asserting &#x60;dispatch_locks&#x60; count remains 0 when the predicate evaluates false.
  - [ ] AC2.3: Existing &#x60;poll_dispatches_matching_row_once&#x60; and &#x60;concurrent_try_claim_yields_exactly_one_winner&#x60; still pass unchanged.
- **Files:** `src/flow/agents_yaml.rs`, `src/handlers/agents_run.rs`, `src/flow/predicate.rs (read-only reuse)`
- **Dependencies:** Phase 1
#### Phase 3: Phase 3: config.yaml drive.max_parallel + DriveCfg plumbing
- **Objective:** Surface a project-tunable concurrency cap for auto-drive in &#x60;.stores/config.yaml&#x60;, defaulting to 1.
- **Tasks:**
  - Task 3.1: In src/flow/config.rs, add &#x60;DriveCfg { max_parallel: u32 (default 1) }&#x60; and &#x60;StoresConfig::drive: Option&lt;DriveCfg&gt;&#x60;. Provide &#x60;resolve_drive_max_parallel(config_path) -&gt; u32&#x60; that returns 1 when unset.
  - Task 3.2: Add a unit test that a config.yaml with &#x60;drive:\n  max_parallel: 3&#x60; parses to &#x60;Some(DriveCfg { max_parallel: 3 })&#x60;, and that an absent block resolves to default 1.
  - Task 3.3: Document the field in docs/agents-yaml-example.yaml under a new &#x60;# .stores/config.yaml — supplemental&#x60; block (or wherever existing examples live).
- **Acceptance Criteria:**
  - [ ] AC3.1: &#x60;cargo test flow::config&#x60; passes with a new &#x60;parses_drive_max_parallel&#x60; test.
  - [ ] AC3.2: &#x60;resolve_drive_max_parallel&#x60; returns 1 when no config file exists.
  - [ ] AC3.3: docs/agents-yaml-example.yaml mentions &#x60;drive.max_parallel&#x60; with the default.
- **Files:** `src/flow/config.rs`, `docs/agents-yaml-example.yaml`
#### Phase 4: Phase 4: builtin:auto-drive — spawn detached drive subprocess
- **Objective:** Ship src/flow/builtins/auto_drive.rs that spawns &#x60;stores tasks drive &lt;id&gt; --claude-code --invoker ai_autonomous&#x60; as a detached subprocess, records the PID + start time on the tasks row, and respects &#x60;drive.max_parallel&#x60;.
- **Tasks:**
  - Task 4.1: Create src/flow/builtins/auto_drive.rs with &#x60;pub fn run(row, ctx) -&gt; BuiltinResult&#x60;. Read &#x60;display_id&#x60;, &#x60;workspace_path&#x60; from row. If &#x60;workspace_path&#x60; empty → log + return Ok(1) (defensive — Phase 2 predicate should already gate).
  - Task 4.2: Idempotency: if &#x60;tasks.drive_pid&#x60; is already set AND &#x60;kill(pid, 0)&#x60; succeeds → no-op return Ok(0). (Daemon-restart-mid-drive case.) If PID stored but process dead AND status !&#x3D; in_review → fall through to watchdog (Phase 5) — here, just return Ok(0) without re-spawning.
  - Task 4.3: Concurrency cap: count &#x60;dispatch_locks&#x60; rows with &#x60;agent_name&#x3D;&#x27;auto-drive&#x27; AND finished_at IS NULL&#x60; whose &#x60;drive_pid&#x60; is alive (kill -0). If count &gt;&#x3D; &#x60;drive.max_parallel&#x60;, return Ok(0) without spawning (the lock will retry next poll? No — claim is already taken). Decision: instead, the cap check happens before &#x60;try_claim&#x60; — see Task 4.5.
  - Task 4.4: Spawn the drive subprocess. Use double-fork detach pattern (mirror &#x60;agents_run::detach_process&#x60;): fork → setsid → fork → exec &#x60;stores tasks drive &lt;id&gt; --claude-code --invoker ai_autonomous&#x60; with cwd&#x3D;&#x60;workspace_path&#x60;, stdout/stderr redirected to &#x60;&lt;workspace_path&gt;/.stores/logs/drive-&lt;id&gt;-&lt;ts&gt;.log&#x60;. Parent records the grandchild PID into &#x60;tasks.drive_pid&#x60; and &#x60;tasks.drive_started_at&#x60; via UPDATE.
  - Task 4.5: The &#x60;drive.max_parallel&#x60; cap must be enforced BEFORE the daemon claims. Move the cap check into a pre-claim hook in &#x60;poll_once&#x60;: when &#x60;agent.command &#x3D;&#x3D; &#x27;builtin:auto-drive&#x27;&#x60; and current live-drive count &gt;&#x3D; cap, skip claim+continue. Document the special-case in a comment.
  - Task 4.6: Register the keyword in &#x60;flow::builtins::dispatch_builtin&#x60; (&quot;auto-drive&quot; arm).
  - Task 4.7: Unit tests with a fake &#x60;stores&#x60; binary on PATH (or a scriptable &#x60;STORES_DRIVE_CMD&#x60; env override): verify spawn happens, PID is recorded, and a re-run is a no-op when PID is alive.
- **Acceptance Criteria:**
  - [ ] AC4.1: &#x60;cargo test flow::builtins::auto_drive&#x60; passes including: (i) spawn happens and tasks.drive_pid &gt; 0; (ii) re-run with live PID is a no-op (no second spawn); (iii) re-run with dead PID does NOT re-spawn (returns Ok(0)).
  - [ ] AC4.2: With &#x60;drive.max_parallel: 1&#x60; and one drive already running, a second auto-drive dispatch is skipped (no claim, dispatch_locks count for auto-drive remains 1).
  - [ ] AC4.3: Spawned process is reparented to PID 1 (orphaned from daemon) — assertable by checking &#x60;getppid()&#x60; from the child via test harness wrapping a stub binary.
  - [ ] AC4.4: &#x60;dispatch_builtin(&quot;auto-drive&quot;, ..)&#x60; resolves to the new module.
- **Files:** `src/flow/builtins/auto_drive.rs`, `src/flow/builtins/mod.rs`, `src/handlers/agents_run.rs`
- **Dependencies:** Phase 1, Phase 2, Phase 3
#### Phase 5: Phase 5: drive watchdog sweep — fail tasks whose drive died without producing wrap
- **Objective:** On every daemon poll iteration, sweep live auto-drive locks; for any whose PID is dead and whose task is not at &#x60;in_review&#x60;, fire &#x60;mark_drive_failed&#x60; with &#x60;blocked_reason&#x3D;&#x27;drive_failed&#x27;&#x60; and dispatch to the configured &#x60;deployment_specialist&#x60; (default &#x60;builtin:user-escalation&#x60;) so ntfy fires.
- **Tasks:**
  - Task 5.1: Add &#x60;pub fn sweep_drive_watchdog(conn, agents, config_path, policies_hash) -&gt; Result&lt;usize&gt;&#x60; in src/flow/builtins/auto_drive.rs (or a sibling module). Selects &#x60;dispatch_locks WHERE agent_name&#x3D;&#x27;auto-drive&#x27; AND finished_at IS NULL&#x60;. For each, read tasks row.
  - Task 5.2: For each open lock: if &#x60;kill(drive_pid, 0)&#x60; fails (ESRCH) → drive subprocess is dead. Read tasks.status. If &#x60;status &#x3D;&#x3D; &#x27;in_review&#x27;&#x60; → drive succeeded; just &#x60;mark_claim_finished&#x60; with status&#x3D;&#x27;ok&#x27;. Otherwise → call &#x60;fire_mark_drive_failed(conn, display_id, &#x27;drive_failed&#x27;, policies_hash)&#x60;, then &#x60;dispatch_to_specialist(row, ctx, display_id, &#x27;auto-drive-watchdog&#x27;)&#x60; (this routes to &#x60;builtin:user-escalation&#x60; which files the observation + ntfy fires via existing plumbing).
  - Task 5.3: Hook the watchdog into &#x60;agents_run::poll_once&#x60; — call once per iteration after the per-agent loop finishes.
  - Task 5.4: Unit tests: (i) live PID + planning state → no flip; (ii) dead PID + planning → row&#x3D;blocked, blocked_reason&#x3D;&#x27;drive_failed&#x27;, user-escalation observation filed, ntfy event captured via MockNotifier; (iii) dead PID + status&#x3D;&#x27;in_review&#x27; → no flip, lock marked finished&#x3D;&#x27;ok&#x27;.
- **Acceptance Criteria:**
  - [ ] AC5.1: &#x60;cargo test flow::builtins::auto_drive::watchdog&#x60; passes the three scenarios above.
  - [ ] AC5.2: After a simulated drive death at &#x60;executing&#x60;, sweep flips row to &#x60;blocked&#x60; with &#x60;blocked_reason&#x3D;&#x27;drive_failed&#x27;&#x60; AND a single observation row exists with &#x60;task_id&#x60; pointing back at the task.
  - [ ] AC5.3: MockNotifier captures exactly one event with &#x60;transition_attempted&#x60; containing &#x27;blocked&#x27; for the failed task.
  - [ ] AC5.4: Daemon-restart simulation: spawn drive, kill subprocess, drop+rebuild Connection, run sweep — flip still fires (idempotency: dispatch_locks UNIQUE-claim respected; sweep keys off the persisted lock row).
- **Files:** `src/flow/builtins/auto_drive.rs`, `src/handlers/agents_run.rs`
- **Dependencies:** Phase 1, Phase 4
#### Phase 6: Phase 6: Wire auto-drive into agents.yaml fixtures + example
- **Objective:** Register the auto-drive entry in the production-shape fixture (used by E2E tests) and the user-facing example so downstream projects can copy it.
- **Tasks:**
  - Task 6.1: Append auto-drive to tests/fixtures/agents.yaml: &#x60;subscribes_to: [{ store: tasks, transition: { from: &#x27;&#x27;, to: planning }, predicate: { op: &#x27;!&#x3D;&#x27;, left: &#x27;$workspace_path&#x27;, right: &#x27;&#x27; } }]&#x60;, &#x60;command: builtin:auto-drive&#x60;, &#x60;retry_policy: { max_attempts: 1 }&#x60;.
  - Task 6.2: Mirror in docs/agents-yaml-example.yaml with surrounding comments explaining the predicate gate and &#x60;drive.max_parallel&#x60;.
  - Task 6.3: Update the existing &#x60;fixture_yaml_includes_t020_builtins&#x60; test in src/flow/agents_yaml.rs to ALSO assert auto-drive presence with the predicate (or add a sibling &#x60;fixture_yaml_includes_auto_drive&#x60; test).
- **Acceptance Criteria:**
  - [ ] AC6.1: &#x60;cargo test flow::agents_yaml&#x60; passes the new &#x60;fixture_yaml_includes_auto_drive&#x60; assertion.
  - [ ] AC6.2: tests/fixtures/agents.yaml round-trips through &#x60;load_from_path&#x60; with no errors.
  - [ ] AC6.3: docs/agents-yaml-example.yaml documents both the agents.yaml entry AND the &#x60;drive.max_parallel&#x60; config setting.
- **Files:** `tests/fixtures/agents.yaml`, `docs/agents-yaml-example.yaml`, `src/flow/agents_yaml.rs (test only)`
- **Dependencies:** Phase 4
#### Phase 7: Phase 7: E2E integration test — ratify→promote→scaffold→drive→wrap
- **Objective:** Extend or clone tests/flow_promote_scaffold_e2e.rs with a test that walks the full pipeline using a mock drive binary (a shell script that emits the expected wrap envelope sequence and exits 0), asserting the row lands at &#x60;in_review&#x60; within ~5s of scaffold completion.
- **Tasks:**
  - Task 7.1: Add tests/flow_promote_scaffold_drive_e2e.rs. Reuse the harness from flow_promote_scaffold_e2e.rs for ratify+promote+scaffold setup.
  - Task 7.2: Stub the drive binary: write a small shell script to a tempdir that, when invoked as &#x60;stores tasks drive T001 --claude-code --invoker ai_autonomous&#x60;, walks the row through planning→plan_review→ready→executing→code_review→in_review by directly invoking the in-process &#x60;compute_submit_*&#x60; handlers (call into the lib crate, not a real &#x60;stores&#x60; binary). Have auto-drive&#x27;s spawn use a &#x60;STORES_DRIVE_CMD&#x60; override pointing at this script.
  - Task 7.3: Run &#x60;poll_once&#x60; repeatedly (with sleeps) until task status reaches &#x60;in_review&#x60; OR a 30s wall-clock timeout. Assert: status&#x3D;&#x3D;in_review, dispatch_locks for auto-drive has finished_at non-null with last_status&#x3D;&#x27;ok&#x27;.
  - Task 7.4: Failure-path test: stub the drive binary to exit 1 without producing wrap. Run sweep. Assert row at &#x60;blocked&#x60;, blocked_reason&#x3D;&#x27;drive_failed&#x27;, observation row filed, ntfy MockNotifier captured the event.
  - Task 7.5: Idempotency test: kill the drive subprocess while running, restart-simulate the daemon (drop conn, rebuild). Run poll_once once — assert auto-drive does NOT re-spawn (dispatch_locks UNIQUE-claim respected) and watchdog flips to blocked.
- **Acceptance Criteria:**
  - [ ] AC7.1: &#x60;cargo test --test flow_promote_scaffold_drive_e2e --features runner-claude-code&#x60; passes the happy-path test.
  - [ ] AC7.2: Wall-clock from observation &#x60;confirmed→ready&#x60; to task &#x60;in_review&#x60; is &lt; 30s in the test (drive stub is instant; the chain is ~3 poll iterations).
  - [ ] AC7.3: Failure-path test asserts blocked + drive_failed + observation filed + ntfy event.
  - [ ] AC7.4: Idempotency test asserts no double-spawn after simulated restart.
- **Files:** `tests/flow_promote_scaffold_drive_e2e.rs`, `tests/fixtures/agents.yaml (read-only consumer)`
- **Dependencies:** Phase 1, Phase 2, Phase 3, Phase 4, Phase 5, Phase 6

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

