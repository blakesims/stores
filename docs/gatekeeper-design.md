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
| `triaging`  | `routed`       | `intake route --decision <D>`         | `ai_autonomous`        | `D ∈ {duplicate, fast_track, normal_observation, arch_review_candidate}` and side-effect rows (observation / arch-review tag / merge target) created in same transaction. |
| `triaging`  | `dropped`      | `intake route --decision reject_noise`| `ai_autonomous`        | `rationale` non-empty; rejection is final unless `intake reopen` (below) fires. |
| `dropped`    | `draft`        | `intake reopen`                       | `ai_with_human`         | Human disagrees with a `reject_noise` decision; rare escape hatch.    |
| `triaging`  | `escalated`    | `intake escalate-arch-review`         | `ai_autonomous`        | Cluster threshold crossed OR `risk_flags` includes any `touches_*` flag; produces an `architecture_reviews` row (or tagged-observation stand-in until that store exists). |

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
**Precondition:** the row describes genuine friction worth filing as an `observations` row, and either no `touches_*` risk flag fires OR a flag fires but the gatekeeper judges the cluster has not yet crossed escalation threshold.
**Downstream effect:** row transitions `triaging → routed`, a new `observations` row is created (state `open`, `intent_contract` empty until ratification), and `routed_to_observation` points at it. The `cluster_key` and `risk_flags` are copied onto the observation as advisory metadata.

### 5. `arch_review_candidate`
**Precondition:** any of:
- `risk_flags` contains any `touches_*` flag (`touches_actor_authority`, `touches_lifecycle`, `touches_subscriber_semantics`, `touches_runner_boundary`, `touches_schema_core`),
- `risk_flags` contains `introduces_new_primitive` or `changes_boundary`,
- the gatekeeper observes that `cluster_key` count has crossed the architecture-review threshold (cluster threshold default: 3, configurable),
- `risk_flags` contains `security_sensitive` or `authority_surface_drift`.
**Downstream effect:** row transitions `triaging → escalated`, an `architecture_reviews` candidate row is produced (or, until that store ships, a tagged observation with reserved tag `arch-review-candidate`), and `routed_to_arch_review` points at it. A normal observation MAY also be created in parallel so local-fix work is not blocked, but its contract carries a `pending_architecture_review = true` field that prevents U1 ratification until the architecture review returns `ok_local_fix` or `reframe_contract`.

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
