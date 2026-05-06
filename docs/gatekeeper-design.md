# Gatekeeper Design

**Path:** `docs/gatekeeper-design.md`
**Status:** design doc (T045 phase 2). Pre-implementation; an executor should be able to schema-codify from this.
**Companion doctrine:** `docs/architecture-coherence.md` (T045 phase 1).
**Companion taxonomy:** `docs/risk-and-cluster-taxonomy.md` (T045 phase 3) — canonical definitions for `risk_flags`, `cluster_key` conventions, and the orthogonal (size_tier, risk_class, approval_policy) triple referenced throughout this doc.
**Brainstorm seed:** `docs/worklog/2026-05-06/06-gatekeeper-architecture-observability.md`.

## Purpose

The substrate has *local* observability: agents file observations when friction surfaces and the orchestrator routes. It does not have *architectural* observability: a layer that asks, across a cluster of locally-correct fixes, whether the underlying shape is still coherent. The gatekeeper is that missing layer.

Concretely the gatekeeper is a typed intake buffer (`intake_items`) plus a specialized router agent that classifies, deduplicates, requests recon, fast-tracks safe trivia, and escalates architecture-risk clusters before they accrete into drift. Local agents continue filing pain freely; the gatekeeper decides where each filing belongs.

This design names:
1. The `intake_items` lifecycle state machine (states, transitions, actor classes, guards).
2. The `intake_items` row schema (fields, types, required-ness).
3. The six routing decisions the gatekeeper emits and their downstream effects.
4. The structured-output schema the gatekeeper agent must return.
5. The narrow contract for the recon agent that fills `needs_info` gaps.
6. How the design reconciles with substrate doctrine (typed rows, CLI-only writes, `--invoker` discipline).

It does not specify the gatekeeper agent's prompt, the architecture-review agent's contract (T045 phase 4), or the implementation plan (T045 phase 5). It does specify enough that a downstream task can be ratified against it.

## Lifecycle

`intake_items` rows move through five states. Transitions are CLI verbs with explicit actor classes and guards. The shape mirrors `observations` and `tasks` so the substrate's existing transition-history machinery applies unchanged.

| From         | To             | Verb (CLI)                            | Actor                  | Guard                                                                 |
|--------------|----------------|---------------------------------------|------------------------|-----------------------------------------------------------------------|
| —            | `draft`        | `intake add`                          | `ai_autonomous`        | `summary` and `source_agent` non-empty.                               |
| `draft`      | `triaging`    | `intake claim-triage`                 | `ai_autonomous`        | One row at a time per gatekeeper instance (advisory, not enforced).   |
| `triaging`  | `needs_info`   | `intake route --decision needs_info`  | `ai_autonomous`        | `gatekeeper_decision_json` set and `missing_info_question` non-empty. |
| `needs_info` | `triaging`    | `intake recon-return`                 | `ai_autonomous`        | `evidence` updated since `needs_info` entry; bumps `recon_round`.     |
| `triaging`  | `routed`       | `intake route --decision <D>`         | `ai_autonomous`        | `D ∈ {duplicate, fast_track, normal_observation}` and side-effect rows (observation / merge target) created in same transaction. `arch_review_candidate` does NOT route here — it routes to `escalated` (see below). |
| `triaging`  | `dropped`      | `intake route --decision reject_noise`| `ai_autonomous`        | `rationale` non-empty; rejection is final unless `intake reopen` (below) fires. |
| `dropped`    | `draft`        | `intake reopen`                       | `ai_with_human`         | Human disagrees with a `reject_noise` decision; rare escape hatch.    |
| `triaging`  | `escalated`    | `intake escalate-arch-review`         | `ai_autonomous`        | Decision is `arch_review_candidate` per its full precondition (any `touches_*` flag, OR `introduces_new_primitive` / `changes_boundary` / `security_sensitive` / `authority_surface_drift` / `contradicts_prior_decision`, OR cluster threshold crossed); produces an `architecture_reviews` row (or tagged-observation stand-in until that store exists). |

State semantics:

- **`draft`** — local agent's raw filing. Not yet seen by the gatekeeper. May contain a `suggested_fix` but is treated as data, not policy.
- **`triaging`** — the gatekeeper has claimed the row and is producing a structured decision. Short-lived.
- **`needs_info`** — gatekeeper has asked a recon agent to gather missing evidence; row holds until evidence returns.
- **`routed`** — terminal-success state. The row points at exactly one downstream artifact (observation row, fast-track resolution row, duplicate target, or arch-review candidate). A `routed` row is read-only history.
- **`escalated`** — synonym sub-state of `routed` for `arch_review_candidate` decisions; kept distinct so dashboards can count architecture pressure separately from normal flow.
- **`dropped`** — terminal-noise state. The row is preserved for audit; nothing downstream is created.

