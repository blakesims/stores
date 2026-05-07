---
name: handover-pi-architect
description: Use during wind-down when acting as Pi architect and handing off architectural state, priorities, and pending decisions.
user_invocable: true
---

# Handover — Pi Architect

Goal: hand the next Pi only live architectural context and first action.

## Rules

- Do not duplicate SOP text or long session history.
- Capture decisions, priority order, pending architecture questions, and active thread path.
- Ensure any SOP edits you made are committed before handover.
- If a named priority lacks L###/I###/GAP, ask engine-controller to file/clarify or record it as a handover concern.

## Create the note

```bash
docs/worklog/new-note.sh --handover pi-architect
```

Read the printed path before editing.

## Fill only live state

Include:

- active/new thread path;
- current priorities from `docs/engine-health.md`;
- active tasks that may need Pi and why;
- architectural rulings issued this session with msg ids;
- pending decisions or doctrine risks;
- first exact next action for the next Pi.

Do not include implementation details unless architecturally relevant.
