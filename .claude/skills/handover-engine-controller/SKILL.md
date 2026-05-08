---
name: handover-engine-controller
description: Use during wind-down when acting as engine-controller and preparing the next engine-controller agent handover.
user_invocable: true
---

# Handover — Engine Controller

Goal: preserve the running engine state so the next engine-controller can resume without guessing.

## Wind-down

When Blake says wind down:

- no new ratifications or widening unless Blake reverses;
- do not spawn new Claude subagents except to preserve/finish already-active work;
- let detached reviewer/codex continue only if reviewer-runner records PID/log/handoff;
- write your own handover with `docs/worklog/new-note.sh --handover engine-controller`;
- include active tasks, branches, worktrees, commits, subprocess/subagent PIDs, blockers, first next action;
- create the next agent-comm thread only after all role handovers exist, then tell Blake the path.


## Rules

- Stop widening: no new ratifications or task starts unless Blake reverses.
- Do not spawn new Claude subagents during handover except to preserve/finish already-active work.
- Do not kill active Claude subagents silently; they are attached to this session and may die with it.
- No raw SQL, no broad cleanup, no unrelated staging.
- Use the worklog script for the note; do not hand-name files.

## Create the note

```bash
docs/worklog/new-note.sh --handover engine-controller
```

Read the printed path before editing.

## Fill only live state

Include:

- active thread path;
- daemon/CLI health;
- active tasks with status, branch, worktree, commit, next action;
- active Claude subagents: purpose, parent/session risk, expected output, whether to wait;
- deploy/accept/rebase blockers;
- dirty worktrees/stashes that must not be dropped;
- first exact next action for the next engine-controller.

Do not paste SOP or long history. SOP belongs in skills; detailed history belongs in worklog/agent-comm.

## New thread

After all three role handovers exist, engine-controller creates the next thread and posts links to the notes. Then wait for Blake to start the three next agents and say “start the engine.”