Guards summary: every transition out of `triaging` requires a complete `gatekeeper_decision_json` payload (see § *Gatekeeper output schema*) and any same-transaction side-effects the decision implies. The substrate's existing transition-history hook captures the actor, invoker, timestamp, and decision blob automatically.

## Fields

`intake_items` row schema. Types are sketched in schema.yaml-style; an executor codifying this should follow the patterns in `schema.yaml` for `observations` and `tasks`.

```yaml
intake_items:
  display_id:                 # string, "I001"… (own namespace; not L###/T###)
    type: text
    required: true
    actor: ai_autonomous
  state:
    type: enum
    values: [draft, triaging, needs_info, routed, escalated, dropped]
    required: true
    default: draft
    actor: ai_autonomous
  summary:
    type: text
    required: true
    actor: ai_autonomous
  body:
    type: text
    required: false
    actor: ai_autonomous
  source_task:                # soft-FK to tasks.display_id (T###); plain text
    type: text
    required: false
    actor: ai_autonomous
  source_agent:               # role name: planner|executor|code_reviewer|orchestrator|...
    type: text
    required: true
    actor: ai_autonomous
  suggested_fix:              # raw filer's proposal; advisory only
    type: text
    required: false
    actor: ai_autonomous
  evidence:                   # ndjson of recon findings (paths, greps, repro)
    type: text
    required: false
    actor: ai_autonomous
  gatekeeper_decision_json:   # full structured-output payload (see schema below)
    type: json
    required: false           # null until first triage
    actor: ai_autonomous
  risk_flags:                 # mirrored out of decision_json for indexed query
    type: json_array
    items: enum               # see § Gatekeeper output schema
    required: false
    actor: ai_autonomous
  cluster_key:                # mirrored out of decision_json for indexed query
    type: text
    required: false
    actor: ai_autonomous
  duplicate_of:               # soft-FK to intake_items.display_id when decision=duplicate
    type: text
    required: false
    actor: ai_autonomous
  routed_to_observation:      # soft-FK to observations.display_id (L###)
    type: text
    required: false
    actor: ai_autonomous
  routed_to_arch_review:      # soft-FK to architecture_reviews.display_id (or tag id) when escalated
    type: text
    required: false
    actor: ai_autonomous
  recon_round:                # 0 on draft; bumps on each needs_info → triaging return
    type: int
    required: true
    default: 0
    actor: ai_autonomous
  captured_at:
    type: timestamptz
    required: true
    actor: ai_autonomous
  captured_week:              # "wNN" — mirrors observations
    type: text
    required: true
    actor: ai_autonomous
```

Two cross-row invariants worth schema-enforcing as validators:

1. A `routed` row with `decision = duplicate` must have `duplicate_of` set and `routed_to_observation` null.
2. A `routed` row with `decision ∈ {fast_track, normal_observation}` must have `routed_to_observation` set.

## Routing decisions

The gatekeeper emits exactly one of six decisions. Each has a precondition (when it applies) and a downstream effect (what the substrate does next).

### 1. `duplicate`
**Precondition:** `duplicate_candidates[]` is non-empty AND `confidence ≥ medium` that the new row repeats one of them. The candidate is itself an `intake_items` row in `routed` state, OR an `observations` row already covering the same cluster.
**Downstream effect:** the new row transitions `triaging → routed` with `duplicate_of = <candidate-id>`. No observation is created. The candidate's `cluster_key` is copied onto the new row so cluster counts increment. If the candidate is an observation, the observation's `body` may receive an appended evidence note (autonomous append) but its contract is untouched.

### 2. `needs_info`
**Precondition:** evidence is insufficient to classify (cannot assess `risk_flags`, cannot decide between `fast_track` and `normal_observation`, cannot confirm or deny duplicate candidates). `missing_info_question` must be non-empty.
**Downstream effect:** row transitions `triaging → needs_info`, a recon agent is dispatched with the narrow brief described in § *Recon agent contract*, and the row holds until `intake recon-return` fires. `recon_round` bumps. A hard cap of `recon_round ≤ 2` prevents loops; on round 3 the gatekeeper must pick a non-`needs_info` decision.

### 3. `fast_track`
**Precondition:** ALL of: `tier_hint = T0` or `T1`; `risk_flags` contains only `docs_only`, `small_local_fix`, or `duplicate_symptom`; `risk_flags` contains NONE of the `touches_*` flags or `security_sensitive`; `confidence = high`.
**Downstream effect:** row transitions `triaging → routed` with `routed_to_observation` set to a freshly created observation row in a pre-resolved or fast-track-eligible state (exact mechanism is phase-5 detail). The fast-track does NOT bypass observation lifecycle — it parameterizes it. Audit trail is preserved by the gatekeeper-decision blob plus the resulting observation's transition history.

