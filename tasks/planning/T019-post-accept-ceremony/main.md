# T019: Post-accept ceremony - cargo install + schema migrate as L018 subscribers

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T12:11:02Z
- **Last Updated:** 2026-05-03T12:29:07Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T019-post-accept-ceremony

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** src/handlers/builtin_cargo_install.rs (new); src/handlers/builtin_schema_migrate.rs (new); registration of these two builtins in the agents.yaml schema&#x27;s known-builtin list (T014&#x27;s contribution); default agents.yaml example showing the chained post-accept ceremony; unit + integration tests covering all (5)(a)-(f) scenarios with mock daemon dispatch; README docs update replacing manual migration runbook with auto-ceremony note.
- **Out:** T014&#x27;s agents.yaml schema, daemon, claim model, chain mechanics, deploy_blocked state, builtin:user-escalation (depends on; not modified — these are the prerequisites). Close-linked-observations-on-accept subscriber (separate follow-up; file as fresh observation when T014 lands). Branch-delete-on-accept subscriber (separate). Multi-host daemon coordination (single-host MVP only). Custom non-cargo build commands (default cargo install only; extensible later via agents.yaml command_args). Task dependency / chain enforcement (separate observation; depends_on field today is stored but unused). L013/L014/L015 auth UX cluster. L020/L021/L023 papercuts. L030 (tier-as-planner-input briefs). L035 (schema-enforced context flow). L032 (worktree substrate access).

### Done When
(1) New builtin subscriber &#x60;builtin:cargo-install&#x60; registered as a known agents.yaml entry alongside builtin:accept-merge from T014. Subscribes to the post-accept-merge transition (row state accepted, branch already merged into main). On fire: runs &#x60;cargo install --path &lt;project_root&gt; --features &lt;configured-features&gt; --quiet&#x60; from project root. Default features: &#x60;runner-claude-code&#x60;. Override via agents.yaml entry&#x27;s command_args.features (or equivalent). On success: emits success log; daemon&#x27;s chain proceeds to next subscriber. On failure: row → deploy_blocked with stderr captured in blocked_reason; ntfy fires; deployment-specialist agent picks it up.

(2) New builtin subscriber &#x60;builtin:schema-migrate&#x60; registered. Subscribes to post-cargo-install (chains after step 1). On fire: runs &#x60;stores migrate --apply&#x60;. On success (no-op or applied): log; chain proceeds. On failure: row → deploy_blocked; ntfy; specialist routing.

(3) Default agents.yaml example updated to show the full post-accept chain in dependency order: builtin:accept-merge (T014, branch into main) → builtin:cargo-install (this task, binary refresh) → builtin:schema-migrate (this task, DB schema sync). Each subscriber fires only after its predecessor reports success. Failure at any link halts the chain and routes to specialist.

(4) Daemon dispatch isolation: cargo install can take 1-2 min; it runs in its own claim window without blocking other tasks&#x27; dispatch (already handled by T014&#x27;s per-row claim model; confirm with a test that two concurrent post-accept chains don&#x27;t interfere).

(5) Tests cover: (a) cargo-install fires post-accept-merge, succeeds, chains to schema-migrate; (b) cargo-install fails (mock failing build) → deploy_blocked with stderr in blocked_reason; (c) schema-migrate fires post-cargo-install, no-op on in-sync DB, exits clean; (d) schema-migrate detects new schema columns (mock schema change), applies them, reports success; (e) schema-migrate fails (mock SQL error) → deploy_blocked; (f) chain failure isolation — accept-merge of task A doesn&#x27;t block dispatch of task B&#x27;s accept-merge.

(6) README &quot;Schema migrations&quot; section updated: replace the manual runbook with a note that these subscribers run automatically as part of the post-accept ceremony; keep the manual &#x60;stores migrate&#x60; verb available for ad-hoc / debug use.

### Phases

