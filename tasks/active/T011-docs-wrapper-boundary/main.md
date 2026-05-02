# T011: Document the wrapper boundary in philosophy.md

## Meta
- **Status:** EXECUTING_PHASE_1
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

### Objective

Add one new prose section to `docs/philosophy.md` titled **"What's outside the substrate"** that pins the substrate-vs-wrapper boundary. The section names what lives outside stores (worktree provisioning, project setup scripts, observing wrappers), restates the one-write-path principle in the wrapper context, and explicitly names and resists the trap of giving an outer agent privileged channels (e.g. "pause drive"). 1–3 paragraphs, prose only.

### Scope

- **In Scope:**
  - Insert one new section into `docs/philosophy.md` between the existing "What falls out" section and the existing "The deeper bet" section.
  - Light editing of the immediately surrounding sections (final sentence of "What falls out", opening of "The deeper bet") only if needed to make the new section flow.
- **Out of Scope:**
  - Reorganizing or rewriting any existing philosophy.md section.
  - Creating new documentation files (e.g. `docs/wrappers.md`).
  - Any code, schema, CLI, or test changes.
  - Worklog/refs updates beyond the standard task-completion worklog note (handled at task-close time, not by the executor of this phase).

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Write and insert the "What's outside the substrate" section into `docs/philosophy.md` | Low |

### Phase Details

#### Phase 1: Insert "What's outside the substrate" section

- **Objective:** Add the new section, in 1–3 paragraphs of prose, hitting all three required claims from DONE_WHEN, in a voice that matches the rest of `docs/philosophy.md` (declarative, opinionated, no hedging, no code samples, no bullet padding).

- **Files to modify:**
  - `docs/philosophy.md` — insert new `## What's outside the substrate` heading and body between the existing `## What falls out` section (currently lines 33–38) and the existing `## The deeper bet` section (currently line 40). Light touch-up of the joining prose only if the transition reads abrupt.

- **Required content (executor checklist — every item must be present in the final prose):**
  - [ ] **C1 — Names what's outside.** The section explicitly enumerates that worktree provisioning, project setup scripts, and observing wrappers (a Claude Code instance — or any other agent — running the stores CLI) live outside the substrate. The framing is "stores does not own these" / "these wrap stores, not the other way around" — not "stores might add these later."
  - [ ] **C2 — Restates one-write-path in the wrapper context.** The section states that the substrate has exactly one write path (the CLI), and that any wrapper — autonomous or observed by a human — has the same authority surface as any other CLI client. A wrapper is not `actor: ai_autonomous`; it cannot write rows directly; if it wants to act on what it sees, it issues CLI commands like anyone else.
  - [ ] **C3 — Names and resists the "special powers" trap.** The section explicitly names and rejects the temptation to give the outer/wrapping agent privileged channels — uses "pause drive" as the canonical example of the trap. The reasoning given: such channels push orchestration up a level, break the substrate's atomicity, and introduce an unverified second write path.
  - [ ] **Tone/length:** 1–3 paragraphs of prose. No code blocks, no bullet lists inside the new section, no headings other than the section heading itself. Voice matches surrounding sections (declarative, opinionated, fits the existing Markdown style of philosophy.md).

- **Acceptance Criteria (verifiable by code-reviewer against DONE_WHEN):**
  - [ ] `docs/philosophy.md` contains a new `## What's outside the substrate` heading positioned between the existing `## What falls out` and `## The deeper bet` sections.
  - [ ] All three required-content items (C1, C2, C3 above) are present in the section body, each clearly identifiable.
  - [ ] Section body is 1–3 paragraphs of prose. No code fences. No new bullet lists or sub-headings inside the section.
  - [ ] No existing section in `docs/philosophy.md` has been reorganized or substantively rewritten. Edits outside the new section are limited to at most one transition sentence at the boundary (and only if needed for flow).
  - [ ] No new files created. No code, schema, CLI, or test changes. `git diff` touches only `docs/philosophy.md`.
  - [ ] File renders as valid Markdown (no broken headings, no orphaned list markers, no unclosed emphasis). Reading the rendered doc top-to-bottom, the new section reads as a natural continuation of "What falls out" and a natural setup for "The deeper bet."

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| Section placement | (a) Between "What falls out" and "The deeper bet". (b) After "Three enforcement moments", before "What falls out". (c) New section at end of file, after "The deeper bet". | (a) | "What falls out" already enumerates consequences of the substrate choice; the wrapper boundary is another consequence (a clarifying corollary), so it sits naturally as the last consequence before the philosophical close-out. Placing it before "What falls out" (option b) interrupts the established problem → move → enforcement → consequences arc. Placing it at the end (option c) means readers finish on a boundary clarification rather than the intended philosophical takeaway, which weakens the doc's close. The Intent Contract proposed (a); planner concurs. |
| Section title | (a) "What's outside the substrate". (b) "The wrapper boundary". (c) "What stores doesn't own". | (a) | Parallels the sibling "What falls out" both grammatically and structurally (both are framed by content, both use everyday English). "The wrapper boundary" (b) leans on jargon ("wrapper") that the reader would need the section itself to define — bad section-title hygiene. "What stores doesn't own" (c) is too negative-framed and reads as defensive. (a) is matter-of-fact and self-explanatory. |
| Phase count | One phase vs. splitting drafting and integration. | One phase. | Scope is a single doc edit of 1–3 paragraphs in one file. Splitting drafting from insertion would manufacture process for its own sake and add a handoff with no value. The phase's required-content checklist plus acceptance criteria are precise enough to gate executor output without an internal phase boundary. |
| Length budget | (a) Strict single paragraph. (b) 1–3 paragraphs as DONE_WHEN allows. (c) Open-ended. | (b) | DONE_WHEN explicitly permits 1–3 paragraphs. Three required claims (C1, C2, C3) plausibly fit in one tight paragraph but can also reasonably span two or three if the executor wants to give the trap (C3) its own paragraph for emphasis. Forcing one paragraph (a) risks the trap getting buried; open-ended (c) invites the over-explaining the Intent Contract warns against. (b) tracks DONE_WHEN exactly. |
| Whether to alter surrounding sections | (a) No edits outside the new section. (b) Allow up to one transition sentence at the boundary if flow requires. (c) Open editing of "What falls out" / "The deeper bet" for cohesion. | (b) | (a) risks an awkward seam if the existing closing sentence of "What falls out" or opening of "The deeper bet" reads abruptly against the new section. (c) is out of scope per the Intent Contract. (b) is the minimum change needed to honor "Light editing of surrounding sections only if needed for flow." Acceptance criteria cap this at one transition sentence so the constraint is verifiable. |
| Code samples / bullets inside the new section | (a) Permit if natural. (b) Forbid — prose only. | (b) | DONE_WHEN says "prose only, no code samples needed." Bullets inside the section would also visually distinguish it from "The deeper bet" (also pure prose) and would invite list-padding. Keeping it pure prose enforces the tightness the Intent Contract calls for. The section heading itself is, of course, still a heading. |

---

## Plan Review
_Plan-reviewer agent fills this section._

- **Gate:** READY
- **Open Questions Finalized:** None — Intent Contract's lone open decision (section title) was resolved in the Decision Matrix with rationale.
- **Issues Found:** None blocking. Six carry-forward notes recorded for the executor (voice-match approach, default-zero surrounding edits, C2 must extend not restate, "pause drive" must appear, heading + placement confirmed).
- **Summary:** Plan is a scope-disciplined 1:1 match for DONE_WHEN clauses C1/C2/C3, with mechanically verifiable acceptance criteria and a Decision Matrix that pre-empts the over-engineering trap. Executor can proceed.

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
