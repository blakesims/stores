# Phase 7 Code Review — `tasks` store schema + bundled templates

- **Gate:** REVISE
- **Reviewed:** 2026-04-27
- **Reviewer:** code-reviewer agent
- **Cycle:** 1 of max 3
- **Status next:** EXECUTING_PHASE_7
- **Findings:** 0 critical / 2 major / 4 minor / 1 info

---

## Verification of Acceptance Criteria

| AC | Result | Evidence |
|----|--------|----------|
| AC7.1 | PASS | Live smoke: `stores init && stores install tasks` → "Installed bundled store 'tasks' (table: tasks)"; `.stores/manifest.yaml` shows `schema_path: bundled:tasks, table_name: tasks, scope: repo`; sqlite `.tables` includes `tasks`. |
| AC7.2 | PASS (with caveat — see info-1) | `stores tasks add --title "Test task" --slug "test-task" --done-when "X works" --scope-in "y" --scope-out "z" --invoker human` returns `T001`; `show T001` returns `status: planning, current_phase: <NULL>, current_cycle: <NULL>`. The plan literal said "0" but field has `actor: framework + auto_increment`, so NULL on add (set to 1 only on the `ready→executing` follow-on) is the correct behavior. |
| AC7.3 | PASS | `ac7_3_bundled_tasks_schema_parses` (mod.rs:1037-1075) loads YAML from `BUNDLED_STORE_SCHEMAS`, asserts `Schema::from_yaml` succeeds, walks all 4 agent_roles, all 4 briefing_templates, all 4 submit_targets entries, all 7 lifecycle states, `scope == Repo`. `ac7_3b_bundled_tasks_templates_present` (mod.rs:1079-1097) asserts 5 templates in `BUNDLED_STORE_TEMPLATES` with non-empty content. |
| AC7.4 | PASS (with caveat — see m4) | `ac7_4_all_four_briefing_templates_render_successfully` (brief.rs:333-402) builds a fixture EntryMap (planning state, contract+plan populated), calls `build_context`, then `render_template(content, &ctx)` for each of the 4 role templates pulled from `BUNDLED_STORE_TEMPLATES`. Asserts each rendering contains both `"Test Task"` and `"Feature X works end-to-end"`. Live CLI: `stores tasks brief T001 --invoker ai_with_human` returns valid planner markdown; `--for executor` and `--for code_reviewer` overrides work end-to-end. |
| AC7.5 | PASS | `ac7_5_framework_fields_have_framework_actor` (mod.rs:1102-1115) asserts `current_phase` and `current_cycle` fields parse with `Some(Actor::Framework)`. Live CLI: `stores tasks update T001 --current-phase 5 --invoker human` and `stores tasks add --current-phase 5 --invoker human` both fail with `validation failed: - current_phase: field 'current_phase' requires actor 'framework'; invoker is 'human'`. |

All five ACs verified. 288 unit tests pass (284 prior + 4 new); all 13 e2e steps green. Live end-to-end smoke (init → install → add → submit-plan → submit-plan-review → submit-execute → submit-review → render) drives the workflow forward correctly through 7 states.

---

## What's good

