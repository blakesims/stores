# tasks store

Schema-driven task tracking: planner → plan-reviewer → executor → code-reviewer.

## Install

```bash
stores install tasks
```

## Quick start

```bash
stores tasks add --title "My task" --slug "my-task" \
  --done-when "Feature X works" --scope-in "..." --scope-out "..."
stores tasks next-action T001        # → planner
stores tasks brief T001              # planner briefing
stores tasks submit-plan T001 --plan-from-file plan.json
stores tasks submit-plan-review T001 --gate READY --summary "..."
stores tasks submit-execute T001 --summary "..." --commit abc123 --files-changed "a.rs,b.rs"
stores tasks submit-review T001 --gate PASS --summary "All ACs met"
stores tasks render T001             # write tasks/active/T001-slug/main.md
```

## Workflow states

`planning` → `plan_review` → `ready` → `executing` → `code_review` → `complete`

Blocked on guard failure; ordinary `blocked` rows use `resume` to continue the work cycle (`blocked` → `ready`/`planning`). Plan-review: NEEDS_WORK 3× → BLOCKED. Code-review: REVISE 3× → BLOCKED.

Deploy ceremony failures enter `deploy_blocked`. After fixing the underlying deploy issue, run `stores tasks retry-deploy <id>` to retry the existing accept-merge → cargo-install → schema-migrate subscriber chain. If the work was recovered manually, run `stores tasks close-out-of-band <id> --commit <sha>` instead.
