# ADR 0002: Inlet triage and observation routing

**Date:** 2026-05-10
**Status:** accepted (implemented in T148)
**Deciders:** Blake + pi-architect

## Context

ADR 0001 split the task engine's overloaded `status` into lifecycle, step, blocker, and integration axes. The upstream stores have the same overload in a different form.

Today raw signals enter through multiple surfaces and then move through `intake`, `observations`, and `architecture_reviews` with status values that mix several meanings:

- row lifecycle;
- current routing or investigation work;
- waiting/gate state;
- terminal result;
- produced downstream artifacts;
- architecture-review side effects.

This makes the front door hard to reason about. A row can be `needs_info`, `routed`, `dropped`, `investigating`, `ready`, `wont_fix`, or `resolved`, but those words do not belong to the same semantic axis. Some are active positions, some are gates, some are terminal outcomes, and some are references to other rows.

The design goal is a legible upstream flow that complements ADR 0001:

```text
INLET -> OBSERVATION -> TASK -> DONE
```

Raw signals should enter one dominant inlet, triage should either close them or canonicalize them, observations should carry approved intent and gates, and tasks should remain the engine-owned work pipeline defined by ADR 0001.

## Decision

### 1. Use a single dominant upstream chain

The upstream engine model is:

```text
raw signal
  -> inlet item
  -> triage decision
  -> observation, duplicate, architecture review, fast-track, or dropped signal
  -> approved observation contract
  -> task or closure
```

The inlet is the dominant front door for raw signal. Direct observation creation remains an escape hatch for explicitly human-supervised or system-owned paths, but the target architecture is inlet-first.

Observations are canonical work candidates, not raw filings. A task row still means approved work exists; proposal and ratification remain upstream in observation/contract space.

### 2. Share ADR 0001's lifecycle discipline, but do not force every store to have every axis

Every engine row should be understandable through these concepts:

```text
lifecycle       where this row is in its own life
current_step    what work is happening now, only when lifecycle is too coarse
waiting/gate    what prevents forward movement, if anything
outcome         how a closed row closed
references      what row/artifact this row produced, consumed, duplicated, or resolved
```

Tasks need the full split from ADR 0001 because they are long-running work items:

```text
lifecycle + active_step + integration_step + blocker overlay
```

Inlet rows are short-lived routing tickets. They do not need a separate step axis in the target model. Their lifecycle can directly name the routing phase.

Observations are longer-lived than inlet rows but less procedural than tasks. They need lifecycle plus contract state, waiting/gate overlays, outcome, and typed references.

Architecture reviews are review artifacts whose result can gate one or more observations. They are not part of the observation lifecycle, but an open architecture review is visible on affected observations as a waiting/gate overlay.

### 3. Reserve `active` and `done` for task semantics

ADR 0001 gives `active` and `done` precise task meanings:

- `active`: engine-owned candidate production is underway;
- `done`: shared truth mutation and required verification completed.

Upstream rows should not reuse those terms casually. Inlet, observation, and architecture-review rows close; tasks become done.

Use `closed` for upstream terminal lifecycle states. Use `outcome` to record how the row closed.

### 4. Target inlet model

Target inlet lifecycle:

```text
new | triaging | waiting | closed
```

Semantics:

| Lifecycle | Meaning |
|---|---|
| `new` | Raw signal captured; not yet triaged. |
| `triaging` | The router/gatekeeper is classifying the signal. |
| `waiting` | Classification cannot complete until narrow evidence or input arrives. |
| `closed` | Routing is complete; `outcome` and references say what happened. |

Target inlet waiting kinds:

```text
evidence_needed
triage_capacity
external_input
```

Target inlet outcomes:

```text
routed_to_observation
marked_duplicate
fast_tracked
escalated_to_architecture_review
dropped_as_noise
```

Target inlet causality/reference fields:

```text
produced_observation_id
produced_architecture_review_id
produced_task_id
produced_artifact_kind
produced_artifact_id
duplicate_of_id
```

Branching belongs in `outcome` plus typed references, not in lifecycle. Examples:

```text
I123 lifecycle = closed
I123 outcome = routed_to_observation
I123 produced_observation_id = L045
```

```text
I124 lifecycle = closed
I124 outcome = escalated_to_architecture_review
I124 produced_observation_id = L046
I124 produced_architecture_review_id = A007
```

```text
I125 lifecycle = closed
I125 outcome = marked_duplicate
I125 duplicate_of_id = L021
```

```text
I126 lifecycle = closed
I126 outcome = dropped_as_noise
```

Fast-track is not invisible magic. A fast-tracked inlet item must still leave a causal trail:

```text
I127 lifecycle = closed
I127 outcome = fast_tracked
I127 produced_observation_id = L047
I127 produced_artifact_kind = check | task | commit | other
I127 produced_artifact_id = ...
```

### 5. Target observation model

Target observation lifecycle:

```text
candidate | ready | in_progress | closed
```

Semantics:

| Lifecycle | Meaning |
|---|---|
| `candidate` | Canonical observation exists; contract/front gate is not yet approved. |
| `ready` | Approved contract exists; observation may open/promote work. |
| `in_progress` | Linked work or fast-track execution is underway. |
| `closed` | Observation no longer needs work; `outcome` and references say why. |

Target contract state:

```text
none | draft | approved
```

Contract approval is the upstream front gate. A candidate observation whose contract needs approval is not a task proposal yet; it is waiting on human-ratified intent. Architecture review may also gate contract approval.

Target observation waiting overlay:

```text
waiting: true | false
waiting_kind:
  info_needed
  architecture_review
  human_ratification
  linked_task_blocked
  external_dependency
```

Waiting is an overlay for observations, not a lifecycle state. An observation can remain `candidate`, `ready`, or `in_progress` while waiting on a gate or external dependency.

Target observation outcomes:

```text
addressed_by_task
addressed_by_commit
closed_as_duplicate
closed_wont_fix
merged_with_cluster
superseded
```

Target observation reference fields:

```text
linked_task_id
open_architecture_review_id
addressed_by_task_id
addressed_by_commit
duplicate_of_id
merged_into_id
superseded_by_id
```

Deduplication can happen at inlet time or observation time. Inlet-time dedup is best-effort classification before canonicalization. Observation-time dedup is later canonicalization after a row already exists. Both must produce typed references to the canonical row or cluster.

### 6. Target architecture-review model

Architecture reviews are typed review artifacts. They can gate multiple observations, and one observation should have at most one open architecture-review gate at a time unless a later design introduces first-class multi-gate records.

Target cardinality:

```text
architecture_review A### covers N observations
observation L### has at most one open architecture-review gate
architecture-review completion may update many linked observations atomically
```

Target architecture-review lifecycle:

```text
pending | reviewing | waiting | closed
```

Target architecture-review outcomes:

```text
local_fix_allowed
contract_reframe_required
merged_with_cluster
primitive_task_created
primitive_task_required
human_decision_required
withdrawn
superseded
```

Per-outcome effects:

| Outcome | Effect on linked observations |
|---|---|
| `local_fix_allowed` | Clear `waiting_kind = architecture_review`; observation may proceed to contract approval or work. |
| `contract_reframe_required` | Return affected contracts to draft/reconciliation; keep or switch waiting overlay until re-ratified. |
| `merged_with_cluster` | Close duplicate/sibling observations with `outcome = merged_with_cluster` and typed merge references. |
| `primitive_task_created` | Record produced task and link affected observations to that task or tracking policy. |
| `primitive_task_required` | Require a primitive task before affected observations can proceed. |
| `human_decision_required` | Switch affected observations to `waiting_kind = human_ratification` or equivalent human gate. |
| `withdrawn` | Clear or replace the architecture gate according to the replacement review, if any. |
| `superseded` | Point affected observations at the superseding review or gate. |

Architecture review is a gate on observations and also real review work. It should be visible either as an observation gate/drilldown or as its own cockpit lane when pressure warrants.

### 7. Watch/cockpit should render derived flow buckets, not raw statuses

The operator watches flow, not table internals. `stores watch` and related UI should prefer derived buckets over raw schema status names.

Example cockpit buckets:

```text
INLET              OBSERVATIONS        TASKS              ENGINE
new        12      candidate    8      queued      3      daemon ✓
triaging    2      ready        2      active      4      locks  ✓
waiting     1      in_progress  5      integration 1      runners ⚠
closed +7/day      closed +3/day      done +2/day
```

Metric semantics:

```text
count         current rows in bucket
arrival_rate  rows entering a lane over a rolling window
closure_rate  rows entering closed/done over a rolling window
age           oldest/p95 age among non-closed rows
```

Terminal/exhaust rows should be summarized by recent closure rate and hidden from main lists by default, with outcome available on drilldown.

### 8. Implementation should be read-model first

This ADR does not require an immediate schema rewrite. Preferred sequence:

1. Build a deterministic read/projection model over current `intake`, `observations`, and `architecture_reviews` states:

```text
current status/fields -> lifecycle / waiting overlay / outcome / typed references
```

2. Update `stores watch` and diagnostics to use the projection for inlet and observation buckets.
3. Add or normalize typed reference fields needed for causality where current soft-FKs are ambiguous.
4. Align architecture-review cardinality and per-outcome effects.
5. Only then migrate underlying schema states if the projection proves stable.

### 9. Primitives consumed and pressure created

This ADR consumes the Router primitive: inlet triage is the canonical router from raw signal to canonical observation, duplicate, architecture review, fast-track, or dropped signal.

It creates explicit pressure for several primitives:

| Primitive | Pressure |
|---|---|
| Causality | Every terminal outcome needs typed produced/duplicate/merged/addressed references. |
| Aggregation | Cluster-driven architecture reviews cover N observations and need coherent grouping. |
| Activity | Dropped or duplicate signals can resurface later; repeated pulses should be detectable without mutating old rows. |
| Decay | Waiting inlet/observation rows need stale-age semantics and pressure indicators. |

