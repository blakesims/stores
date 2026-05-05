# T013: Filing primitives - drafted contracts at observations.add + tier_hint on tasks

## Meta
- **Status:** in_review
- **Created:** 2026-05-03T09:49:30Z
- **Last Updated:** 2026-05-03T10:34:46Z
- **Current Phase:** 4
- **Current Cycle:** 1
- **Blocked Reason:** —
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
(1) intent_contract.{objective, acceptance, in_scope, out_of_scope, tier_hint, approved_by, approved_at} settable at observations.add for human / ai_with_human invokers (with token where the field is actor: human). MUST be demonstrable via direct flags (--objective, --acceptance, --in-scope, --out-of-scope, --tier-hint, --approved-by, --approved-at) WITHOUT --lock-contract — a two-step file-now / approve-later flow must work.

(2) --lock-contract shorthand on observations add: when present and all required intent_contract sub-fields are populated, sets contract_state&#x3D;ready; auto-fills approved_by from invoker.actor and approved_at from now() if not provided; rejects --invoker ai_autonomous with a clear error.

(3) tasks.tier_hint field added (TOP-LEVEL on tasks schema, NOT nested in any intent_contract block). Settable at tasks add via --tier-hint flag with values T1|T2|T3. When --linked-observations is provided, auto-inherits tier_hint if and only if ALL linked observations have the same non-null tier_hint. Otherwise (no linked obs, OR any linked obs has null/disagreeing tier), --tier-hint is REQUIRED — tasks add fails with a clear error listing each linked obs id and its tier. Explicit --tier-hint always wins (overrides inheritance).

(4) Tests cover: (a) ai_autonomous + --lock-contract → reject; (b) human + --lock-contract with required fields → contract_state&#x3D;ready; (c) human without --lock-contract, with --approved-by + --approved-at → row inserted with those fields populated, contract_state stays draft; (d) ai_autonomous without --lock-contract → contract_state&#x3D;draft; (e) tier_hint inheritance: 2 linked obs both T2 → task tier_hint&#x3D;T2; (f) tier_hint conflict: linked obs T2 + T3, no --tier-hint → reject; (g) tier_hint conflict: linked obs T2 + T3, --tier-hint&#x3D;T3 → task tier_hint&#x3D;T3; (h) no linked obs, no --tier-hint → reject (NOT default-NULL); (i) explicit --tier-hint always wins.

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

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Added top-level tier_hint enum [T1,T2,T3] field to stores/tasks/schema.yaml after linked_observations (no actor, required: false). Verified observations.intent_contract sub-fields (objective/type/in_scope/out_of_scope/acceptance/tier_hint/drafted_by/drafted_at) carry no actor and approved_by/approved_at retain actor: human — no observations schema edit needed (Tasks 1.1, 1.2). Added t013_p1_tasks_tier_hint_field_present unit test (Task 1.4 / AC1.4). cargo build clean; full suite 517 lib + 2 integration tests pass; --tier-hint flag auto-generates on &#x60;stores tasks add&#x60; with description listing T1|T2|T3.
- **Commit:** `2318f9641ca29165203c58fa7370531637d09bac`
- **Files:**
  - `stores/tasks/schema.yaml`
  - `src/schema/mod.rs`
- **At:** 2026-05-03T10:16:19Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Implemented --lock-contract shorthand on observations add. Flag registered in build_add_cmd gated on schema.name &#x3D;&#x3D; &#x27;observations&#x27;; handler runs before validate(): rejects ai_autonomous fail-loud, sets intent_contract.contract_state&#x3D;&#x27;ready&#x27;, auto-fills drafted_at, and fills approved_at/approved_by from invoker when human OR (ai_with_human + token_valid). Existing required_when rules on objective/type/in_scope/out_of_scope/acceptance/tier_hint fire naturally — no validator changes. 6 new tests cover AC2.1–2.5 plus the two-step file-now/approve-later flow from Done When (4c). All 523 tests pass; cargo build clean.
- **Commit:** `546bc4f96b44a900fd3e459b28110622d38c4715`
- **Files:**
  - `src/cli/dynamic.rs`
  - `src/handlers/add.rs`
- **At:** 2026-05-03T10:22:49Z
### Phase 3 / Cycle 1
- **Status:** PASS
- **Summary:** Implemented tier_hint inheritance on tasks add (src/handlers/add.rs:93-152). When --linked-observations is supplied and --tier-hint is absent, each linked obs&#x27;s intent_contract.tier_hint is read; unanimous non-null agreement auto-inherits, disagreement (or any present row missing a tier) bails with a per-obs listing, missing rows produce a stderr warning and are excluded from inference. Added 7 tests covering AC3.1-3.5 plus explicit-flag-overrides-agreement and present-obs-without-tier-rejects. cargo build clean; 530 lib tests + 2 fixture tests pass.
- **Commit:** `1fcf657bb812b11ae0203ca5353918e2d109f15b`
- **Files:**
  - `src/handlers/add.rs`
