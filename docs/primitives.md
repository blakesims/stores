# Primitives

The substrate composes from a small set of typed primitives. This document is the single source of truth on what those primitives are, what we know we're missing, and the rules of discovery.

## Working draft, not closed set

This list is a **working draft, not a closed set**. Each design move that the existing primitives cannot compositionally express reveals a missing one. Do not shortcut the discovery — the realistic-pull doctrine (philosophy.md § *Pull from real use*) is what surfaces the gaps. New primitives appear here only when client-side or self-build pressure cannot be expressed with the existing set.

## What we have

| Primitive | Definition |
|---|---|
| **Buffer** | Typed store with a lifecycle (`observations`, `tasks`, `gate`). |
| **Transition** | Edge between states with guard, actor, and side-effects. |
| **Subscriber** | Consumer attached to a transition; often also a producer into the next buffer. |
| **Actor** | Typed identity with privileges (`framework`, `ai_autonomous`, `ai_with_human`, `human`). |
| **Direction** | Property of a buffer's interface to a consuming actor: *push* (producer's queue; consumer faces a perpetual inbox) or *pull* (shared buffer; consumer fetches when ready). At scale (5+ concurrent producers) only pull is sustainable — working memory is ~4 chunks; bundled push shreds it. |
| **Schema** | Type contract enforced at write-time (fields, `required_when`, per-field actor). |

## What we're missing

Surfaced by realistic-pull on real client work; named here so future design moves can compose against them or surface a fresh gap when they can't.

| Missing | What it would mean | Existing ad-hoc instances |
|---|---|---|
| **Loop** | Transition that returns to a prior state (e.g. `needs_info` back to filer; REVISE back to executor). | code_reviewer REVISE; `deploy_blocked` recovery |
| **Aggregation** | N rows into one summary row (batched dispositions, decision rollups). | none |
| **Decay** | Automatic transition on time-since-X (staleness, expiry). | none |
| **Notification** | Producer-initiated push to an actor when their pull-queue has new content. | partial: ntfy hooks tied to `deploy_blocked` |
| **Capacity** | Per-transition rate limit with route-around behavior (rate-pressure). | none |
| **Check** | Deterministic external evaluation gating a transition (linter, `./dev fallow audit`, `cargo test`, pre-commit hooks). Pass → forward. Fail → typed `<check>_blocked` recovery state with diagnostic captured (condition-pressure). | three ad-hoc: `cargo-install → deploy_blocked`; code_reviewer REVISE; `current_phase < plan.phases.length` guard |
| **Causality** | Queryable provenance across the buffer graph (this `T###` caused by `L###` caused by `L###` upstream). | implicit in `transition_history` + soft FKs; no query surface |
| **Activity** | Typed event capturing an actor's attention against a row (read, search-hit, mention, touch) **without** changing row state. Distinct from Transition (no state change) and Causality (actor→row, not row→row). Composes with Decay (low-attention rows demote / fizzy-sink), Aggregation + Causality (intelligent dedup: similarity + co-temporal touches → merge candidates), Notification (proactive resurfacing of repeatedly-searched themes), Refinement (triage-agent uses Activity signal to weight similarity matches at the inlet). | none — reads / searches / mentions are uninstrumented today |
| **Router** | Active classification point that routes a candidate row into another buffer or terminal family when its semantic kind is not known at filing time. Loops preserve identity inside one lifecycle; routers fork candidates into different journeys. | gatekeeper/intake: raw filing → mature observation / duplicate / dropped-noise / needs_info recon / architecture review / task / security escalation |

## Composition rules

An *engine* in this frame is a graph of buffers connected by subscribers. Throughput is measurable at every edge — each crossing writes a `transition_history` row — and fixes localize to a single buffer or transition by design. The btop-style flow visualization is the right mental model: pipes carrying typed rows, junctions transforming them, throughput per pipe.

**Loop inside a buffer; fork across buffers.** Use lifecycle states/transitions when the row remains the same semantic object on the same journey (e.g. code-review REVISE returns a task to execution). Use a Router and often a separate buffer when classification changes the object's identity, schema, actor surface, or terminal family (e.g. raw intake can become an observation, duplicate, dropped noise, recon request, architecture review, or task). When the line is unclear, prefer adding states first; fork only when post-classification entities have meaningfully different lifecycles, invariants, or actor gates. Do not turn a real classification fork into a pile of overloaded statuses just to avoid a store, but also do not create buffers for minor loop variations.

**Specialization is by transition, not by role.** A drive "cycle" is not a hard-coded sequence; it is N transition-subscribers in `agents.yaml`, each owning one edge — including LLM-backed subscribers declared the same shape as rust builtins. The monolithic `tasks drive` is a transitional artifact, not a primitive.

**Stores is the decision-routing substrate for every agent**, not just for tasks-as-work. When an agent stalls on a human decision, the decision is filed as a typed row (`actor: human`) in a buffer the human pulls from. The human drains a unified typed inbox across all in-flight agents, instead of context-switching across N agent contexts to discover what is blocked. The same architectural move as "DB is truth, not markdown," applied to the human-in-the-loop interface itself.

## Boundaries

| Inside the substrate | Outside |
|---|---|
| Declaration that a transition is gated, observed, or measured | The implementation of the gate (linter, CI, `./dev` script, git hook) |
| Routing of pass/fail to typed states | The diagnostic format an external check emits |
| Recovery transitions | The fix itself |
| Measurement of edge throughput (the data) | The visualization layer (btop view, dashboards) |
| The CLI as the only write path | The wrapper / orchestrator above the CLI |

The substrate's job is to enforce workflow structure and route flow through typed buffers. Implementations of checks, drives, and external tools live above the substrate and call its CLI like any other client. See `docs/philosophy.md` § *What's outside the substrate*.

## Changelog

- **2026-05-04** — initial draft. Six named (Buffer, Transition, Subscriber, Actor, Direction, Schema). Seven missing (Loop, Aggregation, Decay, Notification, Capacity, Check, Causality). Three composition rules (engine-as-graph; specialization-by-transition; decision-routing-substrate). Surfaced by realistic-pull on 10.06 client work + the discussion captured in `docs/worklog/2026-05-04/03-primitives-and-engine-metaphor.md`.
- **2026-05-04** (same-day addition) — added **Activity** as an 8th missing primitive (typed event for actor's attention against a row, distinct from state-changing Transitions). Surfaced twice in one session: (1) the dedup-as-search-operation question, (2) the fizzy-kanban reframe (recency / inactivity-decay needs an attention signal, not just timestamps). The substrate today captures writes; Activity is the missing capture of *reads*, *mentions*, *search-hits*. Composes with already-missing Decay, Aggregation, Causality, Notification, and the Refinement composition pattern.
- **2026-05-06** — added **Router** as a missing primitive and codified the loops-vs-forks composition rule. Surfaced by L138/T045 gatekeeper design: raw local filings are not immature observations; they are candidate signals whose classification can fork into different buffers/terminal families.
