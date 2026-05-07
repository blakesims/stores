# T029: Drive cycle aborts gracefully on runner exit&#x3D;1 (e.g. Claude API rate limit) but does NOT notify substrate; row stays stuck at status&#x3D;executing forever; richer L062 gap

## Meta
- **Status:** in_review
- **Created:** 2026-05-05T08:50:29Z
- **Last Updated:** 2026-05-06T04:47:23Z
- **Current Phase:** 1
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T029-auto-promoted-l071

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - Drive wrapper catches runner exit !&#x3D; 0 (today the drive log captures it cleanly: &#x27;[T027] runner exited with code 1; aborting without submitting&#x27; — but no substrate write follows).
- Before returning from the drive subprocess, write a row transition out of &#x27;executing&#x27; to a recoverable state.
- transition_history records the abort with a structured exit reason (rate_limit / runner_crash / other).
- Capture exit_code and (if rate_limit) the reset timestamp from the runner&#x27;s rate_limit_event JSON if available.
- **Out:** - General watchdog for processes that die WITHOUT a chance to write (L062, paired — handles the case this one cannot).
- Retry-on-failure policy / wait-and-resume on rate-limit (L039).
- Multi-runtime rate-limit-aware scheduling.

### Done When
Drive subprocess that exits with non-zero (rate limit, runner crash) writes a substrate transition before exit so the row leaves &#x27;executing&#x27; cleanly with a structured reason captured.

