# Code Review — T011 Phase 1 (cycle 1)

**Reviewer:** code-reviewer
**Date:** 2026-05-02
**Phase:** 1 — Insert "What's outside the substrate" section
**Gate:** PASS
**Revision Count:** 1/3

---

## Method

Doc-only change. Verified by:

1. `git diff HEAD -- docs/philosophy.md` — bounded change, +8 lines, no deletions.
2. `git diff HEAD --stat` — only `docs/philosophy.md` and `tasks/active/T011-docs-wrapper-boundary/main.md` (Execution Log entry) modified.
3. Full read of `docs/philosophy.md` end-to-end (53 lines) to verify in-context cohesion, not just isolated section.
4. `grep -n '^##'` to confirm heading structure and ordering.
5. Walked the executor's Required Content Checklist (C1/C2/C3) against the prose word-for-word.

---

## Verification against acceptance criteria

| # | Criterion | Status | Evidence |
|---|-----------|--------|----------|
| 1 | New `## What's outside the substrate` heading positioned between `## What falls out` and `## The deeper bet` | PASS | philosophy.md L40 (new), L33 (`## What falls out`), L48 (`## The deeper bet`). Heading order via `grep -n '^##'` confirmed. |
| 2 | C1 — names what's outside (worktree provisioning, project setup scripts, observing wrappers); framing is "stores does not own these" | PASS | L42: "Worktree provisioning, project setup scripts, and observing wrappers — a Claude Code instance watching a long-running session, an outer orchestrator that spawned the whole thing — live outside the substrate. `stores` does not own these. They wrap `stores`; `stores` does not wrap them. This is not a gap to fill later; it is the correct boundary." Three required nouns present; framing is correct (ownership boundary, not "might add later"). |
| 3 | C2 — substrate has one CLI write path; wrappers share same authority surface; specifically calls out wrapper is not `actor: ai_autonomous` and cannot write rows directly | PASS | L44: "the substrate has exactly one write path — the CLI — and **any wrapper shares the same authority surface as every other CLI client**. An autonomous outer agent is not `actor: ai_autonomous` at the schema level; it has no privileged row-write path; it cannot reach into the DB directly. If it wants to act on what it observes, it issues CLI commands, the same commands a human operator would type." All three sub-claims present (one write path; same surface as other clients; not `actor: ai_autonomous`; no direct row-write). Strengthened by the closing reminder that "The schema still enforces field-level actor constraints, required-when predicates, and lifecycle guards regardless of who is on the other end of the CLI invocation." |
| 4 | C3 — "pause drive" appears verbatim/near-verbatim; reasoning explicit (orchestration pushed up a level, atomicity broken, unverified second write path) | PASS | L46: `"let the wrapping orchestrator pause `drive`, or inject a state transition, or signal the substrate through some side path."` "pause `drive`" appears (with `drive` in inline code, which is appropriate — `drive` is a CLI verb name). All three reasons explicitly named: "push orchestration up a level, outside the schema's reach; break the substrate's atomicity, since a pause is now an unverified second write path the DB does not know about; and introduce a class of failures the framework cannot detect or log." Closes with the resolution: "the outer layer drives everything through the CLI, same as anyone else." |
| 5 | 1–3 paragraphs of prose | PASS | Three paragraphs (L42, L44, L46). Within DONE_WHEN budget. C3 in its own paragraph is a deliberate executor choice (recorded in Execution Log Notes) to avoid burying the trap; sound reasoning. |
| 6 | No code fences, no bullet lists, no sub-headings inside the new section | PASS | Inline backticks for `stores`, `actor: ai_autonomous`, `drive` — these are inline code spans, not fenced code blocks; allowed and idiomatic for the doc. No `-`/`*`/`1.` list markers in the new section. No `###` sub-headings. |
| 7 | No existing section reorganized; edits outside the new section limited to at most one transition sentence | PASS | Diff is purely additive: 8 inserted lines and zero deletions inside `docs/philosophy.md`. `## What falls out` and `## The deeper bet` bodies unchanged. Executor opted for zero transition sentences (carry-forward note 2: defaulted to zero) — the seam reads cleanly without one. |
| 8 | Only `docs/philosophy.md` modified by Phase 1 | PASS | `git diff HEAD --stat`: `docs/philosophy.md` (+8/-0) and `tasks/active/T011-docs-wrapper-boundary/main.md` (+8/-8 — Execution Log entry; required by tasks/CLAUDE.md, not executor "code"). No other files touched. No new files created. |
| 9 | Markdown validity — no broken headings, no orphaned list markers, no unclosed emphasis. Specifically: `## The deeper bet` heading intact (executor flagged a near-miss in handoff Notes) | PASS | `grep -n '^##'` confirms 5 H2 headings in correct order, including `## The deeper bet` at L48. The executor's reported near-miss (heading consumed by first Edit, restored in follow-up) was successfully recovered. Bold span on L44 (`**any wrapper shares the same authority surface as every other CLI client**`) opens and closes correctly. All inline backtick spans on L44 and L46 are balanced. No orphan list markers. |
| 10 | Voice match — declarative, opinionated, no hedging | PASS | Section reads in the same register as the rest of the doc. Examples: "This is not a gap to fill later; it is the correct boundary." / "The wrapper is just another client." / "The trap to resist is..." / "The temptation is real — it feels like coordination. What it actually does is..." Matches existing aphoristic style (cf. L31: "The thing you can't break in the database, you don't have to remember to enforce in process."). No "may", "might", "could potentially". The em-dash–driven cadence and use of bold for the load-bearing clause both mirror the rest of the doc. |

