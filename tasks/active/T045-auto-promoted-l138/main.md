# T045: Local observation filing lacks a gatekeeper/coherence layer; local fixes can accumulate into architectural drift

## Meta
- **Status:** in_review
- **Created:** 2026-05-06T06:26:01Z
- **Last Updated:** 2026-05-06T07:09:29Z
- **Current Phase:** 5
- **Current Cycle:** 1
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

### Review 1
- **Gate:** READY
- **Summary:** Design-only plan with five well-scoped phases; every AC is mechanically verifiable (file existence, header presence, enumerated counts, git grep). Decision matrix covers the consequential choices (single vs. split docs, design-only vs. schema, observation-first follow-up, halt-for-U1). Done-when is fully traceable: doctrine doc (Phase 1) + lifecycle/routing (Phase 2) + risk taxonomy (Phase 3) + triggers/fast-track (Phase 4) + ratifiable follow-up observation (Phase 5) maps 1:1 onto the five contract acceptance bullets.
- **At:** 2026-05-06T06:48:10Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 1 done. Created docs/architecture-coherence.md as the doctrine doc grounding T045 — &#x27;local correctness is not architectural coherence&#x27; — with three drift examples (T1 plan&#x3D;null, dispatch lifecycle, sidecar token propagation) sourced from worklog 2026-05-06/06. Added a Pointers entry in CLAUDE.md and a forward-link from docs/philosophy.md § &#x27;What\&#x27;s outside the substrate&#x27;. All 4 ACs verified mechanically (git grep returns 3 files: CLAUDE.md, docs/architecture-coherence.md, docs/philosophy.md).
- **Commit:** `f6e69b9c98c98795933204f60f70572f25a8c2d5`
- **Files:**
  - `docs/architecture-coherence.md`
  - `CLAUDE.md`
  - `docs/philosophy.md`
- **At:** 2026-05-06T06:50:59Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Created docs/gatekeeper-design.md with all seven required level-2 sections (Purpose, Lifecycle, Fields, Routing decisions, Gatekeeper output schema, Recon agent contract, Open questions). Lifecycle is a table of every transition with verb + actor + guard. Routing names exactly six decisions (duplicate, needs_info, fast_track, normal_observation, arch_review_candidate, reject_noise) with preconditions and downstream effects. Output schema presented as typed schema.yaml-style enums (no free-form JSON). Substrate-doctrine reconciliation subsection explicitly addresses typed rows, CLI-only writes, --invoker discipline, and confirms no sidecar bypass.
- **Commit:** `405cf9b`
- **Files:**
  - `docs/gatekeeper-design.md`
- **At:** 2026-05-06T06:54:35Z
### Phase 3 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 3 complete. Created docs/risk-and-cluster-taxonomy.md as the canonical reference (decision matrix in §0 justifies standalone vs. inline). AC3.1: nine baseline risk_flags from worklog 06 enumerated with one-sentence definitions and example triggers, plus four phase-2 extension flags. AC3.2: orthogonal (size_tier ∈ T0..T3, risk_class ∈ low|normal|architecture|security|authority, approval_policy ∈ auto|human|architecture) triple defined with explicit 4×5 legality matrix; illegal combinations (T3+low, architecture/security/authority + auto, those three + human-only, low + architecture) named with rationale. AC3.3: six worked examples spanning fast_track, normal_observation, and arch_review_candidate decisions with full (tier, flags, risk_class, policy, decision) mapping. AC3.4: cluster_key conventions specify kebab-case &#x60;^[a-z][a-z0-9-]{2,40}$&#x60; pattern, gatekeeper-only assignment, gatekeeper-merges / arch-review-renames split, default ≥3 forward-only threshold semantics, and organic-then-curated registry promotion. Cross-link added to gatekeeper-design.md frontmatter.
- **Commit:** `930155b7960489a354b7050e11d743638882a9f5`
- **Files:**
  - `docs/risk-and-cluster-taxonomy.md`
  - `docs/gatekeeper-design.md`