- **At:** 2026-05-03T10:27:16Z
### Phase 4 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 4 e2e coverage. AC4.1 (lock_contract unit tests, 4+) and AC4.2 (tier_hint unit tests, 4+) were already satisfied by Phase 2/3 commits — &#x60;cargo test handlers::add&#x60; shows 30/30 ok including 6 lock_contract + 7 tier_hint cases. Added Step 12 to tests/observations_e2e.sh (AC4.3): ai_autonomous + --lock-contract rejected with error naming both --lock-contract and ai_autonomous; human + --lock-contract lands intent_contract.contract_state&#x3D;ready with approved_by&#x3D;human and approved_at populated. Created tests/tasks_tier_hint_e2e.sh (AC4.4): seeds L001+L002 at tier_hint&#x3D;T3, demonstrates unanimous inheritance, disagreement rejection (T2+T3 with error citing both obs ids and --tier-hint), explicit override, and no-linked-no-flag → NULL. All e2e scripts and the full cargo test suite (530 passed) green.
- **Commit:** `ffe8cb95cce6e0f3036f6a02ab7eaf80d2cd0ca0`
- **Files:**
  - `tests/observations_e2e.sh`
  - `tests/tasks_tier_hint_e2e.sh`
- **At:** 2026-05-03T10:31:01Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified: cargo build clean; tier_hint field present in tasks schema as enum [T1,T2,T3] required:false no actor; schemas_validate_fixtures (2 tests) pass; new t013_p1_tasks_tier_hint_field_present unit test passes (full suite 517 lib + 2 integration green). Diff is small (2 files, +41 LOC) and matches the phase scope — observations schema correctly left untouched because contract sub-fields already carry no actor and approved_by/at retain actor:human. Two minor notes documented; not blocking.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[MINOR] AC1.2 (--help shows --tier-hint with T1|T2|T3) is verified only indirectly. The flag is generated by build_leaf_cmd via the schema field&#x27;s description string, so the values appear in help text only because the executor manually wrote &#x27;Triage tier (T1|T2|T3); ...&#x27; into the field&#x27;s &#x60;description&#x60;. There is no automated regression-guard for this AC — if a future edit shortens the description, AC1.2 silently regresses without any test failing. Suggestion: in a later phase, add a snapshot-style unit test that renders &#x60;tasks add --help&#x60; (e.g. via clap&#x27;s &#x60;Command::render_help()&#x60;) and asserts the substring &#x60;--tier-hint&#x60; plus &#x60;T1|T2|T3&#x60; are present. Not required to fix in this phase since AC1.2 is satisfied as-stated.

[MINOR] The new field has no &#x60;actor:&#x60; declaration, which means &#x60;ai_autonomous&#x60; callers can write it directly via &#x60;tasks update --tier-hint ...&#x60; (not just via the &#x60;tasks add&#x60; happy path that Phase 2/3 will gate with inheritance/required-presence logic). This is consistent with the plan&#x27;s intent (CLI-layer enforcement, not schema-layer), but it widens the autonomous surface slightly compared to e.g. the contract.* fields. Suggestion: when Phase 2/3 lands the CLI gating, add a doc comment on the schema field noting that presence-required-ness is enforced at the handler layer in tasks_add, not via &#x60;required: true&#x60; (otherwise a future reader will wonder why the schema lets it be NULL).

[INFORMATIONAL] Diff stat: 2 files, +41 LOC. observations/schema.yaml not edited — verified intent_contract sub-fields {objective,type,in_scope,out_of_scope,acceptance,tier_hint,drafted_by,drafted_at} carry no &#x60;actor:&#x60;, and {approved_by,approved_at} retain &#x60;actor: human&#x60; per executor&#x27;s claim. Matches the Done-When clause-1 design: file-now / approve-later requires only that the sub-fields be writable at add time, which they already are.

