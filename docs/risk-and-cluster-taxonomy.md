# Risk Flags, Cluster Keys, and the Tier/Risk/Policy Triple

**Path:** `docs/risk-and-cluster-taxonomy.md`
**Status:** taxonomy spec (T045 phase 3). Pre-implementation; an executor codifying schema in a follow-up task should be able to lift these enums and matrices into `schema.yaml` directly.
**Companion docs:** `docs/architecture-coherence.md` (doctrine), `docs/gatekeeper-design.md` (lifecycle and routing), `docs/worklog/2026-05-06/06-gatekeeper-architecture-observability.md` (seed brainstorm).

## Decision matrix: standalone vs. inline

This taxonomy lives in its own file rather than as a section of `docs/gatekeeper-design.md` because:

1. The risk-flag enum and the (size_tier, risk_class, approval_policy) matrix are referenced from both the gatekeeper design and the architecture-coherence doctrine; a standalone reference avoids duplicating enum definitions across two docs.
2. Future schema work will lift the flags into `schema.yaml` enums; one canonical reference simplifies that mechanical promotion.
3. Worked examples are inherently long; embedding them inside `gatekeeper-design.md` would dilute the lifecycle/routing focus of that doc.

A short pointer remains in `docs/gatekeeper-design.md` so readers arriving via the lifecycle doc are routed here.

## 1. Risk-flag enumeration

The gatekeeper attaches zero or more `risk_flags` to every triaged intake row. Flags are typed enums; the gatekeeper output schema (in `gatekeeper-design.md` § *Gatekeeper output schema*) validates them. The nine baseline flags from worklog 06 are listed first; four extension flags promoted during phase 2 follow.

### Baseline (worklog 06)

| Flag | One-sentence definition | Example trigger |
|------|-------------------------|-----------------|
| `touches_actor_authority` | The change alters which actor class (`human` / `ai_with_human` / `ai_autonomous`) may write a field or fire a transition. | A proposal to relax `tasks.accept` from `actor: human` to `actor: ai_with_human` to "speed up acceptance." |
| `touches_lifecycle` | The change adds, removes, or rewires a state in a typed store's state machine, or alters the guards on an existing transition. | Adding a `paused` state to `tasks` so drive can be halted; rewiring `planning → ready` to skip plan-reviewer for T1 cases. |
| `touches_subscriber_semantics` | The change alters when a substrate subscriber fires, what it consumes, or what side-effects it produces. | Reordering the auto-promote subscriber to fire before contract ratification is final; adding a new fire condition to L046. |
| `introduces_new_primitive` | The change proposes a new typed row, a new namespace prefix (e.g. `I###`, `R###`), or a new first-class concept the substrate has not previously named. | Filing an observation that proposes an `intake_items` store; introducing an `architecture_reviews` row; coining a `dispatch_attempts` primitive. |
| `changes_boundary` | The change moves the line between substrate-internal and substrate-external (CLI verbs, on-disk artifacts, projection rules, wrapper contract). | Letting a hook inside the substrate write directly to `tasks/<id>/main.md`; exposing a new privileged read endpoint to the orchestrator. |
| `security_sensitive` | The change touches secrets handling, the approval-token mechanism, sandbox boundaries, or any path where leakage / fabrication would compromise the threat model. | Caching the decrypted approval token in the daemon process; adding a `--debug-print-token` flag for diagnostics. |
| `docs_only` | The change is restricted to documentation, doctrine, or comment text and cannot affect runtime behavior. | Fixing a typo in `docs/philosophy.md`; clarifying a `CLAUDE.md` paragraph; adding a worklog cross-reference. |
| `small_local_fix` | The change is narrow (≤5 files / ≤200 LOC), touches no `touches_*` surface, and is mechanically verifiable by tests. | Renaming a misspelled local variable; tightening an error message; bumping a literal in a CLI output template. |
| `duplicate_symptom` | The intake row reports a symptom of a known-routed observation or a fully-resolved root cause; nothing new is being surfaced. | A second filing of "drive prints the same warning twice" against an already-resolved L###; a re-report of stale-PID after L116 shipped. |

### Phase 2 extensions

