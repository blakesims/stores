# Live Fake Traversal Matrix Merged Plan Review

**Date:** 2026-05-17
**Type:** note

## Summary

**Verdict: PASS for Phase 0.** The merged plan accurately folds in the substantive findings from notes 01-04 and Blake's latest correction: default execution is now a real, isolated lab arena, while mutation of the active checkout is an explicit `--mode current` opt-in. I found no blocker to starting Phase 0 implementation.

The plan is strongest where prior reviews pushed hardest: it keeps mocks above the fake-runner boundary only, makes lab mode real rather than simulated, makes raw-SQL live-path removal a Phase 0 gate, distinguishes RED substrate mismatches from harness ERRORs, adds DSL consequence-faking guardrails, uses `transition_history` including lifecycle/step columns as the oracle, and corrects the tier-A/tier-B authority wording.

## Details

### Correct

- **Lab/current correction is incorporated.** The plan now defaults to `--mode lab`, creating an isolated real git repo plus real `.stores/db.sqlite` and daemon path under `.stores/test-labs/<run-id>/` (`05-live-fake-traversal-matrix-merged-plan.md:14-17`, `80-95`). `--mode current` is retained only as an explicit opt-in for bugs that require active-repo residue (`97-107`). This matches Blake's correction: real implementation, isolated arena.
- **No mocks below fake-runner is explicit.** The summary says only nondeterministic runner text generation is substituted, while daemon, database, schemas, validators, subscribers, transition history, ER rows, worktrees, commits, merge/refusal behavior, locks, markers, and telemetry remain real (`05:8-9`). Lab mode repeats the realness checklist (`05:93-95`).
- **Consequence-faking is guarded in the DSL.** The plan states rows may configure fake-runner outputs and real setup/perturbations, but may not fake outcomes such as `stale_base`, `integration_blocked`, `blocked`, or `integrated` (`05:19-23`). It also lists forbidden setup fields like `final_status`, `force_status`, `external_review_status`, `integration_result`, `blocked_reason`, `stale_base`, and `stale_external_review` outside `expect` (`05:109-123`), then makes validation a Phase 1 deliverable (`05:302-304`).
- **`transition_history` expectation model now includes the prior review's missing columns.** The plan says omitted `visited` skips path checking and is never auto-derived (`05:160-165`), and `VisitedEdge` can match `lifecycle_from/to`, `active_step_from/to`, and `integration_step_from/to` in addition to status, verb, and invoker (`05:166-175`). This directly addresses the self-loop/substep concern for freshness refusal, dirty worktree, merge conflict, and clean integration substeps.
- **Authority/provenance wording is corrected.** The plan uses test provenance plus real actor semantics: rows get `test_run_id` / `test_case_id` / `synthetic=true` or loud markers, authority refuses non-current-run rows, tier-A token-required verbs are separated from tier-B no-token-required verbs, tier-B may receive a token for convenience but docs must not imply it is required, and fixture `tasks add` is explicitly not a U-moment (`05:190-203`). This matches the schema evidence: `accept`, `reject`, `abandon`, and `close-out-of-band` are `actor: human`, while `resume`, `amend`, `retry-integration`, and `tasks add` fields are `actor: ai_with_human` (`stores/tasks/schema.yaml:6-7`, `205-217`, `268`, `324`, `339`, `343`, `345`, `348-359`).
- **Raw-SQL live-path concern is a Phase 0 gate.** The merged plan's non-goals forbid raw-SQL writes for live outcomes (`05:45-52`), Phase 0 requires auditing `src/cli/test.rs` and related harness code to remove live-path writes or confine them to isolated non-live fixtures (`05:274-279`), and the Phase 0 exit criteria require no known live-path raw-SQL writes in matrix/harness paths (`05:292-296`). This is necessary because the current harness still has direct SQL writes, including `UPDATE external_reviews` in `src/cli/test.rs:1039` and fixture inserts in `src/cli/test.rs:1538`, `1694`, and `1953`.
- **Prior catalog/feasibility findings are carried forward at the right phase.** Smoke is reduced to five rows with a ~30 minute target (`05:254-262`), must-have rows or waivers before Phase 2 include `T3-pr-not-ready`, `T3-cr-fail`, ER revise from blocked, delegated policy, and T2 multi-phase rejection (`05:264-270`), duplicate-drive uses a controlled fake delay (`05:352-355`), integration-blocked recovery reuses merge-conflict/stale setup (`05:352-356`), L046 timing must be proven under `run --once` loop or a daemon window (`05:361-367`), and full runtime is now documented as ~2-3h initially (`05:375-383`).
- **RED vs ERROR model is clear.** The plan says a row is RED only when asserted substrate behavior mismatches, including terminal/path/cycle/ER/integration/liveness/no-real-LLM/authority mismatches, while setup or preflight failures are `ERROR`, not behavior failures (`05:177-188`). This aligns with the user's requirement that implementation proceed until tests are running and failing RED, not merely erroring.

### Note

- I did not find literal `C1`-`C20` labels in notes 01-04. I treated the substantive findings from `02-live-fake-traversal-matrix-plan-review.md` and `04-traversal-matrix-plan-v2-review.md` as the referenced correction set. The merged plan appears to cover those findings: safety/authority/raw-SQL/leakage from 02 and R1-R13-style expectation/catalog/feasibility/doctrine points from 04.
- Phase 0 should stay tightly scoped to safety and lab foundations. It should not attempt broad matrix orchestration before the Phase 0 review gate at `05:402-404`.
- The worker should make the lab approval token path explicit enough that a lab-local token cannot be confused with or written over the host token. The plan allows lab-local approval tokens (`05:203`); implementation should keep them inside the lab arena and include that path in proof artifacts.
- Phase 0's raw-SQL audit should classify each current SQL write by path: live/current/lab matrix path versus in-memory/unit fixture. Direct writes in isolated unit fixtures are acceptable only if they do not claim live-mode fidelity (`05:52`, `278-279`).
- Phase 2 should preserve the plan's ERROR/RED distinction in output from the first MVP. A row that cannot create the lab, start the fake runner, or pass preflight is not a useful RED; it is a harness ERROR.

### Blocker

- None for starting Phase 0 implementation.

## Follow-ups

- Begin Phase 0 with worker -> reviewer: raw-SQL live-path audit/removal or containment, test provenance/authority wrapper, lab arena creation, fake-runner preflight, and the listed negative tests (`05:274-296`).
- Do not move to Phase 2 matrix orchestration until Phase 0 safety/lab-mode foundations are reviewed.