### 4. `normal_observation`
**Precondition:** evidence is sufficient, no duplicate dominates, **NO high-risk risk_flags are present**, and the cluster threshold has not crossed. The high-risk set is the canonical taxonomy enum (see `docs/risk-and-cluster-taxonomy.md` § *Risk-flag enumeration*): `touches_actor_authority`, `touches_lifecycle`, `touches_subscriber_semantics`, `touches_runner_boundary`, `touches_schema_core`, `introduces_new_primitive`, `changes_boundary`, `security_sensitive`, `authority_surface_drift`, `contradicts_prior_decision`. Any one of these ALWAYS escalates to `arch_review_candidate` (decision 5) regardless of cluster count. Low-risk helper flags (`docs_only`, `small_local_fix`, `duplicate_symptom`) and normal product/bug flags may route to `normal_observation`. Cluster threshold is for lower-risk repeated symptoms that individually look local.
**Downstream effect:** row transitions `triaging → routed`, a new `observations` row is created (state `open`, `intent_contract` empty until ratification), and `routed_to_observation` points at it. The `cluster_key` and `risk_flags` are copied onto the observation as advisory metadata.

### 5. `arch_review_candidate`
**Precondition:** any of:
- `risk_flags` contains any `touches_*` flag (`touches_actor_authority`, `touches_lifecycle`, `touches_subscriber_semantics`, `touches_runner_boundary`, `touches_schema_core`),
- `risk_flags` contains `introduces_new_primitive` or `changes_boundary`,
- the gatekeeper observes that `cluster_key` count has crossed the architecture-review threshold (cluster threshold default: 3, configurable),
- `risk_flags` contains `security_sensitive` or `authority_surface_drift`,
- `risk_flags` contains `contradicts_prior_decision` (the suggested fix directly contradicts a previously ratified observation contract or accepted task outcome — that is itself an architectural-coherence question).
**Downstream effect:** row transitions `triaging → escalated`, an `architecture_reviews` candidate row is produced (or, until that store ships, a tagged observation with reserved tag `arch-review-candidate`), and `routed_to_arch_review` points at it. A normal observation MAY also be created in parallel so local-fix work is not blocked, but its contract carries a `pending_architecture_review = true` field that prevents U1 ratification until the architecture review returns `allow_local_fix` or `reframe_contract`.

### 6. `reject_noise`
**Precondition:** the row is a re-symptom of a fully-resolved observation, an artifact of a known-broken local environment, or otherwise not actionable substrate signal. `rationale` must explain which of these applies.
**Downstream effect:** row transitions `triaging → dropped`. Nothing downstream is created. A future `intake reopen` (`ai_with_human`) is the only path back. Drops are still queryable for noise-rate dashboards.

## Gatekeeper output schema

The gatekeeper agent emits a single JSON object validated against the schema below. Field types and enums are tightened relative to the worklog draft; free-form prose is restricted to `rationale`, `missing_info_question`, and `recommended_next`.

```yaml
# JSON Schema (sketched in schema.yaml-style enums and types)
gatekeeper_decision:
  type: object
  required:
    - decision
    - confidence
    - rationale
  properties:
    decision:
      type: enum
      values:
        - duplicate
        - needs_info
        - fast_track
        - normal_observation
        - arch_review_candidate
        - reject_noise
    confidence:
      type: enum
      values: [low, medium, high]
    tier_hint:
      type: enum
      values: [T0, T1, T2, T3]
      required_when: "decision in {fast_track, normal_observation, arch_review_candidate}"
    risk_flags:
      type: array
      items:
        type: enum
        values:
          # Architectural-risk flags (any one triggers arch_review_candidate eligibility)
          - touches_actor_authority
          - touches_lifecycle
          - touches_subscriber_semantics
          - touches_runner_boundary
          - touches_schema_core
          - introduces_new_primitive
          - changes_boundary
          - security_sensitive
          - authority_surface_drift
          # Low-risk / fast-track-eligible flags
          - docs_only
          - small_local_fix
          # Bookkeeping flags
          - duplicate_symptom
          - contradicts_prior_decision
      uniqueItems: true
    duplicate_candidates:
      type: array
      items:
        type: string                        # intake_items.display_id OR observations.display_id
        pattern: "^(I|L)[0-9]{3,}$"
      required_when: "decision == duplicate"
      min_items_when_required: 1
    cluster_key:
      type: string                          # short kebab-case stable label, e.g. "t1-null-plan"
      pattern: "^[a-z][a-z0-9-]{2,40}$"
      required_when: "decision in {normal_observation, arch_review_candidate, duplicate}"
    missing_info_question:
      type: string
      max_length: 400
      required_when: "decision == needs_info"
    recommended_next:
      type: string
      max_length: 400
    rationale:
      type: string
      max_length: 1200
  additional_properties: false
```

The substrate stores this payload on `intake_items.gatekeeper_decision_json` verbatim. `risk_flags` and `cluster_key` are mirrored onto top-level columns for indexed queries (cluster counts, risk-flag dashboards) without re-parsing JSON.

