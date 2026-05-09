# Architecture Coherence

**Path:** `docs/architecture-coherence.md`
**Status:** doctrine doc (T045 phase 1).
**Companion:** `docs/worklog/2026-05-06/06-gatekeeper-architecture-observability.md` (the brainstorm this doctrine was promoted from).

## Doctrine

**Local correctness is not architectural coherence.**

A fix can be locally correct — closes the bug it names, passes its tests, satisfies the observation it was filed against — and still nudge the architecture in the wrong direction. Local agents see local pain; they file what they see. Repeated local fixes, each individually approved, can splinter the global shape of the substrate without any single decision ever being wrong on its own terms.

The substrate already has *local observability*: agents file observations when friction surfaces; the orchestrator routes; tasks promote and ship. What is missing is *architectural observability*: a layer that asks, across a cluster of locally-correct fixes, whether the underlying shape is still coherent — or whether the local fixes are accreting into drift.

The corollary: an observation's `intent_contract` can be ratifiable on local merits and still be wrong as a unit of architectural change. Approving a contract is not the same as endorsing its architectural framing. Coherence review is a separate gate from contract ratification.

## Three drift examples (worklog 06)

These are the concrete cases that motivated the doctrine. Each is a sequence of locally-correct fixes whose aggregate shape was wrong.

1. **T1 plan = null.** T1 failures (contract-is-plan tasks failing in drive) led to local fixes around representing plan absence as `plan = null` in various spots — null-checks in the renderer, null-tolerant submit handlers, defensive guards in the brief composer. Each fix was locally correct. The deeper issue is that contract-is-plan should be represented as a *normalized execution shape* (the contract IS the plan, projected into the plan slot), not as the absence of a plan. The local fixes ratified the wrong primitive: "plan-or-null" instead of "plan-derived-from-contract."

2. **Dispatch lifecycle.** Dispatch failures led to local fixes around stale PIDs, retry backoff, skip-historical locks, zombie detection. Each was locally correct: stale PIDs do need cleanup; zombies do need detection. The deeper issue is that dispatch attempts are an *unmodeled lifecycle buffer* — there is no first-class row representing a dispatch attempt with its own state machine, so every concern about dispatch state lands as an ad-hoc field or sentinel on `dispatch_locks` or `tasks`. The local fixes accreted into a parallel, unschematized lifecycle that the substrate's own lifecycle machinery cannot inspect.

3. **Sidecar token propagation.** Sidecar convenience led toward eager propagation of approval tokens — pre-fetching, caching, forwarding through subagent briefs. Each step was locally convenient. The deeper issue is *authority-surface drift*: the approval-token mechanism is designed to be cryptographically gated at the moment of write, with the AI possessing the token only inside the session window where the human pasted it. Eager propagation widens that window across subagents and persists tokens in places the schema does not see. The locally-correct convenience erodes the threat model the token was built to defend.

In each case, an architecture-review layer would have asked: *what abstraction are these fixes implicitly defining, and is it the abstraction we want?* Without that layer, the implicit answer is whatever the local fixes happened to converge on.

## What this doctrine grounds

Future work in T045 (the gatekeeper / intake / architecture-review layer) is grounded by this doctrine. Specifically:

- **Why an intake/gatekeeper layer is needed at all.** If local correctness implied architectural coherence, raw observations could flow straight to mature observations and contracts. The doctrine says they cannot — coherence is a separate concern, requiring a separate routing layer.
- **Why risk class is orthogonal to size tier.** A 30-line fix can be high-risk (authority surface) and a 500-line snapshot regen can be low-risk (cosmetic). Coherence concerns track risk, not size; the gatekeeper must classify on both axes.
- **When architecture review fires.** Risk-flag triggers, cluster thresholds, pre-ratification gates, periodic sweeps, post-accept batches — all stand on the claim that local correctness checks (tests, code review, contract ratification) do not catch architectural drift, so a separate trigger surface is required.
- **What fast-track is allowed for.** Fast-track is safe only when the change provably cannot affect coherence: docs typos, snapshot regens, narrow display tweaks. Anything touching authority, lifecycle, schema core, subscriber semantics, or runner boundary is forbidden from fast-track because those are the surfaces where local correctness most reliably hides architectural drift.

## Client adapter boundary: substrate primitives vs repo-specific wiring

When stores is dogfooded inside a real client repo, separate **substrate primitives** from **client adapters** before writing doctrine, scripts, or worklogs.

Rule of thumb:

> If another repo using stores would need the mechanism, it belongs in the substrate. If it encodes one repo's commands, gates, baselines, branch conventions, deployment policy, or cleanup ritual, it belongs in that repo's adapter.

This is an architecture-coherence rule, not only a code-location rule. A client-side workaround can be locally correct and still create global drift if it implements a substrate primitive in shell because the substrate is temporarily incomplete.

Current example: 10.06 may own `accept-merge-real.sh`, `.gate-baseline.yaml`, `./dev test gate`, worktree naming, and known-flake policy. The stores substrate owns the integration lane: accepted-candidate queueing, capacity-1 mutation of `main`, freshness checks, typed `integration_blocked` routing, external-review freshness, and durable base/head/main provenance. The 10.06 script should be treated as a repo-specific adapter until the substrate integration-lane adapter contract is explicit; do not build a competing client-side queue, scheduler, retry loop, task DAG, or file-overlap dispatcher.

**Integration lane (T138) — substrate-vs-adapter, verbatim.** Substrate owns the integration lane (queueing, capacity-1, refresh, freshness, typed integration_blocked, provenance). Client repos must not implement competing queues, schedulers, retry loops, task DAGs, or file-overlap dispatchers; they wire post-integrated subscribers (cargo install, deploy, cleanup, observation resolution) only. See `docs/integration-lane.md` for the lifecycle, configuration shape, freshness contract, stale_base vs stale_external_review distinction, and the JSON-column `integration_attempts` provenance schema.

When documenting cross-repo dogfood work, name ledgers explicitly to avoid ID collisions: e.g. `substrate:T123` / `substrate:L528-integration-lane` versus `10.06:L528-auth.setup`.

## Pointers

- Brainstorm and proposed mechanism: `docs/worklog/2026-05-06/06-gatekeeper-architecture-observability.md`.
- Substrate boundary doctrine: `docs/philosophy.md` § *What's outside the substrate*.
- Tier vs. risk decoupling: see worklog 06 § *Tier is not risk*.
- Client adapter boundary: this doc § *Client adapter boundary: substrate primitives vs repo-specific wiring*.
