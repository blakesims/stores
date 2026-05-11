# T148 Autopsy Consolidation

**Date:** 2026-05-11
**Type:** note

## Summary

T148 friction was not one bug. It was a coupled failure across five layers:

1. **External-review recovery could mint duplicate attempts** (`create-pending` has no active-attempt/current-head guard), so review noise multiplied.
2. **Lifecycle/status overlays are over-compressed** (`in_review` means wrap dispatch, external-review readiness, and human acceptance surface), so subscribers and operators acted on ambiguous state.
3. **Schema drift is real** (fresh YAML vs live DB CHECK/defaults), so recovery verbs like `import-pass` had to work around the installed DB instead of trusting the schema.
4. **Integration/deploy wiring is stale** (`.stores/agents.yaml` still carries pre-T138 accept-edge subscribers), so post-integration cargo-install/schema-migrate required recovery paths and emitted misleading old errors.
5. **Review prompts/process lacked convergence controls**: broad whole-contract ER prompt + no revise budget + over-finding/over-testing bias converted real edge findings into an unbounded compliance hunt.

The most actionable root cause is **unbounded external-review churn over an ambiguous lifecycle surface**. The substrate allowed multiple review attempts for the same task/head, then treated every REVISE as equally authoritative and automatically re-entered execution with no max-attempt, duplicate-finding, or split-follow-up policy.

## Evidence notes

Scout notes created:

- `docs/worklog/2026-05-11/08-t148-autopsy-logs-db.md`
- `docs/worklog/2026-05-11/09-t148-autopsy-schema-lifecycle.md`
- `docs/worklog/2026-05-11/10-t148-autopsy-er-daemon-engine.md`
- `docs/worklog/2026-05-11/11-t148-autopsy-git-integration.md`
- `docs/worklog/2026-05-11/12-t148-autopsy-prompts-review-loop.md`

## What actually happened

T148 progressed through real implementation work and real code-review cycles, then hit external review after wrap. From there:

- Early ER rows (`ER409`-`ER412`) were stale-base/tooling noise.
- `ER414` was the first substantive REVISE.
- After that, T148 repeatedly cycled `in_review -> executing -> code_review -> complete -> in_review`.
- Duplicate/noisy ER rows appeared around the same heads/findings: `ER417/ER418`, `ER424/ER425`, `ER426/ER427`, plus out-of-order/stale rows like `ER422`.
- Several REVISE rows carried zero counted majors/minors because parsing missed Codex `[P1]/[P2]` severity formats.
- The operator finally stopped automation, committed final WIP (`d8a89b4`), added a recovery fix (`244af10`), imported a manual PASS (`ER429`), accepted, integrated, installed, and migrated.

Final state is good: T148 is `integrated`, `post_integration_step=schema_migrated`, and installed `stores` reports main merge `55caf51`. But stale runner metadata still appears in the task row/status even though no live process exists.

## Root cause map

### A. Duplicate/noisy external reviews

Primary bug surface: `external_reviews create-pending`.

Findings:

- It directly inserts a pending row using supplied SHAs.
- It does not validate supplied head/base against the task worktree.
- It does not reject an existing non-superseded pending/running/passed/revise/tooling-held row for the same task/head.
- Engine Layer 2 then dispatches every pending ER row; per-row CAS is safe, per-task/head uniqueness is not.

Consequence: recovery attempts created extra authoritative-looking reviews, sometimes overlapping with framework-created attempts.

### B. Ambiguous lifecycle surface

Primary design issue: `status='in_review'` is overloaded.

It means, depending on actor/subscriber:

- wrap agent should run;
- external-review backfill may create an ER;
- human acceptance may be available;
- release-to-integration may be possible;
- `active_step` may still be `wrapping`.

This made state hard to reason about and created edge-sensitive subscriber ordering. It also made inactive/active behavior confusing during recovery because activation checks are split across schema guards, `.stores/agents.yaml`, scanner classification, and migration backfill.

### C. Schema/live DB drift

Concrete drift:

- `stores/external_reviews/schema.yaml` allows runner values `manual-codex` and `manual`.
- Live DB CHECK still allowed only `codex`, `pi`, `claude-code`.
- Normal migrate does not rebuild CHECK/default drift.
- T148 needed `244af10` to store manual import rows as `codex` while preserving the manual label in transition history.

Broader drift:

- ADR0002 framework migrations add plain TEXT columns on older DBs while fresh schemas may express tighter enum/default intent.
- Task lifecycle defaults/checks differ across historical DBs and current schema/codegen.

### D. Integration/deploy wiring drift

T148 final merge was not stale-base/rebase failure. The final PASS reviewed the current head/base. The first integration attempt failed because main was dirty, not because the lane chose the wrong base.

But `.stores/agents.yaml` still contains stale pre-T138 subscriber wiring:

- old accept-edge `accept-merge` / `cargo-install` / `schema-migrate` wiring;
- current architecture expects integration lane then post-`integrated` cargo-install/schema-migrate.

This created misleading old dispatch-lock errors and meant deploy completion needed recovery/direct builtin behavior (`reconcile-accepted` shape) rather than clean subscriber flow.

### E. Prompt/process convergence failure

The external-review prompt gave Codex the full contract, full plan, wrap, prior reviews, and full diff, then asked for PASS only if no blocking findings remained. For a huge ADR0002 task, that invited whole-system compliance hunting.

Compounding issues:

- No max external-review REVISE budget.
- No duplicate semantic finding fingerprint.
- No PASS-with-follow-up category.
- Code-review prompts bias toward over-finding (`expect 3+`).
- Executor prompts bias toward frequent/full-suite runs.
- Revise executors are not empowered to stop and split broad/marginal findings.

Many findings were real, but the process had no stopping rule once the task entered long-tail edge discovery.

## Severity ranking

### P0 / immediate substrate repair

1. **Harden `external_reviews create-pending`.**
   - Validate current task worktree head/base.
   - Reject active same-task/head attempts.
   - Require `ai_with_human`/human for operator recovery.
   - Prefer computing SHAs internally.

2. **Add external-review convergence policy.**
   - Attempt budget per task/head.
   - Duplicate semantic finding detection.
   - Route to blocked/escalation or PASS-with-follow-up after budget exhaustion.

3. **Fix live agent wiring.**
   - Remove stale accept-edge deploy subscribers.
   - Wire post-integrated cargo-install/schema-migrate to the current T138/T146 lifecycle.

### P1 / structural cleanup

4. **Disambiguate review lifecycle.**
   - Split or gate `in_review` semantics: wrap-ready vs ER-ready vs acceptance-ready.
   - Require wrap_log/current head before external-review minting.

5. **Add CHECK/default drift migration support.**
   - At minimum explicit rebuild for `external_reviews.runner`.
   - Longer-term schema drift detector for enum/default changes.

6. **Clean terminal runner metadata.**
   - Clear `drive_pid`/current-run marker on terminal task transitions.
   - Add health/TUI warning for terminal rows with stale live metadata.

### P2 / prompt/process changes

7. **Rewrite external-review prompt.**
   - Structured categories: `blocking_current_task`, `follow_up`, `out_of_scope`.
   - REVISE only for current diff/contract blockers.
   - No third duplicate REVISE.

8. **Remove over-finding and full-suite bias.**
   - Replace “expect 3+ findings” with “do not invent findings”.
   - Targeted tests by default; full suite before wrap/final only.
   - Known unrelated full-suite failures become residual risk, not infinite repair fuel.

9. **Improve integration diagnostics.**
   - Preserve full dirty-main `git status --short` and merge stderr tail.
   - Clear/render historical `integration_blocked_reason` after success.

## Proposed follow-up task slices

### Slice 1: ER duplicate prevention and convergence guard

Scope:

- `create-pending` guard/shared handler.
- per-task/head active attempt check.
- test for framework + manual duplicate prevention.
- stale running ER watchdog or at least visible held state.

Done when: impossible to create duplicate pending/running ER for the same task/head without explicit force/supersede; repeated REVISE can halt/escalate instead of infinite loop.

### Slice 2: Prompt convergence pass

Scope:

- `render_codex_prompt` structured verdict policy.
- executor/code-reviewer prompt testing/finding language.
- split-follow-up criteria in external-review backpressure brief.

Done when: reviewers can PASS with follow-ups, duplicate findings are discouraged, and revise executors are instructed to stop rather than chase broad adjacent architecture changes.

### Slice 3: Lifecycle/schema drift repair

Scope:

- explicit `external_reviews.runner` CHECK rebuild or schema drift detector.
- task lifecycle/default drift audit.
- ER minting requires wrap readiness/current head.
- terminal runner metadata cleanup.

Done when: live DB matches schema intent for manual ER imports; status output no longer reports stale live runners on terminal tasks; ER backfill cannot run against empty/stale wrap context.

### Slice 4: Integration/deploy wiring repair

Scope:

- update `.stores/agents.yaml` to current integration lane shape.
- add subscriber/dispatch tests for post-integrated cargo-install/schema-migrate.
- improve dirty-main diagnostics.

Done when: accepted task goes through integration lane and post-integrated deploy without stale accept-edge jobs or manual reconcile.

## Bottom line

The friction was “all of the above”, but not equally. The central failure mode was:

> Broad external review found real long-tail issues while the engine allowed duplicate attempts and had no convergence stop, all on top of ambiguous lifecycle/subscriber state and stale live deployment wiring.

Fix duplicate ER creation and add convergence policy first. Then clean lifecycle/schema drift and prompt incentives so the next broad architecture task can stop cleanly instead of needing an operator rescue.