## Recon agent contract

The recon agent fires on every `needs_info` decision. Its brief is deliberately narrow: gather evidence, do NOT design the solution. The contract:

**Inputs:**
- `intake_item.display_id`
- `intake_item.summary`, `intake_item.body`, `intake_item.suggested_fix`
- `intake_item.source_task`, `intake_item.source_agent`
- `gatekeeper_decision_json.missing_info_question` (the specific gap)

**Allowed actions:**
- Read files, grep, run read-only CLI verbs (`stores tasks status`, `stores observations show`, etc.).
- Execute reproduction steps if the item names a specific repro.
- Run `git log` / `git diff` against named paths.

**Forbidden actions:**
- Proposing a fix or rewriting `suggested_fix`.
- Editing any file outside the `evidence` field on the intake row.
- Creating observations, tasks, or other intake rows.
- Calling any `submit-*`, `route`, `accept`, or `reject` verb.
- Calling tier-A or tier-B writes of any kind.

**Output:** a single CLI call `intake recon-return --id Ixxx --evidence-from-file <path> --invoker ai_autonomous`. The `evidence` payload is ndjson with one line per finding: `{kind, path, line?, snippet?, summary}`. The recon agent's exit returns control to the gatekeeper, which re-triages the row from `needs_info` back to `triaging` with `recon_round + 1`. After two unproductive recon rounds, the gatekeeper must pick a non-`needs_info` decision; this is enforced by the lifecycle guard, not the agent.

This split keeps investigation inside substrate-visible flow (consistent with the L043 triage-routing rule) without letting the recon agent drift into design.

## Architecture-review triggers

The architecture-review agent is the coherence gate. It does not fire on every routed row; it fires when the gatekeeper's classification or the cluster state crosses a named trigger. Each trigger class below is decidable from substrate-visible state — no human prompting, no out-of-band signal — so that "did review fire when it should have?" is auditable after the fact.

### Trigger 1: `risk-flag` (per-row, immediate)
**Fires when:** the gatekeeper's `gatekeeper_decision_json.risk_flags` includes any of `touches_actor_authority`, `touches_lifecycle`, `touches_subscriber_semantics`, `touches_runner_boundary`, `touches_schema_core`, `introduces_new_primitive`, `changes_boundary`, `security_sensitive`, `authority_surface_drift`, or `contradicts_prior_decision`.
**Threshold:** 1 (any single flag is sufficient).
**Latency budget:** review must be requested within the same triage transaction; routing to `escalated` without firing review is a schema violation.

### Trigger 2: `cluster-threshold` (cross-row, count-based)
**Fires when:** the count of `routed` + `escalated` `intake_items` rows sharing a single `cluster_key` (within a rolling 30-day window) crosses the configured threshold.
**Threshold:** default `≥ 3` for general clusters; default `≥ 2` for clusters whose registry entry carries `architectural_priors = true` (e.g., `dispatch-lifecycle`, `sidecar-token`, `t1-null-plan`); per-cluster override allowed in the cluster-key registry.
**Effect:** the *triggering* row routes to `arch_review_candidate`; earlier rows in the cluster are *not* retro-escalated by this trigger (see Open Question 6) — but Trigger 5 below sweeps them.

### Trigger 3: `pre-ratification` (per-observation, gating)
**Fires when:** an observation row created by the gatekeeper carries `pending_architecture_review = true` and a U1 ratification (`observations update --contract-state ready`) is attempted.
**Threshold:** 1 (the flag itself is the trigger).
**Effect:** the ratification verb is rejected fail-loud until architecture review returns one of `allow_local_fix`, `reframe_contract`, `merge_with_cluster`, or `propose_doctrine_update`. This is the hard gate that prevents locally-correct fixes from accumulating before coherence has been checked.

### Trigger 4: `periodic-sweep` (scheduled, cluster-wide)
**Fires when:** a scheduled job runs (default cadence: weekly) over the open `intake_items` and `observations` populations and identifies clusters whose count is `≥ 5` over the trailing 90 days but which have *no* `architecture_reviews` row.
**Threshold:** count `≥ 5` per cluster_key over 90 days AND `architecture_reviews.count_for(cluster_key) == 0`.
**Effect:** a `periodic-sweep` review row is created for each unreviewed cluster; the sweep is the safety net for clusters that crept past Trigger 2 because no single triage saw the threshold cross (e.g., the registry threshold was raised after some rows already routed).

### Trigger 5: `post-accept-batch` (retrospective, post-merge)
**Fires when:** a batch of `≥ 3` `tasks` rows reaches `accepted` state within a 14-day window AND the union of their `linked_observations`' `cluster_key` values overlaps in `≥ 2` distinct clusters.
**Threshold:** task count `≥ 3` in 14 days; cluster overlap `≥ 2`.
**Effect:** an architecture-review row is created with `kind = retrospective` and `evidence = list of accepted task display_ids`. Retrospectives can return `propose_doctrine_update` or `create_primitive_task`; they cannot block already-accepted tasks (those are terminal), but they can ratify a doctrine entry or seed a primitive task that prevents the next round of drift.

