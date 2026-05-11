# T148 Autopsy Logs Db

**Date:** 2026-05-11
**Type:** note

## Summary

T148 did complete, but the DB/log chronology shows three distinct friction bands:

1. **Early runner/liveness churn before external review**: repeated `mark_drive_failed` blockers (`silent_zombie_pid_dead`, `stale_binary_inode`) and at least one duplicate-dispatch incident while the task was still in planner/executor/code-review cycles.
2. **External-review row noise and duplication**: `ER409`-`ER412` were stale-base/tooling rows; `ER417`/`ER418`, `ER421`/`ER422`/`ER423`, and `ER424`/`ER425`/`ER426`/`ER427` include repeated or overlapping findings, sometimes against the same head SHA. These rows inflated the apparent review attempt count and made “current authoritative finding” hard to identify.
3. **Nonconvergent loop after first external-review revise**: from `09:45Z` through `13:01Z`, each ER REVISE sent T148 back through `executing -> code_review -> complete -> in_review -> submit-external-review`, but watchdog/liveness failures and no-dispatch recovery repeatedly interrupted reconciliation. The loop became operationally nonconvergent by the ER420/ER421 region: fixes were being applied, but the substrate kept producing duplicate/noisy ER attempts and blocker/resume churn faster than the task could reach stable acceptance. The operator finally paused automation at `13:01:35Z`, manually reconciled a PASS (`ER428`/`ER429`), accepted at `13:11:45Z`, then integrated.

Final task row is clean enough (`status=integrated`, lifecycle `done`), but stale runner metadata remains in `tasks.drive_pid=705579` / `drive_started_at=2026-05-11T12:19:40Z` even though the task is integrated. Existing contemporaneous note `docs/worklog/2026-05-11/07-review-lifecycle-stabilization-plan.md` also records: “The old stale runner metadata still appears in task status, but no matching live process was found.”

## Evidence sources and commands

Ran from `/home/blake/repos/experiments/stores` unless noted:

```bash
stores tasks status T148
stores tasks show T148
sqlite3 .stores/db.sqlite ".schema tasks"
sqlite3 .stores/db.sqlite ".schema external_reviews"
sqlite3 .stores/db.sqlite ".schema transition_history"
sqlite3 -header -column .stores/db.sqlite "select display_id,status,lifecycle,active_step,integration_step,blocked,blocker_kind,updated_at,review_base_sha,review_head_sha,test_base_sha,test_head_sha,branch_head_sha,drive_pid,drive_started_at from tasks where display_id='T148';"
sqlite3 -header -column .stores/db.sqlite "select id,occurred_at,from_status,to_status,verb,invoker,lifecycle_from,active_step_from,integration_step_from,lifecycle_to,active_step_to,integration_step_to,actor_note from transition_history where store='tasks' and display_id='T148' order by id;"
sqlite3 -header -column .stores/db.sqlite "select id,display_id,status,created_at,updated_at,task_id,attempt,runner,model_id,substr(base_sha,1,8) base,substr(head_sha,1,8) head,verdict,critical_count,major_count,minor_count,started_at,completed_at,duration_ms,log_path,transcript_path,prior_review_ref,superseded_by,held_reason,attempts from external_reviews where task_id='T148' order by id;"
sqlite3 -json .stores/db.sqlite "select display_id,findings from external_reviews where task_id='T148' order by id"
sqlite3 -header -column .stores/db.sqlite "select id,display_id,phase,cycle,role,started_at,ended_at,exit_code,transcript_path,runner_exit_kind,payload_valid,payload_error from agent_runs where display_id='T148' order by id;"
```

Run/log paths inspected under:

```text
/home/blake/repos/experiments/stores-T148-auto-promoted-l568/.stores/runs/
```

External-review transcript/stderr log paths from DB:

