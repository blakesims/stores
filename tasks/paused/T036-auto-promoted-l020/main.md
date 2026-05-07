# T036: tasks render writes to new state&#x27;s directory but doesn&#x27;t remove previous-state&#x27;s; rows accumulate empty-shell dirs across active/planning/completed (the substrate-side of the multiple-T001-paths warning, NOT the fs/T001 historical concern)

## Meta
- **Status:** blocked
- **Created:** 2026-05-06T03:12:44Z
- **Last Updated:** 2026-05-06T04:22:36Z
- **Current Phase:** 1
- **Current Cycle:** 1
- **Blocked Reason:** code-reviewer marked FAIL on phase 1: Planning defect: brief reports &#x27;Current Phase: 1 of 0&#x27; with an empty &#x27;Phase Being Reviewed&#x27; section and executor commit&#x3D;&#x27;none&#x27;. No phase exists to review; replanning required.
- **Branch:** feat/T036-auto-promoted-l020

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - src/handlers/render.rs: post-write scan tasks/{active,planning,paused,completed,archived,in_review,rejected}/ for stale dirs matching the row&#x27;s display_id slug; remove (or git mv if tracked)
- warning becomes failure-mode-only (truly distinct rows colliding)
- regression tests: a render walked through 3 states ends with exactly one dir
- **Out:** - fs/T001 historical great-divide warning (intentional per doctrine)
- retroactive cleanup of pre-existing orphans (separate cleanup task)
- symlink-based state mirroring

### Done When
Make tasks render canonicalize state directories: writing to the new state&#x27;s dir cleans previous-state shells of the same row, eliminating the multi-path warning.

Acceptance:
- stores tasks render T### produces exactly one tasks/&lt;state&gt;/T###-&lt;slug&gt;/ directory
- re-running render after a state change cleans the prior dir
- warning fires only on genuine display_id collision (covered by a test)
- existing tests pass + new orphan-cleanup tests

### Phases

_Plan not yet submitted._

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** FAIL
- **Summary:** BLOCKED: Brief contains no executable phase. Header reads &#x27;Current Phase: 1 of 0&#x27; and the &#x27;Current Phase to Execute&#x27; section is empty (no objective/tasks/ACs/files). Top-level Done-When and acceptance criteria are present, but with plan_phases_count&#x3D;0 there is nothing for the executor to implement. Needs the planner to produce at least one phase, or the plan-reviewer gate to reject the empty plan, before executor can run.
- **Commit:** `none`
- **At:** 2026-05-06T04:22:22Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** FAIL
- **Summary:** Planning defect: brief reports &#x27;Current Phase: 1 of 0&#x27; with an empty &#x27;Phase Being Reviewed&#x27; section and executor commit&#x3D;&#x27;none&#x27;. No phase exists to review; replanning required.
- **Findings:** 1 critical, 0 major, 0 minor
**Details:**
[CRITICAL] Empty plan / missing phase
File: (planner output for T036)
Evidence: Brief header &#x27;Current Phase: 1 of 0&#x27;; &#x27;Phase Being Reviewed&#x27; section in brief is empty (no objective, tasks, acceptance criteria, or files). Executor submission is BLOCKED with commit&#x3D;&#x27;none&#x27; and summary explicitly states plan_phases_count&#x3D;0.
Expected: A plan with at least one executable phase containing objective, tasks, ACs, and files (or, for T1 contract-is-plan tasks, the framework should fire skip-plan rather than producing an empty plan that reaches the executor).
Suggestion: Replan T036 — produce a plan with one or more phases. The top-level done-when has 4 acceptance criteria (single state-dir output, re-render cleans prior dir, warning only on genuine display_id collision, existing+new orphan-cleanup tests) which is a one- to two-phase task. If T036&#x27;s tier_hint is T1, verify the StateAction predicates are routing to skip-plan correctly; if T2/T3, the planner must emit non-empty phases and the plan_reviewer must reject an empty-phases plan rather than passing it through to the executor.
- **At:** 2026-05-06T04:22:36Z

---

## Completion
_Not yet complete._

