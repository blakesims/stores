# T016: Brief plumbing - render planner decision_matrix in plan_reviewer brief

## Meta
- **Status:** in_review
- **Created:** 2026-05-03T10:55:03Z
- **Last Updated:** 2026-05-03T11:01:56Z
- **Current Phase:** 2
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T016-plan-reviewer-decision-matrix

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** src/handlers/brief.rs (or wherever plan_reviewer brief generation lives — locate via grep for the existing &quot;Prior Plan Reviews&quot; or &quot;Adversarial Mindset&quot; section); rendering of the planner&#x27;s decision_matrix field; unit and integration tests covering the four test scenarios in done-when (5).
- **Out:** Other agent briefs (executor, code_reviewer, wrap) — they stay narrow by design per the L033 narrowed scope (executor is a focused code-writer; not seeing decision_matrix is intentional). L035 schema-enforced context flow (separate T3 task; this fix is the manual brief-plumbing patch, L035 is the architectural type-system replacement). decision_matrix schema validation at submit-time (assumed already done by the existing submit_plan validator). Other open observations (L032 worktree access, L020 stale dirs, L021 wrap_log render, etc.).

### Done When
(1) Plan_reviewer brief includes a &quot;## Decision Matrix&quot; section that renders the planner&#x27;s decision_matrix field from the row&#x27;s plan column. Each entry is rendered as a subsection with the decision name as heading, options as bullet points, the chosen option called out, and the rationale as a paragraph.

(2) Section position: rendered AFTER &quot;## Current Plan&quot; but BEFORE &quot;## Prior Plan Reviews&quot;, so the reviewer reads the decisions BEFORE seeing prior review feedback.

(3) Backward compatibility: if the row has no decision_matrix (null/empty), the section either omits cleanly or renders an empty placeholder (&quot;(no decisions recorded)&quot;) — pick whichever matches existing brief patterns. Existing brief sections (Persona, Workflow Context, Task, Contract, Current Plan, Prior Plan Reviews, Critical Actions, Gate Decisions, Adversarial Mindset) remain unchanged.

(4) Other agent briefs (executor, code_reviewer, wrap) are NOT modified — they intentionally stay narrow per the doctrine that executors are narrow code-writers (this is the explicit reason L033 was narrowed).

(5) Tests cover: planner submitted decision_matrix with 3 entries → reviewer brief contains a Decision Matrix section listing all 3; planner submitted no decision_matrix → reviewer brief omits or shows empty placeholder; existing brief integration tests still pass.

### Phases

#### Phase 1: Phase 1: Render Decision Matrix section in plan-reviewer brief template
- **Objective:** Insert a &#x27;## Decision Matrix&#x27; block into the plan-reviewer brief template that iterates plan.decision_matrix, positioned between &#x27;## Current Plan&#x27; and &#x27;## Prior Plan Reviews&#x27;.
- **Tasks:**
  - Task 1.1: Edit stores/tasks/templates/plan-reviewer-brief.md.tpl — insert a new &#x27;## Decision Matrix&#x27; section immediately after the closing of the Current Plan phases loop (after line 62 &#x27;{{/each}}&#x27;) and before &#x27;## Prior Plan Reviews&#x27; (line 64).
  - Task 1.2: Implement the section using handlebars block: &#x60;{{#if plan.decision_matrix}}{{#each plan.decision_matrix}}### {{this.decision}}\n**Options:**\n{{#each this.options}}- {{this}}\n{{/each}}\n**Chosen:** {{this.chosen}}\n\n{{this.rationale}}\n\n{{/each}}{{else}}_(no decisions recorded)_{{/if}}&#x60; — mirroring the Prior Plan Reviews if/else placeholder pattern (template lines 64-80).
  - Task 1.3: Run &#x60;cargo build&#x60; to verify the bundled &#x60;include_str!&#x60; in src/cli/dynamic.rs:39 picks up the new template content without compile error.
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds.
  - [ ] AC1.2: The string &#x27;## Decision Matrix&#x27; appears exactly once in stores/tasks/templates/plan-reviewer-brief.md.tpl, between the line containing &#x27;Current Plan&#x27; and the line containing &#x27;## Prior Plan Reviews&#x27;.
  - [ ] AC1.3: The template references &#x60;plan.decision_matrix&#x60;, &#x60;this.decision&#x60;, &#x60;this.options&#x60;, &#x60;this.chosen&#x60;, and &#x60;this.rationale&#x60;.
  - [ ] AC1.4: No other template files (planner-brief.md.tpl, executor-brief.md.tpl, code-reviewer-brief.md.tpl, wrap-brief.md.tpl) are modified.
  - [ ] AC1.5: Existing sections (Persona, Workflow Context, Task, Contract, Current Plan, Prior Plan Reviews, Critical Actions, Gate Decisions, Adversarial Mindset) remain present and unchanged in the template.