#### Phase 1: Phase 1: Schema — post-accept chain transitions
- **Objective:** Extend tasks schema with framework-actor transitions that mark cargo-install and schema-migrate success, giving the chain real edges in transition_history for the daemon to subscribe to.
- **Tasks:**
  - Task 1.1: Add states (or status enum values) &#x60;cargo_installed&#x60; and &#x60;schema_migrated&#x60; to stores/tasks/schema.yaml lifecycle (or, if reviewers prefer status to remain &#x60;accepted&#x60;, add transitions whose to-state&#x3D;&#x3D;from-state&#x3D;&#x3D;&#x60;accepted&#x60; keyed by verb — see decision matrix Q1).
  - Task 1.2: Add transitions &#x60;mark_cargo_installed&#x60; (from accepted, actor&#x3D;framework) and &#x60;mark_schema_migrated&#x60; (from cargo_installed, actor&#x3D;framework) mirroring T014&#x27;s &#x60;mark_deploy_blocked&#x60; shape.
  - Task 1.3: Update bundled tasks schema constant + run codegen to ensure DDL recognises any new state literal; verify &#x60;stores migrate&#x60; (T017) reports clean diff against the bundled schema.
- **Acceptance Criteria:**
  - [ ] AC1.1: cargo build succeeds; cargo test schema:: passes.
  - [ ] AC1.2: A new test in src/handlers/transition.rs proves a framework-invoker &#x60;mark_cargo_installed&#x60; transition writes to transition_history with verb&#x3D;mark_cargo_installed and invoker&#x3D;framework.
  - [ ] AC1.3: &#x60;stores migrate&#x60; against a fresh DB after applying current substrate DDL prints &#x60;no schema drift&#x60; (idempotent).
- **Files:** `stores/tasks/schema.yaml`, `src/handlers/transition.rs`, `src/codegen/ddl.rs`
- **Dependencies:** T014 branch must be the integration base for this task (rebase feat/T019 onto feat/T014-autonomous-flow-engine before Phase 1 — see open question Q0).
#### Phase 2: Phase 2: builtin:cargo-install
- **Objective:** Add a new builtin subscriber that runs &#x60;cargo install --path &lt;project_root&gt; --features &lt;features&gt; --quiet&#x60; from the row&#x27;s workspace_path on the post-accept-merge transition; on success fire mark_cargo_installed; on failure flip to deploy_blocked with stderr captured and route to deployment_specialist.
- **Tasks:**
  - Task 2.1: Create src/flow/builtins/cargo_install.rs cloning the structural shape of accept_merge.rs (DispatchCtx signature, fire helper, specialist dispatch).
  - Task 2.2: Resolve project root via T014&#x27;s &#x60;resolve_main_repo(workspace_path)&#x60; helper (already pub(crate)).
  - Task 2.3: Read features from agents.yaml entry&#x27;s command_args.features (default &#x60;runner-claude-code&#x60;); spawn &#x60;cargo install --path &lt;root&gt; --features &lt;csv&gt; --quiet&#x60;; capture stdout/stderr.
  - Task 2.4: On exit&#x3D;&#x3D;0: call existing &#x60;execute_transition_write&#x60; to fire &#x60;mark_cargo_installed&#x60; (framework actor) so the daemon&#x27;s transition_history poll picks up the next link; emit &#x60;[cargo-install] &lt;id&gt;: ok&#x60; log; return Ok(0).
  - Task 2.5: On exit!&#x3D;0: build blocked_reason from last 20 stderr lines; call &#x60;fire_mark_deploy_blocked&#x60; helper (extract from accept_merge.rs into builtins/mod.rs as pub(crate) so cargo-install + schema-migrate share it); fire ntfy; dispatch_to_specialist (also extract); return Ok(0).
  - Task 2.6: Register &#x60;&quot;cargo-install&quot; &#x3D;&gt; Some(cargo_install::run(...))&#x60; arm in src/flow/builtins/mod.rs::dispatch_builtin and add &#x60;pub mod cargo_install;&#x60;.
  - Task 2.7: Extend AgentsYaml schema&#x27;s known-builtin allow-list (in src/flow/agents_yaml.rs validate(): add &#x60;cargo-install&#x60; to the recognised keyword set if T014 enforces one — confirm with T014&#x27;s parser; if T014 already accepts any non-empty kw, no change).
