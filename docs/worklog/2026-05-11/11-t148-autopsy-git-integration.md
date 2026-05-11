# T148 Autopsy Git Integration

**Date:** 2026-05-11
**Type:** note

## Summary

T148's final git/integration failure was not a stale-ref/rebase problem: by the time it entered the integration lane, `main` was at the T149 merge (`e6f1f95`), T148 had fresh PASS reviews on the final heads (`ER428` for `d8a89b4`, `ER429` for `244af10`), and the branch merged cleanly on retry. The first lane attempt blocked because the main checkout was dirty with files that the T148 merge would overwrite; the lane records only the first stderr line, so the exact file list is lost from `integration_attempts`.

The more important finding is that the repo's live `.stores/agents.yaml` still contains pre-T138 post-accept wiring (`accept-merge`, `cargo-install`, `schema-migrate` on old edges), while code/docs expect the T138 integration lane plus post-`integrated` subscribers. That stale agent wiring made recovery noisier: old subscribers fired at acceptance and failed/errored before the real integrate lane ran, and post-`integrated` `cargo-install`/`schema-migrate` did not appear as dispatch locks. T148/T149 nevertheless reached `post_integration_step='schema_migrated'`, apparently via operator recovery/direct builtins (`reconcile-accepted` shape), not through correctly wired post-integrated subscribers.

## Evidence commands

```bash
# Current branch/main relationship
git rev-parse main
# d8ff8d01ad79603b69ace3c38ef0120b5852fb11
git rev-parse feat/T148-auto-promoted-l568
# 244af1039a02937855b62bd539de40ffb24caf84
git merge-base main feat/T148-auto-promoted-l568
# 244af1039a02937855b62bd539de40ffb24caf84
git merge-base --is-ancestor feat/T148-auto-promoted-l568 main; echo $?
# 0

git log --all --grep='T148' --oneline --decorate --max-count=30
# d8ff8d0 (HEAD -> main) docs: record T148 recovery closure
# 55caf51 Merge branch 'feat/T148-auto-promoted-l568'
# 244af10 (feat/T148-auto-promoted-l568) T148 recovery: keep manual ER imports DB-compatible
# d8a89b4 T148 codex-revise: isolate routed test fixtures
# ... earlier T148 phase/revise commits ...

git log --all --grep='T149' --oneline --decorate --max-count=5
# e6f1f95 (feat/runner-telemetry-recovery, feat/T151-default-inactive, feat/T150-default-inactive) Merge branch 'feat/T149-auto-promoted-l563'
# fc9792c (feat/T149-auto-promoted-l563) T149 P1: canonicalize stores-root routing roots
# f397f72 T149 P1: enforce stores-root conflict and directory validation
# 9ac4a65 T149 P1: add canonical stores-root routing for auto-drive
```

```bash
sqlite3 .stores/db.sqlite "SELECT display_id,status,lifecycle,active_step,integration_step,post_integration_step,branch,substr(workspace_path,1,80),integration_blocked_reason FROM tasks WHERE display_id IN ('T148','T149');"
# T148|integrated|done|none|none|schema_migrated|feat/T148-auto-promoted-l568|/home/blake/repos/experiments/stores-T148-auto-promoted-l568|merge_failure: git merge --no-ff feat/T148-auto-promoted-l568 into main failed: error: Your local changes to the following files would be overwritten by merge:
# T149|integrated|done|none|none|schema_migrated|feat/T149-auto-promoted-l563|/home/blake/repos/experiments/stores-T149-auto-promoted-l563|merge_failure: git merge --no-ff feat/T149-auto-promoted-l563 into main failed: error: Your local changes to the following files would be overwritten by merge:
```

```bash
sqlite3 -json .stores/db.sqlite "SELECT display_id, integration_attempts FROM tasks WHERE display_id IN ('T148','T149');"
# T148 attempt 1: base_main_sha=e6f1f95..., candidate_head_after=244af10..., outcome=merge_failure,
#   pre_land_check_summary="git merge --no-ff feat/T148-auto-promoted-l568 into main failed: error: Your local changes to the following files would be overwritten by merge:"
# T148 attempt 2: base_main_sha=e6f1f95..., candidate_head_after=244af10..., landed_main_sha=55caf51..., outcome=integrated, pre_land_check_summary=ok
# T149 attempt 1: outcome=rebase_conflict, summary="rebase: <no conflict files reported> (error: cannot rebase: You have unstaged changes.)"
# T149 attempt 2: outcome=stale_external_review, summary="ER ER415 reviewed head 404ec86 but candidate is now fc9792c; superseded"
# T149 attempt 3: outcome=merge_failure, same dirty-main overwrite message
# T149 attempt 4: landed_main_sha=e6f1f95..., outcome=integrated
```

