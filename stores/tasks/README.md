# tasks store

Schema-driven task tracking with a 4-agent workflow: planner → plan-reviewer → executor → code-reviewer.

## Install

```bash
stores install tasks
```

## Quick start

```bash
stores tasks add --title "My task" --slug "my-task" \
  --done-when "Feature X works end-to-end" \
  --scope-in "..." --scope-out "..."

stores tasks next-action T001        # → planner
stores tasks brief T001              # planner briefing
stores tasks submit-plan T001 --plan-from-file plan.json
stores tasks submit-plan-review T001 --gate READY --summary "..."
stores tasks submit-execute T001 --summary "..." --commit abc123
stores tasks submit-review T001 --gate PASS --summary "All ACs met"
stores tasks render T001             # write tasks/active/T001-slug/main.md
```

## Workflow states

`planning` → `plan_review` → `ready` → `executing` → `code_review` → `complete`

Blocked paths: any state can reach `blocked` on guard failure; `resume` returns to `executing`.

## Cycle limits

- Plan review: NEEDS_WORK allowed 3× before auto-BLOCKED
- Code review: REVISE allowed 3× (cycles 2/3/4) before auto-BLOCKED
