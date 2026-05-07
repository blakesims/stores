---
name: handover-reviewer-runner
description: Use during wind-down when acting as reviewer-runner and handing off codex/review state to the next reviewer-runner.
user_invocable: true
---

# Handover — Reviewer Runner

Goal: preserve review/codex state without taking substrate ownership.

## Rules

- Remain read-only: no substrate writes, no code edits, no commits, no accepts.
- You may leave detached codex processes running only if the next reviewer can find them.
- Always record PID, command, worktree, task, commit, log path, and expected digest destination.
- Do not spawn new codex reviews after wind-down unless substrate-agent explicitly dispatched them and you can record handoff state.

## Create the note

```bash
docs/worklog/new-note.sh --handover reviewer-runner
```

Read the printed path before editing.

## Fill only live state

Include, in this order (the first item is non-negotiable — the next reviewer needs it before doing anything else):

- **Active thread path** — the full path under `/home/blake/repos/.agent-comm/threads/`. The thread is different each session; the next reviewer must NOT init a fresh one. Spell the path out; do not reference it as "the thread" or "today's thread."
- codex processes still running: PID, task, commit, worktree, log path, stdin closed yes/no;
- review queue awareness: in-flight, ready pings expected, parked tasks;
- last digest/result per active task;
- tooling caveats (e.g. codex stdin must be `</dev/null`; `Monitor` tool is deferred and must be loaded with `ToolSearch select:Monitor` before use);
- first exact next action for the next reviewer-runner.

Do not include full digests; link message ids or log paths.
