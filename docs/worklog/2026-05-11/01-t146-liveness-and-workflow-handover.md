# T146 Liveness And Workflow Handover

**Date:** 2026-05-11
**Type:** handover note

## Summary

This note captures the observed state of T146 after the overnight supervised run, the runner-liveness failure mode discovered during that run, the workflow-state behavior around `FAIL`/`resume`, and the evidence paths the next agent can use to reproduce what happened.

Goal for the next agent: get **T146 — Finish ADR 0001 task schema simplification after T144** to terminal success, merge/integrate it, then ratify/promote **L568 — Complete ADR 0002 upstream lifecycle model after T145 Phase 1** so ADR 0002 work is officially in planning. Do not continue using the broad auto-resume script pattern from this session.

## Current state observed

As of the last check in this session:

```bash
stores tasks status T146
# [01:33:58] T146 status=blocked phase=7/7 cycle=2 next=- blocked=true
# Activation: active
# Disposition: Blocked (recoverable)
```

Structured query used:

```bash
stores tasks show T146 --json | jq '{status,current_phase,current_cycle,blocked_reason,updated_at,drive_pid,drive_started_at,cycles_len:(.cycles|length),last:.cycles[-1]}'
```

Observed values from that query:

```json
{
  "status": "blocked",
  "current_phase": 7,
  "current_cycle": 2,
  "blocked_reason": "drive_failed:silent_zombie_pid_dead",
  "updated_at": "2026-05-11T01:33:35Z",
  "drive_pid": 3417986,
  "drive_started_at": "2026-05-11T01:18:30Z",
  "cycles_len": 12
}
```

Later, after further attempted recovery, another query showed T146 had additional cycle entries and was still blocked in phase 7:

```bash
stores tasks show T146 --json | jq '{status,current_phase,current_cycle,blocked_reason,cycles_len:(.cycles|length),last:.cycles[-1],plan_review_len:(.plan_review_log|length),last_plan_review:.plan_review_log[-1]}'
```

Observed values included:

```json
{
  "status": "blocked",
  "current_phase": 7,
  "current_cycle": 1,
  "blocked_reason": "drive_failed:silent_zombie_pid_dead",
  "cycles_len": 15,
  "last": {
    "phase": 7,
    "cycle": 1,
    "executor": {
      "commit": "4d5e2d238292f1f917b10ddbff9d2abe5e2ffa34",
      "files_changed": [],
      "summary": "T146 P7 verification-only cycle: no Phase 7 files changed; HEAD remains 4d5e2d238292f1f917b10ddbff9d2abe5e2ffa34. Verified cargo build, cargo test --all, ADR 0001 targeted suites, watch/TUI targeted suites, schema/docs/template greps, and AC7.9 Phase 7-scoped git status clean; unrelated dirty files remain unstaged/out of scope."
    },
    "review": null
  }
}
```

No live T146 process was present during the first investigation:

```bash
pgrep -af 'stores tasks drive T146|pi_runner|stores-T146|claude|codex' | head -80
# no T146 drive/pi_runner in the first investigation output; only unrelated claude/codex processes
```

During the later workflow race investigation, live T146 processes were observed:

```bash
pgrep -af 'stores tasks drive T146|pi_runner.*T146|stores-T146' || true
# 3839597 /home/blake/.local/share/stores/bin/stores tasks drive T146 --invoker ai_autonomous
# 3911947 node .../pi_runner.mjs --role code-reviewer --cwd /home/blake/repos/experiments/stores-T146-auto-promoted-l567 ...
# 3982955 /home/blake/repos/experiments/stores-T146-auto-promoted-l567/target/debug/deps/sidecar_handoff-...
# 3999310 /home/blake/repos/experiments/stores-T146-auto-promoted-l567/target/debug/deps/sidecar_handoff-...
```

## What was already proven / changed

### T145 Phase 1 was extracted to main

T145 Phase 1 was cherry-picked to main as:

```text
017e520 T145 P1: add ADR 0002 upstream projection read model
6d27bef T145 P1: map current ready contract state into ADR 0002 approved bucket
```

Validated with:

```bash
cargo test --lib flow::adr0002_projection
# 62 passed
```

T145 was abandoned via stores verbs after extracting Phase 1:

```bash
stores tasks status T145
# T145 status=abandoned ... Terminal retired
```

### Planning feedback rounds were increased

Commit on main:

```text
5ddc9c7 Increase planning feedback rounds to five
```

Validated with:

```bash
cargo test --lib ac5_8_submit_plan_review_needs_work_cycle_limit -- --nocapture
cargo build --locked
```

### Wall-clock liveness was changed to advisory on main

A direct fix was made because T146 repeatedly hit hard wall-clock kills:

```text
8d1c4e2 Make runner wall-clock liveness advisory
```

Files changed:

```text
src/runner/liveness.rs
src/flow/builtins/auto_drive.rs
src/runner/pi.rs
src/cli/watch.rs
```

Validated with:

```bash
cargo test --lib runner::liveness -- --nocapture
# 11 passed

cargo test --lib flow::builtins::auto_drive::tests::watchdog_classifies_alive_but_stalled_runner -- --nocapture
# 1 passed

cargo build --locked
```

Installed to the default `stores` binary path:

```bash
which stores
# /home/blake/.cargo/bin/stores

cargo install --path . --locked
# Replaced /home/blake/.cargo/bin/stores
```

The fix changes wall-clock elapsed from a kill/block condition to an advisory label. `stores watch` now emits a runner warning for unfinished locks whose liveness label contains `wall_clock_elapsed`.

Important: the fix was committed on **main**, not automatically merged into the T146 worktree. The T146 worktree still shows its own branch and dirty state; verify whether it contains or needs this fix before continuing.

## Evidence: hard wall-clock timeout killed legitimate long runs

The runner liveness defaults before the fix were seen in `src/runner/liveness.rs`:

```rust
no_output_secs: 600,
wall_clock_max_secs: 1800,
```

The old streaming runner code killed on elapsed wall-clock time:

```rust
if runtime > t.wall_clock_max_secs {
    let _ = child.kill();
    killed_for = Some(LivenessClass::WallClockTimeout { ... });
    break;
}
```

T146 logs showed real executor/code-reviewer runs often took 12–27 minutes and one was killed at ~30 minutes:

```bash
read logs/T146-manual-drive-20260511-041822.log
```

Observed excerpts from that file:

```text
[T146] phase 5 cycle 1: executor returned (exit=0, 703.9s)
[T146] phase 5 cycle 2: executor returned (exit=0, 959.0s)
[T146] phase 5 cycle 3: executor returned (exit=0, 1085.2s)
[T146] phase 6 cycle 1: executor returned (exit=0, 1648.9s)
[T146] phase 6 cycle 2: executor returned (exit=0, 1518.2s)
[T146] phase 7 cycle 1: executor returned (exit=-1, 1801.4s)
[T146] runner payload validation failed (exit=-1): runner timed out: total runtime 1801s exceeded wall_clock_max 1800s
```

Another log shows the same wall-clock kill:

```bash
read logs/T146-manual-drive-20260511-070739.log
```

Observed excerpt:

```text
[T146] phase 7 cycle 1: executor returned (exit=-1, 1801.2s)
[T146] runner payload validation failed (exit=-1): runner timed out: total runtime 1801s exceeded wall_clock_max 1800s
```

Transition history also shows wall-clock failures:

```bash
sqlite3 .stores/db.sqlite "select id, from_status, to_status, verb, invoker, occurred_at, substr(actor_note,1,180) from transition_history where store='tasks' and display_id='T146' order by id desc limit 80;"
```

Observed rows included:

```text
7033|executing|blocked|mark_drive_failed|framework|2026-05-10T17:34:54Z|wall_clock_1801s_max_1800s
7039|executing|blocked|mark_drive_failed|framework|2026-05-10T19:07:01Z|wall_clock_1800s_max_1800s
7049|executing|blocked|mark_drive_failed|framework|2026-05-10T20:47:27Z|wall_clock_1802s_max_1800s
7066|executing|blocked|mark_drive_failed|framework|2026-05-11T00:06:57Z|wall_clock_1801s_max_1800s
```

## Evidence: code-review FAIL caused resume back to planning

A code-reviewer returned `FAIL` on phase 7 cycle 3. The drive output from `/tmp/t146_finish_after_liveness_fix.sh` included:

```text
[T146] phase 7 cycle 3: code_reviewer returned (exit=0, 107.0s)
[T146] phase 7 cycle 3: code_reviewer → submitted (gate=Some(FAIL); source=sdk)
[T146] blocked: code-reviewer marked FAIL on phase 7: FAIL. This is cycle 3 and I would otherwise REVISE again: targeted Phase 7 tests and cargo build pass, but cargo test --all still fails and AC7.8 still includes out-of-scope liveness stabilization files in the single commit. Prior cycle-1 functional findings are addressed; the remaining blockers are release-contract failures.
```

The broad script then incorrectly auto-resumed this non-transient blocker. Transition history shows the state movement:

```text
7085|code_review|blocked|submit-review|ai_autonomous|2026-05-11T02:22:27Z|
7086|blocked|planning|resume|ai_with_human|2026-05-11T02:22:32Z|
7087|planning|plan_review|submit-plan|ai_autonomous|2026-05-11T02:26:16Z|
7088|plan_review|ready|submit-plan-review|ai_autonomous|2026-05-11T02:26:25Z|
7089|ready|executing|start|framework|2026-05-11T02:26:25Z|
```

Conclusion from observed data: `resume` after a code-review `FAIL` moved the task back to planning. That appears consistent with the workflow, but it was not appropriate for automation to do without operator review.

## Evidence: overlapping drives caused stale submit errors

The broad script spawned additional `stores tasks drive T146` invocations while other daemon/drive processes were also acting on T146. The user-visible errors were:

```text
Error: cannot submit-plan: row is in state 'executing', expected 'planning'
Error: cannot submit-execute: row is in state 'code_review', expected 'executing'
```

The transition history around the first error shows another process had already advanced the row from planning to executing while the planner result was still returning:

```text
7087|planning|plan_review|submit-plan|ai_autonomous|2026-05-11T02:26:16Z|
7088|plan_review|ready|submit-plan-review|ai_autonomous|2026-05-11T02:26:25Z|
7089|ready|executing|start|framework|2026-05-11T02:26:25Z|
```

The process list during the later investigation showed a live drive and code-reviewer:

```text
3839597 stores tasks drive T146 --invoker ai_autonomous
3911947 node ... pi_runner.mjs --role code-reviewer --cwd /home/blake/repos/experiments/stores-T146-auto-promoted-l567
```

Conclusion from observed data: the stale submit errors are consistent with multiple drives/agents trying to submit results after another process had already advanced the row.

## Evidence: code_review -> blocked -> code_review restarts reviewer

Observed transition history:

```text
7090|executing|code_review|submit-execute|ai_autonomous|2026-05-11T02:34:19Z|
7091|code_review|blocked|mark_drive_failed|framework|2026-05-11T02:42:02Z|silent_zombie_pid_dead
7092|blocked|code_review|resume|ai_with_human|2026-05-11T02:42:06Z|
7093|code_review|blocked|mark_drive_failed|framework|2026-05-11T02:43:50Z|silent_zombie_pid_dead
```

The script output at that time showed:

