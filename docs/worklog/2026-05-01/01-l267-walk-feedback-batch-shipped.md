# L267 Walk Feedback Batch Shipped

**Date:** 2026-05-01
**Type:** note

## Summary

A test agent walked the new T009 observations port shape (the L267-shaped trace from the d4-16 schema), captured five concrete friction points, and we shipped the batch as v0.5.0 the same morning. All five fixes verified by the agent on retest; the transition-error message in particular was called "the strongest piece of UX in the system" and "genuinely better than what I asked for."

This is the first feedback loop where an agent ran the substrate, named real friction, the framework absorbed it, and the agent re-ran clean — without going through the full task-workflow ceremony (planner → plan-review → executor → CR cycles). Direct-commit batch shipped in 5 commits + 1 bump commit, ~150 LOC code + 12 new tests + 4 doc edits, ~2 hours wall.

## Details

### The agent's friction punch list

1. **`--linked-observations L001` rejected.** Every other list field on observations is repeatable; this one demanded `'["L001"]'`. T006 P4 had deliberately scoped repeat-flags to `List(_)` only, leaving `ListFk`/`ListRecord` JSON-only. The agent's experience said the assumption was wrong.
2. **Actor-mismatch error wording told you what's wrong, not what to do.** Always suggested `--invoker human` regardless of what the schema actually required. The agent was hitting `claim` (requires `ai_autonomous`) with `--invoker ai_with_human` from the skill template, and the message said "pass --invoker human" — which still doesn't satisfy `ai_autonomous`. Operator stuck in a 2-call recovery loop.
3. **`resolve from confirmed` error read like a bug report.** "no transition from 'confirmed' via verb 'resolve' found in schema" — sounded like the verb didn't exist. Actually `claim` is the missing intermediate; the schema knew but didn't say.
4. **Skill templates had `--invoker ai_with_human` muscle-memory creeping into verbs that need `ai_autonomous`.** Plus gate:walk had real doc rot (`--until` instead of `--defer-until`, a fictitious `--reason` flag, `cancel --invoker human` which would now actively fail).
5. **`./dev observation` → `stores observations` flag mapping was undocumented.** Anyone porting muscle memory from 10.06 would fumble (verb `log` → `add`, D9 production names like `done_when` → `acceptance`, evidence-record flattening, etc.).

### What landed in v0.5.0

| Commit | What | Item |
|---|---|---|
| `02c4403` | `ListFk`/`ListRecord` repeatable flags + bare-string auto-promote on ListFk | 1 |
| `3e6df54` | Transition error names reachable from-states + next-hop hint | 3 |
| `872066e` | Actor-mismatch error names the required actor in the remedy | 2 |
| `137164d` | Skill templates + `stores/observations/README.md` migration table | 4, 5 |
| `c883df5` | Version bump 0.4.1 → 0.5.0 | — |

Test count 416 → 428 (+12). All e2e canaries green throughout (`e2e.sh`, `gate_e2e.sh`, `observations_e2e.sh`).

### The substrate change worth noting

`build_entry_map`'s closure signature changed from `Fn(&str) -> Option<String>` to `Fn(&str) -> Option<Vec<String>>`. Type-aware assembly moved into a new `assemble_field_value()` helper. Three call sites (`add.rs`/`update.rs`/`transition.rs`) updated symmetrically; nothing externally visible to library consumers since stores isn't published as a lib. The new helper handles three input shapes for ListFk/ListRecord: single JSON-array (back-compat), single bare value (auto-promote), repeated `--<flag>` (collect to array).

### The error-message win

The agent's retest specifically called out the transition next-hop hint:

> The transition error message is now the strongest piece of UX in the system. That one line replaces what would have been a 3-call recovery loop (read error → run schema → re-grep for the verb → retry) with a zero-call recovery. It's the kind of error message that makes the schema teach. I'd argue this exact phrasing should be the template for any future schema-driven error: <what failed>; <where it would have worked>; <how to get from here to there>.

That's a candidate template for any future schema-driven error: **what failed; where it would have worked; how to get from here to there.**

### Process meta-observation

Direct-commit instead of the full task-workflow felt right for this batch. Five small, well-defined fixes from real-world feedback, none with material design questions to litigate at planner-time. The task-workflow ceremony adds value when there are real architectural choices to surface (T005 Layer-2 ordering, T006 select_transition extraction, T009 D9 production names) but is overhead when the work is mechanical. Worth keeping in the playbook: feedback batches → direct-commit; substrate features → task-workflow.

## Follow-ups

The agent surfaced two items I deliberately did NOT do in this batch, both worth considering separately:

1. **`add` validation errors with single violations look curt next to the 8-violation contract guard.** With `tail -1`, the agent missed two seed-time validation errors. Possible fix: prefix `add` errors with the row's intended display_id, or always include a header line so `tail -1` always shows context. Genuine UX gap; small T1 if we want it.

2. **Is `priority: required` intentional on observations?** The skill template suggests `--priority normal` as a default, which begs for a schema-level `default: normal` rather than a required flag. Same question applies to other "should-have-defaults" fields (`source` in some contexts, possibly others). Worth a sweep — a schema feature for `default:` would be reusable across the framework. Probably T2-shaped if pursued.

Neither blocks the v0.5.0 ship. Filed here for future picking-up.

The bigger uncrossed item is still the **TUI for `stores tasks drive --auto --claude-code`** — the multi-pane live-updating display the user asked about. Decided it's T3-shaped (substantial new dep, architectural shift on drive's progress emission, real "what is progress?" design questions) and shouldn't be done as a quick subagent execution. That's the natural next big task for stores once the agent's appetite returns.
