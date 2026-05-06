# intake_items store

**Purpose:** Typed intake buffer for the gatekeeper layer. Local agents file friction here; the gatekeeper router agent classifies, deduplicates, fast-tracks, and escalates before creating observations.

**Design doc:** `docs/gatekeeper-design.md`

## When to file via `stores intake add` vs `stores observations add`

| Path | When to use |
|------|-------------|
| `stores intake add` | Autonomous friction filing from planner, executor, code_reviewer, or orchestrator agents during normal task drive cycles. The gatekeeper router then triages the intake item. |
| `stores observations add` | Human-grounded filings (`--invoker ai_with_human` or `--invoker human`) or escape-hatch filings that bypass the gatekeeper. Always valid; the gatekeeper layer is a filter, not a gate. |

## ID namespace

Intake items use the `I###` prefix (e.g. `I001`, `I042`). This is distinct from:
- Observations: `L###`
- Tasks: `T###`
- Gates: `G###`

## Lifecycle states

```
draft ──claim-triage──> triaging ──route (needs_info)──> needs_info
                          │                                    │
                          │                               recon-return
                          │                                    │
                          │◄───────────────────────────────────┘
                          │
                          ├──route (duplicate/fast_track/normal_observation)──> routed
                          ├──route (reject_noise)──> dropped ──reopen (ai_with_human)──> draft
                          └──escalate-arch-review──> escalated
```

- **draft** — filed by a local agent; not yet seen by the gatekeeper
- **triaging** — gatekeeper has claimed the row; structured decision in progress
- **needs_info** — gatekeeper needs recon before deciding; row held pending `recon-return`
- **routed** — terminal-success; points at one downstream artifact (observation, duplicate target, fast-track record)
- **escalated** — arch_review_candidate; routed to architecture review instead of observation
- **dropped** — terminal-noise; preserved for audit; only `reopen` (ai_with_human) recovers it

## Required fields

| Field | Required | Notes |
|-------|----------|-------|
| `summary` | always | One-line description |
| `source_agent` | always | Role filing the item (planner, executor, etc.) |
| `captured_at` | always | ISO timestamp |
| `captured_week` | always | Week label (e.g. w18-d3) |
| `duplicate_of` | when `decision == duplicate` | Soft-FK to I### |
| `routed_to_observation` | when `decision == normal_observation` | Soft-FK to L### |

## Filing an intake item

```bash
stores intake add --invoker ai_autonomous \
  --summary "brief one-liner" \
  --source-agent planner \
  --captured-at "$(date -Iseconds)" \
  --captured-week "w$(date +%V)-d$(date +%u)"
```

Optional: `--body`, `--source-task T###`, `--suggested-fix`.

## Route decisions

The gatekeeper emits one of six decisions via `stores intake route I001 --decision <D>`:

1. **duplicate** — links to an existing intake/observation; requires `--duplicate-of I###`
2. **needs_info** — defers to a recon agent; resumes via `recon-return`
3. **fast_track** — T0/T1 trivia; creates fast-track-eligible observation (classification only in P1)
4. **normal_observation** — standard path; creates observation; requires `--routed-to-observation L###`
5. **arch_review_candidate** — high-risk; routes to escalated via `escalate-arch-review`
6. **reject_noise** — terminal drop; `reopen` (ai_with_human) is the only recovery

## Invoker discipline

All transitions are `ai_autonomous` except `reopen` which is `ai_with_human`. The `reopen` transition reverses a machine-made `reject_noise` decision and is therefore a U-moment requiring human presence.

For details on the gatekeeper decision schema (gatekeeper_decision_json), routing side-effects, and the recon agent contract, see `docs/gatekeeper-design.md`.
