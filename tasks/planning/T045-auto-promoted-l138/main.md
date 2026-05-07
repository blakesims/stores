# T045: Local observation filing lacks a gatekeeper/coherence layer; local fixes can accumulate into architectural drift

## Meta
- **Status:** plan_review
- **Created:** 2026-05-06T06:26:01Z
- **Last Updated:** 2026-05-06T06:47:37Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T045-auto-promoted-l138

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - - Define the raw intake/gatekeeper flow before mature observations
- Define gatekeeper routing decisions: duplicate, needs_info/recon, fast_track, normal_observation, architecture_review_candidate, dropped/noise
- Define risk flags and cluster keys
- Separate size tier from risk class and approval policy
- Define architecture-review triggers and outputs
- Preserve substrate doctrine: typed rows/transitions, CLI-only writes, auditability
- **Out:** - - Implementing the full architecture-review agent in the first step
- Replacing code review or plan review
- Letting sidecar chat decisions bypass typed substrate writes
- Raw SQL/manual DB cleanup
- Fast-tracking authority, lifecycle, schema, subscriber, runner, deploy, security, or approval-token changes

### Done When
Design an intake/gatekeeper and architecture-review routing layer so local observations are deduplicated, risk-classed, evidence-checked, fast-tracked when safe, or escalated for coherence review before local fixes accumulate into architectural drift.

Acceptance:
- - A documented design names the intake buffer/gatekeeper lifecycle and routing schema
- Risk flags and cluster keys are first-class enough for future automation
- The design specifies when architecture review fires and when fast-track is allowed/prohibited
- The design reconciles observation filing with the philosophy that local correctness is not architectural coherence
- At least one follow-up implementation task can be ratified from the design

### Phases