- **Files:** `stores/tasks/templates/plan-reviewer-brief.md.tpl`
#### Phase 2: Phase 2: Test coverage for decision matrix rendering
- **Objective:** Add unit tests to src/handlers/brief.rs that render the plan-reviewer brief template against a fixture row with (a) three decision_matrix entries and (b) no decision_matrix, asserting the section appears with all entries / shows the placeholder respectively.
- **Tasks:**
  - Task 2.1: In src/handlers/brief.rs &#x60;mod tests&#x60;, add a new test &#x60;plan_reviewer_brief_renders_decision_matrix_with_three_entries&#x60; that builds a tasks-schema fixture entry (mirroring the pattern in &#x60;ac7_4_all_four_briefing_templates_render_successfully&#x60; at lines 339-425) where &#x60;plan&#x60; includes a &#x60;decision_matrix&#x60; array with 3 entries (each having decision, options [2-3 strings], chosen, rationale), renders the bundled &#x60;templates/plan-reviewer-brief.md.tpl&#x60;, and asserts: rendered contains &#x27;## Decision Matrix&#x27;; rendered contains all 3 decision names; rendered contains all 3 chosen values; rendered contains all 3 rationales.
  - Task 2.2: Add a second test &#x60;plan_reviewer_brief_omits_decision_matrix_when_absent&#x60; using the same fixture pattern but with &#x60;plan&#x60; lacking &#x60;decision_matrix&#x60; (or set to null/empty array), asserting: rendered contains the placeholder string &#x27;(no decisions recorded)&#x27; and does NOT contain stray &#x27;undefined&#x27;/error markers.
  - Task 2.3: Add a third assertion in either test (or a dedicated &#x60;plan_reviewer_brief_decision_matrix_position&#x60; test) verifying section ordering: byte-index of &#x27;## Decision Matrix&#x27; is greater than byte-index of &#x27;## Current Plan&#x27; AND less than byte-index of &#x27;## Prior Plan Reviews&#x27; in the rendered output.
  - Task 2.4: Run &#x60;cargo test --lib handlers::brief&#x60; to confirm new tests pass and existing &#x60;ac7_4_all_four_briefing_templates_render_successfully&#x60; still passes.
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;cargo test --lib handlers::brief&#x60; passes; the three new test names appear in test output.
  - [ ] AC2.2: Test &#x60;plan_reviewer_brief_renders_decision_matrix_with_three_entries&#x60; asserts presence of all 3 decision names, all 3 chosen options, and all 3 rationales in the rendered output.
  - [ ] AC2.3: Test &#x60;plan_reviewer_brief_omits_decision_matrix_when_absent&#x60; asserts the empty-state placeholder renders cleanly (no template errors, no literal handlebars markup like &#x27;{{&#x27;).
  - [ ] AC2.4: Section-position assertion confirms &#x27;## Decision Matrix&#x27; renders strictly between &#x27;## Current Plan&#x27; and &#x27;## Prior Plan Reviews&#x27;.
  - [ ] AC2.5: Pre-existing test &#x60;ac7_4_all_four_briefing_templates_render_successfully&#x60; continues to pass without modification (it does not provide a decision_matrix, exercising the empty-state path).
- **Files:** `src/handlers/brief.rs`
- **Dependencies:** Phase 1 complete (template edit landed so bundled BUNDLED_STORE_TEMPLATES picks it up)

---

## Plan Review

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable. Phase 1 edits the verified template at the correct insertion point (between line 62 and 64); Phase 2 mirrors the existing ac7_4 fixture pattern with mechanical assertions (string presence, byte-index ordering, cargo test pass). Empty-state placeholder choice matches the existing &#x27;Prior Plan Reviews&#x27; if/else pattern. Phase ordering correct (P2 depends on P1&#x27;s bundled template).
- **At:** 2026-05-03T10:57:52Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Inserted &#x27;## Decision Matrix&#x27; section in plan-reviewer-brief.md.tpl between &#x27;## Current Plan&#x27; and &#x27;## Prior Plan Reviews&#x27;. Uses {{#if plan.decision_matrix}}/{{#each}} block rendering decision/options/chosen/rationale per entry, with &#x27;_(no decisions recorded)_&#x27; fallback mirroring the Prior Plan Reviews pattern. cargo build succeeds; no other templates modified.
- **Commit:** `248182d`
- **Files:**
  - `stores/tasks/templates/plan-reviewer-brief.md.tpl`
- **At:** 2026-05-03T10:58:26Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Added two tests in src/handlers/brief.rs::tests covering plan-reviewer Decision Matrix rendering: plan_reviewer_brief_renders_decision_matrix_with_three_entries asserts all 3 names/chosen/rationales appear and section is ordered between &#x27;## Current Plan&#x27; and &#x27;## Prior Plan Reviews&#x27;; plan_reviewer_brief_omits_decision_matrix_when_absent asserts the &#x27;(no decisions recorded)&#x27; placeholder renders with no &#x27;{{&#x27; or &#x27;undefined&#x27; leaks. Shared helper render_plan_reviewer_brief_with_plan mirrors the ac7_4 fixture pattern. cargo test --lib handlers::brief: 6 passed (2 new + 4 existing including ac7_4_all_four_briefing_templates_render_successfully).
- **Commit:** `987302d8dee986e867b4cf2a44eda9efa2723c86`
- **Files:**
  - `src/handlers/brief.rs`
- **At:** 2026-05-03T11:00:36Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 5 phase ACs verified: cargo build succeeds; &#x27;## Decision Matrix&#x27; appears exactly once at line 64 of plan-reviewer-brief.md.tpl, positioned between &#x27;## Current Plan&#x27; (line 36) and &#x27;## Prior Plan Reviews&#x27; (line 81); template references plan.decision_matrix, this.decision, this.options, this.chosen, this.rationale; only the one template file changed (git diff --stat confirms 1 file, +17 lines); all existing sections (Persona, Workflow Context, Task, Contract, Current Plan, Prior Plan Reviews, Critical Actions, Gate Decisions, Adversarial Mindset) remain intact. cargo test --lib: 543 passed, 0 failed. Two minor stylistic observations noted; do not block.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[MINOR] Fallback wording style diverges from sibling section.
File: stores/tasks/templates/plan-reviewer-brief.md.tpl:78
Evidence: Decision Matrix fallback is &#x60;_(no decisions recorded)_&#x60; (parenthesized lowercase fragment), while Prior Plan Reviews fallback at line 96 is &#x60;_No prior plan reviews — first review cycle._&#x60; (capitalized sentence with em-dash and period).
Expected: Done-When (3) says &#x27;pick whichever matches existing brief patterns&#x27; — the existing pattern is a capitalized sentence.
Suggestion: Consider &#x60;_No decisions recorded — planner did not submit a decision_matrix._&#x60; for consistency with the Prior Plan Reviews fallback. Non-blocking.

[MINOR] No defensive rendering when &#x60;this.chosen&#x60; is missing on an entry.
File: stores/tasks/templates/plan-reviewer-brief.md.tpl:72
Evidence: Template emits &#x60;**Chosen:** {{this.chosen}}&#x60; unconditionally; if a planner submits a decision entry without a &#x60;chosen&#x60; field, the brief renders &#x60;**Chosen:** &#x60; with an empty value.
Expected: Phase 1 ACs do not require per-field guards, so this is non-blocking. Phase 2 (tests) may want a fixture covering this edge case.
Suggestion: Either accept (planner schema should require &#x60;chosen&#x60;) or wrap with &#x60;{{#if this.chosen}}...{{/if}}&#x60; in a follow-up. Defer to planner-side schema enforcement.

[INFORMATIONAL] Phase is a single-file 17-line template edit; the &gt;3-finding heuristic does not meaningfully apply. Both minors above are deferrable; ACs 1.1–1.5 all pass mechanically.
- **At:** 2026-05-03T10:59:33Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All ACs verified: cargo test --lib handlers::brief shows 6 passed including both new tests (plan_reviewer_brief_renders_decision_matrix_with_three_entries, plan_reviewer_brief_omits_decision_matrix_when_absent) and the pre-existing ac7_4_all_four_briefing_templates_render_successfully. Section ordering assertion (Current Plan &lt; Decision Matrix &lt; Prior Plan Reviews) is correct. 0 critical, 0 major, 3 minor (test coverage gaps + AC2.1 wording inconsistency).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Tests do not assert that the per-entry &#x60;options&#x60; are rendered as bullet points.
File: src/handlers/brief.rs (new tests, ~lines 484-579)
Evidence: The Done When (1) specifies &quot;options as bullet points&quot; but the three-entry test only asserts decision names, chosen values, and rationales — the options array (e.g., &#x27;postgres&#x27;, &#x27;in-memory&#x27;, &#x27;tera&#x27;, &#x27;tempfile path&#x27;, &#x27;env var&#x27;) is never asserted to appear in the rendered output.
Expected: Some assertion that non-chosen options also render (otherwise a buggy template that drops options would still pass).
Suggestion: Add e.g. &#x60;for opt in [&quot;postgres&quot;, &quot;in-memory&quot;, &quot;tera&quot;, &quot;tempfile path&quot;, &quot;env var&quot;] { assert!(rendered.contains(opt)); }&#x60;. Non-blocking since P1 template was reviewed; can be deferred.

[MINOR] Tests do not assert that the chosen option is visually &quot;called out&quot; distinctly.
File: src/handlers/brief.rs (plan_reviewer_brief_renders_decision_matrix_with_three_entries)
Evidence: Done When (1) says &quot;the chosen option called out&quot; — the test only asserts the chosen string appears, not that it is marked (e.g., bolded, prefixed with **Chosen:**, etc.). A template that listed the chosen value identically to the options would still pass.
Expected: Assertion on a callout marker (e.g., &#x60;**Chosen:** sqlite&#x60; or similar).
Suggestion: Inspect the P1 template format and assert on the callout token, e.g., &#x60;assert!(rendered.contains(&quot;**Chosen:** sqlite&quot;))&#x60;.

[MINOR] AC2.1 wording inconsistency (planning, not executor defect).
File: phase ACs as written
Evidence: AC2.1 says &quot;the three new test names appear in test output&quot; but AC2.2/AC2.3 describe only two new tests; AC2.4 is described as an in-test assertion, not a separate test. The Done When (5) also describes only two new test scenarios.
Expected: Two new tests (matching AC2.2 + AC2.3).
Suggestion: No executor action — flag for the planner that AC2.1&#x27;s &quot;three&quot; should read &quot;two&quot;. The substantive coverage requirement is met.

[INFORMATIONAL] Pre-existing unused-import warnings (add.rs:370, transition.rs:413, update.rs:167) surfaced during cargo test but are unrelated to this phase.
- **At:** 2026-05-03T11:01:30Z

---

## Completion
- **In Review:** 2026-05-03T11:01:56Z — awaiting human GO/NO_GO

