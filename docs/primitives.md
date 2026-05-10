# Primitives

## intent_contract.harden_log

`observations.intent_contract.harden_log` is a nullable bounded derivation audit for high-leverage contract hardening. It stores structured rationale (decisions, scope cuts, alternatives rejected, compress-vs-surface judgments, source quotes, unresolved questions) that produced the final contract; it is analogous to `plan_review_log` as durable rationale, but it is not coupled to plan review and is not transcript-like chain-of-thought storage.

Repository inspection found no substrate-controlled `/intent-harden` or harden prompt under `agents/` or `skills/`; that Claude Code skill lives outside this repository. Substrate-side hardening guidance is therefore encoded here, in `agents/investigator.md`, and in `agents/sidecar/system-prompt.md`: populate `intent_contract.harden_log` when structured derivation reasoning is produced, but never make its presence a U1 ratification gate.


The substrate composes from a small set of typed primitives. This document is the single source of truth on what those primitives are, what we know we're missing, and the rules of discovery.

## Working draft, not closed set

This list is a **working draft, not a closed set**. Each design move that the existing primitives cannot compositionally express reveals a missing one. Do not shortcut the discovery — the realistic-pull doctrine (philosophy.md § *Pull from real use*) is what surfaces the gaps. New primitives appear here only when client-side or self-build pressure cannot be expressed with the existing set.

Primitives is one of the two meta primitives of the substrate (the other is Philosophy). Architect-led, human-ratified amendments — see `docs/heart-and-architect.md` for the constitutional-governance shape.

## What we have

| Primitive | Definition |
|---|---|
| **Buffer** | Typed store with a lifecycle (`observations`, `tasks`, `gate`). |
| **Transition** | Edge between states with guard, actor, and side-effects. |
| **Subscriber** | Consumer attached to a transition; often also a producer into the next buffer. |
| **Actor** | Typed identity with privileges (`framework`, `ai_autonomous`, `ai_with_human`, `human`). |
| **Direction** | Property of a buffer's interface to a consuming actor: *push* (producer's queue; consumer faces a perpetual inbox) or *pull* (shared buffer; consumer fetches when ready). At scale (5+ concurrent producers) only pull is sustainable — working memory is ~4 chunks; bundled push shreds it. |
| **Schema** | Type contract enforced at write-time (fields, `required_when`, per-field actor). |
| **Check** | Code-level deterministic gate with named id, typed JSON args, evaluator, and structured `CheckResult` (`pass`/`fail`, `check_id`, `args`, `observed_at`, `reason`). In this slice Check is not a schema declaration, policy engine, expression language, or replacement for `validate(...)`; schema validators remain at write-time validation. Postconditions are one application of Check: subscriber completion can evaluate a Check and persist/audit its shaped pass/fail result without new tables. |
| **ResourceLock** | DB-backed capacity/fencing primitive for exclusive shared resources (`main_branch`, deploy target, schema migration lane, shared test DB). Locks are acquired/released/recovered through CLI/framework transitions, carry owner + fencing/attempt metadata, and are checked by truth-mutating transitions. ResourceLock promotes the earlier missing Capacity pressure into a concrete primitive for serialized truth mutation. |

## What we're missing

Surfaced by realistic-pull on real client work; named here so future design moves can compose against them or surface a fresh gap when they can't.

| Missing | What it would mean | Existing ad-hoc instances |
|---|---|---|
| **Loop** | Transition that returns to a prior state (e.g. `needs_info` back to filer; REVISE back to executor). | code_reviewer REVISE; `deploy_blocked` recovery |
| **Aggregation** | N rows into one summary row (batched dispositions, decision rollups). | none |
| **Decay** | Automatic transition on time-since-X (staleness, expiry). | none |
| **Notification** | Producer-initiated push to an actor when their pull-queue has new content. | partial: ntfy hooks tied to `deploy_blocked` |
| **Capacity** | Per-transition or per-resource rate limit with route-around behavior (rate-pressure). ResourceLock covers exclusive shared-resource capacity; broader adaptive capacity/rate-pressure remains open. | ResourceLock for `main_branch` / deploy / shared test resources (ADR 0001 target) |
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

