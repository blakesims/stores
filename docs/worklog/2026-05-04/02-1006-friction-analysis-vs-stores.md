# 1006 Friction Analysis Vs Stores

**Date:** 2026-05-04
**Type:** note

## Summary

Mapped the 6 friction roots an agent surfaced from real client work in 10.06 (`/home/blake/repos/clients/10.06-wt/10.06-main/.claude/skills/ledger-friction-design.md` — 27 uncontracted ledger entries, decision fatigue, `/hotfix:batch` racing T2-direct-to-main) against the stores substrate. The analysis is the realistic-pull pressure stores' philosophy explicitly invites (`docs/philosophy.md` § *Pull from real use*). Two of the six are already structurally solved by stores (#1 mixed-bag → three sibling stores; #4 T2-on-main serialization → auto-scaffold worktree per task, T020 P4). One is half-solved with bones in place (#3 in-flight model → `claimed_by` / `locked_by` / `linked_observations` / `cycles.executor.files_changed` exist; `touches_files` declaration + planning-arrival conflict guard are missing). Three are unaddressed (#2 ratification cost uniformity; #5 staleness decay; #6 intake routing).

Deeper learning: the 10.06 design landed on "fleet aggregator + conflict-aware /focus walk" (a pre-flight check pattern with the orchestrator-AI as fleet planner). Stores' philosophy says the right shape is a write-time guard — declare `touches_files` at contract draft, let the schema reject the conflicting `planning → ready` transition. Same outcome, no race, no orchestrator intelligence at plan time. The orchestrator-as-fleet-planner is exactly the back-channel anti-pattern philosophy.md warns against; the substrate must answer "what's next?" via `next-action` queries, not via outer-layer reasoning.

The `cargo install` leak was caught in worklog 07 (2026-05-03) before 10.06 surfaced it; T2-direct-to-main was retired the same way. Both are evidence the realistic-pull doctrine works — generalizations that survive only the self-build leak; pulling on 10.06 catches them. The sixth issue (intake routing) is interesting: 10.06 has the Q1/Q2/Q3 filing rubric in a doc; stores has it scattered across three parallel `add` verbs the human picks among. A single `stores file <text>` verb that runs the rubric and dispatches would match the philosophy ("schema is the cognitive center") better.

## Details

### The 6 friction roots and stores' state per each

| 10.06 issue | Stores' state | Verdict |
|---|---|---|
| #1 Ledger conflates 7 primitive types | Three sibling stores (`observations` / `gate` / `tasks`); `intent_contract.type ∈ {work, investigation}`; `tier_hint` 1st-class | **Mostly solved structurally** |
| #2 Ratification cost uniform across tier | `required_when: contract_state == 'ready'` fires identically for L094 and L328 | **Not solved** — same bottleneck shape |
| #3 No rolling model of in-flight work | `tasks.claimed_by` (framework), `observations.locked_by` (framework), `linked_observations` (list_fk), `cycles.executor.files_changed` (post-hoc) | **Half-solved** — LID-claim primitive there; file-overlap declaration not |
| #4 T2-direct-to-main serialization | Every `ai_autonomous` task gets its own worktree via `auto-scaffold` (T020 P4); no inline-on-main path | **Solved by design** — caught in worklog 07 before 10.06 surfaced it |
| #5 Stale entries don't decay | `priority`, `priority_rank_at`, `scheduled_for` fields exist; no policy reads them | **Not solved** — pure CLI ergonomics gap |
| #6 Skill choice implicit in framing | `next-action` per-row drives state→agent dispatch; no intake router | **Half-solved at row level**, missing at intake level |

### Schema-level moves to close the gaps

#### Close #3 (the deepest)

Add `touches_files: list<text>` to `observations.intent_contract` and `tasks.contract`. Add a transition guard on `tasks: planning → ready` (or earlier — at `auto-scaffold` time) that rejects when another live task's `touches_files` intersects. Same shape as the existing `current_phase < plan.phases.length` guard. The rejection error itself names the conflicting `T###`s; a `stores tasks next-action --suggest-non-conflicting` query is a thin read on top.

This is the right shape per philosophy.md § *Two-gate operational frame*: **conflict caught at write-time by a schema guard, not by a pre-flight checker that can race**. The 10.06 design proposed `./dev fleet` as a read-time aggregator. Stores' equivalent should reject the spawn at write time — same outcome, no race condition, no orchestrator-AI carrying conflict-detection logic.

