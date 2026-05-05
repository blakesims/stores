# Intent Harden Meta Valuable Context

**Date:** 2026-05-05
**Type:** note

## Summary

First sustained day of /intent-harden discipline applied to T3 substrate work — two runs, on L066 (T027 tier-structural drive cycle) and L075 (T028 TUI watcher with primed side-cars). Both produced cleanly hardened contracts that shipped (T027) or are riding (T028). The meta-insight from running the discipline twice in one session: **the harden produces extremely high-leverage derivation tokens — structured decisions with rationale, scope cuts tied to source-of-intent, ASCII mockups, the audit of what got compressed silently — and ALL of that dies with the chat session.**

The substrate captures end states (intent_contract, plan, wrap_log) but not the derivations that produced them. Derivations are often more expensive than the end states they yield. This is a generalizable gap: the substrate could persist these high-value tokens as typed slots, making them visible to downstream agents as durable rationale.

Filed **L077** to capture the principle + concrete shape (`intent_contract.harden_log`).

## Details

### What worked in today's two harden runs

1. **Codebase grounding before Phase 1.** Both runs peeked at the relevant source (`workflow.rs` / `next_action.rs` / `predicate.rs` for L066; `cli/watch.rs` for L075) BEFORE laying out the design tree. That meant my "default recommendations" were factual, not invented. Without that grounding, both contracts would have proposed extending machinery the codebase already had.

2. **Visualizing earlier than the skill prescribes.** For L075 (UI/UX), I brought ASCII mockups into Phase 1's preview cards instead of waiting for Phase 3. The mockups in the AskUserQuestion previews gave Blake something concrete to push against. Without them, the choice between "hand-off" / "inline pane" / "both" would have been abstract.

3. **The compression actually compressed.** L066: ~10 design branches → 2 user-judgment questions. L075: 18 branches → 4 questions in first AskUserQuestion + 4 in a follow-up bundle. The skill's discipline ("commit to defaults on codebase questions; only ask user-judgment") really worked.

4. **The user's custom answer rewrote my model.** On L075's action-flow question, Blake wrote a custom response (not picking any of my three options) that elevated the side-car from "recommendation-only" to "inherits the operator's token + propose-then-execute discipline." The harden didn't constrain Blake to my framing; the operator's judgment overrode the AI's recommendation. That's the system working as intended.

5. **Phase 5 (REALIGN) caught real cuts.** Both contracts had items that survived through Phase 4 but got cut at Phase 5 because they didn't trace back to source-of-intent. L075 cut "diff view inside watcher" because Blake had said "diff is not important to me" early in the conversation. Without REALIGN, that cut wouldn't have happened.

6. **The compression-with-audit pattern.** Blake's prompt "are there any more questions that the compression might have lost?" forced me to surface what got committed-to silently. That's not in the skill's written discipline but was the most-leverage moment of the L075 harden — Blake reclaimed 4 decisions I'd silently committed to. Worth feeding back into the skill itself as a default Phase-2 last step.

### The five meta-observations on map-into-substrate

1. **The hardened-brief shape IS the substrate's contract shape.** /intent-harden produces `objective`, `in_scope`, `out_of_scope`, `acceptance`, `tier_hint` — exactly the substrate's `intent_contract` fields. The skill is populating existing slots better, not building a new artifact. But: a hardened contract and a sloppy contract look identical to the substrate at write time. **No signal of "this contract was stress-tested."**

2. **The cuts are MORE valuable than the keeps.** L075's `out_of_scope` has 14 items. Each has rationale ("rejected: hand-off model is the design"). The executor downstream will be tempted to add diff views, action keys, multi-DB. The cut list prevents drift. **But the rationale is squashed into prose** rather than typed. If the executor reads `out_of_scope` and forgets the why, the cut won't hold. Could become structured: `cut_record { item, source_of_cut, confidence }`.

3. **Visualization is the cheapest cycle-saver.** Bringing mockups into Phase 1 was load-bearing. For the substrate: this argues for typed visualization slots on T3 contracts. Mockups field surfaces to the planner's brief; planner doesn't have to invent UI.

4. **The substrate boundary holds — but only just.** /intent-harden lives outside the substrate (Claude Code skill on the operator's machine). Per philosophy doc that's correct (it's a wrapper). But today's session showed how much wrappers shape contract quality. **The wrapper is invisible to anyone reading the substrate later** — a future operator won't know L075 was hardened, what cuts were made, or what was considered. The strongest argument for a `harden_log` field: it's the wrapper's audit trail, persisted into substrate.

5. **The principle generalizes beyond /intent-harden.** Same shape works for other high-leverage derivations:
   - Planner produces plan; the reasoning is in the transcript but not on the row (decision_matrix is partial)
   - Code reviewer produces verdict; the reasoning audit is in the transcript
   - Wrap produces summary; underlying analysis is in the transcript

   The pattern: **any process producing expensive structured reasoning should have a typed substrate slot to persist it.** Derivations are often more expensive than end states; deleting them is deleting expensive computations.

### Concrete shape (proposed in L077)

```
intent_contract.harden_log: optional record
  decisions:        list_record { branch, options, chosen, rationale, source_quote }
  cuts:             list_record { item, source_quote, confidence }
  visualizations:   list_record { kind, title, content }
  phases_run:       list_record { name, completed_at, output_summary }
  audit:            { compressed_branches, surfaced_branches, compression_audit_run }
```

Optional. Populated by /intent-harden runs (or any process producing structured derivation). Absence means "contract was not hardened" — downstream can read the signal and decide. Tier-conditional policy could require it for T3 (parallels T027's tier-structural skip in the other direction).

### What downstream consumers gain

- **Planner brief** sees the decisions + cuts: "context that shaped the contract; respect the cuts unless your plan explicitly addresses why they should be lifted"
- **Plan reviewer** validates that the plan doesn't add cuts back without rationale
- **Executor brief** sees the visualizations (mockups guide the build)
- **Code reviewer** sees the cuts ("verify these were NOT added back during execution")

This makes the contract MORE durable across the drive cycle, not just at acceptance.

## Follow-ups

### Filed
- **L077** (filed today, T2) — `intent_contract.harden_log` typed field + downstream brief integration

### Worth filing as siblings to L077
- **/intent-harden skill enhancement** — add a "compression audit" step at end of Phase 2 that surfaces silently-defaulted branches with reasoning, lets the user lift any back to judgment. One round trip; massive correctness gain. Lives in `~/.claude/skills/intent-harden/SKILL.md`. Not a substrate obs (skill is wrapper), but worth a worklog note or PR to the skill.
- **Audit existing substrate fields for derivation completeness** — `wrap_log`, `plan_review_log`, `code_review_log` may benefit from harden_log-style derivation slots beyond their current envelope shape. T2-T3 follow-up; let L077 land first to validate the pattern.

### Open question
- Should harden_log be a top-level row field or nested under intent_contract? Today's filing put it under intent_contract because that's where the harden's output landed. But if other derivations get the same pattern (planner derivation, reviewer derivation), they'd need their own slots — `plan.derivation_log`, `code_review_log.derivation`, etc. Probably fine; each process gets its own typed slot.

### Strategic
- Today's session was the first sustained dogfood of /intent-harden inside the substrate workflow. The discipline shipped two T3 contracts cleanly (T027 + T028 still riding). The compounding bet: every T3 hardening from now on produces a harden_log; downstream agents read those logs; planner quality improves; reviewer catches drift earlier. **L077 is the substrate hook that makes this compounding possible.** Worth ratifying soon — high leverage per LOC.
