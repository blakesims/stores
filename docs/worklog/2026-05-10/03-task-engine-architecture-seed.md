# Task Engine Architecture Seed — lifecycle, review, integration, throughput

**Date:** 2026-05-10
**Type:** architecture seed / pre-ADR context

## Why this note exists

This captures the high-context design discussion before opening the T3 architecture task. The intent is not to make this note canonical. The first T3 task should harden this into an ADR plus `docs/task-engine-architecture.md`, preserving the useful distinctions and explicitly rejecting the bad ones.

The risk we are avoiding: compressing a rich design conversation into a tiny `intent_contract` and losing the actual architecture logic that made the design click.

## Core problem

The current task/status model mixes too many axes into one lifecycle:

- human-facing lifecycle;
- agent workflow step;
- review phase;
- integration/deploy state;
- blockers/waiting;
- subscriber/daemon mechanics.

That overload produces both operator confusion and engine correctness issues. Example: `code_review` is currently easy to perceive as outside `active`, but semantically a coding review is still active engine-owned candidate production.

The deeper design goal is:

> fully autonomous, highly parallelised task work with clean integration.

## Core doctrine

The substrate should:

> parallelise candidate production and serialize shared truth mutation.

Meaning:

- many tasks can plan/code/review/test in branches/worktrees concurrently;
- only operations that mutate shared truth need exclusive resource locks;
- `main` mutation is serialized, but the whole integration lane should not necessarily be capacity 1;
- slow validation/review/testing should parallelize where safe;
- final merge/deploy/schema mutation should be resource-locked and audited.

## Naming grammar convention

Use grammar to encode semantics mechanically.

| Field type | Form | Examples |
|---|---|---|
| Lifecycle states | adjective/participle | `queued`, `active`, `integration`, `done` |
| Step in progress | gerund | `planning`, `coding`, `wrapping`, `refreshing`, `merging` |
| Reviewing a step | `<gerund>_review` | `planning_review`, `coding_review` |
| Worker / job kind | agent noun | `planner`, `planning_reviewer`, `coder`, `coding_reviewer`, `task_reviewer` |
| Events | past participle | `planned`, `coded`, `coding_reviewed`, `merged` |
| Blockers / gates | noun phrase | `human_acceptance`, `stale_base`, `main_red` |
| Policies | `<noun>_required` / `_policy` | `task_review_required`, `human_acceptance_policy` |

Pairing rule: if a step reviews another step, the names should show adjacency. `coding` pairs with `coding_review`; `planning` pairs with `planning_review`.

Anti-pattern: mixing forms inside one enum, e.g. `plan | plan_review | execute | code_review | wrap`.

## Task lifecycle target

Tasks should only exist after upstream intent/contract has been ratified. Therefore `proposed` does not belong inside `tasks`; it belongs upstream in observation/contract space.

Target lifecycle:

```text
queued | active | integration | done
```

Interpretation:

- `queued`: approved task exists, but engine has not taken active ownership because of priority, capacity, dependencies, or scheduling.
- `active`: engine-owned candidate production is underway.
- `integration`: candidate production is complete; shared-truth/integration pipeline owns the work.
- `done`: shared truth mutation and required verification completed.

Lifecycle states mark control-plane / trust-boundary changes. Sub-steps mark activity within a control plane.

## Active step target

```text
active_step:
  none
  planning
  planning_review
  coding
  coding_review
  wrapping
```

Important semantics:

- `coding_review` is still active work.
- A task leaves `active` only when all phases and engine-internal reviews/wrap are complete.
- T1/T2/T3 shape still matters, but should bend the step sequence rather than create unrelated lifecycle states.

## Integration pipeline target

Integration is not one monolithic capacity-1 lane. It is a pipeline with per-step/per-resource capacity.

Likely integration steps:

```text
integration_step:
  none
  refreshing
  task_review
  testing
  merging
  deploying
  verifying
```

Notes:

- `task_review` is the whole-task / PR-style review. Prefer `task_reviewer` over `external_reviewer` as the worker name.
- `coding_reviewer` reviews one active coding phase.
- `task_reviewer` reviews the whole task/branch before merge.
- `integration_step = none` inside `lifecycle = integration` can mean pending/ready for the next integration job; `integration_ready` should be a derived display predicate, not necessarily a stored lifecycle.

## Review semantics

There are three distinct concepts:

| Concept | Where | Authority |
|---|---|---|
| Phase coding review | `active_step = coding_review` | engine-internal phase quality gate |
| Whole-task review | `integration_step = task_review` by default | PR-style branch/task gate before merge |
| Human acceptance | blocker/gate overlay | policy-driven, not universal lifecycle |

Open policy axis:

- whole-task review may be advisory during `active`, authoritative during `integration`, or both;
- default leaning: authoritative `task_review` belongs in integration after refresh/rebase and before merge, because it avoids stale-base review semantics;
- throughput concern: task review/testing can be slow, so they should not necessarily hold the `main_branch` merge lock while running.

## Resource locks, not global integration capacity

The design should model exclusive resources:

```text
main_branch        capacity 1
production_deploy capacity 1 maybe
schema_migration  capacity 1
cargo_install     capacity 1 for stores self-build
```

Integration step examples:

| Step | Resource implication |
|---|---|
| `refreshing` | branch/worktree, no shared truth mutation |
| `task_review` | reviewer capacity, no shared truth mutation |
| `testing` | test runner capacity, no shared truth mutation |
| `merging` | `main_branch` lock, capacity 1 |
| `deploying` | project-specific deploy resource |
| `verifying` | project-specific; may update `last_green_main` |

Better slogan:

> Candidate production is parallel. Validation is parallel when it does not depend on exclusive truth. Truth mutation is serialized by resource.

## Example flow after final coding_review PASS

1. Task finishes final active phase:

```text
lifecycle = active
active_step = coding_review
```

2. Coding reviewer passes. Record actual branch head/diff/affects. Transition:

```text
lifecycle = integration
integration_step = refreshing
```

3. Refresh/rebase onto current green main.

- success -> `task_review`
- conflict -> blocked with `blocker_kind = stale_base` or integration repair/bounce to active
- red main -> blocked/wait with `blocker_kind = main_red`

4. Whole-task review:

```text
integration_step = task_review
job.kind = task_reviewer
```

Records base/head/verdict/artifacts/affected scope.

5. Testing:

```text
integration_step = testing
```

Runs branch-level tests according to policy.

6. Merge candidate/pending:

```text
lifecycle = integration
integration_step = none
```

Derived display: `integration_pending`.

7. Acquire `main_branch` lock and merge:

```text
integration_step = merging
resource_lock = main_branch
```

Before merge, cheaply revalidate freshness. If main changed since review/test base:

- no relevant overlap -> refresh/cheap test/merge;
- overlap -> release merge lock and rerun `task_review`/`testing` as needed.

8. Deploy/verify:

```text
integration_step = deploying
integration_step = verifying
```

9. Done:

```text
lifecycle = done
integration_step = none
```

## Multiple-task example

T201, T202, T203 all finish active around the same time.

They can all enter integration and run slow work concurrently:

```text
T201 integration/task_review
T202 integration/testing
T203 integration/task_review
```

T201 merges first under `main_branch` lock. Main advances A -> B.

T202 reaches merge. Its review was against A. The merge gate checks whether T201 touched dimensions T202 cares about:

- no overlap: refresh cheaply onto B and merge;
- overlap: rerun task_review/testing after refresh.

T203 does the same after T202. This preserves throughput without pretending stale reviews do not exist.

## Blockers are overlays

Blocked/waiting should not be lifecycle states.

Example:

```text
lifecycle = active
active_step = coding
blocked = true
blocker_kind = rate_limit
```

or:

```text
lifecycle = integration
integration_step = merging
blocked = true
blocker_kind = main_red
```

Suggested blocker kinds include:

```text
capacity
dependency
runner
rate_limit
human_acceptance
task_review
stale_base
config
test_failure
main_red
deploy
migration
```

## Human acceptance

Human acceptance is a policy/gate overlay, not a universal lifecycle state.

Potential policies:

```text
human_acceptance_policy: never | optional | required
task_review_policy: none | advisory | authoritative | both
```

Suggested leaning:

- T1: no default human acceptance, no default task review.
- T2: policy/risk driven.
- T3: task review required; human acceptance off by default unless high-risk/architecture/security.
- high-risk/security/architecture: task review + human acceptance may both be required.

## Relationship to current docs

This note should feed:

- an ADR under `docs/adr/`;
- a living canonical doc, likely `docs/task-engine-architecture.md`;
- simplification of `docs/flow-diagrams.md` so diagrams reflect lifecycle/step/resource-lock separation;
- later implementation tasks.

`docs/flow-diagrams.md` should be visual/supporting, not the canonical decision record.

## First T3 task should do

The first T3 should not merely “write docs.” It should harden this seed into durable architecture and prepare the implementation chain.

Meaningful output:

1. ADR with context, decision, alternatives, consequences.
2. Canonical task-engine architecture doc.
3. Updated/simplified flow diagrams.
4. Explicit target enums and naming doctrine.
5. Implementation roadmap split into 2–4 downstream T3 tasks.
6. Concrete downstream intent contracts drafted in the doc, ready to promote.
7. Identification of safe parallelization opportunities between downstream tasks.

Out of scope for first T3:

- schema migration;
- resource-lock implementation;
- automation_jobs table;
- task_reviewer behavior change;
- watch projection code.

## Proposed downstream chain

### T3-A — Architecture hardening and ADR

Harden this note into canonical docs and task contracts.

### T3-B — Task lifecycle read model / watch projection

Create deterministic projection over current task statuses:

```text
raw status -> lifecycle / active_step / integration_step / blocker overlay
```

Use it in watch so `coding_review`/current `code_review` remains active.

### T3-C — Integration pipeline/resource locks

Introduce resource-lock mechanics for truth mutation, especially `main_branch` merging capacity 1, without globally serializing slow validation.

### T3-D — Task reviewer + review freshness policy

Rename/reframe whole-task external review as `task_reviewer`; implement first freshness/staleness policy, likely file-overlap L1.

Potential later tasks:

- automation_jobs first-class queue;
- `main_health` / `last_green_main`;
- typed test failures / flake registry;
- affect declarations and scheduler collision avoidance.
