# Phase 5 Code Review — Cycle 2

- **Gate:** PASS
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 2 of max 3
- **Status next:** EXECUTING_PHASE_6

## Verdict

PASS. Cycle 2 cleanly closes the cycle-1 critical (C1: resume bypass) and both major test-coverage gaps (M1: AC5.13 lock-held; M2: AC5.11 atomic boundary) without regressions. The minor m1 (dead `GuardExpr` re-export) is removed. The three remaining minors from cycle 1 (m2 `--open-questions-from-file`, m3 `submit_targets` lookup, m4 `--details-from-file`/`--summary` conflation) are explicitly deferred to Phase 7 with binding plan-review requirements recorded in main.md and below.

263 tests pass (259 prior + 4 new); all 13 e2e steps green. Diff scope tight (3 source files + main.md). Commit hygiene clean: `17ab325` (fix) + `4c4fee0` (execution-log update); no amends, no force-push.

This is cycle 2 of max 3. Finding zero new issues on a focused 4-fix cycle is plausible: the cycle-1 review enumerated specific code paths and acceptance criteria, and the executor implemented them verbatim. I re-probed each fix via test runs AND direct code reading, and re-read the dispatch caller to confirm it is genuinely a thin caller. No drive-by changes outside the fix scope.

---

## C1 verification — `compute_resume` lands the production resume path

### Code path

`src/cli/dispatch.rs:106-110` — confirmed reduced to:

```rust
Some(("resume", sub)) => {
    let display_id = sub.get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");
    handlers::submit::run_resume(schema, &conn, display_id, invoker)?;
}
```

Zero safety logic in dispatch. The cycle-1 inline block (lines 106-132 prior to this commit) is deleted in `17ab325`.

`src/handlers/submit.rs:987-1057` — `compute_resume` follows the 11-step pattern verbatim:

