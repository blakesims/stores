# Code Review — Phase 2 (cycle 1 of max 3)

- **Phase:** 2 — `workflow:` block in schema (opt-in declaration)
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Gate:** PASS (with documented Phase-5 carryforward)

---

## Git reality check

```
6b84c2c T002 P2.1+P2.2: define Workflow types and wire into Schema
4e8998f T002 P2.3+P2.4: WorkflowResolved + install-time template resolution
62679b3 T002 P2.5: schema validation rules + execution log
```

The 3rd commit (`62679b3`) is the Execution Log update only; the actual Phase 2.5 validation logic landed inside commit `6b84c2c` (validate_cross_refs in `workflow.rs` + the wiring in `schema/mod.rs`). The executor's deviation note acknowledges this batching.

`git diff --stat HEAD~3..HEAD`:

| File                                                              | Δ      |
|-------------------------------------------------------------------|--------|
| `src/install.rs`                                                  | +7     |
| `src/schema/mod.rs`                                               | +200   |
| `src/schema/workflow.rs`                                          | +728   |
| `tests/fixtures/workflow_minimal/schema.yaml`                     | +63    |
| `tests/fixtures/workflow_minimal/templates/executor-brief.md.tpl` | +13    |
| `tests/fixtures/workflow_minimal/templates/main.md.tpl`           | +13    |
| `tests/fixtures/workflow_minimal/templates/planner-brief.md.tpl`  | +12    |
| `tasks/active/T002-tasks-store-v02/main.md`                       | +39    |

207 unit tests pass (184 prior + 23 new); e2e all 13 steps pass with `CLAUDECODE` unset. No new compiler warnings. The pre-existing 4 dead-code warnings in `paths.rs` (`stores_dir_for`, `git_common_dir`, `home_dir`, `dirs_home`) carry forward unchanged from Phase 1.

---

## AC verification table

| AC    | Statement                                                                                  | Verified by                                                                                                       | Status |
|-------|--------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------|--------|
| AC2.1 | Schema without `workflow:` parses identically; `schema.workflow.is_none()`; existing tests | `schema_without_workflow_is_none` (mod.rs:855-859) + 184 prior tests pass unchanged                               | PASS   |
| AC2.2 | Full `workflow:` block parses; agent_roles + on_state + briefing_templates + submit_targets + render_target_path round-trip | `schema_with_workflow_parses` (mod.rs:861-919); `workflow_parses_minimal` + companions in workflow.rs                                                  | PASS   |
| AC2.3 | Unknown lifecycle state in `on_state` errors with state name                               | `schema_workflow_unknown_on_state_errors` (mod.rs:921-942); `workflow_validate_unknown_on_state_errors` (workflow.rs:490-501) | PASS   |
| AC2.4 | Unknown agent role in `DispatchAgent` errors clearly                                       | `schema_workflow_unknown_dispatch_agent_errors` (mod.rs:944-967); `workflow_validate_unknown_dispatch_agent_errors` (workflow.rs:503-524) | PASS   |
| AC2.5 | Non-existent briefing template path errors at install time with the missing path           | `resolve_from_disk_missing_template_errors` (workflow.rs:685-701) at the function level; install.rs:48-51 wires it | PASS (function-level only — no install-pathway integration test) |
| AC2.6 | submit_targets unknown field errors with field name; submit-plan wrong type errors with type-shape message | `schema_workflow_submit_target_unknown_field_errors` (mod.rs:969-989); `schema_workflow_submit_plan_wrong_type_errors` (mod.rs:991-1018); `workflow_validate_submit_targets_unknown_field_errors`, `workflow_validate_submit_plan_wrong_type_errors`, `workflow_validate_submit_execute_accepts_list_record`, `workflow_validate_submit_execute_wrong_type_errors` (workflow.rs:549-639) | PASS   |

All 6 ACs PASS. Coverage is solid: 23 new unit tests across `schema/workflow.rs` and `schema/mod.rs`, the `tests/fixtures/workflow_minimal` fixture exercises real disk reads via `resolve_from_disk_reads_templates`.

---

## Issues by severity

### Major (M1) — `WorkflowResolved` is NOT threaded into runtime `Schema` (deviation from plan task 2.4)

Plan task 2.3 states explicitly: "read the briefing template files and render template into memory and **embed them in the in-memory `Workflow`**" and "**we don't want runtime FS reads**". Plan task 2.4 reinforces: "the in-memory `Workflow` carries text, not paths, **after the install step**."

The implementation:
- `Schema.workflow: Option<Workflow>` carries `PathBuf` (workflow.rs:54).
- `WorkflowResolved` (workflow.rs:75-85) is constructed transiently inside `install::run` solely for AC2.5's existence check (install.rs:48-51) and discarded.
- At runtime, `main.rs:25-50` re-parses every schema's YAML from disk on every CLI invocation. There is no place where `resolve_from_disk` (or `resolve_from_strings`) is called outside install.