```bash
sqlite3 .stores/db.sqlite "SELECT display_id,task_id,attempt,status,verdict,substr(base_sha,1,7),substr(head_sha,1,7),held_reason,superseded_by FROM external_reviews WHERE task_id IN ('T148','T149') ORDER BY task_id, attempt, id;"
# T148 final reviews: ER428 attempt 17 passed PASS base=e6f1f95 head=d8a89b4; ER429 attempt 18 passed PASS base=e6f1f95 head=244af10.
# T149: ER415 passed on old head 404ec86, then superseded by the lane; ER416 passed on base=3f027a0 head=fc9792c.
```

```bash
sqlite3 .stores/db.sqlite "SELECT id,agent_name,display_id,transition_id,claimed_at,last_status,finished_at,postcondition_id,terminal_reason FROM dispatch_locks WHERE display_id IN ('T148','T149') ORDER BY id;"
# T149: accept-merge on accept => ok; cargo-install on accept => error; integrate on integration_queued => ok.
# T148: cargo-install on accept => error; accept-merge on accept => ok; integrate on integration_queued first => error; no dispatch_locks for post-integrated cargo/schema.
```

## Key code and configuration

- `stores/tasks/schema.yaml` lines 277-324 and 338-342: `accepted -> integration_queued`, `integration_queued -> integrating`, `integrating -> integration_blocked|integrated`, and `integration_blocked -> integration_queued` retry. `retry-integration` is `actor: ai_with_human` and intentionally re-traverses the lane.
- `src/flow/builtins/integrate.rs` lines 1-25: lane contract: claim singleton, stale-base check, refresh, ER head freshness, pre-land check, fast-merge, typed block/integrate.
- `src/flow/builtins/integrate.rs` lines 120-220: capacity claim and pre-flight `main`/candidate SHA capture.
- `src/flow/builtins/integrate.rs` lines 260-430: ensure candidate branch, refresh/rebase handling, write `candidate_head_after`, and post-refresh external-review head check (`stale_external_review`).
- `src/flow/builtins/integrate.rs` lines 430-560: pre-land check, freshness re-check under `main_branch` resource lock, and stale refresh/review/test resets.
- `src/flow/builtins/integrate.rs` lines 560-700: checkout main, verify resource-lock ownership, run `git merge --no-ff`, record `merge_failure`, then `mark_merge_done`/`mark_deploy_done`/`mark_verify_done` on success.
- `src/flow/freshness.rs` lines 1-125: `review_base_sha`/`review_head_sha` and `test_base_sha`/`test_head_sha` are compared against current main and branch head; overlap of `affected_scope` with main changes decides rereview/retest/refresh.
- `src/flow/builtins/cargo_install.rs` lines 1-150: stores-specific post-integrated install; expects generic status `integrated` and fires `mark_cargo_installed`.
- `src/flow/builtins/schema_migrate.rs` lines 1-125: stores-specific schema migration after `post_integration_step='cargo_installed'`; fires `mark_schema_migrated` or `mark_deploy_blocked`.
- `src/handlers/reconcile_accepted.rs` lines 1-155: operator-grounded recovery for rows already at `integrated`/`cargo_installed`; it re-runs cargo-install and/or schema-migrate but explicitly does not merge.
- `.stores/agents.yaml` lines 1-55: stale pre-T138 comments/subscribers still wire `accept-merge`, `cargo-install`, and `schema-migrate` to old post-accept edges.
- `.stores/agents.yaml` lines 132-171: current `integrate` subscriber is wired to `accepted|complete|in_review|integration_blocked -> integration_queued` with `pre_land_check: cargo check --quiet`.

## Architecture / data flow

1. Human acceptance (`accept`) records the decision. Framework then releases the task into `integration_queued`.
2. `builtin:integrate` claims `integration_queued -> integrating`, appends one JSON `integration_attempts[]` entry, rebases the task worktree, verifies latest non-superseded PASS head after refresh, runs `cargo check --quiet`, then locks and mutates `main`.
3. On recoverable failures it records a typed attempt outcome and fires `mark_integration_blocked`; recovery is `retry-integration`, which appends another full attempt and re-runs all checks.
4. On success it lands a merge commit on `main` and marks the task `integrated`; stores-specific deployment must then run cargo-install and schema-migrate as post-integrated subscribers or via `reconcile-accepted` if stranded.