- `c177a580-8be1-4e27-8d1a-2f8c273a12be.codex.{stderr,transcript}.log` (`ER414`)
- `64a4af5a-bf6b-44ab-b752-bf766f0fda9c.codex.{stderr,transcript}.log` (`ER417`)
- `c83766c5-d21e-4822-ab1d-684e7858181e.codex.{stderr,transcript}.log` (`ER418`)
- `bcba2362-72b5-45ef-9345-9bcaaf5b4054.codex.{stderr,transcript}.log` (`ER419`)
- `f0948266-d882-4505-a328-e749cfb91bc3.codex.{stderr,transcript}.log` (`ER420`)
- `a4b0eb20-2e99-4e77-ad1b-e443477acbdd.codex.{stderr,transcript}.log` (`ER421`)
- `5b03d745-aff9-4d0d-8b1c-226fe5964be7.codex.{stderr,transcript}.log` (`ER422`)
- `7caf4781-d5d9-4d36-98c7-7ab335a2735c.codex.{stderr,transcript}.log` (`ER423`)
- `d9909673-bc1d-484b-be3c-cab9c31a6fd7.codex.{stderr,transcript}.log` (`ER424`)
- `254998b2-76d1-4ee0-a3a4-9f20cdf50ad6.codex.{stderr,transcript}.log` (`ER425`)
- `64b3f4e7-ffc2-4003-be74-02912f6aaace.codex.{stderr,transcript}.log` (`ER426`)
- `5033744a-b195-44ab-b31c-64c1e569ee0c.codex.{stderr,transcript}.log` (`ER427`)
- `fe6aacdf-fc47-4e8f-b722-a5557837cafe.codex.{stderr,transcript}.log` (`ER428`)

Git history command:

```bash
git -C /home/blake/repos/experiments/stores-T148-auto-promoted-l568 \
  --no-pager log --oneline --decorate --reverse --all --date=iso \
  --pretty=format:'%h %ad %d %s' | grep -E 'T148|auto-promoted-l568|manual|ER|external'
```

## Chronology

### 04:08-09:40Z: normal task work mixed with runner failures

Important transition-history rows:

- `7241` `04:08:03Z`: task created as `planning`.
- `7242` `04:08:28Z`: immediately `planning -> blocked`, `mark_drive_failed`, note `silent_zombie_pid_dead`.
- `7243` `04:08:39Z`: resumed to `planning`.
- `7245` `04:14:06Z`: `plan_review -> blocked`, again `silent_zombie_pid_dead`.
- `7252` `04:48:13Z`: `executing -> blocked`, note `stale_binary_inode`.
- `7273` `06:57:28Z`: `executing -> blocked`, note `silent_zombie_pid_dead`.
- `7402`/`7403` `09:40:39Z`: code review passed to `complete`, then framework requested external review (`in_review`).

Agent-run DB rows show the task nevertheless made structured progress through planner, executor, and code-reviewer cycles. Phase/cycle highlights from `agent_runs`/`tasks.cycles`:

- P1 passed quickly (`04:57:58Z` executor, `05:01:00Z` reviewer PASS).
- P2 needed three code-review cycles before PASS.
- P3 needed two cycles.
- P4 needed two cycles.
- P5 passed one cycle.
- P6 took three in-substrate cycles before code-review PASS and wrap at `09:41:09Z`.

Relevant commits produced by these cycles include:

```text
06b5ee1 T148 P1: add ADR0002 upstream schema columns
cf304f0 T148 P1: stabilize ADR0002 schema and lifecycle regressions
78e7e54 T148 P2: write ADR0002 upstream tuples on transitions
234a5cc T148 P2 revise: enforce canonical intake causal trail
9f03c08 T148 P2 revise: preserve fast-track causal trail for supplied observations
0ec973c T148 P3: backfill ADR0002 upstream tuples from legacy rows
a9f4438 T148 P3 revise: assert ADR0002 backfill projection coverage
484d510 T148 P4: enforce architecture-review gates across linked observations
4a182c3 T148 P4 revise: route withdraw through architecture-review effects
56ae058 T148 P5: render upstream flow from ADR0002 primary state
321349a T148 P6: quarantine upstream status compatibility and assert ADR boundaries
1141bce T148 P6 revise: quarantine legacy status and stabilize final suite
3690f09 T148 P6 revise: gate architecture rulings on ADR0002 lifecycle
```

