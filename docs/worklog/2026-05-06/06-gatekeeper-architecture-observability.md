# Gatekeeper Architecture Observability

**Date:** 2026-05-06
**Type:** note

## Summary

Brainstormed the missing oversight layer: the system has local observability but not architectural observability. Local agents find local pain and file observations, but repeated local fixes can nudge the architecture in the wrong direction. The likely solution is not to run an expensive global architecture review on every raw filing, but to introduce a gatekeeper/router layer that classifies, deduplicates, requests recon, fast-tracks trivial work, and escalates architecture-risk clusters to an architecture-review agent.

## Details

### Problem statement

Agents currently see their exact workflow and file local observations. Those observations may include suggested fixes. A human may approve quickly, or not see the architectural implications. Each local fix can be correct in isolation while still splintering the global architecture.

Examples from today's review:

- T1 failures led to local fixes around `plan = null`, but the deeper issue is that contract-is-plan is represented as absence rather than a normalized execution shape.
- Dispatch failures led to local fixes around stale PIDs, retries, skip-historical locks, and zombie detection, but the deeper issue is that dispatch attempts are an unmodeled lifecycle buffer.
- Sidecar convenience led toward eager token propagation, but the deeper issue is authority-surface drift.

The system therefore needs a middle scale between local observation and global philosophy review.

### Proposed shape: intake/gatekeeper before observations

Instead of letting every agent file directly into mature observations, local agents file raw intake drafts. A specialized gatekeeper/router reviews the draft and decides where it belongs.

Possible flow:

```text
raw intake draft
  -> gatekeeper/router
    -> duplicate / merge
    -> needs_info / recon
    -> trivial fast-track
    -> normal observation
    -> architecture-review candidate
    -> dropped/noise
```

Local agents remain free to report pain. They are not expected to classify architecture correctly.

### Gatekeeper responsibilities

The gatekeeper should be cheap and specialized. It should not run full global synthesis every time. Its responsibilities:

1. Check for duplicates and likely clusters.
2. Decide whether evidence is sufficient.
3. If not sufficient, route to `needs_info` and trigger a recon agent.
4. Assign size tier and, more importantly, risk flags.
5. Fast-track trivial low-risk items.
6. Route normal work into the observation lifecycle.
7. Escalate architecture-risk items or repeated clusters to architecture review.
8. Avoid queue pollution from noise or already-resolved symptoms.

Possible structured output:

```json
{
  "decision": "normal_observation | duplicate | needs_info | fast_track | arch_review | reject_noise",
  "confidence": "low | medium | high",
  "tier_hint": "T0 | T1 | T2 | T3",
  "risk_flags": [
    "touches_actor_authority",
    "touches_lifecycle",
    "touches_subscriber_semantics",
    "introduces_new_primitive",
    "changes_boundary",
    "security_sensitive",
    "docs_only",
    "small_local_fix",
    "duplicate_symptom"
  ],
  "duplicate_candidates": ["L123"],
  "cluster_key": "t1-null-plan",
  "missing_info_question": "...",
  "recommended_next": "...",
  "rationale": "..."
}
```

### Recon agent

When the gatekeeper chooses `needs_info`, a narrow recon agent gathers missing evidence. Its brief should be explicitly constrained: gather reproduction/evidence, do not design the solution. The result returns to the gatekeeper for re-routing.

This prevents the orchestrator-on-main from doing long inline investigations while also keeping investigation inside substrate-visible flow.

### Architecture-review agent

The architecture agent is expensive and global. It should not inspect every typo or every raw intake item. It reviews selected material:

- gatekeeper-flagged architecture-risk items;
- clusters whose count crosses a threshold;
- risky contracts before U1 ratification;
- periodic windows of completed tasks + pending observations;
- security/authority/lifecycle/subscriber/runner changes.

Its job is coherence, not code review. It asks whether local fixes contradict philosophy/primitives or reveal a missing abstraction.

Possible architecture-review outputs:

- allow local fix;
- reframe/amend contract;
- merge with existing cluster;
- create primitive-level task;
- block pending local fixes;
- propose doctrine/doc update;
- request human architecture decision.

### When architecture review should fire

Not on every item. Candidate triggers:

1. **Risk flag trigger** — fire or queue review for actor authority, lifecycle, subscriber semantics, runner boundary, approval-token, schema core, admin/recovery verbs, new primitive, or contradiction flags.
2. **Cluster threshold trigger** — if the same `cluster_key` appears repeatedly, e.g. `t1-null-plan`, `dispatch-lifecycle`, `sidecar-token`, route the cluster to architecture review.
3. **Pre-ratification trigger** — before approving risky contracts, require architecture review to say `ok_local_fix`, `reframe_contract`, `merge_with_cluster`, or `needs_human_arch_decision`.
4. **Periodic sweep** — daily or after N shipped tasks, review recent completed work, pending contracts, and new observations.
5. **Post-accept batch** — after 5-10 shipped tasks, look for aggregate drift.

### Fast-track / auto-approval

Fast-track is necessary or the system will drown in human gates and expensive drive cycles. But it needs hard boundaries.

Fast-track candidates:

- docs typo or narrow rendering fix;
- snapshot/test fixture update;
- small template wording change;
- obvious low-risk CLI display improvement;
- observation closure/reconciliation;
- narrow T1 fix with no schema/lifecycle/actor/subscriber impact.

Never fast-track:

- actor/approval-token changes;
- lifecycle transitions;
- schema core changes;
- daemon/subscriber semantics;
- runner behavior;
- cross-project authority;
- admin/recovery verbs;
- deploy ceremony;
- secrets/security;
- anything with architecture-risk flags.

Fast-track should still be audited: gatekeeper decision, tiny execution path, deterministic check, auto-review if appropriate, terminal closure.

### Tier is not risk

A major insight: size/complexity tier and architectural risk are different dimensions. A 30-line approval-token change may be tiny but high-risk. A large generated snapshot may be big but low-risk.

The system likely needs orthogonal fields:

```text
size_tier: T1 | T2 | T3
risk_class: low | normal | architecture | security | authority
approval_policy: auto | human | architecture
```

The gatekeeper's most important contribution may be assigning risk class, not tier.

### Minimal implementation path

Start with a new intake buffer or draft layer rather than immediately building full global architecture review.

Candidate store:

```text
intake_items
```

Possible states:

```text
draft -> triaging -> needs_info -> routed | dropped
```

Core fields:

```text
summary
body
source_task
source_agent
suggested_fix
evidence
gatekeeper_decision
risk_flags
cluster_key
duplicate_of
routed_to_observation
routed_to_arch_review
```

Longer-term, add an `architecture_reviews` store. But the gatekeeper can initially route architecture-risk items into tagged observations or worklog notes until the dedicated store exists.

## Follow-ups

- Consider filing a primitive/doctrine observation: local correctness is not architectural coherence.
- Design an `intake_items` buffer and gatekeeper role before adding more direct observation automation.
- Define risk flags and cluster keys as first-class fields.
- Separate size tier from risk class and approval policy.
- Add architecture-review triggers based on risk flags, cluster thresholds, and periodic/post-accept sweeps.
- Ensure fast-track remains audited and cannot apply to authority/lifecycle/schema/subscriber/security changes.
