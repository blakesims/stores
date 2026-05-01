---
name: task:wrap
description: Wrap a completed task — read the synthesis brief and approve or reject.
---

Run: `stores tasks <id> guide --claude-code`

The task must be in status `in_review` (drive auto-fires `request_review` after PASS-on-last-phase). The guide agent will spawn in wrap-mode and render the synthesis brief (promise vs reality vs deviations). The actual `stores tasks accept` / `stores tasks reject --reason "..."` commands are human-run — the framework refuses AI invokers via `actor: human` enforcement.
