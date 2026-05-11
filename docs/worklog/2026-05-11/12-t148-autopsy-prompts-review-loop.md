# T148 Autopsy Prompts Review Loop

**Date:** 2026-05-11
**Type:** note

## Summary

Prompts and review-loop policy materially amplified T148 convergence cost. They did not invent all findings — many ER findings were real defects against an unusually broad ADR 0002 contract — but they converted a broad task into an open-ended compliance hunt with no external-review revise budget, no “stop/split follow-up” threshold, and repeated full-suite/test-sweep pressure. The worst prompt/process contributors were:

1. external review asked for whole-task ADR 0002 compliance over the full diff, prior reviews, plan, wrap, and contract, then said `PASS only if no blocking findings remain` with no scope/novelty/staleness cutoff;
2. external-review REVISE has no built-in max-cycle or escalation policy comparable to in-cycle code review;
3. code-reviewer instructions still bias toward finding issues (`expect 3+`) and broad test runs;
4. executor instructions require tests after every task plus final full `cargo test`, which made T148 repeatedly pay for known unrelated failures and encouraged “fix until green” behavior outside the local acceptance slice.

## Evidence inspected

- `tasks/active/T148-auto-promoted-l568/main.md` lines 18-50: T148 had no top-level objective set and a very broad “fully complete ADR 0002” contract spanning inlet, observations, architecture reviews, consumers, subscribers, TUI/watch, diagnostics, and tests.
- `tasks/active/T148-auto-promoted-l568/main.md` lines 55-80 and later phase sections: six large phases, each with many sub-tasks and acceptance criteria.
- `/home/blake/repos/experiments/stores-T148-auto-promoted-l568/.stores/runs/c177a580-8be1-4e27-8d1a-2f8c273a12be.codex.stderr.log` lines 13-36: generated external-review prompt included the entire ADR 0002 done-when and acceptance list before the diff.
- `src/handlers/external_reviews.rs` lines 470-498: `render_codex_prompt` includes contract, plan, wrap log, prior external review attempts, git context, and the full diff; verdict text has no “only review changed/fixed area” or follow-up split rule.
- `src/handlers/submit.rs` lines 1730-1763 and 1886-1888: submit-wrap creates a pending external review for T2/T3 tasks after wrap.
- `src/flow/builtins/external_review.rs` lines 544-595: every external-review REVISE records the verdict and fires `submit-external-review` back to the task; no max-attempt or escalation gate is visible in this path.
- `src/handlers/brief.rs` lines 234-323 and `stores/tasks/templates/executor-brief.md.tpl` lines 109-122: respawned executors get the latest current-head external review finding as backpressure, which is good, but there is no instruction to reject marginal/out-of-scope review demands or split follow-up work.
- `stores/tasks/templates/executor-brief.md.tpl` lines 125-130 and `agents/executor.md` lines 103-127, 149-165: executor prompts require tests after each task/group and a final full `cargo test -- --test-threads=10` sweep.
- `stores/tasks/templates/code-reviewer-brief.md.tpl` lines 113-119 and `agents/code-reviewer.md` lines 112-121, 203-227: reviewers are told to run `cargo test` or equivalent, verify every AC mechanically, and for non-trivial changes expect 3+ findings; REVISE is the default for fixable major issues.

## ER414-ER429 pattern

External-review rows for this window:

| ER | Task | Status | Verdict | Critical | Major | Minor |
|---|---|---|---|---:|---:|---:|
| ER414 | T148 | revise | REVISE | 0 | 0 | 0 |
| ER415 | T149 | superseded | PASS | 0 | 0 | 0 |
| ER416 | T149 | passed | PASS | 0 | 0 | 0 |
| ER417 | T148 | revise | REVISE | 0 | 1 | 0 |
| ER418 | T148 | revise | REVISE | 0 | 0 | 0 |
| ER419 | T148 | revise | REVISE | 0 | 1 | 0 |
| ER420 | T148 | revise | REVISE | 0 | 0 | 0 |
| ER421 | T148 | revise | REVISE | 0 | 2 | 0 |
| ER422 | T148 | revise | REVISE | 0 | 2 | 0 |
| ER423 | T148 | revise | REVISE | 0 | 2 | 0 |
| ER424 | T148 | revise | REVISE | 0 | 2 | 0 |
| ER425 | T148 | revise | REVISE | 0 | 0 | 0 |
| ER426 | T148 | revise | REVISE | 0 | 0 | 0 |
| ER427 | T148 | revise | REVISE | 0 | 1 | 0 |
| ER428 | T148 | passed | PASS | 0 | 0 | 0 |
| ER429 | T148 | passed | PASS | 0 | 0 | 0 |

Notes:

- T148 had 13 external-review REVISE attempts in ER414, ER417-ER427 before ER428/ER429 PASS. Several rows counted `0 major` but still carried REVISE text, because severity counting only recognizes `[major]`/`major:` markers while Codex often used `[P1]`/`[P2]`.
- The sequence is not a simple hallucination loop. Findings walked through real surfaces: comma-delimited CLI parsing, auto-promote contract-state compatibility, cardinality tests, TUI/detail consumers, architecture-review gate clearing, supersede typed references, amend ratification timing, and legacy auto-resolve compatibility.
- But the sequence shows “whole ADR compliance” review behavior: after local fixes, the external reviewer kept discovering another ADR 0002 edge in adjacent surfaces. That was predictable from a prompt that included the whole contract, whole plan, all prior attempts, and full diff.
- ER417/ER418 and ER426/ER427 are clear duplicate/repeat loops around test-helper mismatch and supersede-test expectations. A process with duplicate-finding suppression or “second identical REVISE => escalate/split” would have stopped earlier.

## Prompt/process failure modes

