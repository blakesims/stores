# T012: workspace_path field + tasks next-id verb

## Meta
- **Status:** PLANNING
- **Created:** 2026-05-02
- **Last Updated:** 2026-05-02
- **Blocked Reason:** —

## Task

Add an optional `workspace_path` field to the `tasks` schema so project scripts (e.g. `./dev new`) can pin where each task's spawned agents run. Drive uses the path as the canonicalized cwd at spawn time, preserving the existing SDK session-fresh-on-cwd-mismatch guard at `src/runner/claude_code.rs:305-306`. Drive errors loud at spawn time if the path is set but missing — no silent fallback.

Also add a read-only `stores tasks next-id` verb that scans `tasks/{active,planning,paused,completed,archived}/` for the highest existing `T###` and prints the next available ID. Project scripts call this to coordinate IDs across worktrees without races.

Together these are the substrate-side hooks for the wrapper boundary T011 just documented in `docs/philosophy.md` — the project-script-wraps-stores field (`workspace_path`) and the project-script-asks-stores verb (`next-id`). This is task #2 of the four-task ship plan from the 2026-05-02 worklog (`docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md`, Tensions D + E).

## Intent Contract

**Executive intent.** The substrate today silently inherits the orchestrator's cwd when spawning agents, which makes multi-worktree workflows (`/task:open` in a per-task worktree) unsafe — agents can land in the wrong tree. T012 makes the cwd explicit via a row-stored field and gives project scripts a race-free way to mint IDs across worktrees. Pinning these now closes the substrate side of the wrapper boundary so T013 (reviewer envelope migration) and T014 (framework write-path) can proceed without re-litigating cwd or ID semantics.

**DONE_WHEN.**
1. `tasks` schema has `workspace_path: text, required: false`.
2. When set, drive uses it as the canonicalized cwd for every spawned agent (preserving the SDK session-fresh-on-cwd-mismatch guard at `src/runner/claude_code.rs:305-306`).
3. When unset, drive uses inherited cwd (current behavior, no regression).
4. When set but the path doesn't exist, drive errors at spawn time with a clear message — no silent fallback.
5. `stores tasks next-id` verb scans `tasks/{active,planning,paused,completed,archived}/` for the highest `T###` and prints the next available ID. Read-only, no state.
6. Tests cover the four spawn-time cases (set+exists, set+missing, unset, set+canonicalize-stable across spawn/resume) and the next-id scan.

**Scope boundaries.**
- **In scope:**
  - `stores/tasks/schema.yaml` — add `workspace_path` field (placement near existing `branch` field at line 8)
  - `src/runner/mod.rs` — `Runner::spawn` trait signature gains `Option<&str>` workspace_path
  - `src/runner/claude_code.rs` — implement new signature; canonicalize-and-lock once at spawn (DO NOT re-canonicalize per call); preserve session-fresh guard
  - `src/runner/mock.rs` — update mock to new signature
  - `src/handlers/drive.rs` — read workspace_path from row at the existing `runner.spawn(...)` call site (~line 609); pass through; error at spawn time if path set but missing
  - CLI dispatch site for tasks subcommands — add `next-id` verb (read-only directory scan)
  - Tests for all of the above
- **Out of scope:**
  - No hook system for project-side scripts (workspace_path is written by the project script at task creation; stores does not invoke setup scripts or create worktrees)
  - No worktree creation, no setup-script invocation, no `cd` semantics beyond cwd at spawn
  - No path-existence check at write time (workspace can become invalid later; that's fine, write was valid at the time)
  - No path enum / typed-path (plain `text`, matches existing schema convention)
  - No retroactive backfill of existing tasks (field is optional)
  - No changes to other stores' schemas (tasks-only)

**Proposed approach.** Two natural phases:
- **Phase 1 — workspace_path.** Schema field → Runner trait signature → both runner impls (canonicalize-and-lock in ClaudeCodeRunner) → drive plumbing with spawn-time validation → tests. ~30-50 LOC + test code.
- **Phase 2 — next-id verb.** CLI dispatch → directory scan → tests. Smaller. Planner may fold into Phase 1 if trivial.

**Risks / assumptions.**
- The SDK session-fresh-on-cwd-mismatch guard (`src/runner/claude_code.rs:305-306`) MUST be preserved. Any workspace_path implementation that re-canonicalizes per call (rather than once at spawn) silently breaks session continuity for resumed agents. New code must comment-reference this guard so future readers see the constraint.
- `Runner::spawn` signature change breaks `MockRunner`; both impls move together in the same phase.
- `next-id` scanning multiple status directories assumes the canonical layout in `tasks/CLAUDE.md` (active/planning/paused/completed/archived). If a directory is missing, scan it as empty rather than erroring (lenient).
- Carry-forward from T011: fill `## Completion` section *before* flipping `Status: COMPLETE` (CodeRabbit Stage 6 caught this on T011).

**Open decisions.** None. All five (field placement, type, validation policy, trait signature change, next-id behavior) were locked during the morning design discussion via AskUserQuestion. See `docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md` Tensions D + E for the rationale.

---

## Plan
_Planner agent fills this section._

### Objective
_What we're trying to achieve._

### Scope
- **In Scope:** ...
- **Out of Scope:** ...

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | ... | Low/Medium/High |

### Phase Details

#### Phase 1: [Title]
- **Objective:** ...
- **Files to modify:** ...
- **Acceptance Criteria:**
  - [ ] ...

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| ... | ... | ... | ... |

---

## Plan Review
_Plan-reviewer agent fills this section._

- **Gate:** READY | NEEDS_WORK | NOT_READY
- **Open Questions Finalized:** —
- **Issues Found:** —

> Details: plan-review.md

---

## Execution Log
_Executor agent fills this section per phase._

### Phase 1: [Title]
- **Status:** PENDING | IN_PROGRESS | COMPLETE | BLOCKED
- **Started:** —
- **Completed:** —
- **Commits:** —
- **Files Modified:** —
- **Notes:** —

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

### Phase 1
- **Gate:** PASS | REVISE | FAIL
- **Issues Found:** —
- **Revision Count:** 0/3

> Details: code-review-phase-1.md

---

## Completion
_Final summary when task is complete._

- **Completed:** [DATE]
- **Summary:** ...
- **Commits:** ...
- **Lessons Learned:** ...