- **At:** 2026-05-06T06:58:26Z
### Phase 4 / Cycle 1
- **Status:** PASS
- **Summary:** Added Phase 4 normative sections to docs/gatekeeper-design.md. Architecture-review triggers: all five classes (risk-flag, cluster-threshold default ≥3 with ≥2 override for architectural-priors clusters, pre-ratification, periodic-sweep ≥5/90d, post-accept-batch ≥3 tasks/14d with ≥2 cluster overlap) with concrete numeric thresholds. Architecture-review outputs: all seven typed outcomes (allow_local_fix, reframe_contract, merge_with_cluster, create_primitive_task, block_pending_fixes, propose_doctrine_update, request_human_arch_decision) with one-paragraph definitions. Fast-track policy: ALLOW list (6 cumulative conditions) and PROHIBIT list naming all eight scope_out items verbatim (authority, lifecycle, schema, subscriber, runner, deploy, security, approval-token), plus three-artifact audit trail (decision row, deterministic check record, terminal closure). Failure modes: three abuse cases (risk-flag underuse, cluster-key collision, fast-track creep) each with three counter-measures.
- **Commit:** `4b5b0afe6084801e0da4bcee1c1b85fcdd3957a0`
- **Files:**
  - `docs/gatekeeper-design.md`
- **At:** 2026-05-06T07:01:52Z
### Phase 5 / Cycle 1
- **Status:** PASS
- **Summary:** T045 P5 complete. Filed L142 (intake_items + gatekeeper subscriber, T3) and L143 (risk_class + approval_policy schema columns, T3) via stores observations add --invoker ai_autonomous, both with task_id&#x3D;T045, full intent_contract (objective/in-scope/out-of-scope/acceptance/tier_hint), and bodies referencing docs/gatekeeper-design.md, docs/risk-and-cluster-taxonomy.md, docs/architecture-coherence.md. Added Follow-ups section to docs/gatekeeper-design.md cross-linking L142 and L143. Did NOT self-approve via ai_with_human — surfaced both to user for U1 ratification per AC5.3.
- **Commit:** `c8b04c6`
- **Files:**
  - `docs/gatekeeper-design.md`
- **At:** 2026-05-06T07:05:55Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified: docs/architecture-coherence.md exists with a &#x27;Doctrine&#x27; section containing the verbatim claim &#x27;local correctness is not architectural coherence&#x27; and three concrete drift examples (T1 plan&#x3D;null, dispatch lifecycle, sidecar token propagation); CLAUDE.md Pointers references the new doc (line 160); docs/philosophy.md § &#x27;What&#x27;s outside the substrate&#x27; adds a forward-link; &#x60;git grep -l architecture-coherence&#x60; returns exactly 3 files (CLAUDE.md, docs/architecture-coherence.md, docs/philosophy.md). Commit f6e69b9 matches the executor&#x27;s claim. Diff is small (45 lines), so two minor findings are sufficient.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[MINOR] AC1.4 satisfied at the floor.
File: docs/architecture-coherence.md (self-reference)
Evidence: &#x60;git grep -l architecture-coherence&#x60; returns 3 files; one is the doc itself, so only 2 are external references. AC1.4 says &#x27;at least 3&#x27; so it passes, but the self-reference inflates the count by one.
Suggestion: Non-blocking. If a later phase adds the doc to &#x60;docs/CLAUDE.md&#x60; § References (which currently states &#x27;Promote here when a worklog insight becomes something future-you... will want to revisit&#x27;), the count grows to 4 with two external referrers, which is more durable.

[MINOR] Doctrine doc has task-phase-coupled status line.
File: docs/architecture-coherence.md:4
Evidence: Line 4 reads &#x60;**Status:** doctrine doc (T045 phase 1).&#x60;
Expected: Doctrine docs typically outlive the task that births them. Pinning the status to &#x27;T045 phase 1&#x27; will rot the moment T045 closes.
Suggestion: Drop the parenthetical, or change to &#x60;**Status:** doctrine&#x60; once T045 ships. Non-blocking for this phase.

