# Subscriber-class taxonomy (T140 P2)

Authoritative classification of every module in `src/flow/builtins/` into one of
five subscriber classes. The taxonomy decides which subscribers gate on the
per-row `activation` field (the "ignition primitive" introduced in T140 P1) and
which remain ungated.

**Owner:** Pi (architecture) + engine-controller (operations).
**Doc-only update path:** edits here are governed by the doc-only exception in
`CLAUDE.md` § *Dogfooding* — focused doc commits do not need to round-trip
through the full task workflow as long as they do not change schema, runner
contract, or substrate API.

A completeness test (`tests/subscriber_classes.rs`) parses
`src/flow/builtins/mod.rs` and fails loud if any `pub mod X;` declaration is
missing from this file or carries more than one class label. Adding a new
builtin module therefore requires adding a row here.

## Classes

- **work_starting** — Subscribers that combust work: drive a task forward into
  its planner/executor cycle, or drive an accepted row into the integration
  lane. Gated on `activation == 'active'` (T140 P2).
- **safety_reconcile** — Subscribers that reconcile state, escalate failures,
  or scaffold workspace prerequisites. Always allowed to run; never gated on
  activation. Gating these would break the engine's recovery path.
- **ceremony_post_accept** — Stores-repo-specific post-`integrated` ceremony
  (binary install, schema migration). Subscribed past the activation gate (the
  integration lane already filtered for activation), so they do not need their
  own activation predicate. Other repos opt out by not wiring these.
- **observation_lifecycle** — Subscribers that move observations / intake /
  external_reviews through their lifecycles, or that mint tasks from
  ratified observations. Not gated on task activation — the rows they handle
  are not tasks, or the task they mint lands inactive by design.
- **deprecated_internal** — Module that is no longer registered in
  `dispatch_builtin` but whose helpers are still imported by other builtins.
  Not subscribed; not gated.

## Module classification

| Module                      | Class                 | Rationale                                                                                                                                                                                                                                                              |
| --------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `accept_merge`              | deprecated_internal   | Not registered in `dispatch_builtin` (T138 P3 retired the `accept-merge` keyword); helpers live on as `pub(crate)` callees of `integrate`. No subscriber edge today.                                                                                                  |
| `activate_queued`           | safety_reconcile      | Registered in `dispatch_builtin`; scans tasks with `activation='active'` and lifecycle `queued`, releasing them to `active` once dependency/capacity blockers clear. Must run regardless of current blocked overlay.                                                   |
| `auto_drive`                | work_starting         | Registered in `dispatch_builtin`; driven by the `auto-drive` subscriber on `('' → planning)` to spawn `tasks drive`. Gated on `activation == 'active'` so inactive rows do not auto-combust.                                                                          |
| `auto_promote`              | observation_lifecycle | Registered in `dispatch_builtin`; driven by the `auto-promote` subscriber on `observations: confirmed → ready`. Mints a tasks row at `planning`; the minted row lands inactive by P2 (Task 2.4), so no activation gate on the subscriber itself is required.          |
| `auto_resolve_observation`  | safety_reconcile      | Registered in `dispatch_builtin`; driven by the `auto-resolve-observation` subscriber on a wide menu of terminal-edge transitions. Reconciles linked observations and must run for every task regardless of activation.                                              |
| `auto_scaffold`             | safety_reconcile      | Registered in `dispatch_builtin`; driven by the `auto-scaffold` subscriber on `('' → planning)` to ensure the worktree + main.md projection exist. Workspace scaffolding is a prerequisite for every task; never gated.                                              |
| `cargo_install`             | ceremony_post_accept  | Registered in `dispatch_builtin`; driven by the `cargo-install` subscriber on `(integrating → integrated)`. Post-`integrated` repo-specific ceremony — the integration lane has already enforced activation upstream, so no separate gate is needed here.            |
| `external_review`           | observation_lifecycle | Registered in `dispatch_builtin`; driven by the `external-review` subscriber on `external_reviews: ('' → pending)` and `(tooling_held → pending)`. Operates on the external_reviews store, not tasks; not gated on task activation.                                  |
| `gatekeeper_router`         | observation_lifecycle | Not registered in `dispatch_builtin`; called inline by intake-routing handlers. Moves intake rows through their lifecycle; observation/intake-side, not task-combusting.                                                                                              |
| `gatekeeper_router_drain`   | observation_lifecycle | Not registered in `dispatch_builtin`; drains backlogged intake rows through the router. Observation/intake-side, not task-combusting.                                                                                                                                 |
| `gatekeeper_stub`           | observation_lifecycle | Registered in `dispatch_builtin`; test-shaped subscriber on `intake: (draft → triaging)`. Observation/intake-side, not task-combusting.                                                                                                                               |
| `integrate`                 | work_starting         | Registered in `dispatch_builtin`; driven by the `integrate` subscriber on `(accepted → integration_queued)` and `(integration_blocked → integration_queued)`. Drives the integration lane; gated on `activation == 'active'` (T140 P2) plus a schema guard on `start-integration`. |
| `investigator`              | observation_lifecycle | Registered in `dispatch_builtin`; driven by the `investigator` subscriber on `observations: (open → needs_investigation)` and `(confirmed → needs_investigation)`. Observation-side; not gated on task activation.                                                    |
| `schema_migrate`            | ceremony_post_accept  | Registered in `dispatch_builtin`; driven by the `schema-migrate` subscriber on `(integrated → cargo_installed)`. Post-`integrated` repo-specific ceremony; activation already enforced upstream by the integration lane.                                              |
| `user_escalation`           | safety_reconcile      | Registered in `dispatch_builtin`; driven by the `user-escalation` subscriber on `(accepted → deploy_blocked)` and `(cargo_installed → deploy_blocked)` (and serves as the default `deployment_specialist`). Always runs to file an escalation observation; never gated. |

## Gating discipline

Only the `work_starting` class carries an `activation == 'active'` predicate
on its subscriptions in `.stores/agents.yaml`. The schema additionally guards
`integration_queued → integrating` with `guard: "activation == 'active'"` as
defense-in-depth: even if a future caller bypasses the subscriber, the
substrate refuses to advance an inactive row through the integration lane.

The other four classes (`safety_reconcile`, `ceremony_post_accept`,
`observation_lifecycle`, `deprecated_internal`) carry no `$activation`
predicate. A test in `tests/activation_gating.rs` parses `agents.yaml` and
asserts this property.
