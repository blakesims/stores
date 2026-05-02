# Plan Review — T011: Document the wrapper boundary in philosophy.md

**Reviewer:** plan-reviewer agent
**Date:** 2026-05-02
**Gate:** READY

## Verdict

**READY.** The plan is a clean, scope-disciplined match for the Intent Contract's DONE_WHEN. No revisions required. The executor can proceed.

The plan resists the trap I was instructed to actively guard against — the planner did not over-engineer a one-paragraph doc edit into multi-phase scaffolding. It is a single phase, single file, with a precise required-content checklist that maps 1:1 to the three DONE_WHEN clauses.

## DONE_WHEN coverage

| DONE_WHEN clause | Plan coverage | Verdict |
|---|---|---|
| C1 — names worktree provisioning, project setup scripts, and observing wrappers as outside the substrate | Phase 1 required-content C1 explicitly enumerates all three, with framing constraint ("stores does not own these" not "stores might add these later") | Covered, framing constraint adds value beyond DONE_WHEN literal |
| C2 — substrate has exactly one write path; wrappers share the same authority surface as any other client | Phase 1 required-content C2 restates one-write-path, names "any wrapper — autonomous or observed by a human", and adds the concrete operational consequence ("not `actor: ai_autonomous`; cannot write rows directly; if it wants to act, it issues CLI commands like anyone else") | Covered, with operational specificity that makes verification easier |
| C3 — names and resists the "pause drive" trap | Phase 1 required-content C3 names the trap, requires "pause drive" as the canonical example, and requires the reasoning (push orchestration up a level, break atomicity, introduce unverified second write path) | Covered, the required reasoning prevents the executor from naming the trap without explaining why it's a trap |
| Length: 1–3 paragraphs, prose only | Phase 1 tone/length item enforces this; Decision Matrix row "Length budget" picks (b) explicitly tracking DONE_WHEN; Decision Matrix row "Code samples / bullets" forbids them | Covered |
| Renders as valid Markdown | Acceptance criterion: "File renders as valid Markdown (no broken headings, no orphaned list markers, no unclosed emphasis)" | Covered |

All five demands of DONE_WHEN are reachable from the plan's acceptance criteria without inference.

## Plan quality

**Acceptance criteria are mechanically verifiable.** A code-reviewer can:
- `git diff` to confirm only `docs/philosophy.md` is touched
- Read the new section to check C1/C2/C3 are present (each is concrete enough to spot)
- Count paragraphs (≤3)
- Grep for code fences / new bullets / sub-headings inside the section
- Diff the surrounding sections to confirm at most one transition sentence changed

No criterion requires subjective judgment beyond the unavoidable "tone matches" check, which is bounded by the rest of the file (44 lines, declarative voice, opinionated, no hedging — verifiable by reading).

**The Decision Matrix is unusually thorough for a one-phase doc task.** Five decisions are recorded with options-considered and rationale: placement, title, phase count, length budget, and surrounding-section editing scope. Each closes a question the executor would otherwise have to answer on the fly. The "phase count" row is particularly good — it pre-empts the over-engineering trap by name.

**Scope discipline is tight.** Out-of-scope list explicitly excludes reorganizing existing sections, creating new doc files, code/schema/CLI/test changes, and worklog/refs updates beyond standard close-out. This matches the Intent Contract's out-of-scope list verbatim and adds the worklog clarification (which is correct — task-close worklog is not the executor's job).

## Hidden assumptions / risks

None that block the gate. Two minor observations the executor should be aware of (carry-forward notes below):

1. **Voice match is the only soft criterion.** The acceptance criterion "reads as a natural continuation of 'What falls out' and a natural setup for 'The deeper bet'" is the only criterion that resists pure mechanical check. Mitigation: the existing 44-line philosophy.md is short enough that the executor can read it in full before writing, and the Decision Matrix already pinned tone constraints (declarative, opinionated, no hedging, no bullet padding inside the section). Acceptable risk for a doc task.

2. **"Light editing of surrounding sections" is bounded but ambiguous in count.** Plan caps it at "at most one transition sentence at the boundary (and only if needed for flow)." This is fine — the cap is verifiable, and the "only if needed" framing means the executor's default should be zero edits. Worth flagging in the carry-forward so the executor doesn't reflexively add transitions.

## Risk of misaligned output

Low. The required-content checklist (C1/C2/C3) ties each DONE_WHEN clause to a concrete prose obligation, including the canonical "pause drive" example for C3. The likeliest miss-the-spirit failure modes — burying the trap, hedging on the one-write-path claim, or letting the new section read as "stores might add wrappers later" rather than "stores does not own wrappers" — are each addressed by an explicit constraint in the plan body, not just the acceptance criteria.

The one residual risk: an executor could technically satisfy C2 by restating "exactly one write path: the CLI" without making the wrapper-specific point that an outer Claude Code instance is *just another CLI client*. The plan addresses this with the explicit "not `actor: ai_autonomous`; it cannot write rows directly" requirement, but the executor should be told to pay attention to the wrapper-context framing, not just restate the existing philosophy claim. Captured below.

## Unresolved questions

None. The Intent Contract listed one open decision (section title); the plan resolved it in the Decision Matrix with rationale. No design calls remain implicit.

## Scope creep check

None. The plan is strictly inside the Intent Contract's "in scope" boundary. The acceptance criterion `git diff touches only docs/philosophy.md` enforces this mechanically.

## Carry-forward notes for the executor

1. **Read the full `docs/philosophy.md` (44 lines) before drafting.** Voice-match is the only soft criterion in the acceptance list; reading the whole file is the cheapest way to internalize the cadence (declarative sentences, occasional bolded phrases like **"DB-as-truth + framework-as-engine"**, no hedging words like "perhaps" / "might want to" / "could").

2. **Default to zero edits outside the new section.** The plan permits up to one transition sentence at the boundary, but only if the seam reads abruptly. Try the insertion with no surrounding edits first; only add a transition if the rendered doc actually reads worse without one.

3. **For C2, make the wrapper-context point explicit, not just restated philosophy.** The existing doc already says "exactly one write path: the CLI" (line 15). Your job in C2 is not to repeat that — it's to extend it: an observing Claude Code wrapper is *just another CLI client*, with no special schema role, no `actor: ai_autonomous`, no privileged write path. If the new section's C2 paragraph could be deleted without losing wrapper-specific information, it has not done its job.

4. **For C3, the canonical example "pause drive" must appear verbatim or near-verbatim.** It is the load-bearing concrete instance that distinguishes "we resist this trap" from generic anti-coupling boilerplate. The reasoning (orchestration pushed up a level, atomicity broken, unverified second write path) must be present, not just the trap name.

5. **Section heading is `## What's outside the substrate`** (Decision Matrix row "Section title", choice (a)). Do not deviate without recording a new Decision Matrix row.

6. **Placement is between the existing `## What falls out` (currently lines 33–38) and `## The deeper bet` (currently line 40).** Confirmed against the file as it stands today.
