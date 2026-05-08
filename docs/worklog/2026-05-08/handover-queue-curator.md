# Handover — queue-curator

**Date:** 2026-05-08
**Type:** handover
**Role:** queue-curator

## Active thread

`/home/blake/repos/.agent-comm/threads/2026-05-08-01-stores-2026-05-08-session.md`

## Why this role exists

Queue-curator is a temporary hand-crafted front-of-engine fidelity role. The back end of the engine now has substrate-native external review and stronger retry/rebase behavior; the front end still needs manual curation of observations/intake/tasks, watch semantics, duplicates, stale rows, and priority ordering.

Treat your work as the manual prototype of the native triage/scheduler system we intend to build. Report what feels easy, what is awkward, what schema is missing, and which CLI verbs are insufficient.

## Current state at creation

- Open observations are down from 357 to ~30 after the 2026-05-08 backlog cleanup.
- T098/L480 cockpit attention-protection is still active and should improve `stores watch` semantics.
- Substrate-native review is canonical; reviewer-runner is fallback/audit only.
- Durable front/recovery follow-ups filed today include:
  - L489 stale-binary watchdog
  - L492 schema/DDL drift durability
  - L497 review-output/parser durability
  - L498 L488 recovery durability
  - L486 canonical mainline control-plane doctrine

## First step for queue-curator

1. Read `.claude/skills/queue-curator/SKILL.md`.
2. Join the active thread as `queue-curator`.
3. Start the queue monitors from the skill.
4. Produce an initial `QUEUE-SNAPSHOT` with:
   - task actionable counts,
   - observation counts by status,
   - intake counts by status,
   - ready/draft contract rows,
   - top duplicate clusters if any,
   - top 5 recommended next triage actions.

## Programmatic commands to use

Read/query:

```bash
stores watch --json --all
stores tasks status --json
stores observations list --json --status open --sort display_id
stores intake list --json --status draft --sort display_id
sqlite3 .stores/db.sqlite "SELECT status, count(*) FROM observations GROUP BY status;"
sqlite3 .stores/db.sqlite "SELECT summary, count(*) FROM observations WHERE status='open' GROUP BY summary HAVING count(*) > 1 ORDER BY count(*) DESC;"
```

Cleanup verbs available today:

```bash
stores observations close_as_addressed L### --resolution T### --resolution-kind addressed_by_task --invoker ai_autonomous
stores observations close_as_addressed L### --resolution L### --resolution-kind addressed_by_observation --merge-target-id L### --invoker ai_autonomous
stores observations wont_fix L### --invoker ai_with_human
stores intake route I### --decision <decision> --invoker ai_autonomous ...
```

Important: use `close_as_addressed` for open observations; `resolve` is not the open-row closure verb.

## Guardrails

- No raw SQL writes.
- Do not implement code.
- Do not make architecture/security/schema/lifecycle/authority decisions; route to Pi.
- Use subagents for bulk analysis; do not fill your own context with hundreds of rows.
- Do not close ready contracts or ambiguous rows without Pi/Blake.

## What to feed back

For every triage batch, note:

- Which categories were mechanical vs judgment-heavy.
- Missing schema fields or awkward CLI verbs.
- Whether existing L084/L085/L173/L486/L492/L497/L498 cover the friction.
- Whether `stores watch` buckets match the action the operator should take.
