# Global Task Manager

Tracks all tasks. The orchestrator maintains this file.

## Current Tasks

| ID | Task Name | Priority | Phase | Status | Link |
|:---|:----------|:---------|:------|:-------|:-----|
| T005 | Drive substrate fixes — `blocked` divergence + envelope-mismatch handling + log visibility | High | Phase 4 | COMPLETE | [main.md](./active/T005-drive-substrate-fixes/main.md) |

Next available task id: T006

---

## Recently Completed

| ID | Task Name | Completed | Link |
|:---|:----------|:----------|:-----|
| T004 | Schema-validated agent envelope via `--json-schema` | 2026-04-28 | [main.md](./completed/T004-schema-validated-envelope/main.md) |
| T003 | Framework-bundled workflow agents + runtime-agnostic orchestrator | 2026-04-28 | [main.md](./completed/T003-bundled-agents-and-drive/main.md) |
| T002 | Tasks store on β architecture (workflow engine) | 2026-04-26 | [main.md](./completed/T002-tasks-store-v02/main.md) |
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
