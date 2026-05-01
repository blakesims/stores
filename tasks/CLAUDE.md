---
description: Task documentation procedures for AI agents (task-workflow plugin aligned).
---

## AI Task Documentation Procedures

### Core Task Creation Process

Definitions:
- **Task** = work item tracked in `tasks/global-task-manager.md`
- **Phase** = execution unit inside a Task (filled by planner, executed sequentially)

For new tasks:
1. Get next Task ID from `tasks/global-task-manager.md`
2. Create folder: `tasks/planning/TXXX-task-slug/`
3. Copy template: `tasks/main-template.md` → `tasks/planning/TXXX-task-slug/main.md`
4. Update GTM: add a row linking to the task and increment "Next available task id"
5. Fill out the `## Task` section with the objective
6. Set Status to `PLANNING` — the workflow begins

### Status Values (Orchestrator State Machine)

| Status | Meaning | Directory |
|--------|---------|-----------|
| `PLANNING` | Planner agent creating implementation plan | `tasks/planning/` |
| `PLAN_REVIEW` | Plan-reviewer validating the plan | `tasks/planning/` |
| `READY` | Plan approved, ready for execution | `tasks/active/` |
| `EXECUTING_PHASE_N` | Executor working on phase N | `tasks/active/` |
| `CODE_REVIEW` | Code-reviewer checking phase implementation | `tasks/active/` |
| `BLOCKED` | Needs human input (questions/failed gate) | `tasks/paused/` |
| `COMPLETE` | All phases done and reviewed | `tasks/completed/` |

### Directory Transitions

The **orchestrator** moves task folders at lifecycle gates:

| Gate | Action |
|------|--------|
| Plan approved (`READY`) | `git mv tasks/planning/TXXX tasks/active/` |
| Task blocked | `git mv tasks/active/TXXX tasks/paused/` |
| Task complete (final `PASS`) | `git mv tasks/active/TXXX tasks/completed/` |

When status changes, update the GTM row link to reflect the new path.

### main.md Sections

Each section is owned by a specific agent:

| Section | Owner | When Updated |
|---------|-------|--------------|
| `## Meta` | All agents | Status changes |
| `## Task` | Human/Orchestrator | Task creation |
| `## Plan` | Planner | Planning phase |
| `## Plan Review` | Plan-reviewer | After planning |
| `## Execution Log` | Executor | During execution |
| `## Code Review Log` | Code-reviewer | After each phase |
| `## Completion` | Orchestrator | Task complete |

### Iteration Limits

| Situation | Limit | Action |
|-----------|-------|--------|
| REVISE cycles (code review) | 3 | After 3 REVISE → FAIL → BLOCKED |
| NEEDS_WORK cycles (plan review) | 3 | After 3 NEEDS_WORK → escalate to human |

### BLOCKED Recovery

1. Human reviews the blocker in main.md
2. Human answers open questions or provides guidance
3. Human updates Status to appropriate previous state
4. Human tells orchestrator: "Continue TXXX"
5. Orchestrator resumes from the new status

### Task Completion Notes

When a task reaches `COMPLETE`, run `docs/worklog/new-note.sh <task-slug>` to create a worklog note capturing what shipped (decisions made, surprises, follow-ups). Link the resulting note from the task's `main.md` `## Completion` section. The worklog feeds `/doc:sweep` for the daily summary — see `.notes-config.yml`.
