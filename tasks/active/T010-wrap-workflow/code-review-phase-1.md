# T010 Phase 1 — Code Review

**Reviewer:** code-reviewer
**Phase:** 1 (Lifecycle schema extension)
**Commit reviewed:** `9aaef2d` (+ orchestrator commit `3e4b53c` for SHA recording)
**Date:** 2026-05-01
**Gate:** **REVISE** (substantial — return to executor)
**Revision count:** 0/3

---

## Summary

The schema layer landed correctly: 10 lifecycle states, four new transitions, `wrap_log` `list_record`, `on_state.complete` follow-on, `submit-wrap` registered. The executor correctly identified the actor mismatch in the plan (`actor: framework` not `ai_autonomous` for the on-entry follow-on, because `fire_on_entry_follow_ons` resolves transitions by the framework actor) and corrected it. `compute_submit_review` is correctly extended with `fire_on_entry_follow_ons` so PASS-on-last-phase advances `complete → in_review` in the same tx — confirmed by direct CLI test (status reads `in_review`).

All 446 unit tests pass. `cargo build --features runner-claude-code` succeeds.

**However, two real regressions and one downstream-consumer audit gap mean Phase 1 is not deliverable as-is.** The grep sweep enumerated in AC1.7/AC1.9 was scoped to Rust source; it missed the e2e shell tests and the render template that key on `status == "complete"`. Those break under the new schema, and `tests/drive_e2e.sh::AC7.1` now fails outright.

The fixes are localized and well within the executor's reach without re-planning. Recommending **REVISE** with explicit revision scope, not FAIL.

---

## What landed correctly

1. **Schema YAML (AC1.1, AC1.3, AC1.4, AC1.5, AC1.6).** `lifecycle.states` has exactly the 10 expected states. Four new transitions present with the correct `actor` values (`framework` for `request_review`, `human` for `accept`/`reject`, `ai_with_human` for `amend`). `wrap_log` is a `list_record` with all six expected sub-fields and the four list-typed sub-fields use `{list: text}`. `on_state.complete == [transition_to: in_review]` and `on_state.in_review == [dispatch_agent: wrap]`. `submit_targets.submit-wrap == wrap_log`. `briefing_templates.wrap` registered.

2. **AC1.2 — schema lifecycle tests pass.** All 13 tests in `schema::lifecycle::tests` pass. The `validate_transition_ambiguity` test accepts the new transitions (no two unguarded transitions share `(from, verb)` — no conflict with the existing `executing → ?` and `code_review → ?` verbs).

3. **AC1.7 (in part) — Rust-side test migrations.** `submit.rs::ac5_3_submit_review_pass_last_phase_completes` correctly migrated to assert `out.new_status == "in_review"`. `drive.rs:~990 happy_path_through_one_phase` correctly extended (5-element `MockRunner` queue, `wrap_fixture_json` helper added, assertion now `"in_review"`). `WF_SCHEMA_YAML` fixture in `submit.rs::tests` correctly mirrors the new lifecycle.

4. **AC1.8 — release build succeeds** (`cargo build --features runner-claude-code` and plain `cargo build --release`).

5. **AC1.9 (in part) — Rust source sweep clean.** No `"complete"` literal terminal-status assertion in `src/handlers/submit.rs`, `src/handlers/transition.rs`, or `src/handlers/drive.rs` test bodies. The remaining hits are: comments/setup lines (still valid since `complete` remains a real lifecycle state mid-tx), `drive.rs:351` control-flow check (see Finding 1 — this hit is NOT valid post-Phase-1), and `status.rs::is_terminal` (Finding 3).

6. **The plan's actor mismatch caught and corrected.** Plan said `complete → in_review` is `ai_autonomous`; executor confirmed via inspection of `ready → executing` (also `framework`-actor follow-on) that the on-entry follow-on machinery only matches `actor: Framework`. Schema YAML correctly uses `actor: framework`. This is a real correction, surfaced clearly in the execution log.

7. **`compute_submit_review` follow-on wiring (semantics).** `fire_on_entry_follow_ons` is called within the same tx as `submit-review`'s `write_status_and_fields`. Re-read of `final_status` correctly observes `in_review` post-follow-on. PASS-non-last-phase still works (the follow-on for `executing` has `dispatch_agent: executor`, no `transition_to`, so the existing chain is unaffected). The change is idempotent w.r.t. non-PASS gates: REVISE writes `executing` (no `transition_to`); FAIL writes `blocked` (no `transition_to`). Good.