[INFORMATIONAL] Phase 1 is a docs-only phase; no test surface exists. cargo test was not run because no Rust source changed (verified via &#x60;git show --stat f6e69b9&#x60;). The &#x27;expect 3+ findings on non-trivial changes&#x27; baseline is relaxed here — this is a 45-line, 3-file documentation diff.
- **At:** 2026-05-06T06:51:46Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 5 ACs verified: 7 level-2 headers present (Purpose, Lifecycle, Fields, Routing decisions, Gatekeeper output schema, Recon agent contract, Open questions); lifecycle table covers every transition with verb+actor+guard (8 rows); exactly 6 routing decisions with Precondition/Downstream-effect paragraphs; output schema is schema.yaml-style enums (no free-form JSON); doctrine reconciliation explicitly addresses typed rows, CLI-only writes, --invoker discipline, and no-sidecar-bypass. 0 critical, 0 major, 3 minor (placement nit + state-count consistency + non-standard schema keyword).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] &#x27;Substrate doctrine reconciliation&#x27; is placed as a level-3 subsection under &#x27;## Open questions&#x27;.
File: docs/gatekeeper-design.md:282 (### Substrate doctrine reconciliation appears immediately after the Open questions enumerated list)
Evidence: line 270 starts &#x27;## Open questions&#x27;; line 282 is &#x27;### Substrate doctrine reconciliation&#x27; with no intervening &#x27;##&#x27;.
Expected: AC2.5 is satisfied since &#x27;subsection&#x27; was the requirement, but semantically reconciliation is not an open question — it belongs as its own level-2 section or under a top-level &#x27;Doctrine&#x27; heading.
Suggestion: Promote line 282 to &#x27;## Substrate doctrine reconciliation&#x27; (level-2) so it is no longer nested inside Open questions.

[MINOR] State-count claim is inconsistent with the state enum.
File: docs/gatekeeper-design.md:26 vs 39-46 vs 62
Evidence: Line 26 says &#x27;rows move through five states&#x27;, but the state enum at line 62 lists six values [draft, triaging, needs_info, routed, escalated, dropped] and the lifecycle table treats &#x60;escalated&#x60; as a distinct destination (line 37). Line 45 calls escalated a &#x27;synonym sub-state of routed&#x27; — but the schema models it as its own enum value, not a sub-state.
Expected: Either say &#x27;six states&#x27; and drop the sub-state framing, or remove &#x60;escalated&#x60; from the enum and represent it as a flag/marker on a &#x60;routed&#x60; row.
Suggestion: Update line 26 to &#x27;six states&#x27; for consistency with the enum, or alternatively replace &#x60;escalated&#x60; in the enum with a boolean &#x60;is_arch_escalated&#x60; field on routed rows.

[MINOR] Output-schema uses non-standard &#x60;required_when&#x60; keyword without naming the dialect.
File: docs/gatekeeper-design.md:171-240
Evidence: Lines 195, 224, 228, 232 use &#x60;required_when: &quot;&lt;predicate&gt;&quot;&#x60; and line 224 uses &#x60;min_items_when_required&#x60; — neither exists in JSON Schema draft-2020-12.
Expected: AC2.4 is satisfied because the doc explicitly frames it as &#x27;schema.yaml-style&#x27;, but a downstream executor codifying this will hit an ambiguity.
Suggestion: Add a one-line note under the code block clarifying these are schema.yaml-conditional-validator extensions (or rewrite to JSON Schema&#x27;s &#x60;allOf&#x60;/&#x60;if&#x60;/&#x60;then&#x60; shape) so the executor in phase 5 doesn&#x27;t have to guess the dialect.

[INFORMATIONAL] Six-decision count verified: duplicate (line 139), needs_info (143), fast_track (147), normal_observation (152), arch_review_candidate (155), reject_noise (163). Each has a Precondition and a Downstream effect paragraph as required by AC2.3.
- **At:** 2026-05-06T06:55:29Z

### Phase 3 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. AC3.1 verified: nine baseline risk_flags from worklog 06 reproduced verbatim with one-sentence definitions and example triggers, plus four phase-2 extensions clearly partitioned. AC3.2: 4×5 legal-combinations matrix present with four illegal-cell rationales. AC3.3: six worked examples span legal cells and exercise fast_track / normal_observation / arch_review_candidate decisions. AC3.4: cluster_key naming pattern, gatekeeper-only assignment, gatekeeper-merge / arch-review-rename split, ≥3 forward-only threshold, and organic→curated promotion all named. Cross-link added to gatekeeper-design.md frontmatter. 0 critical, 0 major, 3 minor (subtle matrix/derivation interaction; row-3 size_tier classification; precedence note).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Matrix vs derive_approval_policy interaction is subtle.
File: docs/risk-and-cluster-taxonomy.md:94-101 vs 128-135
Evidence: The matrix shows T1+low → &#x60;auto&#x60; and T2+low → &#x60;auto&#x60;†, but derive_approval_policy returns &#x60;auto&#x60; only when (a) gatekeeper_decision &#x3D;&#x3D; &#x27;fast_track&#x27; OR (b) size_tier &#x3D;&#x3D; &#x27;T0&#x27;. So a T1+low or T2+low row that the gatekeeper does NOT route as fast_track derives &#x60;human&#x60;, not &#x60;auto&#x60;.
Expected: Matrix cells should clearly indicate that &#x60;auto&#x60; for T1/T2 + low is only reachable via the fast_track decision branch, not the default derivation.
Suggestion: Extend the † footnote to cover both T1 and T2 low-risk cells, or add a note under the matrix: &#x27;For T1/T2 + low, &#x60;auto&#x60; is only reachable when the gatekeeper decision is fast_track; otherwise the derivation produces &#x60;human&#x60;.&#x27;

[MINOR] Row 3 in worked examples lists size_tier &#x3D; T2 for a reported crash.
File: docs/risk-and-cluster-taxonomy.md:148
Evidence: &#x27;T1 drive crashes when contract-is-plan because submit-plan checks plan !&#x3D; null&#x27; is classified T2.
Expected: A bug-report intake row&#x27;s size_tier should reflect the size of the proposed FIX, but the example doesn&#x27;t make that explicit; readers may infer T2 refers to the affected feature (T1 drive cycle) rather than the fix-size estimate.
Suggestion: Either add a one-line note clarifying that &#x60;size_tier&#x60; is the proposed-fix size, or relabel to T1/T3 if the fix would be a contract-is-plan or multi-phase change.

[MINOR] Precedence ordering in derive_risk_class is correct but undocumented in prose.
File: docs/risk-and-cluster-taxonomy.md:115-126
Evidence: derive_risk_class checks authority → security → architecture → low → normal in that order; row 4&#x27;s note (line 157) explains this implicitly via &#x27;authority wins the precedence order ... because it is checked first&#x27;, but the function block itself has no comment naming the precedence-order invariant.
Expected: An executor codifying this into schema validators should see the precedence ordering called out as load-bearing.
Suggestion: Add a one-line comment above derive_risk_class such as &#x27;# Precedence: authority &gt; security &gt; architecture &gt; low &gt; normal. Reordering changes routing for multi-flag rows.&#x27;
- **At:** 2026-05-06T06:59:32Z

### Phase 4 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified in docs/gatekeeper-design.md: AC4.1 names 5 trigger classes (risk-flag/cluster-threshold/pre-ratification/periodic-sweep/post-accept-batch) with concrete numeric thresholds (≥3 default &amp; ≥2 priors clusters; ≥5/90d sweep; ≥3 tasks/14d with ≥2 cluster overlap). AC4.2 enumerates all 7 outputs (allow_local_fix, reframe_contract, merge_with_cluster, create_primitive_task, block_pending_fixes, propose_doctrine_update, request_human_arch_decision) with paragraph-length definitions. AC4.3 PROHIBIT list names all 8 scope_out items verbatim (authority/lifecycle/schema/subscriber/runner/deploy/security/approval-token). AC4.4 lists 3 abuse cases (risk-flag underuse, cluster-key collision, fast-track creep) each with 3 counter-measures. Risk-flag and cluster-key references cross-check cleanly against phase-3 taxonomy doc. 0 critical, 0 major, 3 minor.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Trigger 4 (periodic-sweep) predicate references &#x60;architecture_reviews.count_for(cluster_key) &#x3D;&#x3D; 0&#x60;, but the lifecycle table at line 38 notes that store may not yet exist (&#x60;tagged-observation stand-in until that store exists&#x60;). The sweep query needs a fallback path during the stand-in period.
File: docs/gatekeeper-design.md (Trigger 4 section)
Suggestion: Add a sentence noting the sweep falls back to scanning tagged-observation rows by cluster_key until the architecture_reviews store ships.

[MINOR] Abuse case 1 counter-measure 2 says spot-check sampling uses &#x60;a fresh gatekeeper invocation&#x60;, but the abuse-case preamble explicitly states &#x60;None of these counter-measures are policed by the gatekeeper agent itself — that would be the fox guarding the henhouse&#x60;. A fresh invocation of the same role is the same fox.
File: docs/gatekeeper-design.md (Abuse case 1, counter-measure 2)
Suggestion: Clarify that the re-classification uses a distinct verifier role (or a held-out reference classifier), not another gatekeeper instance, to remain consistent with the preamble.

[MINOR] Trigger 5 (post-accept-batch) threshold &#x60;cluster overlap ≥ 2&#x60; is ambiguous: does it mean 2 clusters each touched by ≥2 tasks in the batch, or any 2 distinct clusters touched by anyone in the batch? With 3 accepted tasks, the latter trivially fires for any task with multiple linked_observations.
File: docs/gatekeeper-design.md (Trigger 5 section)
Suggestion: Tighten the predicate (e.g., &#x60;≥2 cluster_keys each appear in linked_observations of ≥2 tasks in the batch&#x60;). Acceptable to defer to Phase 5 implementation if explicitly noted as an open implementation detail.

[INFORMATIONAL] Fast-track ALLOW condition 5 (cluster-threshold-crossed → ineligible) is correctly worded as forward-only; it does not retroactively unwind already-routed fast-tracks, consistent with Trigger 2&#x27;s note that earlier rows are not retro-escalated and Trigger 5 is the safety net. No action required.
- **At:** 2026-05-06T07:03:03Z

### Phase 5 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS, last phase. AC5.2 verified directly: docs/gatekeeper-design.md gains a Follow-ups section listing L142 and L143 with tier_hint&#x3D;T3, scope-grounded in the design&#x27;s own sections, cross-linked to risk-and-cluster-taxonomy.md and architecture-coherence.md. AC5.1/5.3/5.4 are substrate-side (observation rows in .stores/db.sqlite); my tool whitelist excludes &#x60;sqlite3&#x60;/&#x60;stores&#x60;, so they rely on the executor&#x27;s submission claim + the doc reference, which are internally consistent (executor explicitly says they did NOT self-approve via ai_with_human, matching AC5.3). 0 critical, 0 major, 3 minor.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Follow-ups section omits a one-line summary of each observation&#x27;s done_when / scope_in / scope_out.
File: docs/gatekeeper-design.md:421-426
Evidence: Each bullet describes the observation in prose but does not surface the structured contract that U1 ratification will lock in. A reader landing on the design doc must run &#x60;stores observations show L142&#x60; to see what the contract actually says.
Expected: AC5.4 calls for a contract complete enough for U1 without further fields. The doc could mirror that for offline readers.
Suggestion: Append a one-line &#x60;done_when:&#x60; / &#x60;scope_in:&#x60; / &#x60;scope_out:&#x60; snippet under each L### bullet, or add a pointer like &#x60;(see contract: stores observations show L142)&#x60;.

[MINOR] Phase 5 commit modifies only the docs file; the substrate writes (the actual L142 / L143 rows) are invisible to git review.
File: c8b04c6 (1 file, 9 insertions)
Evidence: &#x60;git show c8b04c6 --stat&#x60; shows only docs/gatekeeper-design.md. The observation rows are persisted in .stores/db.sqlite (a symlink to the canonical stores repo).
Expected: For auditability outside this session, a code-reviewer with read-only DB access can verify AC5.1/5.4. My whitelist excludes sqlite3/stores; verification here is by submission claim.
Suggestion: When this is the dominant pattern (substrate-write phases), the executor could include the relevant &#x60;stores observations show L###&#x60; excerpt in the submission summary so reviewers without DB access can verify directly. Process suggestion, not a code change.

[MINOR] The Follow-ups section uses a single forward-link direction (design doc → observations) but no back-link is added to the other docs.
File: docs/gatekeeper-design.md:421-426
Evidence: docs/risk-and-cluster-taxonomy.md and docs/architecture-coherence.md are cited from the new section, but neither doc is updated to mention L142/L143.
Expected: Not strictly required by any AC, but the design&#x27;s other docs are part of the same T045 deliverable; cross-linking them to the seeds would let readers arrive from any of the three doors.
Suggestion: Optionally add a one-line &#x60;See L142, L143 (filed against T045) for implementation seeds&#x60; footer to risk-and-cluster-taxonomy.md and architecture-coherence.md. Defer-able; not a blocker.

[INFORMATIONAL] Last-phase PASS routes to task completion. The substrate&#x27;s auto-promote subscriber will not act on L142/L143 until the user ratifies their contracts (--contract-state ready --approved-by ... --approve-token &lt;T&gt;). The executor correctly did not self-approve.
- **At:** 2026-05-06T07:07:31Z

---

## Completion
- **In Review:** 2026-05-06T07:09:29Z — awaiting human GO/NO_GO