## Brief-at-dispatch persistence

`agent_runs.brief_text TEXT` (nullable, additive) and `plan_review_log[].reviewed_plan` (JSON snapshot, nullable) are the **brief-at-dispatch persistence** primitive. Together they answer the operator question "what exactly did this agent see at dispatch?" without requiring lossy re-generation from current row state.

`agent_runs.brief_text` is populated by the drive spawn handler at the moment `runner.spawn(...)` is called — capturing the bytes that `render_template_with_overlay` produced from the row state at that instant. The value is verbatim; no truncation or transformation. `plan_review_log[].reviewed_plan` is a snapshot of `tasks.plan` taken at `submit-plan-review` time; subsequent mutations to `tasks.plan` (plan-reviewer NEEDS_WORK cycles, re-submissions) cannot retroactively alter the snapshot.

Cross-references:
- **L059** — the `agent_runs` index foundation that this builds on; L503-A adds `brief_text` to the existing runs table rather than creating a separate artifact table.
- **L504-A** — the separate slice that will enforce contracts on these persisted artifacts (prompt-length guards, required-field checks). L503-A persists; L504-A enforces. These are deliberately separate: enforcement gates require runtime integration that is out of scope for the cheap L503-A slice.
- **L012** — the operator inspector view that will surface `agent_runs.brief_text` and related artifacts in a human-readable dashboard. L503-A makes the data durable; L012 surfaces it.

The `cycles[].executor.external_review_id` soft-FK back-link (planned to correlate an executor cycle with the external_review respawn that triggered it) is deferred to a follow-up slice; see the L503-A module docstring in `src/handlers/submit.rs`.

## Changelog

- **2026-05-04** — initial draft. Six named (Buffer, Transition, Subscriber, Actor, Direction, Schema). Seven missing (Loop, Aggregation, Decay, Notification, Capacity, Check, Causality). Three composition rules (engine-as-graph; specialization-by-transition; decision-routing-substrate). Surfaced by realistic-pull on 10.06 client work + the discussion captured in `docs/worklog/2026-05-04/03-primitives-and-engine-metaphor.md`.
- **2026-05-04** (same-day addition) — added **Activity** as an 8th missing primitive (typed event for actor's attention against a row, distinct from state-changing Transitions). Surfaced twice in one session: (1) the dedup-as-search-operation question, (2) the fizzy-kanban reframe (recency / inactivity-decay needs an attention signal, not just timestamps). The substrate today captures writes; Activity is the missing capture of *reads*, *mentions*, *search-hits*. Composes with already-missing Decay, Aggregation, Causality, Notification, and the Refinement composition pattern.
- **2026-05-06** — added **Router** as a missing primitive and codified the loops-vs-forks composition rule. Surfaced by L138/T045 gatekeeper design: raw local filings are not immature observations; they are candidate signals whose classification can fork into different buffers/terminal families.
- **2026-05-07** — promoted **Check** from missing primitive to code-level primitive. Initial registry covers `drive_pid_recorded_or_terminal` and `gatekeeper-decision-valid`; schema validators remain `validate(...)`, and postconditions are a Check application.
- **2026-05-09** — T111 / L503-A first slice: added **Brief-at-dispatch persistence** section naming `agent_runs.brief_text` + `plan_review_log[].reviewed_plan` as the durable artifact-pairing primitive. Closes the lossy-regeneration gap surfaced by c0f45ff/5b6a41a + I022/I027.
- **2026-05-10** — ADR 0001 promoted **ResourceLock** as a concrete primitive for capacity-1 shared truth mutation (`main_branch`, deploy/schema/shared-test resources). Broader adaptive Capacity remains in the missing list.