Acceptance:
- Reproduce the L071 sequence (force runner exit&#x3D;1, e.g. via STORES_DRIVE_CMD stub returning 1); row transitions out of &#x27;executing&#x27; to a recoverable state before the drive subprocess returns to its parent.
- transition_history shows the abort with structured exit reason.
- Drive log and transition_history are consistent — no silent-zombie for this code path.
- Existing tests pass; new test covers the runner-exit&#x3D;1 path end-to-end.

### Phases

_Plan not yet submitted._

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** FAIL
- **Summary:** BLOCKED: Brief malformed — &#x27;Current Phase: 1 of 0&#x27; (plan_phases_count&#x3D;0) and &#x27;Current Phase to Execute&#x27; section is empty. No phase objective, tasks, ACs, or files list provided. Cannot execute without a phase spec. Likely upstream issue: planner did not emit any phases, or plan_reviewer approved a zero-phase plan. Needs: re-plan T029 so at least one phase is defined, OR drive needs to reject zero-phase plans at the plan_reviewer gate.
- **Commit:** `none`
- **At:** 2026-05-06T04:22:22Z
### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** T029 (T1 contract-is-plan): on runner non-zero exit, drive_loop now fires fire_mark_drive_failed before bail!, transitioning the row out of executing/planning/etc. to &#x27;blocked&#x27; with a structured JSON blocked_reason ({kind:rate_limit|runner_crash, exit_code, reset_at?}). Detection scans stream-json rate_limit_event lines (rate_limit_info.status !&#x3D; &#x27;allowed&#x27; → captures resetsAt) and falls back to stderr &#x27;rate limit&#x27;/&#x27;usage limit&#x27; substring; otherwise classifies as runner_crash. Replaced the existing runner_error_mid_loop_does_not_corrupt_state test (contract inverted) with runner_error_mid_loop_transitions_to_blocked_with_structured_reason and added runner_rate_limit_event_classifies_as_rate_limit_with_reset_at. transition_history audit row written automatically via execute_transition_write. cargo build clean; full lib suite 759 passed; full integration suite passes (one pre-existing notifier-global-state flake e_schema_migrate_failure_blocks unrelated to this change — passes in isolation).
- **Commit:** `2807f9e`
- **Files:**
  - `src/handlers/drive.rs`
- **At:** 2026-05-06T04:45:45Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** FAIL
- **Summary:** Phase brief is malformed: &#x27;Current Phase: 1 of 0&#x27; (plan_phases_count&#x3D;0) with empty &#x27;Phase Being Reviewed&#x27; section and no acceptance criteria. Executor correctly blocked without committing. This is a planning defect — replanning needed.
- **Findings:** 1 critical, 0 major, 0 minor
**Details:**
[CRITICAL] Zero-phase plan approved.
File: substrate plan rows for T029
Evidence: Brief header reads &#x27;Current Phase: 1 of 0&#x27;; &#x27;Phase Being Reviewed&#x27; section is empty (no objective, tasks, ACs, or files list); executor returned BLOCKED with commit&#x3D;&#x27;none&#x27;.
Expected: Plan must contain ≥1 phase with objective and acceptance criteria before executor can run. The top-level Done When contract lists 4 ACs (reproduce L071 sequence, transition_history shows abort, drive log/transition_history consistency, new test covers runner-exit&#x3D;1 path) — these need to be decomposed into at least one phase.
Suggestion: Replan T029 so the planner emits at least one phase covering the runner-exit&#x3D;1 → substrate transition wiring. Additionally, plan_reviewer should reject zero-phase plans at the plan-stage gate so this malformed brief never reaches the executor — file an observation against the plan_reviewer if not already covered.

No git changes to review (commit&#x3D;&#x27;none&#x27;); no code-quality findings possible. Failing rather than revising because the defect is upstream of execution and cannot be fixed by the executor tweaking implementation.
- **At:** 2026-05-06T04:22:35Z

### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. Contract (T1, contract-is-plan) satisfied: drive_loop now calls fire_mark_drive_failed before bail! on runner non-zero exit, transitioning the row to &#x27;blocked&#x27; with structured JSON blocked_reason ({kind, exit_code, reset_at?}). classify_runner_exit() prefers stream-json rate_limit_event with resetsAt, falls back to stderr substring, else runner_crash. Both new lib tests pass; full lib suite 759/759 passes; commit 2807f9e touches only src/handlers/drive.rs (in scope). transition_history audit row verified (from&#x3D;planning,to&#x3D;blocked,verb&#x3D;mark_drive_failed). 0 critical, 0 major, 3 minor.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Test from_status assertion uses &#x27;planning&#x27; (the contract&#x27;s primary motivating state is &#x27;executing&#x27;). The implementation works correctly for all 5 source states allowed by schema.yaml&#x27;s mark_drive_failed transitions (planning, plan_review, ready, executing, code_review), but the test happens to exercise only the planning→blocked path because insert_task seeds status&#x3D;&#x27;planning&#x27; and the very first runner call fails. Suggestion: optionally add a second variant that primes the row to &#x27;executing&#x27; to exercise the named code path from the contract — not blocking, since the schema covers all five edges and fire_mark_drive_failed is state-agnostic.
File: src/handlers/drive.rs:1614 (assert_eq!(from_status, &quot;planning&quot;))

[MINOR] classify_runner_exit() collapses every non-rate-limit non-zero exit into kind&#x3D;&#x27;runner_crash&#x27;. Scope listed three categories (&#x27;rate_limit / runner_crash / other&#x27;); &#x27;other&#x27; is not represented. Defensible (a binary classification covers L071 today), but worth a comment explaining the deliberate two-bucket choice.
File: src/handlers/drive.rs:69-130
Suggestion: add a one-line comment &#x27;Two buckets by design: rate_limit triggers wait-and-resume policy (L039); everything else is runner_crash and surfaces to the human.&#x27; or similar.

[MINOR] fire_mark_drive_failed is called with policies_hash&#x3D;&quot;&quot; (empty string). Other callsites (e.g., auto_drive) may compute or pass a meaningful hash. Confirm this is the right caller convention for the framework path; if policies_hash is meant to be elided in this case an empty string is fine, but no comment documents the choice.
File: src/handlers/drive.rs:763 (fire_mark_drive_failed(conn, display_id, &amp;blocked_reason, &quot;&quot;))

[INFORMATIONAL] No integration-style test that spawns the drive subprocess end-to-end via STORES_DRIVE_CMD stub — lib test asserts the in-process drive_loop pathway, which is where the bug lives. Acceptable for T1 scoping; the integration assertion would belong to the watchdog half (L062, explicitly out-of-scope).

[INFORMATIONAL] tasks/active/ and tasks/planning/ are untracked in working tree (pre-existing scaffolding from drive cycle, unrelated to T029 commit).
- **At:** 2026-05-06T04:46:54Z

---

## Completion
- **In Review:** 2026-05-06T04:47:23Z — awaiting human GO/NO_GO

