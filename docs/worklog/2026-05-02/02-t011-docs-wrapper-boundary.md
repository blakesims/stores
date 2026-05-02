# T011 Docs Wrapper Boundary

**Date:** 2026-05-02
**Type:** note

## Summary

T011 shipped: a 3-paragraph `## What's outside the substrate` section in `docs/philosophy.md` pinning the substrate-vs-wrapper boundary. PASS first cycle on plan review, execution, and code review. CodeRabbit Stage 6 surfaced one workflow-level issue (Status flipped to COMPLETE before Completion section was filled) — addressed at task close.

This is task #1 of the 4-task ship plan from the morning's analysis (`01-real-world-workflow-takeover-analysis.md`). Three remain: workspace_path + next-id verb (T012), reviewer envelope + storage schema migration (T013), framework write-path for envelope-lodged observations + brief overlay (T014).

## Details

### Why this task existed

The morning's analysis concluded that a wrapping outer agent (e.g. a Claude Code instance running the stores CLI to observe a drive run) has no schema role — it's just another CLI client. The trap to actively resist is giving such a wrapper privileged channels (e.g. "the wrapping orchestrator can pause drive"), which would push orchestration up a level and break the substrate's atomicity. The principle followed from existing philosophy but was implicit; T011 made it explicit.

### What shipped

`docs/philosophy.md` gained a 3-paragraph section between `## What falls out` and `## The deeper bet`:

- **P1 (C1):** Names worktree provisioning, project setup scripts, and observing wrappers as outside the substrate. Framing: "they wrap stores; stores does not wrap them."
- **P2 (C2):** Extends the one-write-path principle into the wrapper context — an autonomous outer agent is not `actor: ai_autonomous`, has no privileged row-write path, and issues CLI commands like any other client.
- **P3 (C3):** Names and resists the "pause `drive`" trap with all three structural reasons: orchestration pushed up a level, atomicity broken (unverified second write path), and a class of failures the framework cannot detect or log.

### How the workflow ran

| Stage | Result | Notes |
|---|---|---|
| Planner | Single-phase plan, Low complexity | Decision Matrix explicitly defended "don't manufacture more phases for trivial scope" |
| Plan-reviewer | READY first cycle | Six carry-forward notes for the executor (voice match, "pause drive" verbatim, etc.) |
| Executor | Phase 1 COMPLETE | Heading-drop-during-Edit near-miss caught and restored before commit |
| Code-reviewer | PASS first cycle (1/3) | Independently verified the heading restoration; all 10 ACs met |
| CodeRabbit Stage 6 | 1 finding | Workflow-level: Status COMPLETE before Completion section filled |

### Lessons

- **Voice-match was load-bearing.** Reading the full target file before drafting (carry-forward note 1 from plan-review) was what made first-cycle PASS possible. Skipping the read would have produced something competent but off-key.
- **One-phase plans for trivial scope work** when the Decision Matrix explicitly defends the choice. The plan-reviewer found nothing to revise because there was nothing to revise — the smallest possible plan that satisfies DONE_WHEN is the right plan.
- **Heading-drop near-miss** reinforces that doc edits with structured surrounding content need post-edit grep verification. The executor caught it themselves; the reviewer independently confirmed. Worth pinning: any Edit that touches near a heading should be followed by `grep -n '^#' file` to verify structure.
- **Orchestrator should fill `## Completion` *before* declaring task COMPLETE**, not after. CodeRabbit caught this. Worth updating tasks/CLAUDE.md to make the ordering explicit.

### Maps to DONE_WHEN

All three DONE_WHEN clauses satisfied:
- (1) Worktree provisioning, project setup scripts, observing wrappers explicitly named as outside.
- (2) One write path (CLI), wrappers share the same authority surface — explicit including the `actor: ai_autonomous` reference.
- (3) "pause `drive`" named verbatim with all three structural reasons.

## Follow-ups

- **T012 (next):** workspace_path field on tasks + tasks next-id verb. Bundled because both serve the project-script-wraps-stores pattern. ~half-day each, scoped in `01-real-world-workflow-takeover-analysis.md`.
- **Tiny CLAUDE.md update** worth considering: pin the ordering "fill Completion section *before* setting Status: COMPLETE" so the CR Stage 6 finding doesn't recur on every future task. Out of scope for T011 but worth a follow-up commit.
- **T013 / T014** (reviewer-notes propagation, framework write-path) queued behind T012.
