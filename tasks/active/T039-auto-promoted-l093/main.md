# T039: Planner brief lacks tier_hint awareness: T2 planners produce multi-phase plans rejected by submit-plan (L030&#x27;s deferred remainder)

## Meta
- **Status:** in_review
- **Created:** 2026-05-06T04:07:46Z
- **Last Updated:** 2026-05-06T04:16:42Z
- **Current Phase:** 1
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T039-auto-promoted-l093

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - stores/tasks/templates/planner-brief.md.tpl: tier-conditional section using template helpers
- src/handlers/brief.rs: ensure tier_hint is in the planner template context
- snapshot tests covering T1, T2, T3 brief outputs (one fixture per tier)
- **Out:** - redesigning the planner prompt structure beyond the tier-conditional section
- tier-aware code-reviewer brief modulation (L030&#x27;s other deferred sub-task)
- auto-detecting tier from task content; tier_hint must be explicitly populated

### Done When
Add tier_hint awareness to the planner-brief template so T2 planners are explicitly told to produce exactly one phase (avoiding the schema-rejection 213s discard), T3 planners are told to multi-phase, and T1 carries a defensive note that planner shouldn&#x27;t be invoked for T1.

Acceptance:
- T2 brief contains explicit &#x27;produce exactly one phase&#x27; instruction
- T3 brief contains multi-phase decomposition instruction
- T1 brief either is empty/skipped OR contains defensive &#x27;should not be invoked&#x27; note
- Snapshot tests for each of T1, T2, T3 brief outputs added and pass
- existing brief tests pass

### Phases

_Plan not yet submitted._

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Added Tier Guidance section to stores/tasks/templates/planner-brief.md.tpl with branches for T1 (defensive &#x27;SHOULD NOT be invoked&#x27; note), T2 (&#x27;Produce exactly one phase&#x27; instruction citing the phases.length !&#x3D; 1 schema rejection), T3 (multi-phase decomposition), and unset (fallback with flag-in-objective). Added 4 snapshot tests in src/handlers/brief.rs (planner_brief_t1/t2/t3/unset) verifying each branch renders the correct guidance and excludes the others. cargo build clean; all 775 lib tests pass including the 4 new ones plus the existing ac7_4_all_four_briefing_templates_render_successfully regression.
- **Commit:** `a15c575d821659dc4ae6ef03ad73f772d6b5250e`
- **Files:**
  - `stores/tasks/templates/planner-brief.md.tpl`
  - `src/handlers/brief.rs`
- **At:** 2026-05-06T04:10:37Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** Re-applying PASS after L123 schema fix; original reviewer issued PASS at cycle 1; submit-review crashed on missing T1 transition (now fixed).
- **Findings:** 0 critical, 0 major, 0 minor
- **At:** 2026-05-06T04:15:51Z

---

## Completion
- **In Review:** 2026-05-06T04:16:42Z — awaiting human GO/NO_GO