---

## Issues Found

None blocking. Three small observations, none of which warrant REVISE:

1. **`drive` is in inline code in "pause `drive`".** The review brief specified "pause drive" verbatim or near-verbatim. The executor wrote `pause `drive`` (backtick-wrapped). This is correct stylistic choice — `drive` is a CLI verb name and is set in code throughout the codebase. It satisfies "near-verbatim" cleanly. Not an issue.

2. **C3 occupies its own paragraph rather than being folded into C2.** The plan's length budget allowed 1–3 paragraphs and the Decision Matrix explicitly anticipated this option ("Three required claims plausibly fit in one tight paragraph but can also reasonably span two or three if the executor wants to give the trap (C3) its own paragraph for emphasis"). Executor took the 3-paragraph option deliberately and recorded the rationale in the Execution Log. This is within plan, and arguably the stronger choice — the trap reasoning is the load-bearing claim that prevents future drift, and burying it inside a longer C2 paragraph would soften it.

3. **No transition sentence between `## What falls out` and the new section, nor between the new section and `## The deeper bet`.** The plan permitted up to one transition sentence at the boundary if needed; executor defaulted to zero (carry-forward note 2). I read the seam in both directions and concur: each H2 stands cleanly on its own and the new section reads as a natural continuation of the "consequences" arc. No revision needed.

---

## Cynical-reviewer notes

The brief warned that doc-only changes are easy to under-review and that the heading-drop near-miss is exactly the kind of thing that slips through. I checked `## The deeper bet` first. It is present at L48 with the correct text, correct level, and the body below it (L50, L52) is unchanged from master. The executor's recovery worked.

I also looked specifically for softening of the three claims, since the brief flagged that as REVISE-worthy. None found:

- C1's framing is "stores does not own these. They wrap `stores`; `stores` does not wrap them." — this is the strongest possible directional claim and matches the plan's "stores does not own these" / "these wrap stores, not the other way around" requirement exactly.
- C2 doesn't just restate the one-write-path principle; it bolds the load-bearing clause and follows up with three negations ("is not `actor: ai_autonomous`", "has no privileged row-write path", "cannot reach into the DB directly") and then a positive restatement ("it issues CLI commands, the same commands a human operator would type"). The schema-enforcement reminder at the end is the kind of belt-and-braces phrasing the rest of the doc uses.
- C3 names the trap ("pause `drive`", state injection, side-channel signaling), explains why the temptation feels real ("it feels like coordination"), and gives all three structural reasons it must be resisted. No softening.

The diff is bounded. The Markdown is valid. The voice matches. The placement is exactly as planned. All ten acceptance criteria pass cleanly on the first cycle.

---

## Gate

**PASS** — Phase 1 acceptance criteria fully satisfied; no revisions requested.

This is the only phase in T011's plan, so PASS here completes the task. Recommend orchestrator commit `docs/philosophy.md` and the Execution Log entry, set `## Meta` Status to `COMPLETE`, and run the standard task-completion close-out (worklog note via `docs/worklog/new-note.sh`, GTM update, folder move to `tasks/completed/`).
