# Global Task Manager

Tracks all tasks. The orchestrator maintains this file.

## Current Tasks

| ID | Task Name | Priority | Phase | Status | Link |
|:---|:----------|:---------|:------|:-------|:-----|

Next available task id: T012

---

## Recently Completed

| ID | Task Name | Completed | Link |
|:---|:----------|:----------|:-----|
| T011 | Document the wrapper boundary in philosophy.md | 2026-05-02 | [main.md](./completed/T011-docs-wrapper-boundary/main.md) |
| T010 | Wrap workflow + GO/NO_GO (last 10%) | 2026-05-01 | [main.md](./completed/T010-wrap-workflow/main.md) |
| T009 | Port the 10.06 `observations` store — second real migration | 2026-05-01 | [main.md](./completed/T009-port-10-06-observations/main.md) |
| T008 | Add `FieldType::Json` for free-shape opaque payloads | 2026-04-30 | [main.md](./completed/T008-json-fieldtype/main.md) |
| T007 | Port the 10.06 `gate` store — first real migration | 2026-04-30 | [main.md](./completed/T007-port-10-06-gate/main.md) |
| T006 | Substrate cleanup — POC findings (transition guards, list_record, name escaping, list flags) | 2026-04-30 | [main.md](./completed/T006-substrate-cleanup-poc/main.md) |
| T005 | Drive substrate fixes — `blocked` divergence + envelope-mismatch handling + log visibility | 2026-04-30 | [main.md](./completed/T005-drive-substrate-fixes/main.md) |
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