#### Phase 1: Phase 1: Doctrine note — local correctness is not architectural coherence
- **Objective:** Promote the doctrine that local-fix correctness is orthogonal to architectural coherence into a durable doc, grounding every later phase.
- **Tasks:**
  - Task 1.1: Create docs/architecture-coherence.md stating the doctrine, citing the three drift examples from worklog 06 (T1 plan&#x3D;null, dispatch lifecycle, sidecar token propagation)
  - Task 1.2: Add a one-line pointer in CLAUDE.md under &#x27;Pointers&#x27; linking docs/architecture-coherence.md
  - Task 1.3: Cross-link from docs/philosophy.md § &#x27;What&#x27;s outside the substrate&#x27; to the new doctrine doc
- **Acceptance Criteria:**
  - [ ] AC1.1: docs/architecture-coherence.md exists and contains a &#x27;Doctrine&#x27; section with the verbatim claim &#x27;local correctness is not architectural coherence&#x27; and at least three concrete drift examples
  - [ ] AC1.2: CLAUDE.md &#x27;Pointers&#x27; section references docs/architecture-coherence.md
  - [ ] AC1.3: docs/philosophy.md contains a forward-link to docs/architecture-coherence.md
  - [ ] AC1.4: &#x60;git grep -l architecture-coherence&#x60; returns at least 3 files
- **Files:** `docs/architecture-coherence.md`, `CLAUDE.md`, `docs/philosophy.md`
#### Phase 2: Phase 2: Intake/gatekeeper lifecycle + routing schema design
- **Objective:** Specify the &#x60;intake_items&#x60; buffer, its lifecycle states, transitions, fields, and the gatekeeper&#x27;s structured routing decisions in a design doc that an executor could later schema-codify.
- **Tasks:**
  - Task 2.1: Create docs/gatekeeper-design.md with sections: Purpose, Lifecycle, Fields, Routing decisions, Gatekeeper output schema, Recon agent contract, Open questions
  - Task 2.2: Define the lifecycle state machine &#x60;draft → triaging → needs_info → routed | dropped&#x60; (or revised after analysis) with transition verbs, actor classes (ai_autonomous vs ai_with_human vs human), and guards
  - Task 2.3: Enumerate the six routing decisions (duplicate, needs_info, fast_track, normal_observation, arch_review_candidate, reject_noise) with preconditions and downstream effect on observations/tasks
  - Task 2.4: Specify the gatekeeper structured-output JSON schema (decision, confidence, tier_hint, risk_flags[], duplicate_candidates[], cluster_key, missing_info_question, recommended_next, rationale) — tightening field types and enums vs. the worklog draft
  - Task 2.5: Specify the recon agent&#x27;s narrow brief contract (gather evidence, do NOT design solutions; returns to gatekeeper for re-routing)
  - Task 2.6: Reconcile design with substrate doctrine: typed rows, CLI-only writes, auditability, ai_autonomous default, no sidecar bypass — explicit subsection
- **Acceptance Criteria:**
  - [ ] AC2.1: docs/gatekeeper-design.md exists with all seven required sections present as level-2 headers
  - [ ] AC2.2: Lifecycle section lists every state, every transition with verb + actor + guard, in a table or schema-yaml-shaped block
  - [ ] AC2.3: Routing decisions section names exactly six decisions with one paragraph each on preconditions and downstream effect
  - [ ] AC2.4: Gatekeeper output schema is presented as a typed JSON Schema (or schema.yaml-style enum block), not free-form JSON
  - [ ] AC2.5: &#x27;Substrate doctrine reconciliation&#x27; subsection explicitly addresses typed rows, CLI-only writes, --invoker discipline, and confirms no sidecar bypass
- **Files:** `docs/gatekeeper-design.md`
- **Dependencies:** Phase 1 doctrine doc exists so this design can cite it
#### Phase 3: Phase 3: Risk flags, cluster keys, and tier/risk/policy separation spec
- **Objective:** Lift risk_flags, cluster_key, and the orthogonal (size_tier, risk_class, approval_policy) triple to first-class status with a normative enumeration that future schema work can implement directly.
- **Tasks:**
  - Task 3.1: Create docs/risk-and-cluster-taxonomy.md (or top-level section in gatekeeper-design.md — pick in decision matrix) enumerating the canonical risk_flags with definitions and example triggers
  - Task 3.2: Specify cluster_key conventions: kebab-case namespace (e.g. &#x60;t1-null-plan&#x60;, &#x60;dispatch-lifecycle&#x60;, &#x60;sidecar-token&#x60;), threshold-fire semantics, who assigns and who merges
  - Task 3.3: Define the orthogonal triple {size_tier ∈ T0..T3, risk_class ∈ low|normal|architecture|security|authority, approval_policy ∈ auto|human|architecture} and the matrix of legal combinations
  - Task 3.4: Write a worked-example table mapping ~6 hypothetical observations to (tier, risk_class, policy, gatekeeper decision) so executors and reviewers can sanity-check
- **Acceptance Criteria:**
  - [ ] AC3.1: Risk_flags enumeration lists at minimum the nine flags from worklog 06 with one-sentence definitions each
  - [ ] AC3.2: Tier/risk/policy section presents an explicit matrix or table of legal (tier, risk_class, policy) combinations and notes which combinations are illegal
  - [ ] AC3.3: At least 6 worked examples are present, each mapping observation → (tier, risk_class, approval_policy, gatekeeper decision)
  - [ ] AC3.4: Cluster_key conventions specify naming, who assigns, threshold semantics for arch-review escalation
- **Files:** `docs/risk-and-cluster-taxonomy.md`, `docs/gatekeeper-design.md`
- **Dependencies:** Phase 2 gatekeeper design names the slots these taxonomies fill
#### Phase 4: Phase 4: Architecture-review triggers + fast-track allow/prohibit policy
- **Objective:** Specify when the architecture-review agent fires, its outputs, and the auditable boundary of fast-track — closing the design&#x27;s normative core.
- **Tasks:**
  - Task 4.1: Add &#x27;Architecture-review triggers&#x27; section to docs/gatekeeper-design.md naming the five trigger classes (risk-flag, cluster-threshold, pre-ratification, periodic-sweep, post-accept-batch) with quantitative thresholds where decidable
  - Task 4.2: Enumerate architecture-review outputs (allow_local_fix, reframe_contract, merge_with_cluster, create_primitive_task, block_pending_fixes, propose_doctrine_update, request_human_arch_decision) with one-paragraph definitions each
  - Task 4.3: Add &#x27;Fast-track policy&#x27; section with explicit ALLOW list and PROHIBIT list (verbatim from scope: never fast-track authority/lifecycle/schema/subscriber/runner/deploy/security/approval-token), and the audit trail required for every fast-track (gatekeeper decision row, deterministic check, terminal closure)
  - Task 4.4: Add a &#x27;Failure modes / abuse cases&#x27; subsection naming at least three ways the gatekeeper itself could drift (e.g., risk-flag underuse, cluster-key collision, fast-track creep) and proposed counter-measures
- **Acceptance Criteria:**
  - [ ] AC4.1: Architecture-review triggers section names all five trigger classes with concrete thresholds (numeric where applicable)
  - [ ] AC4.2: Architecture-review outputs section enumerates all seven outputs with definitions
  - [ ] AC4.3: Fast-track section&#x27;s PROHIBIT list explicitly names every item from scope_out: authority, lifecycle, schema, subscriber, runner, deploy, security, approval-token
  - [ ] AC4.4: Failure-modes subsection lists ≥3 abuse cases with counter-measures
- **Files:** `docs/gatekeeper-design.md`
- **Dependencies:** Phase 3 risk taxonomy exists so triggers can reference flags by name
#### Phase 5: Phase 5: Follow-up observation(s) and ratifiable task seed
- **Objective:** Convert the design into at least one substrate-visible, ratifiable follow-up — proving the design is actionable and satisfying the contract&#x27;s last acceptance criterion.
- **Tasks:**
  - Task 5.1: File observation via &#x60;stores observations add --invoker ai_autonomous&#x60; for &#x27;Implement intake_items store + gatekeeper subscriber (P1 of T045 design)&#x27; with intent_contract.tier_hint&#x3D;T3, scope_in/scope_out drawn directly from docs/gatekeeper-design.md, and body referencing the design doc by path
  - Task 5.2: File a second observation for &#x27;Add risk_class + approval_policy fields to observations schema&#x27; (tier_hint T2 or T3 — set by analysis) referencing docs/risk-and-cluster-taxonomy.md
  - Task 5.3: Add a &#x27;Follow-ups&#x27; section to docs/gatekeeper-design.md cross-linking the L### IDs of the filed observations so the design points forward to its implementation seeds
  - Task 5.4: Halt for U1 ratification rather than self-approving — surface the filed L### IDs to the user with a one-line proposal for each
- **Acceptance Criteria:**
  - [ ] AC5.1: At least one new observation row exists referencing T045 in &#x60;task_id&#x60; and the design doc paths in its body (verifiable via &#x60;stores observations list&#x60;)
  - [ ] AC5.2: docs/gatekeeper-design.md &#x27;Follow-ups&#x27; section lists the filed observation IDs as L### references
  - [ ] AC5.3: Executor surfaces the filed observation IDs to the user with proposed contracts for U1 ratification — does NOT self-approve via ai_with_human
  - [ ] AC5.4: At least one filed observation has a complete intent_contract (done_when, scope_in, scope_out, tier_hint) sufficient for U1 ratification without further fields
- **Files:** `docs/gatekeeper-design.md`
- **Dependencies:** Phases 2-4 produce the design content the observations cite

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