#### Close #2 (ratification cost uniformity)

Make `required_when` predicates tier-aware:

```yaml
required_when: "intent_contract.contract_state == 'ready' AND intent_contract.tier_hint != 'T1'"
```

T1 disposals already have a shortcut transition `open → wont_fix` at `actor: ai_with_human` honor-system (observations/schema.yaml:24-26). What's missing is the *batch surface* — a `stores observations bulk-disposition` verb that walks N rows applying `wont_fix` / `close_as_addressed` with a single ratification per disposition class. CLI ergonomics, not schema.

Stronger move worth considering: `tier_hint` should default to `T1` when the source is autonomous draft, requiring no further fields until the human steps it up. Default path becomes cheap; ratification cost is paid only on escalation.

#### Close #5 (staleness)

A `policies.yaml` predicate: `WHERE priority='low' AND status='open' AND captured_at < NOW() - 14 days → auto-park`. Wired as a daemon subscriber on the autonomous-flow engine. Same machinery as `auto-promote` (T020). No schema change.

#### Close #6 (intake routing)

A `stores intake "<text>"` skill that runs the 10.06 Q1/Q2/Q3 rubric and proposes one of `observations add` / `gate add` / `tasks add` with rationale. One new skill, not a substrate change.

### What we should learn (the deeper lessons)

1. **Realistic pull is working.** `cargo install` as the deploy verb (caught in worklog 07); T2-direct-to-main (caught in worklog 07); now `touches_files` and tier-aware-ratification (this analysis). All three are generalizations the self-build cannot surface and 10.06 pressure does. Continue pulling on 10.06 by default for design moves.

2. **Conflict as a guard, not a view.** Stores' substrate philosophy says reject the conflicting transition at write time. 10.06's design proposed a read-time aggregator (`./dev fleet`). Pick the guard. Race-free; the rejection error IS the recommendation.

3. **The orchestrator-as-fleet-planner is an anti-pattern in stores' frame.** Decision 7 in the 10.06 design ("orchestrator becomes fleet-planner; suggests non-conflicting alternatives") is exactly the back-channel philosophy.md warns against. That intelligence belongs in `next-action` queries against the schema, not in the orchestrator-AI.

4. **Type confusion at intake is a real problem.** 10.06's filing-rubric is the routing logic stores assumes the human resolves at filing time by picking among three parallel `add` verbs. Worth considering a single `stores file <text>` verb that asks the rubric questions and dispatches.

5. **Ratification cost uniformity will hit stores at ~50 open observations.** The self-build doesn't have that volume yet. When it does (or 10.06 pressure pushes it sooner), the bottleneck appears. Tier-aware `required_when` + bulk-disposition verb is the answer.

6. **"Bugs are observations, not blockers" already encodes the right discipline.** Tonight's friction analysis itself would be filed as an observation in stores' shape, with `tier_hint: T2`, ratified by Blake, promoted to a substrate task that adds `touches_files` and the planning-arrival conflict guard.

## Follow-ups

Observations to file (autonomous; filing is autonomous work). All six tracked here so they're not lost; pickup will route them.

| # | Topic | Priority | Tier | Notes |
|---|---|---|---|---|
| 1 | Add `touches_files: list<text>` to `observations.intent_contract` and `tasks.contract`; planning-arrival transition guard rejecting overlap with live tasks | high | T2 | The deepest fix. Closes #3. Mirrors 10.06 design decision 6 but as schema guard, not contract field declaration only. |
| 2 | Tier-aware `required_when`: relax contract requirements when `tier_hint == 'T1'` | high | T1-T2 | Closes #2. Plus `stores observations bulk-disposition` CLI verb (separate observation). |
| 3 | `stores observations bulk-disposition` verb: walk N rows applying `wont_fix` / `close_as_addressed` with one ratification per class | normal | T2 | Pairs with #2 above. |
| 4 | Staleness auto-park policy: daemon subscriber flips low-prio + N-day-stale rows to a hidden state | normal | T1 | Closes #5. Pure policy.yaml + subscriber. |
| 5 | `stores intake <text>` skill: runs Q1/Q2/Q3 rubric, dispatches to right `add` verb | normal | T2 | Closes #6. New skill, no substrate change. |
| 6 | Continue pulling 10.06 friction into stores design before shipping new generalizations | meta | n/a | Doctrine, not a task. The realistic-pull discipline. |