- **Bundled-name install works first try.** `stores install tasks` (no path) shortcuts to the embedded YAML + templates with zero filesystem dependency. The `BUNDLED_STORE_TEMPLATES` map (`dynamic.rs:36-49`) is keyed correctly and matches both `briefing_templates` paths and `render_template` path in the schema YAML.
- **Schema authoring is faithful to the plan.** Every Phase 1-6 feature is exercised: `actor: framework` (4 fields), `auto_increment` (current_phase top-level + current_cycle scoped), `list_fk` (depends_on, linked_observations), `list_record` (plan.phases nested in plan record; plan_review_log; cycles with executor + review sub-records), `record` (contract, plan), `enum` on gate fields, `{list: text}` on six list-of-text fields, `guard:` on 4 transitions, `requires_gate:` on 8 transitions, `pattern:` on slug, `scope: repo`. Spec match line-by-line against plan main.md:455-561.
- **Workflow-verb dedup.** `WORKFLOW_VERBS` constant in `dynamic.rs:153-156` plus `registered_verbs: HashSet<String>` (line 180-198) correctly prevents the schema-declared `submit-*` and `resume` transition verbs from being double-registered as transition subcommands. `--help` output cleanly lists each verb exactly once.
- **`list_text` typo fix.** Commit `d555844` correctly changed the schema from `type: list_text` to `type: {list: text}` to match the parser's nested-shape syntax. Without this, the schema would not have parsed.
- **Bundled-sentinel detection (P6-m2 carry-forward).** Both `brief.rs:122-135` and `render.rs:108-120` detect `manifest.schema_path.strip_prefix("bundled:")`, look up template content from `BUNDLED_STORE_TEMPLATES` rather than the disk, and propagate clear error messages naming the template path. Live `stores tasks brief T001` and `stores tasks render T001` against the bundled store both succeed.
- **submit_targets lookup (P5-m3 carry-forward).** All four `compute_submit_*` functions consult `workflow.submit_targets.get(verb)` (submit.rs:464, 543, 694, 830) before falling back to a hardcoded literal. The lookup actually fires for the bundled tasks store — the literal fallback is unreachable in production. Phase 7's tasks schema uses the canonical names so the regression surface is small but the framework value-prop is now real.
- **Carry-forward closures all wired.** P5-m2 (`--open-questions-from-file`), P5-m3 (submit_targets lookup), P5-m4 (`--details-from-file` separate from `--summary`) all land. Verified via live CLI: `submit-plan-review --open-questions-from-file` populates the array; `submit-review --summary "..." --details-from-file f.md` writes both as separate sub-fields in `cycles[N].review.{summary,details}`.
- **Render context plan_phases_count + current_phase_idx.** `context.rs:62-75` derive these from the entry; `executor-brief.md.tpl:26` and `code-reviewer-brief.md.tpl:26` use `{{#if (eq @index ../current_phase_idx)}}` to filter to the current phase only — the executor briefing is correctly token-efficient (one phase, not the whole plan).
- **Idempotency holds end-to-end.** Two consecutive `stores tasks render T001` calls produced byte-identical output (verified via `diff` after capturing first render).

---

## Findings

### M1 — `--files-changed` stored as CSV string, schema declares `{list: text}` — render breaks

`compute_submit_execute` at submit.rs:727-729:

```rust
if let Some(files) = files_changed {
    executor_obj.insert("files_changed".to_string(), Value::String(files.to_string()));
}
```

The bundled tasks schema declares `executor.files_changed: {list: text}` (schema.yaml:52). The CLI flag `--files-changed` is described as "Comma-separated list of changed files" (dynamic.rs:321), suggesting parse-into-array intent, but the handler stores the raw CSV string verbatim.

**Live reproduction (verified):**

```
$ stores tasks submit-execute T001 --summary "did stuff" --commit abc1234 \
    --files-changed "src/foo.rs,src/bar.rs" --invoker ai_autonomous
$ sqlite3 .stores/db.sqlite "SELECT cycles FROM tasks WHERE display_id='T001'"
[{"cycle":1,"executor":{"commit":"abc1234","files_changed":"src/foo.rs,src/bar.rs",...}}]
```

Then `stores tasks render T001` produces (rendered Execution Log section):

```
### Phase 1 / Cycle 1
- **Status:** Submitted — awaiting review
- **Summary:** did stuff
- **Commit:** `abc1234`
- **Files:**
```

The `**Files:**` heading is bare — the template `{{#each this.executor.files_changed}}` (main.md.tpl:87-89) cannot iterate a string as an array, so all expected file paths are silently dropped from the rendered output.

This is a marquee-path failure: the executor brief tells agents to call `submit-execute --files-changed "<csv>"` (executor-brief.md.tpl is silent on syntax, but the existing CLI suggests CSV), and the rendered main.md is the user-facing artifact. AC6.4 idempotency is preserved (string-in / string-out is deterministic) but the render is wrong.

