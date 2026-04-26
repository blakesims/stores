# Global Task Manager

Tracks all tasks. The orchestrator maintains this file.

## Current Tasks

| ID | Task Name | Priority | Phase | Status | Link |
|:---|:----------|:---------|:------|:-------|:-----|

Next available task id: T002

---

## Recently Completed

| ID | Task Name | Completed | Link |
|:---|:----------|:----------|:-----|
| T001 | Stores Framework v0.1 | 2026-04-26 | [main.md](./completed/T001-stores-framework-v01/main.md) |

---

## Status Legend

| Status | Meaning |
|--------|---------|
| `PLANNING` | Planner creating implementation plan |
| `PLAN_REVIEW` | Plan-reviewer validating plan |
| `READY` | Plan approved, awaiting execution |
| `EXECUTING_PHASE_N` | Executor working on phase N |
| `CODE_REVIEW` | Code-reviewer checking implementation |
| `BLOCKED` | Needs human input |
| `COMPLETE` | All phases done |

## Directory Rules

- `PLANNING` / `PLAN_REVIEW` → `tasks/planning/`
- `READY` / `EXECUTING_*` / `CODE_REVIEW` → `tasks/active/`
- `BLOCKED` → `tasks/paused/`
- `COMPLETE` → `tasks/completed/`

When moving directories, update the Link column.
