---
name: task:wrap
description: Wrap a completed task — read the synthesis brief and approve or reject.
---

Run: `stores tasks <id> guide --claude-code`

The task must be in status `in_review` (drive auto-fires `request_review` after PASS-on-last-phase). The guide agent will spawn in wrap-mode, render the synthesis brief (promise vs reality vs deviations), and accept your GO/NO_GO decision via `stores tasks accept` or `stores tasks reject --reason "..."`.
