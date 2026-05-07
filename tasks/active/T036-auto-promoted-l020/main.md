# T036: tasks render writes to new state&#x27;s directory but doesn&#x27;t remove previous-state&#x27;s; rows accumulate empty-shell dirs across active/planning/completed (the substrate-side of the multiple-T001-paths warning, NOT the fs/T001 historical concern)

## Meta
- **Status:** in_review
- **Created:** 2026-05-06T03:12:44Z
- **Last Updated:** 2026-05-06T04:45:44Z
- **Current Phase:** 1
- **Current Cycle:** 1
- **Blocked Reason:** —
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
### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** T036 (T1 contract-is-plan): render now canonicalizes state directories. Added find_all_task_dirs + cleanup_stale_task_dirs in src/render/path.rs; find_existing_task_dir now picks most-recently-modified on multi-match instead of bailing. run_render_in invokes cleanup_stale_task_dirs after the atomic write — empty shells removed, user files migrated, same-name collisions warn without data loss. New tests: 3-state walkthrough ends with exactly one dir (render_canonicalizes_state_dirs_across_transitions), pre-existing accumulated shells consolidated (render_removes_pre_existing_stale_shells), find_all returns every match, cleanup removes empty shells / migrates user files / warns on collision / no-ops without stale. Updated find_existing_dir_returns_none_on_multiple_matches → find_existing_dir_returns_some_on_multiple_matches per new semantics. cargo build clean; all 66 render tests pass; pre-existing sidecar_handoff failures verified unrelated (reproduce on stash of clean baseline).
- **Commit:** `4efefa2efb3d804f6ee0779be1f363f680a19be8`
- **Files:**
  - `src/handlers/render.rs`
  - `src/render/path.rs`
- **At:** 2026-05-06T04:44:29Z

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

### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. T036 contract satisfied: render now canonicalizes state dirs via post-write cleanup_stale_task_dirs. All 4 ACs verified — single-dir invariant after 3-state walkthrough, pre-existing shell consolidation, collision warning preserves user data, all 66 render tests pass. Scope respected (only src/handlers/render.rs + src/render/path.rs touched). 3 minor findings.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Contract says &#x27;warning fires only on genuine display_id collision&#x27;, but the implementation warns on per-file name collisions inside a stale dir, not on the distinct-rows-sharing-display_id case. In practice this is fine (two distinct rows with same display_id is impossible by substrate invariant — uniqueness on wf_tasks.display_id), so the only collision path that can fire is per-file. Worth a code comment noting the substrate invariant that makes the &#x27;distinct row&#x27; interpretation a non-case.
File: src/render/path.rs:152-184 (cleanup_stale_task_dirs)

[MINOR] find_existing_task_dir multi-match path uses .modified() metadata; on filesystems where mtime resolution is coarse (e.g. some network FS, ext3 1s granularity) the sort may pick non-deterministically. The test mitigates with a 20ms sleep, which is below 1s mtime resolution. Not a correctness issue under normal use (target rename touches mtime), but the test could be tightened by setting explicit mtimes via filetime crate or asserting set-membership rather than ordering.
File: src/render/path.rs:140-148

[INFORMATIONAL] cleanup_stale_task_dirs is wired with eprintln-only error reporting (not propagated). That&#x27;s the right call for a cleanup pass — we don&#x27;t want the post-rename canonicalization to fail the render — but worth confirming this matches the run_render_in caller&#x27;s error policy. Caller already wraps cleanup in &#x60;if let Err(e) &#x3D; ... eprintln!&#x60;, so cleanup returning Ok(()) on inner errors is double-defensive but harmless.
File: src/handlers/render.rs:229-235

[INFORMATIONAL] target_dir.canonicalize() on line 165 falls back to the raw path on error. Since cleanup runs after the atomic rename creates target_dir, canonicalize should succeed; the fallback is defensive padding only.
File: src/render/path.rs:165
- **At:** 2026-05-06T04:45:18Z

---

## Completion
- **In Review:** 2026-05-06T04:45:44Z — awaiting human GO/NO_GO

