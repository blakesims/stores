# ADR 0001: Task engine lifecycle and integration architecture

**Date:** 2026-05-10
**Status:** accepted
**Deciders:** Blake + pi-architect

## Context

The current task store uses `status` to carry several different meanings at once:

- human-facing lifecycle;
- agent workflow step;
- phase review state;
- integration/deploy state;
- blocked/waiting state;
- subscriber/daemon mechanics.

This overload creates both operator confusion and engine correctness risk. In particular, `code_review` currently looks like a task has left active work, but semantically a phase-level code review is still engine-owned candidate production. Similarly, post-accept integration states mix shared-truth mutation with slow validation/review work, which encourages either a low-throughput global integration lane or unsafe parallel mutation of `main`.

The design goal is fully autonomous, highly parallelised task work with clean integration.

## Decision

### 1. Split lifecycle from step and blocker state

Task lifecycle will be modeled as control-plane / trust-boundary states:

```text
queued | active | integration | done
```

- `queued`: a ratified task exists, but engine ownership is delayed by capacity, dependency, priority, scheduling, or activation policy.
- `active`: engine-owned candidate production is underway.
- `integration`: candidate production is complete; the integration pipeline owns refresh/review/test/merge/deploy/verify work.
- `done`: shared truth mutation and required verification completed.

Tasks do not have a `proposed` lifecycle state. Proposal/ratification belongs upstream in observations/intent contracts; a task row means approved work exists.

Blocked/waiting is an overlay, not a lifecycle state:

```text
blocked: bool
blocker_kind: capacity | dependency | runner | rate_limit | human_acceptance |
              task_review | stale_base | config | test_failure | main_red |
              deploy | migration | ...
```

### 2. Use grammatical naming conventions

Project-wide naming should encode semantic role mechanically:

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

### 3. Active work remains active through phase review

Active work uses an active-step axis:

```text
active_step:
  none
  planning
  planning_review
  coding
  coding_review
  wrapping
```

`coding_review` is engine-internal active work. A task leaves `active` only when all phases, engine-internal reviews, and wrap/finalization are complete.

### 4. Integration is a pipeline, not a single global capacity-1 lane

Integration uses an integration-step axis:

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

`task_review` is the whole-task / PR-style branch review. The worker name is `task_reviewer`, not `external_reviewer`, to distinguish it from a generic external service and from phase-local `coding_reviewer`.

`integration_step = none` while `lifecycle = integration` can represent pending/ready-for-next-integration-work only when derived predicates say so. `integration_step` must be `none` whenever `lifecycle != integration` unless a compatibility projection explicitly maps legacy state. A display bucket such as `integration_pending` is derived as:

```text
lifecycle == integration
&& integration_step == none
&& blocked == false
&& required predecessor artifacts exist
```

`integration_ready` should not be a separate lifecycle state by default.

### 5. Serialize shared truth mutation by resource lock

The substrate should parallelise candidate production and serialize shared truth mutation.

This ADR promotes capacity/resource locking from missing primitive pressure into an explicit target primitive. The primitive inventory in `docs/primitives.md` must name this before implementation.

Integration is not globally capacity 1. Instead, exclusive resources are capacity-constrained:

```text
main_branch        capacity 1
production_deploy capacity 1 maybe, project-specific
schema_migration  capacity 1
cargo_install     capacity 1 for stores self-build
```

Examples:

| Step | Resource implication |
|---|---|
| `refreshing` | branch/worktree; no shared truth mutation |
| `task_review` | reviewer capacity; no shared truth mutation |
| `testing` | test runner capacity; no shared truth mutation only for hermetic tests; shared test databases, staging environments, external API quota, and similar resources require explicit locks |
| `merging` | `main_branch` lock, capacity 1 |
| `deploying` | project-specific deploy resource |
| `verifying` | project-specific; may update `last_green_main` |

Slogan:

> Candidate production is parallel. Validation is parallel when it does not depend on exclusive truth. Truth mutation is serialized by resource.

Lock invariants:

- resource locks are DB-backed rows or guarded fields, not process conventions;
- lock acquisition, renewal, release, expiry, and recovery happen through CLI/framework transitions;
- every lock records resource id, owner task/job, fencing token or attempt id, acquired_at, and enough expiry/heartbeat metadata to recover stale ownership;
- merge/deploy/schema-mutation transitions must Check that the caller owns the required resource lock;
- lock release and stale-lock recovery are audited in transition history or an equivalent typed audit surface;
- wrappers and external agents cannot mutate locks except through the normal CLI authority surface.

### 6. Review semantics are distinct

There are three distinct review/acceptance concepts:

| Concept | Where | Authority |
|---|---|---|
| Phase coding review | `active_step = coding_review` | engine-internal phase quality gate |
| Whole-task review | `integration_step = task_review` by default | PR-style branch/task gate before merge |
| Human acceptance | blocker/gate overlay | policy-driven, not universal lifecycle |

