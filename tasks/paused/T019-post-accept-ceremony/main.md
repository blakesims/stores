# T019: Post-accept ceremony - cargo install + schema migrate as L018 subscribers

## Meta
- **Status:** blocked
- **Created:** 2026-05-03T12:11:02Z
- **Last Updated:** 2026-05-03T12:21:16Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** plan-reviewer marked NOT_READY: The submitted plan is empty: no objective text, zero phases, and no decision matrix entries. There is nothing to review against the contract — the planner must produce an actual plan before review can proceed.
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

_Plan not yet submitted._

---

## Plan Review

### Review 1
- **Gate:** NOT_READY
- **Summary:** The submitted plan is empty: no objective text, zero phases, and no decision matrix entries. There is nothing to review against the contract — the planner must produce an actual plan before review can proceed.
- **Open Questions:**
  - Planner must supply an Objective statement.
  - Planner must supply phases with tasks, files, and mechanical acceptance criteria covering done_when items (1)-(6), including the two new handlers, agents.yaml registration, default chain example, the six test scenarios (a)-(f), and the README update.
  - Planner must populate the decision matrix for non-trivial choices (e.g., how features are configured via command_args, how the post-cargo-install transition name is wired, how concurrent-chain isolation is tested).
- **At:** 2026-05-03T12:21:16Z

---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