**Required action:** In `compute_submit_execute`, split the CSV on `,`, trim each token, drop empties, and store as `Value::Array(Vec<Value::String>)`. Mirror the shape of `open_questions` (already done correctly in submit.rs:565-569). Add a unit test asserting `files_changed` is an array of strings post-submit, and a render-path test asserting the rendered Execution Log includes each filename.

### M2 — `at` timestamp not set on any list_record entry — schema declares it, handlers omit it

The bundled tasks schema declares `at: timestamp` on three sub-records:

- `plan_review_log[].at` (schema.yaml:41)
- `cycles[].executor.at` (schema.yaml:54)
- `cycles[].review.at` (schema.yaml:64)

None of the four submit handlers populate `at`. Verified by inspecting `compute_submit_plan_review` (lines 562-571 — log_entry_obj built without `at`), `compute_submit_execute` (lines 722-732 — executor_obj built without `at`), and `compute_submit_review` (lines 877-886 — review_obj_map built without `at`).

**Live reproduction (verified):**

```
$ sqlite3 .stores/db.sqlite "SELECT cycles, plan_review_log FROM tasks WHERE display_id='T001'"
[{"cycle":1,"executor":{"commit":"abc1234","files_changed":"...","summary":"did stuff"},"phase":1,
  "review":{"critical":1,"details":"...","gate":"REVISE","major":0,"minor":2,"summary":"needs fixes"}}]
|[{"gate":"READY","summary":"good"}]
```

No `"at"` key in any entry. The render template uses `{{#if this.at}}- **At:** {{this.at}}{{/if}}` (main.md.tpl:69, 91, 108) — defensively guarded, so the rendered output omits the line gracefully. But the data is missing from the DB, which means: (a) the audit trail is incomplete; (b) any downstream consumer (orchestrator skill, future analytics, manual queries) gets no temporal ordering of events; (c) the schema's contract is violated — fields declared as `timestamp` exist but are uninitialized.

`now_iso8601()` (row.rs) is already imported in submit.rs (line 37) and used elsewhere (lines 75, 185) — adding `obj.insert("at", json!(now_iso8601()))` to each of the three sub-records is a 6-line fix.

**Required action:** Insert `now_iso8601()` into each list_record entry at submit time:
- `compute_submit_plan_review` log_entry_obj (after line 564)
- `compute_submit_execute` executor_obj (after line 723)
- `compute_submit_review` review_obj_map (after line 879)

Add a unit test asserting `at` is present and ISO-8601 shaped on each appended entry. Add an end-to-end test: render after submit-execute → main.md "At:" line is present and parses as a timestamp.

### m1 — executor-brief.md.tpl tells the agent to edit main.md, contradicting DB-as-truth

`stores/tasks/templates/executor-brief.md.tpl:67-72`:

```
1. **READ** the entire phase above before starting
2. **SET** Status to `EXECUTING_PHASE_{{current_phase}}` in tasks/active/{{display_id}}-{{slug}}/main.md
3. **EXECUTE** tasks in order — do not skip or reorder
...
```

And line 94: `4. Set Status: BLOCKED with reason in main.md`.

This contradicts the executive-intent contract on main.md:24 ("main.md is rendered from DB rows on demand via `stores tasks render T<NNN>`. Agents NEVER edit main.md. There is exactly one write path: the CLI") and contradicts the rest of the same template (line 73 correctly says: "Call `stores tasks submit-execute {{display_id}} ...`").

The other three briefs (planner, plan-reviewer, code-reviewer) consistently use CLI-only verbs and do NOT instruct the agent to touch main.md. Only the executor brief drifted.

**Required action:** Rewrite executor-brief.md.tpl lines 67-72 and 90-95 to remove all main.md / Status edits. Replace with the CLI-only flow (already partially present at line 73: `stores tasks submit-execute --summary "..." --commit <sha>`). For "When Blocked": instruct the agent to call `submit-execute` with a notes field describing the block, then halt — the orchestrator/skill takes it from there. Mirror the structure of plan-reviewer-brief.md.tpl, which is correctly framework-aligned.