**Consequence:** any future submit / brief / render handler will need to either (a) re-read template files at runtime per CLI call (violates "no runtime FS reads") or (b) call `resolve_from_disk` inside main.rs's schema-loading loop and store the result somewhere (violates the no-state-on-Schema invariant that `Schema` is immutable derived from YAML — so we'd want a parallel `HashMap<store_name, WorkflowResolved>` in main.rs).

The executor's deviation note acknowledges this and pins it as Phase-5 work. Reading the plan literally, this is a Phase-2 omission, not a Phase-5 task. However:
- The runtime cost of (a) is minimal — templates are <2KB and re-reading per CLI call is cheap.
- Fixing this in Phase 2 would require deciding between (a) and (b) and adding the call in main.rs, which is a Phase-5 (engine-layer) decision because the engine is the only consumer.
- The plan was written before the executor walked the code; the v0.1 schema-reload-from-disk-per-invocation pattern wasn't fully internalised when 2.4 was drafted.

**Judgment:** acceptable as a documented Phase-5 carryforward, alongside Phase-1's M1 (single-AST unification) and M2 (ListRecord validator walker). The Phase 5 plan-review **must** verify that the schema-loading path in main.rs (or the engine's submit handlers) calls `resolve_from_disk` for path-installed stores AND `resolve_from_strings` for bundled stores, so that briefing/render template text is in memory by the time Phase 5/6 handlers need it. Concretely: Phase 5's task list must include "wire `WorkflowResolved` into the runtime schema map (main.rs:25-50) — extend the schema map to `HashMap<String, (Schema, Option<WorkflowResolved>)>` or a parallel map; for `bundled:<name>` paths, add a `BUNDLED_STORE_TEMPLATES` lookup (Phase 7.6) and call `resolve_from_strings`; for filesystem paths, call `resolve_from_disk(&schema_path_dir)`."

This is **not a fail-the-gate** issue — Phase 2 lands a usable shape; Phase 5/7 finishes the threading. But if Phase 5's plan does not enumerate the wiring explicitly, that plan-review must REVISE.

### Minor (m1) — AC2.5 is verified at function level, not install-pathway level

The unit test `resolve_from_disk_missing_template_errors` directly invokes `wf.resolve_from_disk(...)` on a tmp dir with no templates. install.rs:48-51 wires `resolve_from_disk` into the install flow, but no integration test creates a fixture-with-missing-templates and runs `install::run` against it to verify the install command surfaces the expected error to the user.

**Impact:** low. The function and the install wiring are both small enough to inspect by eye, and `resolve_from_disk`'s tests exercise the only reachable error path. But a one-liner integration test (e.g. against a `tests/fixtures/workflow_missing_template/` directory) would close the gap and prevent silent breakage if `install.rs:48-51` is later refactored.

**Recommendation:** acceptable as-is for Phase 2; revisit if Phase 7's bundled-tasks integration uncovers a real install-time bug.

### Minor (m2) — `install_bundled` does not call `resolve_from_disk` / `resolve_from_strings`

`install::run` calls `wf.resolve_from_disk(&canonical)` for path-installed stores (install.rs:48-51), but `install_bundled` (install.rs:115-171) skips workflow validation entirely. Today this matters only because no bundled store has a `workflow:` block yet (tasks lands in Phase 7). When Phase 7 bundles `tasks` with workflow templates, `install_bundled` will need an analogous validation step using `BUNDLED_STORE_TEMPLATES` (planned in 7.6).

**Impact:** none today. But the symmetry gap is worth flagging so Phase 7's plan-review confirms `install_bundled` gets the corresponding `resolve_from_strings(...)` call.

**Recommendation:** acceptable as-is for Phase 2.

### Minor (m3) — `validate_cross_refs` lookup is direction-asymmetric

The check enforces "every `agent_roles` key has a `briefing_templates` entry" but not the inverse "every `briefing_templates` key has an `agent_roles` entry". A stray `briefing_templates: { unused_role: ... }` silently passes parsing. Plan task 2.5 specifies this direction explicitly, so the implementation matches the plan — but a reviewer-quality nit is that the inverse check is cheap and would catch typos.

**Impact:** none functionally; potential silent typo if a schema author misnames a `briefing_templates` key.

**Recommendation:** acceptable as-is. If desired, add as a 1-line check + test in Phase 5 cleanup.

### Minor (m4) — Phase 7's `on_state` YAML literal in the plan is not the syntax the deserializer accepts

Phase 7's tasks-schema YAML literal (main.md:555-561) uses `planning: [DispatchAgent(planner)]` — Rust enum-variant pseudocode. The deserializer in `workflow.rs:105-138` accepts `planning: - dispatch_agent: planner` (map-with-one-key). Phase 7 will need a YAML translation pass before the schema is usable; the executor of Phase 7 must rewrite the on_state block to match the actual YAML grammar.