Whole-task review may later support policy variants (`none`, `advisory`, `authoritative`, `both`). The default architectural leaning is that authoritative `task_review` belongs in integration after refresh/rebase and before merge, because that avoids stale-base review semantics. Slow review/testing must not necessarily hold the `main_branch` lock while running; freshness is revalidated before merge.

Human acceptance is not a universal lifecycle state. It is a policy/gate overlay, e.g.:

```text
human_acceptance_policy: required | optional | delegated_by_policy
task_review_policy: none | advisory | authoritative | both
```

This ADR amends the older two-gate wording in `docs/philosophy.md`: the back gate remains a grounded acceptance decision, but the accepting authority can be a human signature or a human-ratified project policy that delegates acceptance to required checks such as authoritative `task_review`. The policy itself is the human-grounded decision; automation cannot silently remove a required human gate.

Suggested defaults:

- T1: human acceptance may be delegated by policy; no default task review.
- T2: policy/risk driven.
- T3: task review required; human acceptance delegated by policy unless high-risk/architecture/security.
- high-risk/security/architecture: task review and explicit human acceptance may both be required.

## Example flow

After the final `coding_review` passes:

1. Record actual branch head/diff/affects.
2. Transition to:

```text
lifecycle = integration
integration_step = refreshing
```

3. Refresh/rebase onto the current green main.
4. Run whole-task review:

```text
integration_step = task_review
job.kind = task_reviewer
```

5. Run branch-level tests:

```text
integration_step = testing
```

6. Become a merge candidate:

```text
lifecycle = integration
integration_step = none
```

7. Acquire `main_branch` lock:

```text
integration_step = merging
resource_lock = main_branch
```

8. Before merge, revalidate freshness. Reuse of prior review/test results is forbidden unless durable inputs exist: review base, test base, branch head, and machine-checkable affected scope. If those inputs are missing, any main change forces refresh plus required review/testing rerun. If inputs exist and main changed since review/test base:
   - no relevant overlap: refresh/cheap-test/merge;
   - relevant overlap: release merge lock and rerun `task_review`/`testing` as needed.
9. Deploy/verify as project policy requires.
10. Transition to:

```text
lifecycle = done
integration_step = none
```

## Alternatives considered

### Keep current task statuses

Rejected. The current model works mechanically but mixes lifecycle, step, blockers, and integration mechanics in one enum. This is the ambiguity this ADR is meant to remove.

### Use `proposed | active | review | accepted | done`

Rejected. `proposed` belongs upstream of tasks; `review` is ambiguous because phase review, whole-task review, and human acceptance have different meanings; `accepted` overstates human acceptance as universal when acceptance should be policy-driven.

### Make the entire integration lane capacity 1

Rejected as the target architecture. It is simple and safe, but slow task review/testing would serialize all ready tasks. Only shared truth mutation must be serialized; slow validation should parallelize when freshness can be revalidated.

### Put authoritative task review at the end of active

Partially accepted as a possible advisory/policy variant, but not the default authoritative gate. Review at the end of active can go stale before merge as `main` changes. The default authoritative review belongs in integration after refresh/rebase, with freshness checks before merge.

## Consequences

### Positive

- Operator-facing task state becomes simpler and less ambiguous.
- `coding_review` remains correctly visible as active work.
- Integration throughput can scale because slow validation does not globally serialize.
- Merge/main mutation safety is represented by explicit resource locks.
- Human acceptance can be required for high-risk work without blocking every task.
- The model maps cleanly to first-class automation jobs later.

### Negative / costs

- Requires migration from existing status semantics or a compatibility projection.
- Requires watch/read-model work before schema migration to avoid operator confusion.
- Requires explicit freshness/staleness checks when task review/testing occur before merge lock acquisition.
- Requires resource-lock mechanics for `main_branch` and possibly deploy/schema resources.
- Existing task lifecycle tests, briefs, and subscribers will need careful adaptation.

## Implementation direction

This ADR does not itself mandate an immediate schema rewrite. Preferred sequence:

1. Build a deterministic read/projection model over current task states:

```text
current status -> lifecycle / active_step / integration_step / blocker overlay
```

2. Update `stores watch` and diagnostics to use the projection.
3. Introduce resource-lock mechanics for truth-mutating integration steps, starting with `main_branch` for merging.
4. Reframe whole-task review as `task_reviewer` and implement first freshness/staleness policy.
5. Only then migrate underlying schema fields if the projection proves stable.

## Links

- Seed note: `docs/worklog/2026-05-10/03-task-engine-architecture-seed.md`
- Related doctrine: `docs/philosophy.md`
- Related primitives: `docs/primitives.md`
- Current flow map: `docs/flow-diagrams.md`
- Existing integration lane doc: `docs/integration-lane.md`
