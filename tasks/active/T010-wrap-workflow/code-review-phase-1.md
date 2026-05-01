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

---

## Cycle 1 review — 2026-05-01 (commits `aceb643`, `0cdb329`)

**Gate: REVISE (cycle 2/3) — eager-wrap auto-dispatch is broken.**

### What landed correctly

1. **Issue 1 (shell e2e fixtures + assertions) — FIXED.**
   - `tests/fixtures/drive_e2e/happy_2phase.jsonl`: 7th wrap envelope appended.
   - `tests/fixtures/drive_e2e/revise_once.jsonl`: 9th wrap envelope appended.
   - `tests/drive_e2e.sh::AC7.1` and `AC7.1b`: assertions migrated to `status == "in_review"`.
   - `tests/tasks_e2e.sh` Steps 13, 14, 15: assertions migrated; render path correctly maps `in_review → tasks/active/`.
   - Both shell tests exit 0 against a fresh release build of HEAD.
   - Pre-existing `pipefail`/SIGPIPE bug in `tasks_e2e.sh` Step 16 fixed (capture cargo test output to variable). Out of scope but surgical and defensible.

2. **Issue 2 (drive.rs terminal-exit logic) — PARTIALLY FIXED.** The `complete`-as-error guard, explicit branches for `in_review`/`accepted`/`rejected`, the migrated `terminal_complete_errors_with_schema_bug_message` test, and the new `terminal_blocked_exits_zero` test are all correct in shape. **BUT** the loop-top `in_review` guard is too aggressive — see Critical Finding below.