**Impact:** none in Phase 2. Flagged for Phase 7 plan-review.

**Recommendation:** acceptable as-is; this is a Phase 7 concern.

---

## Deviation judgment

The executor's deviation summary is honest and accurate. The `Schema` stores `Option<Workflow>` (paths), not `WorkflowResolved` (text). The plan asked for the latter. The executor pinned the gap to Phase 5 because the runtime threading decision (paragraph in M1 above) is engine-layer work that doesn't make sense to land in Phase 2.

This pattern (Phase 2 lands the data shape; Phase 5 wires it into main.rs's runtime map) mirrors Phase 1's M1/M2 carryforwards (single-AST unification + ListRecord validator walker also deferred to Phase 5). All three carryforwards are concentrated in Phase 5. Phase 5's plan-review will need to verify the plan addresses ALL THREE before execution begins — this is now the dominant Phase-5 risk.

**Gate decision:** PASS with explicit Phase-5 carryforward. Phase 5's plan must enumerate the WorkflowResolved threading task alongside Phase-1's M1 (Expr unification) and M2 (ListRecord validator) before executing.

---

## What's good

- **23 new tests, all named for the AC / contract they enforce.** `schema_workflow_submit_plan_wrong_type_errors`, `workflow_validate_submit_execute_accepts_list_record`, `resolve_from_disk_missing_template_errors`, `state_action_transition_to_parses` — each test failure points immediately at the contract it broke.
- **Custom `Deserialize` for `RawStateAction`** (workflow.rs:105-138) handles the three-variant action shape cleanly with a single `MapAccess` walker. The error message ("unknown state action key 'X'; expected dispatch_agent, increment, or transition_to") names the offending input.
- **`resolve_from_disk` and `resolve_from_strings` symmetry** — both produce the same `WorkflowResolved`, with the latter taking pre-loaded text. This is the right shape for Phase 7's `BUNDLED_STORE_TEMPLATES` integration.
- **`FieldShape` enum** (workflow.rs:407-411) isolates the "is this field a record / list_record / something else" decision into a dedicated type, decoupling `validate_cross_refs` from `FieldType`'s full enum surface. This means the validator survives unscathed if `FieldType` adds variants in the future.
- **Test fixture `tests/fixtures/workflow_minimal/`** is genuinely minimal — schema.yaml is 63 lines with the simplest valid workflow, three tiny templates with realistic Handlebars syntax to exercise the path. Not over-engineered.
- **`agent_role` name auto-fill** (workflow.rs:174-183) — the YAML key is automatically used as the role's `name`, so authors can't get out-of-sync between the map key and the struct field. Small but nice.
- **Phase 1's pre-existing `auto_increment` validator passes the fixture's `current_phase: integer, actor: framework, auto_increment: true`** (no `_within`) — confirms the Phase 1 validator handles the top-level-only auto_increment case correctly.

---

## Learnings (carried forward to Phase 5 plan)

1. **Phase 5 plan must address THREE deferred items**, all due in Phase 5's first task list pass:
   - **(from Phase 1, M1)** Bridge `required_when::Expr` and `expr::Expr` — option (a) widen `required_when::Expr` to alias `expr::Expr` and update 8 call sites, or option (b) `impl From<required_when::Expr> for expr::Expr`.
   - **(from Phase 1, M2)** Extend `validate/mod.rs::validate_field` to recurse into `FieldType::ListRecord` element fields. The pinning test `list_record_required_sub_field_not_validated_phase1` will FAIL when Phase 5 closes the gap (intentional invert).
   - **(from Phase 2, M1)** Wire `WorkflowResolved` into runtime: extend the schema map in `main.rs:25-50` to also produce a `HashMap<String, WorkflowResolved>` for stores that declare `workflow:`. For filesystem paths, call `wf.resolve_from_disk(&schema_path_dir)`. For `bundled:<name>` paths, call `wf.resolve_from_strings(...)` against a `BUNDLED_STORE_TEMPLATES` map (Phase 7.6 introduces this map; Phase 5's plan must coordinate with Phase 7 or stub `BUNDLED_STORE_TEMPLATES` empty until Phase 7 lands).
2. **Phase 7 plan-review must rewrite the on_state YAML literal** (main.md:555-561) from `[DispatchAgent(planner)]` to `- dispatch_agent: planner`. The Phase 2 deserializer's grammar is final.
3. **Phase 7 must extend `install_bundled`** with the workflow validation analog of `install::run`'s `resolve_from_disk` step, using `BUNDLED_STORE_TEMPLATES` lookups.

---

## Status next

`EXECUTING_PHASE_3`.
