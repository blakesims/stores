# T033: tasks drive: pre-flight depends_on guard

## Meta
- **Status:** in_review
- **Created:** 2026-05-05T10:40:51Z
- **Last Updated:** 2026-05-06T04:24:05Z
- **Current Phase:** 1
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T033-auto-promoted-l038

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - src/handlers/drive.rs: pre-flight check that every entry in depends_on has status in {accepted, cargo_installed, schema_migrated}
- Error message names which dep is unmet and its current status; suggests &#x27;tasks status &lt;unmet_dep&gt;&#x27; for context
- Direct deps only; no transitive walk
- **Out:** - Active chain (auto-fire dependents on parent accept) — Layer 2, separate task
- Transitive depends_on graph traversal — Layer 2
- UI/render of the dependency graph

### Done When
tasks drive refuses to start when any depends_on task is not yet accepted (passive guard, Layer 1).

Acceptance:
- tasks drive TX where depends_on&#x3D;[TY] and TY.status&#x3D;&#x27;executing&#x27; exits non-zero with a clear named-dep error
- tasks drive TX where depends_on&#x3D;[TY] and TY.status&#x3D;&#x27;accepted&#x27; drives normally
- Unit test for both branches

### Phases

_Plan not yet submitted._

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** Submitted — awaiting review
- **Summary:** Added pre-flight depends_on guard at top of drive_loop (src/handlers/drive.rs). New helper check_depends_on_guard reads the row, looks up each dep&#x27;s status, and bails with a named-dep error when any dep !&#x3D; &#x27;accepted&#x27;. Two unit tests added: drive_refuses_when_dep_not_accepted (TY status&#x3D;&#x27;executing&#x27; → Err naming T002 + &#x27;executing&#x27; + &#x27;depends_on&#x27;) and drive_proceeds_when_dep_accepted (TY status&#x3D;&#x27;accepted&#x27; → full happy-path mock queue drains). cargo build succeeds; all 765 lib tests pass.
- **Commit:** `25afdea`
- **Files:**
  - `src/handlers/drive.rs`
- **At:** 2026-05-05T10:44:13Z
### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Phase already implemented in HEAD commit 25afdea (T033: pre-flight depends_on guard in drive_loop). Adds Layer-1 passive guard at drive entry that refuses to start when any depends_on dep is not status&#x3D;&#x27;accepted&#x27;, surfacing a named-dep error per unmet dep. Both required unit tests pass: drive_refuses_when_dep_not_accepted (dep executing → Err) and drive_proceeds_when_dep_accepted (dep accepted → happy path). Full drive test suite green (56 passed). No further changes needed.
- **Commit:** `25afdea139df017a691637d12dcff3dca70d6c44`
- **Files:**
  - `src/handlers/drive.rs`
- **At:** 2026-05-06T04:22:33Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** All 3 acceptance criteria verified. Pre-flight guard added at top of drive_loop (src/handlers/drive.rs:486), helper check_depends_on_guard reads depends_on (list_fk per stores/tasks/schema.yaml:13), looks up each dep&#x27;s status, and bails with a named-dep error including dep id and current status. Both unit tests pass in isolation: drive_refuses_when_dep_not_accepted (executing → Err naming T002 + &#x27;executing&#x27; + &#x27;depends_on&#x27;) and drive_proceeds_when_dep_accepted (accepted → all 5 mock responses consumed). cargo build clean. One unrelated pre-existing flaky test (h_ntfy_halt_event_body) fails under full-suite parallelism but passes in isolation — not introduced by this commit.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[INFORMATIONAL] Helper check_depends_on_guard at src/handlers/drive.rs:426-470 is well-scoped: uses quote_ident for the table name (SQL-injection safe), uses parameterized query for dep_id, and short-circuits on empty/non-array depends_on.

[INFORMATIONAL] Tests are clean and assert the right error fragments (id, status, &#x27;depends_on&#x27; keyword) — not over-coupled to exact phrasing.

[MINOR] Missing-dep handling: &#x60;Err(QueryReturnedNoRows)&#x60; is folded into not_accepted with status&#x3D;&#x27;&lt;missing&gt;&#x27; rather than treated as a hard error. This is a reasonable choice (fail-loud with a clear status string) but is not covered by a test. A third test case for a dangling dep id (e.g. depends_on&#x3D;[&#x27;T999&#x27;] when T999 doesn&#x27;t exist) would lock in this behavior. Non-blocking — the current behavior is sensible and the AC only required two branches.

[MINOR] The error phrasing &#x27;each dep must reach status&#x3D;\&#x27;accepted\&#x27; before this task can drive&#x27; is good operator-facing copy, but the parenthetical is appended to a flat sentence — readability is fine but a follow-up could split into two lines. Non-blocking.

[INFORMATIONAL] Pre-existing flaky test handlers::agents_run::tests::policy::h_ntfy_halt_event_body fails under parallel &#x60;cargo test --lib&#x60; but passes in isolation. Unrelated to T033 — file under separate observation if not already filed.
- **At:** 2026-05-06T04:23:36Z

---

## Completion
- **In Review:** 2026-05-06T04:24:05Z — awaiting human GO/NO_GO

