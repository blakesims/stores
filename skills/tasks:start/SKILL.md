---
name: tasks:start
description: Drive the next workflow task to completion via Claude Code.
---

Invoke from the shell:

    stores tasks drive --auto --claude-code

This selects the next non-complete task by `created_at ASC`, spawns the
appropriate agent for the current workflow state via `claude -p`, and loops
until the task reaches `complete` or `blocked`.

If `blocked`, run `stores gate <id> guide --claude-code` to invoke the
guide agent on the blocking gate.

See `stores tasks drive --help` for flags (`--max-iters`, `--mock`).