## Architecture-review outputs

Each architecture-review invocation returns exactly one outcome. Outcomes are typed enums (mirroring the gatekeeper-decision schema) so downstream consumers do not parse prose. Every outcome carries a `rationale` (`max_length: 1200`) and, where applicable, a typed payload (a target row id, a doctrine path, a proposed primitive name).

### 1. `allow_local_fix`
The reviewer agrees the cluster does not require architectural change; the local observation may proceed to U1 ratification on its own contract. The pending-review flag on the observation is cleared. Rationale must explicitly name the architectural shape the reviewer considered and rejected — not "looks fine," but "considered whether this widens authority surface; it does not, because X." This output is the most common; it is also the most dangerous if the reviewer becomes a rubber stamp, so the audit trail is mandatory.

### 2. `reframe_contract`
The local fix is allowed *only after* the observation's `intent_contract.shape_change` is rewritten in line with reviewer-supplied guidance. The reviewer attaches a `reframe_directive` blob (`max_length: 2000`) describing what the contract must say differently. Until the observation's contract is updated to match (and `pending_architecture_review` re-evaluated), U1 ratification stays blocked.

### 3. `merge_with_cluster`
The row is duplicative of an already-routed cluster at the architectural level even when the surface symptom appeared distinct. The output names a `merge_target_id` (an `architecture_reviews` row or a parent `cluster_key`) and the row's downstream observation (if any) is dropped or pointed at the merged cluster's outcome. Distinct from the gatekeeper's `duplicate` decision because merge is *architectural* equivalence, not surface duplication.

### 4. `create_primitive_task`
The cluster's resolution is not a local fix at all; it is a new typed primitive (a new store, a new state, a new subscriber, a new CLI verb class). The output names a `proposed_primitive` label and authorizes a downstream `tasks add --invoker ai_with_human` for a primitive-introduction task. The local observations remain `open` until the primitive task accepts; they are then closed with `resolved_by = T###`.

### 5. `block_pending_fixes`
The cluster is currently incoherent enough that *no* further local fixes in the area should accept-merge until a sequencing decision is made. The output names a `block_scope` (a list of cluster_keys, paths, or risk_flags) and a `block_until` predicate (e.g., "until `tasks/T0NN` accepts"). Tasks whose `linked_observations` intersect the block scope are held at `in_review` (cannot transition to `accepted`) until the block lifts. This is the heaviest hammer and is reserved for the cases the doctrine in `docs/architecture-coherence.md` explicitly anticipates.

### 6. `propose_doctrine_update`
The cluster surfaces a doctrinal gap rather than a code change: `CLAUDE.md`, `docs/philosophy.md`, `docs/architecture-coherence.md`, or another doctrine doc lacks the rule that would have caught this class. The output names the target doc and a `proposed_paragraph` (`max_length: 4000`). A separate `tasks add` for the doctrine update is authorized; the cluster's observations close with `resolved_by = doctrine`.

### 7. `request_human_arch_decision`
The reviewer cannot decide between two architectural shapes within the substrate's autonomous-decision surface — the choice itself is a U-moment. The output names the two (or more) candidate shapes with one-paragraph trade-off summaries and halts the cluster pending human decision. The downstream observation's contract stays unratified; tasks stay un-promoted; the human is the only path forward. This output is rare and is the architecture-review analog of the gatekeeper's `needs_info` — except the gap is judgment, not evidence.

## Fast-track policy

Fast-track is the gatekeeper's mechanism for moving locally-trivial filings through with reduced ceremony. It is power that, mishandled, becomes the exact drift this whole layer exists to prevent. The policy below defines its boundary in two parts: an explicit ALLOW list (the only conditions under which fast-track is permitted) and an explicit PROHIBIT list (surfaces on which fast-track is structurally forbidden, regardless of how locally-trivial the change appears).

### ALLOW list (fast-track is permitted only when ALL conditions hold)

1. `tier_hint ∈ {T0, T1}` AND
2. `risk_flags ⊆ {docs_only, small_local_fix, duplicate_symptom}` AND
3. `risk_flags ∩ {touches_actor_authority, touches_lifecycle, touches_subscriber_semantics, touches_runner_boundary, touches_schema_core, introduces_new_primitive, changes_boundary, security_sensitive, authority_surface_drift, contradicts_prior_decision} = ∅` AND
4. `confidence == high` AND
5. The cluster_key (if any) has not crossed the `cluster-threshold` trigger (Trigger 2) — even one fast-track-eligible row gets re-routed to `normal_observation` once the cluster is in arch-review territory AND
6. The change is mechanically verifiable: there exists a deterministic check (test, lint, typecheck, doc render) whose pass/fail outcome is the entire acceptance criterion.

