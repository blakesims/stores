# T050: Formalize dispatch_locks as a typed lifecycle buffer with explicit columns (daemon_epoch, claim_source, attempt, pid, heartbeat, expected_postcondition, terminal_reason, retry_eligibility) — currently 7+ implicit states leak through last_status string parsing

## Meta
- **Status:** in_review
- **Created:** 2026-05-06T09:16:57Z
- **Last Updated:** 2026-05-06T11:22:07Z
- **Current Phase:** 5
- **Current Cycle:** 2
- **Blocked Reason:** —
- **Branch:** feat/T050-auto-promoted-l134

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - Add typed columns to dispatch_locks: daemon_epoch (i64, recorded at daemon start), claim_source (enum: try_claim / retry_claim / manual / legacy), attempt (i64), pid (i64, nullable), heartbeat_at (timestamp, nullable), postcondition_id (TEXT, registered enum value), postcondition_args (JSON object, optional parameters to the named predicate), terminal_reason (enum: ok / exit_nonzero / error / silent_zombie / timeout / halted / legacy_unknown), next_retry_at (timestamp, nullable; computed by retry rescheduler instead of a stored retry_eligibility bool to avoid drift). Subscriber execution contract: each subscriber declares its postcondition_id at registration; framework looks up the named predicate (in a code-owned registry, not a row-stored predicate language) and verifies against substrate state before writing terminal_reason&#x3D;ok. Existing consumers (auto_drive subscriber, T040 watchdog, T041 retry rescheduler) migrate to read typed columns. Backward compatibility: last_status string column kept and populated from terminal_reason during a migration window.
- Migration: existing dispatch_locks rows backfilled with daemon_epoch&#x3D;0, claim_source&#x3D;&#x27;legacy&#x27;, attempt&#x3D;1 (synthetic, documented), pid&#x3D;NULL, heartbeat_at&#x3D;NULL, postcondition_id&#x3D;NULL, postcondition_args&#x3D;NULL, terminal_reason&#x3D;parsed-from-last_status with &#x27;legacy_unknown&#x27; for ambiguous cases, next_retry_at&#x3D;NULL. Migration MUST NOT change live lock behavior based on parsed historical terminal_reason — only populate observability fields. retry_eligibility for legacy_unknown rows is FALSE unless an explicit safe mapping is documented (e.g. last_status&#x3D;&#x27;ok&#x27; → terminal_reason&#x3D;&#x27;ok&#x27; is safe). Tests cover the L087/L107/L116/L122/L141 reproductions converted to typed-shape regression cases. Per-subscriber postcondition_id declarations land for at least: auto_promote (postcondition: task_exists_for_linked_observation), auto_scaffold (postcondition: task_workspace_exists), auto_drive (postcondition: drive_pid_recorded_or_terminal), mark_cargo_installed (postcondition: cargo_installed_state), mark_schema_migrated (postcondition: schema_migrated_state). Postcondition predicates are pure functions (substrate state, transition_history) -&gt; bool, owned in code and registered by string id at boot.
- **Out:** - Path B: full split into dispatch_attempts (typed transitions) + dispatch_locks (claim ownership only). Deferred explicitly to a follow-up task once Path A&#x27;s typed buffer reveals where the lock-vs-attempt seam should land. Per-subscriber concurrency leases. Heartbeat-driven liveness checks beyond what T040 watchdog already does. Schema-version row management for dispatch_locks (use existing substrate_migrations once L144 ships). Arbitrary row-level predicate language (rejected — postconditions are code-owned and named, not row-stored predicate trees). Stored retry_eligibility column (computed booleans drift; use next_retry_at + attempt instead). New subscribers added in this task. Re-shaping the framework subscriber registration API beyond adding the postcondition_id declaration. Read-time replay of postconditions to detect retroactive lock corruption.

### Done When
Type the existing dispatch_locks operational buffer with explicit lifecycle columns (daemon_epoch, claim_source, attempt, pid, heartbeat_at, postcondition_id, postcondition_args, terminal_reason, next_retry_at) so the L087/L107/L116/L122/L141 zombie/stale/duplicate-drive cluster has a typed surface to fix on. Subscribers declare a registered postcondition_id at registration; framework verifies via the named predicate before marking terminal_reason&#x3D;ok. Path B (split into dispatch_attempts + dispatch_locks) is explicitly deferred to a follow-up task; this contract types the existing buffer as a clean stepping stone.

Acceptance:
- dispatch_locks has 9 new typed columns. substrate_migrations records the L134 migration. Existing tests pass. New tests cover: (i) typed silent_zombie detection via terminal_reason&#x3D;&#x27;silent_zombie&#x27; rather than last_status string parsing; (ii) retry_eligibility computed correctly via next_retry_at across exit_nonzero/error/silent_zombie/halted/ok terminal reasons (legacy_unknown is non-retry-eligible by default); (iii) at least one subscriber&#x27;s named postcondition rejects a row whose substrate state did not converge after subscriber returned; (iv) backfilled rows with terminal_reason&#x3D;&#x27;legacy_unknown&#x27; do not get auto-retried. L087/L107/L116/L122/L141&#x27;s symptoms covered by typed-column-aware paths in regression tests. Pi reviews the typed schema before ratify (already done; this contract embeds pi&#x27;s amendments).

### Phases

