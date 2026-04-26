# T002: Tasks store on β architecture (DB-as-truth + workflow engine)

## Meta
- **Status:** EXECUTING_PHASE_3
- **Created:** 2026-04-26
- **Last Updated:** 2026-04-26 (code-reviewer: Phase 2 PASS — advance to Phase 3)
- **Blocked Reason:** —

## Task

Port the multi-agent task workflow onto the stores framework with **DB-as-truth, framework-as-engine**. The framework gains generic workflow CLI verbs driven by per-store lifecycle metadata; agents become thin workers receiving scoped briefings; main.md is rendered from DB rows on demand.

This is the marquee v0.2 task. After it lands, 10.06's `task:open`/`task:wrap`/`task:next` skills can move onto the framework, and every future workflow-shaped store (research pipelines, content review, approval flows) gets the same engine for free.

### Intent Contract (ratified by user 2026-04-26)

#### Executive intent

The original v0.2 handoff (`docs/handoff-v0.2.md`) sketched a tasks store with `list<record>` fields for phases / execution_log / code_review_log, three options for sub-document CLI surface, and section-ownership concerns. After deeper analysis, both the orchestrator and a second-opinion agent independently converged on a stronger design: **DB-as-truth, framework-as-engine** (we'll call this "β").

In β:
- The framework owns workflow state: current_phase, current_cycle, agent routing, transition predicates.
- Agents are thin workers. They receive a scoped briefing (only the slice they need), do their unit of work, submit structured output back through CLI verbs.
- main.md is rendered from DB rows on demand via `stores tasks render T<NNN>`. Agents NEVER edit main.md. There is exactly one write path: the CLI.
- Schema-level guards enforce the 3-cycle REVISE limit; the 4th attempt routes to BLOCKED automatically.

#### DONE_WHEN

> A real T3 task is created, planned, executed, and completed end-to-end via `stores tasks` CLI only. The orchestrator skill calls `next-action`, `brief`, `submit-execute`, `submit-review`, `render` — nothing else. Framework owns: state, current-phase, cycle counting, agent routing, render output. 4th REVISE attempt on any phase is rejected by schema-level guard with status auto-set to BLOCKED. main.md is regeneratable any time from DB rows and matches the canonical layout.

#### Scope

**In:**
- New schema features:
  - `actor: framework` (engine-fired transitions, no agent invocation)
  - `guard:` predicate on transitions (extends `required_when`'s expression language with equality + length comparison; defer full AND/OR/inequalities)
  - `auto_increment` and `auto_increment_within` field attributes
  - Optional `workflow:` block in schema (declares per-state agent role mapping + briefing template paths)
  - `scope: repo | worktree | user` storage resolution; `scope: repo` resolves `.stores/` to the canonical `.git/` location via `git rev-parse --git-common-dir`
- New CLI verbs (generic across opt-in workflow-shaped stores; tasks is the first):
  - `stores <store> next-action <id> [--json]` — returns which agent should act next, current phase/cycle, blocked status
  - `stores <store> brief <id> [--for <agent>] [--json]` — markdown for LLM by default, `--json` for orchestrator
  - `stores <store> submit-execute <id> --summary-from-file <f> --commit <sha> --files-changed <csv> [--notes-from-file <f>]`
  - `stores <store> submit-review <id> --gate PASS|REVISE|FAIL --critical N --major N --minor N --summary <text> --details-from-file <f>`
  - `stores <store> submit-plan <id> --plan-from-file <f>`
  - `stores <store> submit-plan-review <id> --gate READY|NEEDS_WORK|NOT_READY --summary <text> --open-questions-from-file <f>`
  - `stores <store> render <id>` — writes main.md from DB rows; idempotent; regeneratable any time
- `tasks` store schema:
  - Fields: `title, slug, branch, capability, sub_item, infra, depends_on (list_fk → tasks), linked_observations (list_fk → observations), contract (record), plan (record with phases list_record), plan_review_log (list_record), cycles (list_record per phase × cycle), current_phase (integer, actor: framework), current_cycle (integer, actor: framework), claimed_by (text), claimed_at (timestamp)`
  - Lifecycle: `[planning, plan_review, ready, executing, code_review, blocked, complete]` (7 states; no phase_review, no merge_review)
  - Transitions with guards enforcing 3-cycle limit
  - Capability/sub_item/infra fields are mutually-exclusive optional (project-specific YAML check stays in wrapper skills, not framework)
- Briefing templates bundled with the tasks store package: `stores/tasks/templates/{planner,plan-reviewer,executor,code-reviewer}-brief.md.tpl`
- Render template: `stores/tasks/templates/main.md.tpl` for the canonical main.md layout
- Bundled `tasks:start` skill (the orchestrator that drives the loop via the new CLI; replaces /task:start when working against a stores-installed tasks store)
- `claimed_by`/`claimed_at` lock pattern with 5-minute default timeout (releases on submit or expiry); prevents two agents in different worktrees double-advancing
- Adjacent fix: add `priority` field to gate schema (decision/script gate items need priority for /task:open's "Blake-only-step" filing pattern)
- Smoke test: take one real task end-to-end through the new workflow. Target: expand `observations` lifecycle to 10.06's full set (`investigating`, `confirmed`, `needs_info`, `in_progress`).

**Out (deferred):**
- 10.06's Stage 0 size check / Stage 1.5 capability YAML reconciliation (project-specific; lives in `/task:open` wrapper, not bundled `tasks:start`)
- Stage 6 CodeRabbit integration → separate `tasks:wrap` skill (not in this task)
- Phase-reviewer agent / state / transitions
- Merge-reviewer agent / MERGE_REVIEW / MERGE_READY states
- Importing existing main.md docs into DB (start fresh; legacy filesystem T001/T002 stay as-is)
- HTTP/JSON API for tasks store
- `runs` event log store
- `notes` store
- Multi-worktree concurrent-write contention beyond the simple `claimed_by` lock

**Unchanged:**
- v0.1 locked decisions: per-field actor model preserved (just adding `framework` as a new actor value); identity scheme; single SQLite per scope; Rust; YAML schema declaration
- All 110 existing tests
- Existing observations + gate stores (gate gets the `priority` field as the only adjacent change)

#### Locked decisions (ratified by user 2026-04-26)

| # | Decision | Locked value |
|---|----------|--------------|
| 1 | Architecture: DB-as-truth + workflow engine in framework | β confirmed |
| 2 | Workflow opt-in mechanism | Explicit `workflow:` block in schema (not implicit) |
| 3 | Brief/submit CLI shape | Split verbs: `brief` returns prompt, `submit-execute`/`submit-review`/`submit-plan`/`submit-plan-review` are explicit verbs (no overloaded args) |
| 4 | Smoke-test target task | Expand `observations` lifecycle to 10.06's full set: add `investigating`, `confirmed`, `needs_info`, `in_progress` states with appropriate transitions |
| 5 | ID prefix for stores-tasks rows | `T{:03d}` shared with filesystem; T001-T002 stay as legacy filesystem-only; T003 onwards are DB rows rendered to filesystem |
| 6 | `guard:` expression language scope | Equality (`==`, `!=`) + `.length <`, `.length <=`, `.length >`, `.length >=`, `.length ==` only; defer full AND/OR/inequalities for non-length comparisons |
| 7 | Capability fields on bundled tasks schema | Bake in as optional (`capability: text`, `sub_item: text`, `infra: text`); the 10.06 YAML check stays in the project-specific wrapper skill, not the framework |
| 8 | Concurrency lock | `claimed_by`/`claimed_at` with 5-min default timeout (releases on submit or expiry) |

#### Risks / assumptions

- **Briefing templates are the LLM-quality bottleneck.** Bad templates produce bad agent output regardless of schema enforcement. Smoke test surfaces gaps; iterate within plan-review cycles.
- **`guard:` evaluator extends `required_when`** beyond `lhs.dotted == 'literal'`. Constrained scope (D6) keeps this small (~150 LOC max).
- **Two-write boundary** (DB submit → main.md render). Idempotent re-render available any time mitigates risk.
- **`stores` binary on PATH inside spawned subagents** — verified via `Bash` allowed tool, but worth confirming in smoke test.
- **Effort estimate**: 3-4 weeks elapsed, ~4000-6000 LOC framework + schema + skills. Significantly larger than the original handoff sketched (~2 weeks, 1500-2500 LOC) — this is the cost of β being a workflow engine, not just a store.

#### Workflow rules for THIS task's execution

This task is being executed via the existing `/task:start` skill (with its Stage 6 CodeRabbit). The bundled `tasks:start` we are *building* will not include CodeRabbit — that goes into a separate `tasks:wrap` skill (out of scope for this task). Different things; no contradiction.

### Read-first context for downstream agents

In approximate order of importance:

1. **`docs/handoff-v0.2.md`** — original v0.2 handoff. Captures the v0.1 design history, locked decisions, deferred bugs, and the original (now-superseded) tasks-store schema sketch.
2. **`tasks/completed/T001-stores-framework-v01/main.md`** — full v0.1 audit trail: 8 phases, plan reviews, code reviews, lessons learned. Decision Matrix at the bottom is the most concentrated source of "why this design, not that one."
3. **`README.md`** — user-facing 13-step demo path; the v0.1 contract.
4. **`tests/e2e.sh`** — byte-identical script that exercises the README; e2e source-of-truth.
5. **`src/schema/mod.rs`, `src/schema/required_when.rs`, `src/schema/lifecycle.rs`** — current schema model; β extensions plug into here.
6. **`src/schema/flatten.rs`** — leaf flattening for CLI args; β changes how this works for workflow-shaped stores (briefing templates take over).
7. **`src/handlers/{add,update,transition}.rs`** — current Op shapes; β adds new Op shapes for `submit-*` verbs.
8. **`src/validate/mod.rs`, `src/validate/actor.rs`** — current validation; β adds `guard:` evaluation and `actor: framework`.
9. **`stores/observations/schema.yaml`, `stores/gate/schema.yaml`** — current bundled stores; observations gets lifecycle expansion as smoke test target.
10. **`~/repos/plugins/task-workflow-plugin/agents/{planner,plan-reviewer,executor,code-reviewer}.md`** — agent personas. Planner should mine these to understand what each agent expects in their briefing.
11. **`~/repos/plugins/task-workflow-plugin/skills/{plan,review-plan,execute,review-code}/SKILL.md`** — skill prose; same purpose as #10.
12. **`~/repos/plugins/task-workflow-plugin/templates/main.md`** — canonical main.md layout the render template must match.
13. **`~/repos/plugins/task-workflow-plugin/schemas/*.json`** — structured output shapes per agent. The submit verbs' parameters should map cleanly onto these.
14. **`~/repos/plugins/task-workflow-plugin/pi-extension/section-serializers.ts`** — pi-extension's render approach (TS); useful pattern for the Rust render.
15. **`tasks/CLAUDE.md`** — task-workflow conventions for THIS task's execution (orchestrator-level, not the framework being built).

---

## Plan

### Objective

Extend the v0.1 framework with a **generic workflow engine** (declared per-store via an opt-in `workflow:` block) and ship the first workflow-shaped store, `tasks`, on top of it. After this lands, any store can declare per-state agent routing + briefing templates + framework-fired transitions, and downstream skills drive the whole multi-agent task lifecycle through five generic CLI verbs (`next-action`, `brief`, `submit-execute`, `submit-review`, `submit-plan`/`submit-plan-review`, `render`). main.md becomes a deterministic projection of DB state.

### Scope

- **In:** Schema feature additions (`actor: framework`, `guard:`, `auto_increment*`, `scope:`, `workflow:` block); five generic workflow CLI verbs in the framework; one render verb that materialises main.md from DB rows; the `tasks` store schema + its briefing/render templates; bundled `tasks:start` orchestrator skill; smoke-test (`observations` lifecycle expansion + a real T3 task driven end-to-end via the new CLI); adjacent fix `priority` field on the `gate` schema.
- **Out:** Every item in the Out section of the Intent Contract — Stage 0/1.5 wrapper logic, Stage 6 CodeRabbit, phase-reviewer/merge-reviewer agents and states, importing legacy main.md docs, HTTP API, `runs` event log, multi-worktree contention beyond the simple `claimed_by` lock.

### Effort & shape

10 phases (~4000–6000 LOC framework + schema + skills + templates + tests). Phases 1–3 are pure schema/library extensions (small, well-tested, no agent-visible surface). Phases 4–6 build the generic workflow CLI engine layer-by-layer, each landing a CLI surface that is exerciseable in isolation. Phases 7–8 wire up the `tasks` store schema and its bundled orchestrator skill on top of the engine. Phases 9–10 are the smoke test (real task end-to-end + observations-lifecycle expansion) and documentation/e2e integration.

The order matters: every later phase's acceptance criteria can be tested against the binary built in the previous phase, so plan-review and code-review at each gate has a working artefact to lean on.

---

### Phases

#### Phase 1: Schema feature foundation — `actor: framework`, `guard:`, `auto_increment*`, `scope:`, `list_record`, `list_fk`, `requires_gate` (revised cycle 2: added list_record/list_fk per C1; pulled requires_gate forward per M4; m1: `expr.rs` added beside `required_when.rs` rather than renaming)

- **Objective:** Land six small, orthogonal schema-language features that the workflow engine will compose. No CLI surface, no engine yet — pure parser + validator unit tests. **Cycle 2 addition:** `list_record` and `list_fk` field types (the deeper-nested variants Phase 7's `tasks` schema requires); `requires_gate` on `Transition` (used by Phase 5's gate-keyed transition lookup); these were originally placed in Phase 7 and are now correctly foundational. The phase grows to ~700-900 LOC; if cycle-3 review judges it too large, split into 1a (actor+guard+scope+auto_increment) and 1b (list_record+list_fk+requires_gate).
- **Tasks:**
  - 1.1: Extend `Actor` enum at `src/schema/actor.rs` with `Framework` variant. Update `Display`, `Deserialize`, `Actor::from_env()` (env never resolves to `Framework` — that's only set programmatically by the engine). Update `validate/actor.rs::actor_allowed` so `framework` required is satisfied **only** by `invoker == Framework` (no override). Add a `--invoker framework` rejection path in `cli/dispatch.rs::detect_invoker` (the `framework` actor is internal; users cannot pass it).
  - 1.2: Add `auto_increment: bool` and `auto_increment_within: Option<String>` attributes to `RawField` / `Field` in `src/schema/mod.rs`. Validation rule (in `Schema::from_yaml`): a field with `auto_increment` must have `ty: Integer` AND `actor: framework`; `auto_increment_within` must name an existing top-level integer field with `actor: framework` on the same schema (cycle check; also reject `auto_increment_within: <self>`). The semantics are: when the engine bumps an `auto_increment_within` field (e.g. `current_cycle`), it is reset to `1` whenever its parent counter (`current_phase`) increments — but the **engine** owns this; the schema only declares the dependency. The handler-level reset/bump logic lands in Phase 5. (See Phase 5.4's reset/bump table for the exact lifecycle of `current_cycle`.)
  - 1.3: Add `src/schema/expr.rs` *alongside* (NOT renaming) `src/schema/required_when.rs` per m1 (smaller diff, no churn on existing call sites). The new module defines a more general `Expr` AST that supports the locked subset (D6): `==`, `!=`, and the length operators `.length <`, `.length <=`, `.length >`, `.length >=`, `.length ==`. Add `pub fn parse_guard(input: &str) -> Result<Expr>` here; `required_when.rs` keeps its narrower equality-only parser for backwards compatibility (guards get the wider grammar; `required_when` does NOT). New AST shape: `enum Op { Eq, Neq, Lt, Le, Gt, Ge }` + `enum Rhs { Literal(String), Integer(i64) }` + an `enum Lhs { Path(Vec<String>), PathLength(Vec<String>) }`. The single-quote literal rule, AND/OR rejection, and double-quote rejection from the v0.1 parser carry over verbatim. Re-export `Expr` from `expr.rs` and have `required_when.rs` `pub use` it where the AST overlaps so there's a single AST type.
  - 1.4: Add `src/validate/expr_eval.rs` exporting `pub fn eval(expr: &Expr, entry: &EntryMap) -> bool`. Length operators read the value at the path and, if it's an Array or String, compare its length; otherwise the predicate is `false` (so missing/null paths never crash; an `auto_increment_within` reset on `current_phase < 7` will simply not fire for malformed entries). Comprehensive unit tests cover each operator + null/missing path. The path lookup must handle `current_phase` and `current_cycle` (top-level integer scalars) as well as nested paths into list_record / record (used later by Phase 7's plan-record guards).
  - 1.5: Add `scope: Option<StoreScope>` to `Schema` (parsed from a top-level YAML key). `enum StoreScope { Repo, Worktree, User }`. Default when absent: `Worktree` (preserves v0.1 cwd-only semantics — current default is "wherever cwd happens to be," which equals the worktree root for git-aware users).
  - 1.6: Implement `src/paths.rs::stores_dir_for(scope: StoreScope) -> Result<PathBuf>` that resolves: `Worktree → cwd/.stores`, `Repo → <git rev-parse --git-common-dir parent>/.stores`, `User → $HOME/.stores`. Add `git_common_dir() -> Result<PathBuf>` helper that shells out to `git rev-parse --git-common-dir` (errors clearly outside a git repo). Existing `stores_dir()` stays as the bare default (Worktree) so the framework binary still works in non-git tmp dirs as `tests/e2e.sh` requires. **Note:** the active *scope* per command is set by `manifest.yaml`, not the per-store schema — the manifest records which scope a store was installed into and `paths.rs` resolves accordingly. This keeps init/install single-pass.
  - **1.7 (NEW per C1): `FieldType::ListRecord(Vec<Field>)`.** Add a new `FieldType` variant in `src/schema/mod.rs:21-30` parallel to the existing `List(Box<FieldType>)`. Parses from YAML shape `{name: foo, type: list_record, fields: [...]}`. Storage: serialised as JSON in a `TEXT` column (same as the existing `Record(_)`). DDL codegen emits `TEXT` for the column (no schema-level type-checking on the JSON; the Rust validator owns shape correctness). Extend `RawFieldType` deserialiser to recognise `list_record`. The validator must recursively walk into nested fields: a list_record element can itself contain `record`, `list_record`, `list_text`, or scalar fields (the tasks schema's `cycles[].executor` is record-inside-list-record, depth 3; the tasks schema's `plan.phases` is list_record-inside-record, also depth 3).
  - **1.8 (NEW per C1): `FieldType::ListFk { ref_store: String }`.** Add a new `FieldType` variant for soft foreign keys to other stores. Parses from YAML shape `{name: depends_on, type: list_fk, ref: tasks}`. Storage: TEXT JSON column holding an array of display_ids (e.g. `["T001", "T002"]`). NO insert-time enforcement (the v0.1 `task_ref` precedent is "soft" — refs not validated against the target store at write). The reference is resolved lazily at render time (Phase 6) by joining the JSON array against the `ref_store`'s table to fetch each row's display_id (and optionally a display label). If the target row doesn't exist, render emits the bare ID without crashing.
  - **1.9 (NEW per C1): Extend `read_row` / `build_entry_map` for depth-3 nests.** The current `src/handlers/row.rs:188`-area logic walks Record at depth ≤ 2 only via `path.len() <= 2` checks; this is insufficient for `cycles[].executor.summary` (depth 3) and `plan.phases[].name` (depth 3). Extend the recursive walker so a `ListRecord(fields)` element is materialised by recursively building entry maps for each list element (using the same code path that handles `Record(fields)`); the result is a `Vec<EntryMap>` stored under the list-record's key in the parent entry. `ListFk` is materialised as a `Vec<String>` (the JSON array of ids) without dereferencing — dereferencing is render-only. Audit every `path.len() <= 2` check; either lift the limit or branch on type.
  - **1.10 (NEW per M4): `requires_gate: Option<String>` on `Transition`.** Extend `Transition` in `src/schema/lifecycle.rs` with the optional gate-key field. Parses from YAML key `requires_gate: PASS` (or any string). Engine selection logic for Phase 5 lives there, not here — Phase 1 just lands the parsed shape and the round-trip test. Validation rule: when multiple transitions share `(from, verb)`, at least N-1 of them must declare `requires_gate` (only one transition per (from, verb) is allowed to be unguarded by gate); a schema where two `(from, verb, requires_gate=None)` transitions exist fails to load with a clear "ambiguous transition selection" error.
  - **1.11 (NEW per M6): Framework-actor DDL test.** Add a unit test asserting that a schema with an `actor: framework` field of type `text` / `integer` / `timestamp` generates the same DDL column as any other field of that type (storage is type-only; actor scoping is enforced by the validator, not the database). This is the "framework-actor field DDL/storage" AC the original plan-review (M6) flagged as missing.
- **Acceptance Criteria:**
  - AC1.1: `actor: framework` parses; `--invoker framework` is rejected with a clear "internal actor; cannot be passed by user" error; an `Op::Add` with a value at a `framework`-actor field by a human invoker fails with the standard actor-mismatch error.
  - AC1.2: A schema declaring `auto_increment` on a non-integer field fails to load. A schema with `auto_increment_within: nonexistent_field` fails to load with a clear "auto_increment_within target not found" error. A schema with `auto_increment_within: <self>` (cycle) also fails.
  - AC1.3: `parse_guard("phases.length < 4")` returns the expected AST. `parse_guard("current_cycle <= 4")` returns the expected AST (top-level integer comparison). `parse_guard("a == 'x' OR b == 'y'")` returns an error citing OR (same word-boundary heuristic the v0.1 parser already has).
  - AC1.4: `eval` on the same fixture entry as Phase 1.3 returns `true` for matching guards, `false` for non-matching, `false` for missing-path operands. A test fires every operator at least once. Specifically: `eval(parse_guard("current_cycle <= 4"), entry_with_current_cycle_4)` is `true`; `eval(..., entry_with_current_cycle_5)` is `false`. `eval(parse_guard("current_phase < plan.phases.length"), entry_with_current_phase_1_and_2_phases)` is `true` (depth-3 path lookup). (This AC is the foundation C2's and M9's guard logic relies on.)
  - AC1.5: `scope: repo` parses; missing scope key parses as `Worktree`; unknown scope value errors.
  - AC1.6: `stores_dir_for(Repo)` in a git worktree resolves to the canonical `.git/`'s parent + `.stores` (test by creating a tmp git repo + worktree). Outside a git repo, `Repo` errors clearly.
  - **AC1.7 (NEW per C1):** A schema with `cycles: list_record` whose element fields include another `list_text` and a nested `record` parses successfully. DDL codegen emits a single `TEXT` column. Round-trip test: `add` an entry with a populated `cycles` list (3 elements, each with a populated `executor` sub-record), `show` reads it back identical (after JSON round-trip), `update` replaces a list element correctly.
  - **AC1.8 (NEW per C1):** A schema with `depends_on: list_fk, ref: tasks` parses successfully. DDL emits `TEXT`. Round-trip: store `["T001", "T002"]` and read it back as a `Vec<String>`. No referential check at write time.
  - **AC1.9 (NEW per C1):** `read_row` round-trips a 3-level nested entry: `plan.phases[2].name` (record → list_record → record-element → string) reads back identical to what was written. `cycles[1].executor.summary` reads back identical (list_record → record → string).
  - **AC1.10 (NEW per M4):** A `Transition` with `requires_gate: PASS` parses. A schema with two `(from: code_review, verb: submit-review, requires_gate: None)` transitions fails to load with the "ambiguous transition selection" error.
  - **AC1.11 (NEW per M6):** A schema with `claimed_by: text, actor: framework` produces a `claimed_by TEXT` column in DDL identical to a non-framework `text` field's column. (Confirms framework actor is a validator concern, not a storage concern.)
  - AC1.12: All 110 existing tests still pass.
- **Files:**
  - `src/schema/actor.rs` — extend enum + serde
  - `src/schema/mod.rs` — `auto_increment*` field attrs, `scope` on Schema, **`FieldType::ListRecord` and `FieldType::ListFk` variants**
  - `src/schema/expr.rs` — new (alongside `required_when.rs`, not replacing it)
  - `src/schema/required_when.rs` — re-export `Expr` from `expr.rs` so the AST is shared
  - `src/schema/lifecycle.rs` — **`requires_gate: Option<String>` on Transition**, ambiguity validation
  - `src/validate/expr_eval.rs` — new
  - `src/validate/actor.rs` — `framework` actor handling
  - `src/handlers/row.rs` — **lift `path.len() <= 2` depth limits to handle list_record + record nests at depth ≥ 3**
  - `src/cli/dispatch.rs` — reject `--invoker framework`
  - `src/paths.rs` — `StoreScope`, `stores_dir_for`, `git_common_dir`
  - `src/manifest.rs` — record `scope` per `InstalledStore`
- **Dependencies:** None. Pure schema/validator/storage work.

#### Phase 2: `workflow:` block in schema — opt-in declaration (revised cycle 2: `submit_targets` and `render_target_path` pulled into 2.1 per M3)

- **Objective:** Define the YAML shape of an opt-in workflow declaration and make it parseable. Still no engine; this is a pure data-model phase. Stores without `workflow:` keep behaving exactly as v0.1.
- **Tasks:**
  - 2.1: Define `Workflow { agent_roles: BTreeMap<String, AgentRole>, briefing_templates: BTreeMap<String, PathBuf>, render_template: Option<PathBuf>, render_target_path: Option<String>, on_state: BTreeMap<String, Vec<StateAction>>, submit_targets: BTreeMap<String, String>, max_revise_cycles: Option<u32> }` in a new `src/schema/workflow.rs`. `AgentRole { name: String, description: Option<String> }` is just a typed name binding. `enum StateAction { DispatchAgent(String), Increment(String), TransitionTo(String) }` is the per-state action list executed on entry to that state. Fields:
    - `agent_roles`: `{ planner: { description: "..." }, executor: ..., ... }` — names match `briefing_templates` keys.
    - `briefing_templates`: `{ planner: "templates/planner-brief.md.tpl", executor: ..., ... }` — paths relative to the store package root (resolved at install time and read into memory; no runtime FS access).
    - `render_template`: `templates/main.md.tpl` — path to the main.md render template, resolved similarly.
    - **`render_target_path` (added in 2.1 per M3/M4):** Handlebars-templated string like `"tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md"` resolved by the render verb (Phase 6).
    - `on_state`: `{ planning: [DispatchAgent(planner)], plan_review: [DispatchAgent(plan_reviewer)], executing: [DispatchAgent(executor)], ... }` — drives `next-action`'s "which agent acts now" answer.
    - **`submit_targets` (added in 2.1 per M3):** `BTreeMap<String, String>` from submit-verb name (`submit-execute`, `submit-review`, `submit-plan`, `submit-plan-review`) to the list-record / record field that verb writes to. This binds Phase 5's generic CLI to per-store schema field names. Validation: every key must be one of the four submit verbs; every value must name an existing field on the schema; `submit-plan` must target a `record` field; the other three must target a `list_record` or `record` field. (The original plan added this retroactively in Phase 5.3; pulling forward eliminates the cross-phase dependency hazard.)
    - `max_revise_cycles`: defaults to 3 (D6 says 4th attempt rejects; this is a hint for the schema-level guard the planner writes by hand on `tasks` transitions; the framework reads this only to error-message nicely).
  - 2.2: Add `pub workflow: Option<Workflow>` to `Schema`. Parse via `serde_yaml`. A schema with no `workflow:` key keeps `Option::None` — every existing test passes.
  - 2.3: At install time, for stores that declare `workflow:`, **read the briefing template files and render template into memory and embed them in the in-memory `Workflow`** (the templates are part of the store package, like `schema.yaml`; we don't want runtime FS reads). For bundled stores, the templates are `include_str!`'d (mirroring how `BUNDLED_STORE_SCHEMAS` works in `src/cli/dynamic.rs`). For installed-from-path stores, read from disk during `install`. This makes `tests/e2e.sh` reproducible from a fresh tmp dir.
  - 2.4: Add `WorkflowResolved` (an in-memory variant where template paths are replaced with `template_text: String`) — the in-memory `Workflow` carries text, not paths, after the install step.
  - 2.5: Schema validation rules: every `agent_roles` key must have a corresponding `briefing_templates` entry; every `on_state.<state>` must reference an existing state in `lifecycle.states`; every `DispatchAgent(role)` must reference an existing `agent_roles` key. Error messages name the offending key.
- **Acceptance Criteria:**
  - AC2.1: A schema **without** `workflow:` parses identically to v0.1; `schema.workflow.is_none()` and all 110+Phase-1 tests pass.
  - AC2.2: A schema with a complete `workflow:` block parses; **agent_roles + on_state + briefing_templates + submit_targets + render_target_path** round-trip.
  - AC2.3: A schema referencing a state in `on_state` that is missing from `lifecycle.states` errors with that state name in the message.
  - AC2.4: A schema referencing an unknown agent role in `DispatchAgent(...)` errors clearly.
  - AC2.5: At install time, a non-existent briefing template path errors with the missing path.
  - **AC2.6 (NEW per M3):** A `submit_targets` entry pointing at a non-existent field errors with the field name. A `submit_targets` entry for `submit-plan` pointing at a non-record field errors with a type-shape message.
- **Files:**
  - `src/schema/workflow.rs` — new
  - `src/schema/mod.rs` — wire `Workflow` into `Schema`
  - `src/install.rs` — read template files into memory at install time (path-installed stores); same shape applies to bundled stores via Phase 8
  - `tests/fixtures/workflow_minimal/` — new directory with `schema.yaml` + tiny templates for parser tests
- **Dependencies:** Phase 1 (uses `Lhs/Op/Rhs` for `guard:` parsing in transitions; uses `requires_gate` and `list_record` from 1.7/1.10; otherwise independent).

#### Phase 3: Briefing template engine — Handlebars-style partial substitution

- **Objective:** Pick a template engine and exercise it with the briefing fixtures. The `render` verb in Phase 6 reuses the same engine for `main.md`. This phase ends with: `pub fn render_template(text: &str, ctx: &serde_json::Value) -> Result<String>` working on a fixture briefing with all the substitutions the tasks store will need.
- **Tasks:**
  - 3.1: **Decide template engine** (see Decision Matrix). Recommended: `handlebars` (Rust crate, BSD-3, ~50KB compiled, supports the partials/iterators we'll need for `phases:` and `cycles:` lists). Add to `Cargo.toml`. Alternative considered: `tinytemplate` (smaller, but no list iteration helpers — render template needs `{{#each phases}}`).
  - 3.2: Implement `src/render/engine.rs` exposing `pub fn render_template(text: &str, ctx: &serde_json::Value) -> Result<String>`. Wraps a `handlebars::Handlebars` instance. Register helpers we need: `eq` (for `{{#if (eq status "BLOCKED")}}`), `default` (for `{{default field "—"}}`), `gt`/`lt` (occasionally needed in render templates for "latest cycle" arithmetic).
  - 3.3: Implement `src/render/context.rs` exposing `pub fn build_context(schema: &Schema, entry: &EntryMap) -> serde_json::Value`. Walks the entry; emits a JSON shape that mirrors the schema, with one engine-only addition: `current_cycle_for_phase: { 1: 2, 2: 1, ... }` derived from the `cycles` list — a convenience the templates use. (We could compute this in the template via `{{#each}}` iteration; computing it in Rust keeps templates dumber.)
  - 3.4: Add unit tests covering: text passthrough, variable substitution, `{{#if}}` branching, `{{#each list}}`, missing key returns empty string (NOT error — matches Handlebars default; our render must never crash on partial DB rows).
  - 3.5: Add a fixture briefing template under `tests/fixtures/workflow_minimal/templates/planner-brief.md.tpl` with all four substitution patterns and assert the rendered output byte-for-byte.
- **Acceptance Criteria:**
  - AC3.1: `render_template` round-trips a static template (no `{{}}`) byte-identical.
  - AC3.2: Variable substitution: `{{title}}` resolves from the context; missing variables render empty.
  - AC3.3: `{{#each phases}}…{{this.name}}…{{/each}}` iterates a list of records (this is what render uses for the Plan section).
  - AC3.4: `{{#if (eq status "BLOCKED")}}…{{/if}}` works (this is what render uses for conditional sections like Blocked Reason).
  - AC3.5: Build context for a tasks-shaped fixture entry produces a JSON object whose top-level keys equal the schema's field names plus `current_cycle_for_phase`.
- **Files:**
  - `src/render/mod.rs` — module root
  - `src/render/engine.rs` — engine + helpers
  - `src/render/context.rs` — entry → context shape
  - `Cargo.toml` — add `handlebars = "5"`
  - `tests/fixtures/workflow_minimal/templates/planner-brief.md.tpl` — fixture
- **Dependencies:** Phase 2 (uses `Workflow`'s in-memory template text).

#### Phase 4: Generic workflow CLI verbs (read-only) — `next-action` + `brief`

- **Objective:** Land the two read-only workflow verbs on **every** workflow-shaped store. Read DB state, compute "which agent should act," produce a markdown briefing for the LLM (or JSON for the orchestrator). After this phase, the orchestrator can poll a workflow store but cannot yet write submissions.
- **Tasks:**
  - 4.1: Add `next_action` and `brief` verbs to `src/cli/dynamic.rs::build_store_command`, but **only when the schema has `workflow: Some(_)`**. Stores without `workflow:` keep the v0.1 surface (no behavioural change for `observations`/`gate` until Phase 9 where `observations` opts in selectively). Both verbs take a positional `<id>` and a `--json` flag (already global).
  - 4.2: Implement `src/handlers/next_action.rs::run(schema, conn, matches, invoker) -> Result<()>`. Logic:
    1. Read the row by display_id (existing `read_row`).
    2. Read `status` from row + `workflow.on_state[status]`.
    3. The first `DispatchAgent(role)` in the action list = the agent who should act now. (Engine-fired actions like `Increment` and `TransitionTo` execute synchronously inside Phase 5 verbs; they never appear as the "agent who should act now.")
    4. Output (text default, `--json` for orchestrator). Per M7, the JSON includes `claimed_by` and `claimed_at` so the orchestrator can surface a useful "row claimed by <other> until <time>" error without a separate `show` call:
       ```
       {
         "id": "T003",
         "status": "executing",
         "current_phase": 2,
         "current_cycle": 1,
         "next_agent": "executor",
         "blocked": false,
         "blocked_reason": null,
         "claimed_by": null,
         "claimed_at": null
       }
       ```
       Text form is the same key/value rendered as `key: value` lines.

    **Per C4 (cycle 2 fix):** This verb is purely a **read primitive**. It returns which agent SHOULD be spawned next; it is NOT used to validate submission writes. The actor model in `validate/actor.rs` is what enforces "is this the right agent for this state" — `next-action` does not gate writes. The original plan-review (Phase 5 dependencies bullet) incorrectly described `next-action` as a validator-side check; that line is removed in the revised Phase 5.
  - 4.3: Implement `src/handlers/brief.rs::run(schema, conn, matches, invoker) -> Result<()>`. Logic:
    1. Read the row.
    2. Resolve `--for <agent>` (if absent, default to `next-action`'s answer).
    3. Look up the briefing template via `workflow.briefing_templates[agent]`.
    4. Build the render context (Phase 3.3).
    5. Render the template.
    6. Output: markdown to stdout (default) or `{ "agent": "...", "brief_markdown": "..." }` JSON if `--json`.
  - 4.4: Add an `--for <agent>` flag on `brief` (free-text; validated against `workflow.agent_roles` keys).
  - 4.5: Both verbs call `paths::stores_dir_for(scope)` so a workflow store installed under `scope: repo` works from any worktree of the same repo.
- **Acceptance Criteria:**
  - AC4.1: `stores tasks next-action T003` on a row with `status: executing, current_phase: 2, current_cycle: 1` prints the nine-key text response with `next_agent: executor` and `claimed_by`/`claimed_at` (NULL on an unlocked row).
  - AC4.2: `stores tasks next-action T003 --json` prints valid JSON with the same nine fields. **(NEW per M7):** On a row with a held lock, `claimed_by` reads back the invoker name and `claimed_at` is non-null.
  - AC4.3: `stores tasks brief T003` prints the executor briefing markdown for phase 2 with substituted task title, DONE_WHEN, phase-2 tasks, and prior code-review feedback (if any).
  - AC4.4: `stores tasks brief T003 --for plan-reviewer` overrides the default and prints the plan-reviewer briefing.
  - AC4.5: `stores tasks brief T003 --for nonexistent_agent` errors; the error message names the unknown role AND lists every available role (test asserts the strings `planner`, `plan_reviewer`, `executor`, `code_reviewer` all appear). (m3 fix.)
  - AC4.6: `next-action` on a row with `status: blocked` reports `blocked: true` and `next_agent: null` (no agent acts on a blocked row; human input required).
  - AC4.7: `next-action` on a v0.1-style store (no `workflow:` block) errors with "store '<name>' has no workflow declaration; verb only works on workflow-shaped stores."
- **Files:**
  - `src/handlers/next_action.rs` — new
  - `src/handlers/brief.rs` — new
  - `src/handlers/mod.rs` — add the modules
  - `src/cli/dynamic.rs` — wire verbs (gated on `workflow.is_some()`)
  - `src/cli/dispatch.rs` — route `next-action` and `brief` to their handlers
- **Dependencies:** Phase 2 (Workflow shape), Phase 3 (template rendering).

#### Phase 5: Generic workflow CLI verbs (write) — `submit-execute`, `submit-review`, `submit-plan`, `submit-plan-review` (revised cycle 2: C2 guard fix; C3 transaction boundary; C4 dropped next-action claim; M8/M10/M11 explicit engine post-actions; M9 PASS disambiguation; M5 ready→executing on-entry firing)

- **Objective:** Land the four write-path workflow verbs. Each verb is a thin wrapper around an extended `Op` shape that the validator + transition handler already understand; the new bits are: input parsing (file flags), engine-fired post-actions (`auto_increment*` reset, framework-actor field writes, follow-on transitions), and the schema-level guard evaluation that enforces D6's 3-cycle limit. After this phase, an orchestrator can drive a row through the entire state machine via the new verbs alone.

  **Cycle 2 framing changes:**
  - The 4th-revise guard now uses **`current_cycle <= 4`** (per-phase counter, post-increment ordering) instead of the broken `cycles.length < 3` (cumulative). The exact form is justified in 5.5; the initial pass at `current_cycle <= 3` was off by one and was corrected during the worked-example transcript audit at the end of the Plan section.
  - The two-write transaction is **explicitly specified end to end**: where it begins, what's inside it, what's outside it, and what happens on crash mid-transaction (C3 / 5.7).
  - The "next-action used to confirm agent invariant" claim is **dropped** (C4); validator's actor model is the only invariant enforcement on submission.
  - Every engine post-action — increment, reset, lock release, follow-on transition — is enumerated in a per-transition table (M8/M10/M11 / 5.4).
  - The two PASS transitions on `code_review` are **disambiguated by guard expression** (M9 / 5.5b).

- **Tasks:**
  - 5.1: Add new `Op` variants in `src/validate/mod.rs`:
    ```
    pub enum Op {
        Add,
        Update(EntryMap),
        Transition(String, EntryMap),
        SubmitPlan(EntryMap),                  // verb: submit-plan
        SubmitPlanReview(String, EntryMap),    // gate ∈ {READY, NEEDS_WORK, NOT_READY}
        SubmitExecute(EntryMap),               // verb: submit-execute
        SubmitReview(String, EntryMap),        // gate ∈ {PASS, REVISE, FAIL}
    }
    ```
    Each `Submit*` op maps to a specific transition verb internally. The `String` parameter on the gate-bearing variants is the gate decision, used by the engine to choose the transition target. Validator runs the same actor/required/required_when/enum/pattern checks against the diff just like `Transition`. Actor scoping for `Submit*` ops mirrors `Transition`: actor checks scoped to diff only (per A15).
  - 5.2: Add `guard:` evaluation. Extend `Transition` in `src/schema/lifecycle.rs` with `guard: Option<Expr>` (`requires_gate` already landed in Phase 1.10). In `src/handlers/transition.rs::run`, after the state-machine legality check, **evaluate the guard against the merged entry** (the entry that includes the engine's pending writes — current_cycle++, etc., applied to a working copy). If the guard is present and `eval(guard, &merged) == false`, the transition is rejected; the engine then looks up an alternate transition that handles the failure case (e.g. `code_review → blocked`, declared with `requires_gate: REVISE` on `tasks` but with an unmet guard means the `→ blocked` fallback fires instead — see 5.5 for the precise lookup algorithm). Engine-fired transitions (actor: framework) skip the auto-block-on-guard-fail step — the framework only fires transitions whose guards it has already verified.
  - **5.3 (revised per C3, M3): Submit handler structure.** Implement `src/handlers/submit.rs` with one entry point per verb (`submit_execute::run`, `submit_review::run`, `submit_plan::run`, `submit_plan_review::run`). Each follows this strict sequence:
    1. **Open the transaction immediately.** `let tx = conn.unchecked_transaction()?;` is the first DB-touching line. **The transaction is the boundary.** Every read, write, lock-acquire, lock-release, and follow-on transition in this submit happens inside `tx`. Nothing the submit handler does after this line touches `conn` directly; only `tx` (or `&*tx` viewed as a `&Connection`).
    2. **Acquire the row lock inside the transaction.** UPDATE `tasks SET claimed_by = ?, claimed_at = now() WHERE display_id = ? AND (claimed_by IS NULL OR claimed_at < now() - INTERVAL 5 MINUTES)`. If 0 rows changed, return the lock-error and `tx` rolls back on drop.
    3. **Read the row** (`read_row`) using the locked transaction.
    4. **Parse CLI flags into the verb-specific diff.** Flags are fixed per verb (no overloaded args, D3):
       - `submit-execute`: `--summary <text>` or `--summary-from-file <f>`, `--commit <sha>`, `--files-changed <csv>`, optional `--notes-from-file <f>`
       - `submit-review`: `--gate <PASS|REVISE|FAIL>`, `--critical N`, `--major N`, `--minor N`, `--summary <text>`, `--details-from-file <f>`
       - `submit-plan`: `--plan-from-file <f>` (JSON; per A10)
       - `submit-plan-review`: `--gate <READY|NEEDS_WORK|NOT_READY>`, `--summary <text>`, `--open-questions-from-file <f>`
    5. **Translate flags into a list-record/record entry.** The target field is looked up via `workflow.submit_targets[verb]` (M3: this Workflow field landed in Phase 2.1). For list-record targets (cycles, plan_review_log) the entry is appended; for record targets (plan) it overwrites.
    6. **Validator pass** with the appropriate `Op::Submit*` variant against the (locked, diffed) entry. On validator failure, `tx` rolls back; lock release happens via the rollback (lock was only set on UPDATE).
    7. **Compute engine post-actions** (full list in 5.4 below).
    8. **Apply the user-write UPDATE** inside `tx`: write the diff into the target column (JSON-as-TEXT for list-record / record), update `status` to the transition's `to` state, write any framework-actor field changes (current_phase, current_cycle, blocked_reason).
    9. **Fire follow-on transitions** (5.4) by recursively calling the same handler shape with `Actor::Framework` as the invoker, **passing `&tx` not `&conn`** — see 5.7 for the refactor that makes this possible. Follow-on transitions also write inside `tx`. They do their own validator/guard evaluation; if a follow-on guard fails, the entire `tx` rolls back (because the engine should not have fired a transition whose guard didn't hold; that's a schema-author bug and should surface as an error).
    10. **Release the lock** as the FINAL action inside `tx`: `claimed_by = NULL, claimed_at = NULL`. (Per M11: lock release is the last action of the transaction, after all engine-fired post-actions complete. This way a follow-on's `read_row` inside `tx` still sees the row's lock as held by the original invoker, and a third party trying to acquire while we're mid-tx is blocked at the SQLite-locking level.)
    11. **Commit `tx`.** A successful commit is the only way any of the writes become visible.
    12. **Print one-line summary** to stdout, e.g. `Submitted execute for T003 phase 2 cycle 1; status now: code_review`.
    13. **(After commit, OUTSIDE `tx`):** No render call here — the orchestrator skill (Phase 8) calls `stores tasks render <id>` as a separate command. If render fails, the DB is consistent and re-render is idempotent; if render succeeds and we crash before printing, the next orchestrator iteration sees the consistent DB and re-renders harmlessly.

  - **5.4 (revised per M8/M10/M11): Engine post-action table.** For each combination of (current state, submit verb, gate), the engine fires a specific set of post-actions. **All of these execute INSIDE the submit handler's transaction** (5.3 step 9). The lock is released as the very last step before commit (5.3 step 10).

    | Trigger | Status transition | Framework-field writes | Follow-on |
    |---|---|---|---|
    | Initial `add` | (none — `Op::Add` path) | `current_phase = 0`, `current_cycle = 1` (initial values) | None |
    | submit-plan | planning → plan_review | (none) | None |
    | submit-plan-review --gate READY | plan_review → ready → executing | On `→ ready`: nothing. On `ready → executing` (TransitionTo on-entry, M5): `current_phase = 1`, `current_cycle = 1`. | The `on_state.ready: [TransitionTo(executing)]` action fires synchronously inside the same `tx` (M5; specifically tested in AC5.11). |
    | submit-plan-review --gate NEEDS_WORK | plan_review → planning | (none) | None. The `plan_review_log.length < 3` guard on the `→ planning` transition gates this; if it fails, the engine matches the `→ blocked` transition instead (per 5.5's lookup algorithm). |
    | submit-plan-review --gate NOT_READY | plan_review → blocked | `blocked_reason = "plan-reviewer marked NOT_READY: <summary>"` | None. |
    | submit-execute | executing → code_review | (none — `current_cycle` only increments on REVISE; the cycle counter labels the cycle that's CURRENTLY in flight) | None. |
    | submit-review --gate REVISE | code_review → executing | `current_cycle += 1` (increments BEFORE the guard fires — see 5.5 for the precise sequencing) | None. |
    | submit-review --gate REVISE *(post-increment guard fails: `current_cycle <= 4` evaluates false at would-be value 5)* | code_review → blocked | `blocked_reason = "4th revise rejected by guard current_cycle <= 4 on phase {N} cycle {M}: <last-review summary>"` (Q5 close: engine writes context-rich reason) | None. |
    | submit-review --gate PASS *(non-last phase)* | code_review → executing | `current_phase += 1`, `current_cycle = 1` (reset, per `auto_increment_within: current_phase`) | None (the executing state's `on_state` dispatches the executor agent for the new phase via `next-action`). |
    | submit-review --gate PASS *(last phase)* | code_review → complete | (none) | None. |
    | submit-review --gate FAIL | code_review → blocked | `blocked_reason = "code-reviewer marked FAIL on phase {N}: <summary>"` | None. |
    | resume (verb: resume, actor: ai_with_human) | blocked → ready → executing | `current_cycle = 1` (reset; `cycles` list preserved as audit trail per M10). On `ready → executing`: `current_phase` UNCHANGED (resume returns the row to where it was blocked, not to phase 1). | `ready → executing` follow-on fires per `on_state.ready: [TransitionTo]` (M5). |

    **Lock semantics across follow-ons (M11):** The lock acquired in 5.3 step 2 is held throughout `tx`. Follow-on transitions executed inside `tx` see `claimed_by` as the original invoker (or `framework` if a prior follow-on already swapped it — but we don't swap; the original lock stays). The lock is released in 5.3 step 10 (final action before commit). If the process crashes mid-`tx`, SQLite rolls back the transaction; the lock UPDATE is part of the same transaction so it also rolls back, leaving the row's `claimed_by` as it was before this submit started.

    **`current_cycle` initial value (cycle 2 clarification):** The schema's `auto_increment` on `current_cycle` initializes to `1` (not `0`) on `add`. This makes "cycle 1" the natural label for the first execute attempt; cycle 2 the first revise; cycle 3 the second revise; cycle 4 the third revise — and on the 4th submit-review --gate REVISE the engine attempts to bump `current_cycle` to `5`, the post-increment guard `current_cycle <= 4` evaluates `false`, and the engine routes to BLOCKED. (See 5.5 below for the full step-by-step.)

  - **5.5 (revised per C2): The 3-cycle guard, fixed.** The original plan used `cycles.length < 3`, which is wrong on two counts (per plan-review C2): it counts cumulatively across all phases (so a phase 2 re-execute hits the limit even if phase 1 had zero revises), and it's algebraically off-by-one (after 3 revise cycles, `cycles.length == 3`, so the guard fails on the 3rd attempt instead of the 4th). The fix:

    **Use the per-phase counter `current_cycle`, with post-increment ordering and the expression `current_cycle <= 4`.** The schema declares `current_cycle: integer, actor: framework, auto_increment: true, auto_increment_within: current_phase`, which means it resets to `1` whenever `current_phase` advances (including on the initial `add` and on PASS-non-last). The submit-review handler bumps `current_cycle` first (in a working copy), then evaluates the guard:

    | Submit-review attempt | current_cycle BEFORE | engine bumps to | Guard `current_cycle <= 4` (post-bump) | Outcome |
    |---|---|---|---|---|
    | 1st REVISE (after initial cycle-1 execute) | 1 | 2 | true (2 <= 4) | status: executing, cycle 2 begins |
    | 2nd REVISE | 2 | 3 | true (3 <= 4) | status: executing, cycle 3 begins |
    | 3rd REVISE | 3 | 4 | true (4 <= 4) | status: executing, cycle 4 begins |
    | 4th REVISE attempt | 4 | 5 (working copy) | **false (5 > 4)** | working-copy bump rolled back; engine routes to `code_review → blocked` (the unguarded REVISE-fallback transition); status: blocked, blocked_reason populated |

    **Why `<= 4`, not `<= 3`?** The user's stated semantics are "3-cycle REVISE limit; 4th attempt routes to BLOCKED." Three REVISEs means cycles 2, 3, 4 all proceed (the 1st/2nd/3rd revise produce cycles 2/3/4 of execution); the 4th REVISE attempt — which would create cycle 5 — fails. Initial value 1, post-increment, guard `<= 4`. (The cycle-2 first-pass mistakenly wrote `<= 3`, which would have allowed only 2 REVISEs; this was caught by the worked-example transcript audit at the end of the Plan section.)

    **Why post-increment, not pre-increment?** Pre-increment would allow cleaner `current_cycle < 4` semantics, but it makes the engine's working-copy / rollback logic more complex (the bump has to be reversed on guard fail; with post-increment-on-success we just don't apply the bump on guard fail). Engineering preference for the simpler implementation.

    **Phase 7 schema** uses `guard: "current_cycle <= 4"` on the REVISE → executing transition (see Phase 7's revised schema YAML).

  - **5.5b (NEW per M9): Two PASS transitions disambiguated by guard.** On `code_review` with `requires_gate: PASS`, the schema declares two separate transitions:
    - `{from: code_review, to: complete, verb: submit-review, requires_gate: PASS, guard: "current_phase >= plan.phases.length", actor: ai_autonomous}`
    - `{from: code_review, to: executing, verb: submit-review, requires_gate: PASS, guard: "current_phase < plan.phases.length", actor: ai_autonomous}`

    Engine selection on PASS:
    1. Filter transitions to those matching `(from=code_review, verb=submit-review, requires_gate=PASS)`.
    2. Of those, find the one whose guard evaluates `true` against the (engine-bumped) entry.
    3. Exactly one should match by construction (the guards partition `current_phase` against `plan.phases.length`). If zero match, error "no PASS transition's guard satisfied"; if multiple, error "ambiguous PASS — guards overlap" (a schema-author bug).

    **This requires the guard expression language to compare against `plan.phases.length`.** Per D6, length operators are in scope. The path `plan.phases.length` requires the expression evaluator to walk a record-into-list-record path — which Phase 1.4's eval already supports (Phase 1.9 AC depends on this same depth).

    **Tradeoff considered:** the alternative was a single transition with imperative engine logic ("if last phase complete else executing"). Schema-declared guards win because (a) schema is the source of truth and a code reviewer can audit transition semantics without reading Rust, (b) the path is consistent with the 4th-revise guard's approach, (c) it lets a future schema author override the "what counts as last phase" rule without changing engine code.

    **Engine-fired follow-on (5.4 / M9 hand-off):** The submit-review handler's user-action write transitions code_review → {complete | executing}. The original plan-review noted "the engine fires this with `actor: framework` after the code_reviewer's submit-review" — but per the table in 5.4, this is actually a single transition that both writes the cycles[].review entry and moves the status forward. There is no separate `framework`-actor follow-on for the PASS path (unlike the `ready → executing` case which IS a separate framework follow-on per `on_state.ready: [TransitionTo(executing)]`). The actor on the transition is `ai_autonomous` (the code-reviewer); the engine does the framework-field bumps (current_phase++, current_cycle reset) inside the same write — these are framework-actor field writes, not a separate transition. This stays inside `tx`.

  - **5.6 (revised per C4): Drop next-action-as-validator.** The original plan listed Phase 4 as a dependency of Phase 5 with the rationale "next-action used by submit handlers to confirm the agent invariant before writing." This is incorrect: the validator's actor model already enforces "is this the right agent for this state" via the per-field actor checks. `next-action` is purely a read primitive that the orchestrator skill calls to figure out which subagent to spawn next; it does not gate writes. Submit handlers do NOT call `next-action`. The dependency on Phase 4 is removed (Phase 5 still depends on Phases 1-2-3 for schema features). State machine legality is enforced by the existing `read_row → check status matches transition.from` check in `transition.rs`.

  - **5.7 (NEW per C3): Refactor `transition::run` to compose with the submit handler's transaction.** Today `src/handlers/transition.rs` does `conn.execute(...)` directly with no transaction wrapping. To make follow-on transitions composable inside the submit handler's `tx`:
    - Split `transition::run` into a thin entry point and a transaction-agnostic core: `pub fn run(...)` opens its own transaction (preserves existing single-call semantics for direct CLI `transition` use); `pub(crate) fn run_in_tx(tx: &Transaction, schema, ..., invoker) -> Result<()>` does the read/validate/write/follow-on logic against a caller-supplied transaction.
    - Submit handlers call `run_in_tx` directly with their own `tx`; the engine's follow-on dispatch also calls `run_in_tx` with the same `tx`.
    - All existing transition tests get a one-line wrapper that opens an in-memory transaction and forwards to `run_in_tx`; existing call sites of `run(...)` continue to work unchanged.

  - 5.8: Locking specifics (already covered in 5.3 / 5.4; consolidating here for the file): each `submit-*` verb starts by attempting to claim the row via the conditional UPDATE in 5.3 step 2. If 0 rows changed, error "row T003 is claimed by <other> until <time>; retry after expiry." On submit completion (5.3 step 10) the same transaction releases the lock. **Lock is held across follow-on transitions** (M11) and released as the final action inside `tx`.
- **Acceptance Criteria:**
  - AC5.1: `submit-execute T003 --summary "phase 2 done" --commit abc123 --files-changed src/foo.rs,src/bar.rs` writes a new `cycles[]` row, transitions `executing → code_review`, releases the lock at commit; row reads back with `status: code_review`, `cycles[<idx>].executor.summary == "phase 2 done"`, `claimed_by IS NULL`.
  - AC5.2: `submit-review T003 --gate PASS --critical 0 --major 0 --minor 1 --summary "approved"` on a non-last phase (`current_phase < plan.phases.length`) writes `cycles[<idx>].review`, fires `code_review → executing`, bumps `current_phase` from N to N+1, resets `current_cycle` to 1. **Single transaction; lock released on commit.** (M9 / M11 verified by inspecting row post-call.)
  - AC5.3: `submit-review T003 --gate PASS --critical 0 --major 0 --minor 0 --summary "final"` on the last phase (`current_phase == plan.phases.length`) fires `code_review → complete`; `current_phase` does NOT increment past the last; `status: complete`. **(M9: guard `current_phase >= plan.phases.length` selects the `→ complete` transition over the `→ executing` one.)**
  - AC5.4 **(C2 fix):** `submit-review T003 --gate REVISE` three times in a row succeeds (current_cycle bumps to 2, then 3, then 4); the **4th** REVISE attempt fails the post-increment guard `current_cycle <= 4` (would-be value 5); status becomes `blocked`; `blocked_reason` reads back populated with the guard-fail message naming the phase and cycle. Verbatim from DONE_WHEN.
  - **AC5.4b (NEW per C2):** Cross-phase isolation test. Run a task through phase 1 with 2 REVISE cycles, then PASS to phase 2. Phase 2's first submit-review --gate REVISE: `current_cycle` was reset to 1 on the phase advance, so the guard sees `current_cycle == 2` post-bump (well within `<= 4`) and the revise proceeds normally. **Asserts that the per-phase counter does NOT carry forward across phases** (the original `cycles.length < 3` bug).
  - AC5.5: Two concurrent `submit-execute` calls on the same row: the second fails with the lock error and names `claimed_by`. After 5 minutes (simulate by manipulating `claimed_at` directly in the test), the second succeeds.
  - AC5.6: `submit-plan T003 --plan-from-file plan.json` writes the plan as a record into the `plan` field, fires `planning → plan_review`.
  - AC5.7: `submit-plan-review T003 --gate READY --summary "approved" --open-questions-from-file -` (stdin) fires `plan_review → ready` THEN immediately the engine fires `ready → executing` per `on_state.ready: [TransitionTo(executing)]` — both writes inside the same transaction. After commit: `status: executing`, `current_phase: 1`, `current_cycle: 1`. **(M5 + M8: addresses the on-entry-action firing.)**
  - AC5.8: `submit-plan-review T003 --gate NEEDS_WORK --summary "..."` fires `plan_review → planning` (back to planner). After 3 NEEDS_WORK cycles, the 4th fails the `plan_review_log.length < 3` guard and the engine routes to `plan_review → blocked` with a populated `blocked_reason`.
  - AC5.9: `submit-plan-review T003 --gate NOT_READY --summary "..."` fires `plan_review → blocked`; `blocked_reason` populated with summary.
  - AC5.10: A submit-* on a v0.1 store (no `workflow:`) errors clearly.
  - **AC5.11 (NEW per C3): Atomic boundary test.** Force a panic between the submit-write and the framework-fired follow-on transition (insert a `panic!()` at a defined test hook in `submit.rs` between 5.3 step 8 and 5.3 step 9). Restart the process. Assert: the row's `status` is the pre-submit value, `current_phase`/`current_cycle` are pre-submit, the `cycles` list has NOT gained a new entry, and `claimed_by IS NULL` (the lock was acquired and released by the same rolled-back transaction). Either both writes apply or neither does.
  - **AC5.12 (NEW per C3): Render is downstream of commit.** A failure of `stores tasks render <id>` after a successful submit does NOT corrupt the DB. Re-running render produces the consistent output. (Confirms the boundary: render is post-commit, idempotent, retry-able.)
  - **AC5.13 (NEW per M11): Lock held across follow-on.** During the framework-fired `ready → executing` follow-on inside a submit-plan-review --gate READY, the row's `claimed_by` is the original invoker (asserted by reading the row from a separate connection during a debug hook between the two writes — for the test, hold the tx open with a debugging breakpoint). After commit, lock is NULL.
  - **AC5.14 (NEW per M10 / M11): BLOCKED → READY recovery.** A row at `status: blocked, current_cycle: 4, current_phase: 1` resumes via `stores tasks resume T003`. Assertions: `status: executing` (after the on-entry `ready → executing`), `current_phase: 1` (unchanged — resume returns to the blocked phase), `current_cycle: 1` (reset; per the post-action table). The `cycles` list is preserved as audit trail (N entries from the failed run remain).
- **Files:**
  - `src/validate/mod.rs` — `Op::Submit*` variants
  - `src/schema/lifecycle.rs` — `guard: Option<Expr>` on `Transition` (recall `requires_gate` already added in Phase 1.10)
  - `src/handlers/submit.rs` — new (one module, four sub-handlers)
  - `src/handlers/transition.rs` — guard evaluation; auto-block on guard fail; **split into `run` and `run_in_tx` per C3 / 5.7**
  - `src/cli/dynamic.rs` — register submit-* subcommands when workflow.is_some()
  - `src/cli/dispatch.rs` — route submit-* verbs
- **Dependencies:** Phase 1 (Expr/eval, list_record, requires_gate), Phase 2 (Workflow + submit_targets). **Phase 4 dependency removed (C4): submit handlers do NOT call `next-action`; the validator's actor model is the invariant enforcement.**

#### Phase 6: `render` verb + idempotent main.md projection

- **Objective:** Land the `render` verb. Reads a row + the store's render template, emits `tasks/<dir>/main.md`. Idempotent: re-running on an unchanged DB produces a byte-identical file. Files are written next to the on-disk task dir, computed from the row's `slug` field.
- **Tasks:**
  - 6.1: Implement `src/handlers/render.rs::run(schema, conn, matches, invoker) -> Result<()>`. Reads the row, builds the render context, renders the template, writes to disk. Output path is determined by a new `render_target_path: Option<String>` on `Workflow` (a Handlebars-renderable template string like `"tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md"` evaluated against the same context). For tasks: `tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md` where `status_dir` is computed by `render::context.rs` from `status` (`planning|plan_review → "planning"`, `ready|executing|code_review → "active"`, `blocked → "paused"`, `complete → "completed"`).
  - 6.2: Atomic write: write to `<path>.tmp` then rename. (Same pattern as `manifest.rs::save`.)
  - 6.3: Directory move on `status_dir` change: when the row's render path differs from the existing on-disk location, the render verb moves the directory (`std::fs::rename`) before writing. Detection logic: query the FS for a directory matching `tasks/*/{{display_id}}-*`. If exactly one match exists at a non-current `status_dir`, move it. If zero or more than one match exists, render to the canonical path and emit a warning (don't error — render must be idempotent and recoverable). **Per M1: `slug` is set at `add` time and is conventionally immutable thereafter (the schema marks it `actor: ai_with_human` with a `pattern` constraint; the framework does NOT enforce set-once at the validator level — that's deferred). If a user does mutate slug, the directory-move detection still works correctly: the glob matches by display_id (T003-*), so the old directory is found and moved; the new render writes the file under the new slug; the old empty directory is then removed by render.**
  - 6.4: `main.md` template authoring: `stores/tasks/templates/main.md.tpl` mirrors the canonical layout from `~/repos/plugins/task-workflow-plugin/templates/main.md`, with sections rendered from DB rows. Each `## Section` body is a Handlebars partial. Sections fed by list-records (Plan, Execution Log, Code Review Log) iterate the list via `{{#each}}`. Empty sections render as `_<placeholder>_` per the template's existing convention.
  - 6.5: `--dry-run` flag prints the rendered text to stdout without writing. Used by `tests/e2e.sh` for byte-identical assertions.
  - 6.6: Idempotency test: `render T003`, capture mtime, `render T003` again with no DB change, assert mtime advanced (atomic write replaces every time) but file content is byte-identical.
- **Acceptance Criteria:**
  - AC6.1: `stores tasks render T003` on a row in `executing` writes `tasks/active/T003-<slug>/main.md` with the canonical layout (Meta, Task, Plan, Plan Review, Execution Log, Code Review Log, Completion).
  - AC6.2: `--dry-run` prints to stdout, writes nothing.
  - AC6.3: A row in `complete` causes the directory to move from `tasks/active/T003-<slug>/` to `tasks/completed/T003-<slug>/` before the render write.
  - AC6.4: Re-running `render T003` produces byte-identical content (per AC6.6 in tasks).
  - AC6.5: A row in `blocked` renders the Blocked Reason section with the actual blocked_reason from the DB.
  - AC6.6: `render` does NOT modify any DB row (read-only against SQLite).
- **Files:**
  - `src/handlers/render.rs` — new
  - `src/cli/dynamic.rs` — register `render` verb when workflow.is_some()
  - `src/cli/dispatch.rs` — route `render`
  - `src/render/path.rs` — render-target-path resolver + directory-move logic (split out for testability)
- **Dependencies:** Phase 3 (template engine), Phase 4 (read-row), Phase 5 (state machine drives `status`).

#### Phase 7: `tasks` store schema + bundled templates (revised cycle 2: schema YAML updated for C2 guard fix and M9 dual-PASS-transition guards; list_record/list_fk are now Phase 1 features used by the schema, not introduced here)

- **Objective:** Author the `tasks` schema YAML using every feature from Phases 1–6. Bundle the four briefing templates + the main.md render template. Bundle the store via the existing `BUNDLED_STORE_SCHEMAS` mechanism. **Cycle 2:** the schema's `transitions` block has been corrected: the 4th-revise guard uses `current_cycle <= 4` (post-increment, per C2 / 5.5); the two PASS transitions on `code_review` are explicitly disambiguated by guard expressions (per M9 / 5.5b). Field-type extensions (`list_record`, `list_fk`) and the `requires_gate` Transition field were moved to Phase 1 — Phase 7 just authors YAML that uses them.
- **Tasks:**
  - 7.1: Author `stores/tasks/schema.yaml`. Concrete shape (follows D7 for capability fields):
    ```yaml
    name: tasks
    id_format: "T{:03d}"
    scope: repo

    fields:
      - {name: title, type: text, required: true, actor: ai_with_human}
      - {name: slug, type: text, required: true, pattern: "^[a-z0-9-]+$", actor: ai_with_human}
      - {name: branch, type: text, required: false}
      - {name: capability, type: text}
      - {name: sub_item, type: text}
      - {name: infra, type: text}
      - {name: depends_on, type: list_fk, ref: tasks}
      - {name: linked_observations, type: list_fk, ref: observations}
      - name: contract
        type: record
        fields:
          - {name: executive_intent, type: text}
          - {name: done_when, type: text, required: true}
          - {name: scope_in, type: text, required: true}
          - {name: scope_out, type: text, required: true}
          - {name: assumptions, type: text}
      - name: plan
        type: record
        fields:
          - {name: objective, type: text}
          - name: phases
            type: list_record
            fields: [{name: name, type: text}, {name: objective, type: text}, {name: tasks, type: list_text}, {name: acceptance_criteria, type: list_text}, {name: files, type: list_text}, {name: dependencies, type: list_text}]
      - name: plan_review_log
        type: list_record
        fields: [{name: gate, type: enum, enum_values: [READY, NEEDS_WORK, NOT_READY]}, {name: summary, type: text}, {name: open_questions, type: list_text}, {name: at, type: timestamp}]
      - name: cycles
        type: list_record
        fields:
          - name: phase
            type: integer
          - name: cycle
            type: integer
          - name: executor
            type: record
            fields: [{name: summary, type: text}, {name: commit, type: text}, {name: files_changed, type: list_text}, {name: notes, type: text}, {name: at, type: timestamp}]
          - name: review
            type: record
            fields: [{name: gate, type: enum, enum_values: [PASS, REVISE, FAIL]}, {name: critical, type: integer}, {name: major, type: integer}, {name: minor, type: integer}, {name: summary, type: text}, {name: details, type: text}, {name: at, type: timestamp}]
      - {name: current_phase, type: integer, actor: framework, auto_increment: true}
      - {name: current_cycle, type: integer, actor: framework, auto_increment: true, auto_increment_within: current_phase}
      - {name: claimed_by, type: text, actor: framework}
      - {name: claimed_at, type: timestamp, actor: framework}
      - {name: blocked_reason, type: text}

    lifecycle:
      states: [planning, plan_review, ready, executing, code_review, blocked, complete]
      initial_state: planning
      transitions:
        # planning → plan_review (submit-plan)
        - {from: planning, to: plan_review, verb: submit-plan, actor: ai_autonomous}

        # plan_review → ready / planning / blocked (gate-keyed by submit-plan-review)
        - {from: plan_review, to: ready, verb: submit-plan-review, actor: ai_autonomous, requires_gate: READY}
        - {from: plan_review, to: planning, verb: submit-plan-review, actor: ai_autonomous, requires_gate: NEEDS_WORK, guard: "plan_review_log.length < 3"}
        - {from: plan_review, to: blocked, verb: submit-plan-review, actor: ai_autonomous, requires_gate: NEEDS_WORK}  # falls through when above guard fails (4th NEEDS_WORK)
        - {from: plan_review, to: blocked, verb: submit-plan-review, actor: ai_autonomous, requires_gate: NOT_READY}

        # ready → executing (framework, fired on entry via on_state.ready)
        - {from: ready, to: executing, verb: start, actor: framework}

        # executing → code_review (submit-execute)
        - {from: executing, to: code_review, verb: submit-execute, actor: ai_autonomous}

        # code_review → executing / complete / blocked (gate-keyed; PASS branches by current_phase guard per M9)
        # PASS-non-last: bump current_phase, reset current_cycle, return to executing
        - {from: code_review, to: executing, verb: submit-review, actor: ai_autonomous, requires_gate: PASS, guard: "current_phase < plan.phases.length"}
        # PASS-last: complete
        - {from: code_review, to: complete, verb: submit-review, actor: ai_autonomous, requires_gate: PASS, guard: "current_phase >= plan.phases.length"}
        # REVISE: bump current_cycle (post-increment guard <= 4 means cycles 2/3/4 OK; the 4th REVISE attempt would push to 5 and fail)
        - {from: code_review, to: executing, verb: submit-review, actor: ai_autonomous, requires_gate: REVISE, guard: "current_cycle <= 4"}
        # REVISE-fallback: 4th-revise routes to blocked (no guard; selected when REVISE→executing's guard fails)
        - {from: code_review, to: blocked, verb: submit-review, actor: ai_autonomous, requires_gate: REVISE}
        # FAIL: code-reviewer hard-fail goes to blocked
        - {from: code_review, to: blocked, verb: submit-review, actor: ai_autonomous, requires_gate: FAIL}

        # blocked → ready (resume)
        - {from: blocked, to: ready, verb: resume, actor: ai_with_human}

    workflow:
      max_revise_cycles: 3
      agent_roles:
        planner: {description: "Creates implementation plans"}
        plan_reviewer: {description: "Reviews plans, READY/NEEDS_WORK/NOT_READY gate"}
        executor: {description: "Executes one phase"}
        code_reviewer: {description: "Reviews phase output, PASS/REVISE/FAIL gate"}
      briefing_templates:
        planner: "templates/planner-brief.md.tpl"
        plan_reviewer: "templates/plan-reviewer-brief.md.tpl"
        executor: "templates/executor-brief.md.tpl"
        code_reviewer: "templates/code-reviewer-brief.md.tpl"
      render_template: "templates/main.md.tpl"
      render_target_path: "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md"
      submit_targets:
        submit-plan: plan
        submit-plan-review: plan_review_log
        submit-execute: cycles
        submit-review: cycles
      on_state:
        planning: [DispatchAgent(planner)]
        plan_review: [DispatchAgent(plan_reviewer)]
        ready: [TransitionTo(executing)]                # framework-fired immediately on entry
        executing: [DispatchAgent(executor)]
        code_review: [DispatchAgent(code_reviewer)]
        blocked: []                                      # human input required
        complete: []
    ```
    Notes on the gate-bearing transitions:
    - `submit-plan-review` appears 4× (target: ready / planning / blocked-from-NEEDS_WORK-guard-fail / blocked-from-NOT_READY).
    - `submit-review` appears 5× (PASS-non-last / PASS-last / REVISE-allowed / REVISE-blocked-fallback / FAIL).
    - Engine selection per 5.5b: filter by `(from, verb, requires_gate)`, then by guard satisfaction. Exactly one survives by construction.
    - `current_phase < plan.phases.length` and `current_phase >= plan.phases.length` partition the PASS space — no overlap, no gap, exactly one fires.
    - `current_cycle <= 4` is the post-increment 4th-revise guard (per C2 / 5.5; allows cycles 2/3/4, blocks at would-be 5).
    - `plan_review_log.length < 3` is the 4th-NEEDS_WORK plan-review guard (per A11).
  - **7.2 (was: add requires_gate to Transition; now done in Phase 1.10).** This bullet is removed from Phase 7 — `requires_gate` lands in Phase 1 alongside the other Transition extensions.
  - **7.3 (was: add list_fk/list_text/list_record; now done in Phase 1.7/1.8).** This bullet is removed from Phase 7 — list_record / list_fk / list_text land in Phase 1. Confirm `list_text` already works in v0.1 (it's `List(Text)`); confirm `list_record` and `list_fk` are available from Phase 1.7/1.8 before authoring `tasks` schema. Storage: TEXT JSON columns.
  - 7.4: Author the four briefing templates at `stores/tasks/templates/`:
    - `planner-brief.md.tpl` — task title, contract, prior plan reviews (if any), file pointers (the read-first context list this task uses for itself, parameterised over the task row).
    - `plan-reviewer-brief.md.tpl` — task title, contract, current plan, prior plan reviews.
    - `executor-brief.md.tpl` — task title, DONE_WHEN, **the current phase only** (not the entire plan), prior code reviews of the same phase.
    - `code-reviewer-brief.md.tpl` — task title, DONE_WHEN, current phase, executor's submit output, files changed, prior cycle's review (for revise cycles).
    Mine the existing agent personas at `~/repos/plugins/task-workflow-plugin/agents/{planner,plan-reviewer,executor,code-reviewer}.md` for the prose conventions. Each template is ~50–100 lines.
  - 7.5: Author `stores/tasks/templates/main.md.tpl` mirroring the canonical layout from `~/repos/plugins/task-workflow-plugin/templates/main.md`. Sections: Meta, Task, Plan, Plan Review, Execution Log, Code Review Log, Completion. Each section pulls from a DB-backed field. Empty list-records render as `_<placeholder>_`. Idempotency requirement: render N times without DB change → byte-identical output.
  - 7.6: Add `tasks` to `BUNDLED_STORE_NAMES` and `BUNDLED_STORE_SCHEMAS` in `src/cli/dynamic.rs`. Bundled templates loaded similarly: `BUNDLED_STORE_TEMPLATES: &[(&str, &[(&str, &str)])]` mapping store name → list of (template-relative-path, content). At install time, the workflow loader prefers the bundled in-memory map for bundled stores.
  - 7.7: Author `stores/tasks/README.md` (terse — 30 lines max — mirrors the layout of `stores/observations/README.md`).
- **Acceptance Criteria:**
  - AC7.1: `stores install tasks` (bundled-name shortcut) succeeds; `tasks` table appears in the DB; manifest gains an entry.
  - AC7.2: `stores tasks add --title "Test" --slug "test-task" --done-when "X works" --scope-in "..." --scope-out "..."` returns `T001`; row reads back with `status: planning, current_phase: 0, current_cycle: 0`.
  - AC7.3: A schema validation pass on the bundled YAML succeeds (no parse errors, no missing-template errors, no missing-state errors, no missing-agent-role errors).
  - AC7.4: All four briefing templates render successfully on a fixture row (no missing keys, no empty critical sections — title/done_when always present).
  - AC7.5: The `current_phase` and `current_cycle` fields cannot be set via `add` or `update` by a non-framework invoker (actor: framework enforced).
- **Files:**
  - `stores/tasks/schema.yaml` — new
  - `stores/tasks/templates/{planner,plan-reviewer,executor,code-reviewer}-brief.md.tpl` — new
  - `stores/tasks/templates/main.md.tpl` — new
  - `stores/tasks/README.md` — new
  - `src/cli/dynamic.rs` — register tasks in BUNDLED_STORE_NAMES + bundled-templates map
  - `src/schema/lifecycle.rs` — `requires_gate` on Transition
- **Dependencies:** Phases 1–6 (uses every feature).

#### Phase 8: Bundled `tasks:start` orchestrator skill

- **Objective:** Ship a Claude Code skill that an LLM-orchestrator can invoke to drive a real task end-to-end via the new CLI verbs only. Replaces the pi-extension's TS engine for the limited-scope tasks store. The skill prose calls `stores tasks next-action`, `brief`, `submit-execute`, `submit-review`, `submit-plan`, `submit-plan-review`, `render` — nothing else.
- **Tasks:**
  - 8.1: Author `skills/tasks:start/SKILL.md` with the orchestrator workflow:
    1. `stores tasks add ...` to create the row OR accept an existing `<id>`.
    2. Loop: `stores tasks next-action <id> --json` → parse `next_agent` + `blocked` + `current_phase` + `current_cycle`.
    3. Branch: if blocked, surface the blocked_reason and stop. Otherwise, `stores tasks brief <id>` to get the agent's briefing markdown. Spawn the appropriate Task subagent with the briefing as input.
    4. Read the subagent's structured output, translate to `submit-*` flags, run `stores tasks submit-<verb> <id> ...`.
    5. After every submit, run `stores tasks render <id>` so main.md stays current.
    6. Continue until status is `complete` or `blocked`.
    The skill matches the persona conventions of `~/repos/plugins/task-workflow-plugin/skills/start/SKILL.md` but cuts everything below Stage 5 (Stage 6 CodeRabbit, Stage 7 Completion belong to a separate `tasks:wrap` skill outside this task's scope).
  - 8.2: The skill's frontmatter declares `requires_stores: [tasks]` (the v0.2 framework will read this at load time per the v0.2 handoff conventions; for v0.2 the field is a documentation hint — no automated enforcement yet, but mark it so future framework versions can verify).
  - 8.3: Add `tasks:start` to `BUNDLED_SKILLS` in `src/cli/skills.rs`. Test that `stores skills install tasks:start` writes the SKILL.md correctly.
  - 8.4: Update the v0.1 `task:next` skill draft (`skills/task:next/SKILL.md`) to point at the new tasks store (it was a stub against the un-built tasks store; now tasks exists). Verb usage: `stores tasks list --status ready --limit 1 --sort updated_at` to find the next ready task; `stores tasks start <id>` to invoke `tasks:start`.
- **Acceptance Criteria:**
  - AC8.1: `stores skills list` includes `tasks:start`. `stores skills install tasks:start` writes `.claude/skills/tasks:start/SKILL.md` byte-identical to the bundled file.
  - AC8.2: The skill prose mentions ONLY: `stores tasks next-action`, `brief`, `submit-execute`, `submit-review`, `submit-plan`, `submit-plan-review`, `render`, `add`, `list`, `show`. Verified by grep against the skill file.
  - AC8.3: The skill markdown parses as valid YAML frontmatter + body (`yq` round-trips frontmatter).
  - AC8.4: The skill mentions `DONE_WHEN propagation rule` (every spawned subagent prompt includes the DONE_WHEN verbatim — same as the source plugin's start skill).
- **Files:**
  - `skills/tasks:start/SKILL.md` — new
  - `skills/task:next/SKILL.md` — update to point at tasks
  - `src/cli/skills.rs` — register tasks:start
- **Dependencies:** Phase 7 (the store the skill targets must exist).

#### Phase 9: Smoke test — `observations` lifecycle expansion + real T3 task end-to-end

- **Objective:** Two sub-goals. (a) Expand `observations` lifecycle to 10.06's full set (D4) — pure schema change exercising the existing CLI surface, no engine surface. (b) Drive one real T3 task through the whole new workflow via `stores tasks` CLI only — proves DONE_WHEN.
- **Tasks:**
  - 9.1: Edit `stores/observations/schema.yaml`: add states `investigating, confirmed, needs_info, in_progress` to `lifecycle.states` and the corresponding transitions. Specifically:
    - `triaged → investigating` (verb: investigate, actor: ai_with_human)
    - `investigating → confirmed` (verb: confirm, actor: ai_with_human)
    - `investigating → needs_info` (verb: request_info, actor: ai_autonomous)
    - `needs_info → confirmed` (verb: provide_info, actor: human)
    - `confirmed → in_progress` (verb: claim, actor: ai_autonomous)
    - `in_progress → resolved` (verb: resolve, actor: ai_autonomous) — replaces the v0.1 direct `triaged → resolved` (the existing transition stays for backward compat in tests, but we add the longer path).
    - `confirmed → wont_fix` (verb: wont_fix, actor: ai_with_human)
    Update `tests/e2e.sh` to optionally walk the longer lifecycle path on a second observation row (existing OBS001 path stays unchanged for compat).
  - 9.2: Add the `priority` field (D? not locked yet — see Open Questions Q4) — actually **adjacent fix from Intent Contract**: add `priority: enum [high, normal, low]` to `gate` schema only. Observations doesn't get it (the v0.2 handoff suggested adding to both, but the locked Intent Contract says the priority adjacent fix is on the gate schema for "decision/script gate items need priority for /task:open's 'Blake-only-step' filing pattern" — it does NOT mention observations). Confirm in Open Questions.
  - 9.3: End-to-end test as a new `tests/tasks_e2e.sh` (don't bloat the existing e2e.sh). Steps:
    1. `cargo install --path .`; `mktemp -d`; `cd $TMP`; `git init` (so `scope: repo` resolves).
    2. `stores init`; `stores install observations`; `stores install gate`; `stores install tasks`.
    3. `stores tasks add --title "Smoke test task" --slug "smoke-test" --capability "test" --done-when "smoke passes" --scope-in "x" --scope-out "y"` → `T001`.
    4. `stores tasks next-action T001 --json` → assert `next_agent: planner`, `current_phase: 0`, `status: planning`.
    5. `stores tasks brief T001 --for planner` → assert output is non-empty markdown containing the title.
    6. `stores tasks submit-plan T001 --plan-from-file <fixture-plan.md>` → assert status: plan_review.
    7. `stores tasks submit-plan-review T001 --gate READY --summary "ok"` → assert status: ready, then immediately framework-fires `ready → executing`, current_phase: 1.
    8. `stores tasks submit-execute T001 --summary "phase1 done" --commit abc --files-changed "src/foo.rs"` → assert status: code_review.
    9. `stores tasks submit-review T001 --gate REVISE --critical 1 --major 0 --minor 0 --summary "..."` → assert status: executing, current_cycle: 1.
    10. Repeat steps 8+9 two more times to hit cycle 3.
    11. 4th submit-review with REVISE → assert exit code non-zero AND status: blocked AND error mentions the guard `current_cycle <= 4` AND `blocked_reason` field is populated with the phase/cycle context.
    12. **(M10 fix): Recovery via `resume`.**
        - 12a. `stores tasks resume T001` → assert status: executing (the resume verb triggers `blocked → ready` and the engine immediately fires `ready → executing` per `on_state.ready: [TransitionTo(executing)]`).
        - 12b. Assert `current_cycle == 1` (reset by the engine's resume post-action per 5.4).
        - 12c. Assert `current_phase == 1` (UNCHANGED — resume returns to the blocked phase, not to phase 1; here phase was already 1, so this also asserts non-mutation).
        - 12d. Assert the `cycles` list still has its 4 prior entries (audit trail preserved per M10 / 5.4).
        - 12e. `stores tasks submit-execute T001 ...` then `stores tasks submit-review T001 --gate PASS --critical 0 --major 0 --minor 0 --summary "..."` for phase 1 → assert status: executing, current_phase: 2, current_cycle: 1 (PASS-non-last, M9 / 5.5b dispatched the `→ executing` transition with the `current_phase < plan.phases.length` guard).
    13. Submit-execute + submit-review --gate PASS for phase 2 → assert status: complete (PASS-last, M9 / 5.5b dispatched the `→ complete` transition with the `current_phase >= plan.phases.length` guard).
    14. `stores tasks render T001` → asserts file `tasks/completed/T001-smoke-test/main.md` exists, byte-identical between two render calls.
    15. `cat .stores/db.sqlite | sqlite3 .stores/db.sqlite "select status, current_phase from tasks"` → final state assertion.
    16. **(NEW per AC5.11 / M11): atomic-boundary harness.** The Rust integration test side of the e2e — the bash script doesn't easily simulate panics — covers AC5.11/AC5.13/AC5.14 in `tests/submit_atomicity.rs`. Bash e2e references the Rust suite by running `cargo test --test submit_atomicity` once before the bash steps.
  - 9.4: Add `bash tests/tasks_e2e.sh` to the `Makefile`/CI surface (or just to the README's "How to test" section if there's no CI yet — there isn't, per v0.1).
- **Acceptance Criteria:**
  - AC9.1: All 110 + Phase-1-through-8 unit tests still pass.
  - AC9.2: `bash tests/e2e.sh` (the original) still exits 0 byte-identically (observations + gate behaviour preserved aside from the added longer-path transitions).
  - AC9.3: `bash tests/tasks_e2e.sh` exits 0 from a fresh tmp dir **in under 30 seconds** (m5: perf cap; the original e2e walks 13 steps in <2s, the workflow e2e shells out 15+ times so cap to 30s with `timeout 30 bash tests/tasks_e2e.sh`).
  - AC9.4: The 4th-revise smoke step (9.3 step 11) emits an error mentioning "guard" and "current_cycle <= 4" and the row's status reads back as `blocked`.
  - AC9.5: The two `render T001` calls in step 14 produce byte-identical files (`diff` returns empty).
  - AC9.6: The end-to-end run NEVER calls a CLI verb outside the contract DONE_WHEN's allowed set (`next-action`, `brief`, `submit-*`, `render`, plus `add`/`list`/`show` which are framework basics). Verified by grepping the test script.
- **Files:**
  - `stores/observations/schema.yaml` — lifecycle expansion
  - `stores/gate/schema.yaml` — priority field
  - `tests/tasks_e2e.sh` — new
  - `tests/fixtures/smoke_plan.md` — the fixture plan submitted in step 6
- **Dependencies:** Phases 1–8.

#### Phase 10: Documentation update + e2e integration + version bump

- **Objective:** Update the README, the v0.2 handoff doc, the bundled-stores listing, and version metadata. Bump test count expectation. No code changes; pure docs.
- **Tasks:**
  - 10.1: Update `README.md` to add a third install step (`stores install tasks`) and a "Workflow stores" section pointing at `stores tasks --help` and the bundled `tasks:start` skill. Keep the existing 13-step demo path byte-identical (it stays correct).
  - 10.2: Update `docs/handoff-v0.2.md` to mark Section 2 (`tasks` store) as DELIVERED. Add a "v0.3 candidates" section with the deferred items from the Intent Contract Out section (phase-reviewer, merge-reviewer, runs store, etc.).
  - 10.3: Bump `Cargo.toml` version to `0.2.0`.
  - 10.4: Update test count expectation in the v0.2 handoff (110 v0.1 + ~80–120 new tests across phases 1–9, depending on final density).
  - 10.5: Author a short `stores/tasks/README.md` if not already done in Phase 7 (5–10 lines: what it is, how to install, link to `tasks:start`).
  - 10.6: Self-host check: this T002 task itself (the legacy filesystem one) is NOT migrated — per D5, T001+T002 stay legacy filesystem-only. T003+ are DB rows. Add a one-paragraph note in the v0.2 handoff explaining the boundary so future agents don't try to import legacy tasks/.
- **Acceptance Criteria:**
  - AC10.1: README's 13-step demo still passes (`bash tests/e2e.sh` exit 0).
  - AC10.2: Cargo version is `0.2.0`.
  - AC10.3: `docs/handoff-v0.2.md` mentions `tasks` as DELIVERED.
  - AC10.4: README has a "Workflow stores" section.
- **Files:**
  - `README.md`
  - `docs/handoff-v0.2.md`
  - `Cargo.toml`
- **Dependencies:** Phase 9 (the e2e test must exist + pass before we mark anything DELIVERED).

---

### Decision Matrix

#### Decisions Made (already ratified — see Intent Contract)

| # | Decision | Locked value | Source |
|---|----------|--------------|--------|
| 1 | Architecture: DB-as-truth + workflow engine in framework | β | Intent Contract D1 |
| 2 | Workflow opt-in mechanism | Explicit `workflow:` block in schema | Intent Contract D2 |
| 3 | Brief/submit CLI shape | Split verbs, fixed flags per verb | Intent Contract D3 |
| 4 | Smoke-test target task | Expand `observations` lifecycle | Intent Contract D4 |
| 5 | ID prefix for stores-tasks rows | `T{:03d}` shared with filesystem | Intent Contract D5 |
| 6 | `guard:` expression scope | Eq/Neq + `.length` ops only | Intent Contract D6 |
| 7 | Capability fields on bundled tasks schema | Bake in as optional | Intent Contract D7 |
| 8 | Concurrency lock | `claimed_by`/`claimed_at` 5-min timeout | Intent Contract D8 |

#### Decisions Made (autonomous, this plan)

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| A1 | Template engine | `handlebars` v5 | Supports `{{#each}}` for list-records (executor cycles, plan phases) and `{{#if (eq …)}}` for conditional sections. `tinytemplate` is smaller but lacks list iteration helpers; `tera` is heavier and brings async deps; `liquid` is comparable but less common in Rust. Handlebars binary-size cost is ~60KB compiled — acceptable. |
| A2 | Phase boundary at briefing-template authoring | Author the four templates inside Phase 7, NOT a separate Phase | The templates can't be tested in isolation — they need a real fixture row, which only exists once the schema is authored. Bundling them keeps the cycle tight: schema + templates + bundled-store entry land together in one phase. |
| A3 | `Op::Submit*` variants vs reusing `Op::Transition` | New variants | The submit verbs carry semantic information (gate decision, list-record append target) that `Op::Transition` doesn't model cleanly. Adding variants is a precedent already set by T001 cycle 2 (`Op::TransitionWithDiff`). The validator's match arms are short. |
| A4 | Two-write atomicity boundary | Single `conn.unchecked_transaction()` per submit verb | SQLite's WAL plus an explicit transaction is sufficient. Phase 5.3 wraps the DB submit + engine post-actions (current_phase++/current_cycle reset/follow-on transition) in one transaction. Either both writes commit or neither does. The render call is a *separate* command run by the orchestrator after submit returns success — failure of render does not corrupt DB state, only main.md. |
| A5 | Engine-fired transitions: how the `framework` actor is set | Programmatic `Actor::Framework` passed to validator | `Actor::from_env()` never returns `Framework`; only the engine itself constructs it when firing follow-on transitions inside a submit handler's transaction. Users cannot pass `--invoker framework` (rejected at CLI parse). |
| A6 | `auto_increment_within` semantics | Schema declares the dependency; engine resets the inner counter on outer increment | Same as the legacy plugin's `current_cycle` reset behaviour. The schema attribute is descriptive (declares the "within" relationship); the imperative reset logic lives in the submit handler's engine post-actions. |
| A7 | Render verb directory move | Render moves the on-disk directory before writing main.md | The legacy convention moves task dirs as `tasks/active/` ↔ `tasks/completed/` ↔ `tasks/paused/` (per `tasks/CLAUDE.md`). Render is the only write-side actor for main.md, so it owns the move. Detection: glob `tasks/*/{{display_id}}-*` matches one entry → move it. Multiple matches → log warning, render to canonical path without moving. Zero matches → just write to canonical. |
| A8 | Where briefing templates live in the bundled-store package | `stores/tasks/templates/<role>-brief.md.tpl` | Mirrors the existing convention of `stores/<name>/{schema.yaml, README.md}`. Templates are part of the store package, embedded via `include_str!` for bundled stores. |
| A9 | `submit-plan-review` and `submit-review` mapping multiple gate values to multiple transitions | New `requires_gate: Option<String>` on `Transition` + engine selection logic | The simplest extension. The alternative (one super-transition that takes a `gate_to_target_state: BTreeMap` would be a bigger schema change). With `requires_gate`, multiple transitions share `(from, verb)` and the engine picks by gate; if no transition matches, error. |
| A10 | List-record CLI surface | `--<flag>-from-file <path>` accepting JSON for `submit-plan` (the plan's full record); fixed flags for the simpler submits | The plan record is too nested for flat flags (phases is a list of records each with several lists). `--plan-from-file plan.json` is the cleanest input shape. The simpler submits (`submit-execute`, `submit-review`) get one flag per leaf because each cycle's executor + review records have a fixed flat shape. |
| A11 | What happens when `submit-plan-review` fires `plan_review → planning` (revise) | Same row, current_phase stays 0, plan_review_log gets a new entry | The plan_review_log accumulates each iteration's review. The next planner spawn's briefing includes all prior reviews. After 3 NEEDS_WORK, the guard `plan_review_log.length < 3` fails and the engine fires `plan_review → blocked` instead. |
| A12 | Briefing template rendering: per-section vs whole-template | Whole-template per agent role | Each agent gets one template that consumes the whole row context. The template authors decide what to surface. Simpler than per-section partial rendering and easier to iterate on briefing quality. |
| A13 | `flatten.rs::leaf_args` behaviour for workflow stores | Skip leaf_args entirely for workflow-shaped stores; the submit-* verbs have their own fixed flag sets | Workflow stores don't expose flat flags for their sub-document arrays; the submit verbs are the only write path. v0.1 stores keep current behaviour. |
| A14 | `requires_stores` skill metadata | Document in skill frontmatter; framework reads it but does NOT enforce in v0.2 | Per v0.2 handoff line 91 — enforcement is deferred. Documentation hint only for now. |
| A15 | The `Op::Submit*` actor scoping rule | Same as `Op::Transition`: actor checks scoped to diff only | Avoids the same regression that T001 phase 7 hit (actor errors firing on unrelated fields read from the merged entry). |
| A16 | Tests for submit-* with a partially-populated row | Mock-fixture approach: build rows by direct INSERT into in-memory SQLite, not by repeated `add`+`update` chains | Faster per-test setup, mirrors the existing `transition.rs` test pattern. |
| A17 | Whether `tests/e2e.sh` should exercise the new tasks workflow | No — keep it focused on the v0.1 13-step demo. New `tests/tasks_e2e.sh` is the home for the workflow walk | The original e2e is the v0.1 contract; we don't want to grow it indefinitely. Tasks e2e is its own script. |
| A18 | `start` verb (`ready → executing`) | Framework-only verb, NOT a CLI subcommand. The engine fires it when entering `ready` | The `on_state.ready: [TransitionTo(executing)]` action makes `ready` a transient state — the engine fires the transition immediately on entry, no orchestrator action needed. Keeps the orchestrator's CLI surface minimal. |
| **A19 (cycle 2)** | Render dir-move synchronous in `render` vs. explicit `promote` verb (Q2 closed) | Synchronous in `render` | Plan-review noted the planner had recommended A and the failure mode (dir-move race) is bounded by render being idempotent and recoverable. Closing Q2 as A; documenting in Phase 6.3. |
| **A20 (cycle 2)** | Briefing template structure: shared partial vs self-contained (Q4 closed) | Self-contained per-agent templates | Pure DRY-vs-locality tradeoff. The four briefings duplicate ~20 lines of context preamble; deduping via Handlebars partials adds template-loading complexity (must register all partials at engine init). For v0.2, locality wins. Revisit if templates grow past ~150 lines each. |
| **A21 (cycle 2)** | Engine writes `blocked_reason` on auto-block (Q5 closed) | Engine writes context-rich `blocked_reason` | DONE_WHEN says "status auto-set to BLOCKED"; the human reading the blocked task needs context to unblock — option B (silent NULL) leaves them with a mystery. The exact format is in Phase 5.4's post-action table: `"4th revise rejected by guard current_cycle <= 4 on phase {N} cycle {M}: <last-review summary>"` for guard-fail; `"plan-reviewer marked NOT_READY: <summary>"` for NOT_READY; etc. |
| **A22 (cycle 2)** | NEEDS_WORK plan-review cycles vs blocks (Q6 closed) | Auto-cycle to planning (with `plan_review_log.length < 3` guard for 4th-attempt block) | Plan-review noted Q6 contradicted itself: AC5.8 already specifies the cycle behavior. The Phase 7 schema declares the cycle and the guard. Q6 closed as A; the previous "OPEN" was a planner oversight. |
| **A23 (cycle 2)** | Phase 9's e2e as bash vs Rust (Q8 closed) | Bash, mirroring `tests/e2e.sh` | Pure consistency-vs-tooling tradeoff. v0.1's D17 (in T001) set the bash precedent; follow it. The atomic-boundary tests (AC5.11/AC5.13/AC5.14) live in `tests/submit_atomicity.rs` (Rust integration test invoked from the bash e2e) because panic injection isn't sensibly bash-shaped. |
| **A24 (cycle 2)** | 4th-REVISE guard expression form (C2) | `current_cycle <= 4`, evaluated AFTER the engine bumps `current_cycle` for this submit | The original `cycles.length < 3` was wrong (cumulative counter, off-by-one). `current_cycle` is the existing per-phase counter declared with `auto_increment_within: current_phase`, initial value 1. Post-increment-then-guard ordering with `<= 4` gives the intended "3 revise cycles allowed; 4th rejected" semantics: cycles 2, 3, 4 of execution proceed (1st/2nd/3rd revise); the 4th REVISE attempt would push to cycle 5 and fails. (The cycle-2 first-pass mistakenly wrote `<= 3` allowing only 2 REVISEs; corrected during the worked-example transcript audit. See Phase 5.5 for the corrected table.) |
| **A25 (cycle 2)** | Two PASS transitions on `code_review` disambiguated (M9) | Two transitions with explicit guards: `current_phase < plan.phases.length` (→ executing) and `current_phase >= plan.phases.length` (→ complete) | Schema-declared partition is preferable to imperative engine logic because (a) the schema is the source of truth and the lifecycle is auditable end-to-end, (b) the guard expression language already supports the path required (per D6 + Phase 1.4's evaluator), (c) future schema authors can override "what counts as last phase" without touching engine code. Tradeoff: more transitions in the YAML; mitigated by clear comments. |
| **A26 (cycle 2)** | Transaction boundary plumbing (C3) | Split `transition::run` into `run` (opens own tx) and `run_in_tx(tx, ...)` (uses caller-supplied tx) | The submit handler holds the outer transaction; follow-on transitions reuse the same tx by calling `run_in_tx`. Existing call sites of `run(...)` continue to work; existing tests get a one-line wrapper. The lock release is the FINAL action inside the tx (M11). |
| **A27 (cycle 2)** | Submit handler does NOT call `next-action` (C4) | Drop the dependency entirely | The validator's actor model is the invariant enforcement. `next-action` is a read primitive for the orchestrator. The original Phase 5 dependency was incoherent. |
| **A28 (cycle 2)** | `current_cycle` initial value | Initializes to `1` on `add` and on phase advance (PASS-non-last) | Makes "cycle 1" the first execute attempt; "cycle 2" the first revise; etc. Simpler human reading vs the alternative `0` initialization. The post-increment guard `<= 3` is calibrated against this initial value. |
| **A29 (cycle 2)** | Phase 1 size growth | Accept as-is for cycle 2; mark for split into 1a/1b if cycle 3 review judges too large | Phase 1 grew from 6 tasks to 11 tasks (~700-900 LOC) by absorbing list_record/list_fk/requires_gate. Splitting now risks cross-phase-dependency hazards exactly like the ones cycle 1 surfaced; deferring the split decision to the executor or the next plan review. |

#### Open Questions (Need Human Input)

| # | Question | Options | Impact | Resolution |
|---|----------|---------|--------|------------|
| Q1 | Should the `priority` field adjacent fix be on **just `gate`**, or **`gate` + `observations` + `tasks`**? Intent Contract says `gate` only ("decision/script gate items need priority for /task:open's 'Blake-only-step' filing pattern"). The v0.2 handoff §1c mentions both observations and gate. Clarify scope. | A) gate only (per Intent Contract literal reading). B) gate + observations + tasks (broader; cheap). C) gate + tasks (no obs; tasks rows are the actual prioritization unit). | Affects 3 schemas; each addition is ~10 LOC YAML. Doesn't touch engine code. | OPEN |
| Q3 | The `requires_gate` Transition extension (A9 / Phase 1.10): is it acceptable as a schema-level concept, or does the reviewer prefer a different selection mechanism (e.g. a `transitions` map keyed by `(from, verb, gate)` instead of a list)? | A) Add `requires_gate: Option<String>` field, list-of-transitions stays (this plan). B) Restructure transitions as a map. C) Inline gate-→-target mapping inside one transition record. | A is the smallest delta; B is cleaner; C is least flexible. Plan-review marked this "borderline; A is fine if no opinion." | OPEN — default A unless user objects |
| Q7 | `scope: repo` resolution: should `git rev-parse --git-common-dir` failures (outside a git repo) **fall back to `Worktree`** or **error hard**? | A) Fall back to Worktree. B) Hard error: "scope: repo requires a git repo." | A is more permissive (test environments without git can still install). B is more honest. The smoke test creates a git repo explicitly, so either works. | OPEN — recommend B |
| **Q-NEW-1 (cycle 2)** | Legacy T001/T002 (filesystem-only) vs DB-backed `tasks list` UX. When the orchestrator skill runs `stores tasks list --status ready`, the legacy filesystem T001/T002 don't show up (per D5). Is that the desired UX (clean break, downstream skills only see DB-backed tasks), or do we add a "filesystem-imported placeholder" row? Surface to user. | A) Clean break: `tasks list` shows only DB-backed rows (T003+); legacy T001/T002 stay invisible to the new CLI. Downstream `task:next` and similar skills must check both sources or accept the clean break. B) Placeholder rows: import T001/T002 as DB rows with `status: complete, frozen: true` so they show in `list` for context. C) Document as a known boundary; defer placeholder work to v0.3 if needed. | Affects how Phase 8's `task:next` skill UPDATE handles "next ready task" queries. Plan-review predicted this would matter for downstream skills. | OPEN — recommend C (clean break in v0.2; document boundary; revisit if user feedback flags it) |

---

### Risks called out (executor-relevant)

| # | Risk | Mitigation |
|---|------|------------|
| R1 | Briefing-template quality dominates LLM output quality. Bad templates → bad agent output regardless of schema enforcement. | Smoke test (Phase 9) surfaces gaps. Iterate template prose during plan-review and code-review cycles. The four templates are ~50–100 lines each; revising one is a 30-min loop. |
| R2 | Two-write boundary in submit handlers (DB submit + engine post-actions) — partial failures could leave a row mid-transition. | Single SQLite transaction wraps both writes (A4). Rollback on any error. The `render` step is *separate* and idempotent; corrupted main.md is recoverable. |
| R3 | Guard expression evaluator scope creep — temptation to add AND/OR. | Locked at D6: equality + length ops only. The parser rejects unsupported tokens with the same heuristic the v0.1 `required_when` parser uses. ~150 LOC max. Plan-reviewer should reject any AND/OR addition. |
| R4 | `git rev-parse --git-common-dir` shells out — adds a process spawn per CLI call for `scope: repo` stores. | Cache the resolved path per process invocation. For batch-y scripts, this is one-shot; the cost is negligible. |
| R5 | `claimed_by`/`claimed_at` lock could deadlock if a submit handler crashes mid-transaction. | The 5-minute auto-expiry handles this. Worst case: 5min wait before retry. The expiry is a UPDATE conditional, not a sleep. |
| R6 | Template engine (`handlebars`) crate adds a new dep; size + audit surface. | ~60KB compiled overhead. The crate is widely used (>1M downloads/month), BSD-3, maintained. Vendoring is overkill. |
| R7 | `flatten.rs::leaf_args` skipping for workflow stores changes the CLI surface for any future store that mixes workflow + flat flags. | This plan asserts: workflow stores are workflow-shaped end-to-end; they do not expose flat flags. If a future store needs both, it's a v0.3 concern. |
| R8 | Test count growing significantly — Phase 1–9 likely adds 80–120 unit tests + integration tests, plus the new e2e script. | Phase 10's docs update bumps the expected count. CI doesn't exist yet (per v0.1), so the contract is "all tests pass via `cargo test`." |
| R9 | The bundled `tasks:start` skill replicates orchestrator logic that exists in `~/repos/plugins/task-workflow-plugin`. Drift risk. | The two skills target different runtime environments (this one is `stores`-CLI-only; the plugin's start uses pi-extension's TS engine). They will drift; that's intentional. The v0.2 handoff already mentions this divergence. |
| R10 | The submit-plan flag (`--plan-from-file plan.json`) accepts JSON, but planner output from a Claude Code subagent is markdown-shaped. Translation must happen in the orchestrator skill. | Phase 8's skill prose is explicit about the planner subagent producing structured-JSON output (matching `~/repos/plugins/task-workflow-plugin/schemas/planner-output.json`), then the orchestrator passes the JSON file to `submit-plan`. The skill includes a one-paragraph "structured output" note. |
| **R11 (cycle 2)** | Phase 1 LOC growth from absorbing list_record/list_fk/requires_gate may exceed the executor's per-phase appetite. | Per A29, the phase is monitored; if cycle-3 plan-review judges it too large, split into 1a (actor + guard + scope + auto_increment) and 1b (list_record + list_fk + requires_gate + read_row depth). |
| **R12 (cycle 2)** | The post-increment guard ordering for `current_cycle` is non-obvious and could be misimplemented as pre-increment. | Phase 5.5 contains an explicit increment-then-guard table. AC5.4 is constructed to fail loudly if the implementation is pre-increment (the 4th-REVISE guard would fail one cycle early — exactly the C2 bug). AC5.4b cross-checks per-phase isolation. |

---

### Worked-example transcript (cycle 2 addition)

The following is a step-by-step transcript of one full `revise → re-execute → 4th-revise → BLOCKED → resume → PASS` flow for a hypothetical T003 smoke-test task with a 2-phase plan. Every CLI invocation is shown with the framework's writes annotated. Lines beginning `#` are explanatory commentary, not actual CLI output.

This transcript exists to make every implicit decision explicit (per the cycle-1 review's recommendation). C2, M8, M9, M10 all fall out of doing this exercise.

```
# === SETUP ===
$ stores tasks add --title "Smoke test" --slug "smoke-test" \
    --capability "test" --done-when "smoke passes" \
    --scope-in "x" --scope-out "y"
T003 created
# Framework writes (all in initial Op::Add tx):
#   status = planning
#   current_phase = 0    (initial; no phase started yet)
#   current_cycle = 1    (initial; per A28, cycles are 1-indexed)
#   claimed_by = NULL
#   claimed_at = NULL

$ stores tasks next-action T003 --json
{
  "id": "T003", "status": "planning", "current_phase": 0, "current_cycle": 1,
  "next_agent": "planner", "blocked": false, "blocked_reason": null,
  "claimed_by": null, "claimed_at": null
}

$ stores tasks brief T003                          # default --for resolves to "planner"
[planner brief markdown — title, contract, file pointers, prior plan reviews (none)]

# Orchestrator (tasks:start skill) spawns planner subagent with the brief; receives plan.json.

$ stores tasks submit-plan T003 --plan-from-file plan.json
Submitted plan for T003; status now: plan_review
# Inside one tx (per Phase 5.3 / Phase 5.4 row "submit-plan"):
#   - Acquire lock (claimed_by = "ai_autonomous", claimed_at = now)
#   - Validator passes Op::SubmitPlan(diff)
#   - Write plan record into plan column (TEXT JSON)
#   - status = plan_review
#   - Release lock (claimed_by = NULL, claimed_at = NULL)
#   - Commit

$ stores tasks brief T003
[plan-reviewer brief — current plan + prior reviews (none) + decision matrix template]

# Orchestrator spawns plan-reviewer; receives gate=READY.

$ stores tasks submit-plan-review T003 --gate READY --summary "approved" \
    --open-questions-from-file -    # stdin: empty
Submitted plan-review for T003; status now: executing
# Inside one tx (Phase 5.4 row "submit-plan-review --gate READY"):
#   - Acquire lock
#   - Validator passes Op::SubmitPlanReview("READY", diff)
#   - Append plan_review_log entry
#   - status = ready  (matches transition with requires_gate: READY)
#   - on_state.ready: [TransitionTo(executing)] fires INSIDE the same tx (M5 / AC5.7):
#       * Engine validates the framework-fired transition (actor: framework)
#       * status = executing
#       * current_phase = 1  (initial → first phase per Phase 5.4 row)
#       * current_cycle = 1  (initial; per A28)
#   - Release lock
#   - Commit
# User saw ONE CLI call but two state changes, both atomic.

$ stores tasks next-action T003 --json
{ "id": "T003", "status": "executing", "current_phase": 1, "current_cycle": 1,
  "next_agent": "executor", "blocked": false, "blocked_reason": null,
  "claimed_by": null, "claimed_at": null }

$ stores tasks brief T003
[executor brief — phase 1 only, no prior reviews (cycle 1 = first attempt)]

# Orchestrator spawns executor; receives execute-output.json.

$ stores tasks submit-execute T003 --summary "phase 1 done" --commit abc1 \
    --files-changed "src/foo.rs"
Submitted execute for T003 phase 1 cycle 1; status now: code_review
# Inside one tx (Phase 5.4 row "submit-execute"):
#   - Acquire lock
#   - Validator passes Op::SubmitExecute(diff)
#   - Append cycles[].executor entry: { phase: 1, cycle: 1, executor: { summary: "phase 1 done", commit: "abc1", ... } }
#   - status = code_review
#   - (current_cycle stays 1 — execute does not bump per the table)
#   - Release lock
#   - Commit

# === REVISE CYCLE 1 (1st REVISE; current_cycle is 1 going in) ===
$ stores tasks brief T003
[code-reviewer brief — cycle 1 executor output + files changed]

# Orchestrator spawns code-reviewer; receives gate=REVISE.

$ stores tasks submit-review T003 --gate REVISE --critical 1 --major 0 --minor 0 \
    --summary "needs work on foo" --details-from-file review1.md
Submitted review for T003 phase 1 cycle 1; status now: executing
# Inside one tx (Phase 5.4 row "submit-review --gate REVISE"):
#   - Acquire lock
#   - Validator passes Op::SubmitReview("REVISE", diff)
#   - Append cycles[idx].review entry
#   - Engine BUMPS current_cycle: 1 → 2  (post-increment, per Phase 5.5)
#   - Engine evaluates guard "current_cycle <= 3" against post-bump value 2: TRUE
#   - status = executing  (matches REVISE transition with guard satisfied)
#   - Release lock
#   - Commit

# === REVISE CYCLE 2 (2nd REVISE) ===
$ stores tasks submit-execute T003 --summary "addressed feedback" --commit abc2 ...
[status: code_review; cycles[].executor for phase 1, cycle 2 written]

$ stores tasks submit-review T003 --gate REVISE ...                # 2nd REVISE
[engine bumps current_cycle: 2 → 3; guard "current_cycle <= 3" → TRUE; status: executing]

# === REVISE CYCLE 3 (3rd REVISE) ===
$ stores tasks submit-execute T003 ...
[status: code_review; cycle 3]

$ stores tasks submit-review T003 --gate REVISE ...                # 3rd REVISE
[engine bumps current_cycle: 3 → 4; guard "current_cycle <= 3" → FALSE]
# Wait — the 3rd REVISE bumps to 4 and the guard fails on 4 <= 3? Let's re-check.
#
# Actually no: per the Phase 5.5 trace table, the 3rd REVISE attempt bumps current_cycle
# from 3 to 4 and the guard "current_cycle <= 3" evaluates against post-bump value 4 — FALSE.
# This would route to BLOCKED on the 3rd REVISE, which is one cycle too early.
#
# CORRECTION (cycle 2 self-audit): The post-increment guard `<= 3` allows up to 3 increments
# from the initial value of 1: 1 → 2 (1st REVISE), 2 → 3 (2nd REVISE), then a 3rd REVISE
# would be 3 → 4 which fails the guard. That's only TWO REVISEs allowed.
#
# To allow THREE REVISEs (matching DONE_WHEN's "3-cycle REVISE limit; 4th attempt routes
# to BLOCKED"), the guard should be `current_cycle <= 4` post-increment, OR the initial
# value should be 0 (so current_cycle is 0 before any execute, becomes 1 on first REVISE
# bump, etc. — but that conflicts with A28).
#
# RESOLVED (cycle 2): The guard expression in the schema is updated to
# `current_cycle <= 4`. Phase 7's transitions block reflects this. The semantics:
#   - Initial current_cycle = 1
#   - 1st execute proceeds (cycle 1 of execution)
#   - 1st REVISE: bump 1 → 2; guard 2 <= 4 TRUE; cycle 2 begins
#   - 2nd REVISE: bump 2 → 3; guard 3 <= 4 TRUE; cycle 3 begins
#   - 3rd REVISE: bump 3 → 4; guard 4 <= 4 TRUE; cycle 4 begins
#   - 4th REVISE: bump 4 → 5; guard 5 <= 4 FALSE; routes to BLOCKED
# This gives the user three full REVISE cycles before the 4th attempt is rejected.
# Phase 5.5's table and Phase 7's transitions YAML are corrected accordingly below this transcript.

# (Continuing the transcript with the corrected guard `current_cycle <= 4`:)

[engine bumps current_cycle: 3 → 4; guard "current_cycle <= 4" → TRUE; status: executing; cycle 4 begins]

# === ATTEMPTED 4th REVISE (current_cycle is 4 going in) ===
$ stores tasks submit-execute T003 ...                             # cycle 4 work
[status: code_review; cycles[].executor for phase 1, cycle 4 written]

$ stores tasks submit-review T003 --gate REVISE \
    --critical 1 --major 0 --minor 0 \
    --summary "still broken"                                       # 4th REVISE attempt
ERROR: row T003 routed to BLOCKED on 4th REVISE attempt of phase 1
       guard: current_cycle <= 4 (would be 5)
       blocked_reason auto-populated; resume via 'stores tasks resume T003'
# Inside one tx:
#   - Acquire lock
#   - Validator passes Op::SubmitReview("REVISE", diff)
#   - Append cycles[].review entry (audit trail; preserves the failing review)
#   - Engine attempts to bump current_cycle: 4 → 5 (working copy)
#   - Engine evaluates guard "current_cycle <= 4" against post-bump value 5: FALSE
#   - Engine looks up alternate transition matching (from: code_review, verb: submit-review,
#     requires_gate: REVISE) with NO guard — finds the "→ blocked" fallback
#   - status = blocked
#   - blocked_reason = "4th revise rejected by guard current_cycle <= 4 on phase 1 cycle 4: still broken"
#   - current_cycle stays at 4 (we DON'T persist the would-be-5; the working-copy bump is rolled back)
#   - Release lock
#   - Commit
# (Cycle 2 audit: per AC5.11, this is verified atomic — the cycles[].review write and
#  the status/blocked_reason write are in the same tx.)

$ stores tasks next-action T003 --json
{ "id": "T003", "status": "blocked", "current_phase": 1, "current_cycle": 4,
  "next_agent": null, "blocked": true,
  "blocked_reason": "4th revise rejected by guard current_cycle <= 4 on phase 1 cycle 4: still broken",
  "claimed_by": null, "claimed_at": null }

# === HUMAN UNBLOCK ===
# Human reviews blocked_reason in main.md (rendered after each step), updates plan/code as needed.

$ stores tasks resume T003
Resumed T003; status now: executing
# Inside one tx (Phase 5.4 row "resume"):
#   - Acquire lock (invoker: ai_with_human, since resume's actor: ai_with_human)
#   - Validator passes Op::Transition("resume", diff) — the resume verb is a regular transition
#   - status = ready
#   - current_cycle = 1  (RESET per the post-action table; M10 fix; cycles list preserved as audit trail)
#   - blocked_reason = NULL  (cleared on resume)
#   - on_state.ready: [TransitionTo(executing)] fires inside the same tx:
#       * status = executing
#       * current_phase UNCHANGED (still 1; resume returns to the blocked phase)
#   - Release lock
#   - Commit

# === RECOVERY: RE-EXECUTE PHASE 1 ===
$ stores tasks submit-execute T003 --summary "post-unblock fix" --commit fixed
[cycles[].executor appended for phase 1, cycle 1 (fresh count); status: code_review]

$ stores tasks submit-review T003 --gate PASS --critical 0 --major 0 --minor 0 \
    --summary "approved after unblock"
Submitted review for T003 phase 1 cycle 1; status now: executing
# Inside one tx:
#   - Acquire lock
#   - Validator passes Op::SubmitReview("PASS", diff)
#   - Append cycles[].review entry
#   - Engine evaluates the two PASS transitions (M9 / 5.5b):
#       * (→ executing): guard "current_phase < plan.phases.length"
#         (1 < 2) → TRUE
#       * (→ complete): guard "current_phase >= plan.phases.length"
#         (1 >= 2) → FALSE
#     Selects → executing
#   - current_phase = 2  (PASS-non-last: bump phase)
#   - current_cycle = 1  (PASS-non-last: reset cycle, per auto_increment_within: current_phase)
#   - status = executing
#   - Release lock
#   - Commit

# === PHASE 2 ===
$ stores tasks submit-execute T003 --summary "phase 2 done" --commit ph2 ...
[status: code_review; cycles[].executor for phase 2, cycle 1]

$ stores tasks submit-review T003 --gate PASS --critical 0 --major 0 --minor 0 \
    --summary "all done"
Submitted review for T003 phase 2 cycle 1; status now: complete
# Inside one tx:
#   - Engine evaluates the two PASS transitions:
#       * (→ executing): guard "current_phase < plan.phases.length" (2 < 2) → FALSE
#       * (→ complete): guard "current_phase >= plan.phases.length" (2 >= 2) → TRUE
#     Selects → complete
#   - status = complete
#   - current_phase = 2 (unchanged; we don't bump past last)
#   - Release lock
#   - Commit

$ stores tasks render T003
Rendered tasks/completed/T003-smoke-test/main.md
# (Render is a SEPARATE command after submit's tx commits. Idempotent.)
# - Detects on-disk dir at tasks/active/T003-smoke-test/, status_dir is now "completed"
# - Moves dir: tasks/active/T003-smoke-test/ → tasks/completed/T003-smoke-test/
# - Writes main.md atomically (.tmp + rename)
```

**Cycle-2 audit point (surfaced by drafting this transcript):** The original guard expression `current_cycle <= 3` allows only 2 REVISE cycles before BLOCKED, not 3. The corrected expression `current_cycle <= 4` (post-increment) gives the intended "3 REVISE cycles allowed; 4th rejected" semantics. Phase 5.5's narrative and Phase 7's schema YAML are aligned to `current_cycle <= 4` in the cycle-2 revision.

**Decision Matrix update:** A24 is amended in cycle 2: "Use `current_cycle <= 4`, post-increment" (correcting the cycle-2 first-pass which said `<= 3`).

---

## Plan Review

- **Gate:** READY (cycle 2 of max 3)
- **Reviewed:** 2026-04-26
- **Reviewer:** plan-reviewer agent
- **Summary:** Cycle 2 resolves all four cycle-1 critical findings (C1 list_record/list_fk + depth-3 walk into Phase 1; C2 4th-REVISE guard fixed to `current_cycle <= 4` post-increment with initial value 1; C3 transaction boundary specified end-to-end via `transition::run_in_tx` split with explicit lock-release-as-final-action; C4 next-action-as-validator dependency dropped) and all 11 majors (M1-M11). The Decision Matrix grew to A29 with the five autonomous closures (Q2/Q4/Q5/Q6/Q8 → A19/A20/A21/A22/A23) and the four C-fix rationales (C1/C2/C3/C4 → A24/A26/A27 + 5.7 plumbing). Q-NEW-1 surfaced and well-formed. The new worked-example transcript caught its own off-by-one in cycle-2 first-pass (`<= 3` → `<= 4`) — exactly the implicit-decision-forcing exercise cycle-1 requested.
- **Issues:** 0 critical, 0 major, 3 minor propagation-hygiene flags + 2 documentation cleanups (see plan-review.md). All three hygiene flags fold into normal Phase 7 / Phase 9 execution; none block READY.
- **Cycle-2 hygiene flags for executor (apply during implementation):**
  - **H1:** AC7.2 says initial `current_cycle: 0` — should be `1` (consistent with A28 / 5.4 table / worked-example).
  - **H2:** Phase 9.3 step 9 says `current_cycle: 1` after 1st REVISE — should be `2` (post-increment from 1).
  - **H3:** Phase 9.3 step 10 says "to hit cycle 3" after 2 more iterations — should be "to hit cycle 4" (3 REVISEs from initial 1 land at cycle 4).
- **Open Questions Finalized (cycle 1 verdicts confirmed; cycle 2 closes the 5 autonomous):**
  - **Q1** (priority field scope): OPEN — user input required.
  - **Q2** → **A19** (render dir-move synchronous in `render`).
  - **Q3** (`requires_gate` shape): OPEN — borderline; default A unless user objects.
  - **Q4** → **A20** (self-contained per-agent templates).
  - **Q5** → **A21** (engine writes context-rich `blocked_reason`).
  - **Q6** → **A22** (auto-cycle to planning with 4th-attempt block guard).
  - **Q7** (`scope: repo` outside git: fall back vs. error): OPEN — UX call; recommend B (hard error).
  - **Q8** → **A23** (bash for e2e walk; Rust integration test for panic-injection atomicity).
  - **Q-NEW-1** (legacy T001/T002 vs. DB-backed `tasks list` UX boundary): OPEN — recommend C (clean break in v0.2; document boundary).
- **Net open questions for user:** Q1, Q3, Q7, Q-NEW-1 (4 of 9). All four are genuinely user-level decisions.

→ Details: `plan-review.md` (cycle 2)

---

## Execution Log

### Phase 1: Schema feature foundation

- **Status:** COMPLETE
- **Started:** 2026-04-26
- **Completed:** 2026-04-26
- **Commits:**
  - `bcd5b84` T002 P1.1: actor: framework enum value + invoker rejection
  - `ae3aef0` T002 P1.2+1.3+1.5+1.7+1.8: schema features — auto_increment, expr.rs, scope, ListRecord, ListFk
  - `4ef518e` T002 P1.4: expr_eval.rs — evaluate guard Exprs against EntryMap
  - `435084c` T002 P1.6: paths.rs — StoreScope-aware stores_dir_for + git_common_dir
  - `f5cbf5b` T002 P1.9: lift depth limits in read_row / build_entry_map for depth-3 nests
  - `0d12485` T002 P1.10: requires_gate on Transition + ambiguity validation
  - `e93eff6` T002 P1.11: framework-actor DDL test
  - `2e8202c` T002 P1: manifest + install record scope per InstalledStore
- **Files Modified:**
  - `src/schema/actor.rs` — Actor::Framework variant, Display, Deserialize, from_env guard
  - `src/schema/mod.rs` — StoreScope, auto_increment attrs, ListRecord, ListFk FieldType variants, scope on Schema, validate_auto_increment, validate_transition_ambiguity wired in
  - `src/schema/expr.rs` — NEW: Lhs/Op/Rhs/Expr AST + parse_guard()
  - `src/schema/required_when.rs` — pub use GuardExpr re-export from expr.rs
  - `src/schema/lifecycle.rs` — requires_gate on Transition, validate_transition_ambiguity()
  - `src/validate/actor.rs` — actor_allowed: Framework arm; test for framework field
  - `src/validate/expr_eval.rs` — NEW: eval(expr, entry) → bool
  - `src/validate/mod.rs` — register expr_eval module
  - `src/validate/required.rs` — Field init: auto_increment fields
  - `src/validate/regex_check.rs` — Field init: auto_increment fields
  - `src/validate/enum_check.rs` — Field init: auto_increment fields
  - `src/codegen/ddl.rs` — ListRecord/ListFk → TEXT; framework DDL test (AC1.11)
  - `src/handlers/schema_show.rs` — field_type_str: ListRecord/ListFk arms
  - `src/handlers/row.rs` — read_row: ListRecord/ListFk deserialization; insert_at_path for depth-3; depth-3 round-trip tests
  - `src/cli/dispatch.rs` — reject --invoker framework with clear error
  - `src/paths.rs` — stores_dir_for(scope), git_common_dir(), Mutex-serialized CWD tests
  - `src/manifest.rs` — InstalledStore.scope: StoreScope, serialize/deserialize
  - `src/install.rs` — pass schema.scope into InstalledStore
  - `Cargo.toml` — dev-dependencies: tempfile = "3"
- **Test count:** 174 (110 original + 64 new; all pass)
- **e2e:** All 13 steps pass when CLAUDECODE is unset from environment. Pre-existing: e2e fails when CLAUDECODE is inherited from Claude Code session (ai_autonomous vs ai_with_human mismatch on observations triage step) — identical behavior on unmodified baseline.
- **Deviations from plan:**
  - Tasks 1.2, 1.3, 1.5, 1.7, 1.8 batched into one commit (all in schema/mod.rs and new expr.rs — logically coherent unit)
  - **M1 (deferred — two ASTs, not one):** `required_when.rs` re-exports `GuardExpr` (= `expr::Expr`) but keeps its own narrower `Expr { lhs_path, rhs_literal }` struct for backwards compatibility. Existing call sites in `validate/required.rs` and `handlers/schema_show.rs` use `.lhs_path` / `.rhs_literal` on the narrower type. The plan's "single AST type" intent is **genuinely deferred to Phase 5**, not satisfied here. Phase 5 will need to either (a) widen `required_when.rs::Expr` to an alias of `expr::Expr` and update the 8 existing call sites, or (b) add `impl From<required_when::Expr> for expr::Expr` as a bridge. The two ASTs coexist without conflict in Phase 1 because no code path compares or passes them interchangeably yet; that changes in Phase 5's transition handler.
  - **M2 (deferred — ListRecord sub-fields not validated at runtime):** `validate/mod.rs::validate_field` does not recurse into `FieldType::ListRecord` element fields. A `required: true` field inside a list element does NOT trigger a validation error. This is safe in Phase 1 (no submit path writes individual list elements). Phase 5 must add the ListRecord walker before `submit-execute` writes `cycles[].executor.summary`. A TODO comment at `validate/mod.rs` line ~80 and a pinning test (`list_record_required_sub_field_not_validated_phase1`) document this contract; when Phase 5 adds the walker the test expectation inverts from `unwrap()` to `unwrap_err()`.
  - `tempfile = "3"` added as dev-dependency for paths.rs tests (needed for `tempfile::tempdir()`). Not in plan's file list but consistent with the test requirement.
  - CWD-mutating path tests use a `Mutex<()>` guard rather than `#[serial_test]` crate (avoided adding an extra test dep).
- **Notes:**
  - AC1.4 marquee test (`current_cycle <= 4`, value 4 → true; value 5 → false) fully implemented and passing.
  - AC1.6 outside-git hard error confirmed (per user decision Q7→B).
  - All 12 ACs verified by unit tests that can be individually run.
  - The `flatten.rs` module was NOT modified — ListRecord/ListFk have no flat CLI arg surface in Phase 1 (CLI surface is Phase 4+). The build_entry_map insert_at_path helper is future-safe for arbitrary depth.

### Phase 2: `workflow:` block in schema

- **Status:** COMPLETE
- **Started:** 2026-04-26
- **Completed:** 2026-04-26
- **Commits:**
  - `6b84c2c` T002 P2.1+P2.2: define Workflow types and wire into Schema
  - `4e8998f` T002 P2.3+P2.4: WorkflowResolved + install-time template resolution
- **Files Modified:**
  - `src/schema/workflow.rs` — NEW: `Workflow`, `AgentRole`, `StateAction` (DispatchAgent/Increment/TransitionTo), `WorkflowResolved`, `FieldShape`, serde deserialization, `validate_cross_refs`, `resolve_from_disk`, `resolve_from_strings`
  - `src/schema/mod.rs` — `pub mod workflow`; `pub use workflow::{...}`; `Schema.workflow: Option<Workflow>`; `RawSchema.workflow`; validation call in `Schema::from_yaml`; 8 AC-level schema tests added
  - `src/install.rs` — call `wf.resolve_from_disk(&canonical)` at install time to verify template files exist (AC2.5)
  - `tests/fixtures/workflow_minimal/schema.yaml` — new fixture with lifecycle + fields + workflow block
  - `tests/fixtures/workflow_minimal/templates/planner-brief.md.tpl` — fixture template
  - `tests/fixtures/workflow_minimal/templates/executor-brief.md.tpl` — fixture template
  - `tests/fixtures/workflow_minimal/templates/main.md.tpl` — fixture template
- **Test count:** 207 (184 prior + 23 new; all pass)
- **e2e:** All 13 steps pass
- **AC verification:**
  - **AC2.1** (backward compat): `schema_without_workflow_is_none` test + all 184 prior tests pass unchanged
  - **AC2.2** (full parse): `schema_with_workflow_parses` — agent_roles, on_state, briefing_templates, submit_targets, render_target_path round-trip
  - **AC2.3** (unknown on_state state): `schema_workflow_unknown_on_state_errors` names the state
  - **AC2.4** (unknown DispatchAgent role): `schema_workflow_unknown_dispatch_agent_errors` names the role
  - **AC2.5** (missing template path): `resolve_from_disk_missing_template_errors` errors with path in message
  - **AC2.6** (submit_targets field existence + type shape): `schema_workflow_submit_target_unknown_field_errors` + `schema_workflow_submit_plan_wrong_type_errors`
- **Deviations from plan:**
  - Tasks 2.1 and 2.2 batched into one commit (both in new `workflow.rs` + `mod.rs` — coherent unit)
  - Task 2.5 validation logic implemented inside 2.1 commit's `validate_cross_refs` method (not a separate commit); all validation rules are present and tested
  - `tempfile` crate used in `workflow::tests::resolve_from_disk_missing_template_errors` (already in dev-dependencies from Phase 1)
  - `resolve_from_disk` called at install time for validation only; in-memory `WorkflowResolved` is not persisted (consistent with the plan: it's created fresh at each load/dispatch, driven by the schema YAML on disk). The `Schema` struct itself holds `Option<Workflow>` (paths); resolution happens on demand. This is a minor deviation from "embed in memory at install" — but since the manifest doesn't store workflow state and schemas are re-parsed from YAML at startup, producing a `WorkflowResolved` at dispatch time (Phase 5) is the correct final shape. Phase 5 will add the load-time resolution in the main.rs schema loading loop.
- **Notes:**
  - The `workflow_minimal` fixture schema deliberately uses `auto_increment: true` on `current_phase` without `auto_increment_within` (valid: top-level auto-incrementor). This is the simplest valid shape for Phase 2 tests.
  - No CLI verbs added (deferred to Phases 4-6 per plan).
  - No engine logic added (deferred to Phase 5 per plan).

---

## Code Review Log

### Phase 1 — cycle 2

- **Gate:** PASS
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 2 of max 3
- **Issues:** 0 new (cycle 1's 1 critical / 2 major / 3 minor: critical fixed; both majors deferred to Phase 5 with explicit notes + TODO + pinning test; minors fixed or accepted)
- **Status next:** EXECUTING_PHASE_2
- **Summary:** Cycle 2 closes the cycle-1 critical (AC1.4 parser/eval gap) cleanly. New `Rhs::Path` + `Rhs::PathLength` AST variants land at the parser, with eval-side resolution that mirrors LHS path semantics; 8 new tests at parse + eval level cover the EXACT AC1.4 form `current_phase < plan.phases.length` (true on 1<2, false on 2<2, false on missing path) plus the M9-companion `>=` form and the path-vs-path equality form. The two majors (M1 single-AST unification; M2 ListRecord validator walker) are explicitly deferred to Phase 5. The deferrals are acceptable on Phase-1 cohesion grounds: M2 has a `TODO(phase-5)` block comment AND a self-naming pinning test (`list_record_required_sub_field_not_validated_phase1`) that will FAIL when Phase 5 closes the gap; M1's deviation note is now accurate ("two ASTs coexist; Phase 5 must bridge"). m1 test names corrected; m2 added a `cycles_update_round_trips` test exercising add → UPDATE → read on a list_record column (covers element-modify and element-add; element-remove not tested but acceptable since Phase 5 only appends).
- **What's good:** 184 tests pass (174 + 10 new), 0 failed; e2e all 13 steps green; the C1 fix is surgical (~80 LOC code + ~80 LOC tests, no API churn at existing call sites); pinning-test pattern with `_phase1` in the name will surface itself when Phase 5 tries to silently keep it.
- **Verified actions:** All 6 cycle-1 required actions addressed (C1 fully fixed; M1 and M2 explicitly deferred with documented Phase-5 obligations; m1 + m2 fixed; m3 accepted with executor's "don't churn history" rationale).
- **Carry-forward to Phase 5:** The Phase 5 plan MUST enumerate (a) bridging `required_when::Expr` and `expr::Expr` (option a — widen to alias and update 8 call sites — or option b — `impl From<...>`) under task 5.2, and (b) extending `validate/mod.rs::validate_field` to recurse into `FieldType::ListRecord` sub-fields under task 5.3. Phase 5's plan-review must verify both are present before execution begins.
- → Details: `code-review-phase-1.md`

### Phase 2 — cycle 1

- **Gate:** PASS (with documented Phase-5 carryforward)
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 1 of max 3
- **Issues:** 1 major (deferred) / 4 minor (3 accepted, 1 cross-phase note)
- **Status next:** EXECUTING_PHASE_3
- **Summary:** All 6 ACs verified by named tests (23 new + 184 prior all green). The fixture `tests/fixtures/workflow_minimal/` is appropriately minimal; `validate_cross_refs` covers all 4 plan-2.5 rules + AC2.6's submit_targets type-shape branches with dedicated tests (`workflow_validate_submit_plan_wrong_type_errors`, `workflow_validate_submit_execute_accepts_list_record`, `workflow_validate_submit_execute_wrong_type_errors`). e2e all 13 steps green. The single major issue (M1) is the executor's documented deviation: `Schema.workflow` carries `PathBuf` paths, and `WorkflowResolved` is constructed transiently inside `install::run` for AC2.5 validation only — never threaded into runtime. Plan task 2.4 explicitly asked for the in-memory `Workflow` to carry text after install. The deviation is acceptable as a Phase-5 carryforward because the runtime-threading decision (where to store the resolved map; how to integrate with main.rs's per-CLI schema reload) is engine-layer work that doesn't make sense in Phase 2; same shape as Phase 1's M1/M2 carryforwards. Minor m1 (no install-pathway integration test for AC2.5), m2 (`install_bundled` skips workflow validation), m3 (validate_cross_refs is direction-asymmetric on agent_roles ↔ briefing_templates) all accepted as-is. m4 flags that Phase 7's `on_state` YAML literal in the plan (main.md:555-561) uses pseudo-Rust syntax that will not parse via the deserializer the executor implemented; Phase 7 plan-review must catch this.
- **What's good:** Tests are named for the contract they enforce (every failure points at the broken AC by name); custom `Deserialize` for `RawStateAction` cleanly handles the three-variant action shape with explicit error messages; `FieldShape` enum decouples `validate_cross_refs` from `FieldType`'s full enum surface; resolve_from_disk and resolve_from_strings have symmetric outputs (Phase 7-ready); fixture has real Handlebars-syntax templates for Phase 3 to chew on.
- **Carry-forward to Phase 5:** The Phase 5 plan MUST add a third item alongside Phase-1's M1 and M2: **(P2-M1)** wire `WorkflowResolved` into the runtime schema map in `main.rs:25-50`. For filesystem paths, call `wf.resolve_from_disk(&schema_path_dir)`. For `bundled:<name>` paths, call `wf.resolve_from_strings(...)` against `BUNDLED_STORE_TEMPLATES` (introduced by Phase 7.6; Phase 5 may need to stub this map empty until Phase 7 lands). Phase 5's plan-review must verify all three carryforwards (P1-M1 expr unification, P1-M2 ListRecord walker, P2-M1 WorkflowResolved threading) are enumerated in the plan before execution begins.
- **Carry-forward to Phase 7:** Rewrite `on_state` YAML literal in plan main.md:555-561 from `[DispatchAgent(planner)]` to `- dispatch_agent: planner`. Extend `install_bundled` (install.rs:115-171) with workflow validation analog using `BUNDLED_STORE_TEMPLATES`.
- → Details: `code-review-phase-2.md`

---

## Completion

_Final summary when task is complete._