### 09:40-09:45Z: stale-base/tooling ER row burst, then first substantive ER

External-review rows:

- `ER409` attempt 1 at `09:40:40Z`: `TOOLING_FAILURE`, stale-base preflight. Findings say rebase failed because there were unstaged changes.
- `ER410` attempt 2 at `09:41:09Z`: same base/head (`e723ef8a` -> `93dfd307`), same stale-base/unstaged-change failure. This appears duplicative/noisy with `ER409`.
- `ER411` attempt 3 at `09:42:33Z`: stale-base failure on head `7184bab8`; later superseded by `ER414`.
- `ER412` attempt 4 at `09:43:10Z`: stale-base/conflicted rebase, conflicted file `docs/engine-health.md`, long stderr full of skipped cherry-picks, failed applying a T148 P6 commit. It has no corresponding successful task transition; it is another noisy tooling row.
- `ER414` attempt 5 at `09:44:35Z`: first real Codex REVISE, base `e723ef8a`, head `b212cab1`, completed `09:45:24Z`.

`ER414` transcript begins:

```text
The patch leaves at least one documented architecture-review cardinality path broken and one subscriber path inconsistent with ADR 0002's primary contract_state enum. These require executor changes before the task can pass.
```

Transition `7437` at `09:45:24Z` then sent `in_review -> executing` with actor note `external-review`, establishing the post-external-review revise loop.

### 09:45-11:14Z: first repair attempt interrupted by pause/block/resume semantics

Key transitions:

- `7437` `09:45:24Z`: `in_review -> executing` from `ER414`.
- `7449` `10:16:51Z`: manual `deactivate`, actor note: `stabilization-first: pause review/wrap churn while lifecycle-stage/external-review semantics are cleaned up`.
- `7451` `10:26:46Z`: `executing -> blocked`, `silent_zombie_pid_dead` while active step was `wrapping`.
- `7486` `11:14:37Z`: resumed to `executing` with note `no-dispatch: recovery resume left activation inactive and skipped on-entry follow-ons`.
- `7487`/`7488`/`7489`/`7490`: quick manual/automated repair cycle back to `in_review` by `11:27:13Z`.

This region matches the engine repair commits in main/worktree history around duplicate dispatch/live status/no-dispatch recovery:

```text
4532176 docs: plan T148 duplicate dispatch repair
9067576 T148 fix live run selection
f579e27 T148 guard duplicate auto-drive dispatch
30f965d Merge T148 live-run status selection fix
dbc45cb Merge T148 duplicate dispatch guard
c70cbb7 Add no-dispatch resume recovery
d3c70aa Add binary identity diagnostics
```

Contemporaneous `docs/worklog/2026-05-11/04-t148-duplicate-dispatch-live-runner-repair-plan.md` records the concrete duplicate process shape:

```text
471567 stores tasks drive T148 --invoker ai_autonomous
510248 node ... pi_runner.mjs --role executor ...
530874 stores tasks drive T148 --invoker ai_autonomous
530908 node ... pi_runner.mjs --role executor ...
```

It also records that a completed planner marker was masking a live executor and that executor marker `updated_at` could remain at spawn time while semantic events advanced. That explains the “stale runner metadata/status” part of this friction area: the task row and live-status surfaces were not reliable indicators of the actual runner.

### 11:27-11:40Z: rapid external-review loop, duplicates start becoming clear

Rows/transitions:

- `ER417` attempt 6: created `11:27:19Z`, started `11:29:20Z`, completed `11:30:04Z`, head `46da1168`, REVISE with 1 major. Transition `7498` sent `in_review -> executing`.
- `ER418` attempt 7: created `11:29:31Z` before `ER417` completed, same base/head (`e6f1f951` -> `46da1168`), completed `11:30:49Z`, REVISE with 0 major/minor counts but same substantive issue. This is a duplicate/noisy row for the same semantic review.
- `ER419` attempt 8: head `5369367e`, completed `11:33:44Z`, REVISE (TUI loading architecture_reviews).
- `ER420` attempt 9: head `ce2bfe19`, completed `11:38:30Z`, REVISE with 0 counts but clear blocking prose.

