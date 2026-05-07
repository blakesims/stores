# Code Review — Phase 7, Cycle 2

- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 2 of max 3
- **Gate:** PASS
- **Status next:** EXECUTING_PHASE_8
- **Issues found this cycle:** 0 critical / 0 major / 0 minor

## Summary

All six cycle-1 REVISE items (2 major, 4 minor) closed cleanly. Each fix verified at three layers: source line, dedicated unit test, and live end-to-end smoke. 297 unit tests pass (288 prior + 9 new), 0 failed. Diff confined to the four files cycle 1 specified plus task main.md. Commit hygiene clean (4 functional + 1 log). PASS recommended; advance to Phase 8.

## Cycle-1 item closure

### M1 — files_changed CSV → JSON array

**Source:** `src/handlers/submit.rs:729-737`

```rust
if let Some(files) = files_changed {
    let files_vec: Vec<Value> = files
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(Value::String)
        .collect();
    executor_obj.insert("files_changed".to_string(), Value::Array(files_vec));
}
```

The CSV-string write that cycle 1 flagged is gone. The replacement is JSON array construction with whitespace-trim + empty-drop semantics matching the `open_questions` list pattern.

**Tests:**
- `m1_files_changed_stored_as_json_array` (submit.rs:2249) — passes `"src/foo.rs,src/bar.rs"`; asserts `files.is_array()`, `arr.len() == 2`, exact contents.
- `m1_files_changed_trims_whitespace_and_drops_empties` (submit.rs:2273) — passes `" a.rs , b.rs , , c.rs "`; asserts length 3 and trimmed contents.

**Live smoke (DB level):**

```sql
SELECT cycles FROM tasks WHERE display_id='T001';
-- → cycles[0].executor.files_changed: ["src/foo.rs", "src/bar.rs"]
```

**Live smoke (render level):** rendered main.md shows

```
- **Files:**
  - `src/foo.rs`
  - `src/bar.rs`
```

The `{{#each this.executor.files_changed}}` template iteration that was broken in cycle 1 now produces a proper bullet list.

### M2 — `at` timestamps on all three list_record sub-records

**Source:** verified at three insertion sites:
- `submit.rs:563` — `compute_submit_plan_review` log_entry_obj.
- `submit.rs:724` — `compute_submit_execute` executor_obj.
- `submit.rs:886` — `compute_submit_review` review_obj_map.

All three call `Value::String(now_iso8601())`. The helper was already in scope (use at submit.rs:37).

**Tests:**
- `m2_plan_review_log_entry_has_at_timestamp` (submit.rs:2296)
- `m2_executor_entry_has_at_timestamp` (submit.rs:2321)
- `m2_review_entry_has_at_timestamp` (submit.rs:2346)

Each asserts `at.is_some()` and ISO-8601 shape (`'T'` and `'-'` both present).

**Live smoke:** rendered main.md `Plan Review`, `Execution Log`, and `Code Review Log` sections each carry `**At:** 2026-04-26T17:16:56Z`.

### m1 — executor-brief.md.tpl CLI-only

**Source:** `stores/tasks/templates/executor-brief.md.tpl` (96 lines).

Critical Actions step 5 (line 71): `stores tasks submit-execute {{display_id}} --summary "..." --commit <sha> --files-changed "a.rs,b.rs"`.
Step 6 (line 72): `stores tasks render {{display_id}}`.
Line 74: explicit `**Do NOT edit main.md directly.** The framework regenerates it from DB rows via render. Status transitions are framework-managed — do NOT set Status manually.`
"When Blocked" (line 95): `stores tasks submit-execute {{display_id}} --summary "BLOCKED: <reason>" --commit <sha-or-none>`.

The cycle-1 drift (instructing the agent to edit main.md and set Status) is entirely removed. Template now mirrors plan-reviewer-brief.md.tpl's CLI-only shape.

### m2 — four carry-forward unit tests

All four landed at submit.rs:2379-2634 with exact contracted names:

- `ac7_p5m2_open_questions_appended_to_plan_review_log_entry` (submit.rs:2379) — passes `vec!["question one", "question two"]`; asserts `plan_review_log[0].open_questions` is array of length 2 with exact contents.
- `ac7_p5m3_submit_targets_consulted_for_field_lookup` (submit.rs:2407) — constructs custom YAML schema with `submit_targets: {submit-execute: my_exec_log}`; calls `compute_submit_execute`; asserts entry written to `my_exec_log` (not `cycles`). This proves the lookup fires — would fail if the code fell back to canonical name.
- `ac7_p5m4_review_summary_and_details_separate_keys` (submit.rs:2531) — calls `compute_submit_review` with summary `"short summary S"` and details `"long detailed report D"`; asserts both stored at distinct keys with `assert_ne!` on values.
- `ac7_p6m2_bundled_sentinel_routes_to_in_memory_template` (submit.rs:2566) — pulls planner template from `BUNDLED_STORE_TEMPLATES`, builds context via `render::build_context`, calls `render::render_template`; asserts "Methodical and thorough" persona text in output. Exercises the in-memory template path that `brief::compute` uses after sentinel detection.

### m3 — framework-actor filter (dynamic.rs)

**Source:** `src/cli/dynamic.rs:183-186`

```rust
// Skip framework-actor transitions — they are engine-fired and must not appear in user-facing help
if transition.actor == Some(crate::schema::actor::Actor::Framework) {
    continue;
}
```

