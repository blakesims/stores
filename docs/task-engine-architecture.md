# Task Engine Architecture

This document is the living architecture reference for the stores task engine target model. ADR 0001 records the decision; this document explains the model operators and implementation agents should use.

## Core doctrine

Stores should support:

> fully autonomous, highly parallelised task work with clean integration.

The task engine does this by separating candidate production from shared truth mutation:

> Candidate production is parallel. Validation is parallel when it does not depend on exclusive truth. Truth mutation is serialized by resource.

Branches/worktrees can plan, code, review, and test concurrently. Operations that mutate shared truth — especially advancing `main` — require explicit resource locks.

## Naming grammar

Use grammar to encode semantic role.

| Field type | Form | Examples |
|---|---|---|
| Lifecycle states | adjective/participle | `queued`, `active`, `done` |
| Step in progress | gerund | `planning`, `coding`, `wrapping`, `refreshing`, `merging` |
| Reviewing a step | `<gerund>_review` | `planning_review`, `coding_review` |
| Worker / job kind | agent noun | `planner`, `planning_reviewer`, `coder`, `coding_reviewer`, `task_reviewer` |
| Events | past participle | `planned`, `coded`, `coding_reviewed`, `merged` |
| Blockers / gates | noun phrase | `human_acceptance`, `stale_base`, `main_red` |
| Policies | `<noun>_required` / `_policy` | `task_review_required`, `human_acceptance_policy` |

Pairing rule: if a step reviews another step, the names should show adjacency. `coding` pairs with `coding_review`; `planning` pairs with `planning_review`.

Avoid mixing grammatical forms inside one enum, e.g. `plan | plan_review | execute | code_review | wrap`.

## Task lifecycle

Tasks only exist after upstream intent/contract has been ratified. `proposed` belongs upstream in observation/contract space, not in the task lifecycle.

Target lifecycle:

```text
queued | active | integration | done
```

| Lifecycle | Meaning |
|---|---|
| `queued` | Approved task exists, but engine ownership is delayed by capacity, dependency, priority, scheduling, or activation policy. |
| `active` | Engine-owned candidate production is underway. |
| `integration` | Candidate production is complete; the integration pipeline owns refresh/review/test/merge/deploy/verify work. |
| `done` | Shared truth mutation and required verification completed. |

Lifecycle states mark control-plane / trust-boundary changes. Steps mark activity inside one control plane.

## Active step

```text
active_step:
  none
  planning
  planning_review
  coding
  coding_review
  wrapping
```

`coding_review` is still active work. A task leaves `active` only when all phases, engine-internal reviews, and wrap/finalization are complete.

Tier shape still matters:

- T1 can skip planning/planning_review when contract-is-plan.
- T2 should have a constrained planning shape.
- T3 can use full multi-phase planning/coding/review/wrap.

Those tier differences bend the step sequence; they should not create unrelated lifecycle states.

## Integration step

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

| Step | Meaning |
|---|---|
| `none` | No integration job currently executing; often a pending/ready-for-next-work state. |
| `refreshing` | Refresh/rebase the candidate branch/worktree onto current green main. |
| `task_review` | Whole-task / PR-style review by `task_reviewer`. |
| `testing` | Branch-level or integration-policy test execution. |
| `merging` | Main-branch mutation under `main_branch` resource lock. |
| `deploying` | Project-specific post-merge deploy/install/migrate ceremony. |
| `verifying` | Post-merge/project verification; may update `last_green_main` later. |

`integration_pending` can be a derived display bucket for `lifecycle = integration`, `integration_step = none`, and not blocked. It should not be a separate lifecycle state by default.

## Review semantics

There are three different review/acceptance concepts.

| Concept | Where | Worker/gate | Authority |
|---|---|---|---|
| Phase coding review | `active_step = coding_review` | `coding_reviewer` | Engine-internal phase quality gate. |
| Whole-task review | `integration_step = task_review` by default | `task_reviewer` | PR-style branch/task gate before merge. |
| Human acceptance | blocker/gate overlay | human gate | Policy-driven; not a universal lifecycle state. |

`task_reviewer` is preferred over `external_reviewer` because the important semantic is scope: it reviews the whole task/branch. The implementation may use Codex, CodeRabbit, Pi, Claude, or another reviewer.

Whole-task review policy may later support:

```text
task_review_policy: none | advisory | authoritative | both
human_acceptance_policy: required | optional | delegated_by_policy
```

The back gate remains a grounded acceptance decision. The accepting authority can be an explicit human signature or a human-ratified policy that delegates acceptance to required checks such as authoritative `task_review`. Automation cannot silently remove a required human gate.

Default leaning:

- T1: human acceptance may be delegated by policy; no default task review.
- T2: policy/risk driven.
- T3: task review required; human acceptance delegated by policy unless high-risk/architecture/security.
- high-risk/security/architecture: task review and explicit human acceptance may both be required.

Authoritative `task_review` belongs in integration after refresh/rebase and before merge by default. Slow task review/testing should not necessarily hold the `main_branch` lock while running; freshness is revalidated before merge.

## Blockers as overlays

Blocked/waiting is orthogonal to lifecycle.

Example:

```text
lifecycle = active
active_step = coding
blocked = true
blocker_kind = rate_limit
```