These primitives are not fully introduced by this ADR. The ADR names where the pressure appears so implementation work can sequence them deliberately.

## Example flow

```text
RAW SIGNAL
  │
  ▼
I### INLET
  lifecycle = new
  │
  ▼
triage
  lifecycle = triaging
  │
  ├─ insufficient evidence
  │     ▼
  │   lifecycle = waiting
  │   waiting_kind = evidence_needed
  │     │
  │     └─ evidence returns -> triaging
  │
  ├─ duplicate
  │     ▼
  │   lifecycle = closed
  │   outcome = marked_duplicate
  │   duplicate_of_id = I###/L###
  │
  ├─ noise
  │     ▼
  │   lifecycle = closed
  │   outcome = dropped_as_noise
  │
  ├─ normal signal
  │     ▼
  │   lifecycle = closed
  │   outcome = routed_to_observation
  │   produced_observation_id = L###
  │
  ├─ architecture risk / cluster threshold
  │     ▼
  │   lifecycle = closed
  │   outcome = escalated_to_architecture_review
  │   produced_observation_id = L###
  │   produced_architecture_review_id = A###
  │          │
  │          ▼
  │      L### waiting_kind = architecture_review
  │
  └─ fast-track
        ▼
      lifecycle = closed
      outcome = fast_tracked
      produced_observation_id = L###
      produced_artifact_id = check/task/commit
```

Observation path:

```text
L### OBSERVATION
  lifecycle = candidate
  contract_state = draft
  │
  ├─ architecture gate pending
  │     waiting = true
  │     waiting_kind = architecture_review
  │
  ├─ human contract/front-gate pending
  │     waiting = true
  │     waiting_kind = human_ratification
  │
  ▼
contract approved
  lifecycle = ready
  contract_state = approved
  │
  ▼
task opened / fast-track executing
  lifecycle = in_progress
  linked_task_id = T###
  │
  ▼
closed
  outcome = addressed_by_task / addressed_by_commit / closed_wont_fix / ...
```

## Alternatives considered

### Keep current inlet and observation statuses

Rejected as the target architecture. Current statuses work mechanically but mix lifecycle, routing step, waiting state, and terminal outcome in one enum.

### Give inlet rows lifecycle plus step axes like tasks

Rejected for the target model. Inlet rows are routing tickets. A separate step axis would add indirection without buying task-like parallelism or trust-boundary clarity. The asymmetry is deliberate: tasks need step axes; inlet rows do not.

### Make architecture review only an observation flag

Rejected. Architecture review gates observations, but it is also real review work that may cover multiple observations and produce tasks or doctrine changes. It deserves a typed artifact even if the cockpit initially renders it under observation gates.

### Let terminal branches be separate lifecycle states

Rejected. `routed`, `duplicate`, `dropped`, `wont_fix`, and `resolved` are outcomes, not row positions. Lifecycle should stay small; outcomes and typed references carry terminal meaning.

## Consequences

### Positive

- The upstream engine speaks the same lifecycle/overlay language as ADR 0001.
- The inlet can branch without bloating lifecycle states.
- Watch/cockpit can show flow pressure instead of raw status noise.
- Fast-track, duplicate, architecture-review, and dropped paths become auditable through outcomes and references.
- Architecture reviews can operate over clusters instead of pretending every review is 1-to-1.
- The model can be proven as a read projection before schema migration.

### Negative / costs

- Requires compatibility projection over existing statuses.
- Requires naming cleanup across docs, watch renderers, and agent briefs.
- Requires typed causality fields or a consistent reference convention where current soft-FKs are ambiguous.
- Requires careful migration to avoid breaking current intake/observation/architecture-review handlers.
- May expose pressure for primitives that are not yet first-class: Causality, Aggregation, Activity, and Decay.

## Implementation direction

This ADR should seed a follow-up implementation task. A reasonable phase-1/phase-2 vertical slice is:

1. Implement an upstream read model for inlet, observations, and architecture reviews that projects current rows into ADR 0002 lifecycle/waiting/outcome/reference buckets.
2. Update `stores watch`, detail views, and diagnostics to render those buckets for inlet and observations.
3. Add the minimal typed reference fields or compatibility mapping needed for produced/duplicate/escalated/addressed causality.
4. Leave destructive schema lifecycle migration for a later phase after the projection proves stable.

## Links

- Prior ADR: `docs/adr/0001-task-engine-lifecycle-and-integration.md`
- Task engine architecture: `docs/task-engine-architecture.md`
- Current flow map: `docs/flow-diagrams.md`
- UI/UX doctrine: `docs/ui-ux.md`
- Gatekeeper design: `docs/gatekeeper-design.md`
- Risk and cluster taxonomy: `docs/risk-and-cluster-taxonomy.md`
- Related primitives: `docs/primitives.md`

## Status

Implemented in task `T148` on 2026-05-11. Merge PR/commit: T148 implementation branch; final upstream merge commit to be recorded by the repository merge operator after this executor phase lands.