### 1. External review scope was too broad for a convergence gate

The generated Codex prompt says to “Review task T148 using the contract, plan, wrap log, prior reviews, and rebase-aware diff” and then supplies the whole ADR 0002 contract and all plan JSON. That makes the reviewer a whole-task architecture auditor, not a final diff sanity checker. For T148, the contract itself required every upstream consumer/subscriber surface to comply, so any missed edge became a blocking REVISE.

Recommended change: external review prompt should have an explicit gate policy:

- Review only the net diff from `base_sha..head_sha` against the ratified contract and wrap claims.
- Classify findings as `blocking_current_task`, `follow_up`, or `out_of_scope`.
- REVISE only for `blocking_current_task` defects that are both introduced by this diff and necessary to satisfy the task contract.
- If a finding is a valid adjacent improvement but not required by the current contract slice, return PASS with `follow_up_recommendations`.
- If the same semantic finding appears in two consecutive attempts, do not emit a third REVISE; return TOOLING_FAILURE/escalate or PASS-with-follow-up depending on severity.

### 2. External review lacks a revise budget / stop criterion

In-cycle code review has a documented ≤3 REVISE cycle policy; external review does not. `record_terminal` fires `submit-external-review` on every REVISE, and the executor brief says to address it directly. The framework therefore treats the external reviewer as an unbounded queue of work.

Recommended change:

- Add an external-review attempt budget per task/head (e.g. max 3 REVISE attempts after wrap, max 1 duplicate semantic finding).
- On budget exhaustion, route to blocked with a concise “external-review convergence stall” reason and latest findings, or require human/supervisor decision to split/accept risk.
- Track normalized finding fingerprints so duplicate ERs are detected mechanically.

### 3. Prompts reward over-finding and over-testing

The code-reviewer template says `RUN tests yourself: cargo test or equivalent` and `FIND issues thoroughly (for non-trivial changes expect 3+; explain if fewer)`. The agent instruction repeats acceptance verification and says zero findings on >3-file changes usually means insufficient review. This is useful early in a task, but harmful at final convergence: it biases reviewers toward finding marginal issues even when acceptance criteria pass.

Recommended change:

- Replace “expect 3+ findings” with “do not invent findings; PASS with 0 findings is expected when acceptance criteria pass and the diff is coherent.”
- For revise cycles and external review, explicitly say: “Do not introduce new non-blocking style/test-expansion findings unless they are critical/major regressions introduced by the fix.”
- Require reviewers to identify whether each finding is `contract_blocker`, `regression`, `test_gap`, `style`, or `follow_up`.

### 4. Full-suite test pressure was too high

Executor instructions require tests after each task/group and a final `cargo test -- --test-threads=10`; T148 execution logs repeatedly mention full-suite failures from pre-existing `drive_silent_zombie_e2e` tests and later `cargo test --workspace` sweeps. For a large migration task, full-suite runs are appropriate at phase boundaries or before wrap, but not after every sub-task or every external-review micro-fix.

Recommended change:

- Executor prompt should say: run targeted tests after each task group; run full suite only before wrap/final submission or when acceptance criteria explicitly require it.
- If the full suite fails in a known unrelated/pre-existing test, record it once as residual risk and continue with targeted green evidence; do not chase unrelated suite failures unless the current diff plausibly caused them.
- External-review revise executors should run the smallest test that proves the finding fixed, plus `cargo build` if relevant; full-suite rerun only after the final external-review pass candidate.

### 5. Missing “split follow-up” instruction caused ADR0002 marginal loops

The executor brief tells the agent to address external findings directly and not re-implement unrelated scope, but it does not empower the executor to say “this is valid but should be a follow-up” or to stop when the external reviewer expands into adjacent ADR surfaces. The task contract was so broad that many adjacent surfaces were arguably in scope, but the process still needed a convergence safety valve.

Recommended change:

- Add to executor revise briefs: if a review finding requires broad new design, touches files not in the phase/files list, or would exceed a small patch (e.g. >2 files or new schema/API semantics), emit `BLOCKED: split follow-up recommended` with rationale instead of implementing.
- Add to external-review prompt: reviewers should recommend split-follow-up rather than REVISE when the finding is not a regression from the current diff or is architectural/contract-amending.
- Add wrap prompt requirement: list external-review REVISE churn and identify which remaining risks should be follow-ups, so acceptance has a human-readable stop surface.

## Recommended concrete edits

1. `src/handlers/external_reviews.rs::render_codex_prompt` — rewrite verdict instructions to include blocking/follow-up/out-of-scope categories, duplicate-finding suppression, and “do not require unrelated full-suite cleanup.”
2. External review schema/parser — add structured fields for `blocking_findings[]`, `follow_up_findings[]`, and `finding_fingerprint` or `semantic_key`; stop relying only on severity marker counts.
3. External-review workflow — enforce max REVISE attempts per task/head and route to blocked/escalation on repeated semantic findings.
4. `stores/tasks/templates/executor-brief.md.tpl` and `agents/executor.md` — change testing policy from mandatory full-suite after all tasks to targeted tests by default, full-suite before wrap/final only; add a known-unrelated-failure recording rule.
5. `stores/tasks/templates/code-reviewer-brief.md.tpl` and `agents/code-reviewer.md` — remove “expect 3+ findings” as a standing instruction; replace with a no-invented-findings/no-new-minors-on-re-review rule.
6. `stores/tasks/templates/executor-brief.md.tpl` — in `External Review Backpressure`, add explicit stop/split-follow-up criteria for broad, duplicate, stale-base, or architectural findings.

## Follow-ups

- File a substrate task to add external-review convergence policy and prompt/schema changes.
- File a smaller prompt-only task to remove over-finding and full-suite-overuse language from executor/code-reviewer instructions.
