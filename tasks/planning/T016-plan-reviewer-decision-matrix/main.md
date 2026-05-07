# T016: Brief plumbing - render planner decision_matrix in plan_reviewer brief

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T10:55:03Z
- **Last Updated:** 2026-05-03T10:57:12Z
- **Current Phase:** 
- **Current Cycle:** 
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