| Flag | One-sentence definition | Example trigger |
|------|-------------------------|-----------------|
| `touches_runner_boundary` | The change alters how the runner (Mac / Pi / sidecar) interacts with the substrate, including dispatch-lock semantics, sidecar protocols, or cross-host claims. | Letting the Pi runner skip sidecar handshake; adding a runner-only field to `dispatch_locks`. |
| `touches_schema_core` | The change modifies the schema's invariant scaffolding: `actor:` discipline, transition-history machinery, validators that all stores depend on, or the rendered-projection contract. | Generalizing the transition-history hook to skip rows above a size threshold; adding an opt-out flag for validators. |
| `authority_surface_drift` | The change widens, lengthens, or duplicates the surface on which approval-tokens or human-authority signals live, even when each step is locally convenient. | Pre-fetching the approval token into a subagent brief; persisting `--approve-token` values in dispatch metadata. |
| `contradicts_prior_decision` | The intake row's `suggested_fix` directly contradicts a previously ratified observation contract or accepted task outcome without naming the contradiction. | A filing that proposes `plan = null` handling after T1 contract-is-plan was already adopted as the canonical shape. |

The first nine columns of any future `risk_flags` enum table MUST be the baseline. Extensions append; they do not replace.

## 2. Cluster-key conventions

`cluster_key` is a short, kebab-case label that names the *underlying shape* a filing belongs to, distinct from the filing's surface symptoms. It is the index the gatekeeper and architecture-review layers count over.

### Naming

- **Format:** kebab-case, ASCII lowercase letters / digits / hyphens, length 3–40 characters. Pattern: `^[a-z][a-z0-9-]{2,40}$` (matches the gatekeeper output schema).
- **Namespace shape:** `<area>-<subject>` or `<area>-<subject>-<qualifier>`. Examples: `t1-null-plan`, `dispatch-lifecycle`, `sidecar-token`, `intake-routing`, `subscriber-fire-order`, `auth-token-leak`.
- **Style:** name the *missing abstraction* or *drifted surface*, not the surface symptom. `t1-null-plan` (abstraction) is preferred to `submit-plan-crashes-on-null` (symptom). The cluster key is a label for the architectural concern; the row's `summary` carries the symptom.
- **Stability:** once assigned to a routed row, a cluster key is immutable. A row whose cluster key was wrong is corrected by escalation (`arch_review_candidate` reframes), not by mutation.

### Who assigns

- **The gatekeeper assigns `cluster_key`** as part of its triage decision, on every `normal_observation`, `arch_review_candidate`, and `duplicate` decision. (Fast-track decisions also accept a `cluster_key`, but it is optional because fast-tracked items by definition do not fire cluster-threshold escalation.)
- **The filer (raw intake author) does NOT assign `cluster_key`.** Raw intake rows have no cluster key; the gatekeeper synthesizes it after triage. This prevents local agents from coining cluster keys ad-hoc.

### Who merges

- **The gatekeeper merges clusters.** When a new filing's `cluster_key` matches an existing key, the new row's `duplicate_candidates[]` may include rows from that cluster, and the gatekeeper's decision (merge into existing observation as `duplicate`, file a fresh `normal_observation` in the same cluster, or escalate the now-overflowing cluster) is the merge act.
- **The architecture-review agent renames or splits clusters.** If architecture-review judges that two cluster keys are actually one underlying concern (or that one cluster key conflates two), it emits a rename / split directive. The gatekeeper applies it on subsequent triages; existing routed rows are not retroactively edited (consistent with `routed`-is-terminal in `gatekeeper-design.md` § *Open questions* item 6).

### Threshold-fire semantics

- **Default threshold:** `cluster_count ≥ 3` triggers `arch_review_candidate` eligibility for the *next* filing in the cluster (i.e. the third filing escalates). The threshold is per-cluster-key configurable; the default is a starting point, not a doctrine.
- **What counts:** cluster_count is the number of `routed` (or `escalated`) intake rows whose `cluster_key` equals the candidate key. `dropped` rows do not count. `duplicate` rows DO count (they incremented the cluster as their reason for existing).
- **First-fire vs. backlog:** threshold checks fire forward only. If the threshold default drops from 5 to 3, prior clusters at count 4 are not retroactively escalated; the next filing into them is.
- **Curated vs. organic registry:** cluster keys may be coined freely by the gatekeeper today (organic). When a key has been used ≥3 times, it promotes into a curated registry (a config file the gatekeeper reads on startup). Promoted keys gain (a) a canonical definition, (b) a tunable threshold, (c) a pointer to the architecture concern they name. Promotion is a phase-5 implementation detail; the convention is named here so the executor can scaffold the registry slot.