Example:

```text
lifecycle = integration
integration_step = merging
blocked = true
blocker_kind = main_red
```

Suggested blocker kinds:

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

## Resource locks

Integration is a pipeline with per-step/per-resource capacity, not a single global capacity-1 lane.

Capacity/resource locking is an explicit target primitive introduced by ADR 0001; `docs/primitives.md` names the primitive and its relation to existing Capacity pressure.

Exclusive resources should be explicit:

```text
main_branch        capacity 1
production_deploy capacity 1 maybe, project-specific
schema_migration  capacity 1
cargo_install     capacity 1 for stores self-build
```

| Step | Resource implication |
|---|---|
| `refreshing` | branch/worktree; no shared truth mutation |
| `task_review` | reviewer capacity; no shared truth mutation |
| `testing` | test runner capacity; no shared truth mutation only for hermetic tests; shared test databases, staging environments, external API quota, and similar resources require explicit locks |
| `merging` | `main_branch` lock, capacity 1 |
| `deploying` | project-specific deploy resource |
| `verifying` | project-specific; may update health pointers |

## Resource lock invariants

Resource locks are substrate-owned, not process conventions.

- Locks are DB-backed rows or guarded fields.
- Lock acquisition, renewal, release, expiry, and recovery happen through CLI/framework transitions.
- Every lock records resource id, owner task/job, fencing token or attempt id, acquired_at, and enough expiry/heartbeat metadata to recover stale ownership.
- Merge/deploy/schema-mutation transitions must Check that the caller owns the required resource lock.
- Lock release and stale-lock recovery are audited in transition history or an equivalent typed audit surface.
- Wrappers and external agents cannot mutate locks except through the normal CLI authority surface.

## Flow after final coding review

1. Final phase coding review passes:

```text
lifecycle = active
active_step = coding_review
```

2. Record branch head/diff/affected scope.
3. Enter integration:

```text
lifecycle = integration
integration_step = refreshing
```

4. Refresh/rebase onto current green main.
   - success -> `task_review`
   - conflict -> blocked with `blocker_kind = stale_base` or bounce/repair
   - red main -> blocked/wait with `blocker_kind = main_red`
5. Run whole-task review:

```text
integration_step = task_review
job.kind = task_reviewer
```

6. Run branch-level tests:

```text
integration_step = testing
```

7. Become pending for merge:

```text
lifecycle = integration
integration_step = none
```

Derived predicate:

```text
integration_pending := lifecycle == integration
  && integration_step == none
  && blocked == false
  && required predecessor artifacts exist
```

`integration_step` must be `none` whenever `lifecycle != integration`, unless a compatibility projection explicitly maps legacy state.

8. Acquire `main_branch` lock:

```text
integration_step = merging
resource_lock = main_branch
```

9. Before merge, revalidate freshness. Reuse of prior review/test results is forbidden unless durable inputs exist: review base, test base, branch head, and machine-checkable affected scope. If those inputs are missing, any main change forces refresh plus required review/testing rerun. If inputs exist and main changed since review/test base:
   - no relevant overlap -> refresh/cheap-test/merge;
   - relevant overlap -> release merge lock and rerun `task_review`/`testing`.
10. Deploy and verify as project policy requires.
11. Transition to:

```text
lifecycle = done
integration_step = none
```

## Multiple-task integration example

T201, T202, and T203 all finish active work around the same time. They can all enter integration and run slow work concurrently:

```text
T201 integration/task_review
T202 integration/testing
T203 integration/task_review
```

T201 merges first under the `main_branch` lock, advancing main from A to B.

T202 reaches merge. Its review was against A. The merge gate checks whether T201 touched dimensions T202 cares about:

- no overlap: refresh cheaply onto B and merge;
- overlap: rerun task_review/testing after refresh.

T203 repeats the same check after T202. This preserves throughput without pretending stale reviews do not exist.

## Implementation sequence

Do not rewrite the task schema first. Prove the model through a deterministic read model, then migrate underlying schema if the projection holds.

Suggested implementation chain:

1. **Task lifecycle read model / watch projection — shipped in T144**
   - `tasks.lifecycle` / `active_step` / `integration_step` / blocker overlay are maintained from ADR 0001's model; `stores watch` reads the projection.

2. **Integration resource locks — shipped in T144**
   - `main_branch` capacity-1 mutation is protected by the ResourceLock primitive in `src/handlers/resource_locks.rs`; see ADR 0001 and `docs/primitives.md`.

3. **Task reviewer + freshness policy — shipped in T144**
   - External-review base/head freshness is persisted and enforced by `builtin:integrate`; ADR 0001 remains the target model for broader `task_reviewer` naming.

4. **Later slices**
   - First-class automation jobs.
   - `main_health` / `last_green_main`.
   - Typed test failures / flake registry.
   - Affect declarations and scheduler collision avoidance.

## Related documents

- ADR: `docs/adr/0001-task-engine-lifecycle-and-integration.md`
- Seed note: `docs/worklog/2026-05-10/03-task-engine-architecture-seed.md`
- Current flow diagrams: `docs/flow-diagrams.md`
- Philosophy: `docs/philosophy.md`
- Primitives: `docs/primitives.md`
- Current integration lane doctrine: `docs/integration-lane.md`