The duplicate pair evidence:

`ER417` transcript:

```text
[major] tests/architecture_reviews_cardinality.rs:39 builds the test --linked-observations arg with ArgAction::Append but no .value_delimiter(',').
```

`ER418` transcript:

```text
A newly added cardinality test helper still does not parse comma-separated linked observations ...
[P1] Split linked observations in cardinality test helper — tests/architecture_reviews_cardinality.rs:39-42
```

Both target the same head `46da1168`, same defect, and were created/started with overlapping timestamps. `ER418` is therefore not a distinct semantic review result.

Transition churn in this segment:

- `7507` `11:33:44Z`: `in_review -> executing` for `ER419`.
- `7508` `11:34:52Z`: `executing -> blocked`, `silent_zombie_pid_dead`.
- `7509` `11:36:42Z`: resume no-dispatch back to `executing`.
- `7517` `11:38:30Z`: `in_review -> executing` for `ER420`.
- `7518` `11:39:00Z`: `executing -> blocked`, `pid_never_recorded`.
- `7519`/`7520`/`7521`: resume/activate/submit-execute.
- `7522` `11:49:38Z`: code-reviewer submitted `blocked` (not complete), which sent the task into a worse recovery path.

### 12:03-12:19Z: resume resets phase state; ER421-ER427 converge on real findings but substrate loop is noisy

After `7522` blocked, transition `7523` at `12:03:02Z` resumed `blocked -> planning`, not just back to the interrupted external-review repair. Then `7524`-`7530` rapidly replayed planning, plan review, execution, code review, complete, and in_review in 36 seconds. This created a duplicated-looking `tasks.cycles` shape: P6 cycles 1-7 from the original task, then another set of P6 cycles 1-5 with mostly manual/no-transcript entries.

External-review rows:

- `ER421` attempt 10, head `9c28378d`, REVISE, completed `12:07:06Z`, two major findings: linked-observation gate clearance and superseded predecessor typed reference.
- `ER422` attempt 11, created `12:10:13Z`, head `39a55e8d`, started much later at `12:16:35Z`, completed `12:16:58Z`, REVISE. This row appears stale by the time it ran: later rows were already active/completed against nearby heads.
- `ER423` attempt 12, also created `12:10:13Z`, head `a416f8be`, completed earlier at `12:11:59Z`, REVISE on deferred architecture effects for amend reviews.
- `ER424` attempt 13, created `12:15:30Z`, head `39a55e8d`, completed `12:16:06Z`, REVISE on supersede CLI + legacy auto-resolve. Same head as `ER422`, but it completed before `ER422` even started.
- `ER425` attempt 14, created `12:15:35Z`, same head `39a55e8d`, completed `12:16:33Z`, same semantic findings as `ER424`, but counts differ (`major_count=0`). Duplicate/noisy.
- `ER426` attempt 15, head `23ab20f5`, completed `12:19:04Z`, REVISE on test inconsistency for supersede CLI path.
- `ER427` attempt 16, created `12:18:48Z`, same head `23ab20f5`, completed `12:19:15Z`, same semantic finding as `ER426`, duplicate/noisy.

The most obvious noisy clusters:

```text
ER421: head 9c28378d, two real major findings.
ER422: head 39a55e8d, starts at 12:16:35, after ER423/ER424/ER425 creation; stale/noisy ordering.
ER424/ER425: same head 39a55e8d, same supersede/auto-resolve class; duplicate semantic review.
ER426/ER427: same head 23ab20f5, same supersede-cardinality-test finding; duplicate semantic review.
```

Transitions showing watchdog/recovery still interleaving:

- `7534` `12:07:06Z`: `ER421` sends `in_review -> executing`.
- `7544` `12:11:59Z`: `ER423` sends `in_review -> executing`.
- `7545` `12:14:39Z`: `executing -> blocked`, `silent_zombie_pid_dead`.
- `7546`-`7550`: resume/activate/submit-execute/submit-review/request-review all at/near `12:15:30Z`.
- `7555` `12:16:06Z`: `ER424` sends `in_review -> executing`.
- `7561` `12:17:26Z`: `executing -> blocked`, `pid_never_recorded`.
- `7562`-`7566`: resume/activate/submit-execute/submit-review/request-review all at `12:18:44Z`.
- `7571` `12:19:04Z`: `ER426` sends `in_review -> executing`.

This is the point where the process was nonconvergent operationally even though the code findings were individually actionable: each fix generated another review, but the runner/ER lane was creating overlapping rows and requiring manual resumes/activations.

### 13:01-13:13Z: operator pause, manual PASS import, acceptance, integration

Key transitions:

- `7581` `13:01:35Z`: manual `deactivate`, actor note: `manual pause: automated ER/executor loop no longer converging; operator will move task manually`.
- `7582`/`7583`/`7584`: final manual recovery cycle to `in_review` by `13:07:54Z`.
- `7589` `13:11:45Z`: `in_review -> accepted`, invoker `human`.
- `7591`-`7607` `13:11:52Z`-`13:12:47Z`: integration queued, refreshing, task_review, testing, merging, deploying, verifying, then `integrated`.
- `7597` `13:12:34Z`: one `integration_blocked` on merging, immediately retried at `7599` `13:12:47Z`.
- `7609`/`7610` `13:13:30Z`: post-integration `mark_cargo_installed` and `mark_schema_migrated`.

Final ER rows:

- `ER428` attempt 17: created `13:08:02Z`, head `d8a89b42`, PASS, completed `13:08:38Z`.
- `ER429` attempt 18: created/completed `13:11:38Z`, head `244af103`, PASS, `duration_ms=0`, `transcript_path=/tmp/t148-manual-er-pass.txt`, no log path. This is explicitly a manual import / DB-compatible PASS row, not a normal Codex run.

Relevant final commits:

```text
81bc462 On feat/T148-auto-promoted-l568: preserve interrupted T148 executor WIP after stopping nonconvergent loop
d8a89b4 T148 codex-revise: isolate routed test fixtures
244af10 T148 recovery: keep manual ER imports DB-compatible
55caf51 Merge branch 'feat/T148-auto-promoted-l568'
d8ff8d0 docs: record T148 recovery closure
```

## Duplicated/noisy external review rows

### Stale/tooling rows before first real review

- `ER409` and `ER410`: same base/head, same unstaged-change stale-base failure. Treat as duplicate tooling noise.
- `ER411` and `ER412`: same base/head, stale-base/conflicted rebase class; `ER412` contains the useful conflict detail, but both are tooling-held/pre-review noise.

All four were later marked `superseded` at `13:08:36Z`, but that update happened after the main loop had already consumed operator attention. Their `held_reason=cap-held` and `attempts=1` values are confusing because the findings themselves say `held_reason: stale_base_requires_rebase`.

### Same-head duplicate Codex reviews

- `ER417` and `ER418`: same head `46da1168`, same cardinality test-helper finding. `ER418` was created before `ER417` completed. This is a duplicate dispatch/concurrency artifact.
- `ER424` and `ER425`: same head `39a55e8d`, same supersede/auto-resolve findings, inconsistent count fields.
- `ER426` and `ER427`: same head `23ab20f5`, same cardinality supersede-test issue.

### Stale ordering/noisy currentness

- `ER422` was created at the same time as `ER423` but started after `ER423`, `ER424`, and `ER425` had already completed or started. Its head (`39a55e8d`) was no longer clearly the current semantic state. This is a good example of why “latest row by attempt number” was not necessarily “authoritative current review.”

## Stale runner metadata / status confusion

Current final task row:

```text
status=integrated
lifecycle=done
active_step=none
integration_step=none
blocked=0
updated_at=2026-05-11T13:13:30Z
drive_pid=705579
drive_started_at=2026-05-11T12:19:40Z
review_base_sha=e6f1f951...
review_head_sha=244af103...
test_base_sha=e6f1f951...
test_head_sha=244af103...
branch_head_sha=244af103...
```