#### Phase 1: Phase 1: DDL + idempotent runtime migration + backfill
- **Objective:** Add 9 typed columns to dispatch_locks DDL and a one-shot ALTER-pass migration that populates legacy rows without changing live behavior.
- **Tasks:**
  - Task 1.1: In src/codegen/ddl.rs SUBSTRATE_DDL, add the 9 columns to dispatch_locks: daemon_epoch TEXT, claim_source TEXT CHECK(claim_source IN (&#x27;try_claim&#x27;,&#x27;retry_claim&#x27;,&#x27;manual&#x27;,&#x27;legacy&#x27;)), attempt INTEGER, pid INTEGER, heartbeat_at TEXT, postcondition_id TEXT, postcondition_args TEXT, terminal_reason TEXT CHECK(terminal_reason IN (&#x27;ok&#x27;,&#x27;exit_nonzero&#x27;,&#x27;error&#x27;,&#x27;silent_zombie&#x27;,&#x27;timeout&#x27;,&#x27;halted&#x27;,&#x27;legacy_unknown&#x27;)), next_retry_at TEXT
  - Task 1.2: Add substrate_migrations table (id TEXT PRIMARY KEY, applied_at TEXT NOT NULL, note TEXT) to SUBSTRATE_DDL — minimal ledger; full L144 management remains scope_out
  - Task 1.3: Add fn ensure_dispatch_locks_typed(conn) -&gt; Result&lt;()&gt; in src/handlers/agents_run.rs that uses PRAGMA table_info(&#x27;dispatch_locks&#x27;) to detect missing columns and ALTER TABLE ADD each one; idempotent via skip-if-present; records &#x27;L134-dispatch-locks-typed&#x27; row in substrate_migrations on first run
  - Task 1.4: Add fn backfill_legacy_locks(conn) -&gt; Result&lt;usize&gt; that, for rows where claim_source IS NULL, sets claim_source&#x3D;&#x27;legacy&#x27;, attempt&#x3D;COALESCE(attempts,1), terminal_reason parsed from last_status (last_status&#x3D;&#x27;ok&#x27; → &#x27;ok&#x27;; LIKE &#x27;exit&#x3D;%&#x27; AND !&#x3D; &#x27;exit&#x3D;0&#x27; → &#x27;exit_nonzero&#x27;; LIKE &#x27;error:%&#x27; → &#x27;error&#x27;; LIKE &#x27;halted:%&#x27; → &#x27;halted&#x27;; &#x27;skip-historical&#x27; → &#x27;legacy_unknown&#x27;; everything else → &#x27;legacy_unknown&#x27;), next_retry_at&#x3D;NULL, daemon_epoch&#x3D;&#x27;&#x27;. MUST NOT change live lock semantics — only populate observability fields
  - Task 1.5: Call ensure_dispatch_locks_typed + backfill_legacy_locks once at run_daemon() startup BEFORE seed_starting_line, and once at the top of every CLI verb that opens the DB (via crate::db::open) so single-shot CLI flows also migrate
- **Acceptance Criteria:**
  - [ ] AC1.1: cargo build succeeds and cargo test --lib codegen::ddl passes (snapshot updated)
  - [ ] AC1.2: A new test in src/handlers/agents_run.rs (or a fresh tests/dispatch_locks_migration.rs) opens an in-memory DB containing the OLD SUBSTRATE_DDL (column set without the 9 new fields), inserts 3 legacy rows with last_status in {&#x27;ok&#x27;,&#x27;exit&#x3D;11&#x27;,&#x27;error: x&#x27;}, runs ensure_dispatch_locks_typed + backfill_legacy_locks, and asserts: all 9 columns exist; the 3 rows have terminal_reason in {&#x27;ok&#x27;,&#x27;exit_nonzero&#x27;,&#x27;error&#x27;} respectively; claim_source&#x3D;&#x27;legacy&#x27; on all; substrate_migrations has exactly one row with id&#x3D;&#x27;L134-dispatch-locks-typed&#x27;
  - [ ] AC1.3: Re-running ensure_dispatch_locks_typed + backfill_legacy_locks is a no-op (idempotent): no error, no second substrate_migrations row, no row updates
  - [ ] AC1.4: All existing tests in agents_run::tests pass unchanged
- **Files:** `src/codegen/ddl.rs`, `src/handlers/agents_run.rs`, `src/handlers/mod.rs`, `tests/dispatch_locks_migration.rs`
#### Phase 2: Phase 2: Postcondition registry
- **Objective:** Introduce a code-owned named-predicate registry mapping postcondition_id strings to pure (Connection, &amp;Value) -&gt; Result&lt;bool&gt; functions, with no behavioural wiring yet.
- **Tasks:**
  - Task 2.1: Create src/flow/postconditions.rs with: enum PostconditionId variants for task_exists_for_linked_observation, task_workspace_exists, drive_pid_recorded_or_terminal, cargo_installed_state, schema_migrated_state, plus an Other(String) escape; fn lookup(id: &amp;str) -&gt; Option&lt;PostconditionFn&gt;; type PostconditionFn &#x3D; fn(&amp;Connection, &amp;Value, Option&lt;&amp;Value&gt;) -&gt; Result&lt;bool&gt;
  - Task 2.2: Implement each postcondition as a pure fn reading substrate state — task_exists_for_linked_observation queries tasks.linked_observations LIKE &#x27;%LXXX%&#x27; for the source obs id; task_workspace_exists queries tasks.workspace_path !&#x3D; &#x27;&#x27; AND row exists; drive_pid_recorded_or_terminal returns true if tasks.drive_pid &gt; 0 OR tasks.status IN (&#x27;blocked&#x27;,&#x27;accepted&#x27;,&#x27;deploy_blocked&#x27;,&#x27;schema_migrated&#x27;); cargo_installed_state returns true if tasks.status&#x3D;&#x27;cargo_installed&#x27;; schema_migrated_state returns true if tasks.status&#x3D;&#x27;schema_migrated&#x27;
  - Task 2.3: Add fn postcondition_for_builtin(keyword: &amp;str) -&gt; Option&lt;&amp;&#x27;static str&gt; in src/flow/builtins/mod.rs returning the postcondition_id each builtin keyword owns: &#x27;auto-promote&#x27; → &#x27;task_exists_for_linked_observation&#x27;, &#x27;auto-scaffold&#x27; → &#x27;task_workspace_exists&#x27;, &#x27;auto-drive&#x27; → &#x27;drive_pid_recorded_or_terminal&#x27;, &#x27;cargo-install&#x27; → &#x27;cargo_installed_state&#x27;, &#x27;schema-migrate&#x27; → &#x27;schema_migrated_state&#x27;
  - Task 2.4: Add unit tests for each postcondition: insert a synthetic row that satisfies and another that fails; assert the predicate returns true / false respectively
- **Acceptance Criteria:**
  - [ ] AC2.1: cargo test --lib flow::postconditions passes with at least 5 tests (one per postcondition); each test covers both the satisfies and fails cases
  - [ ] AC2.2: lookup() returns Some for every PostconditionId string and None for an unknown id
  - [ ] AC2.3: postcondition_for_builtin returns the documented mapping for all 5 builtin keywords; returns None for unknown keywords
- **Files:** `src/flow/postconditions.rs`, `src/flow/mod.rs`, `src/flow/builtins/mod.rs`
- **Dependencies:** Phase 1 columns exist (postcondition runs read substrate state, not dispatch_locks rows)
#### Phase 3: Phase 3: Write-side wiring — populate typed columns on claim, finish, retry, watchdog
- **Objective:** Make every dispatch_locks write populate the typed columns alongside the legacy last_status string, including a postcondition check before recording terminal_reason&#x3D;&#x27;ok&#x27;.
- **Tasks:**
  - Task 3.1: Modify try_claim (src/handlers/agents_run.rs) to take daemon_epoch, claim_source, postcondition_id args and populate them in the INSERT; callers in poll_once (around line 250) pass daemon_epoch + &#x27;try_claim&#x27; + postcondition_for_builtin(agent.command keyword); pid stays NULL until subscriber records it
  - Task 3.2: Modify mark_claim_finished to compute terminal_reason from the exit_code/error string at the call-site: 0 → &#x27;ok&#x27;, N&gt;0 → &#x27;exit_nonzero&#x27;, error string → &#x27;error&#x27;, drive_failed branches in auto_drive → &#x27;silent_zombie&#x27; or &#x27;error&#x27;. Continue writing last_status from a derive(terminal_reason) helper for backward compat. UPDATE writes both last_status AND terminal_reason in the same statement
  - Task 3.3: When terminal_reason would be &#x27;ok&#x27;, call run_postcondition_for_lock(conn, store, row_id, agent_name) which reads postcondition_id off the row, looks up the predicate, evaluates against the refreshed substrate row, and demotes terminal_reason to &#x27;error&#x27; (with last_status&#x3D;&#x27;error: postcondition &lt;id&gt; failed&#x27;) when the predicate returns false. Legacy rows with postcondition_id&#x3D;NULL skip the check (back-compat)
  - Task 3.4: Modify mark_retry_halted to write terminal_reason&#x3D;&#x27;halted&#x27;; modify claim_for_retry to bump claim_source&#x3D;&#x27;retry_claim&#x27; and increment attempt
  - Task 3.5: Modify auto_drive.rs sweep_drive_watchdog and the silent_zombie path to call a new mark_claim_silent_zombie(conn, store, row_id, agent, reason) that writes terminal_reason&#x3D;&#x27;silent_zombie&#x27; AND last_status with the existing &#x27;drive_failed:silent_zombie_*&#x27; suffix preserved
  - Task 3.6: After every terminal write, compute next_retry_at by reading the agent&#x27;s retry_policy and the current attempt: when terminal_reason ∈ {&#x27;exit_nonzero&#x27;,&#x27;error&#x27;,&#x27;silent_zombie&#x27;} AND attempt &lt; max → next_retry_at &#x3D; finished_at + compute_backoff_secs(...); else → NULL. Replace the LIKE-string predicate inside find_retryable_locks (Phase 4 finishes the read-side switch)
- **Acceptance Criteria:**
  - [ ] AC3.1: New unit test in agents_run::tests inserts an in-memory dispatch — try_claim populates daemon_epoch (non-empty), claim_source&#x3D;&#x27;try_claim&#x27;, attempt&#x3D;0, postcondition_id matching the builtin; values readable via SELECT after the call
  - [ ] AC3.2: New unit test exercises the postcondition-failed demotion path: a subscriber returns exit&#x3D;0 but the named postcondition returns false against the substrate state; terminal_reason ends up &#x27;error&#x27;, last_status starts with &#x27;error: postcondition &#x27;; transition_history is NOT written by the postcondition itself (postcondition is read-only)
  - [ ] AC3.3: Watchdog test (extension of watchdog_silent_zombie_lock_already_closed) asserts post-sweep dispatch_locks row has terminal_reason&#x3D;&#x27;silent_zombie&#x27; and last_status retains &#x27;drive_failed:silent_zombie_pid_dead&#x27; (or pid_never_recorded)
  - [ ] AC3.4: Halt-policy test asserts mark_retry_halted writes terminal_reason&#x3D;&#x27;halted&#x27;
  - [ ] AC3.5: All pre-existing tests in agents_run::tests, drive_silent_zombie_e2e, flow_starting_line_e2e, flow_promote_scaffold_drive_e2e still pass
- **Files:** `src/handlers/agents_run.rs`, `src/flow/builtins/auto_drive.rs`, `src/flow/builtins/mod.rs`
- **Dependencies:** Phase 1 columns, Phase 2 registry
#### Phase 4: Phase 4: Read-side switch — find_retryable_locks reads typed columns; legacy_unknown is non-retry-eligible
- **Objective:** Replace the last_status LIKE string predicates with typed-column predicates so retry eligibility is computed mechanically and legacy_unknown rows are not auto-retried.
- **Tasks:**
  - Task 4.1: Rewrite find_retryable_locks SQL to: WHERE dl.agent_name&#x3D;?1 AND dl.attempt &lt; ?2 AND dl.next_retry_at IS NOT NULL AND dl.next_retry_at &lt;&#x3D; ?3 AND dl.terminal_reason IN (&#x27;exit_nonzero&#x27;,&#x27;error&#x27;,&#x27;silent_zombie&#x27;); the next_retry_at-based gating obsoletes the LIKE string scans and the runtime parse_iso8601_to_epoch backoff comparison (which now lives at write-time in Phase 3 Task 3.6)
  - Task 4.2: Update claim_for_retry CAS guard to key on attempt + terminal_reason (UPDATE ... WHERE attempt&#x3D;?N AND terminal_reason&#x3D;?M) instead of last_status&#x3D;&#x27;exit&#x3D;...&#x27;; preserve the atomic semantics — only one daemon flips the row per retry cycle
  - Task 4.3: Update auto_drive watchdog query (src/flow/builtins/auto_drive.rs scan_zombie_tasks) to additionally exclude rows with terminal_reason&#x3D;&#x27;silent_zombie&#x27; so the watchdog does not re-fire on already-marked zombies; preserve the existing daemon_epoch + grace-window semantics
  - Task 4.4: Verify legacy_unknown rows are never returned by find_retryable_locks (their next_retry_at is NULL by Phase 1 backfill)
- **Acceptance Criteria:**
  - [ ] AC4.1: New test inserts a backfilled legacy row with terminal_reason&#x3D;&#x27;legacy_unknown&#x27; and confirms find_retryable_locks returns 0 candidates for any agent
  - [ ] AC4.2: New test inserts a freshly-failed row with terminal_reason&#x3D;&#x27;exit_nonzero&#x27; and next_retry_at in the past; find_retryable_locks returns it; claim_for_retry CAS returns true once and false on the second concurrent caller (the existing retry_race_double_claim_excluded test pattern updated for typed columns)
  - [ ] AC4.3: All retry tests in agents_run::tests (retry_dispatch_succeeds_then_marks_ok, retry_max_attempts_giveup, halt_on_retry_parks_last_status_no_storm) still pass with the typed-column read path
- **Files:** `src/handlers/agents_run.rs`, `src/flow/builtins/auto_drive.rs`
- **Dependencies:** Phase 3 must populate typed columns on every write
#### Phase 5: Phase 5: Regression suite — typed-column-aware reproductions of L087 / L107 / L116 / L122 / L141
- **Objective:** Convert each cluster member&#x27;s symptom into a typed-shape regression test that fails if the typed-column path regresses.
- **Tasks:**
  - Task 5.1: tests/dispatch_locks_typed_regression.rs — L087 case (auto-promote silent-fail on rapid sequential ratifies): two observations ratified back-to-back; assert both produce dispatch_locks rows with postcondition_id&#x3D;&#x27;task_exists_for_linked_observation&#x27; and terminal_reason&#x3D;&#x27;ok&#x27; (not &#x27;error: postcondition ...&#x27;)
  - Task 5.2: L107 case: a subscriber that completes with exit&#x3D;0 but does NOT converge the substrate state (e.g. auto_scaffold mock that fails to set workspace_path) — assert terminal_reason demoted to &#x27;error&#x27; via postcondition gate
  - Task 5.3: L116 case: starting-line seeder + new transition between polls — assert seeded marker rows have claim_source&#x3D;&#x27;legacy&#x27; (skip-historical seed treated as legacy) and the live transition&#x27;s claim has claim_source&#x3D;&#x27;try_claim&#x27; AND attempt&#x3D;0; daemon_epoch matches across the daemon&#x27;s lifetime
  - Task 5.4: L122 case: dispatch_lock orphans on subagent kill — simulate by inserting a row with terminal_reason&#x3D;&#x27;silent_zombie&#x27; and verify watchdog does not re-fire (Phase 4.3 exclusion) AND retry rescheduler treats it as eligible only when next_retry_at is non-null
  - Task 5.5: L141 case: auto-drive marks last_status&#x3D;&#x27;ok&#x27; on dispatch (not on completion) — assert the typed-column path writes terminal_reason&#x3D;&#x27;ok&#x27; ONLY after the postcondition check passes; if drive_pid never lands, postcondition fails and terminal_reason becomes &#x27;error&#x27; (or stays unset until watchdog flips it to &#x27;silent_zombie&#x27;)
- **Acceptance Criteria:**
  - [ ] AC5.1: tests/dispatch_locks_typed_regression.rs has 5 named tests (l087_*, l107_*, l116_*, l122_*, l141_*); cargo test --test dispatch_locks_typed_regression passes 5/5
  - [ ] AC5.2: Reverting Phase 3 Task 3.3 (the postcondition demotion) makes l107_* and l141_* fail — proves the test is gating the new behavior, not the old
  - [ ] AC5.3: cargo test --workspace passes overall (no regressions in legacy tests)
- **Files:** `tests/dispatch_locks_typed_regression.rs`
- **Dependencies:** Phase 4 read-side complete

---

## Plan Review

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable. Five phases trace cleanly to done_when (typed columns + migration + postcondition registry + write-side + read-side + regression suite for L087/L107/L116/L122/L141). Acceptance criteria are mechanical (specific cargo test invocations, SELECT-checkable column states, named test files). Decision matrix covers all six consequential choices (substrate_migrations bootstrap, ALTER pass location, postcondition-failed semantics, last_status back-compat, pid ownership, phase ordering) with concrete rationale. Phase ordering is correct: Phase 3 populates terminal_reason before Phase 4 reads it, avoiding the silent-disable-retries trap explicitly called out in the matrix. No open questions remain.
- **At:** 2026-05-06T09:50:39Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 1 complete. Added 9 typed lifecycle columns (daemon_epoch, claim_source, attempt, pid, heartbeat_at, postcondition_id, postcondition_args, terminal_reason, next_retry_at) plus substrate_migrations ledger to SUBSTRATE_DDL.dispatch_locks. ensure_dispatch_locks_typed() in src/handlers/agents_run.rs uses PRAGMA table_info to ALTER missing columns idempotently and records &#x27;L134-dispatch-locks-typed&#x27; in substrate_migrations only when columns are added. backfill_legacy_locks() updates only rows where claim_source IS NULL, deriving terminal_reason from last_status (ok/exit_nonzero/error/halted/legacy_unknown) without changing live lock semantics. Wired into run_daemon() before seed_starting_line and into db::open() so CLI flows also migrate. tests/dispatch_locks_migration.rs (2 tests) covers AC1.2 + AC1.3 against the OLD pre-L134 schema. All 833 lib tests + 26 agents_run tests pass; sidecar_handoff pre-existing failures verified unrelated via stash check.
- **Commit:** `edbada2`
- **Files:**
  - `src/codegen/ddl.rs`
  - `src/db.rs`
  - `src/handlers/agents_run.rs`
  - `tests/dispatch_locks_migration.rs`
- **At:** 2026-05-06T09:55:14Z
### Phase 2 / Cycle 1
- **Status:** Submitted — awaiting review
- **Summary:** Implemented Phase 2 postcondition registry. Added src/flow/postconditions.rs with PostconditionId enum, PostconditionFn type, lookup() and 5 pure predicates (task_exists_for_linked_observation, task_workspace_exists, drive_pid_recorded_or_terminal, cargo_installed_state, schema_migrated_state). Added postcondition_for_builtin() in flow::builtins/mod.rs mapping 5 builtin keywords to their postcondition_ids. 7 new unit tests in flow::postconditions and 1 in flow::builtins all pass; cargo build clean. No behavioural wiring (deferred to later phases per plan). Pre-existing sidecar_handoff failures verified on prior commit edbada2 — unrelated.
- **Commit:** `d6de109cff3f1bd1060acfbf7a43d385b02c0576`
- **Files:**
  - `src/flow/postconditions.rs`
  - `src/flow/mod.rs`
  - `src/flow/builtins/mod.rs`
- **At:** 2026-05-06T10:00:39Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Implemented T050 P2 postcondition registry in src/flow/postconditions.rs and exported it from src/flow/mod.rs. Added postcondition_for_builtin mapping and test coverage in src/flow/builtins/mod.rs. Verified cargo build, cargo test --lib flow::postconditions, and builtin mapping test pass; full cargo test still fails in unrelated tests/sidecar_handoff.rs.
- **Commit:** `d6de109cff3f1bd1060acfbf7a43d385b02c0576`
- **Files:**
  - `src/flow/postconditions.rs`
  - `src/flow/mod.rs`
  - `src/flow/builtins/mod.rs`
- **At:** 2026-05-06T10:47:54Z
### Phase 3 / Cycle 1
- **Status:** REVISE
- **Summary:** Implemented T050 P3 write-side lifecycle wiring: try_claim stamps daemon_epoch/claim_source/attempt/postcondition columns; terminal writes stamp terminal_reason/next_retry_at; retry claims/halt and watchdog silent-zombie paths use typed columns. Added agents_run T050 unit coverage and extended watchdog_silent_zombie_lock_already_closed for terminal_reason&#x3D;&#x27;silent_zombie&#x27;. Verified cargo build, cargo test --lib, handlers::agents_run T050 tests, watchdog silent-zombie test, drive_silent_zombie_e2e, flow_starting_line_e2e, and flow_promote_scaffold_drive_e2e pass.
- **Commit:** `ea89c23c74ee48940258d5c2b7ccb592a7cc00e9`
- **Files:**
  - `src/handlers/agents_run.rs`
  - `src/flow/builtins/auto_drive.rs`
- **At:** 2026-05-06T10:56:08Z
### Phase 3 / Cycle 2
- **Status:** PASS
- **Summary:** Fixed T050 P3 revise findings in src/handlers/agents_run.rs: retry claims now clear terminal_reason, finished_at, and next_retry_at, and find_retryable_locks excludes retrying rows. Fixed retry scheduling off-by-one by computing completed attempts from attempts+1; added regressions for duplicate retry eligibility and max_attempts&#x3D;3 third-dispatch scheduling. Verified cargo build, cargo test --lib, T050 agents_run tests, watchdog_silent_zombie_lock_already_closed, drive_silent_zombie_e2e, flow_starting_line_e2e, and flow_promote_scaffold_drive_e2e pass; full cargo test still fails in pre-existing/out-of-scope tests/sidecar_handoff.rs (missing --append-system-prompt / --message).
- **Commit:** `8959ad378a294ffc6632fc2bfabc41bab81f6fff`
- **Files:**
  - `src/handlers/agents_run.rs`
- **At:** 2026-05-06T11:01:24Z
### Phase 4 / Cycle 1
- **Status:** PASS
- **Summary:** T050 P4 implemented typed retry read path in src/handlers/agents_run.rs: find_retryable_locks gates on attempt/next_retry_at/terminal_reason, claim_for_retry CAS guards on attempt+terminal_reason, and legacy_unknown rows are covered by tests. Updated src/flow/builtins/auto_drive.rs scan_zombie_tasks to exclude terminal_reason&#x3D;&#x27;silent_zombie&#x27; and added regression coverage. cargo build, cargo test --lib agents_run::tests, and the new auto_drive T050 test pass; full cargo test still fails pre-existing tests/sidecar_handoff.rs (missing --append-system-prompt / --message).
- **Commit:** `c5d2b02111544c5e603f923b38f28a8ac746bebb`
- **Files:**
  - `src/handlers/agents_run.rs`
  - `src/flow/builtins/auto_drive.rs`
- **At:** 2026-05-06T11:08:00Z
### Phase 5 / Cycle 1
- **Status:** REVISE
- **Summary:** T050 P5 implemented tests/dispatch_locks_typed_regression.rs with 5 named L087/L107/L116/L122/L141 typed-column regression tests; cargo test --test dispatch_locks_typed_regression passes 5/5 and cargo build succeeds. Updated src/handlers/agents_run.rs so starting-line seed rows are stamped claim_source&#x3D;&#x27;legacy&#x27;, attempt&#x3D;0, terminal_reason&#x3D;&#x27;legacy_unknown&#x27;. cargo test --workspace was attempted and fails in unrelated pre-existing tests/sidecar_handoff.rs (3 failures: missing --append-system-prompt / --message).
- **Commit:** `385f4975aced3e5cc4e20b9e1bc33dd7c660a106`
- **Files:**
  - `src/handlers/agents_run.rs`
  - `tests/dispatch_locks_typed_regression.rs`
- **At:** 2026-05-06T11:13:46Z
### Phase 5 / Cycle 2
- **Status:** PASS
- **Summary:** Fixed Phase 5 revise findings: tests/dispatch_locks_typed_regression.rs now restores STORES_DRIVE_CMD with an RAII guard; sidecar_handoff and workflow_tier_t1 regressions were updated to current argv/T1 schema behavior; topology snapshots were refreshed so cargo test --workspace passes. Verified cargo build, cargo test --test dispatch_locks_typed_regression (5/5), cargo test --test sidecar_handoff, and cargo test --workspace pass; working tree is clean.
- **Commit:** `2c276130f3f033d8c8c3d0868b486e08caebb1a6`
- **Files:**
  - `tests/dispatch_locks_typed_regression.rs`
  - `tests/fixtures/sidecar/mock-claude.sh`
  - `tests/fixtures/topology/expected.dot`
  - `tests/fixtures/topology/expected.md`
  - `tests/sidecar_handoff.rs`
  - `tests/workflow_tier_t1.rs`
- **At:** 2026-05-06T11:20:19Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified mechanically: cargo build succeeds, codegen::ddl tests pass (16/16) with snapshot updated, tests/dispatch_locks_migration.rs covers AC1.2 (3 legacy rows → ok/exit_nonzero/error, claim_source&#x3D;&#x27;legacy&#x27;, exactly one L134 substrate_migrations row) and AC1.3 (idempotent — n2&#x3D;0, no new migration row, row contents unchanged), and all 26 agents_run::tests pass unchanged (AC1.4). Commit edbada2 contains exactly the 4 claimed files (ddl.rs +14, db.rs +6, agents_run.rs +93, dispatch_locks_migration.rs +193). 0 critical, 0 major, 4 minor (style/coverage nits).
- **Findings:** 0 critical, 0 major, 4 minor
**Details:**
[MINOR] daemon_epoch &#x3D; &#x27;&#x27; (empty string sentinel) for legacy rows is inconsistent with next_retry_at &#x3D; NULL on the same backfilled rows. Readers may special-case &#x27;&#x27; vs NULL. Suggestion: use NULL for both, or document why &#x27;&#x27; was chosen for daemon_epoch specifically (e.g. as a non-null &#x27;legacy/unknown epoch&#x27; sentinel that downstream code can safely string-compare).
File: src/handlers/agents_run.rs:638

[MINOR] attempt &#x3D; COALESCE(attempts, 1) is dead defensive code. The legacy DDL declares &#x60;attempts INTEGER NOT NULL DEFAULT 1&#x60;, so attempts is never NULL. Suggestion: just &#x60;attempt &#x3D; attempts&#x60; — or keep COALESCE and note in a one-line comment that it&#x27;s defensive against pre-default-NOT-NULL DBs.
File: src/handlers/agents_run.rs:629

[MINOR] Backfill CASE branches for &#x27;halted:%&#x27; and &#x27;skip-historical&#x27; are not exercised by the migration tests. AC1.2 only required ok/exit&#x3D;11/error coverage (which passes), but the implementation added additional branches (halted, skip-historical, legacy_unknown fallthrough) without test coverage. Suggestion: extend migration_adds_typed_columns_and_backfills_legacy_rows with two more legacy rows (&#x27;halted: shutdown&#x27;, &#x27;skip-historical&#x27;) and assert their terminal_reason values. Low-risk gap; the patterns are simple and the fallthrough is sound.
File: tests/dispatch_locks_migration.rs:93-95

[MINOR] migration_adds_typed_columns_and_backfills_legacy_rows does not assert that &#x60;attempt&#x60; was backfilled (e.g. attempt &#x3D;&#x3D; 1 after backfill, derived from attempts&#x3D;1). claim_source is asserted (&#x27;legacy&#x27;) and terminal_reason is asserted, but the attempt column — one of the 9 typed columns — is only checked for existence in PRAGMA, not for value. Suggestion: add an assertion &#x60;assert_eq!(attempt_value, 1)&#x60; for at least one row.
File: tests/dispatch_locks_migration.rs:117-127

[INFORMATIONAL] db::open now performs a write-transaction (idempotent UPDATE) on every CLI verb that opens the DB, including read-mostly verbs like &#x60;status&#x60;, &#x60;render&#x60;, &#x60;next-action&#x60;. The UPDATE matches 0 rows once migrated, so cost is negligible — flagging only because it changes the read-only invariant of those verbs slightly. No action required.
File: src/db.rs:13-19

[INFORMATIONAL] src/handlers/mod.rs is listed in &#x27;Expected Files&#x27; but was not modified. The two new pub fns are exposed via the existing &#x60;pub mod agents_run&#x60; declaration, so no mod.rs edit was needed. Not a finding.
- **At:** 2026-05-06T09:56:43Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All Phase 2 ACs verified: cargo test --lib flow::postconditions passes with 7 tests, lookup covers all 5 registered ids plus unknown, postcondition_for_builtin maps all 5 builtin keywords plus unknown, cargo build and cargo test --lib pass. 0 critical, 0 major, 0 minor findings.
- **Findings:** 0 critical, 0 major, 0 minor
**Details:**
AC2.1 PASS: &#x60;cargo test --lib flow::postconditions&#x60; ran 7 tests and all passed; the five predicate-specific tests each assert satisfying and failing substrate rows.
AC2.2 PASS: &#x60;lookup_returns_some_for_known_ids_and_none_for_unknown&#x60; verifies all five registered postcondition_id strings return Some and unknown/empty return None; implementation in src/flow/postconditions.rs:58-67 matches the Phase 2 registry.
AC2.3 PASS: &#x60;cargo test --lib flow::builtins::tests::postcondition_for_builtin_mapping&#x60; passed; src/flow/builtins/mod.rs:71-79 maps auto-promote, auto-scaffold, auto-drive, cargo-install, and schema-migrate to the documented ids and returns None for unknown.
Build/regression evidence: &#x60;cargo build&#x60; passed; &#x60;cargo test --lib&#x60; passed 841/841.
Git evidence: commit d6de109cff3f1bd1060acfbf7a43d385b02c0576 is valid and contains exactly the claimed tracked files for this phase (src/flow/postconditions.rs, src/flow/mod.rs, src/flow/builtins/mod.rs). &#x60;git status --porcelain&#x60; shows only untracked task metadata directories, not tracked code changes.
- **At:** 2026-05-06T10:49:07Z

### Phase 3 / Cycle 1
- **Gate:** REVISE
- **Summary:** REVISE. AC3.1–AC3.5 targeted tests and cargo build/cargo test --lib pass, but retry write-side lifecycle wiring has two major correctness bugs: in-flight retry rows remain eligible for duplicate claims, and retry scheduling is off by one attempt. 0 critical, 2 major, 1 minor.
- **Findings:** 0 critical, 2 major, 1 minor
**Details:**
[MAJOR] Retry claim leaves the row retry-eligible while the retry is in flight, allowing duplicate retry dispatches.
File: src/handlers/agents_run.rs:1137-1142 and 1181-1194
Evidence: claim_for_retry only sets last_status&#x3D;&#x27;retrying&#x27;, claim_source&#x3D;&#x27;retry_claim&#x27;, and attempt&#x3D;...; it does not clear terminal_reason, next_retry_at, or finished_at. find_retryable_locks selects rows solely by attempts &lt; max, finished_at IS NOT NULL, terminal_reason IN (&#x27;exit_nonzero&#x27;,&#x27;error&#x27;,&#x27;silent_zombie&#x27;), next_retry_at IS NOT NULL, next_retry_at &lt;&#x3D; now. After one daemon claims a retry, those predicates remain true, and a later poll/daemon can SELECT the same row with last_status_snapshot&#x3D;&#x27;retrying&#x27;; the CAS then succeeds because WHERE last_status&#x3D;?5 matches &#x27;retrying&#x27;. This re-opens the duplicate-dispatch/duplicate-drive class the typed lifecycle is meant to close.
Expected: A retry claim should move the row out of retry-eligible terminal state until the retry finishes.
Suggestion: In claim_for_retry, clear next_retry_at and either clear terminal_reason/finished_at or otherwise mark a nonterminal in-flight state; also add an explicit guard in find_retryable_locks such as last_status !&#x3D; &#x27;retrying&#x27; / next_retry_at consumed. Add a regression test where a second find_retryable_locks after claim_for_retry returns no candidate.

[MAJOR] completed_attempt is computed off-by-one after retry_claim, suppressing valid retries before max_attempts.
File: src/handlers/agents_run.rs:988-996 and 1139-1140
Evidence: claim_for_retry pre-increments attempt to COALESCE(attempt, attempts, 0) + 1. mark_claim_finished_typed then computes completed_attempt as COALESCE(attempt, attempts, 0) + 1. For a max_attempts&#x3D;3 always-failing subscriber: after first failure attempts&#x3D;1/attempt&#x3D;0; retry claim sets attempt&#x3D;2; finishing that retry computes completed_attempt&#x3D;3 and next_retry_at_for returns None because 3 &lt; 3 is false, even though attempts is only incremented to 2 and the third allowed dispatch should still be scheduled.
Expected: retry_eligibility/next_retry_at should be based on the attempt that just completed, matching attempts after the UPDATE. With max_attempts&#x3D;3, a second failed dispatch should schedule the third dispatch.
Suggestion: Do not add 1 to the already-claimed attempt on finish, or compute from attempts + 1 consistently and keep attempt as the zero/one-based current attempt with documented semantics. Add a test for max_attempts&#x3D;3 where two failures still leave next_retry_at populated and a third poll fires.

[MINOR] Worktree is dirty outside the submitted commit.
File: git status
Evidence: git status --porcelain shows modified tasks/active/T001-test-task/main.md, tasks/planning/T001-test-task/main.md and untracked T050 task directories. Executor submission only claimed source files in commit ea89c23. This does not affect the Rust diff under review, but the review did not start from a clean tree.
Expected: Reviewable phase submission should normally leave only the committed implementation diff, or explain workflow-generated/untracked task artifacts.
Suggestion: Commit/stash/remove unrelated task-file changes before re-submission or call them out explicitly if they are orchestrator-generated.

Verification run:
- git log --oneline -10: HEAD ea89c23 T050 P3: populate typed dispatch_locks lifecycle writes.
- git diff --name-only HEAD~3 includes prior P1/P2 files plus P3 source files; commit ea89c23 itself changes src/handlers/agents_run.rs and src/flow/builtins/auto_drive.rs as claimed.
- cargo build: PASS.
- cargo test handlers::agents_run::tests::t050 -- --nocapture: PASS (4 tests).
- cargo test flow::builtins::auto_drive::tests::watchdog_silent_zombie_lock_already_closed -- --nocapture: PASS.
- cargo test --test drive_silent_zombie_e2e -- --nocapture: PASS (2 tests).
- cargo test --test flow_starting_line_e2e -- --nocapture: PASS (2 tests).
- cargo test --test flow_promote_scaffold_drive_e2e -- --nocapture: PASS (4 tests).
- cargo test --lib: PASS (845 tests).
- **At:** 2026-05-06T10:57:58Z

### Phase 3 / Cycle 2
- **Gate:** PASS
- **Summary:** PASS. Prior retry lifecycle findings are addressed, and AC3.1–AC3.5 were mechanically verified: build, agents_run T050 tests, watchdog silent-zombie test, cargo test --lib, and required e2e tests all pass. 0 critical, 0 major, 2 minor findings remain.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
Verification:
- git log --oneline -10: HEAD is 8959ad3 T050 P3 revise: consume retry eligibility during retry claims.
- git diff 8959ad3~ 8959ad3 --stat: only src/handlers/agents_run.rs changed in the submitted commit, matching executor submission.
- cargo build: PASS.
- cargo test --lib t050 -- --nocapture: PASS (5 T050 tests, including AC3.1/AC3.2/AC3.4 and retry regressions).
- cargo test flow::builtins::auto_drive::tests::watchdog_silent_zombie_lock_already_closed -- --nocapture: PASS (AC3.3).
- cargo test --lib: PASS (846 tests).
- cargo test --test drive_silent_zombie_e2e / flow_starting_line_e2e / flow_promote_scaffold_drive_e2e: PASS (2/2, 2/2, 4/4).

Prior REVISE consistency:
- Duplicate retry eligibility: addressed. claim_for_retry now clears terminal_reason, next_retry_at, and finished_at; regression asserts find_retryable_locks is empty after claim.
- Retry scheduling off-by-one: addressed. finish computes completed_attempt from attempts+1, and max_attempts&#x3D;3 regression schedules the third dispatch.

[MINOR] Worktree remains dirty outside the submitted commit.
File: git status
Evidence: git status --porcelain shows modified tasks/active/T001-test-task/main.md and tasks/planning/T001-test-task/main.md plus untracked tasks/active/T050-auto-promoted-l134/ and tasks/planning/T050-auto-promoted-l134/.
Expected: Review submissions should normally leave a clean tree or explicitly identify workflow-generated task artifacts.
Suggestion: Clean, commit, or explicitly document unrelated task-file artifacts before the next phase submission.

[MINOR] Retry claim lifecycle metadata is still only partially refreshed.
File: src/handlers/agents_run.rs:1137-1144
Evidence: claim_for_retry updates claim_source&#x3D;&#x27;retry_claim&#x27;, attempt, terminal_reason, next_retry_at, and finished_at, but does not refresh daemon_epoch/claimed_at/claimed_by for the daemon currently taking the retry.
Expected: Not an AC3 blocker after this revise, but the phase objective is typed write-side lifecycle wiring; stale claim metadata may be confusing for later daemon/stale diagnostics.
Suggestion: In a follow-up phase or small cleanup, pass claimer/daemon_epoch into claim_for_retry and refresh claimed_at/claimed_by/daemon_epoch when the retry CAS succeeds.
- **At:** 2026-05-06T11:03:24Z

### Phase 4 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. Phase 4 acceptance criteria are mechanically satisfied: typed retry read path excludes legacy_unknown, elapsed exit_nonzero rows are retry candidates, typed CAS admits one claimant, and agents_run retry tests pass. 0 critical, 0 major, 2 minor findings; full cargo test still fails only in unrelated sidecar_handoff tests as the executor disclosed.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[MINOR] Worktree has unrelated dirty/untracked task-render files not included in executor files_changed.
File: tasks/active/T001-test-task/main.md, tasks/planning/T001-test-task/main.md, tasks/active/T050-auto-promoted-l134/, tasks/planning/T050-auto-promoted-l134/
Evidence: git status --porcelain shows modified T001 task files and untracked T050 task dirs; executor claimed only src/handlers/agents_run.rs and src/flow/builtins/auto_drive.rs. Commit c5d2b02 itself only changes the two claimed source files.
Expected: Review worktree should be clean or submission should explain non-code task-file artifacts.
Suggestion: Clean or commit task-render artifacts separately before the next phase to keep review diffs unambiguous.

[MINOR] AC4.1 regression test does not isolate terminal_reason as the reason legacy_unknown is excluded.
File: src/handlers/agents_run.rs:t050_legacy_unknown_rows_are_not_retry_candidates
Evidence: The inserted legacy_unknown row has next_retry_at &#x3D; NULL, so find_retryable_locks would exclude it even without the terminal_reason IN (...) predicate. Production backfill also sets next_retry_at NULL, so behavior is correct, but the test is weaker than the AC wording.
Expected: AC4.1 says a terminal_reason&#x3D;&#x27;legacy_unknown&#x27; row should return 0 candidates; best coverage would prove the typed terminal_reason gate by setting next_retry_at in the past and still expecting no retry.
Suggestion: Optionally add a second assertion/row with terminal_reason&#x3D;&#x27;legacy_unknown&#x27; and next_retry_at&#x3D;&#x27;2000-01-01T00:00:01Z&#x27; to prove legacy_unknown is non-retry-eligible by type, not only by NULL retry time.

[INFORMATIONAL] Git reality: git log HEAD is c5d2b02 T050 P4; git show c5d2b02 --stat reports src/flow/builtins/auto_drive.rs and src/handlers/agents_run.rs only.
[INFORMATIONAL] AC4.1: t050_legacy_unknown_rows_are_not_retry_candidates passes under cargo test --lib agents_run::tests.
[INFORMATIONAL] AC4.2: t050_exit_nonzero_retryable_and_typed_cas_single_winner passes; code uses terminal_reason/attempt snapshots in claim_for_retry.
[INFORMATIONAL] AC4.3: cargo test --lib agents_run::tests passes 33/33; cargo build passes. Full cargo test fails only tests/sidecar_handoff.rs missing --append-system-prompt/--message, outside the changed files and disclosed by executor.
- **At:** 2026-05-06T11:09:26Z

### Phase 5 / Cycle 1
- **Gate:** REVISE
- **Summary:** REVISE. AC5.1 passes and the l107/l141 tests appear to gate postcondition demotion, but AC5.3 fails because cargo test --workspace is red. 0 critical, 1 major, 2 minor findings.
- **Findings:** 0 critical, 1 major, 2 minor
**Details:**
[MAJOR] AC5.3 workspace test suite does not pass
File: tests/sidecar_handoff.rs (existing failing integration test target)
Evidence: Ran &#x60;cargo test --workspace&#x60;; result ended with &#x60;error: test failed, to rerun pass --test sidecar_handoff&#x60;. Failures: &#x60;token_round_trip_through_chat_context_not_env&#x60; panicked at tests/sidecar_handoff.rs:214 with &#x60;--message arg must be present&#x60;; &#x60;per_row_handoff_passes_token_in_message_and_priming_file&#x60; failed after &#x60;missing --append-system-prompt&#x60;; &#x60;obs_draft_handoff_returns_obs_draft_body&#x60; failed after &#x60;missing --append-system-prompt&#x60;. Expected: AC5.3 explicitly requires &#x60;cargo test --workspace&#x60; passes overall. Suggestion: Either fix the sidecar_handoff regressions in this phase or provide/commit an appropriate test isolation/configuration fix so the workspace suite is green under the normal command.

[MINOR] Working tree is dirty with uncommitted task artifacts not in the executor submission
File: tasks/active/T001-test-task/main.md; tasks/planning/T001-test-task/main.md; tasks/active/T050-auto-promoted-l134/; tasks/planning/T050-auto-promoted-l134/
Evidence: &#x60;git status --porcelain&#x60; showed modified T001 task files and untracked T050 task directories. Executor claimed only &#x60;src/handlers/agents_run.rs&#x60; and &#x60;tests/dispatch_locks_typed_regression.rs&#x60;; the commit itself contains only those two files, but the review workspace is not clean. Expected: reviewable submissions should leave no unexplained uncommitted changes. Suggestion: clean, commit, or explicitly segregate workflow-generated task artifacts before re-submitting.

[MINOR] Environment variable cleanup in regression tests is not panic-safe
File: tests/dispatch_locks_typed_regression.rs:199-202 and 221-224
Evidence: tests set &#x60;STORES_DRIVE_CMD&#x60; and manually remove it after &#x60;poll_once&#x60;; if &#x60;poll_once&#x60; or an assertion before removal panics, the process environment remains contaminated for later tests despite the mutex. Expected: robust test isolation for global state. Suggestion: use a small RAII guard that restores/removes &#x60;STORES_DRIVE_CMD&#x60; in Drop, or wrap the mutation in a helper.

Acceptance evidence:
AC5.1 PASS: &#x60;grep -n &quot;fn l087_\|fn l107_\|fn l116_\|fn l122_\|fn l141_&quot; tests/dispatch_locks_typed_regression.rs&#x60; found five named tests; &#x60;cargo test --test dispatch_locks_typed_regression&#x60; passed 5/5.
AC5.2 PASS by static gating evidence: l107 asserts &#x60;postcondition_id &#x3D;&#x3D; task_workspace_exists&#x60;, &#x60;terminal_reason &#x3D;&#x3D; error&#x60;, and postcondition failure text; l141 asserts &#x60;postcondition_id &#x3D;&#x3D; drive_pid_recorded_or_terminal&#x60;, &#x60;drive_pid &#x3D;&#x3D; None&#x60;, &#x60;terminal_reason &#x3D;&#x3D; error&#x60;, and postcondition failure text. Reverting postcondition demotion should make these expect-error tests fail.
AC5.3 FAIL: &#x60;cargo test --workspace&#x60; failed in &#x60;tests/sidecar_handoff.rs&#x60; as documented above.
Git reality: &#x60;git show 385f4975aced3e5cc4e20b9e1bc33dd7c660a106 --stat&#x60; confirms the submitted commit changed &#x60;src/handlers/agents_run.rs&#x60; and &#x60;tests/dispatch_locks_typed_regression.rs&#x60;, matching the claimed committed files.
- **At:** 2026-05-06T11:14:47Z

### Phase 5 / Cycle 2
- **Gate:** PASS
- **Summary:** PASS. AC5.1 and AC5.3 pass mechanically, and AC5.2 is supported by the l107/l141 tests asserting typed postcondition demotion outcomes. 0 critical, 0 major, 1 minor test-isolation finding.
- **Findings:** 0 critical, 0 major, 1 minor
**Details:**
Acceptance evidence:
AC5.1 PASS: &#x60;grep -n &quot;fn l087_\|fn l107_\|fn l116_\|fn l122_\|fn l141_&quot; tests/dispatch_locks_typed_regression.rs&#x60; found exactly five named tests; &#x60;cargo test --test dispatch_locks_typed_regression&#x60; passed 5/5.
AC5.2 PASS by static gating evidence: l107 asserts &#x60;postcondition_id &#x3D;&#x3D; task_workspace_exists&#x60;, &#x60;terminal_reason &#x3D;&#x3D; error&#x60;, and postcondition failure text; l141 asserts &#x60;postcondition_id &#x3D;&#x3D; drive_pid_recorded_or_terminal&#x60;, &#x60;drive_pid &#x3D;&#x3D; None&#x60;, &#x60;terminal_reason &#x3D;&#x3D; error&#x60;, and postcondition failure text. The production demotion path in &#x60;mark_claim_finished_typed&#x60; only rewrites ok to error when &#x60;run_postcondition_for_lock&#x60; fails, so reverting that Phase 3 behavior would make these expect-error tests fail.
AC5.3 PASS: &#x60;cargo test --workspace&#x60; passed: 800 unit tests plus all integration/doc tests, including &#x60;sidecar_handoff&#x60;, &#x60;workflow_tier_t1&#x60;, and &#x60;dispatch_locks_typed_regression&#x60;.
Git reality: initial &#x60;git status --porcelain&#x60; was clean; submitted commit &#x60;2c276130f3f033d8c8c3d0868b486e08caebb1a6&#x60; is valid and changes the six files claimed by the executor. &#x60;cargo build&#x60; also succeeds.

[MINOR] Workspace tests leave task-render artifacts in the working tree
File: tests/workflow_tier_t1.rs / other workflow integration tests using task rendering
Evidence: Initial &#x60;git status --porcelain&#x60; was clean before verification. After running &#x60;cargo test --workspace&#x60;, &#x60;git status --porcelain&#x60; showed modified &#x60;tasks/active/T001-test-task/main.md&#x60;, &#x60;tasks/planning/T001-test-task/main.md&#x60;, and untracked &#x60;tasks/active/T001-t1-task/&#x60;, &#x60;tasks/active/T100-t3-task/&#x60;, &#x60;tasks/planning/T100-t3-task/&#x60;.
Expected: Integration tests should normally isolate filesystem writes under temporary directories and not dirty the repository checkout.
Suggestion: In a follow-up, set the task rendering root/meta path to a tempdir for workflow integration tests or clean up generated task directories in test teardown. This does not block Phase 5 because AC5.3 only requires the workspace suite to pass, and the executor&#x27;s submitted tree was clean before tests ran.
- **At:** 2026-05-06T11:21:36Z

---

## Completion
- **In Review:** 2026-05-06T11:22:07Z — awaiting human GO/NO_GO

