---
name: task:next
description: Pull the highest-priority queued task and run it autonomously. The intent contract is already on the task (set at triage time); no human pause needed before the executor starts.
requires_stores: [tasks, observations, gate]
default_invoker: ai_autonomous
user_invocable: true
---

# Run the next queued task

The point of capturing the intent contract at triage time is that downstream
work needs no further human input to start. This skill drains the queue: pick
the highest-priority `READY` or `QUEUED` task, advance its status, run the
phase loop, write results back. No `/task:open` ritual; the contract is
already there.

## Pre-requisite: a `tasks` store

v0.1 of the framework does NOT ship a `tasks` store — it has `observations`
and `gate` only. This skill is forward-looking. Until tasks ships:

- The pattern still works against `observations` directly (a T3 observation
  with a `contract` is queue-able).
- Substitute `stores tasks` calls with `stores observations` calls; treat
  T3 + status=`triaged` + contract-present as the queue.

## Discover

```bash
stores tasks --help                     # or stores observations --help
stores tasks schema --json
```

## Pick

Highest-priority `READY` (or `QUEUED`) task:

```bash
stores tasks list --status READY --sort priority_rank --json --limit 1
# fall back to observations if no tasks store yet:
stores observations list --status triaged --has-contract --sort priority_rank --json --limit 1
```

## Run

For a task with `contract` already populated:

1. Transition to `EXECUTING_PHASE_1`:
   `stores tasks update <id> --status EXECUTING_PHASE_1`
2. Spawn the executor (Task subagent, pi-agent, whatever your runtime uses).
   Pass the contract verbatim as the alignment anchor.
3. On phase completion, append to execution_log:
   `stores tasks update <id> --execution-log-phase-1-status COMPLETE \
       --execution-log-phase-1-commits "<sha>" ...`
4. Repeat per phase until done or BLOCKED.
5. On COMPLETE: `stores tasks update <id> --status COMPLETE`.

## On BLOCKED

If any phase produces an open question:

```bash
stores gate add --type decision \
    --question "<the question>" \
    --options "<a|b|c>" \
    --task-ref <task-id> \
    --invoker ai_autonomous
stores tasks update <id> --status BLOCKED
```

The run terminates cleanly. The task waits for the human to answer the gate
item. A future re-run of `/task:next` (or a daemon) sees the gate is
answered and resumes.

## Rules

- This skill is `ai_autonomous` — runs without human in loop. Never call
  `ask_user`-style synchronous prompts; only async via gate.
- Honor the contract verbatim. The executor's writes must trace back to the
  contract's `done_when` — that's the alignment check.
- One task per invocation. The next call picks up the next.
- If the task's contract is missing, fail loud — do not invent one.