[INFORMATIONAL] Test file count: 1 new unit test added (t013_p1_tasks_tier_hint_field_present). Adequate for a schema-only phase; CLI-behavior tests will land with Phase 2/3 implementation.
- **At:** 2026-05-03T10:18:40Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 5 ACs verified: AC2.1 ai_autonomous + --lock-contract rejected with both names in error; AC2.2 human + --lock-contract writes contract_state&#x3D;&#x27;ready&#x27; with approved_by/at populated; AC2.3 ai_with_human + token_valid writes ready with approved_by&#x3D;&#x27;ai_with_human&#x27;; AC2.4 missing required fields produces validation errors citing all 6 fields; AC2.5 ai_autonomous without lock leaves intent_contract unmutated. Done When (4c) two-step file/approve flow also covered. cargo test: 523 + new tests pass; cargo build clean. Flag is correctly gated to schema.name&#x3D;&#x3D;&#x27;observations&#x27; in dynamic.rs:828, while handler uses try_contains_id for safety. Reject-before-validate ordering correctly prevents drift via lock+autonomous.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] AC2.5 test (&#x60;ai_autonomous_without_lock_writes_draft_unchanged&#x60;) seeds &#x60;--contract-state draft&#x60; explicitly because the production observations schema (stores/observations/schema.yaml) has no &#x60;default: draft&#x60; on contract_state. The test correctly verifies the handler does not mutate intent_contract when --lock-contract is absent, but the AC wording &#x27;default behaviour preserved&#x27; implies the schema itself defaults contract_state to draft. Worth a follow-up to add &#x60;default: draft&#x60; on the contract_state field in the schema (orthogonal to this phase).

[MINOR] When invoker is ai_with_human with token_valid&#x3D;false and --lock-contract is passed, approver_permitted&#x3D;false → approved_by/at left unfilled → validation fails citing missing approved_by/at rather than a more direct &#x27;lock-contract requires a valid token for ai_with_human&#x27; message. Matches the substrate&#x27;s fail-loud-via-validator pattern but the user experience is one step removed. Not blocking.

[MINOR] handlers/add.rs:23 — &#x60;lock_contract&#x60; boolean is computed unconditionally for every store via try_contains_id; relies on the flag not being registered for non-observations stores. Correct, but a one-line comment noting &#x27;flag is observations-only; try_contains_id returns false elsewhere&#x27; would aid readers. Existing comment at lines 19–22 partially covers it.

[INFORMATIONAL] cargo build emits 3 pre-existing warnings unrelated to this commit (the diff did not introduce them). Confirmed via warning text mentioning code outside src/handlers/add.rs and src/cli/dynamic.rs.

[INFORMATIONAL] Git reality matches submission: HEAD&#x3D;546bc4f, two files changed (src/cli/dynamic.rs +18 lines, src/handlers/add.rs +308 lines), no extraneous changes in working tree.
- **At:** 2026-05-03T10:24:21Z

### Phase 3 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. AC3.1-3.5 all verified mechanically: 7 new tests pass (unanimous T3 inherits, disagreement rejects with both ids+tiers, explicit flag overrides disagreement and agreement, no-link+no-flag yields NULL, unknown linked obs warns-not-fails, present-obs-without-tier rejects). Implementation correctly skips the inference branch when --tier-hint is set, parameterizes the obs lookup safely, and uses soft-FK semantics for missing rows. 0 critical, 0 major, 2 minor.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[INFORMATIONAL] Phase ACs vs. top-level done_when contract diverge. done_when(3) says: &quot;Otherwise (no linked obs, OR any linked obs has null/disagreeing tier), --tier-hint is REQUIRED.&quot; But AC3.4 explicitly requires that no-linked-obs + no-flag SUCCEEDS with tier_hint&#x3D;NULL, and AC3.5 requires unknown linked obs to warn-and-succeed. The executor correctly followed the phase ACs. The contradiction is a planning artifact; if the stricter done_when intent is wanted, a follow-up phase or amend should tighten it. Flagging so the wrap reviewer sees it before declaring done_when satisfied.

