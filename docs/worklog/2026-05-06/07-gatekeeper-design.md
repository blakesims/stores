# Gatekeeper Design

**Date:** 2026-05-06
**Type:** note

## Summary

Scoped design artifact for L138 / T045: introduce a middle-scale intake/gatekeeper layer between local agents and mature observations. Goal: preserve cheap local filing while preventing duplicate symptoms, under-evidenced reports, unsafe fast-tracks, and local fixes that accumulate into architectural drift. This note is intended as canonical design input for T045 review.

## Details

## 1. Core problem

The system currently has local observability: agents can notice pain in their own workflow and file observations. It lacks architectural observability: no routine step clusters those local reports against philosophy, primitives, recent shipped tasks, pending contracts, and related observations.

Therefore local fixes can be individually correct but globally wrong. Examples:

- T1 null-plan symptoms suggested local guard/template patches, but collectively reveal the wrong execution representation.
- Dispatch symptoms suggested local watchdog/retry/PID fixes, but collectively reveal that dispatch attempts are an unmodeled lifecycle buffer.
- Watch sidecar convenience suggested token/session fixes, but collectively reveals authority-surface drift.

The design target is a middle layer: local -> gatekeeper -> routed queues -> architecture review only when warranted.

## 2. New buffer: `intake_items`

Preferred shape: add a new typed store, not overloaded `observations.open`.

Why not direct observations?

- `observations` should represent accepted substrate facts/frictions with enough evidence to route.
- Raw local filings are lower maturity: they may be duplicates, noise, insufficiently evidenced, or suggested in the wrong architectural direction.
- Mature observations should not be polluted by every local symptom before dedup/risk classification.

Candidate lifecycle:

```text
draft -> triaging -> needs_info -> triaging -> routed
                      \-> dropped
```

Alternative simpler MVP:

```text
draft -> routed | needs_info | dropped
```

Recommended states:

- `draft`: created by local agent or human; cheap, low-authority filing.
- `triaging`: claimed by gatekeeper.
- `needs_info`: gatekeeper found missing evidence; recon agent should gather bounded information.
- `routed`: terminal; resulted in observation/task/arch_review/fast_track/duplicate link.
- `dropped`: terminal; noise, already-resolved, or not actionable.

Candidate fields:

```yaml
summary: text, required
body: text, required
source: enum[task_agent, orchestrator, human, watch_sidecar, recon, other]
source_task: fk tasks, optional
source_observation: fk observations, optional
source_agent_role: text, optional
source_run_ref: text, optional
suggested_fix: text, optional
observed_files: list[text]
commands_run: list[text]
evidence_refs: list[text]
created_context: record # cwd, branch, commit, stores_version if available

routing_decision: record, actor=framework/gatekeeper
risk_flags: list[enum]
cluster_key: text
confidence: enum[low, medium, high]
duplicate_candidates: list_fk observations/intake_items
routed_to_observation: fk observations
routed_to_task: fk tasks
routed_to_arch_review: fk architecture_reviews
fast_track_ref: text
missing_info_question: text
```

Important: local agents may suggest a fix, but the suggested fix is evidence, not a contract. The gatekeeper decides route/risk, not the filing agent.

## 3. Gatekeeper role

The gatekeeper is a cheap, specialized router. It should not run the full global architecture review on every item.

Gatekeeper inputs:

- intake item;
- recent related observations by keyword/cluster/file path;
- recent tasks linked to same area;
- risk taxonomy;
- compact doctrine checklist, not full philosophy every time.

Gatekeeper outputs a structured routing envelope:

```json
{
  "decision": "duplicate | needs_info | fast_track | normal_observation | arch_review_candidate | drop_noise",
  "confidence": "low | medium | high",
  "size_tier": "T0 | T1 | T2 | T3",
  "risk_class": "low | normal | architecture | security | authority",
  "approval_policy": "auto | human | architecture",
  "risk_flags": ["touches_lifecycle"],
  "cluster_key": "t1-null-plan",
  "duplicate_candidates": ["L123"],
  "missing_info_question": null,
  "routed_payload": {},
  "rationale": "..."
}
```

