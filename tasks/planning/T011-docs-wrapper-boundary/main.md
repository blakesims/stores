# T011: Document the wrapper boundary in philosophy.md

## Meta
- **Status:** PLANNING
- **Created:** 2026-05-02
- **Last Updated:** 2026-05-02
- **Blocked Reason:** —

## Task

Add a section to `docs/philosophy.md` documenting what is **outside** the substrate — specifically that worktree provisioning, project hooks, and observing wrappers (e.g. a Claude Code instance running the stores CLI) are not stores' responsibility. The substrate has exactly one write path (the CLI). Any wrapper, autonomous or observed, has the same authority surface as any other CLI client.

The section must explicitly name the trap to resist: giving the outer agent privileged channels (e.g. "the wrapping orchestrator can pause drive"). That pushes orchestration up a level and breaks the substrate's atomicity.

This is task #1 of a four-task ship plan emerging from the 2026-05-02 worklog note (`docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md`). Shipping it first prevents anyone from giving the outer agent special powers while later substrate work is in flight.

## Intent Contract

**Executive intent.** Pin the substrate-vs-wrapper boundary in writing so future contributors don't reinvent it or violate it. The boundary already follows from philosophy §"DB-as-truth + framework-as-engine," but it is implicit; the wrapper question makes it explicit.

**DONE_WHEN.** `docs/philosophy.md` contains a new section explaining: (1) worktree provisioning, project setup scripts, and observing wrappers live outside the substrate; (2) the substrate has exactly one write path (the CLI), and wrappers — autonomous or observed — share the same authority surface as any other client; (3) the trap of giving the outer agent privileged channels (e.g. "pause drive") is named and resisted. Section is 1–3 paragraphs, prose only, no code samples needed. Renders as valid Markdown.

**Scope boundaries.**
- **In scope:** New section in `docs/philosophy.md`. Light editing of surrounding sections only if needed to make the new section flow.
- **Out of scope:** Reorganizing existing philosophy.md sections; creating new doc files (e.g. `docs/wrappers.md`); changing CLI behavior; implementing any wrapper; writing tests; touching any code.

**Proposed approach.** Add a new section after "What falls out" and before "The deeper bet" — so it reads as boundary clarification before the philosophical takeaway. (Planner may pick a different placement; rationale should be recorded in the Decision Matrix.)

**Risks / assumptions.** None significant. Pure doc change. The only "risk" is over-explaining — keep it tight. 1–3 paragraphs is the budget.

**Open decisions.** Section title: "What's outside the substrate" vs "The wrapper boundary" vs something else. Planner to choose; matter-of-fact title is fine.

---

## Plan
_Planner agent fills this section._

### Objective
_What we're trying to achieve._

### Scope
- **In Scope:** ...
- **Out of Scope:** ...

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | ... | Low/Medium/High |

### Phase Details

#### Phase 1: [Title]
- **Objective:** ...
- **Files to modify:** ...
- **Acceptance Criteria:**
  - [ ] ...

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| ... | ... | ... | ... |

---

## Plan Review
_Plan-reviewer agent fills this section._

- **Gate:** READY | NEEDS_WORK | NOT_READY
- **Open Questions Finalized:** —
- **Issues Found:** —

> Details: plan-review.md

---

## Execution Log
_Executor agent fills this section per phase._

### Phase 1: [Title]
- **Status:** PENDING | IN_PROGRESS | COMPLETE | BLOCKED
- **Started:** —
- **Completed:** —
- **Commits:** —
- **Files Modified:** —
- **Notes:** —

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

### Phase 1
- **Gate:** PASS | REVISE | FAIL
- **Issues Found:** —
- **Revision Count:** 0/3

> Details: code-review-phase-1.md

---

## Completion
_Final summary when task is complete._

- **Completed:** [DATE]
- **Summary:** ...
- **Commits:** ...
- **Lessons Learned:** ...