### m2 — Carry-forward closures (P5-m2/m3/m4, P6-m2) have no dedicated unit tests

The plan-reviewer cycle-1 noted Phase 7 is where carry-forwards from Phase 5 (m2/m3/m4) get real-world tested. None of the four carry-forwards has a dedicated regression test:

- **P5-m2 (`--open-questions-from-file`):** No test in `dispatch.rs::tests` asserts `read_lines_from_file` returns the right `Option<Vec<String>>` shape; no test in `submit.rs::tests` asserts `compute_submit_plan_review` with `Some(vec!["q1", "q2"])` writes those strings as an array sub-field. Live smoke shows it works, but a regression that drops the flag binding (e.g. typo to `open_questions_from_file`) would not fail any test.
- **P5-m3 (submit_targets lookup):** All four submit handlers retain their hardcoded fallback (`unwrap_or("plan")`, `unwrap_or("cycles")`, etc.). The fallback path masks any divergence — every existing test asserts on the canonical-named tasks/wf_minimal schema, so the hardcoded literal would always match. A regression where `workflow.submit_targets.get(verb)` returns `None` (e.g. a third-party schema with a custom field name) would silently use the wrong field.
- **P5-m4 (`--details-from-file` / `--summary`):** No unit test asserts that `compute_submit_review` with `review_summary="X", review_details=Some("Y")` writes `{summary: "X", details: "Y"}` as separate keys. Live CLI confirmed it.
- **P6-m2 (bundled-sentinel):** `ac7_4` test calls `render_template(content, &ctx)` directly with content pulled from `BUNDLED_STORE_TEMPLATES`; it bypasses the actual bundled-sentinel detection in `brief::compute` (lines 114-135) and `render::compute_render_in` (lines 103-120). A regression to the sentinel-detection logic (e.g. `strip_prefix("bundled:")` returning None for some reason) would not fail any test. Live CLI smoke proves it works today.

288 tests is only 4 more than 284 — for ~600-900 LOC of work plus 4 carry-forwards, plus a real handler refactor (submit_targets lookup) and a CLI flag addition (open-questions), 8-12 new tests would have been the expected coverage. The 4 ACs are tested but the carry-forwards are not.

**Required action (cycle 2):** Add four targeted unit tests:
1. `ac7_p5m2_open_questions_appended_to_plan_review_log_entry` — calls `compute_submit_plan_review` with `Some(vec!["question one".into(), "question two".into()])`, reads back the row, asserts `plan_review_log[0].open_questions == ["question one", "question two"]`.
2. `ac7_p5m3_submit_targets_consulted_for_field_lookup` — load schema with `submit_targets: {submit-execute: my_custom_cycles_field}` (modify the workflow_minimal fixture or a new fixture), call `compute_submit_execute`, assert the row's `my_custom_cycles_field` column receives the entry. Asserts the lookup actually fires.
3. `ac7_p5m4_review_summary_and_details_separate_keys` — calls `compute_submit_review` with summary "S" and details Some("D"), reads cycles[0].review, asserts `summary == "S"` AND `details == "D"` (two distinct keys).
4. `ac7_p6m2_bundled_sentinel_routes_to_in_memory_template` — installs a manifest with `schema_path: bundled:tasks`, calls `brief::compute` (or a thin path probe), asserts the returned `brief_markdown` is non-empty and contains a known string from the bundled planner template (e.g. "Methodical and thorough"). The point is to exercise lines 122-135 of brief.rs.

### m3 — `start` verb leaks into help output for the `framework` actor

The schema declares the framework-fired transition `ready → executing` with `verb: start, actor: framework` (schema.yaml:86). At dynamic.rs:153-156, the `WORKFLOW_VERBS` constant lists the eight workflow CLI verbs; `start` is not on it. So at line 192-193, the `start` verb is registered as a regular transition subcommand and appears in `stores tasks --help`:

```
start               start an entry
```

