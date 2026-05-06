# Deep Architecture Checks

**Date:** 2026-05-06
**Type:** note

## Summary

Second-pass architectural review of the upcoming/open observations and recent task direction. The system's philosophy remains strong, but several open directions risk contradicting it if implemented as local patches: daemon code is becoming a shadow workflow engine, `dispatch_locks` is acting like an unmodeled lifecycle buffer, T1 contract-is-plan is represented as `plan = null`, recovery verbs risk becoming authority bypasses, and sidecars risk moving decisions/tokens outside the typed substrate.

## Details

### 1. Daemon/builtin code is becoming a shadow workflow engine

The philosophy says the schema is the cognitive center. But current/open fixes place increasing lifecycle intelligence in specific builtins and daemon handlers: `auto_promote.rs`, `auto_drive.rs`, `agents_run.rs`, watchdog scans, schema-migrate ceremony, dispatch lock seeding, retry behavior.

Relevant observations: L087, L107, L116, L117, L122.

These are not independent bugs. They show that subscribers can create rows, mark locks successful, bypass on-entry actions, and interpret liveness independently of the generic lifecycle engine. This contradicts the schema-centered architecture.

Preferred direction: define a generic subscriber execution contract. A subscriber should declare expected postconditions; the framework should verify them before `last_status=ok`; all row creations/transitions performed by subscribers should route through normal lifecycle/on-entry machinery.

### 2. `dispatch_locks` is really a lifecycle buffer, but is not modeled as one

`dispatch_locks` currently carries lifecycle-like states implicitly: candidate, claimed, running, succeeded, failed, stale, retry-wait, skipped-historical. Because this is not represented as a first-class buffer/state machine, bugs cluster around stale locks, false zombie detection, retry gaps, and duplicate drives.

Relevant observations: L039, L087, L107, L116, L122.

This conflicts with the primitives doc: important operational flow should be typed buffers connected by transitions/subscribers. Dispatch attempts should either become their own buffer or `dispatch_locks` should be formalized with daemon epoch, claim source, attempt number, pid/session identity, heartbeat, expected postcondition, terminal reason, and retry eligibility.

### 3. T1's design is sound, but its representation is wrong

T1 means: contract is the plan. The implementation effectively makes it: plan is absent. That null-plan representation leaks everywhere.

Relevant observations: L109, L117, L123, L126, L130.

Adding scattered T1 special cases weakens the architecture. Better: normalize execution shape. Either introduce `execution_shape = contract_only | one_phase_plan | multi_phase_plan`, or synthesize a one-phase plan during skip-plan from the contract. Then executor briefs, code-review guards, resume logic, and next-action can consume a uniform structure.

### 4. Transition ordering/fallback semantics need schema-level validation

The current task schema has an unguarded `blocked -> ready` resume transition before a guarded `blocked -> planning` transition intended for non-T1 plan-missing recovery. If first-match semantics apply, the specific guarded path is shadowed by the default. Even if the implementation handles this differently, the schema is too easy to misread and too easy to misorder.

Architectural recommendation: make fallback transitions explicit (`fallback: true`) or validate that guarded transitions precede unguarded fallbacks for the same `(from, verb, gate)` tuple.

### 5. Abandon/drop is needed but risks becoming an authority bypass

L124 is directionally correct: stale/duplicate tasks need a terminal path. But a broad `tasks abandon` verb can become a way to make inconvenient rows disappear, bypassing the back gate and audit semantics.

Cases should be separated:

- `abandoned`: task should not be done.
- `superseded`: replaced by another task/commit.
- `closed_out_of_band`: work already shipped outside the normal drive.
- `rejected`: human reviewed output and declined it.

Each needs reason/provenance. Some need tier-A human approval. This is Loop + Causality, not an admin delete.

### 6. Recovery/admin pressure must stay inside lifecycle transitions

Open rows L002, L069, L070, L092, L124, and L072 all ask for recovery paths. That need is real. The danger is implementing privileged status mutation. Recovery verbs must be typed lifecycle transitions, not `set-status`, raw status updates, or delete/rollback shortcuts.

Good shapes: `tasks supersede --by`, `tasks close-out-of-band --commit`, `tasks resume-deploy`, `tasks retry-ceremony`, `tasks replan`. Bad shape: `tasks force-close` or `tasks set-status accepted`.

### 7. Observation workflow direction is internally confused

L006 proposes codifying a C-hybrid pattern where T1 is handled in chat and T2/T3 promote to task, removing the idea of T2 observation lifecycle work. But the philosophy says filing is cheap because refinement is the substrate's burden, and current doctrine says T1/T2 may be handled inside observation lifecycle while T3 promotes.

This is a real strategic fork:

- Observation-as-intake-only: simpler, but pushes most work into tasks.
- Observation-as-refinement-buffer: aligns with philosophy, but requires a real observation workflow/drive.

The current docs lean toward the second. L006 leans toward the first. This should be reconciled before building more observation machinery.

### 8. Auto-investigator should not become a push-shaped contract generator

L043 is aligned with the orchestrator discipline: deep investigation should route to a subagent instead of blocking the main thread. But it could violate the human-attention principle if it produces large contract drafts or broad menus.

The investigator should output evidence, duplicate candidates, confidence, proposed tier, and at most one next grill question. Human attention should remain a pull/pruning operation, not validation of a large LLM-generated contract.

### 9. Watch sidecars risk undermining the typed decision ledger

L075's TUI sidecars are useful, but they pull toward per-row chat contexts where decisions happen outside the substrate. This conflicts with the primitives doc's unified typed inbox doctrine.

Relevant observations: L081, L090, L091.

Sidecars should be read/analysis/proposal surfaces only. Decisions must become typed rows or transitions via CLI writes. The token model must be lazy and U-moment-scoped; eager token preload into every sidecar contradicts approval-token doctrine.

### 10. DockerRunner belongs at the runner boundary only

L019's DockerRunner direction is mostly aligned if it remains a generic Runner implementation: resource limits, mounts, network policy, environment policy, transcript capture. It becomes misaligned if stores starts owning project-specific setup, dependency bootstrapping, or deploy semantics. Those remain outside the substrate and should be project-declared checks/scripts.

### 11. `--meta` is aligned but needs explicit provenance

L056 supports realistic-pull by making cross-project filing cheap. But it blurs authority/provenance unless every meta write records source cwd, source substrate path if any, target substrate path, invoker, detection source, target policy hash, and filed-from-project metadata. This is Causality/Activity pressure.

### 12. Checks are emerging everywhere but still ad hoc

Many open observations are really asking for deterministic checks: L061 pre-promotion acceptance, deploy conflict prechecks, L060 schema migration verification, cargo-install/schema-migrate ceremony, L110 Pi smoke test, L121 runner timeout/liveness, L120 planner persistence verification.

The missing Check primitive should be promoted. Otherwise every ceremony will grow bespoke pass/fail states and custom recovery logic.

## Follow-ups

- Promote Loop, Check, Causality, and Dispatch Attempt Buffer from missing primitives into near-term architectural design.
- Normalize T1 execution shape rather than adding more `plan = null` special cases.
- Add schema validation for guarded transition/fallback ordering.
- Split terminal recovery outcomes: abandoned, superseded, closed_out_of_band, rejected.
- Reconcile L006 with the philosophy's observation-refinement doctrine before implementing more observation workflow.
- Constrain sidecars to proposal/read surfaces and remove eager token propagation.