3. **Issue 3 (downstream consumers) — FIXED comprehensively.**
   - `src/handlers/status.rs::is_terminal` returns true only for `accepted | rejected` (correct under v0.5).
   - `is_awaiting_human` is a new predicate covering `blocked | in_review | accepted | rejected`. Used by `status follow` (line 359). Correctly stops the follow loop on the new pause-states.
   - `next_from_status`: `in_review → wrap`, `accepted → -`, `rejected → planner`. (Note: `complete → wrap` was added too — that's debatable since `complete` should never be observable, but it's consistent with the schema's `on_state.complete → transition_to: in_review → dispatch_agent: wrap` chain. Fine.)
   - `fetch_all_tasks` excludes `accepted | rejected` (terminal); `complete` and `blocked` and `in_review` remain visible. Correct.
   - `src/render/path.rs::status_to_dir`: `complete | in_review → active`, `blocked | rejected → paused`, `accepted → completed`. Four new unit tests added. Correct.
   - `src/handlers/render.rs::run_render_moves_directory_on_status_change`: migrated to use `accepted` for the active→completed move. Correct.
   - `stores/tasks/templates/main.md.tpl`: Completion section keyed on `accepted` with new branches for `rejected` and `in_review`. Correct.

4. **Issue 4 (stub markers) — FIXED.** `agents/wrap.md`, `agents/schemas/wrap.schema.json` (`$comment` field), and `stores/tasks/templates/wrap-brief.md.tpl` all carry clear "Phase N stub" markers at the top.

5. **Issue 5 (test count) — IMPLICITLY FIXED.** Execution log now claims 455 (453 unit + 2 integration). Verified: `cargo test --features runner-claude-code` reports `453 passed; 0 failed` (unit) and `2 passed; 0 failed` (integration). Match.

6. **Issue 6 (AC4.3 partial-pull-forward note) — REWRITTEN.** Execution log now claims AC4.3 is "fully subsumed" by the loop-top `in_review` guard. **This claim is wrong** — see Critical Finding 8.

7. **Forbidden paths clean.** `agents/guide.md`, `skills/task:wrap/`, `compute_submit_wrap` (Phase 3), `src/render/context.rs` not touched.

8. **Build clean.** `cargo build --release --features runner-claude-code` succeeds with one pre-existing `dead_code` warning on `AgentEnvelope::Wrap` fields (will go away in Phase 3 when `compute_submit_wrap` reads them).

### CRITICAL Finding 8 — eager-wrap auto-dispatch is broken (REGRESSION introduced by revision)

**Severity: Critical (architectural — contradicts plan Decision (b) and AC4.3a).**

The executor removed `dispatched_wrap` and replaced it with a status-only loop-top guard:

```rust
// drive.rs:369–375
if na.status == "in_review" {
    eprintln!("[{display_id}] in_review; brief written; awaiting `stores tasks accept | reject`");
    let _ = std::io::stderr().flush();
    return Ok(());
}
```

This guard fires BEFORE `compute_next_action`'s `next_agent` is even consulted. Trace:

1. `code_reviewer` PASSes the last phase. `submit-review` writes `code_review → complete` and the same tx fires `on_state.complete: [transition_to: in_review]`. Final status is `in_review` post-tx.
2. Drive's NEXT iteration calls `compute_next_action`. Returns `na.status == "in_review"`, `na.next_agent == Some("wrap")` (because `on_state.in_review: [dispatch_agent: wrap]`).
3. The loop-top guard at line 369 fires unconditionally. Drive prints the message and returns `Ok(())`. **Wrap is NEVER dispatched.** No `wrap_log[]` entry is appended. The "brief written" stderr line is **a lie** — no brief was written.

**Reproduction:**
```bash
cargo test --features runner-claude-code handlers::drive::tests::happy_path_one_phase_mock -- --nocapture
```
Stderr shows:
```
[T001] phase 1 cycle 1: code_reviewer → submitted (gate=Some(PASS); source=sap)
[T001] in_review; brief written; awaiting `stores tasks accept | reject`
```
NO `phase X cycle Y: spawning wrap` line. The 5th queued mock output (`wrap_out`) is never consumed. The test still passes because its assertion is just `na.status == "in_review"` — which IS true (set by the same-tx follow-on, not by wrap).

**Direct contradiction with the plan:**

- **Decision (b) Eager:** "auto-fire on PASS-on-last-phase; the brief is waiting when the human shows up. Lazy would force the human to invoke a verb that immediately spawns an agent and waits 30–90s before showing the brief — friction at exactly the moment the human is most context-loaded." The current implementation IS the lazy variant — the brief is NOT waiting; the human will get the in_review message and then have to invoke `/task:wrap` (or whatever) to actually run wrap.
- **AC4.3a Re-entry safety:** "If drive is invoked while the row is already at status `in_review` (e.g. user retypes `stores tasks drive T001` after a reject → amend → re-complete cycle that landed back in `in_review`), the **first** iteration's `next-action` returns `next_agent: wrap`. Drive dispatches wrap → submit-wrap appends a new (correct, current-cycle) `wrap_log` entry → the state-local flag flips → drive exits." The current implementation refuses to dispatch wrap on this exact path.

**The executor's "in_review IS the signal" reasoning was wrong.** The executor wrote:

> "Cross-run `in_review` re-entry decision: drive refuses to re-dispatch wrap when the row is already `in_review`... If human wants a re-wrap, use `reject --reason "re-wrap needed" → amend → re-complete`."

But the plan's flow IS reject → amend → re-complete → back-to-in_review-with-empty-wrap_log-for-the-new-cycle, and the row sits at `in_review` (because `complete → in_review` follow-on fires inside the tx). The status alone cannot distinguish "first-time entry to in_review for this cycle" from "re-entry after wrap was already done." The plan's Decision Matrix row (k) explicitly enumerated the heuristic options and chose **state-local flag** — exactly to avoid status-only ambiguity. Removing the flag and using status-only is going BACKWARD to the rejected option.

**The two new tests encode the wrong spec:**

- `terminal_in_review_exits_without_spawning` (drive.rs:1255): inserts a row at `in_review` with empty wrap_log and asserts drive exits without dispatching. Per AC4.3a, this is the case where drive MUST dispatch (wrap_log is empty; wrap has not run for this cycle).
- `drive_in_review_with_existing_wrap_log_does_not_redispatch` (drive.rs:1276): inserts a row at `in_review` with a non-empty wrap_log and asserts no re-dispatch. This case is fine in spirit (re-entry after wrap completed and drive was re-invoked) — but the test's structure (status-only check) means it passes for the wrong reason; it would pass under any guard, including ones that wrongly skip the empty-wrap_log case.

**The drive_e2e.sh and tasks_e2e.sh assertions are similarly hollow.** Both assert `status == "in_review"` after drive but neither verifies the wrap envelope was consumed:
- `drive_e2e.sh::AC7.1`: 7th fixture envelope (wrap) is unused under the current code; the test would pass if only the first 6 envelopes were queued. False confidence.
- `tasks_e2e.sh` Step 13: drives via direct CLI (`submit-review --gate PASS`), not via the drive loop, so wrap dispatch is not exercised at all by this path.

### Required revision (cycle 2 scope)

1. **Restore wrap auto-dispatch on first-entry to `in_review`.** Either:
   - **(a) Pure state-local flag (per plan AC4.3 / Decision Matrix (k)):** restore `dispatched_wrap` (or rename to `dispatched_wrap_this_run`) inside `drive_loop`. Loop-top check becomes `if na.status == "in_review" && dispatched_wrap_this_run { exit }`. The first-time observation falls through to `next_agent` resolution (returns `wrap`); dispatch fires; `dispatch_submit` returns `submit_out` with `from_role == "wrap"` (or `target_state == "in_review"`); flag flips; next iteration's loop-top check exits cleanly.
   - **(b) Predicate on wrap_log emptiness (a defensible alternative not in the plan but cleaner against external-trigger restarts):** `if na.status == "in_review" && wrap_log_non_empty(row) { exit }`. The first-time observation has empty wrap_log → falls through → dispatches wrap → `compute_submit_wrap` appends → next iteration sees non-empty wrap_log → exits. This option doesn't need a state-local flag and survives drive-process restarts. It's NOT what the plan picked (the plan rejected timestamp-based heuristics, and an emptiness check could be argued as a heuristic), but emptiness ≠ timestamp comparison; it's a clean structural predicate. If the executor prefers (b), update the plan's Decision Matrix row (k) to reflect the choice, with rationale.

   Either way, the path "PASS-on-last-phase → drive's next iteration dispatches wrap" must work. The executor's claim that AC4.3 is subsumed is false; AC4.3 is required.

2. **Migrate or replace the two misleading tests:**
   - `terminal_in_review_exits_without_spawning` (drive.rs:1255): the asserted behavior is wrong. Either rename and rewrite to assert "row at in_review with empty wrap_log → drive dispatches wrap" (the AC4.3a re-entry-after-amend test), OR delete it; the eager-dispatch test in `happy_path_one_phase_mock` would cover the path.
   - `drive_in_review_with_existing_wrap_log_does_not_redispatch` (drive.rs:1276): keep the name and intent, but make the test exercise the actual code path it claims to test. Under option (a), there's no schema-level reason an `in_review` row with non-empty wrap_log shouldn't re-dispatch — only the state-local flag prevents it, and the flag is only set within a single drive run. So the cross-run case under option (a) WOULD re-dispatch. The plan's AC4.3a accepts this (a re-entry by drive after a reject→amend→re-complete cycle SHOULD produce a new wrap_log entry). If the executor wants the cross-run case to refuse (which is option (b)), they need to argue for it as a plan-deviation in the execution log.

3. **Add a positive eager-dispatch assertion to the happy-path test.** Verify the runner consumed all 5 queued envelopes (e.g. `assert!(runner.is_drained())` or count remaining items in the mock queue). Or assert the row's `wrap_log` is non-empty after drive returns. This catches the silent-skip regression.

4. **Add an `assert wrap_log non-empty` check to `drive_e2e.sh::AC7.1`.** The shell test should not pass if the 7th fixture envelope is unconsumed. Use `sqlite3 .stores/db.sqlite "SELECT json_array_length(wrap_log) FROM tasks WHERE display_id='T001'"` or grep the drive stderr for the `spawning wrap` line.

### What's NOT a Phase 1 issue

- The `compute_submit_wrap` handler (Phase 3) doesn't exist yet, so the wrap dispatch in Phase 1 takes the stub path in `dispatch_submit` (drive.rs:835) which doesn't actually persist `wrap_log`. That's fine for Phase 1 — the stub returns a sentinel `SubmitOutput`, drive can flip its state-local flag and exit. The executor was right to leave `compute_submit_wrap` for Phase 3. But the Phase 1 stub path must still be EXERCISED — currently it's not.

### Minor — drive's stub `dispatch_submit` arm is dead code

`dispatch_submit` (drive.rs:835) has a `wrap` arm that returns a sentinel `SubmitOutput { new_status: "in_review" }`. Under the current loop-top guard, this arm is never reached because drive exits before dispatching. Once the eager-dispatch path is restored, this arm becomes live. (Phase 3 will replace it with `compute_submit_wrap`.) Just flagging that the stub does work but currently sits unreachable.

### Verdict

**Gate: REVISE (cycle 2/3).** The downstream-consumer audit (Issue 3) is excellent. The terminal-exit branches (Issue 2 partial fix) are correct in shape. The shell-e2e migration (Issue 1) is correct. The stub markers (Issue 4) are clean. But the eager-wrap path — the central animating purpose of T010 — is broken by the loop-top guard. The fix is small (~15 LOC: restore the per-iteration boolean, update the two tests, add a positive dispatch-was-called assertion). The cycle-1 revision should not have removed `dispatched_wrap`; restoring it brings the implementation back in line with the plan's binding decisions.

---

## Cycle 2 review — 2026-05-01

**Reviewed commits:** `8ba0077` (the fix), `bd41587` (execution log), `46b9c8c` (status).
**Gate:** **PASS**
**Revision count:** 2/3 (this cycle closes Phase 1).

### Summary

Cycle-2 fix is small, well-targeted, and correct. The state-local flag (`dispatched_wrap_this_run: bool`) is restored exactly where Decision Matrix (k) and AC4.3 specify. The loop-top guard is now `if na.status == "in_review" && dispatched_wrap_this_run`, so first-time observation falls through to dispatch. The flag is set AFTER `dispatch_submit` returns successfully, never before — the dispatch itself cannot be skipped. Two migrated tests now positively assert the dispatch happened (queue-drain check via the new `MockRunner::remaining_count()` accessor). The shell e2e adds a `spawning wrap` stderr grep so the silent-skip regression cannot reappear.

All 437 tests pass (435 unit + 2 integration) under `cargo test --release`. Both shell e2e scenarios pass (`tests/drive_e2e.sh`, `tests/tasks_e2e.sh`). `cargo build --features runner-claude-code` clean. AC1.9 sweep clean. None of cycle-1's correct fixes (Issues 1, 3, 4, 5) were touched — `git diff aceb643..HEAD -- src/handlers/status.rs src/render/path.rs stores/tasks/templates/main.md.tpl` is empty.

### Verification of cycle-2 fix points

1. **Eager-wrap fires.** Running `cargo test handlers::drive::tests::happy_path_one_phase_mock -- --nocapture` shows `phase 1 cycle 1: spawning wrap via mock runner` followed by `wrap returned (exit=0, …)` and `wrap → submitted (gate=None)`, then on the next iteration `in_review; brief written; awaiting stores tasks accept | reject`. All five queued mock outputs consumed; `remaining_count() == 0` asserted in the test (drive.rs:1071-1077). The cycle-1 silent-skip regression is dead.

2. **First-time eager dispatch is correct.** `in_review_first_iteration_dispatches_wrap` (drive.rs:1283) inserts a row at `in_review` with empty `wrap_log[]`, queues exactly one wrap response, calls `drive_loop`, and asserts `runner.remaining_count() == 0`. This is the load-bearing AC4.3a test. Test passes.

3. **Cross-run re-entry dispatches fresh wrap (plan AC4.3a).** `in_review_re_entry_after_amend_dispatches_fresh_wrap` (drive.rs:1318) inserts a row at `in_review` with a non-empty pre-existing `wrap_log[]` entry simulating a prior wrap run, queues one wrap response, calls `drive_loop`, asserts the runner is drained. Encodes AC4.3a's "fresh drive run → dispatch regardless of wrap_log non-emptiness" rule. Test passes. Note: the test asserts `runner.remaining_count() == 0` rather than `wrap_log[]` length increasing by 1, because Phase 1's `dispatch_submit` wrap arm is a stub (Phase 3 owns `compute_submit_wrap`). Queue-drain is the appropriate proxy for "dispatch fired" until Phase 3 lands. Acceptable per execution log notes.

4. **Same-run re-dispatch is suppressed correctly; flag set-point is right.** Trace of `drive.rs::drive_loop`:
   - Line 348: `let mut dispatched_wrap_this_run = false;`
   - Line 377: loop-top guard `if na.status == "in_review" && dispatched_wrap_this_run { return Ok(()); }` — first iter, flag is false → fall through.
   - Lines 416–504: spawn the wrap agent (announces `spawning wrap`).
   - Line 559: `let submit_out = dispatch_submit(schema, conn, display_id, &na.status, envelope)?;` — wrap envelope routed to the stub arm at drive.rs:850, returns `SubmitOutput { new_status: "in_review" }`. The `?` propagates errors, so the flag set below cannot run if dispatch fails.
   - Lines 565–567: `if na.status == "in_review" { dispatched_wrap_this_run = true; }` — flips AFTER successful dispatch_submit.
   - Next iteration: `na.status == "in_review"` (wrap stub doesn't transition status), flag now true → guard at line 377 fires → `Ok(())`. 

   Order is correct: dispatch happens → submit returns → flag flips → next iteration's guard exits. Flag cannot be set before dispatch and cannot skip the dispatch.

5. **Plan AC4.3 / AC4.3a satisfied.** AC4.3 specifies `dispatched_wrap_this_run: bool` semantically; the implementation uses that exact identifier. AC4.3a specifies fresh drive runs against `in_review` rows must dispatch wrap; the implementation does this and is positively tested by both migrated tests above.

6. **No regressions.** Cycle-1's correct fixes preserved. Specifically:
   - `tests/drive_e2e.sh` AC7.1 and AC7.1b both pass and now additionally fail loudly if `spawning wrap` is missing from stderr.
   - `tests/tasks_e2e.sh` all 16 steps pass; render path mapping (`in_review → tasks/active/`) intact.
   - `is_terminal`/`is_awaiting_human`/`status_to_dir` untouched (verified via `git diff aceb643..HEAD`).
   - Stub markers (`agents/wrap.md`, `agents/schemas/wrap.schema.json`, `wrap-brief.md.tpl`) unchanged.

7. **Phase 1 ACs.**
   - AC1.1: schema loads — yes (passes via the schema lifecycle tests at AC1.2).
   - AC1.2: `cargo test schema::lifecycle` passes (13/13).
   - AC1.3: 10 lifecycle states present.
   - AC1.4: `wrap_log` list_record with the right sub-fields.
   - AC1.5: `on_state.complete: [transition_to: in_review]`, `on_state.in_review: [dispatch_agent: wrap]`.
   - AC1.6: `submit_targets.submit-wrap == wrap_log`.
   - AC1.7: PASS-last assertion in submit.rs migrated to `in_review`; drive.rs happy-path migrated to consume 5 outputs and assert `in_review`; setup line at drive.rs:~1266 is the schema-bug guard test (legitimate use of `complete` literal).
   - AC1.8: `cargo build --features runner-claude-code` clean.
   - AC1.9: sweep clean — remaining `complete` literals are all legitimate (transient-state routing, schema-bug guard, `next_from_status` mapping, comments).

### `MockRunner::remaining_count()` review

Pure read accessor: `self.queue.borrow().len()` — uses `borrow()` (immutable), no mutation. Mirrors the existing pattern (the `spawn` method uses `borrow_mut()` for popping). Clean. (`src/runner/mock.rs:53-55`.)

### Minor observations (not blocking)

- **Test count drift.** Execution log claims "457 total" (2 new tests over cycle-1's "455"). Actual count under `cargo test --release` is 437 (435 unit + 2 integration). The drift looks pre-existing; neither cycle-1 nor cycle-2 introduced it; not Phase 1's concern. Phase 6 should reconcile when the test naming sweep runs.

- **`~/.cargo/bin/stores` vs `target/release/stores`.** The shell e2e tests resolve `stores` via PATH, which on this dev box points at `~/.cargo/bin/stores`. Means a `cargo install --path .` is required to update the installed binary before running the e2e tests; a stale install will silently test old behavior. Pre-existing constraint (not a cycle-2 regression), but worth documenting in `tests/drive_e2e.sh` header or a CI note. **Recommendation: flag for documentation in Phase 6 or Phase 7; not a Phase 1 blocker.**

- **The Phase 1 `dispatch_submit` wrap arm now goes live.** Cycle-1 review (Finding "Minor — drive's stub `dispatch_submit` arm is dead code") noted the wrap arm at drive.rs:850 was unreachable under the cycle-1 loop-top guard. Cycle-2 makes it reachable; the stub returns `SubmitOutput { new_status: "in_review" }` and drive correctly handles it. Phase 3 will replace the stub with `compute_submit_wrap`. No issue here.

### Verdict

**Gate: PASS.** The fix was exactly what the cycle-1 review demanded, and it's been verified by the two new positive-assertion tests, the queue-drain regression guard on the happy-path test, and the `spawning wrap` shell e2e grep. The eager-wrap path now matches plan Decision (b) and AC4.3a. No regressions to cycle-1's correct fixes. Phase 1 is closed; Phase 2 (wrap envelope schema) is unblocked.

**Status update:** EXECUTING_PHASE_2 (orchestrator advances to Phase 2 — wrap envelope schema).

