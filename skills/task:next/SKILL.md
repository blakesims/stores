---
name: task:next
description: Pull the highest-priority ready task from the tasks store and run it autonomously via tasks:start. The intent contract is already on the task row (set at creation time); no human pause needed before the executor starts.
requires_stores: [tasks, observations, gate]
default_invoker: ai_autonomous
user_invocable: true
---

# Run the next queued task

The point of capturing the intent contract at creation time is that downstream
work needs no further human input to start. This skill drains the queue: pick
the highest-priority `ready` task, then hand it off to `tasks:start` to run
the full phase loop. No `/task:open` ritual; the contract is already on the
row.

## Discover

```bash
stores tasks list --status ready --limit 5
stores tasks show <id>
```

## Pick

Find the next ready task:

```bash
stores tasks list --status ready --limit 1 --sort updated_at
```

If the output is empty, no tasks are ready:

```bash
# Check for blocked tasks that may need human attention:
stores tasks list --status blocked
```

Surface "no ready tasks" to the user and stop. Do not invent work.

## Run

For a task with an existing row in `ready` status, spawn `tasks:start` with the
task ID:

```bash
stores tasks show <id>
```

Then invoke the orchestrator:

* `Task(subagent_type="tasks:start", prompt="<id>: <title> — run to completion")`

Or if running interactively, invoke:

```
/tasks:start <id>
```

The `tasks:start` orchestrator owns the full phase loop from that point.

## On BLOCKED

If the task transitions to `blocked` during `tasks:start` execution, `tasks:start`
will surface the blocker. This skill's job is only to find and hand off — not
to own the loop.

To check blocked tasks awaiting human input:

```bash
stores tasks list --status blocked
```

## Rules

- This skill is `ai_autonomous` — runs without human in loop. Never call
  `ask_user`-style synchronous prompts.
- Honor the contract verbatim. The executor's writes must trace back to the
  task's `done_when` — that is the alignment check.
- One task per invocation. The next call picks up the next.
- If the task's `done_when` is missing, fail loud — do not invent one.
- If no ready tasks exist, report clearly and stop.
