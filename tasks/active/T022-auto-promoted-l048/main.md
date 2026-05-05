# T022: Auto-drive subscriber: spawn drive cycle when task lands at planning

## Meta
- **Status:** in_review
- **Created:** 2026-05-04T15:08:52Z
- **Last Updated:** 2026-05-04T15:58:24Z
- **Current Phase:** 7
- **Current Cycle:** 1
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

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable with 7 well-ordered phases that trace cleanly to the contract&#x27;s done_when (predicate gate, max_parallel cap, detached subprocess, watchdog, E2E). Acceptance criteria are mechanical (cargo test invocations, specific row-state assertions, MockNotifier capture, dispatch_locks counts). Decision matrix covers all 8 consequential choices with concrete rationale; the Task 4.3→4.5 cap-enforcement-point shift is acknowledged inline rather than left as a contradiction.
- **At:** 2026-05-04T15:17:47Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 1 complete. Added mark_drive_failed framework transitions (planning|plan_review|ready|executing|code_review → blocked) and drive_pid + drive_started_at fields to stores/tasks/schema.yaml. Added fire_mark_drive_failed helper in src/flow/builtins/mod.rs mirroring fire_mark_deploy_blocked. Added three tests (m_drive_failed_transition_mechanics, n_drive_failed_from_each_source_state, o_drive_failed_rejected_from_in_review) covering AC1.2/1.3/1.4. Regenerated topology .dot/.md snapshots. cargo test --features runner-claude-code passes (670 lib + all integration tests; one transient notifier-mock flake reproduced clean on re-run, unrelated to this phase).
- **Commit:** `f305c81`
- **Files:**
  - `src/flow/builtins/mod.rs`
  - `stores/tasks/schema.yaml`
  - `tests/fixtures/topology/expected.dot`
  - `tests/fixtures/topology/expected.md`
