# Architecture Oversight Findings

**Date:** 2026-05-06
**Type:** note

## Summary

Reviewed `docs/philosophy.md`, `docs/primitives.md`, `docs/engine-health.md`, recent completed/in-flight tasks, and the non-terminal observation backlog. The core philosophy remains coherent, but the live substrate is drifting in three places: stale truth surfaces, unclosed causal links between shipped tasks and observations, and recovery/refinement gaps that keep pushing work back onto the orchestrator.

## Details

### 1. DB truth vs. queue truth has diverged

Many tasks are `schema_migrated` while their linked observations remain `ready`, including T021→L050, T022→L048, T024→L045, T025→L063, T026→L055, T027→L066, T029→L071, T030→L062, T031→L060, T033→L038, T036→L020, T037→L049, T039→L093, T040→L107, and T041→L039.

This directly contradicts the philosophy's DB-as-truth model: the backlog is no longer a trustworthy pull queue if shipped work still appears ratified/awaiting completion. T037 claims to ship auto-resolve-on-`schema_migrated`, but either it only handles future transitions, failed to fire, or cannot reconcile historical rows.

### 2. `engine-health.md` is stale relative to the live DB

The health doc still presents L109 as the top blocker and T029/T033 as blocked behind it, while the live DB shows T029 and T033 as `schema_migrated`, along with later fixes T039/T040/T041. The document is useful as architectural commentary, but it currently reads like an operational queue snapshot while lagging behind the source of truth.

### 3. Backlog entropy has exceeded the substrate's refinement capacity

There are 105 non-terminal observations, including 61 `open` rows without contracts. A large share are duplicate/symptom rows, especially repeated `deploy_blocked` observations for the same tasks/branches. This conflicts with the philosophy's claim that filing is cheap because refinement is the substrate's burden. Today the burden still lands on the human/orchestrator.

The observed pain maps directly to missing primitives already named in `docs/primitives.md`: Aggregation for duplicate symptom clusters, Causality for task→observation provenance, Check for deterministic deploy/preflight gates, Loop for recovery transitions, and Activity/Decay for queue aging and attention signals.

### 4. T1's contract-is-plan design leaked through null-plan assumptions

The T027 philosophy is sound: T1 skips planner and plan_reviewer because the contract is the plan. But the implementation appears to encode T1 as `plan = null`, and downstream code still assumes phases exist. Recent/open observations show this repeatedly: L109 (`next-action` null), L117 (auto-promote does not fire on-entry skip-plan), L123 (`submit-review` guard references `plan.phases.length`), L126 (briefs render `Current Phase: 1 of 0`), and L130 (resume on plan-null non-T1 cascades into broken execution).

The architectural issue is not one missing conditional; T1 needs a normalized execution shape, e.g. `execution_shape = contract_only | one_phase_plan | multi_phase_plan`, or a synthetic contract-derived phase so all consumers see a consistent structure.

### 5. Recovery pressure is exposing the missing Loop primitive

Observations L002, L069, L070, L092, and L124 all point at the same class of gap: once a row is stale, duplicate, already shipped, deploy-blocked, or needs re-entry, the substrate lacks legitimate recovery paths. This creates pressure toward manual DB edits or out-of-band orchestration, exactly what the philosophy forbids.

A coherent Loop/recovery primitive would absorb abandon/drop, already-shipped close-out, deploy-blocked retry, merge-conflict recovery, and code-review REPLAN rather than adding one-off escape hatches.

### 6. Watch sidecars conflict with approval-token doctrine

T028's sidecar model immediately produced L081, L090, and L091. Persisted sidecar JSONL with permissive modes and eager `STORES_APPROVE_TOKEN` loading contradict the philosophy's lazy, U-moment-scoped approval-token doctrine. This is a security/authority-surface problem, not merely UX.

## Follow-ups

- Reconcile completed tasks to linked observations before relying on the backlog for prioritization.
- Update or regenerate `docs/engine-health.md` after reconciliation; make clear which parts are hand-curated commentary vs. DB-derived operational state.
- Promote Loop/recovery from missing primitive to near-term design target.
- Normalize T1 execution shape so downstream lifecycle consumers do not special-case `plan = null`.
- Treat observation refinement as a first-class workflow, not just individual auto-investigator work.
- Harden watch sidecar token handling before expanding watch-driven workflows.
