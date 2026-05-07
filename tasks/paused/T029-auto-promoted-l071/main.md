# T029: Drive cycle aborts gracefully on runner exit&#x3D;1 (e.g. Claude API rate limit) but does NOT notify substrate; row stays stuck at status&#x3D;executing forever; richer L062 gap

## Meta
- **Status:** blocked
- **Created:** 2026-05-05T08:50:29Z
- **Last Updated:** 2026-05-06T04:22:35Z
- **Current Phase:** 1
- **Current Cycle:** 1
- **Blocked Reason:** code-reviewer marked FAIL on phase 1: Phase brief is malformed: &#x27;Current Phase: 1 of 0&#x27; (plan_phases_count&#x3D;0) with empty &#x27;Phase Being Reviewed&#x27; section and no acceptance criteria. Executor correctly blocked without committing. This is a planning defect — replanning needed.
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

---

## Completion
_Not yet complete._

