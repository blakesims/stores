# T014: Autonomous flow engine - agent registry + daemon + policy + accept-merge

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T10:37:36Z
- **Last Updated:** 2026-05-03T10:41:30Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T014-autonomous-flow-engine

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** .stores/agents.yaml schema + parser; .stores/policies.yaml schema + predicate evaluator; daemon (&#x60;stores agents run&#x60;) with claim + retry + idempotency via SQLite advisory locks; polling-based dispatch loop; policy_ref audit field on row history; ntfy hook integration with .stores/config.yaml + env-var fallback; builtin:accept-merge subscriber; builtin:user-escalation subscriber; tasks lifecycle extension (deploy_blocked state) including state-machine guards and transition definitions; &#x60;stores agents backfill&#x60; one-off verb; deployment-specialist agent routing infrastructure (configurable in agents.yaml; default user-escalation); unit + integration tests covering all (a)-(m) items in done-when (10); operator docs (example agents.yaml, example policies.yaml, brief runbook).
- **Out:** T013&#x27;s deliverables (L024 tasks.tier_hint, L029 drafted-contract-at-filing) — prerequisite, ships separately as T013 (in_review awaiting accept). L035 schema-enforced context flow — separate T3 task (follow-up after this). L030 tier-as-planner-input briefs — separate task. DockerRunner — deferred per L031 (revisit on incident). L010 cargo install on accept — fits the daemon-subscriber pattern; defer to a follow-up task that adds it as a builtin. L011 binary version on rows — separate audit improvement. L013/L014/L015 auth UX cluster. L020/L021/L023 lifecycle papercuts. L012 inspector. L028 agent /observe access. Smart deployment-specialist agent (real merge-conflict resolver) — separate follow-up task (this task ships only the routing infrastructure + permissive user-escalation default). Subscription-based dispatch (post-MVP — polling first). Multi-host daemon coordination (single-host for now). Web UI / TUI for policies / agents (yaml-edit suffices).

### Done When
(1) Agent registry: .stores/agents.yaml schema with declarative subscriptions. Each agent entry: name (string, unique), subscribes_to (list of {store, transition} triples), command (shell command OR builtin keyword like &quot;builtin:accept-merge&quot; / &quot;builtin:user-escalation&quot;), claim_window (duration; default 300s matching existing 5-min lock), retry_policy ({max_attempts: int, backoff: linear|exponential}).

(2) Daemon: &#x60;stores agents run&#x60; long-lived process. POLLING (not subscriptions) at 5s interval, configurable via --poll-interval. SQLite advisory locks for dispatch idempotency (multiple daemon processes safe). Foreground by default; --detach for daemonize. Logs to stdout; --log-file optional. Graceful shutdown on SIGTERM (finishes in-flight dispatch).

(3) Policy layer: .stores/policies.yaml schema + evaluator. DEFAULT ACTION: ALLOW (matches &quot;everything flows between the gates&quot; doctrine). Substrate&#x27;s existing guards (required_when, actor, lifecycle) remain the floor; policies do not override them. NEVER policies are sacrosanct (explicit halt; cannot be overridden by other policies). Predicate language: simple match operators (&#x3D;&#x3D;, !&#x3D;, in, not in, matches regex). Operands: row field paths (e.g. tasks.tier_hint), literals, derived helpers like {linked_observation_count: int}. Each policy entry: {id: string, transition: {store, edge}, predicate: &lt;expr&gt;, action: allow|halt}.

(4) policy_ref audit: every automatic state transition records policy_ref: &quot;&lt;policy-id&gt;&quot; on the row&#x27;s audit trail (new field on row history). Manual transitions record policy_ref: null. Full policies.yaml hash also recorded so historical decisions can be re-verified.

(5) ntfy on policy-halt: when the daemon attempts a transition and policy halts (or no policy matches and the substrate&#x27;s actor enforcement halts), an ntfy notification fires. ntfy URL configured in .stores/config.yaml (key: ntfy.url); falls back to env var STORES_NTFY_URL; if neither set, log to stderr and continue (no error). Notification body: row id, transition attempted, policy id that halted (or &quot;actor-enforced halt&quot;), 1-line summary.

(6) Accept-merges-branch (L018&#x27;s first builtin subscriber): &quot;builtin:accept-merge&quot; registered in agents.yaml subscribes to tasks: in_review→accepted. Reads &#x60;branch&#x60; from row; reads &#x60;workspace_path&#x60; to infer project main repo. On fire: cd to project root; &#x60;git fetch&#x60;; &#x60;git merge --no-ff &lt;branch&gt;&#x60; into main. On clean merge: report success. On merge conflict: row → deploy_blocked state, ntfy fires, AND daemon dispatches the row to the configured &quot;deployment-specialist&quot; agent in agents.yaml (default: builtin:user-escalation). On missing branch (edge case): log warning, leave row in accepted (already merged or work happened in-place).

(7) State extension: tasks lifecycle gains &#x60;deploy_blocked&#x60; state. Reachable from accepted (via accept-merge conflict). Resolvable via &#x60;tasks resume&#x60; after specialist intervention. Dispatched-to via the deployment-specialist agent registered in agents.yaml.

(8) Built-in user-escalation agent: dispatched to deploy_blocked rows by default. Files a substrate observation pointing at the blocked row with conflict context (file list, branch name, last-attempted merge). Fires ntfy. Exits. Stand-in until a real deployment specialist agent is built (follow-up task).

(9) &#x60;stores agents backfill&#x60; one-off verb (NOT auto on daemon startup). Scans for accepted-but-unmerged rows and applies the accept-merge logic in sequence. Surfaces conflicts via ntfy. Logs results. Exits when scan complete.

(10) Tests cover: (a) agents.yaml schema validation (well-formed parses; malformed fails with field path); (b) daemon dispatch (poll loop picks up matching row, claims, runs command, releases); (c) daemon idempotency (two concurrent daemon processes don&#x27;t double-dispatch); (d) policy match (predicate evaluates correctly across operators); (e) default-allow semantics (no matching policy → flows; substrate guards still enforce); (f) NEVER override (allow-policy + NEVER-policy → halts); (g) policy_ref recording (audit trail captures policy id + hash); (h) ntfy mock (halt event fires correct notification body); (i) accept-merge clean (row accepted → branch merged → row stays accepted with merge commit); (j) accept-merge conflict (row → deploy_blocked with conflict files in blocked_reason); (k) deploy_blocked state transition; (l) user-escalation agent dispatch (mock observation filing); (m) backfill verb (one-off, not daemon-on-startup).

(11) Operator docs: example .stores/agents.yaml + .stores/policies.yaml + brief runbook on starting/stopping daemon and how to add subscribers.

### Phases

#### Phase 1: Phase 1: Lifecycle extension + transition_history audit table
- **Objective:** Extend tasks lifecycle with deploy_blocked state and create a per-store transition_history substrate so policy_ref / hash can be recorded on every transition (manual or automatic).
- **Tasks:**
  - Task 1.1: In stores/tasks/schema.yaml, add &#x60;deploy_blocked&#x60; to lifecycle.states; add transitions {accepted → deploy_blocked, verb: mark_deploy_blocked, actor: framework} and {deploy_blocked → ready, verb: resume, actor: ai_with_human}; add on_state.deploy_blocked: [] (no auto-dispatch — daemon dispatches via subscription).
  - Task 1.2: Add a generic &#x60;transition_history&#x60; SQLite table in src/codegen/ddl.rs (or src/db.rs) created at &#x60;stores init&#x60; / store install time, with columns: id INTEGER PK, store TEXT, row_id INTEGER, display_id TEXT, from_status TEXT, to_status TEXT, verb TEXT, invoker TEXT, policy_ref TEXT NULL, policies_hash TEXT NULL, occurred_at TEXT.
  - Task 1.3: Modify src/handlers/transition.rs::execute_transition_write to insert one row into transition_history on every successful transition; accept optional (policy_ref, policies_hash) parameters threaded through run_in_tx (default None → manual transition).
  - Task 1.4: Update src/handlers/submit.rs and src/handlers/transition.rs::run_reject / run_close_as_addressed to thread None for policy_ref (manual paths).
  - Task 1.5: Update README/topology to surface the new state (visible via &#x60;stores topology&#x60;).
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds with new state and table.
  - [ ] AC1.2: &#x60;stores tasks accept T&lt;id&gt;&#x60; writes one row to transition_history with policy_ref&#x3D;NULL.
  - [ ] AC1.3: Existing tests in src/handlers/transition.rs pass (deploy_blocked addition does not break in_review→accepted/rejected).
  - [ ] AC1.4: &#x60;stores topology&#x60; lists deploy_blocked among tasks states.
  - [ ] AC1.5: New unit test in src/db.rs (or new src/handlers/history.rs): &#x60;transition_history&#x60; row count increases by exactly 1 per transition; columns populated as expected.
- **Files:** `stores/tasks/schema.yaml`, `src/codegen/ddl.rs`, `src/db.rs`, `src/handlers/transition.rs`, `src/handlers/submit.rs`, `src/cli/topology.rs`
#### Phase 2: Phase 2: agents.yaml + policies.yaml schemas, parsers, and predicate evaluator
- **Objective:** Add config-file types and parsers for the agent registry and policy layer, including a small predicate evaluator with default-ALLOW + NEVER-sacrosanct semantics. No daemon yet — pure parsing + evaluation.
- **Tasks:**
  - Task 2.1: Create src/flow/mod.rs with submodules &#x60;agents_yaml&#x60;, &#x60;policies_yaml&#x60;, &#x60;predicate&#x60;. Add to src/lib.rs.
  - Task 2.2: Define &#x60;AgentsYaml { agents: Vec&lt;AgentEntry&gt;, deployment_specialist: Option&lt;String&gt; }&#x60; with AgentEntry { name, subscribes_to: Vec&lt;{store, transition: {from, to}}&gt;, command, claim_window: Duration (default 300s), retry_policy: {max_attempts: u32, backoff: linear|exponential} }. Implement serde_yaml load + structural validator (unique names, parseable command, builtin: prefix recognized).
  - Task 2.3: Define &#x60;PoliciesYaml { hash: String, policies: Vec&lt;PolicyEntry&gt; }&#x60; with PolicyEntry { id, transition: {store, from, to}, predicate: PredicateExpr, action: Allow|Halt|Never }. Compute SHA-256 of canonical YAML bytes on load → store as hash for policy_ref audit.
  - Task 2.4: Implement predicate language in src/flow/predicate.rs: leaf operators &#x3D;&#x3D;, !&#x3D;, in, not in, matches; operands are &#x60;path.to.field&#x60; (resolved against a serde_json::Value row map), string/number literals, and helpers: linked_observation_count, branch, status. Expose &#x60;eval(expr, row) -&gt; bool&#x60;.
  - Task 2.5: Implement evaluator entry point &#x60;decide(policies, store, from, to, row) -&gt; Decision { Allow | Halt(policy_id) }&#x60; with semantics: NEVER first wins; otherwise first matching Halt; else Allow (default). Return policy_id of the rule that decided (or sentinel &#x60;default-allow&#x60;).
  - Task 2.6: Tests in src/flow/{agents_yaml,policies_yaml,predicate}.rs covering: well-formed parses; malformed (missing required field) fails with field path; predicate operator coverage; default-allow; NEVER overrides allow; SHA-256 hash stability.
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;cargo test flow::&#x60; passes (≥10 tests across submodules).
  - [ ] AC2.2: Loading a sample .stores/agents.yaml fixture parses to expected struct; loading a malformed one returns an error message containing the offending field path.
  - [ ] AC2.3: predicate::eval returns correct booleans across all 5 operators with mixed operand types.
  - [ ] AC2.4: decide() returns Halt(&lt;NEVER policy id&gt;) when a NEVER-policy and an Allow-policy both match the same transition.
  - [ ] AC2.5: PoliciesYaml::hash is identical across two loads of the same bytes; differs when any byte changes.
- **Files:** `src/lib.rs`, `src/flow/mod.rs`, `src/flow/agents_yaml.rs`, `src/flow/policies_yaml.rs`, `src/flow/predicate.rs`, `tests/fixtures/agents.yaml`, `tests/fixtures/policies.yaml`
- **Dependencies:** Phase 1 complete (transition_history table exists for Phase 5 to write into).
#### Phase 3: Phase 3: ntfy notifier + .stores/config.yaml reader
- **Objective:** Add a small notifier hook that posts to an ntfy URL discovered from .stores/config.yaml (key: ntfy.url) with env STORES_NTFY_URL fallback, and degrades to stderr when neither is set (no error).
- **Tasks:**
  - Task 3.1: Define &#x60;StoresConfig { ntfy: Option&lt;NtfyCfg { url: String }&gt; }&#x60; in src/flow/config.rs; load from .stores/config.yaml relative to repo root (or path resolved via existing manifest helper).
  - Task 3.2: Implement &#x60;notify(event: NotifyEvent) -&gt; Result&lt;()&gt;&#x60; in src/flow/ntfy.rs. NotifyEvent { row_id, transition_attempted, policy_id_or_actor_halt, summary }. Use std http via curl shell-out OR the existing &#x60;ureq&#x60;-style approach? — propose curl shell-out (no new deps; matches topology.rs&#x27;s dot shell-out pattern) with stderr fallback when curl missing.
  - Task 3.3: Add a NotifierBackend trait + a MockNotifier (records sent events in a Vec) for tests; expose a process-global notifier for the daemon to install.
  - Task 3.4: Tests: config-yaml present → URL parsed; env fallback when config missing; both missing → notify logs to stderr and returns Ok(()).
- **Acceptance Criteria:**
  - [ ] AC3.1: &#x60;cargo test flow::config&#x60; and &#x60;flow::ntfy&#x60; pass.
  - [ ] AC3.2: With STORES_NTFY_URL set and no config.yaml, notify() targets env URL (verified via MockNotifier injection point).
  - [ ] AC3.3: With neither set, notify() returns Ok and writes a single stderr line containing the row_id.
  - [ ] AC3.4: MockNotifier captures NotifyEvent fields verbatim.
- **Files:** `src/flow/config.rs`, `src/flow/ntfy.rs`, `src/flow/mod.rs`, `tests/fixtures/config.yaml`
- **Dependencies:** Phase 2 complete (uses Decision/PolicyEntry types in NotifyEvent payload).
#### Phase 4: Phase 4: Daemon (&#x60;stores agents run&#x60;) — polling, claim, dispatch, retry, lifecycle
- **Objective:** Ship the long-running daemon: poll loop scans subscribed transitions, acquires SQLite advisory lock, dispatches to shell command or builtin, applies retry policy, releases. Foreground default; --detach; SIGTERM graceful shutdown.
- **Tasks:**
  - Task 4.1: Add &#x60;agents run&#x60; subcommand in src/cli/dynamic.rs with flags --poll-interval (default 5s), --detach, --log-file. Add &#x60;agents backfill&#x60; placeholder (impl in Phase 7).
  - Task 4.2: Create src/handlers/agents_run.rs: load agents.yaml + policies.yaml at startup; on parse error, refuse to start (fail-loud).
  - Task 4.3: Implement poll loop: every interval, query each subscribed store for rows whose (status, last transition.to) matches a subscription AND that have not been dispatched within claim_window. Use a &#x60;dispatch_locks&#x60; table (row_id, agent_name, claimed_at, claimed_by) with UNIQUE(row_id, agent_name) and INSERT to claim atomically (idempotent against parallel daemons).
  - Task 4.4: Dispatch: for shell &#x60;command&#x60;, spawn via std::process::Command with env vars STORES_ROW_ID, STORES_DISPLAY_ID, STORES_TRANSITION_FROM/TO, STORES_STORE; capture stdout/stderr to log. For builtin: keyword (&#x60;builtin:accept-merge&#x60;, &#x60;builtin:user-escalation&#x60;), call into Phase 6&#x27;s Rust functions.
  - Task 4.5: Retry policy: on non-zero exit, increment attempt counter on the lock row; reschedule per backoff (linear: N*30s; exponential: 2^N*30s). After max_attempts, mark lock failed and ntfy.
  - Task 4.6: SIGTERM handler installed via libc::signal: set shutdown flag, finish any in-flight dispatch, exit 0.
  - Task 4.7: --detach: fork + setsid on Linux (libc); redirect stdio to log_file; parent prints child PID and exits.
  - Task 4.8: Tests in src/handlers/agents_run.rs: (b) poll-loop picks up matching row, claims, runs mock command, releases (use --max-iters&#x3D;1 + a noop command); (c) idempotency: two threads invoking the dispatch fn simultaneously result in exactly one claim row.
- **Acceptance Criteria:**
  - [ ] AC4.1: &#x60;cargo build&#x60; succeeds; &#x60;stores agents run --help&#x60; lists --poll-interval, --detach, --log-file.
  - [ ] AC4.2: Test (b): a tasks row freshly transitioned to in_review is dispatched once to a registered noop subscriber within one poll iteration.
  - [ ] AC4.3: Test (c): two concurrent dispatch invocations against the same row result in exactly one row in dispatch_locks (the loser&#x27;s INSERT fails on UNIQUE).
  - [ ] AC4.4: SIGTERM during in-flight noop dispatch → process exits 0 after the noop completes (test via spawn-and-signal).
  - [ ] AC4.5: Daemon refuses to start when agents.yaml is malformed; stderr names the failing field.
- **Files:** `src/cli/dynamic.rs`, `src/cli/dispatch.rs`, `src/handlers/agents_run.rs`, `src/handlers/mod.rs`, `src/codegen/ddl.rs`
- **Dependencies:** Phase 2 (config types), Phase 3 (notifier for retry-exhausted halt).
#### Phase 5: Phase 5: Policy integration + policy_ref audit recording on automatic transitions
- **Objective:** Wire the policy evaluator into the daemon&#x27;s pre-dispatch check; when a transition is policy-halted (or actor-enforced-halt), fire ntfy and skip dispatch. Record policy_ref + policies_hash on every transition the daemon causes.
- **Tasks:**
  - Task 5.1: In agents_run.rs, before each dispatch, call decide(policies, store, from, to, row). On Halt: fire notify() with policy_id (or &#x27;actor-enforced-halt&#x27; sentinel) and skip; do NOT claim or retry.
  - Task 5.2: When the dispatched subscriber later submits a state transition (via the substrate CLI it shells out to), the substrate must record policy_ref. Approach: daemon writes the deciding policy_id to an env var STORES_POLICY_REF for the dispatched process; substrate&#x27;s transition.rs reads it (when set) and records it on transition_history. Document the contract.
  - Task 5.3: For builtin subscribers (which call execute_transition_write directly), pass policy_ref through the function signature (Phase 1 already added the optional parameter).
  - Task 5.4: Distinct sentinel in transition_history.policy_ref: NULL &#x3D; manual, &#x27;default-allow&#x27; &#x3D; no rule matched, &#x27;&lt;id&gt;&#x27; &#x3D; matched rule. Always write policies_hash when daemon-driven.
  - Task 5.5: Tests: (d) policy match — predicate evaluates correctly across operators (already in Phase 2; add an integration test that exercises decide → daemon dispatch path); (e) default-allow semantics — row with no matching policy still flows; (f) NEVER override — allow + NEVER → halt; (g) policy_ref recording — transition_history captures id + hash on auto path; null on manual path; (h) ntfy mock — halt event notification body contains row_id + policy_id.
- **Acceptance Criteria:**
  - [ ] AC5.1: &#x60;cargo test agents_run::policy&#x60; passes (5 cases for d/e/f/g/h).
  - [ ] AC5.2: A row matching a Halt-policy is NOT dispatched and a single MockNotifier event is recorded with the correct policy_id.
  - [ ] AC5.3: After a daemon-driven transition, transition_history row has policy_ref &#x3D; &#x27;&lt;id&gt;&#x27; (or &#x27;default-allow&#x27;) and policies_hash &#x3D; the SHA-256 of the loaded policies.yaml.
  - [ ] AC5.4: Manual &#x60;stores tasks accept T###&#x60; (no daemon) records policy_ref &#x3D; NULL.
- **Files:** `src/handlers/agents_run.rs`, `src/handlers/transition.rs`, `src/flow/policies_yaml.rs`
- **Dependencies:** Phase 4 daemon, Phase 3 notifier.
#### Phase 6: Phase 6: Builtin subscribers — accept-merge + user-escalation; deploy_blocked path
- **Objective:** Implement the two ship-with-this-task builtins: accept-merge (in_review→accepted handler that fast-merges branch into main; conflict → deploy_blocked + ntfy + dispatch to deployment-specialist) and user-escalation (deploy_blocked default specialist that files an observation + ntfy).
- **Tasks:**
  - Task 6.1: Create src/flow/builtins/accept_merge.rs. Signature: &#x60;pub fn run(row: &amp;TaskRow, ctx: &amp;DispatchCtx) -&gt; BuiltinResult&#x60;. Read &#x60;branch&#x60; and &#x60;workspace_path&#x60;; cd to git common dir of workspace_path (project main repo); &#x60;git fetch&#x60;; &#x60;git merge --no-ff &lt;branch&gt;&#x60;. On clean: return Ok (row remains accepted). On conflict: invoke transition &#x60;mark_deploy_blocked&#x60; with blocked_reason &#x3D; list of conflict files + branch + last attempt; fire ntfy; dispatch the row to the agent named by agents.yaml &#x60;deployment_specialist&#x60; (default: builtin:user-escalation). On missing branch: log warning, leave accepted.
  - Task 6.2: Create src/flow/builtins/user_escalation.rs: read deploy_blocked task row → &#x60;stores observations add&#x60; (via in-process call) with task_id link, body containing branch + conflict files; fire ntfy; exit success.
  - Task 6.3: Register both builtins in agents_run.rs dispatcher: &#x60;builtin:accept-merge&#x60; → accept_merge::run; &#x60;builtin:user-escalation&#x60; → user_escalation::run.
  - Task 6.4: Wire the framework-actor &#x60;mark_deploy_blocked&#x60; transition such that builtins (running in-process as the daemon) can fire it. Use Actor::Framework invoker in execute_transition_write.
  - Task 6.5: Add tests using a temp git repo (tempfile + Command::new(&#x27;git&#x27;)): (i) accept-merge clean — branch with non-conflicting commit merged; row stays accepted; HEAD has merge commit. (j) accept-merge conflict — row → deploy_blocked; blocked_reason includes the conflicted file path. (k) deploy_blocked transition mechanics. (l) user-escalation files an observation (verify observations row count increment; verify task_id soft-FK).
- **Acceptance Criteria:**
  - [ ] AC6.1: &#x60;cargo test flow::builtins&#x60; passes (≥4 integration tests).
  - [ ] AC6.2: Test (i): post-merge &#x60;git log --oneline main&#x60; shows a merge commit; tasks row status&#x3D;&#x27;accepted&#x27;.
  - [ ] AC6.3: Test (j): tasks row status&#x3D;&#x27;deploy_blocked&#x27; and blocked_reason text contains the conflict filename.
  - [ ] AC6.4: Test (l): observations table gains exactly one new row whose body cites the blocked task display_id.
  - [ ] AC6.5: Both ntfy events fire (MockNotifier captures the deploy_blocked event).
- **Files:** `src/flow/builtins/mod.rs`, `src/flow/builtins/accept_merge.rs`, `src/flow/builtins/user_escalation.rs`, `src/handlers/agents_run.rs`, `stores/tasks/schema.yaml`
- **Dependencies:** Phase 1 (deploy_blocked state), Phase 4 (dispatcher), Phase 5 (policy_ref threading).
#### Phase 7: Phase 7: backfill verb + operator docs + e2e tests
- **Objective:** Ship the one-off &#x60;stores agents backfill&#x60; scan, plus docs/operator/agents-and-policies.md with example yaml + runbook. Add the remaining e2e test (a daemon spinning up, a row transitioning, accept-merge running end-to-end against a temp git repo).
- **Tasks:**
  - Task 7.1: Implement src/handlers/agents_backfill.rs: load agents.yaml; SELECT all tasks WHERE status&#x3D;&#x27;accepted&#x27; AND no merge commit yet (heuristic: branch not in &#x60;git branch --merged main&#x60;); for each, run accept-merge sequentially; print per-row result; exit when scan complete.
  - Task 7.2: Wire &#x60;stores agents backfill&#x60; in src/cli/dynamic.rs and dispatch from src/main.rs.
  - Task 7.3: Create docs/agents-and-policies.md with: example .stores/agents.yaml (with accept-merge + a sample shell subscriber); example .stores/policies.yaml (one allow + one NEVER); runbook (&#x60;stores agents run&#x60; foreground vs --detach; how to add a subscriber; how to inspect transition_history).
  - Task 7.4: Update README.md with a brief Autonomous Flow section linking to docs/agents-and-policies.md.
  - Task 7.5: Write tests/agents_e2e.sh shell e2e: init substrate → install fixture agents.yaml + policies.yaml → start daemon in background with --max-iters&#x3D;3 (add a hidden flag mirroring drive --max-iters) → add a tasks row → accept it via human-token → assert daemon dispatched accept-merge against a temp git repo → assert HEAD has merge commit. Test (m): backfill verb scans an accepted-but-unmerged row and merges it.
  - Task 7.6: Update tests/e2e.sh aggregator if needed.
- **Acceptance Criteria:**
  - [ ] AC7.1: &#x60;stores agents backfill --help&#x60; works; running it on an empty substrate prints &#x27;0 rows scanned&#x27; and exits 0.
  - [ ] AC7.2: Test (m) in agents_e2e.sh: a pre-seeded accepted-but-unmerged row is merged after backfill; transition_history NOT updated (backfill does not transition state — it just merges).
  - [ ] AC7.3: tests/agents_e2e.sh exits 0 on a clean checkout.
  - [ ] AC7.4: docs/agents-and-policies.md exists and contains both sample yaml blocks plus the runbook.
  - [ ] AC7.5: README.md links to the new doc.
- **Files:** `src/handlers/agents_backfill.rs`, `src/handlers/mod.rs`, `src/cli/dynamic.rs`, `src/cli/dispatch.rs`, `src/main.rs`, `docs/agents-and-policies.md`, `README.md`, `tests/agents_e2e.sh`, `tests/fixtures/agents.yaml`, `tests/fixtures/policies.yaml`
- **Dependencies:** Phase 6 (accept-merge logic that backfill reuses), Phase 4 daemon (--max-iters extension).

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