```text
[2026-05-11T09:42:06+07:00] loop T146: [02:42:06] T146 status=blocked phase=7/7 cycle=1 next=- blocked=true Activation: active Disposition: Blocked (recoverable)
[2026-05-11T09:42:06+07:00] resuming blocked T146: drive_failed:silent_zombie_pid_dead
Resumed T146; status now: code_review
[2026-05-11T09:42:07+07:00] driving T146: [02:42:07] T146 status=code_review phase=7/7 cycle=1 next=code-reviewer blocked=false Activation: active Disposition: Active engine work
[T146] phase 7 cycle 1: spawning code_reviewer via pi runner... (may take 30-90s)
```

Conclusion from observed data: resuming a `drive_failed:silent_zombie_pid_dead` blocker while interrupted in `code_review` restores the row to `code_review`, and a fresh drive spawns a new code-reviewer. This restarts code review from the brief; it does not continue the previous code-reviewer process.

## Agent context evidence

The code-reviewer brief files observed under `/tmp/stores-pi-*` contained the prior review context.

Commands used:

```bash
grep -n "Revision Context\|Code Review\|FAIL\|REVISE\|phase 7\|Current" /tmp/stores-pi-82u2zK/brief.md | head -80
grep -n "Revision Context\|Code Review\|FAIL\|REVISE\|phase 7\|Current" /tmp/stores-pi-YRxXWz/brief.md | head -80
```

Observed excerpts:

```text
18:**Current Phase:** 7 of 7
19:**Current Cycle:** 1
1972:- **Gate:** REVISE
1973:- **Summary:** REVISE. The prior review findings appear addressed and the targeted ADR 0001 tests pass, but AC7.8 is not satisfied...
2013:- **Gate:** FAIL
2014:- **Summary:** FAIL. This is cycle 3 and I would otherwise REVISE again: targeted Phase 7 tests and cargo build pass, but cargo test --all still fails and AC7.8 still includes out-of-scope liveness stabilization files...
2038:Evidence: Current Cycle is 3. The findings below would normally require another REVISE, but the workflow rule says if current_cycle == 3 and another REVISE would be needed, FAIL instead.
```

Conclusion from observed data: the restarted code-reviewer did receive prior REVISE/FAIL context in its brief. It was not blind to previous findings.

## T146 branch/worktree evidence

Worktree:

```text
/home/blake/repos/experiments/stores-T146-auto-promoted-l567
```

Branch from task row:

```text
feat/T146-auto-promoted-l567
```

Commits observed in that worktree:

```bash
git -C /home/blake/repos/experiments/stores-T146-auto-promoted-l567 log --oneline -8
```

Observed output included:

```text
b7af41b T146 P7: address primary consumer review findings
0aaf7ff T146 P7: port consumers to primary lifecycle columns
8f2dfcb T146 P6: remove stores post-integration verbs from lifecycle schema
c60405b T146 P6: quarantine stores post-integration state from task lifecycle
39a6198 T146 P5: make integration concurrency test assert attempted work
e8c8c00 T146 P5: fix acceptance policy test filters and release subscriber wiring
0c28b83 T146 P5: durable acceptance policy + release-to-integration subscriber
b183214 T146 P4: require machine-checkable affected scope before merge
```

Later cycle summary reported HEAD/commit:

```text
4d5e2d238292f1f917b10ddbff9d2abe5e2ffa34
```

The next agent should verify current HEAD before acting:

```bash
git -C /home/blake/repos/experiments/stores-T146-auto-promoted-l567 rev-parse --short HEAD
git -C /home/blake/repos/experiments/stores-T146-auto-promoted-l567 status --short
git -C /home/blake/repos/experiments/stores-T146-auto-promoted-l567 log --oneline -12
```

During one investigation, `git status --short` in the T146 worktree showed many modified files outside Phase 7 scope. The code-review FAIL text specifically said AC7.8 still included out-of-scope liveness stabilization files in a single commit. The next agent must inspect whether the T146 worktree currently includes unrelated/uncommitted changes before staging or merging anything.

## L568 / ADR 0002 state

L568 is drafted but not ratified/promoted yet.

Observed query:

