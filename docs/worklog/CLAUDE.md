---
description: Worklog conventions for transient notes.
---

## Worklog

Transient chronological notes. Created via `./new-note.sh`.

**Rules:**
- Always use `./new-note.sh` to create notes. Never create them manually — the script enforces naming, sequence numbers, and directory layout based on `.notes-config.yml`.
- After running `./new-note.sh`, call the **Read** tool on the returned path before calling Edit. Claude Code requires this.
- Edit sections with the Edit tool; never rewrite a note wholesale with Write.
- Slugs are kebab-case: `auth-bug-investigation`, `caching-decision`.

**Commands:**
- `./new-note.sh <slug>` — detailed note (auto-increments sequence)
- `./new-note.sh --summary` — today's daily summary (skipped in weekly-only cadence)
- `./new-note.sh --tomorrow` — pre-create tomorrow's daily summary
- `./new-note.sh --weekly` — this week's weekly summary

Promote durable insights into the refs directory (see `.notes-config.yml`) when they deserve permanence.