8. **Bundled-agent registration parity.** `BUNDLED_AGENTS` and `BUNDLED_AGENT_SCHEMAS` count test now expects 6 and the role-name set equality holds. `wrap-brief.md.tpl` registered in `BUNDLED_STORE_TEMPLATES`. `submit-wrap` added to `SUBMIT_VERBS`.

9. **Forbidden-paths check clean.** Phase 1 did NOT touch `agents/guide.md`, `skills/task:wrap/`, `tests/drive_e2e.sh`, or `src/render/context.rs`. (Those four paths are reserved for Phases 4–6.)

---

## Findings

### Finding 1 — `tests/drive_e2e.sh::AC7.1` fails outright (REGRESSION)

**Severity:** Critical (broken e2e test in committed code).

`tests/drive_e2e.sh` line 78 asserts `[[ "$STATUS" == "complete" ]]` after the drive loop completes. Under the new schema, drive walks `... → code_review → complete → in_review` (the follow-on chain advances inside the `submit-review` tx) and then drive's loop dispatches the `wrap` agent against `in_review`. The fixture `tests/fixtures/drive_e2e/happy_2phase.jsonl` only has 4 envelope items (planner, plan-reviewer, executor×2, code-reviewer×2 = 6 envelopes; but no wrap envelope). So the mock runner exhausts and drive errors with `mock runner: response queue exhausted (role=wrap)`, and the row's final status is `in_review`, not `complete`.

**Reproduction:**
```bash
cargo build --release
PATH="target/release:$PATH" bash tests/drive_e2e.sh
# → FAIL: AC7.1: expected status=complete; got: in_review
```

**Why the AC1.7/AC1.9 sweep missed it:** the sweep was scoped to `src/handlers/submit.rs src/handlers/drive.rs src/handlers/transition.rs`. Shell-script tests under `tests/` were not grep'd. The plan said `tests/drive_e2e.sh` changes are Phase 6 territory — but Phase 1's lifecycle change IS what breaks it; deferring the fix to Phase 6 leaves the repo in a state where a CI-level e2e test is red.

**Required fix (Phase 1 scope):** Either (a) extend `happy_2phase.jsonl` with a 7th wrap envelope and update AC7.1 to assert `status == "in_review"` (the natural post-drive state under the new schema), OR (b) update both `drive_e2e.sh` AC7.1 and AC7.1b to assert the wrap-exit message and `status == "in_review"`. Option (a) is closer to the existing test shape but requires touching `tests/drive_e2e.sh` (which the plan reserved for Phase 6); option (b) is the same modification scope. Either way, the test must pass on a fresh checkout of HEAD.

`tests/tasks_e2e.sh` has the symmetric break at lines 290 and 313 — `[[ "$STATUS" == "complete" ]] || fail "PASS phase 2: expected complete; got: $STATUS"` and `echo "$FINAL" | grep -q "complete" || fail "final status not complete"`. Direct CLI submission via `submit-review --gate PASS` on the last phase produces `status=in_review` (verified by isolated CLI walk). Both assertions need migration to `in_review`.

### Finding 2 — `src/handlers/drive.rs:351` hard-codes `complete` as terminal exit (REGRESSION risk)

**Severity:** Major (correctness contradiction with schema).

```rust
// drive.rs:350–355
// Terminal: complete
if na.status == "complete" {
    eprintln!("[{display_id}] status=complete; drive finished");
    let _ = std::io::stderr().flush();
    return Ok(());
}
```

Under the new schema, `complete` is a transient state (the follow-on advances to `in_review` inside the tx). In the happy path (PASS-on-last-phase), drive never sees `na.status == "complete"` because the follow-on already fired. So this branch is effectively dead code in the happy path.

**But:** the existing test `terminal_complete_exits_without_spawning` (line ~1213) inserts a row at `complete` directly and asserts drive exits. This passes under Phase 1 because the `na.status == "complete"` exit-branch fires before the on-entry follow-on can be invoked (follow-ons fire only inside `submit` tx, not from `next-action` observation).