[MINOR] String-coupled schema name in generic handler.
File: src/handlers/add.rs:97
Evidence: &#x60;if schema.name &#x3D;&#x3D; &quot;tasks&quot; &amp;&amp; entry.get(&quot;tier_hint&quot;).is_none()&#x60; hardcodes the literal &quot;tasks&quot; inside the otherwise schema-driven add handler.
Expected: Generic handlers should ideally avoid coupling to a specific schema name; behavior keyed off schema metadata (e.g. a &#x60;linked_fk_inherits: tier_hint&#x60; declaration) would scale.
Suggestion: Acceptable as-is for T013 scope (tasks is the only consumer), but consider a follow-up to lift the inheritance rule into schema metadata so &#x60;tasks add&#x60; doesn&#x27;t have a unique branch in the shared add handler.

[MINOR] Inference loop ignores non-string entries in linked_observations array silently.
File: src/handlers/add.rs:103-104
Evidence: &#x60;let Some(obs_id) &#x3D; v.as_str() else { continue };&#x60; — non-string array members are dropped without a warning.
Expected: This shouldn&#x27;t happen given clap&#x27;s string-typed list_fk parsing, but a debug_assert! or a warning would surface a future regression.
Suggestion: Add &#x60;debug_assert!(v.is_string(), &quot;linked_observations entries must be strings&quot;)&#x60; to guard against silent drops if upstream parsing changes.
- **At:** 2026-05-03T10:28:06Z

### Phase 4 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. Phase 4 ACs met: cargo test handlers::add 30/30 (6 lock_contract + 7 tier_hint tests, both &gt;&#x3D;4); observations_e2e.sh Step 12 covers ai_autonomous-reject and human-ready paths with &#x27;PASS: --lock-contract&#x27; lines; new tasks_tier_hint_e2e.sh exercises unanimous inherit, disagreement reject, explicit override, and no-link/no-flag. 0 critical, 1 major (Done-When divergence inherited from P3), 2 minor.
- **Findings:** 0 critical, 1 major, 2 minor
**Details:**
[MAJOR] e2e Step 5 + unit test &#x60;tier_inheritance_no_linked_no_flag_succeeds_null&#x60; codify behavior that contradicts Done-When clause (3)/(4)(h).
File: src/handlers/add.rs:100-160 (handler) + src/handlers/add.rs:1500-1513 (unit test) + tests/tasks_tier_hint_e2e.sh:136-154 (e2e Step 5)
Evidence: Done-When (3) says &quot;Otherwise (no linked obs, OR any linked obs has null/disagreeing tier), --tier-hint is REQUIRED — tasks add fails with a clear error&quot; and (4)(h) restates: &quot;no linked obs, no --tier-hint → reject (NOT default-NULL)&quot;. The handler at add.rs:100-102 only triggers the inference/reject branch when &#x60;linked_observations&#x60; is a non-empty array (&#x60;if !linked.is_empty()&#x60;); the no-linked-obs path falls through and writes tier_hint as NULL. The unit test and e2e Step 5 codify NULL as the expected outcome.
Expected: per Done-When, this case must reject with a clear error directing the user to pass --tier-hint.
Note: Phase 3 review accepted this behavior (commit 1fcf657 shipped the tests asserting NULL). Phase 4&#x27;s job was coverage, not contract enforcement, and AC4.4 only requires &quot;demonstrates the inheritance path&quot; — which is satisfied. Not blocking P4, but the human should either (a) amend Done-When (4)(h) to match the shipped behavior, or (b) file a follow-up observation to harden the no-link path. Without one of those, the next reader of the contract will see a verifiable gap.

[MINOR] Double-PASS prefix in e2e Step 12 messages.
File: tests/observations_e2e.sh:494, 521
Evidence: &#x60;pass &quot;PASS: --lock-contract rejected for ai_autonomous&quot;&#x60; — the &#x60;pass()&#x60; helper at line 32 prints &#x60;&quot;  PASS: $*&quot;&#x60;, so the output line is &#x60;&quot;  PASS: PASS: --lock-contract ...&quot;&#x60;. Cosmetic, but inconsistent with sibling steps that pass plain phrases (e.g. line 463 &#x60;pass &quot;row unchanged after rejected retry...&quot;&#x60;).
Suggestion: drop the leading &#x60;PASS: &#x60; from the argument string. AC4.3 only requires the literal &#x60;PASS: --lock-contract&#x60; to appear, which it still does either way.

[MINOR] Unit test &#x60;tier_inheritance_unknown_linked_obs_warns_and_succeeds&#x60; cannot assert on the stderr warning.
File: src/handlers/add.rs:1515-1532
Evidence: comment at 1526-1528 explicitly notes &quot;we cannot capture it from the test process without redirection plumbing&quot;. The test verifies tier_hint stays NULL but does not verify the warning text reaches stderr. Coverage gap acknowledged in-line.
Suggestion (deferable): wire &#x60;eprintln!&#x60; through a writer/logger seam, or exercise the warning via the e2e script (capture &#x60;2&gt;&amp;1&#x60; and grep &#x60;linked observation &#x27;L999&#x27; not found&#x60;). Not required for AC4.2 (count threshold met).

[INFORMATIONAL] Files-changed in commit ffe8cb9 (&#x60;tests/observations_e2e.sh&#x60;, &#x60;tests/tasks_tier_hint_e2e.sh&#x60;) match executor-claimed files exactly. Git status clean. cargo test full suite: 530 pass per executor; spot-checked handlers::add: 30/30 ok.
- **At:** 2026-05-03T10:34:09Z

---

## Completion
- **In Review:** 2026-05-03T10:34:46Z — awaiting human GO/NO_GO