## Findings

### 1. No stale T148 branch/main ref at final integration

T148's final candidate `244af10` is an ancestor of current `main`, and the merge base is the T148 head. During integration, `base_main_sha` was `e6f1f95` (the T149 merge), so T148 was integrated after T149, not against a pre-T149 base. ER429 reviewed exactly `244af10` on base `e6f1f95`, so `review_base/head` was fresh for the final T148 attempt.

### 2. T149 did exercise stale-review recovery

T149 attempt 2 correctly caught `stale_external_review`: ER415 reviewed `404ec86`, but rebase produced `fc9792c`. The lane superseded ER415 and blocked. After ER416 PASS on `fc9792c`, later retry could proceed. This is the intended `review_head_sha != candidate_head_after` guard.

### 3. Dirty main, not main-red tests, caused the merge failures

Both T149 attempt 3 and T148 attempt 1 failed at the `git merge --no-ff` step with Git's dirty-worktree protection: "Your local changes to the following files would be overwritten by merge". The lane truncates the diagnostic to `stderr.lines().next()`, so the affected file list is not preserved. Given this repo's repeated generated-file churn (`src/codegen/ddl.rs`, `Cargo.lock`/schema-generated projections, rendered task/worklog artifacts), the most plausible class is dirty generated files in the main checkout, but the DB does not retain the file names.

This is a lane observability gap: when dirty main blocks merge, store the full `git status --short` and full merge stderr tail in `integration_attempts[].pre_land_check_summary` or a separate diagnostic field. Otherwise recovery cannot distinguish dirty generated files from unrelated operator edits.

### 4. Live `.stores/agents.yaml` is stale for post-integrated deployment

The live agents file still says and wires:

- `accept-merge` on `in_review -> accepted` / `deploy_blocked -> accepted`;
- `cargo-install` on the same accepted edges;
- `schema-migrate` on `accepted -> cargo_installed`.

But current code intentionally unregisters `builtin:accept-merge` from normal dispatch and expects the T138 integration lane to own merge. Current `cargo-install` expects status `integrated`, and `schema-migrate` expects the post-integration step chain. The dispatch locks show exactly the resulting friction: stale accept-edge `cargo-install` ran and errored for both T148/T149, while no post-integrated cargo/schema dispatch lock appears after `mark_verify_done`.

### 5. Cargo-install/schema-migrate deployment happened, but likely through recovery not subscribers

Transition history shows `mark_cargo_installed` and `mark_schema_migrated` for T148/T149 after integration. Dispatch locks do not show post-integrated `cargo-install`/`schema-migrate` jobs, which points to operator recovery/direct builtins (the `reconcile-accepted` pattern) rather than subscriber-driven deployment. This is an operational gap: a task can be integrated yet require manual reconciliation because live agents wiring is stale.

### 6. Integration lane state machine helped provenance but made recovery ceremony-heavy

Helpful:

- Each retry has durable `integration_attempts` provenance.
- T149's stale external review was caught before merge.
- Dirty-main failures did not advance `main` and were recoverable.

Harmful/friction:

- `integration_blocked` requires human-grounded `retry-integration` even for transient dirty-main cleanup seconds later, so quick operator cleanup becomes multiple U4 hops.
- Because old accept-edge subscribers are still present, acceptance fires confusing obsolete jobs before integration starts.
- The lane records `integration_blocked_reason` even after final success; final status is `integrated/schema_migrated` but stale blocked reason remains on the row, making status/audit reads look worse than they are.

## Follow-ups

1. Update live `.stores/agents.yaml` to remove stale `accept-merge` accepted-edge subscribers and wire stores post-integrated subscribers to the T138/T146 shape (`integrating -> integrated` for cargo-install; `integrated -> integrated` with `post_integration_step=cargo_installed` for schema-migrate).
2. Add a pre-merge cleanliness check in `builtin:integrate` after acquiring `main_branch` and before `git checkout/merge`; record `git status --short` and full merge stderr tail on failure.
3. Consider auto-clearing `integration_blocked_reason` on successful `mark_integrated` or at least render it as historical when `blocked=false` and `post_integration_step=schema_migrated`.
4. Consider allowing framework auto-retry for typed transient `merge_failure` caused only by dirty generated files after the main checkout is clean, while keeping human retry for conflicts/stale review/pre-land failures.
5. Verify docs: `docs/integration-lane.md` still says schema-migrate subscribes to `integrated -> cargo_installed`, while current code uses `integrated -> integrated` plus `post_integration_step`.
