Inherited decisions:
- Approved DONE_WHEN: implement a named live fake-runner `stale-base-refuses` scenario that uses the real repo/daemon path, fake runners only, real worktree/branch/commits/ER row, advances main after fake ER PASS, attempts normal acceptance/integration, prints proof artifacts, and asserts no LLM calls.
- Prior design rule from `01-live-fake-runner-scenario-tdd-plan.md`: fabricate real preconditions; do not write/mock the expected outcome.
- The plan intentionally targets the T146/T148 freshness/non-convergence battlescar family, not the silent-zombie family.
- Existing live harness in `src/cli/test.rs` already drives `stores agents run --once` with fake-mode env and creates live synthetic tasks, but it is happy-path/failed-ER shaped.

Diagnosis:
- The plan is directionally right, but its central expected failure label is too loose. With an additive fenced commit on `main` after fake ER PASS, the latest ER `base_sha` remains an ancestor of current `main`; `integrate.rs`' pre-rebase `stale_base` check will usually NOT fire. The likely real refusal is the later `stale_external_review` check after integration refresh/rebase changes the candidate branch head (`integrate.rs` around the ER head freshness re-check), or possibly an accept precheck if the workspace HEAD has changed. Exact `stale_base` in current code requires the ER base to be no longer reachable from main, as the existing unit test demonstrates by force-rewriting `main` with an orphan branch. That is not the same as a normal fenced main-advance commit and should not be done to this live repo.
- The plan should therefore revise the expected outcome to “freshness refusal, likely `stale_external_review`, caused by genuine main movement after review,” while keeping `stale-base-refuses` as a user-facing battlescar name if desired. If the implementation insists on exact `stale_base`, the precondition would require history rewrite/orphaning, which conflicts with safe live-repo marker-commit doctrine.
- Existing live harness auto-flow is too narrow: it only calls `accept_for_integration` when `expect.task_status == integrated`. A stale/freshness case needs its own scenario branch: wait for fake ER PASS, collect proof, advance main, call accept/enqueue/integration path, then assert the refusal.
- The existing `matches_expect` model only checks task status/lifecycle/latest ER status. It cannot express “command attempted and refused with reason X,” “task did not integrate,” or “integration_attempts outcome contains stale/freshness.” The plan needs a concrete expectation extension or a special-case assertion for this scenario.
- `snapshot()` currently does not load `base_sha`, `head_sha`, task `workspace_path`, branch, `integration_blocked_reason`, `integration_attempts`, or superseded ER status. Those are necessary for the proof transcript and assertions.
- `create_live_task()` relies on normal task add/activation/scaffold/daemon behavior to eventually fill worktree/branch, but the plan says “create real worktree/branch” and “record base A.” Implementation must explicitly wait until `workspace_path` and `branch` exist before attempting ER/proof, and must record base from the correct main repo resolved from that worktree.
- Fake ER base/head should be trusted only after reading the actual external_reviews row. Current external review preflight may rebase before review if branch base differs from main; because the main-advance must happen after ER PASS, this is okay, but the proof must assert ER base/head were captured before the later main movement.
- The current `isolate_live_case()` path deactivates non-integrated cases and directly updates retry timestamps for tooling-held ER rows. For this scenario, deactivation/freezing after proof would undercut the requirement that Blake can watch the live issue in `stores watch`. Avoid calling the generic isolation path for stale/freshness proof unless the user explicitly asks for cleanup.
- Current `accept_for_integration()` invokes `tasks accept --invoker human` from inside the harness. That is existing harness behavior, but it is an authority smell relative to repo doctrine. The implementation should either preserve it only under explicit test-mode harness semantics and document it in output, or use whatever test-mode/approval path already exists. Do not silently expand this pattern beyond the fake live harness.

