# T035: Resume leaves stale auto-drive PID causing immediate re-block

## Meta
- **Status:** planning
- **Created:** 2026-05-05T14:57:03Z
- **Last Updated:** 2026-05-05T14:57:03Z
- **Current Phase:** 
- **Current Cycle:** 
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

_Plan not yet submitted._

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