### PROHIBIT list (fast-track is forbidden, no exceptions)

The following surfaces may NEVER be fast-tracked, regardless of how trivial any individual filing on them looks. This list is verbatim from the task's `scope_out`:

1. **authority** — actor-class assignments on any field or transition; `--invoker` discipline; the `human` / `ai_with_human` / `ai_autonomous` boundary.
2. **lifecycle** — state-machine shape on any typed store; transition guards; on-entry / on-exit hooks.
3. **schema** — `schema.yaml` itself; transition-history machinery; validators; the rendered-projection contract.
4. **subscriber** — auto-promote (L046), any future fire-on-event subscriber, the conditions under which subscribers fire and their side-effect surface.
5. **runner** — Mac / Pi runner protocols; sidecar handshakes; dispatch-lock semantics; cross-host claim mechanics.
6. **deploy** — release pipelines; container orchestration; the carry-forward-secrets pattern; broker / sidecar deployment shape.
7. **security** — secrets handling, sandbox boundaries, the threat model surface broadly.
8. **approval-token** — token generation, encryption-at-rest, decryption flow, verification path, anywhere the token could be widened, persisted, or pre-fetched.

A filing that touches any PROHIBIT surface MUST route to `normal_observation` at minimum and to `arch_review_candidate` if any of the corresponding `touches_*` or `*_drift` flags fire (which they will by construction). The gatekeeper output schema enforces this: the `fast_track` decision is rejected fail-loud when any PROHIBIT-surface flag is present.

### Required audit trail for every fast-track

Every fast-tracked row produces, in the same transaction, three artifacts:

1. **Gatekeeper decision row** — the full `gatekeeper_decision_json` payload on `intake_items.gatekeeper_decision_json`, including the `rationale` field naming why fast-track was permitted (which ALLOW conditions held).
2. **Deterministic check record** — a `fast_track_record` (or, until that store ships, a structured note appended to the observation's `body`) naming the verifying check, its command line, and its captured exit code / output. Without a deterministic-check record, the fast-track is not auditable and the gatekeeper output schema rejects the decision.
3. **Terminal closure** — the resulting observation row is created in a pre-resolved state (or a fast-track-eligible state) with `resolved_by = fast_track:I###` pointing back at the intake item. The observation is not eligible for re-opening except via `intake reopen` followed by a fresh triage.

These three artifacts together form the fast-track audit trail. Any one missing turns the row's `routed` state into a schema-rejection condition.

## Failure modes / abuse cases

The gatekeeper itself can drift. The cases below name three concrete drift modes plus their counter-measures, so a future reviewer can check whether the layer is still doing its job or has begun rubber-stamping. None of these counter-measures are policed by the gatekeeper agent itself — that would be the fox guarding the henhouse — they are external-to-the-agent surfaces (dashboards, schema validators, periodic reviews) the substrate maintains.

### Abuse case 1: Risk-flag underuse
**Drift shape:** the gatekeeper, optimizing for throughput, omits `touches_*` flags on rows that genuinely touch those surfaces — classifying schema-touching changes as `small_local_fix` because the diff is short, or marking subscriber-semantics changes as `docs_only` because they ship as a comment edit. Local-fix routing accelerates; arch-review traffic dwindles; cluster-threshold (Trigger 2) never fires because rows never enter the relevant `cluster_key`.
**Counter-measures:**
1. **Risk-flag rate dashboard.** A computed view tracks the moving rate of each `touches_*` flag per 100 triaged rows. A drop of `> 50%` from the trailing 90-day baseline (without a corresponding drop in PR-touch rate on the same surfaces, measurable from `git log --stat` against schema/subscriber/runner paths) triggers a periodic-sweep review of recent classifications.
2. **Spot-check sampling.** Periodic-sweep (Trigger 4) randomly re-classifies `5%` of `routed` rows in the trailing window using a fresh gatekeeper invocation; disagreements above a tolerance cross-trigger an architecture-review of *the gatekeeper's own behavior*, not the underlying rows.
3. **Schema-side pattern matching.** A validator scans the row's `evidence` and `body` for token patterns indicating `touches_*` surfaces (e.g., `schema.yaml`, `actor:`, `subscribers/`, `dispatch_locks`, `approve_token`); if any pattern matches and no corresponding `touches_*` flag is set, the row is held in `triaging` for re-review rather than allowed to route.

### Abuse case 2: Cluster-key collision (over-coalescing or under-coalescing)
**Drift shape (over-coalescing):** the gatekeeper coins overly broad `cluster_key`s (`runner-stuff`, `lifecycle-thing`) under which many architecturally-distinct shapes pile, suppressing the cluster-threshold trigger for any one shape because the bucket is permanently above threshold and no single sub-shape is differentiable.
**Drift shape (under-coalescing):** the gatekeeper coins overly narrow `cluster_key`s (`runner-stuck-on-tuesday`, `dispatch-lock-near-bedtime`) so each filing lands in its own singleton bucket and Trigger 2 never fires.
**Counter-measures:**
1. **Curated registry promotion.** Cluster keys appearing `≥ 3` times promote into a curated registry (Open Question 1's expected resolution); free-form coining is permitted only outside the registry. Registry entries carry a one-paragraph definition; the gatekeeper must select an existing entry when its proposed key would be a near-synonym (Levenshtein-distance check at write-time).
2. **Cluster-cardinality guardrails.** Any cluster_key whose count `> 25` over a 30-day window AND whose underlying observations span `> 4` distinct file roots fires a forced architecture-review of *the cluster definition itself* — the suspicion is over-coalescing.
3. **Singleton-rate dashboard.** The proportion of `cluster_key`s with count `== 1` over the trailing 90 days is tracked; sustained values above `80%` indicate under-coalescing and trigger a registry-curation pass.

### Abuse case 3: Fast-track creep
**Drift shape:** the fast-track ALLOW list is honored letter-by-letter, but the gatekeeper begins classifying a wider population as `T0/T1 + small_local_fix` to qualify them. The PROHIBIT list still holds at the surface boundary, but borderline cases (a "small" lifecycle-adjacent change reframed as a "doc clarification") slip through. Over months, the local-fix queue grows, arch-review queue shrinks, and architectural pressure becomes invisible.
**Counter-measures:**
1. **Fast-track ratio cap.** The ratio of `decision == fast_track` to total triaged rows is tracked over a 30-day window; a hard cap of `35%` (configurable, but not by the gatekeeper) holds. When the cap is reached, further `fast_track` decisions are rejected fail-loud and the gatekeeper must pick `normal_observation` or escalate.
2. **Deterministic-check audit.** The `fast_track_record`'s named check is sampled (`10%` of fast-tracks) and re-run by a separate verifier role; mismatches between recorded and re-run outcomes block the fast-track surface for the gatekeeper instance pending review.
3. **PROHIBIT-surface diff scan.** Every fast-tracked observation's referenced files are matched against a maintained list of PROHIBIT-surface paths (`schema.yaml`, `subscribers/**`, `dispatch_*`, `auth/**`, deploy / runner roots). Any match — even via `body` mention — auto-escalates the row to architecture-review and flags the fast-track for retrospective audit.

These three abuse cases are not exhaustive — they are the classes most directly anticipated by the doctrine in `docs/architecture-coherence.md`. Additional drift modes (recon-loop abuse, decision-blob inflation, dropped-rate inflation) are listed under § *Open questions* with deferred counter-measures.

## Open questions

These are explicitly deferred; they do not block ratification of the design but should be resolved before the implementation task in phase 5.

1. **Cluster-key namespace.** Is `cluster_key` a free string the gatekeeper coins per call, or is there a curated registry (`t1-null-plan`, `dispatch-lifecycle`, `sidecar-token`, …) the gatekeeper must select from? The latter improves cross-row counting; the former allows organic growth. Likely answer: free now, registry later (cluster keys appearing ≥3× promote into the registry).
2. **Architecture-review store vs. tagged observations.** Should `architecture_reviews` be a dedicated typed store from day one, or do `arch_review_candidate` decisions land as observations with a reserved tag until the dedicated store earns its place? The latter ships sooner; the former matches the doctrine that architectural concerns deserve a typed surface.
3. **Fast-track audit shape.** A fast-tracked observation skips the planner/executor cycle. What is the minimum audit trail required — gatekeeper decision blob alone, or an additional `fast_track_record` row with the deterministic check that ran? Bias: the latter, but only if it can be done without bloating the gatekeeper's responsibility.
4. **Cluster threshold default and tuning.** The default of 3 for cluster-threshold escalation is a guess. It should be configurable per cluster_key in a config file the gatekeeper reads, not hardcoded.
5. **Gatekeeper concurrency.** If two filings arrive close in time, can two gatekeeper invocations both `claim-triage` distinct rows? The lifecycle guard is advisory; do we need an explicit lease, or is the per-row `triaging` state sufficient because each row is single-claimed?
6. **Re-evaluation of `routed` rows.** If a cluster threshold is crossed *after* a row was already routed to `normal_observation`, can earlier rows in the cluster be re-escalated? Or is the escalation only forward-looking? This determines whether `routed` is truly terminal or merely "currently terminal."
7. **Drop-rate auditing.** What is an acceptable `dropped`-rate? Too low suggests the gatekeeper is too permissive; too high suggests local agents are being trained to file noise. Needs a dashboard surface, not just a number.

### Substrate doctrine reconciliation

The design must hold under the substrate's existing doctrine. Each axis below is explicitly addressed.

**Typed rows.** `intake_items` is a first-class typed store with its own `display_id` namespace (`I###`), state machine, and validators — not a free-text sidecar, not a JSON blob inside another row, not a worklog convention. Risk flags and cluster keys are typed enums / patterned strings, not free-form prose. The gatekeeper's structured output is schema-validated before persistence; an invalid payload is rejected by the substrate, not silently downgraded.

**CLI-only writes.** Every transition is a named CLI verb (`intake add`, `intake claim-triage`, `intake route`, `intake recon-return`, `intake escalate-arch-review`, `intake reopen`). No state of the row mutates except through one of these verbs. There is no admin escape, no `intake force-state`, and no raw-SQL update path — the *Session doctrine — 2026-05-06* rule against raw-SQL writes applies to `intake_items` exactly as it does to `tasks` and `observations`. Reads via `sqlite3 ... SELECT` remain fine for dashboards.

**`--invoker` discipline.** Every CLI verb in the lifecycle table is `ai_autonomous` except `intake reopen`, which is `ai_with_human`. This matches the doctrine's default: filing, routing, and triaging are autonomous work; reversing a `dropped` decision is a U-moment because the human is overriding a machine judgment. No verb is `actor: human` because no transition is high-stakes enough to need the approval-token tier-A gate — the *consequential* gate is observation-contract ratification (still U1, still tier-A), which the gatekeeper does not perform. The gatekeeper produces the *candidate* observation; the human still ratifies. Risky cases route to `arch_review_candidate` and gate the downstream observation's U1 on architecture review's verdict.

**No sidecar bypass.** The gatekeeper does not give itself a privileged channel that other agents lack. It uses the same CLI verbs any agent could use, with the same `--invoker` enforcement. The only thing distinguishing the gatekeeper from a normal agent is *role* (it is the routing role) and *prompt* (it is told to produce structured decisions). The schema treats it identically. This matches the philosophy doctrine that the orchestrator does not get privileged access into the substrate.

**Auditability.** Every transition lands in the substrate's transition-history table (same machinery `tasks` and `observations` use). The full `gatekeeper_decision_json` is persisted; nothing about the routing decision is reconstructible-only-from-prose. Drops, fast-tracks, and escalations are queryable by SQL read or by a future `intake list --filter ...` verb. Cluster counts are computable from indexed `cluster_key` columns without re-parsing JSON.

**Architectural coherence (the doctrine the design is grounded by).** Per `docs/architecture-coherence.md`, local correctness does not imply architectural coherence. The gatekeeper does not claim to *enforce* coherence — it routes to architecture-review, which is a separate agent (T045 phase 4). The gatekeeper's contribution to coherence is making sure that risk-flagged and cluster-threshold-crossed items reach the coherence gate at all, rather than slipping through as locally-correct observations whose contracts ratify on local merits.

## Follow-ups

The design's last acceptance criterion (T045 § Done When) is "at least one follow-up implementation task can be ratified from the design." Two ratifiable observations have been filed against this doc; both await U1 ratification before they auto-promote to tasks.

- **L142 — Implement intake_items store + gatekeeper subscriber (P1 of T045 design)** — `tier_hint: T3`. Schema-codify the `intake_items` typed store and ship the gatekeeper router agent so locally-filed friction is dedup/risk-classed/fast-tracked/escalated before becoming observations. Scope is drawn directly from § *Lifecycle*, § *Schema*, § *Routing decisions*, § *Gatekeeper output schema*, and § *Recon agent contract* of this doc.
- **L143 — Add `risk_class` + `approval_policy` fields to observations schema** — `tier_hint: T3`. Promote the `(size_tier, risk_class, approval_policy)` triple from `docs/risk-and-cluster-taxonomy.md` into typed columns on the `observations` row so risk and policy are queryable and enforceable, not prose. Co-ratifies with L142 — the gatekeeper writes these columns; without them it has nowhere to land its decision blob.

Both observations cite `docs/gatekeeper-design.md`, `docs/risk-and-cluster-taxonomy.md`, and `docs/architecture-coherence.md` in their bodies. Their `task_id` field is set to `T045` so the surfacing-task linkage is preserved in the substrate's soft-FK convention.

T053/P1 shipped the Router seam only. Phase 3-5 rollout work is deferred into substrate observations cross-linked here:

- **L171 — Implement dedicated architecture_reviews typed store (P3 of T045 design)** — replaces the P1 tagged-observation stand-in for `arch_review_candidate` routing with the dedicated typed store described in § *Architecture-review outputs*.
- **L172 — Implement fast-track auto-execution + L135 Check primitive (P4 of T045 design)** — implements the deferred fast-track execution/check audit shape from § *Fast-track policy* and § *Required audit trail for every fast-track*.
- **L173 — Curated cluster_key registry + watch/observability dashboards (P5 of T045 design)** — implements registry curation and observability from § *Open questions* and § *Abuse case 2: Cluster-key collision*.