This means: a row that somehow lands at `complete` outside a submit tx (e.g. manual SQL surgery, an old DB pre-migration) gets stuck — drive would exit treating it as terminal, but the schema says it should advance to `in_review`. The plan's Decision (e) ratified `complete` as transient: "the row never sits in it (the follow-on fires within the same tx)." That's true for the new path, but drive should ideally either (a) be aware that `complete` is now transient and trigger the on-entry follow-on itself, or (b) at least change the exit message from "drive finished" to "stuck at complete; expected on-entry follow-on to advance to in_review — manual investigation needed."

**Recommended fix (Phase 1 scope):** rewrite the terminal-state check to recognize `accepted` and `rejected` as new terminals, and either delete the `complete` branch OR change it to a warning. The existing `terminal_complete_exits_without_spawning` test should be migrated to assert the new behavior or removed (its premise — `complete` as terminal — is no longer schema-true). This is also the natural place to add an `accepted` terminal-exit branch which does NOT exist anywhere yet (see Finding 3).

### Finding 3 — Downstream consumers not audited (`is_terminal`, `next_from_status`, `status_to_dir`, `main.md.tpl`)

**Severity:** Major (multiple silent inconsistencies with the new schema).

The plan acknowledged the audit need ("the plan says state additions are 'mostly additive' but flags that downstream consumers need an audit"), but the executor did not perform that audit in Phase 1. Concrete violations:

