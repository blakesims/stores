# T035: Resume leaves stale auto-drive PID causing immediate re-block

## Meta
- **Status:** in_review
- **Created:** 2026-05-05T14:57:03Z
- **Last Updated:** 2026-05-06T02:49:07Z
- **Current Phase:** 1
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T035-auto-promoted-l113

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - Clear or neutralize stale drive_pid/drive_started_at and relevant auto-drive lock state on tasks resume; add regression test covering resume after drive_failed stale PID; verify cargo test target.
- **Out:** - Changing normal auto-drive spawn semantics; changing Pi runner implementation; broad daemon redesign.

### Done When
Fix resume/watchdog bookkeeping so resuming a drive_failed task does not immediately re-block from stale auto-drive PID state.

Acceptance:
- A task blocked by mark_drive_failed with stale drive_pid can be resumed without immediate watchdog re-block; regression test covers stale PID/dispatch_lock; relevant cargo tests pass.

### Phases

#### Phase 1: single T1 phase
- **Objective:** 
- **Tasks:**
- **Acceptance Criteria:**

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** T035 implemented in src/handlers/submit.rs: compute_resume clears stale drive_pid/drive_started_at and auto-drive dispatch_locks before resume transition. Added regression test resume_clears_stale_auto_drive_bookkeeping_before_watchdog; cargo build and targeted lib test pass. Full cargo test currently fails in unrelated tests/sidecar_handoff.rs (3 sidecar handoff assertions).
- **Commit:** `6413ef715d930938499b9e17d5ffcc23308ad540`
- **Files:**
  - `src/handlers/submit.rs`
- **At:** 2026-05-06T02:46:08Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. The T035 acceptance is satisfied: resume now clears stale drive_pid/drive_started_at and auto-drive dispatch_locks, the regression test covers stale PID plus dispatch_lock followed by watchdog sweep, and relevant build/tests pass. 0 critical, 0 major, 0 minor findings.
- **Findings:** 0 critical, 0 major, 0 minor
**Details:**
Git reality: HEAD is 6413ef7 (executor commit) and git show confirms only src/handlers/submit.rs changed in that commit, matching the submission; worktree is clean. git diff --name-only HEAD~3 includes many prior checkpoint files plus src/handlers/submit.rs, but the reviewed commit itself is scoped correctly.
AC verification: PASS — compute_resume calls clear_auto_drive_bookkeeping_for_resume before the blocked→ready resume transition and ready on-entry follow-on. That helper NULLs drive_pid and drive_started_at when present and deletes matching auto-drive dispatch_locks for the resumed row.
AC verification: PASS — regression test handlers::submit::tests::resume_clears_stale_auto_drive_bookkeeping_before_watchdog inserts a blocked T900 with stale drive_pid and auto-drive dispatch_lock, resumes it, asserts status executing, blocked_reason clear, drive_pid NULL, lock deleted, then runs sweep_drive_watchdog and asserts acted &#x3D;&#x3D; 0 and status remains executing.
Test evidence: cargo build passed. cargo test resume_clears_stale_auto_drive_bookkeeping_before_watchdog -- --nocapture passed in lib and bin harnesses. cargo test --lib passed: 767 tests. Full cargo test still fails in tests/sidecar_handoff.rs (3 failures around --append-system-prompt/--message), matching executor&#x27;s note and unrelated to the single changed file/acceptance scope.
- **At:** 2026-05-06T02:48:35Z

---

## Completion
- **In Review:** 2026-05-06T02:49:07Z — awaiting human GO/NO_GO

