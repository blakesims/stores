---
name: observation:triage
description: Pick the next open observation, classify it, and (for T3) capture the intent contract at the moment the user has context. Resolve in place when possible; spawn deeper agents only when needed.
requires_stores: [observations, gate]
default_invoker: ai_with_human
user_invocable: true
---

# Triage an observation

Pick one open item. Classify. Resolve at the lightest tier that fits. For T3,
**capture the intent contract right now** — the schema will reject a T3
triage without it, which is the whole point of doing this here.

## Discover the surface

```bash
stores observations --help
stores observations schema --json    # see triage + contract field shape
stores gate --help                    # for filing blockers if needed
```

The `contract` record is gated by a `required_when` rule on
`triage.verdict == 'T3'`. Translation: if you pass `--verdict T3`, the CLI
will refuse to commit unless `--done-when`, `--scope-in`, `--scope-out` are
also provided. Don't fight this — that's the contract enforcement working.

## Pick + lock

```bash
stores observations list --status open --json --limit 1
# pick the displayed display_id (e.g. L042)
```

(A `lock` verb is on the backlog; for v0.1 just race-tolerate by re-checking
status before writing.)

## Triage rubric

| Verdict | Meaning | Next action |
|---------|---------|-------------|
| **T1** | ≤2 files, ≤50 LOC, no migrations | Propose the fix inline, do it, mark resolved. No contract needed. |
| **T2** | ≤5 files, ≤200 LOC, single subsystem | Mark `--verdict T2`. Spawn an executor + reviewer (your call which mechanism). No contract needed. |
| **T3** | Migrations, cross-subsystem, capability-level change | **Capture the contract NOW** — see below. |
| **IN_FLIGHT** | A feat branch / task already covers it | Mark `--verdict IN_FLIGHT`, note the task_id. Stop. |
| **ALREADY_RESOLVED** | No longer occurring | Mark `--verdict ALREADY_RESOLVED`, cite the fix. Stop. |
| **CONTINUE** | Can't classify in ~10 tool calls | Spawn an investigator subagent; re-triage with their findings. |

## T3 path — capture the contract

Read the observation summary back to the user. Ask:

1. **DONE_WHEN** — one or two lines, written like a test assertion.
2. **In-scope** — what changes.
3. **Out-of-scope** — what should remain unchanged.
4. **Capability** (if your project tracks them) — which one this advances.

Then commit:

```bash
stores observations triage <id> \
    --verdict T3 \
    --done-when "<1-2 line outcome>" \
    --scope-in "<bullets, |-separated>" \
    --scope-out "<bullets, |-separated>" \
    --invoker ai_with_human
```

If the CLI rejects with a `required_when` error, you missed a field — read
the error, add the missing flag, retry. Never bypass.

## Mid-flow: hit a question you can't resolve?

File a gate decision and BLOCK rather than guessing:

```bash
stores gate add \
    --type decision \
    --question "<the question>" \
    --options "<a|b|c>" \
    --task-ref <observation-display-id> \
    --invoker ai_with_human
```

Then exit cleanly. The user picks up via `/gate:walk` later.

## Rules

- One observation per invocation. No batching.
- Triage before doing any work beyond ~10 tool calls.
- Never write `triage.verdict = T3` without the contract — the CLI enforces;
  don't try to work around it.
- The contract IS the alignment anchor for downstream automation. Get it
  right; the work after this depends on it.