1. **`src/handlers/status.rs:128 is_terminal`:**
   ```rust
   fn is_terminal(status: &str) -> bool {
       status == "complete" || status == "blocked"
   }
   ```
   Wrong on both halves under the new schema. `complete` is transient (false-positive — `status follow` would exit prematurely on a row mid-follow-on). `accepted` and `rejected` are not recognised (false-negative — `status follow` on an accepted task loops forever). The correct definition under v0.5: `status == "blocked" || status == "accepted"` (rejected is also kind-of-terminal but `amend` reopens it; that's a defensible question).

2. **`src/handlers/status.rs:115 next_from_status`:**
   ```rust
   match status {
       "planning" => "planner",
       "plan_review" => "plan-reviewer",
       "ready" | "executing" => "executor",
       "code_review" => "code-reviewer",
       "complete" => "-",
       "blocked" => "-",
       _ => "?",
   }
   ```
   `in_review`, `accepted`, `rejected` all return `"?"`. Status frames will print `next=?` for these states. Should be `in_review` → `wrap`, `accepted` → `-`, `rejected` → `planner` (since `amend` re-opens to `planning`).

3. **`src/render/path.rs:29 status_to_dir`:**
   ```rust
   match status {
       "planning" | "plan_review" => "planning",
       "ready" | "executing" | "code_review" => "active",
       "blocked" => "paused",
       "complete" => "completed",
       _ => "active",
   }
   ```
   `in_review`, `accepted`, `rejected` all fall through to `"active"`. `accepted` should map to `completed` (the task is done). `in_review` is debatable (`active` is OK for v0.5 — the row is awaiting human action but not "complete" yet; whether to put it under `completed` or `active` or a new `in-review` directory is a UX question, not blocking). `rejected` is also debatable (it re-opens to planning via amend; could stay in `active` or move to `paused`). At minimum `accepted` MUST map somewhere other than `active`.

4. **`stores/tasks/templates/main.md.tpl:116`:**
   ```handlebars
   {{#if (eq status "complete")}}- **Completed:** {{updated_at}} ...
   {{else}}_Not yet complete._
   {{/if}}
   ```
   Renders `_Not yet complete._` for `accepted` rows. The Completion section needs to also recognise `accepted` (and probably `in_review` deserves its own state-aware rendering).

**Recommended fix (Phase 1 scope, since this is "schema additivity"):** audit and update `is_terminal`, `next_from_status`, `status_to_dir`, and `main.md.tpl` to recognise the three new states. Each is a small mechanical change. None requires re-planning. Adding a unit test per audit point (e.g. `is_terminal_recognises_accepted`, `status_to_dir_accepted_maps_to_completed`) gives Phase 6 a head-start and prevents future regressions.

### Finding 4 — Scope creep beyond Phase 1's "Files to modify"

**Severity:** Minor (the executor explained why and the deviations are defensible).

Phase 1 plan listed only `stores/tasks/schema.yaml` and `src/handlers/submit.rs` (test fixture) as files to modify. Phase 1 actually modified seven additional files:
- `src/handlers/drive.rs` (Phase 2 + Phase 4 territory: `AgentEnvelope::Wrap`, `dispatch_submit` stub, `dispatched_wrap` exit branch, `wrap_fixture_json`).
- `src/cli/agents.rs` (Phase 2 + Phase 4 territory: `BUNDLED_AGENT_SCHEMAS`, `BUNDLED_AGENTS` registration; count tests bumped 5 → 6).
- `src/cli/dynamic.rs` (Phase 4 territory: `BUNDLED_STORE_TEMPLATES`).
- `src/schema/workflow.rs` (Phase 3 territory: `SUBMIT_VERBS`).
- `agents/wrap.md`, `agents/schemas/wrap.schema.json`, `stores/tasks/templates/wrap-brief.md.tpl` (all Phase 2/4 territory).

The execution log explains why each deviation was unavoidable (schema validation requires `briefing_templates` per `agent_roles`; bundled-agent set-equality test fails without registration; drive panics without an envelope variant for the wrap dispatch in tests). All defensible. The main concern is **forward compatibility with Phase 2's planned schema rewrite**:

- The Phase 1 stub `wrap.schema.json` is essentially fully-formed but missing the `reasoning` slot that Phase 2 plan-text says will land. With `additionalProperties: false`, Phase 2's addition of `reasoning` is a real schema edit, not a no-op. Recommend the executor add a comment block at the top of `wrap.schema.json` saying "Phase 1 stub — Phase 2 will add `reasoning` slot" so a future executor doesn't mistake the file for finalized. Same for `agents/wrap.md` (a placeholder, not a finished prompt) and `stores/tasks/templates/wrap-brief.md.tpl` (a placeholder template).
- The Phase 1 dispatch-stub for `AgentEnvelope::Wrap` in `drive.rs` returns a sentinel `SubmitOutput` and exits drive. Phase 3 will replace it with `compute_submit_wrap` which actually persists `wrap_log[]`. The current behavior — dispatch exits drive without persisting anything — is OK as a stub but means a Phase 1 wrap envelope is lost (the `executive_summary` etc. never make it to the DB). The happy-path test passes because the test only asserts `na.status == "in_review"`, not that `wrap_log[]` was populated. This is a known consequence of pulling forward Phase 2/3 territory; just flagging.

**Recommended action (low effort):** add stub-marker comments to the three stub files (`agents/wrap.md`, `agents/schemas/wrap.schema.json`, `stores/tasks/templates/wrap-brief.md.tpl`) so Phase 2/4 executors don't mistake them for done.

### Finding 5 — Test count claim is wrong (Notes section)

**Severity:** Trivial.

Execution log says "All 430 tests pass." Actual count: 446 unit tests + 2 integration tests = **448 tests pass**. Pre-Phase-1 baseline at commit `22d0180`: also 446 + 2 = 448 (verified via `git worktree`). So Phase 1 added zero new tests. The plan's AC1.7 only specifies test migrations (no new tests), so this is consistent with plan intent — but the executor's claim of "430" is a stale number. Update the execution log to "446 unit tests + 2 integration tests pass; zero new tests added (per plan; Phase 6 owns new test coverage)."

### Finding 6 — `dispatched_wrap` flag is the Phase 4 design pulled forward

**Severity:** Minor (clean implementation, but counts as scope creep).

`drive.rs:514–528` introduces a per-iteration boolean (`dispatched_wrap`) and an exit branch when a wrap envelope is dispatched. This is essentially the AC4.3 state-local flag mechanism that Phase 4 was supposed to introduce, with the variable name slightly different (`dispatched_wrap` vs the AC's `dispatched_wrap_this_run`). The executor's comment correctly notes "Phase 4 adds state-local flag; for now a per-iteration boolean suffices since Phase 1 has no loops that could re-enter in_review within a single drive run."

This is correct in scope (Phase 1 cannot make the happy-path test pass without an exit branch — drive would otherwise loop forever once `na.status == "in_review"`). Fine. But Phase 4 plan AC4.3 is now partially-already-implemented; that phase's work shrinks to (a) fixing variable name to match plan AC4.3 (`dispatched_wrap_this_run`), and (b) adding the AC4.3a re-entry-after-amend test. Recommend the executor flag this in the execution log so Phase 4 estimate gets adjusted.

### Finding 7 — No test directly exercises the new transitions (defensible scope)

**Severity:** Trivial (per plan intent).

No unit test resolves `find_transition` against `accept`, `reject`, `amend`, or `request_review` directly. AC1.2 only requires the ambiguity validator pass (which it does — these are unguarded but no two share `(from, verb)`). The plan defers transition-resolution tests to Phase 6 (transition.rs::tests). This is consistent with plan intent. Adding a smoke test for `find_transition("complete", "request_review")` returning the framework transition would be cheap and would catch any future schema mutation that accidentally breaks the follow-on chain — but it's not required by Phase 1. Optional improvement.

---

## DONE_WHEN propagation check

The full task DONE_WHEN repeated in the user's review prompt: "New `wrap` agent + envelope schema; `tasks/schema.yaml` lifecycle extended with `in_review`/`accepted`/`rejected` and the four new transitions (with `actor: human` on accept/reject); `executive_summary` persisted via `wrap_log` list_record; `agents/guide.md` wrap-mode; `/task:wrap` skill rewrite; drive auto-fires `request_review` on PASS-on-last-phase via state-local flag; end-to-end fixture passes; CLI-level actor enforcement test passes."

Phase 1 is the schema-layer foundation, NOT the full DONE_WHEN. Verifying just the Phase 1 deliverables (not the whole task):

| Phase 1 sub-deliverable | Status |
|---|---|
| Lifecycle states `in_review`, `accepted`, `rejected` added | ✓ |
| Four new transitions (`request_review` framework, `accept`/`reject` human, `amend` ai_with_human) | ✓ |
| `wrap_log` `list_record` field with 6 sub-fields | ✓ |
| `on_state.complete → transition_to: in_review` | ✓ |
| `on_state.in_review → dispatch_agent: wrap` | ✓ |
| `submit_targets.submit-wrap = wrap_log` | ✓ |
| Existing `"complete"` terminal-status assertions migrated to `"in_review"` | ✓ in Rust, ✗ in shell e2e tests (Finding 1) |
| Schema-additivity audit on downstream consumers | ✗ (Finding 3) |
| All tests pass | ✗ (Finding 1: drive_e2e.sh AC7.1 fails) |

---

## Revision scope (substantial — return to executor)

The executor needs to:

1. **Fix `tests/drive_e2e.sh::AC7.1` and `tests/tasks_e2e.sh::Step 13` + `Step 15`** so they pass under the new schema. Either migrate assertions to `in_review` OR extend the JSONL fixture with a wrap envelope and migrate to `in_review`. The plan reserved `tests/drive_e2e.sh` for Phase 6, but Phase 1's lifecycle change makes the existing AC7.1 red as-of the Phase 1 commit. This must land in Phase 1.

2. **Audit and update downstream consumers** (Finding 3): `is_terminal`, `next_from_status`, `status_to_dir`, `main.md.tpl`. Each is a small mechanical match-arm extension. Add unit tests where straightforward.

3. **Update `drive.rs:351 na.status == "complete"` exit-branch** (Finding 2) to either trigger the on-entry follow-on, or change to a warning that `complete` should not be observable outside an in-flight tx. Add a unit test for the new behavior or migrate `terminal_complete_exits_without_spawning`.

4. **Mark stub files** (Finding 4) with an in-file comment indicating Phase 1 stub status so Phase 2/4 don't accidentally treat them as finalized.

5. **Correct test count claim** in the execution log (Finding 5): 448 (446 + 2), not 430.

6. **Optional but recommended:** flag in the execution log that the AC4.3 state-local flag (`dispatched_wrap` boolean in `drive.rs`) is partially implemented in Phase 1, so Phase 4 estimate shrinks (Finding 6).

None of these requires re-planning. They are all scope-tightening fixes consistent with the existing plan, especially Decisions (e) and (k). Estimated revision effort: ~50–100 LOC across 4–6 files plus 4–6 small unit tests.

---

## Recommendation

**Gate: REVISE (substantial)** — return to executor with the revision scope above. Counts as revision cycle 1/3.

The schema layer is solid. The migration is genuinely subtle and the executor caught the actor mismatch correctly. The misses are all in "downstream of schema" territory that the plan acknowledged needed audit but didn't enumerate. Now we know what the audit surface is.