Gatekeeper decisions:

1. `duplicate`: link to existing row/cluster; do not create a new mature observation unless extra evidence is valuable.
2. `needs_info`: move intake to needs_info; trigger recon agent with exactly one focused evidence request.
3. `fast_track`: tiny, low-risk, bounded fix; create fast-track task or direct patch path with audit.
4. `normal_observation`: create mature observation with drafted contract or draft investigation note.
5. `arch_review_candidate`: route to architecture-review queue/cluster; do not approve local fix yet.
6. `drop_noise`: terminal with rationale.

## 4. Recon agent

Recon is not architecture review and not solution design. It answers one missing evidence question.

Examples:

- reproduce this command and capture stderr;
- inspect whether existing observation L### already covers this;
- find the code path that writes this field;
- verify whether the symptom still occurs on current main.

Recon output returns to the gatekeeper. Recon should not create tasks or mature observations directly except through the gatekeeper path.

## 5. Risk taxonomy

Tier is not risk. Size/complexity, architectural risk, and approval policy must be orthogonal.

Suggested dimensions:

```text
size_tier: T0 | T1 | T2 | T3
risk_class: low | normal | architecture | security | authority
approval_policy: auto | human | architecture
```

Risk flags:

### Authority/security

- `touches_actor_authority`
- `touches_approval_token`
- `persists_sensitive_context`
- `cross_project_authority`
- `secrets_or_credentials`

### Lifecycle/schema

- `touches_lifecycle`
- `changes_transition_guards`
- `adds_recovery_or_admin_verb`
- `changes_schema_core`
- `changes_required_when_or_actor_fields`

### Runtime/subscriber

- `touches_subscriber_semantics`
- `touches_dispatch_locks`
- `touches_daemon_startup`
- `touches_runner_boundary`
- `touches_deploy_ceremony`

### Architecture/primitives

- `introduces_new_primitive`
- `symptom_of_missing_primitive`
- `contradicts_philosophy`
- `changes_substrate_boundary`
- `repeated_cluster`

### Low-risk helpers

- `docs_only`
- `test_fixture_only`
- `cli_display_only`
- `template_wording_only`
- `observation_reconciliation_only`

Fast-track is prohibited if any high-risk flag is present, even if the code delta is small.

## 6. Cluster keys

Cluster keys make architecture visible before a human manually reads 100 observations.

Candidate initial clusters from current backlog:

- `t1-execution-shape` — L109/L117/L123/L126/L130/L133.
- `dispatch-attempt-lifecycle` — L039/L087/L107/L116/L122/L134.
- `check-primitive` — L061/L110/L120/L121/L135 plus deploy/schema checks.
- `loop-recovery` — L002/L069/L070/L092/L124/L132.
- `sidecar-authority` — L075/L081/L090/L091.
- `observation-intake-architecture` — L006/L043/L138/T045.
- `auto-resolve-causality` — L049/L137 and stale task->obs pairs.

Cluster keys can start as strings in gatekeeper output; later become typed taxonomy.

## 7. Architecture review triggers

Architecture review should not fire on every intake item. It should fire on selected predicates.

Immediate trigger if risk flags include:

- actor authority / approval token / secrets;
- lifecycle/transition/schema core;
- subscriber semantics / dispatch locks / runner boundary;
- deploy ceremony;
- new primitive or substrate boundary change;
- recovery/admin verb;
- explicit contradiction with philosophy.

Cluster trigger:

```text
same cluster_key >= 3 open/intake items within window
same cluster_key has >= 2 proposed local fixes
same cluster_key reappears after a shipped fix
```

Pre-ratification trigger:

- any ready contract with architecture/security/authority risk requires architecture review result before U1 ratification.

Periodic trigger:

