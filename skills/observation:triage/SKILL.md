---
name: observation:triage
description: Pick the next open observation, classify it, and (for T3) capture the intent contract at the moment the user has context. Resolve in place when possible; spawn deeper agents only when needed.
requires_stores: [observations, gate]
default_invoker: ai_with_human
user_invocable: true
---

# Triage an observation

Pick one open item. Classify. Resolve at the lightest tier that fits. For T3,
**capture the intent contract right now** — the schema will reject a
`confirmed` transition until `intent_contract.contract_state == 'ready'`,
which is the whole point of doing this here.

## Discover the surface

```bash
stores observations --help
stores gate --help    # for filing blockers if needed
```

The `intent_contract` record is gated by a `required_when` rule on
`intent_contract.contract_state == 'ready'`. Translation: the `confirm`
transition guard requires `contract_state` to be `ready`; setting it to
`ready` without `objective`, `acceptance`, `in_scope`, `out_of_scope`,
`tier_hint`, `approved_by`, and `approved_at` will fail with all violations
listed in one error. Don't fight this — that's the contract enforcement working.

## Pick + lock

```bash
stores observations list --status open --json --limit 1
# pick the displayed display_id (e.g. L042)
```

(A `lock` verb is on the backlog; for now just race-tolerate by re-checking
status before writing.)

## Triage rubric

| Verdict | Meaning | Next action |
|---------|---------|-------------|
| **T1** | ≤2 files, ≤50 LOC, no migrations | Propose the fix inline, do it, mark resolved. No contract needed. |
| **T2** | ≤5 files, ≤200 LOC, single subsystem | Mark `--tier-hint T2`. Spawn an executor + reviewer (your call which mechanism). No full contract needed. |
| **T3** | Migrations, cross-subsystem, capability-level change | **Capture the contract NOW** — see below. |
| **IN_FLIGHT** | A feat branch / task already covers it | Note the `task_id`. Stop. |
| **ALREADY_RESOLVED** | No longer occurring | Update `resolution`, resolve. Stop. |
| **CONTINUE** | Can't classify in ~10 tool calls | Spawn an investigator subagent; re-triage with their findings. |

## T3 path — ratify the intent contract

Read the observation summary back to the user. Ask:

1. **Objective** — one line, what the fix achieves.
2. **Acceptance** — one or two lines, written like test assertions (list).
3. **In-scope** — what changes (list).
4. **Out-of-scope** — what should remain unchanged (list).
5. **Capability** (if your project tracks them) — which one this advances.

Then run the full ratify flow:

```bash
# Step 1: move open → investigating
stores observations investigate <id> --invoker ai_with_human

# Step 2: fill the intent contract (sets contract_state to ready)
stores observations update <id> \
    --contract-state ready \
    --objective "<one-line goal>" \
    --type work \
    --in-scope "<item>" --in-scope "<item>" \
    --out-of-scope "<item>" \
    --acceptance "<criterion>" --acceptance "<criterion>" \
    --tier-hint T3 \
    --approved-by <human-name> \
    --approved-at <YYYY-MM-DD> \
    --invoker human    # approved_by/approved_at carry actor: human; AI invokers are rejected

# Step 3: confirm (guard: contract_state == 'ready')
stores observations confirm <id> --invoker ai_with_human
```

If the CLI rejects at Step 2 with a `required_when` error, you missed a field —
read the error, add the missing flag, retry. Never bypass.

## Mid-flow: hit a question you can't resolve?

File a gate decision and BLOCK rather than guessing:

```bash
stores gate add \
    --type decision \
    --one-liner "<the question>" \
    --options "<a|b|c>" \
    --task-ref <observation-display-id> \
    --filed-by observation:triage \
    --source converge \
    --invoker ai_with_human
```

Then exit cleanly. The user picks up via `/gate:walk` later.

## Rules

- One observation per invocation. No batching.
- Triage before doing any work beyond ~10 tool calls.
- Never attempt `confirm` without `contract_state == 'ready'` — the CLI
  enforces it; don't try to work around it.
- The contract IS the alignment anchor for downstream automation. Get it
  right; the work after this depends on it.