```bash
stores observations show L568 --json | jq '{status,task_id,contract_state:.intent_contract.contract_state}'
```

Observed output:

```json
{
  "status": "open",
  "task_id": null,
  "contract_state": "draft"
}
```

The intended gating condition from the prior plan was: ratify/promote L568 only after T146 lands. That has not happened.

## What not to do next

Do not run broad auto-resume loops that resume every blocked state. The previous script auto-resumed a code-review `FAIL`, which caused `blocked -> planning` and restarted the workflow after a substantive review failure.

Do not run multiple concurrent `stores tasks drive T146` processes. The observed `cannot submit-plan` and `cannot submit-execute` errors are consistent with overlapping drives returning stale agent results after another process had advanced the row.

Do not `git add -A` in the T146 worktree. There were many unrelated dirty files observed there, and review findings explicitly complained about out-of-scope files being included.

## Recommended next steps

1. Stop or verify no overlapping T146 drive/runner processes:

```bash
pgrep -af 'stores tasks drive T146|pi_runner.*T146|stores-T146' || true
```

If live processes exist, inspect before killing; if they are stale/duplicative, stop them intentionally and record what was stopped.

2. Inspect current T146 DB row and transition history:

```bash
stores tasks status T146
stores tasks show T146 --json | jq '{status,current_phase,current_cycle,blocked_reason,drive_pid,drive_started_at,cycles_len:(.cycles|length),last:.cycles[-1]}'
sqlite3 .stores/db.sqlite "select id, from_status, to_status, verb, invoker, occurred_at, substr(actor_note,1,180) from transition_history where store='tasks' and display_id='T146' order by id desc limit 80;"
```

3. Inspect T146 worktree and commits:

```bash
git -C /home/blake/repos/experiments/stores-T146-auto-promoted-l567 status --short
git -C /home/blake/repos/experiments/stores-T146-auto-promoted-l567 log --oneline -12
git -C /home/blake/repos/experiments/stores-T146-auto-promoted-l567 show --stat --oneline HEAD
```

4. Review the latest code-review findings from the DB row rather than assuming the blocker is transient:

```bash
stores tasks show T146 --json | jq '.cycles[-1].review // .cycles[-2].review // empty'
```

5. If the current blocker is `drive_failed:*` and there is no live T146 runner, a single controlled resume/drive may be appropriate. If the blocker is a code-review `FAIL` or non-drive failure, halt and inspect; do not auto-resume.

6. If continuing T146 after the liveness fix, ensure the active `stores` binary is the one containing commit `8d1c4e2`, or run from main/current binary. The installed default binary was updated during this session with `cargo install --path . --locked`.

7. Once T146 reaches `in_review`, run external review, accept with the provided token only if review passes, and integrate. The prior token used in this session was provided by the user in chat; do not reprint it in logs/notes beyond the fact that token-mediated approval was used.

8. Only after T146 reaches terminal success (`schema_migrated` for this repo), ratify/confirm L568 so it promotes to a task and reaches planning. Verify with:

```bash
stores observations show L568 --json | jq '{status,task_id,contract_state:.intent_contract.contract_state}'
stores tasks status <L568 task_id>
```

## What is not yet known

- Whether the current T146 branch is actually mergeable and test-green after the latest phase-7 attempts. The last observed review context included both targeted-pass evidence and full-suite failure evidence; the next agent must rerun or inspect the latest committed state.
- Whether the T146 worktree currently contains unrelated dirty changes that must be left untouched, salvaged, or cleaned by the owning task. The next agent must inspect `git status --short` in the T146 worktree before staging anything.
- Whether the last live code-reviewer process observed during the session completed, was killed, or left partial output after the context was interrupted. Check process list and latest `.stores/runs/*.jsonl` files before resuming.
- Whether T146 should continue through the substrate after the code-review `FAIL`, or whether a narrower manual repair/review path is safer. The observed data says the automatic resume-from-FAIL path restarted planning; it does not prove that was the correct product decision.