Placed before the `BASE_VERBS` check, which is the right ordering (framework verbs are skipped silently rather than emitting the "collides with a base verb" warning that BASE_VERBS produces).

**Live smoke:** `stores tasks --help | grep -i 'start'` returns nothing. The `start` verb (`ready → executing`) no longer leaks into user-facing CLI output. Generic across all current and future framework verbs.

### m4 — README trimmed to 29 lines

`wc -l stores/tasks/README.md` = 29 (under the 30-line limit). Quick-start preserved; Workflow states + Cycle limits collapsed to one paragraph (line 29).

## Live end-to-end smoke (executed by reviewer)

Tempdir: `/tmp/tmp.5OFkdGm9hK`

```
$ stores init && stores install tasks
$ stores tasks add --invoker ai_with_human --title "Test" --slug "test-task" \
    --done-when "X" --scope-in "y" --scope-out "z"
T001
$ stores tasks next-action T001
  status: planning, next_agent: planner
$ stores tasks submit-plan T001 --plan-from-file …
  status now: plan_review
$ stores tasks submit-plan-review T001 --gate READY --summary "approved"
  status now: executing
$ stores tasks submit-execute T001 --summary "phase 1 done" --commit abc123 \
    --files-changed "src/foo.rs,src/bar.rs"
  status now: code_review
$ stores tasks submit-review T001 --gate PASS --critical 0 --major 0 --minor 0 \
    --summary "ok"
  status now: complete
$ stores tasks render T001
  rendered: tasks/completed/T001-test-task/main.md
```

Final main.md contains: complete plan, plan-review entry with `**At:**`, execution log with `**Files:**` list of two entries and `**At:**`, code-review entry with findings counts and `**At:**`, completion section. Directory move from `tasks/active/` to `tasks/completed/` worked. No errors at any step.

## Discrepancy / drift check

`git diff 84b6385..HEAD --stat`:

```
src/cli/dynamic.rs                           |   4 +
src/handlers/submit.rs                       | 403 ++++++++++++++++++++++++++-
stores/tasks/README.md                       |  15 +-
stores/tasks/templates/executor-brief.md.tpl |  15 +-
tasks/active/T002-tasks-store-v02/main.md    |  30 +-
5 files changed, 446 insertions(+), 21 deletions(-)
```

Exactly the four target files plus the task's own main.md. No unrelated edits, no scope creep.

Commits since cycle 1 (`84b6385`):
- `27a4302` — M1 + M2 fixes + tests
- `aaf2717` — m1 executor-brief
- `bab3e93` — m3 framework-actor filter
- `8891210` — m4 README trim
- `caa7c84` — execution log update

Each functional commit corresponds to one cycle-1 item label; no amends; no force-push.

## What's good

- The reviewer's cycle-1 labels (M1, M2, m1, m2, m3, m4) were honored 1:1 in test names (`m1_files_changed_*`, `m2_*_has_at_timestamp`, `ac7_p5m2_*`, etc.) and in commit subject lines. Future readers can grep cycle-1 → cycle-2 closure trivially.
- The M1 split-trim-filter ordering precisely mirrors the `open_questions` construction site so the codebase has one canonical list-from-CSV pattern instead of two divergent ones.
- The m2 `ac7_p5m3_submit_targets_consulted_for_field_lookup` test deliberately constructs a non-canonical schema (with `my_exec_log` instead of `cycles`) — this is the right answer to cycle 1's critique that hardcoded fallbacks masked the lookup. Without this construction the lookup could be a no-op and the test would still pass against a buggy implementation.
- The m3 fix is generic on `Actor::Framework` rather than special-casing the `start` verb — any future engine-fired transition is hidden automatically.

## Findings this cycle

None. The reviewer's cynical default expectation of 3+ findings did not materialize. Honest assessment: this was a tightly scoped 6-item REVISE cycle with crisp acceptance criteria from cycle 1, the executor delivered all six fixes plus the requested test coverage, and live smoke exercises the marquee path end-to-end. The fix targets are localized (single insert points, single template, single CLI registration loop, single README) so there's no surface area for collateral regressions. The new tests directly assert what cycle 1 asked for, not weaker proxies. Search for plausible defects:

- Hidden status-state effects from M2's new `at` keys on validation? — Walked the validator; `at: timestamp` is declared in the schema's list_record sub-fields, validates as a timestamp, no incremental validator error.
- Race between `now_iso8601()` calls within a single handler? — Each handler builds one record per invocation; monotonic ordering across records not asserted but also not relied upon.
- m3 over-filter (legitimate non-framework verbs hidden)? — `tasks` schema uses framework actor only on `start` (ready → executing). All other verbs (submit-plan, submit-plan-review, submit-execute, submit-review, resume) are user-facing actors and remain in `--help`. Confirmed via live `--help` output: 8 workflow verbs present, no extras filtered.
- README trim losing critical info? — Workflow states sentence + cycle-limit sentence both present (line 29).

PASS recommendation stands.

## Verified actions

- M1 (files_changed JSON array) — closed.
- M2 (at timestamps on 3 sub-records) — closed.
- m1 (executor-brief CLI-only) — closed.
- m2 (4 carry-forward unit tests) — closed.
- m3 (framework-actor filter) — closed.
- m4 (README ≤30 lines) — closed (29 lines).

## Carry-forward

None new. Phase 8 begins clean.