## 3. The orthogonal triple: (size_tier, risk_class, approval_policy)

Worklog 06 § *Tier is not risk* observed that size and risk are different dimensions; phase 1's coherence doctrine grounds the same point. This section names the triple, enumerates legal values, and lists the legal combinations.

### Field definitions

```
size_tier        ∈ {T0, T1, T2, T3}
risk_class       ∈ {low, normal, architecture, security, authority}
approval_policy  ∈ {auto, human, architecture}
```

- **`size_tier`** is the existing tier_hint enum from observations and intake. T0 = doctrinal-only, T1 = contract-is-plan, T2 = one-phase plan, T3 = full multi-phase. Reused unchanged.
- **`risk_class`** is the architectural-coherence dimension. `low` = cosmetic / cannot affect coherence. `normal` = ordinary substrate work, no `touches_*` flags fired. `architecture` = any `touches_*` (lifecycle, subscriber, runner boundary, schema core), or `introduces_new_primitive`, or `changes_boundary`. `security` = `security_sensitive` flag fired. `authority` = `touches_actor_authority` or `authority_surface_drift` fired. (Authority is split out from architecture because it has its own escalation path: tier-A approval-token gates and a stricter review focus.)
- **`approval_policy`** is the routing decision the triple implies. `auto` = gatekeeper may fast-track without human or architecture gate. `human` = ordinary U1 ratification by the human owner of the contract. `architecture` = must pass architecture-review BEFORE U1 ratification can fire (the observation's contract carries `pending_architecture_review = true`).

### Legal combinations matrix

Read down: each (size_tier × risk_class) cell shows the *only* legal `approval_policy` value(s). Cells marked `—` are illegal — the substrate's schema validators must reject any intake-decision payload that produces an illegal combination.

| ↓ tier / risk → | low | normal | architecture | security | authority |
|-----------------|-----|--------|--------------|----------|-----------|
| **T0** (doctrine) | `auto` | `auto` | `architecture` | `architecture` | `architecture` |
| **T1** (contract-is-plan) | `auto` | `human` | `architecture` | `architecture` | `architecture` |
| **T2** (one-phase) | `auto`† | `human` | `architecture` | `architecture` | `architecture` |
| **T3** (multi-phase) | — | `human` | `architecture` | `architecture` | `architecture` |

† T2 + low is rare but legal: a one-phase plan whose only work is e.g. snapshot regen across many files. It auto-fast-tracks if the gatekeeper can mechanically verify "no behavior change."

### Illegal combinations (and why)

- **T3 + low.** A multi-phase plan whose risk is `low` is incoherent: by definition, work that decomposes into multiple phases touches enough surface to escape the cosmetic / no-behavior-change bound. If a T3 plan's pieces are each individually low-risk, the *aggregate* is at least `normal`. Reject and re-classify.
- **`architecture` / `security` / `authority` with `approval_policy = auto`.** Forbidden in every tier. Fast-track exists only for changes that cannot affect coherence; any of these three risk classes by definition can. This is the doctrine of `gatekeeper-design.md` § *Routing decisions* item 3 (`fast_track`) made explicit as a matrix constraint.
- **`architecture` / `security` / `authority` with `approval_policy = human` (only).** Forbidden. The human ratifying the contract is not a substitute for architecture review; the doctrine of `architecture-coherence.md` is that contract-ratifiability does not imply coherence. Any of these three risk classes routes through `architecture` first; `human` U1 fires after the architecture-review verdict allows it.
- **`low` with `approval_policy = architecture`.** Forbidden. Routing low-risk work to architecture review pollutes the coherence queue; the threshold-fire semantics already handle the "low-but-cluster-overflowing" edge by escalating the cluster, not the individual filing.

### How the triple is computed from gatekeeper output

The gatekeeper does not emit `risk_class` and `approval_policy` directly; they are *derived* from `risk_flags` + `tier_hint` by a deterministic function the substrate runs:

```
def derive_risk_class(risk_flags) -> risk_class:
    if "touches_actor_authority" in risk_flags or "authority_surface_drift" in risk_flags:
        return "authority"
    if "security_sensitive" in risk_flags:
        return "security"
    if any(f.startswith("touches_") for f in risk_flags) \
       or "introduces_new_primitive" in risk_flags \
       or "changes_boundary" in risk_flags \
       or "contradicts_prior_decision" in risk_flags:
        return "architecture"
    if risk_flags <= {"docs_only", "small_local_fix", "duplicate_symptom"}:
        return "low"
    return "normal"

def derive_approval_policy(size_tier, risk_class, gatekeeper_decision) -> approval_policy:
    if gatekeeper_decision == "fast_track":
        return "auto"
    if risk_class in {"architecture", "security", "authority"}:
        return "architecture"
    if size_tier == "T0":
        return "auto"
    return "human"
```

The derivation is part of the substrate's validators; the gatekeeper agent only chooses `risk_flags`, `tier_hint`, and `decision`. This keeps the gatekeeper's output surface narrow and the matrix's enforcement mechanical.

## 4. Worked examples

Six hypothetical observations, each mapped end-to-end. Examples are constructed to span the matrix; reviewers should cross-check that every legal cell in the matrix is exercised by at least one row.

| # | Filing summary | size_tier | risk_flags | risk_class | approval_policy | gatekeeper decision |
|---|---------------|-----------|------------|------------|-----------------|---------------------|
| 1 | "Typo in `docs/philosophy.md`: 'sustrate' → 'substrate'." | T0 | `docs_only` | low | auto | `fast_track` |
| 2 | "CLI `tasks status` prints `(none)` instead of `—` for empty deps." | T1 | `small_local_fix` | low | auto | `fast_track` |
| 3 | "T1 drive crashes when contract-is-plan because `submit-plan` checks `plan != null`." | T2 | `touches_lifecycle`, `contradicts_prior_decision` | architecture | architecture | `arch_review_candidate` (cluster `t1-null-plan`) |
| 4 | "Sidecar should pre-fetch the approval token at session start so subagents inherit it." | T2 | `touches_actor_authority`, `authority_surface_drift`, `security_sensitive` | authority | architecture | `arch_review_candidate` (cluster `sidecar-token`) |
| 5 | "Add `paused` state to `tasks` so drive can be halted mid-cycle." | T3 | `touches_lifecycle`, `touches_schema_core`, `changes_boundary` | architecture | architecture | `arch_review_candidate` (cluster `dispatch-lifecycle`; threshold may already be crossed) |
| 6 | "Stale dispatch-lock PIDs are not cleaned when the runner crashes mid-claim." | T2 | (none → derived) | normal | human | `normal_observation` (cluster `dispatch-lifecycle`; cluster_count = 1, no escalation yet) |

Notes on the table:

- Row 3 illustrates `contradicts_prior_decision`: the suggested fix re-introduces `plan = null`, which the T1 contract-is-plan adoption already ruled against. The architecture-review verdict will likely be `reframe_contract` (project the contract into the plan slot, do not null-check).
- Rows 5 and 6 share `cluster_key = dispatch-lifecycle`. Row 6 is the third filing into that cluster in the worked-example narrative; the gatekeeper would therefore fire cluster-threshold escalation on the *next* filing, not on row 6 itself (forward-only semantics). If row 5 followed row 6 in time, row 5's escalation is firing both on its `risk_flags` AND on the threshold — both triggers are independently sufficient.
- Row 4 demonstrates the `authority` risk class: `touches_actor_authority` AND `authority_surface_drift` AND `security_sensitive` all fire; `authority` wins the precedence order in `derive_risk_class` because it is checked first. The `approval_policy = architecture` constraint forces architecture review even though the human is presumably willing to ratify "for convenience."
- Row 1 and row 2 are the only fast-tracks. The gatekeeper still records the decision blob, the cluster (if any), and the audited fast-track artifact per `gatekeeper-design.md` § *Open questions* item 3.
- No row maps to `(T3, low, auto)` or `(architecture, ..., auto)` — those cells are illegal per the matrix.

## Pointers

- Lifecycle and routing decisions: `docs/gatekeeper-design.md` (T045 phase 2).
- Doctrine grounding the triple: `docs/architecture-coherence.md` (T045 phase 1).
- Seed brainstorm and original nine flags: `docs/worklog/2026-05-06/06-gatekeeper-architecture-observability.md`.
- Schema patterns the executor should mimic when promoting these enums into `schema.yaml`: `observations` and `tasks` enum/validator definitions.