- daily or every N shipped tasks: review recent completed tasks + ready observations + new intake clusters.

Post-accept trigger:

- after a batch of 5-10 shipped tasks, review whether local fixes changed primitives/doctrine or created stale observations.

## 8. Architecture-review output

Architecture reviewer is not a super code reviewer. Its output should be structured coherence findings:

```json
{
  "result": "ok_local_fix | reframe_contract | merge_cluster | create_primitive_task | needs_human_arch_decision | doctrine_update",
  "findings": [
    {
      "type": "contradiction | drift | missing_primitive | duplicate_local_fix | authority_risk | stale_doctrine",
      "severity": "low | medium | high | critical",
      "evidence": ["L123", "T039", "docs/philosophy.md#..."],
      "recommendation": "Normalize T1 execution shape instead of adding more plan=null guards."
    }
  ]
}
```

Architecture review may propose observations/tasks/doc updates, but should not silently mutate contracts. It should block/reframe risky local work through typed transitions.

## 9. Fast-track policy

Fast-track is necessary for throughput but must be audited.

Allowed candidates:

- docs typo or narrow rendered-doc fix;
- snapshot/test fixture update;
- narrow template wording;
- CLI display/readability fix;
- stale observation reconciliation;
- tiny low-risk T1 code fix with no high-risk flags.

Forbidden candidates:

- actor/approval-token/security/secrets;
- lifecycle/schema/transition guards;
- daemon/subscriber/dispatch locks;
- runner/deploy ceremony;
- recovery/admin verbs;
- cross-project authority;
- any architecture-review candidate.

Fast-track should still write a row/transition. It is not an invisible patch path.

## 10. Relation to primitives

This design composes existing and missing primitives:

- **Buffer**: `intake_items`, later `architecture_reviews`.
- **Transition**: intake routing decisions and recon loops.
- **Subscriber**: gatekeeper, recon, architecture reviewer.
- **Actor**: local agent files draft; gatekeeper routes; human/architecture gates remain explicit.
- **Direction**: human pulls curated decisions, not raw local noise.
- **Schema**: risk flags, routing envelope, cluster keys.
- **Aggregation**: duplicate/cluster detection.
- **Causality**: intake -> observation/task/arch_review provenance.
- **Check**: fast-track deterministic checks and architecture-review predicates.
- **Activity**: future weighting of repeated searches/touches by cluster.
- **Loop**: needs_info -> recon -> triage; recovery/admin issues routed as Loop primitive work.

Possible new primitive:

- **Coherence**: semantic review over a window/cluster of substrate activity against doctrine/primitives, producing typed objections or reframing recommendations. Coherence is not just Aggregation; it evaluates whether local fixes preserve global architecture.

## 11. Doctrine vs implementation split

Doctrine updates:

- Add principle: local correctness is not architectural coherence.
- Add rule: local agents file intake drafts; mature observations are gatekeeper-routed facts/frictions.
- Add rule: tier is not risk; small high-risk work cannot fast-track.
- Add rule: architecture review gates risky contracts/clusters, not every item.

Implementation tasks:

1. Add `intake_items` store and lifecycle.
2. Add gatekeeper agent role + JSON schema.
3. Add recon agent role + narrow evidence schema.
4. Add cluster/risk fields to observations or intake routing outputs.
5. Add architecture-review store or initial tagged queue.
6. Add fast-track policy enforcement.

Contract amendments / follow-up observations:

- L138/T045 should likely produce design + doctrine, not full implementation in one task.
- L132/L133/L134/L135 should be referenced as architecture-review clusters, not handled as isolated local fixes.
- L137 remains concrete local engine hygiene and can proceed separately.

## Follow-ups

- Send this note to substrate-agent as canonical T045 design input.
- Consider updating `docs/primitives.md` with candidate **Coherence** primitive after human review.
- Consider updating `docs/philosophy.md` with “local correctness is not architectural coherence.”
- Ensure T045 executor output is judged against this split: design first, implementation later.
