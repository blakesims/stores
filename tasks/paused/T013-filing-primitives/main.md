# T013: Filing primitives - drafted contracts at observations.add + tier_hint on tasks

## Meta
- **Status:** blocked
- **Created:** 2026-05-03T09:49:30Z
- **Last Updated:** 2026-05-03T09:54:45Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** plan-review NEEDS_WORK cycle limit exceeded (plan_review_log.length &gt;&#x3D; 3): Plan is structurally sound and phases are well-ordered with mostly mechanical ACs, but three referenced decision-matrix entries are absent from the rendered brief and one done-when clause has an unresolved interpretation that changes Phase 3 behavior. Resolve these and the plan is executable.
- **Branch:** feat/T013-filing-primitives

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** schema.yaml relaxations on observations.intent_contract.* and transition guards; observations add CLI flag additions (--objective, --acceptance, --in-scope, --out-of-scope, --lock-contract); tasks add CLI flag addition (--tier-hint) + inheritance logic; unit + integration tests.
- **Out:** L022 policy layer (deferred to T006/engine task); L018 watcher (T006); L026 accept-merges-branch (T006); L030 agent-tier-aware briefs (separate task); L013/L014/L015 auth UX cluster.

### Done When
(1) intent_contract.{objective,acceptance,in_scope,out_of_scope,tier_hint,approved_by,approved_at} settable at observations.add for human/ai_with_human invokers (with token where the field is actor: human). (2) --lock-contract shorthand on observations add: when present and all required fields are populated, sets contract_state&#x3D;ready; rejects ai_autonomous. (3) tasks.tier_hint field added; settable at tasks add via --tier-hint flag; auto-inherits from --linked-observations if all linked obs agree on tier; --tier-hint required if observations disagree or none provided. (4) Tests cover: ai_with_human + --lock-contract → ready; ai_autonomous without lock → draft; tier_hint inheritance from observations; conflict-on-mixed-tiers rejection.

### Phases

#### Phase 1: Phase 1: Schema relaxations + tier_hint on tasks
- **Objective:** Update schema.yaml for both stores so the contract sub-fields are settable at add time and tasks gains a tier_hint enum.
- **Tasks:**
  - Task 1.1: Verify (no edit) that observations.intent_contract.{objective,acceptance,in_scope,out_of_scope,tier_hint,type,drafted_by,drafted_at} carry no actor declaration so ai_with_human / human can write them at add (current schema already complies — confirm in plan_notes; only edit if a hidden actor: human exists).
  - Task 1.2: Confirm observations.intent_contract.{approved_by,approved_at} retain actor: human (token-mediated path already wired via T001 P3) — no schema change needed; document in plan_notes.
  - Task 1.3: Add tier_hint field to stores/tasks/schema.yaml as enum [T1, T2, T3], required: false, no actor (declared at top-level fields list, after linked_observations).
  - Task 1.4: Run &#x60;stores schema observations&#x60; / &#x60;stores schema tasks&#x60; smoke and confirm leaf_args generates --tier-hint flag automatically on tasks add via dynamic.rs.
- **Acceptance Criteria:**
  - [ ] AC1.1: cargo build succeeds.
  - [ ] AC1.2: &#x60;stores tasks add --help&#x60; lists &#x60;--tier-hint&#x60; with values T1|T2|T3.
  - [ ] AC1.3: schemas_validate_fixtures test passes (no schema regression).
  - [ ] AC1.4: A unit test (new) reads tasks schema and asserts a field named &#x60;tier_hint&#x60; exists with enum_values [T1,T2,T3].
- **Files:** `stores/tasks/schema.yaml`, `stores/observations/schema.yaml`, `tests/schemas_validate_fixtures.rs`
#### Phase 2: Phase 2: --lock-contract shorthand on observations add
- **Objective:** Implement a CLI shorthand that finalises a drafted contract atomically at add time; rejects ai_autonomous; auto-fills approved_at when the invoker is permitted.
- **Tasks:**
  - Task 2.1: In src/cli/dynamic.rs::build_add_cmd, add &#x60;--lock-contract&#x60; flag (ArgAction::SetTrue) for the observations store only (gate on schema.name &#x3D;&#x3D; &quot;observations&quot; or apply universally — see decision matrix).
  - Task 2.2: In src/handlers/add.rs::run, after build_entry_map and before validate, detect &#x60;--lock-contract&#x60;. When present: (a) set entry[&quot;intent_contract&quot;][&quot;contract_state&quot;] &#x3D; &quot;ready&quot;; (b) if &#x60;intent_contract.drafted_at&#x60; absent, fill with now_iso8601(); (c) if &#x60;intent_contract.approved_at&#x60; absent and invoker is human OR (ai_with_human + token_valid), fill with now_iso8601(); (d) if &#x60;intent_contract.approved_by&#x60; absent and same condition, fill with invoker.actor.to_string(); (e) explicitly reject if invoker.actor &#x3D;&#x3D; AiAutonomous with a clear error before validation runs.
  - Task 2.3: Ensure the validator&#x27;s required_when on objective/acceptance/in_scope/out_of_scope/tier_hint/type fires when --lock-contract is set without those fields (relies on existing required_when logic — verify, don&#x27;t reimplement).
  - Task 2.4: Wire the &#x60;--lock-contract&#x60; arg through &#x60;is_reserved&#x60; skip-list (it&#x27;s not a schema field; ensure build_entry_map ignores it).
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;stores observations add --invoker ai_autonomous ... --lock-contract&#x60; exits non-zero with an error that names &#x60;--lock-contract&#x60; and &#x60;ai_autonomous&#x60;.
  - [ ] AC2.2: &#x60;stores observations add --invoker human ... --lock-contract&#x60; with all required contract sub-fields → row inserted with intent_contract.contract_state &#x3D;&#x3D; &#x27;ready&#x27; and approved_by/at populated.
  - [ ] AC2.3: &#x60;stores observations add --invoker ai_with_human --approve-token &lt;T&gt; --lock-contract&#x60; (with required fields) succeeds; row has approved_by&#x3D;&#x27;ai_with_human&#x27; (or &#x27;human&#x27; — see decision matrix).
  - [ ] AC2.4: &#x60;stores observations add --invoker human --lock-contract&#x60; WITHOUT objective/acceptance/in_scope/out_of_scope/tier_hint/type → validation fails citing each missing field.
  - [ ] AC2.5: &#x60;stores observations add ... --invoker ai_autonomous&#x60; WITHOUT --lock-contract → row inserted with intent_contract.contract_state &#x3D;&#x3D; &#x27;draft&#x27; (default behaviour preserved).
- **Files:** `src/cli/dynamic.rs`, `src/handlers/add.rs`
- **Dependencies:** Phase 1 complete
#### Phase 3: Phase 3: tier_hint inheritance on tasks add
- **Objective:** When tasks are created with --linked-observations, auto-inherit tier_hint from the linked observations when they unanimously agree; otherwise require an explicit --tier-hint.
- **Tasks:**
  - Task 3.1: In src/handlers/add.rs::run (after build_entry_map, before validate), if schema.name &#x3D;&#x3D; &quot;tasks&quot; and entry has &#x60;linked_observations&#x60; (non-empty array), look up each linked observation row from the same connection. For each, read intent_contract.tier_hint. Collect into a set.
  - Task 3.2: If &#x60;entry[tier_hint]&#x60; is absent: (a) all linked obs agree on a single non-null tier → set entry[tier_hint] to that value; (b) linked obs disagree (or some have no tier) → bail with a clear error listing each obs id + its tier and instructing user to pass --tier-hint; (c) no linked obs → leave entry[tier_hint] absent (no auto-fill, no rejection unless schema marks it required, which it does not).
  - Task 3.3: If &#x60;entry[tier_hint]&#x60; is present, skip inference (explicit flag wins).
  - Task 3.4: Gracefully handle unknown linked obs ids (already validated as soft-FK elsewhere — a missing row produces a warning, not a hard fail; see decision matrix).
- **Acceptance Criteria:**
  - [ ] AC3.1: &#x60;stores tasks add ... --linked-observations L001 --linked-observations L002&#x60; where both have tier_hint&#x3D;T3 and no --tier-hint flag → task row stored with tier_hint&#x3D;&#x27;T3&#x27;.
  - [ ] AC3.2: Same scenario with L001&#x3D;T2 and L002&#x3D;T3 and no --tier-hint → exit non-zero with error naming both ids and their tiers.
  - [ ] AC3.3: Same disagreement scenario WITH &#x60;--tier-hint T3&#x60; → succeeds; row stored with tier_hint&#x3D;&#x27;T3&#x27;.
  - [ ] AC3.4: &#x60;stores tasks add ...&#x60; (no linked obs, no --tier-hint) → succeeds with tier_hint NULL.
  - [ ] AC3.5: &#x60;stores tasks add --linked-observations L999&#x60; (non-existent) → succeeds with a stderr warning (tier_hint stays absent unless flag passed).
- **Files:** `src/handlers/add.rs`
- **Dependencies:** Phase 1 complete
#### Phase 4: Phase 4: Tests + e2e coverage
- **Objective:** Cover the four Done-When clauses with unit + e2e tests.
- **Tasks:**
  - Task 4.1: Unit tests in src/handlers/add.rs::tests covering: lock_contract_with_ai_autonomous_rejected; lock_contract_with_human_lands_ready; lock_contract_without_required_fields_fails; add_without_lock_contract_defaults_to_draft.
  - Task 4.2: Unit tests covering tier_hint inheritance: tier_hint_inherits_when_obs_agree; tier_hint_rejects_when_obs_disagree; tier_hint_explicit_overrides_disagreement; tier_hint_absent_when_no_linked_obs.
  - Task 4.3: Extend tests/observations_e2e.sh with a Step 9 demonstrating --lock-contract end-to-end: ai_autonomous attempt → fail; human attempt → succeed with contract_state&#x3D;ready.
  - Task 4.4: New tests/tasks_tier_hint_e2e.sh (or extend tasks_e2e.sh) demonstrating: create two L### with tier_hint&#x3D;T3, then &#x60;stores tasks add --linked-observations L001 --linked-observations L002&#x60; → row tier_hint&#x3D;T3.
- **Acceptance Criteria:**
  - [ ] AC4.1: &#x60;cargo test handlers::add&#x60; passes with 4+ new lock_contract tests.
  - [ ] AC4.2: &#x60;cargo test handlers::add&#x60; passes with 4+ new tier_hint tests.
  - [ ] AC4.3: &#x60;bash tests/observations_e2e.sh&#x60; exits 0; output contains &#x27;PASS: --lock-contract&#x27; or equivalent.
  - [ ] AC4.4: &#x60;bash tests/tasks_e2e.sh&#x60; (or new tier_hint script) exits 0 and demonstrates the inheritance path.
- **Files:** `src/handlers/add.rs`, `tests/observations_e2e.sh`, `tests/tasks_e2e.sh`
- **Dependencies:** Phase 2 complete, Phase 3 complete

---

## Plan Review

### Review 1
- **Gate:** NEEDS_WORK
- **Summary:** Plan is structurally sound and phases are well-ordered with mostly mechanical ACs, but three referenced decision-matrix entries are absent from the rendered brief and one done-when clause has an unresolved interpretation that changes Phase 3 behavior. Resolve these and the plan is executable.
- **Open Questions:**
  - Decision matrix is referenced in Task 2.1 (&#x27;gate --lock-contract on observations only or apply universally — see decision matrix&#x27;), Task 2.3/AC2.3 (&#x27;approved_by&#x3D;ai_with_human or human — see decision matrix&#x27;), and Task 3.4 (&#x27;unknown linked obs ids — see decision matrix&#x27;), but no decision matrix appears in the plan. Add explicit entries (decision/options/chosen/rationale) for each so the executor is not guessing.
  - Done-when clause (3) says &#x27;--tier-hint required if observations disagree or none provided&#x27;. Phase 3 Task 3.2(c) and AC3.4 interpret &#x27;none provided&#x27; as &#x27;no linked observations exist&#x27; and allow tier_hint to be NULL in that case. But the natural reading is that --tier-hint must be required whenever it cannot be inherited (no linked obs OR linked obs disagree OR linked obs lack tier). Pin the interpretation: should &#x60;tasks add&#x60; without --linked-observations and without --tier-hint succeed (current plan) or fail (literal reading of done_when)? Update AC3.4 accordingly.
  - Done-when clause (1) names approved_by/approved_at as settable at observations.add. Phase 1 Task 1.2 leaves them as actor: human with no schema change, and Phase 2 only sets them via --lock-contract auto-fill. Confirm that the dynamic CLI auto-generates --approved-by / --approved-at flags for direct setting under tier-A (human or ai_with_human + token), and add an AC asserting &#x60;stores observations add --invoker human --approved-by human --approved-at &lt;iso&gt;&#x60; works without --lock-contract. Otherwise done_when (1) is only partially demonstrable.
  - Phase 1 Task 1.3 places tier_hint as a top-level field on tasks. Confirm that placement matches the existing actor/grouping conventions in stores/tasks/schema.yaml (e.g., not nested under intent_contract on tasks the way it is on observations). If tasks already has an intent_contract block, decide and document whether tier_hint belongs there or as a top-level field — this is a decision-matrix-worthy choice.
- **At:** 2026-05-03T09:54:45Z

---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