1. `require_workflow(schema, "resume")` — guards against non-workflow stores.
2. `let tx = conn.unchecked_transaction()` — open tx.
3. `acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?` — step 2 lock acquisition (NOT in dispatch).
4. `read_row(schema, &tx, display_id)?` — step 3 row read.
5. State-machine check: `current_status != "blocked"` bails with informative error.
6. Build empty diff `let diff: EntryMap = BTreeMap::new()` — resume has no user-supplied data.
7. **Validator pass**: `validate::validate(schema, &existing, Op::Transition("resume".to_string(), diff), invoker)` — this is THE production actor-enforcement path. The validator's `check_transition_actor` (validate/actor.rs:51-71) reads the resume transition's declared `actor: ai_with_human` (defined at submit.rs:1161-1164's inline fixture and at tests/fixtures/workflow_minimal/schema.yaml:81-84) and rejects mismatched invokers via `actor_allowed` (validate/actor.rs:82-91, where `AiWithHuman` requires invoker ∈ {Human, AiWithHuman}; `AiAutonomous` does NOT satisfy).
8. `fw_fields = {current_cycle: 1}`, `txt_fields = {blocked_reason: ""}` — clears the stale 4th-revise reason.
9. `write_status_and_fields(&tx, ..., "ready", ..., &fw_fields, &txt_fields)?` — step 8.
10. `fire_on_entry_follow_ons(&tx, schema, display_id, row_id, "ready")?` — step 9. Per `compute_on_entry_framework_fields` (submit.rs:381-422), `current_phase` is preserved when already > 0 (resume path); only initial-plan-approval path sets it to 1.
11. `release_lock(&tx, &schema.name, display_id)?` — step 10.
12. `tx.commit()?` — step 11.

### Test coverage

Three tests landed:

**`ac5_14_blocked_to_ready_recovery`** (replaced; submit.rs:1959-2005). Drives `compute_resume(&schema, &conn, "WF001", Actor::AiWithHuman)` directly (production code path; cycle 1's version called raw helpers). Asserts: `out.new_status == "executing"`, status==executing, current_phase==1 (UNCHANGED — resume preserves), current_cycle==1 (RESET), `blocked_reason` empty (cycle-1 critical fix verified), `claimed_by` NULL post-commit, cycles audit trail length==4 preserved.

**`ac5_14_resume_actor_mismatch_rejected`** (submit.rs:2012-2034). Calls `compute_resume(..., Actor::AiAutonomous)` against a blocked row. Per validate/actor.rs:62-67's format string `transition '{verb}' requires actor '{required}'; invoker is '{invoker}'`, the produced error contains BOTH "ai_with_human" AND "resume" verbatim — the test asserts both substrings via `msg.contains(...)`. Also asserts post-error DB state is unchanged (status still blocked) and `claimed_by` is NULL (the tx was rolled back when the validator returned Err, releasing the acquire_lock side-effect).

**`ac5_14_resume_acquires_lock`** (submit.rs:2040-2065). Pre-claims `WF001` as `other-agent` via raw UPDATE, then calls `compute_resume(..., Actor::AiWithHuman)`. The error originates in `acquire_lock` (submit.rs:90-101) which bails with `row {display_id} is claimed by '{holder}' since {held_at}; retry after 5 minutes`. Test asserts `msg.contains("other-agent") || msg.contains("claimed")` — both are present in the actual error string. Status remains blocked.

All three pass on `cargo test resume`.

---

## M1 verification — AC5.13 mid-tx lock probe is real

`ac5_13_lock_held_during_follow_on` (submit.rs:2074-2128) reproduces the resume/submit-plan-review READY-path sequence on a live tx with three lock probes:

1. **After acquire_lock** (line 2092): `tx.query_row("SELECT claimed_by FROM wf_tasks WHERE display_id = 'WF001'", [], |r| r.get(0))` — asserts `Some("ai_autonomous")`.
2. **BETWEEN `write_status_and_fields` and `fire_on_entry_follow_ons`** (line 2105): same `tx.query_row` — asserts `Some("ai_autonomous")`. This is the load-bearing checkpoint: a future regression that accidentally released the lock inside `fire_on_entry_follow_ons` would be caught here.
3. **After `fire_on_entry_follow_ons`, before `release_lock`** (line 2117): same `tx.query_row` — asserts `Some("ai_autonomous")`.

After explicit `release_lock(&tx, ...)` and `tx.commit()`, a separate `read_text(&conn, "claimed_by")` asserts NULL and final status is `executing`.

**Critically:** all three mid-tx probes use the SAME `tx` handle (not `conn`, not a separate connection, not post-commit). This proves the lock value the writes themselves see between steps 8 and 9 — the exact contract AC5.13 requires.

The cycle-1 review's lighter-touch acceptance ("same-connection probe is acceptable") is met. The "second-observer" version is unnecessary because the failure mode being guarded against is "the writes between steps 8 and 9 see a released lock", which is observable from the same tx.

---

## M2 verification — handler-path rollback proves Phase 5 atomicity, not just SQLite

`ac5_11b_handler_path_validator_failure_rolls_back` (submit.rs:2137-2178) was the lighter-weight option from cycle 1's "Fix" alternatives, and it lands cleanly:

1. Inserts a row in `executing` state, phase 1 cycle 1.
2. Captures pre-call state: status, current_phase, current_cycle, cycles length, claimed_by.
3. Calls `compute_submit_execute(&schema, &conn, "WF001", "attempted summary", Some("abc"), None, None, Actor::AiWithHuman)`.
4. The `submit-execute` transition declares `actor: ai_autonomous` (visible in submit.rs's inline fixture). Per `actor_allowed` (validate/actor.rs:82-91), `Actor::AiAutonomous` requirement is satisfied ONLY by `Actor::AiAutonomous` invoker. `Actor::AiWithHuman` does NOT satisfy. The validator's `check_transition_actor` produces an Err.
5. The handler's `?` propagation surfaces the validator failure as Err from `compute_submit_execute` BEFORE `tx.commit()`.
6. The test asserts the error message contains "submit-execute" or "actor" or "validation" (covers the format-string variants). Then asserts post-call DB state IDENTICAL to pre-call — including `claimed_by`.

**Why this proves more than SQLite rollback semantics:** the cycle-1 atomic-boundary test (`ac5_11_atomic_boundary_rollback_leaves_db_unchanged`) opened a raw tx, wrote inside it, then dropped it. That tested SQLite's behavior. This new test exercises the actual production handler — including its `acquire_lock` call (which would have set `claimed_by` if the tx were not rolled back). The fact that `claimed_by` is unchanged post-Err proves the handler's `tx` rolled back the lock acquisition, which means the validator failure correctly aborts before commit.

A bug like "step 8 writes via `conn` not `tx`" (one of the failure modes the cycle-1 review enumerated) would now fail this test: the lock side-effect would persist on `conn` even after the tx is dropped.

---

## m1 verification — dead `GuardExpr` re-export deleted

`grep -rn GuardExpr src/` returns zero matches. `src/schema/required_when.rs` no longer contains `pub use crate::schema::expr::Expr as GuardExpr;` (lines 3-6 of the prior file are removed). `cargo build` is clean.

---

## Test runs

- `cargo build` — clean (no warnings introduced; 3 pre-existing `unused crate::db` warnings in unrelated files unchanged).
- `cargo test` — 263 passed; 0 failed; 0 ignored.
- `cargo test resume` — `ac5_14_resume_acquires_lock` ok, `ac5_14_resume_actor_mismatch_rejected` ok.
- `cargo test ac5_13` — `ac5_13_lock_held_during_follow_on` ok, `ac5_13_lock_released_after_commit_with_follow_on` ok.
- `cargo test ac5_11` — `ac5_11_atomic_boundary_rollback_leaves_db_unchanged` ok, `ac5_11b_handler_path_validator_failure_rolls_back` ok.
- `cargo test ac5_14` — all 3 ok (`blocked_to_ready_recovery`, `resume_actor_mismatch_rejected`, `resume_acquires_lock`).
- `env -u CLAUDECODE bash tests/e2e.sh` — all 13 DONE_WHEN steps PASS.

---

## Diff scope check

`git diff 9bfaba2..HEAD --stat`:

```
 src/cli/dispatch.rs                       |  23 +-
 src/handlers/submit.rs                    | 340 +++++++++++++++++++++++++++---
 src/schema/required_when.rs               |   5 -
 tasks/active/T002-tasks-store-v02/main.md |  29 ++-
 4 files changed, 339 insertions(+), 58 deletions(-)
```

Tightly scoped: only the three Phase-5 files cycle-1 enumerated + main.md log update. No drive-by changes. No fixture-schema changes. No unrelated test churn.

Commit hygiene:
- `17ab325` T002 P5.cycle2: C1+M1+M2+m1 — compute_resume, lock probe tests, handler rollback test, dead re-export removed
- `4c4fee0` T002 P5.cycle2: update execution log + set status CODE_REVIEW

Two commits, fix-then-log, in chronological order; no amends, no force-push, no skipped hooks.

---

## What's good

- **The compute/run split is consistent across all five workflow verbs.** `compute_resume` returning `ResumeOutput { display_id, new_status, summary }` with `Serialize+Deserialize` mirrors `compute_submit_plan` / `compute_submit_plan_review` / `compute_submit_execute` / `compute_submit_review`. A future structured-output assertion test (e.g. JSON shape contract) is one line away.
- **The mid-tx lock probe pattern (`ac5_13_lock_held_during_follow_on`) is reusable.** Any future invariant that must hold DURING a transaction (not just AFTER) can adopt the same `tx.query_row(...)` checkpoint pattern. This is the right engineering response to "structurally guaranteed but not tested" — make it falsifiable with a same-connection probe at the relevant code line.
- **The handler-path rollback test (`ac5_11b_handler_path_validator_failure_rolls_back`) closes the SQLite-vs-handler gap correctly.** It exercises a real validator failure (actor mismatch) on a real compute fn, and asserts the lock side-effect (`claimed_by`) is rolled back along with the data writes. This proves both (a) the handler uses the tx for the lock, and (b) the validator failure propagates to abort the tx without commit.
- **The dispatch caller is now exactly two lines.** Cycle 1 said "shrinks to a thin caller"; the implementation is literally `handlers::submit::run_resume(schema, &conn, display_id, invoker)?;` after the `display_id` parse. Zero safety logic remaining in dispatch.

---

## Carry-forward to Phase 7 (binding — must be addressed in Phase 7 plan-review)

The three minors deferred from cycle 1 (m2, m3, m4) are accepted as Phase 7 work because Phase 7 builds the actual `tasks` schema where the field-shape decisions become concrete. Phase 7's plan-review MUST verify each is addressed before Phase 7 execution begins:

- **P5-m2 (`--open-questions-from-file` flag).** `submit-plan-review` CLI must accept `--open-questions-from-file <f>` (or `-` for stdin), parse newline-separated list to `Vec<String>`, and append as `open_questions` on the new `plan_review_log` entry. Phase 7's tasks schema declares `open_questions: list_text` on plan_review_log entries (main.md:481); without the flag the bundled `tasks:start` orchestrator cannot populate the field via stores CLI alone, forcing it to either drop the field or smuggle structured data through a stringly-typed `--summary` (forbidden by spec).
- **P5-m3 (`submit_targets` lookup).** Submit handlers must replace hardcoded `"plan"` / `"cycles"` / `"plan_review_log"` field names with `workflow.submit_targets[verb]` lookups. The framework's value proposition ("workflow-shaped stores get the engine for free") depends on a third-party schema author being able to use different list-record/record names. Phase 7's tasks schema happens to use canonical names; the abstraction will leak the moment a second workflow-shaped store is authored.
- **P5-m4 (`--details-from-file` / `--summary` separation).** Phase 7 plan-review must decide between (a) schema a `cycles[].review.details` sub-field on the tasks schema and thread `--details-from-file` separately from `--summary`, or (b) explicitly accept the conflation in Phase 7 with a documented note. Today `submit-review` collapses both flags into one string via `read_text_or_file(sub, "summary", "details-from-file")`, which silently drops one of the two if both are passed.

## Carry-forward to Phase 6 (still owed from cycle 1, unchanged)

- **P2-M1 (WorkflowResolved threading).** Phase 6 `brief.rs` disk-read AND render template need the resolved form. Phase 6 plan-review must verify P2-M1 lands.

---

## Cycle accounting

This is cycle 2 of max 3. PASS gate. Cycle 3 not needed; advancing to `EXECUTING_PHASE_6`. The three Phase 7 carry-forwards are recorded in the cycle-2 entry of `## Code Review Log` for Phase 7's plan-reviewer to enforce.