The `drive_pid`/`drive_started_at` are stale after integration. They point to the old recovery/loop period rather than an active runner. This matches the stabilization note stating no matching live process was found.

Earlier live-runner/status confusion is documented in `04-t148-duplicate-dispatch-live-runner-repair-plan.md`:

- two `stores tasks drive T148` processes and two `pi_runner --role executor` processes were observed concurrently;
- status selected a completed planner marker while a live executor emitted events;
- marker `updated_at` could stay stale while semantic `status.json.last_event_at` advanced;
- dispatch lock pid pointed to the newer drive tree while the older duplicate was still present.

Hypothesis: T148’s status/no-dispatch recovery work improved live selection during the run, but the durable task row still lacks a cleanup/invalidation edge for `drive_pid`/`drive_started_at` on terminal task transitions (`accepted`, `integrated`, or `done`).

## Where the loop became nonconvergent

The loop was not purely “bad Codex findings.” Many ER findings were legitimate and got fixed. The nonconvergence was the interaction of legitimate findings with runner/ER lane churn:

1. `ER414` started the external-review revise loop at `09:45:24Z`.
2. `10:16`-`11:14` shows the first pause/block/resume cycle because automation and runner state were already unreliable.
3. `ER417`/`ER418` prove duplicate same-head review creation by `11:29`-`11:30`.
4. `11:34` and `11:39` blockers (`silent_zombie_pid_dead`, `pid_never_recorded`) interleaved with ER reconciliation.
5. `12:03` resume reset the row to `planning`, creating duplicate-looking P6 cycles and fast replay into review.
6. `ER424`/`ER425` and `ER426`/`ER427` repeated same-head findings after the loop should have been narrowing.
7. Operator explicitly declared nonconvergence in transition `7581` at `13:01:35Z` and moved manually.

Best single timestamp for “became nonconvergent”: **around `12:15`-`12:19Z`**, when the system had already required multiple no-dispatch resumes, had reset through planning, and then produced same-head duplicate ER rows (`ER424`/`ER425`, `ER426`/`ER427`) with more blocker churn. The explicit human-recognized point is `13:01:35Z`.

## Hypotheses / follow-ups

1. **External review needs authoritative-current semantics.** Same task/head should not spawn overlapping review rows unless intentionally marked as duplicate/stale. At minimum, duplicate same-head rows should be auto-superseded/excluded from “current finding” surfaces.
2. **Held/tooling reason columns are inconsistent.** `ER409`-`ER412` findings say `stale_base_requires_rebase`, but table rows later show `held_reason=cap-held`. The row-level held reason lost the original operational reason.
3. **Task transitions should clear runner identity on terminal/blocked paths.** `drive_pid` persists after `integrated`; status consumers need either cleanup or explicit stale labels.
4. **Resume from blocked during wrapping/external-review repair is too coarse.** The `12:03` `blocked -> planning` reset caused confusing duplicate P6 cycles. Recovery should preserve the semantic lane/step when possible, or record that the next cycles are recovery cycles.
5. **ER reconciliation from blocked must be idempotent.** Several transitions show ER result, watchdog block, and manual resume racing. The task should not need manual activate/resume gymnastics to consume a terminal ER verdict.
6. **Analysis should classify T148 ER rows.** For model-quality metrics, exclude tooling rows (`ER409`-`ER412`), duplicate same-head rows (`ER418`, likely `ER425`, `ER427`), stale/out-of-order row (`ER422`), and manual import (`ER429`). Keep them for operational reliability metrics.

## Follow-ups

- Link this note to the T148 ER/daemon and schema/lifecycle autopsy notes so each area uses the same canonical ER row classification.
- File or update an observation for stale `drive_pid`/`drive_started_at` cleanup on terminal task transitions if not already covered.
- File or update an observation for same-head external-review duplicate suppression/current-authority marking if not already covered.