Drift / contradiction check:
- Main contradiction: plan says normal main movement should produce `stale_base`; current integration code says normal main movement is more likely `stale_external_review` after refresh. Revise the plan’s assertion language from exact stale_base to canonical freshness refusal, with accepted substrings including `stale_external_review`, `stale external review head`, `freshness`, and `stale_base`.
- Plan says “real live daemon running,” but existing harness runs `stores agents run --once` repeatedly. If the parent/user expects an already-running detached daemon, this is a scope expansion. The safer interpretation is “real daemon code path, not mocked daemon,” matching the approved DONE_WHEN. Note that in the plan.
- Plan says leave visible in watch, while current non-integrated live harness isolates/deactivates. Revise to bypass isolation for this case and leave the row in the real refused state.
- Plan says no raw-SQL final-state mutation. Existing live helper `freeze_latest_tooling_held_review_retry()` uses a direct UPDATE. Do not use that helper for this scenario; assertions should be read-only except normal CLI/subscriber mutations.

Required plan revisions before worker implementation:
1. Rename the expected assertion from “stale_base exact” to “freshness refusal after stale review,” with `stale_external_review` as the expected likely canonical outcome for additive main movement. Keep `stale-base-refuses` as the case name only if documented as a historical/battlescar label.
2. Add a dedicated live scenario branch/method, e.g. `run_live_stale_base_refuses`, instead of trying to force it through `matches_expect`/happy-path release logic.
3. Extend proof data loading for the scenario: task status/lifecycle/active_step, workspace_path, branch, latest ER display_id/status/verdict/runner/base_sha/head_sha/superseded_by, integration_blocked_reason, integration_attempts latest outcome/summary, main SHA before/after, task branch HEAD before/after.
4. Add an explicit wait for worktree/scaffold readiness before driving to ER PASS.
5. Advance `main` only after fake ER PASS has persisted base/head. Use fenced additive marker commit; do not force-rewrite main in live mode.
6. Attempt normal accept/enqueue/integration after main movement. If `tasks accept` itself refuses due stale external review, treat that as valid freshness proof; otherwise enqueue/integration should produce `integration_blocked`/non-integrated freshness proof.
7. Bypass generic isolation/deactivation for this case so `stores watch --all` can show the live refused row. Print any operator cleanup command separately as optional, not automatic.
8. Add tests for the refusal classifier and scenario routing. Use existing `integrate.rs` unit tests as evidence for exact stale_base semantics; do not create live-repo-dependent tests.
9. Update output requirements to include the likely actual labels: `stale_external_review` and/or `stale external review head`, not just `stale_base`.
10. Document that the harness uses the real `agents run --once` daemon path repeatedly unless/until a separate mode is added for attaching to an already-running detached daemon.

Recommendation:
- Proceed after revising the plan as above. The best first TDD candidate is still this freshness case because it is deterministic, uses existing fake runner and integration seams, and avoids the timing nondeterminism of silent-zombie/watchdog races.
- Do not pivot to silent-zombie for the first implementation pass; it is a better second battlescar once the live fake scenario harness has a robust proof-transcript path.

Risks:
- If integration refresh produces a merge/rebase conflict instead of stale review freshness, the marker file choice may be colliding with task marker paths. Use a distinct main marker file path that should not conflict with the fake executor marker.
- A detached real daemon running concurrently could race with the harness’s own `agents run --once`. The plan should either preflight/report daemon status or accept this as part of “real live” behavior and make output clear.
- Leaving refused rows active may cause future daemon retries. This is desirable for watchability but may need a printed cleanup/deactivate command for after the demo.
- Existing unrelated dirty tracked files in the repo can affect main marker commits unless the implementation stages only the fenced marker path. The worker must use explicit `git add <marker-path>`, never `git add .`.

Need from main agent:
- No further user decision is needed if the plan is revised to “freshness refusal, likely stale_external_review” rather than exact `stale_base`.

Suggested execution prompt:
- Implementation handoff is warranted after plan revision: implement the revised `stale-base-refuses` live fake-runner scenario in `src/cli/test.rs`, preserving no-LLM fake runner guarantees, printing proof artifacts, adding focused tests for preset/scenario routing and freshness refusal classification, and validating with the real live command if safe.