- **Acceptance Criteria:**
  - [ ] AC2.1: cargo build succeeds; cargo clippy clean.
  - [ ] AC2.2: Unit test &#x60;i_cargo_install_clean_chains_to_mark_cargo_installed&#x60; in src/flow/builtins/mod.rs#tests: insert accepted task with workspace_path&#x3D;tempdir of a tiny crate fixture (fixture has Cargo.toml + lib.rs in tests/fixtures/cargo-install-noop/ that compiles in &lt;30s), run cargo_install::run, assert transition_history has a row with verb&#x3D;mark_cargo_installed for the row.
  - [ ] AC2.3: Unit test &#x60;j_cargo_install_failure_flips_deploy_blocked&#x60;: workspace_path points at a tempdir whose Cargo.toml has a deliberate compile error; assert row.status&#x3D;&#x3D;&#x27;deploy_blocked&#x27;, blocked_reason contains a substring of cargo&#x27;s stderr (e.g. &#x27;error[E&#x27;) and the failing crate path; ntfy MockNotifier captured one event with transition_attempted containing &#x27;deploy_blocked&#x27;.
  - [ ] AC2.4: agents.yaml fixture with &#x60;command: &quot;builtin:cargo-install&quot;&#x60; parses without error in src/flow/agents_yaml.rs::tests.
- **Files:** `src/flow/builtins/cargo_install.rs`, `src/flow/builtins/mod.rs`, `src/flow/agents_yaml.rs`, `tests/fixtures/cargo-install-noop/Cargo.toml`, `tests/fixtures/cargo-install-noop/src/lib.rs`, `tests/fixtures/cargo-install-broken/Cargo.toml`, `tests/fixtures/cargo-install-broken/src/lib.rs`
- **Dependencies:** Phase 1 transitions exist, Phase 2.5 helper extraction precedes Phase 3 so schema-migrate can reuse fire_mark_deploy_blocked + dispatch_to_specialist.
#### Phase 3: Phase 3: builtin:schema-migrate
- **Objective:** Add a builtin subscriber to the post-cargo-install transition that runs &#x60;stores migrate --apply&#x60; (or in-process equivalent of the T017 migrate handler) and chains success / flips deploy_blocked on failure.
- **Tasks:**
  - Task 3.1: Create src/flow/builtins/schema_migrate.rs mirroring cargo_install.rs structure.
  - Task 3.2: Prefer in-process call: invoke &#x60;crate::handlers::migrate::run_apply(&amp;conn)&#x60; (or the existing T017 entry point — confirm symbol name in src/handlers/migrate.rs) rather than shelling out, to avoid spawning the same binary.
  - Task 3.3: On in-sync DB: handler returns Ok with no-op flag → log &#x60;[schema-migrate] &lt;id&gt;: no-op (in-sync)&#x60;; on applied: log applied column count.
  - Task 3.4: On success: fire &#x60;mark_schema_migrated&#x60; framework transition (terminal — no further chain).
  - Task 3.5: On failure: capture migrate error string, fire_mark_deploy_blocked + ntfy + specialist (shared helpers from Phase 2.5).
  - Task 3.6: Register &#x60;&quot;schema-migrate&quot; &#x3D;&gt; Some(schema_migrate::run(...))&#x60; in dispatch_builtin and &#x60;pub mod schema_migrate;&#x60;.
- **Acceptance Criteria:**
  - [ ] AC3.1: cargo build + clippy clean.
  - [ ] AC3.2: Test &#x60;c_schema_migrate_no_op_in_sync&#x60;: fresh DB whose ddl matches the bundled schema; run schema_migrate::run; assert exit 0, no transition_history flip to deploy_blocked, mark_schema_migrated row present.
  - [ ] AC3.3: Test &#x60;d_schema_migrate_applies_new_columns&#x60;: open DB, drop one column from a substrate table to simulate drift, run schema_migrate::run, assert column re-added and mark_schema_migrated fired.
  - [ ] AC3.4: Test &#x60;e_schema_migrate_failure_blocks&#x60;: monkey-patch migrate to return Err (e.g. by passing a read-only connection or invalid DB), assert row → deploy_blocked, blocked_reason contains the error text.