Calling `stores tasks start T001 --invoker ai_autonomous` is harmless (the state-machine guard in `transition.rs` rejects it because the row is in `planning`, not `ready`), but the surface is exposed and documented as a callable verb. A user reading `--help` could reasonably try `stores tasks start T001` once the row is in `ready`, hitting the framework-actor rejection (already correct, but a confusing error).

**Required action:** Either (a) add `"start"` to `WORKFLOW_VERBS` so framework-only transitions are de-registered from the user-facing CLI surface (cleanest), or (b) add a generic guard: `if transition.actor == Some(Actor::Framework) { continue }` in the registration loop — handles all current and future framework transitions without enumeration. Option (b) is more general; ~3 LOC.

### m4 — README claim is 30-line max but file is 36 lines

Plan task 7.7 (main.md:580): "Author `stores/tasks/README.md` (terse — 30 lines max — mirrors the layout of `stores/observations/README.md`)."

Actual: `stores/tasks/README.md` is 36 lines (verified via `wc -l`). Trivial — the README content itself is appropriate. Either trim to 30 lines or note the deviation. Optional.

### info — H1 (current_cycle initial value) — plan said 0, actual is NULL — by design

AC7.2 plan literal said "row reads back with `status: planning, current_phase: 0, current_cycle: 0`." The cycle-2 plan-reviewer's H1 hygiene flag asked the executor to interpret this as "current_cycle: 1 on initial add." Neither matches reality.

The actual behavior is: on `add`, both `current_phase` and `current_cycle` are NULL (uninitialized). They are set to 1 only when the `ready → executing` on-state follow-on fires (as part of `submit-plan-review --gate READY`). This is the correct framework semantics — these fields have `actor: framework + auto_increment: true`, so their first non-null value is set by the engine's auto-increment logic during the framework-fired transition.

The executor's deviation note (main.md:1643) correctly identifies this. AC7.2 PASSes against the row-shape that the plan actually intended (planning state, framework-owned fields uninitialized until the engine fires). Treat as an intent-vs-literal documentation gap, not a defect. No action required; the deviation note is sufficient.

---

## Required actions for cycle 2

1. **[M1] Fix `files_changed` shape mismatch.** Split CSV into `Vec<String>` in `compute_submit_execute`; add unit test + render-path integration test.
2. **[M2] Set `at` timestamps on all three list_record sub-records.** Three insertions of `now_iso8601()` in `compute_submit_plan_review`, `compute_submit_execute`, `compute_submit_review`. Add unit tests asserting `at` is present and ISO-8601 shaped.
3. **[m1] Rewrite executor-brief.md.tpl lines 67-72 and 90-95** to remove main.md edits / Status setting. Use only CLI verbs, mirror the framework-aligned shape of plan-reviewer-brief.md.tpl.
4. **[m2] Add four carry-forward unit tests** (P5-m2 open_questions roundtrip, P5-m3 custom submit_targets field, P5-m4 summary/details separation, P6-m2 bundled-sentinel exercise).

## Optional / accept

- **m3 — `start` verb in CLI help.** Recommend option (b): generic `actor: framework` filter in dynamic.rs's transition-registration loop. Future-proof for any new framework transitions in tasks or other workflow stores.
- **m4 — README at 36 lines.** Trim or document.

## Carry-forwards to Phase 8 (binding)

None new. The Phase 7 fixes above must land before Phase 8 (orchestrator skill) begins, because the orchestrator skill exercises submit-execute / submit-review / submit-plan-review end-to-end and would surface M1/M2 immediately as "render output is wrong" / "where's my timestamp."

## Carry-forwards to Phase 8 (informational)

- The `compute_submit_*` handlers all retain hardcoded fallback literals (`unwrap_or("plan")` etc.) for `submit_targets`. Phase 8's orchestrator skill targets the bundled tasks schema, which uses canonical names — so the fallback is never reached. If a future store author defines a custom `submit_targets` map AND a transition path that doesn't match the canonical names, the m2 cycle-2 test (P5-m3 custom submit_targets field) catches the regression. No action in Phase 8.