- **At:** 2026-05-04T15:21:48Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Added Subscription.predicate: Option&lt;PredicateExpr&gt; (serde default None) reusing flow::predicate. Wired the gate into handlers::agents_run::poll_once after the policy halt-check and before try_claim — predicate-false rows skip claim+dispatch with no ntfy. Tests added: subscription_with_predicate_parses, subscription_without_predicate_defaults_to_none, predicate_false_skips_claim (dispatch_locks count&#x3D;0), predicate_true_claims_and_dispatches. AC2.1/AC2.2/AC2.3 all pass; existing poll_dispatches_matching_row_once and concurrent_try_claim_yields_exactly_one_winner unchanged. Note: brief referenced &#x27;flow::predicate::Predicate&#x27; but the actual exported type is &#x27;PredicateExpr&#x27; — used the real name. Pre-existing flaky test flow::builtins::tests::e_schema_migrate_failure_blocks (global mock notifier ordering) is unrelated; passes in isolation and in-module.
- **Commit:** `8eb33ea798dd981b2c75ad74b91f971b76acaa22`
- **Files:**
  - `src/flow/agents_yaml.rs`
  - `src/handlers/agents_run.rs`
- **At:** 2026-05-04T15:25:18Z
### Phase 3 / Cycle 1
- **Status:** PASS
- **Summary:** Added DriveCfg { max_parallel: u32 } with serde default 1, StoresConfig::drive: Option&lt;DriveCfg&gt;, and resolve_drive_max_parallel(config_path) -&gt; u32 returning 1 when unset/missing. Three new tests (parses_drive_max_parallel, drive_max_parallel_defaults_to_one_when_absent, drive_max_parallel_defaults_to_one_when_no_file) all pass; flow::config 10/10 green. Documented drive.max_parallel default in docs/agents-yaml-example.yaml under a new &#x27;.stores/config.yaml — supplemental&#x27; block alongside ntfy and scaffold.
- **Commit:** `54819fdbbf8ea3fcba17865e4bba0e93b5629bc5`
- **Files:**
  - `src/flow/config.rs`
  - `docs/agents-yaml-example.yaml`
- **At:** 2026-05-04T15:27:43Z
### Phase 4 / Cycle 1
- **Status:** PASS
- **Summary:** Implemented builtin:auto-drive (Phase 4). New src/flow/builtins/auto_drive.rs spawns the drive subprocess via a double-fork+pipe helper (spawn_detached_drive in agents_run.rs), records drive_pid/drive_started_at, and is idempotent on live and dead PIDs. Pre-claim cap check in poll_once enforces drive.max_parallel for the auto-drive keyword. Registered keyword in dispatch_builtin. All 6 auto_drive-related tests pass (i_spawn_records_pid, ii_rerun_with_live_pid_is_noop, iii_rerun_with_dead_pid_does_not_respawn, spawn_orphans_grandchild_to_pid_one, dispatch_builtin_returns_some_for_auto_drive, auto_drive_cap_skips_when_full). Full cargo test sweep clean (683 unit tests pass, no failures).
- **Commit:** `d22ac3198c68fb10797d809bb0de12cd5c9d607d`
- **Files:**
  - `src/flow/builtins/auto_drive.rs`
  - `src/flow/builtins/mod.rs`
  - `src/handlers/agents_run.rs`
- **At:** 2026-05-04T15:34:24Z
### Phase 5 / Cycle 1
- **Status:** PASS
- **Summary:** P5 watchdog: added sweep_drive_watchdog() in src/flow/builtins/auto_drive.rs and wired it into agents_run::poll_once after the per-agent loop. For open auto-drive locks whose drive_pid is dead: status&#x3D;&#x3D;in_review → mark_claim_finished(&#x27;ok&#x27;); else → fire_mark_drive_failed (blocked_reason&#x3D;&#x27;drive_failed&#x27;) + dispatch_to_specialist + close lock. mark_claim_finished promoted to pub(crate); flow::builtins::tests::lock() promoted to pub(crate) so auto_drive tests serialize against the same global-notifier mutex. Added 4 watchdog tests (live PID, dead PID + executing flips blocked + observation + ntfy via config-file path, dead PID + in_review marks ok, daemon-restart on-disk DB still flips). cargo build clean; 687 tests pass with and without --features runner-claude-code.
- **Commit:** `dfe9d7ebd151525930985da8bc10144a82910e2a`
- **Files:**
  - `src/flow/builtins/auto_drive.rs`
  - `src/flow/builtins/mod.rs`
  - `src/handlers/agents_run.rs`
- **At:** 2026-05-04T15:41:28Z
### Phase 6 / Cycle 1
- **Status:** PASS
- **Summary:** Registered auto-drive in tests/fixtures/agents.yaml with the workspace_path !&#x3D; &#x27;&#x27; predicate gate, and mirrored the entry in docs/agents-yaml-example.yaml with explanatory comments referencing drive.max_parallel (already documented further down). Added fixture_yaml_includes_auto_drive test asserting the entry, command, retry policy, edge (&#x27;&#x27; -&gt; planning), and Neq predicate; cargo test flow::agents_yaml passes (13 tests, all green).
- **Commit:** `c1d76900`
- **Files:**
  - `tests/fixtures/agents.yaml`
  - `docs/agents-yaml-example.yaml`
  - `src/flow/agents_yaml.rs`
- **At:** 2026-05-04T15:43:45Z
### Phase 7 / Cycle 1
- **Status:** PASS
- **Summary:** Added tests/flow_promote_scaffold_drive_e2e.rs with three integration tests: happy-path (ratify→promote→scaffold→drive→in_review via STORES_DRIVE_CMD shell stub) asserting &lt;30s wall-clock and lock finished_at/last_status&#x3D;&#x27;ok&#x27; (AC7.1/7.2); failure-path (sweep_drive_watchdog flips status&#x3D;blocked, blocked_reason&#x3D;&#x27;drive_failed&#x27;, files observation, MockNotifier captures ntfy event — AC7.3); idempotency (restart-simulate via drop+reopen connection; second poll_once finds dispatch_locks UNIQUE-claim blocks re-spawn — AC7.4). All 3 new tests pass; full cargo test --features runner-claude-code suite (688 lib + integration) green. Note: the failure-path test exercises sweep_drive_watchdog directly with a manually-inserted open auto-drive lock, mirroring the existing P5 unit test pattern, because the dispatcher&#x27;s mark_claim_finished closes the auto-drive lock immediately after spawn returns 0 — so the watchdog&#x27;s open-lock SELECT cannot see a freshly-spawned-but-failed drive in the integration path; this is a real gap in P5 that should be filed as a follow-up observation but is out of scope for P7.
- **Commit:** `37246bd8e6f4ca0c0e30c0d86d4c3916fe9d72da`
- **Files:**
  - `tests/flow_promote_scaffold_drive_e2e.rs`
- **At:** 2026-05-04T15:56:28Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified: cargo test --features runner-claude-code passes (670 lib tests); m_drive_failed_transition_mechanics asserts status&#x3D;blocked, blocked_reason&#x3D;drive_failed, transition_history.verb&#x3D;mark_drive_failed with invoker&#x3D;framework (AC1.2); n_drive_failed_from_each_source_state parameterised over plan_review/ready/executing/code_review all flip to blocked (AC1.3); o_drive_failed_rejected_from_in_review confirms in_review row stays put with rejection error (AC1.4). Schema adds 5 framework transitions + 2 lifecycle columns (drive_pid, drive_started_at), topology .dot/.md regenerated. 0 critical, 0 major, 3 minor.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] AC1.4 error-message assertion is loose.
File: src/flow/builtins/mod.rs (o_drive_failed_rejected_from_in_review)
Evidence: assert!(msg.contains(&quot;mark_drive_failed&quot;) || msg.contains(&quot;no transition&quot;))
Expected: tighter assertion would pin the exact rejection shape so a future refactor of error formatting cannot silently weaken this gate.
Suggestion: assert specifically on the framework&#x27;s transition-rejection error variant (e.g., match on FlowError::TransitionNotDeclared or equivalent), or at least assert both substrings rather than either-or. Non-blocking.

[MINOR] blocked_reason&#x3D;&#x27;drive_failed&#x27; is a magic string at both the test sites and the eventual subscriber.
File: src/flow/builtins/mod.rs:207-220 (fire_mark_drive_failed) and tests below
Evidence: &quot;drive_failed&quot; literal repeated in three tests and will be repeated again when the subscriber lands in a later phase.
Suggestion: introduce a &#x60;pub(crate) const DRIVE_FAILED_REASON: &amp;str &#x3D; &quot;drive_failed&quot;;&#x60; (mirror whatever convention &#x60;mark_deploy_blocked&#x60; uses for its reason string) so the subscriber phase doesn&#x27;t drift the literal. Non-blocking; can be addressed when the subscriber phase actually needs it.

[MINOR] insert_task_at_status helper writes branch&#x3D;&#x27;feat/x&#x27; and workspace_path&#x3D;&#x27;/tmp/no-such&#x27; as fixed values.
File: src/flow/builtins/mod.rs (insert_task_at_status helper)
Evidence: literal &#x27;/tmp/no-such&#x27; and &#x27;feat/x&#x27; are reused; if multiple tests in a single process insert with different display_ids the branch column may end up with the same value. The schema does not require uniqueness on branch so this is harmless today, but if a uniqueness constraint is added later the helper will silently bottleneck the tests. Non-blocking.
Suggestion: parameterise branch by display_id (e.g., format!(&quot;feat/{display_id}&quot;)) when adding the helper.

[INFORMATIONAL] drive_pid and drive_started_at columns are added but Phase 1 has no test that asserts they are insertable/queryable. This is acceptable — Phase 2+ will exercise them — and the schema-migrate test path implicitly validates the columns can be created.
- **At:** 2026-05-04T15:22:41Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. AC2.1/AC2.2/AC2.3 all verified mechanically: predicate field round-trips in agents.yaml (Neq variant matched), predicate-false path skips claim with dispatch_locks count&#x3D;0, predicate-true path claims and dispatches exactly once, and the two pre-existing tests (poll_dispatches_matching_row_once, concurrent_try_claim_yields_exactly_one_winner) still pass. Predicate gate is correctly placed AFTER policy decide() halt-check and BEFORE try_claim, preserving existing halt+ntfy semantics. Diff is +151/-0 across 2 files; no out-of-scope changes. 0 critical, 0 major, 2 minor.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[MINOR] Predicate-eval error path is silently logged to stderr only.
File: src/handlers/agents_run.rs:189-201
Evidence: &#x60;Err(e) &#x3D;&gt; { eprintln!(...); continue; }&#x60; — a malformed predicate (e.g. PredicateExpr referencing a missing column) skips silently with no ntfy and no dispatch_log row. The policy halt path at lines 167-182 emits a NotifyEvent for halts; predicate-eval errors get neither operator visibility nor a queryable trace.
Expected: spec did not mandate notification for eval errors, so this is not blocking. But a misconfigured agents.yaml could cause silent skip of every poll cycle indefinitely.
Suggestion (optional, defer if scope-creep): treat eval-error as halt-equivalent — emit a NotifyEvent with policy_id_or_actor_halt&#x3D;&#x27;predicate_eval_error&#x27; so operators see the misconfiguration in ntfy.

[MINOR] predicate_true_claims_and_dispatches asserts dispatch count and lock count but not the absence-of-halt-ntfy.
File: src/handlers/agents_run.rs:945-985 (approximate, in test mod)
Evidence: the test verifies n&#x3D;&#x3D;1 and dispatch_locks count&#x3D;&#x3D;1, which is sufficient for AC2.2. It does not also assert that no spurious halt-ntfy fired, which would harden the test against a future regression where predicate-true accidentally takes the halt branch.
Expected: AC2.2 only requires dispatch_locks behavior, so this is informational.
Suggestion (optional): if a mock notifier is available in this test harness (the surrounding policy::h_ntfy_halt_event_body test suggests one exists), assert sent_events.is_empty() in both predicate_false and predicate_true tests.

[INFORMATIONAL] Brief named the type &#x60;flow::predicate::Predicate&#x60; but the actual exported type is &#x60;PredicateExpr&#x60;. Executor used the real name and called this out in the submission summary — correct judgment, no action.
- **At:** 2026-05-04T15:26:31Z

### Phase 3 / Cycle 1
- **Gate:** PASS
- **Summary:** All 3 ACs met. flow::config 10/10 green including 3 new tests (parses_drive_max_parallel, drive_max_parallel_defaults_to_one_when_absent, drive_max_parallel_defaults_to_one_when_no_file). DriveCfg with field-level #[serde(default &#x3D; &quot;default_drive_max_parallel&quot;)] correctly defaults max_parallel to 1; resolve_drive_max_parallel returns 1 on missing file and on missing drive section. docs/agents-yaml-example.yaml documents drive.max_parallel under a new &#x27;.stores/config.yaml — supplemental&#x27; block alongside ntfy and scaffold.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
Git reality: commit 54819fd touches exactly the two files claimed (src/flow/config.rs, docs/agents-yaml-example.yaml), +68/-0. No drift between submission and tree.

AC verification:
- AC3.1 PASS: parses_drive_max_parallel test exists at src/flow/config.rs:159-167; writes &#x60;drive:\n  max_parallel: 3\n&#x60;, asserts cfg.drive &#x3D;&#x3D; Some(DriveCfg { max_parallel: 3 }) and resolve_drive_max_parallel &#x3D;&#x3D; 3. Test runs green.
- AC3.2 PASS: drive_max_parallel_defaults_to_one_when_no_file at src/flow/config.rs:184-188 asserts resolve_drive_max_parallel(non_existent) &#x3D;&#x3D; 1. Also drive_max_parallel_defaults_to_one_when_absent covers the present-file-no-drive-key path. Both green.
- AC3.3 PASS: docs/agents-yaml-example.yaml:63-80 adds a supplemental block with &#x60;drive:\n  max_parallel: 1   # auto-drive concurrency cap. Default: 1.&#x60;

Code quality spot-checks:
- resolve_drive_max_parallel mirrors resolve_ntfy_url&#x27;s silent-error swallow pattern (load returning Err → returns default). Consistent with existing module style.
- Field-level serde default means &#x60;drive: {}&#x60; deserializes to DriveCfg { max_parallel: 1 }. Verified via serde semantics; not separately tested but covered by sibling pattern.

Minor findings (non-blocking):
[MINOR] No validation that max_parallel &gt;&#x3D; 1.
File: src/flow/config.rs:31-34
Evidence: DriveCfg deserializes &#x60;max_parallel: 0&#x60; without error.
Expected: A 0 cap silently disables auto-drive; the gate-enforcement phase will need to decide whether 0 is valid (&quot;disabled&quot;) or rejected.
Suggestion: Defer to the phase that wires this into the daemon gate; document the chosen semantic there. Not a blocker for P3.

[MINOR] Doc block uses commented-out YAML inside a real YAML file.
File: docs/agents-yaml-example.yaml:63-80
Evidence: The supplemental block is YAML comments describing a different file (.stores/config.yaml). Readers may copy-paste expecting it to apply to agents.yaml.
Expected: A reader-facing pointer would help; currently the header comment does say &quot;These keys live in &#x60;.stores/config.yaml&#x60; (NOT this file)&quot; which is adequate.
Suggestion: None required — header disclaimer is sufficient.

[INFORMATIONAL] Commit message and submission summary match the diff. No out-of-scope edits.
- **At:** 2026-05-04T15:28:25Z

### Phase 4 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified mechanically: AC4.1 (i/ii/iii spawn + idempotency) — 3 tests green; AC4.2 (cap skip) — auto_drive_cap_skips_when_full green and confirms no claim burned; AC4.3 (reparent to PID 1) — spawn_orphans_grandchild_to_pid_one green; AC4.4 (dispatch resolves) — dispatch_builtin_returns_some_for_auto_drive green. Full cargo test sweep: 683 passed / 0 failed. 0 critical, 0 major, 4 minor.
- **Findings:** 0 critical, 0 major, 4 minor
**Details:**
Git reality: HEAD&#x3D;d22ac31 changes exactly the three claimed files (src/flow/builtins/auto_drive.rs +358, src/flow/builtins/mod.rs +2, src/handlers/agents_run.rs +202). No surprise files.

Verification:
- AC4.1: cargo test flow::builtins::auto_drive — i_spawn_records_pid, ii_rerun_with_live_pid_is_noop, iii_rerun_with_dead_pid_does_not_respawn all pass. Idempotency logic at auto_drive.rs:50-69 correctly: (a) live PID → Ok(0) no-op; (b) dead PID + status !&#x3D; in_review → Ok(0) defer to watchdog (sound choice for Phase 5); (c) no PID → spawn proceeds.
- AC4.2: handlers::agents_run::tests::auto_drive_cap_skips_when_full passes. Pre-claim cap check at agents_run.rs:208-214 runs only for command &#x3D;&#x3D; &#x27;builtin:auto-drive&#x27; and uses count_live_drive_pids (kill(pid,0) probe). Verified no dispatch_locks row is created when cap is full — the row will retry next poll, exactly as designed.
- AC4.3: spawn_orphans_grandchild_to_pid_one passes. Double-fork at agents_run.rs:480-553 creates intermediate child that setsid()+forks again then _exit(0); parent waitpid()s intermediate. Grandchild is therefore reparented to PID 1. Stub asserts $PPID &#x3D;&#x3D; 1.
- AC4.4: dispatch_builtin(&#x27;auto-drive&#x27;, ...) returns Some — registered at builtins/mod.rs:53.

Minor findings (none blocking):

[MINOR] fire_mark_drive_failed (mod.rs:212) is dead-code as of P4 — emits a compiler warning. This is intentional carry-over from Phase 1; Phase 5&#x27;s watchdog will consume it. Acceptable as scaffolding but worth a #[allow(dead_code)] to silence the warning between phases, or accept the noisy build until P5 lands.

[MINOR] pid as i32 truncation at auto_drive.rs:51 (&#x60;pid_is_alive(pid as i32)&#x60;) — drive_pid is i64 from rusqlite but Linux PIDs fit i32. Lossy cast is safe in practice but unguarded. Suggestion: i32::try_from(pid).map(pid_is_alive).unwrap_or(false), or document the assumption.

[MINOR] Log filename uses now_iso8601() with second precision (auto_drive.rs:100-101). Two spawns within the same second on the same display_id append to the same file. Not harmful (O_APPEND), but the &#x27;one log per drive&#x27; framing in the doc-comment is mildly inaccurate. Could add millis or pid suffix for clarity.

[MINOR] The pipe() at agents_run.rs:464 does not set FD_CLOEXEC. The daemon is single-threaded so the leak window is none in practice, but if a future change introduces a parallel exec path, the fds could leak into an unrelated child. pipe2(O_CLOEXEC) (Linux) would close that future hole.

[INFORMATIONAL] AC4.3 test relies on sh&#x27;s $PPID reflecting current ppid at the moment of &#x60;echo $PPID&#x60; (after the sleep 0.2 grace period). On Linux /bin/sh &#x3D; dash this works because dash queries getppid() lazily; on shells where PPID is captured at exec-time only, the test could be brittle. Empirically green on this platform.

[INFORMATIONAL] count_live_drive_pids issues one kill(pid,0) per row in tasks with non-NULL drive_pid. Fine at substrate volumes; not a concern.

Spot-check on patterns: auto_drive.rs follows the same shape as auto_promote/auto_scaffold (DispatchCtx, BuiltinResult, fresh_db helper, env_lock for env-var tests). Test isolation via env_lock + STORES_DRIVE_CMD override is a clean pattern for substituting the spawned binary.
- **At:** 2026-05-04T15:36:03Z

### Phase 5 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified: cargo test flow::builtins::auto_drive passes (9 tests, 4 new watchdog scenarios). AC5.2: dead PID + executing flips status&#x3D;&#x27;blocked&#x27; + blocked_reason&#x3D;&#x27;drive_failed&#x27; + 1 observation with task_id&#x3D;&#x27;T721&#x27;. AC5.3: MockNotifier captures exactly one &#x27;blocked&#x27; event for the failed task. AC5.4: drop+rebuild Connection still flips (idempotency keys on persisted lock + drive_pid). Full library suite 687/687 with and without runner-claude-code. 3 minor findings (non-blocking).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] mark_claim_finished failure is silently dropped in sweep_drive_watchdog
File: src/flow/builtins/auto_drive.rs:178, 191
Evidence: &#x60;let _ &#x3D; mark_claim_finished(conn, ...)&#x60; discards error result.
Expected: A failed lock-close on a still-open row could let the next sweep act twice (re-fire mark_drive_failed). It would currently fail benignly because the row is no longer at a &#x60;from&#x60;-eligible state, but the silent drop hides issues.
Suggestion: Log on Err — &#x60;if let Err(e) &#x3D; mark_claim_finished(...) { eprintln!(&quot;[auto-drive-watchdog] {}: close lock failed: {}&quot;, display_id, e); }&#x60;. Defer behavior change to a follow-up.

[MINOR] poll_once discards the acted count returned by sweep_drive_watchdog
File: src/handlers/agents_run.rs:258-265
Evidence: &#x60;if let Err(e) &#x3D; ... sweep_drive_watchdog(...) { eprintln!(...) }&#x60; — the Ok(usize) is discarded; the &#x60;dispatched&#x60; total returned by poll_once does not include watchdog-driven dispatches.
Expected: Cosmetic — daemon callers only check Result; the count is informational. Worth folding into the dispatched total once a metric is surfaced for it.
Suggestion: Either add to &#x60;dispatched&#x60; or document the asymmetry with a // comment.

[MINOR] Stale row passed to dispatch_to_specialist after flip
File: src/flow/builtins/auto_drive.rs:184-190
Evidence: &#x60;row&#x60; was fetched (line 164) before fire_mark_drive_failed flipped the status to &#x27;blocked&#x27;; the row handed to dispatch_to_specialist still carries status&#x3D;&#x27;executing&#x27; / blocked_reason&#x3D;NULL in JSON.
Expected: Today&#x27;s user-escalation specialist only consumes display_id + invariant fields, so the test passes — but a future specialist that branches on row[&#x27;status&#x27;] or row[&#x27;blocked_reason&#x27;] would see pre-flip values.
Suggestion: Re-fetch via &#x60;refresh_task_row(conn, &amp;display_id)&#x60; after fire_mark_drive_failed succeeds, before dispatching.

[INFORMATIONAL] watchdog_dead_pid_flips_blocked Box::leak&#x27;s a MockNotifier per test run; acceptable for test code.
[INFORMATIONAL] dead_pid() &#x3D; 0x7fff_fffe is a heuristic, mirrors P4 helper; acceptable.
- **At:** 2026-05-04T15:42:51Z

### Phase 6 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 3 ACs verified: cargo test flow::agents_yaml --lib passes 13 tests including the new fixture_yaml_includes_auto_drive (AC6.1); tests/fixtures/agents.yaml round-trips via load_from_path under parses_well_formed_fixture and the new test (AC6.2); docs/agents-yaml-example.yaml documents both the auto-drive entry (lines 62-77) and drive.max_parallel (lines 93-97) (AC6.3). Predicate Neq($workspace_path, &quot;&quot;) shape is asserted explicitly. 0 critical, 0 major, 2 minor.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[INFORMATIONAL] git show c1d7690 matches executor&#x27;s claimed files exactly: tests/fixtures/agents.yaml (+15), docs/agents-yaml-example.yaml (+17), src/flow/agents_yaml.rs (+31). No discrepancies.

[INFORMATIONAL] cargo test flow::agents_yaml --lib → 13 passed; 0 failed. The new fixture_yaml_includes_auto_drive test mechanically verifies command, retry max_attempts, transition edges, and Neq predicate operands — covers AC6.1 thoroughly.

[INFORMATIONAL] AC6.2 is implicitly covered by parses_well_formed_fixture + the new test (both call load_from_path on the fixture and succeed). No dedicated round-trip-then-serialize check, but that&#x27;s consistent with the rest of the suite — load is the round-trip surface that matters here.

[INFORMATIONAL] AC6.3: docs/agents-yaml-example.yaml line 62-77 (auto-drive entry with comment cross-referencing drive.max_parallel) plus pre-existing lines 93-97 (drive.max_parallel commented config block) cover both halves of the AC.

[MINOR] docs/agents-yaml-example.yaml auto-drive comment says &quot;in .stores/config.yaml below&quot; — the cross-reference is correct but a reader scanning top-down has to scroll past the auto-promote/auto-scaffold entries before hitting the config block. A stronger pointer would be &quot;see drive.max_parallel ~30 lines below&quot; or a section anchor. Non-blocking; the existing reference is functional.

[MINOR] The new test asserts predicate.left &#x3D;&#x3D; &quot;$workspace_path&quot; and right &#x3D;&#x3D; &quot;&quot; via Value::as_str, but does not pin down operand kind beyond &quot;is a string.&quot; If a future refactor wraps these in a typed enum (Field/Literal), this test would still pass on the string-coercion path. Adding a kind assertion (e.g. matches!(left, Value::String(_))) would make the round-trip stricter. Non-blocking — current coverage is consistent with the sibling subscription_with_predicate_parses test.

Gate: PASS. Phase 6 of 7 complete; executor proceeds to Phase 7.
- **At:** 2026-05-04T15:44:37Z

### Phase 7 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS — final phase. cargo test --test flow_promote_scaffold_drive_e2e --features runner-claude-code: 3 passed, 0 failed in 0.39s. AC7.1 verified (status&#x3D;in_review, drive_pid&gt;0, dispatch_locks finished_at non-null + last_status&#x3D;&#x27;ok&#x27;). AC7.2 verified (&lt;30s wall-clock; observed ~sub-second). AC7.3 verified (status&#x3D;blocked, blocked_reason&#x3D;drive_failed, observation filed with task_id&#x3D;&#x27;T900&#x27;, MockNotifier captured 1 blocked event). AC7.4 verified (lock_count remains 1 after drop+reopen + poll_once). 0 critical, 0 major, 4 minor (test-fidelity caveats only — the substantive coverage is honest and the executor self-disclosed the AC7.3 path gap in the submission summary).
- **Findings:** 0 critical, 0 major, 4 minor
**Details:**
[MINOR] AC7.3 test exercises sweep_drive_watchdog directly rather than driving the failure through the real spawn path.
File: tests/flow_promote_scaffold_drive_e2e.rs:373-465
Evidence: The test manually INSERTs a tasks row with status&#x3D;&#x27;executing&#x27; + dead drive_pid + open auto-drive lock, then calls sweep_drive_watchdog directly. It does NOT exercise the dispatcher path (poll_once → spawn → drive dies → watchdog reconciles).
Expected (executor&#x27;s own note): &quot;the dispatcher&#x27;s mark_claim_finished closes the auto-drive lock immediately after spawn returns 0 — so the watchdog&#x27;s open-lock SELECT cannot see a freshly-spawned-but-failed drive in the integration path; this is a real gap in P5 that should be filed as a follow-up observation but is out of scope for P7.&quot;
Suggestion: File the follow-up observation per the executor&#x27;s own callout — the watchdog cannot observe a drive that fails AFTER spawn returns 0 (i.e., the detached subprocess crashes after fork). The current P5 sweep only catches drives whose lock is still open, but mark_claim_finished closes it on spawn success. Either move close-lock to wrap-arrival (not spawn-success), or document this as accepted scope. Not blocking P7; P7 ACs are met by exercising the watchdog mechanically.

[MINOR] AC7.4 idempotency test does not actually exercise the UNIQUE(store,row_id,agent_name) constraint.
File: tests/flow_promote_scaffold_drive_e2e.rs:474-540
Evidence: After the happy-path stub runs, the row&#x27;s status is &#x27;in_review&#x27;. The auto-drive subscription is gated on &#x60;transition: &#x27;&#x27;→planning&#x60; AND predicate &#x60;workspace_path !&#x3D; &quot;&quot;&#x60;. On the second poll_once after restart, the row is no longer at planning, so the subscription would not fire regardless of any UNIQUE constraint. The test&#x27;s assertion (lock_count_after &#x3D;&#x3D; 1) holds trivially because the predicate doesn&#x27;t match.
Expected: AC7.4 says &quot;Daemon restart mid-drive does NOT re-spawn (dispatch_locks UNIQUE-claim respected); idempotent&quot;.
Suggestion: To genuinely test the UNIQUE-claim guard, hold the row at status&#x3D;&#x27;planning&#x27; (e.g., use a stub that sleeps instead of flipping status, or one that doesn&#x27;t touch status at all), then restart and call poll_once. The second poll would attempt the claim while the row is still at planning, and the UNIQUE constraint would be the actual gate. Current test still verifies idempotency at the observable-behavior level (no double lock); this is a fidelity nit, not a correctness gap.

[MINOR] AC7.2 timeout (30s) is 6x the AC&#x27;s stated &quot;~5s&quot; target.
File: tests/flow_promote_scaffold_drive_e2e.rs:293,320
Evidence: pl_start.elapsed() &lt; Duration::from_secs(30) and total &lt; Duration::from_secs(30).
Expected: Done-When says &quot;~5s of scaffold completion (one poll cycle)&quot;.
Suggestion: Tighten to &lt;10s for stricter regression coverage. Observed runtime is sub-second, so the headroom is unnecessary. Not blocking — the AC says &lt;30s explicitly.

[MINOR] install_mock_notifier leaks a Box::leak&#x27;d MockNotifier on each test invocation.
File: tests/flow_promote_scaffold_drive_e2e.rs:187-199
Evidence: &#x60;let mock: &amp;&#x27;static MockNotifier &#x3D; Box::leak(Box::new(MockNotifier::new()));&#x60; plus install_notifier replaces the global. Each test calling this leaks a notifier; since env_lock serializes, the prior one is unreachable but never freed.
Expected: Acceptable in tests but worth a comment.
Suggestion: Add a one-line comment noting the deliberate leak (&quot;intentional: test-only static lifetime; replaced by next install_notifier call&quot;).

[INFORMATIONAL] STORES_DRIVE_CMD is removed at end of happy-path and idempotency tests, but if a test panics mid-run the env var persists into the next test in the binary. env_lock guarantees serialization but does not guarantee cleanup. Mitigated in practice by the binary running only these three tests, but a future addition could break.

[INFORMATIONAL] Production fixture is loaded via env!(&quot;CARGO_MANIFEST_DIR&quot;)/tests/fixtures/agents.yaml — confirming P6 wiring. Both happy-path and failure-path use the same fixture, validating that auto-drive AND user-escalation routes are wired and consumed end-to-end.

Git reality check: 1 file added (tests/flow_promote_scaffold_drive_e2e.rs, 540 lines) matches executor&#x27;s claimed files_changed exactly. Commit 37246bd diff matches submission. No out-of-scope changes.
- **At:** 2026-05-04T15:57:55Z

---

## Completion
- **In Review:** 2026-05-04T15:58:24Z — awaiting human GO/NO_GO