- **Files:** `src/flow/builtins/schema_migrate.rs`, `src/flow/builtins/mod.rs`
- **Dependencies:** Phase 1 transitions, Phase 2 shared helpers extracted, T017 migrate handler exposes a callable in-process API (verify in src/handlers/migrate.rs — if only CLI-bound, expose a new &#x60;pub fn apply(conn: &amp;Connection) -&gt; Result&lt;MigrateReport&gt;&#x60; in Phase 3.0).
#### Phase 4: Phase 4: agents.yaml default chain + chain-isolation test
- **Objective:** Ship a default agents.yaml example showing the three-link post-accept ceremony in dependency order and prove that two concurrent post-accept chains do not interfere.
- **Tasks:**
  - Task 4.1: Update the default agents.yaml example (location TBD — likely docs/agents-yaml-example.yaml or embedded in src/flow/agents_yaml.rs default_empty) to declare three agents: accept-merge subscribed to in_review→accepted, cargo-install subscribed to accepted→cargo_installed, schema-migrate subscribed to cargo_installed→schema_migrated.
  - Task 4.2: Add a fixture tests/fixtures/agents-yaml/post-accept-chain.yaml mirroring the example and parse-test it (existing AgentsYaml::from_yaml).
  - Task 4.3: Add integration test tests/flow_chain_isolation.rs: insert two accepted tasks (T100, T101) on independent tempdir repos, both with non-conflicting branches; run accept-merge for each in sequence (or via poll_once with mock fixture); assert each row&#x27;s chain progresses independently — mark_cargo_installed and mark_schema_migrated rows present for both ids; failure of one chain (insert a conflict for T101) does not prevent T100 reaching mark_schema_migrated.
- **Acceptance Criteria:**
  - [ ] AC4.1: cargo test --test flow_chain_isolation passes.
  - [ ] AC4.2: AgentsYaml::from_yaml on the new fixture parses; deployment_specialist resolves; no validate() error.
  - [ ] AC4.3: The example file is referenced by README&#x27;s agents.yaml docs (Phase 5).
- **Files:** `docs/agents-yaml-example.yaml`, `tests/fixtures/agents-yaml/post-accept-chain.yaml`, `tests/flow_chain_isolation.rs`, `src/flow/agents_yaml.rs`
- **Dependencies:** Phases 1–3 complete
#### Phase 5: Phase 5: README docs swap
- **Objective:** Replace the manual &#x60;stores migrate&#x60; runbook (README.md §Schema migrations) with a note that schema-migrate runs automatically as part of the post-accept ceremony, while keeping the manual verb documented for ad-hoc/debug use.
- **Tasks:**
  - Task 5.1: Edit README.md §Schema migrations: lead paragraph now says the post-accept ceremony (accept-merge → cargo-install → schema-migrate) auto-runs after every &#x60;stores tasks accept&#x60;; demote the manual recipe to a §Manual / debug subsection.
  - Task 5.2: Add a new §Post-accept ceremony block (or extend an existing T014 daemon section if T014 added one) showing the three-link agents.yaml example.
  - Task 5.3: Cross-link to docs/agents-yaml-example.yaml from Phase 4.
- **Acceptance Criteria:**
  - [ ] AC5.1: &#x60;grep -n &#x27;after every cargo install&#x27; README.md&#x60; returns 0 matches (the old manual-only language is gone).
  - [ ] AC5.2: README includes the literal string &#x27;post-accept ceremony&#x27; and a fenced agents.yaml block listing all three builtins.
  - [ ] AC5.3: cargo test --doc passes (no broken doctests).
- **Files:** `README.md`
- **Dependencies:** Phase 4 example file landed

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** Submitted — awaiting review
- **Summary:** BLOCKED: Brief contains no executable phase content. Header reports &#x27;Current Phase: 1 of 0&#x27; (plan_phases_count&#x3D;0) and the &#x27;Current Phase to Execute&#x27; section is empty — no objective, no tasks, no acceptance criteria, no files list. The Done When contract (6 items) is present but that is the task-level contract, not a phase spec. Cannot execute without a phase brief; planner output appears to be missing or drive dispatched executor before plan phases were populated. Need planner to (re)emit phase 1 with concrete tasks/ACs/files before executor can proceed.
- **Commit:** `none`
- **At:** 2026-05-03T12:22:27Z

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

