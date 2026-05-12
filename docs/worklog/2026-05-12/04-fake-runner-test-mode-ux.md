# Fake Runner Test Mode UX

**Date:** 2026-05-12
**Type:** note

## Summary

The fake runner is usable today through environment variables and existing `tasks drive`, `agents run`, `watch`, `status`, and `runs` commands. It is not yet the operator UX Blake wants: a first-class `stores test` verb that creates named synthetic cases, runs them under no-LLM mode, and shows them moving through the substrate.

Manual stress testing on the live stores DB found three important gaps:

1. `STORES_LLM_OFF=1 stores agents run --once` initially reexeced into a stale private daemon binary and launched a real Claude planner for an auto-promoted task. This is a test-mode preflight problem.
2. Fake external review cannot persist on the upgraded live DB because the existing SQLite `external_reviews.runner` CHECK constraint still allows only `codex`, `pi`, and `claude-code`.
3. `stall-no-heartbeat` blocks the task, but the user-visible failure is currently `payload_invalid` / `runner_payload_error`, not a clear liveness/watchdog failure.

Database backup before testing: `.stores/backups/db.sqlite.20260512-122249.bak`.

## Current UX: how to use it now

Build/install both binaries:

```bash
cargo install --path .
```

For daemon paths, ensure the private daemon binary is also current. In this repo the daemon reexec path is:

```bash
mkdir -p ~/.local/share/stores/bin
cp ~/.cargo/bin/stores ~/.local/share/stores/bin/stores
cp ~/.cargo/bin/stores-fake-agent ~/.local/share/stores/bin/stores-fake-agent
```

Minimal no-LLM drive:

```bash
export STORES_LLM_OFF=1
export STORES_FAKE_DELAY_MS=5000
export STORES_FAKE_SCENARIO=all-pass
export STORES_FAKE_EXECUTOR_MODE=marker_file
export STORES_FAKE_AGENT_BIN="$HOME/.local/share/stores/bin/stores-fake-agent"

stores tasks drive T123 --max-iters 30
```

Run one pending external review without starting daemon sweeps:

```bash
STORES_LLM_OFF=1 \
STORES_FAKE_DELAY_MS=5000 \
STORES_FAKE_SCENARIO=all-pass \
stores external_reviews run ER123
```

Run daemon once / detached in fake mode:

```bash
STORES_LLM_OFF=1 \
STORES_FAKE_DELAY_MS=5000 \
STORES_FAKE_SCENARIO=all-pass \
STORES_FAKE_EXECUTOR_MODE=marker_file \
stores agents run --once

STORES_LLM_OFF=1 \
STORES_FAKE_DELAY_MS=5000 \
STORES_FAKE_SCENARIO=all-pass \
STORES_FAKE_EXECUTOR_MODE=marker_file \
stores agents run --detach --log-file .stores/logs/fake-daemon.log
```

Watch surfaces:

```bash
stores watch --all
stores tasks status T123 --follow
stores runs current T123
stores runs tail T123
stores runs list T123
stores external_reviews list --status pending
```

Config-file equivalent:

```yaml
fake_runner:
  delay_ms: 5000
  seed: blake-demo
  scenario: all-pass
  executor_mode: marker_file
  fake_external_review: true
```

Available scenario names:

```bash
all-pass
plan-reviewer-reject-once
code-review-revise-once
external-review-revise-once
external-review-tooling-failure
payload-invalid-exit-0
nonzero-exit
long-delay-heartbeat
stall-no-heartbeat
sigterm-ignoring-stall
messy-prose-legacy-output
```

## Desired UX: `stores test`

The operator UX Blake is asking for is one layer above the fake runner:

```bash
stores test run happy-path --delay-ms 5000 --watch
stores test run t1 --delay-ms 5000 --watch
stores test run t2 --delay-ms 5000 --watch
stores test run t3 --delay-ms 5000 --watch
stores test run t3-er-fail --delay-ms 5000 --watch
stores test suite dogfood-smoke --delay-ms 5000 --watch
stores test suite battlescars --delay-ms 5000 --watch
```

That command should:

- set `STORES_LLM_OFF=1` mechanically;
- verify the effective `stores` and `stores-fake-agent` binaries before dispatch;
- create isolated synthetic tasks / worktrees / config per case;
- optionally run via daemon so watch/status surfaces move naturally;
- optionally run direct `tasks drive` / `external_reviews run` for tighter repros;
- summarize final task, external-review, agent-run, lock, and integration states;
- leave all transcripts/runs inspectable;
- never invoke Claude, Codex, Pi, or `node pi_runner.mjs` unless an explicit real-runner opt-back-in test says so.

A useful case manifest shape:

```yaml
cases:
  happy-path:
    tier: T3
    scenario: all-pass
    executor_mode: marker_file
    expect:
      task_status: in_review
      external_review: passed

  plan-reject-once:
    tier: T3
    scenario: plan-reviewer-reject-once
    expect:
      plan_review_first: NEEDS_WORK
      final_task_status: in_review

  code-review-revise-once:
    tier: T3
    scenario: code-review-revise-once
    expect:
      code_review_first: REVISE
      final_cycle: 2
      final_task_status: in_review

  er-tooling-failure:
    tier: T3
    scenario: external-review-tooling-failure
    expect:
      external_review_status: tooling_held

  payload-invalid:
    tier: T3
    scenario: payload-invalid-exit-0
    expect:
      task_status: blocked
      runner_exit_kind: payload_invalid

  no-heartbeat-stall:
    tier: T3
    scenario: stall-no-heartbeat
    expect:
      task_status: blocked
      runner_exit_kind: liveness_stalled_no_output
      dispatch_locks_released: true
```

## Battlescar repro cases from recent notes

Scout read the recent worklog notes from 2026-05-10 through 2026-05-12. The concrete repro cases Blake likely wants visible in test mode are:

1. **Watchdog / silent-zombie kill path** — fake runner emits partial transcript/status, then stalls or dies; watchdog must classify cleanly without losing telemetry.
2. **Legitimate long runner with heartbeat** — long fake run should stay live/advisory, not be killed solely by wall-clock.
3. **No-heartbeat controlled stall** — no heartbeat should take the liveness/watchdog path, not semantic REVISE or stale live marker.
4. **Runner infra crash / nonzero exit / SIGTERM-ignoring child** — distinct infra/signal failure classes.
5. **Payload-invalid with exit 0** — child exits successfully but final structured output is invalid/missing; task blocks as payload/tooling error.
6. **Duplicate same-task/same-worktree dispatch** — second drive for same task/worktree should refuse/hold, not race stale submits.
7. **Stale submit after overlapping drives** — stale `submit-plan` / `submit-execute` must fail safely after state has advanced.
8. **Resume after drive failure in code_review** — blocked code-review failure should resume the correct role with prior context.
9. **Code-review convergence guard** — repeated REVISE/FAIL should not spin forever.
10. **External-review REVISE loop / convergence stall** — T098-style ER non-convergence without token cost.
11. **Duplicate/noisy external-review attempts** — pending framework ER plus manual/recovery ER should fail/supersede, not dispatch parallel reviews.
12. **Stale-base / stale external-review freshness** — fake executor changes head or main moves; accept/integration should reject stale PASS.
13. **No-real-LLM sentinel** — under `STORES_LLM_OFF=1`, prove no `claude`, `codex`, `node pi_runner.mjs`, or Pi runner process launches.
14. **Fake-reviewed acceptance safety** — fake review cannot be accepted as production-reviewed without explicit test/allow marker.
15. **Executor realism / integration pressure** — marker/scripted commits push real git/integration paths.
16. **Stale binary update while worker is live** — updating installed `stores` should not misclassify in-flight work.
17. **Stale/dead current-run marker truth** — status/watch/runs-current distinguish marker-only stale state from live work.
18. **Foreground daemon/watch truth** — foreground/nohup daemon should not make watch report absolute daemon death incorrectly.
19. **Auto-drive disabled stays disabled** — fake mode should not redispatch when project config disables `builtin:auto-drive`.
20. **Spawn-side child binary preflight** — broken child binary should fail before `drive_pid` is recorded.

Source notes include:

- `docs/worklog/2026-05-12/01-llm-off-fake-agent-proposal.md`
- `docs/worklog/2026-05-12/02-fake-runner-no-llm-dogfood-proposal.md`
- `docs/worklog/2026-05-12/03-combined-fake-runner-implementation-plan.md`
- `docs/worklog/2026-05-11/01-t146-liveness-and-workflow-handover.md`
- `docs/worklog/2026-05-11/05-t148-remaining-runner-safety-partials-plan.md`
- `docs/worklog/2026-05-11/13-t148-autopsy-consolidation.md`
- `docs/worklog/2026-05-10/02-handover-watch-cockpit-daemon-triage-and-next-picks.md`
- `docs/worklog/2026-05-10/05-engine-controller-handover-liveness-and-integration.md`

## Manual stress test run — 2026-05-12

Preparation:

```bash
mkdir -p .stores/backups
cp .stores/db.sqlite .stores/backups/db.sqlite.20260512-122249.bak
cargo install --path .
cp ~/.cargo/bin/stores ~/.local/share/stores/bin/stores
cp ~/.cargo/bin/stores-fake-agent ~/.local/share/stores/bin/stores-fake-agent
```

Tasks created:

| task | scenario | result |
|---|---|---|
| T152 | all-pass, 5s delay | Drive reached `in_review`; pending ER created. Initial daemon ER accidentally used stale private binary/codex before private binary was updated. |
| T154 | all-pass, 2s delay | Drive reached `in_review`; all `agent_runs` show `harness_id=fake`, `provider_id=stores-fake`, scenario model id. ER431 fake run failed to persist due live DB CHECK constraint. |
| T155 | plan-reviewer-reject-once, 2s delay | First plan review `NEEDS_WORK`, second `READY`, final `in_review`. ER432 fake run failed to persist due live DB CHECK constraint. |
| T156 | code-review-revise-once, 2s delay | First code review `REVISE`, second `PASS`, final `in_review` at cycle 2. ER433 fake run failed to persist due live DB CHECK constraint. |
| T157 | payload-invalid-exit-0, 2s delay | Drive blocked immediately with `runner_payload_error`; `agent_runs.runner_exit_kind=payload_invalid`. |
| T158 | stall-no-heartbeat, 5s requested delay | Drive blocked after 60s, but as `payload_invalid` / `runner_payload_error`, not a clear liveness failure. |
| T159 | nonzero-exit, 1s delay | Drive blocked with `runner_crash`; `agent_runs.runner_exit_kind=infra_nonzero`. |

Verification query for T154-T159 showed drive roles used fake runner only:

```text
harness_id=fake
provider_id=stores-fake
api_id=stores-fake-agent-v1
model_id=fake-scripted:<scenario>-v1
```

External review fake dispatch did run the fake runner after the private binary update, but could not persist because of the existing live DB CHECK constraint:

```text
TOOLING_FAILURE: persist review result failed: CHECK constraint failed: runner IN ('codex', 'pi', 'claude-code')
```

Intake filed:

- `I044` — stale private daemon binary under `STORES_LLM_OFF` launched real runners.
- `I045` — live external_reviews DB CHECK rejects `runner=fake`.
- `I046` — `stall-no-heartbeat` user-visible failure is classified as payload invalid.

## Follow-ups

1. Build `stores test` as the operator-facing harness for named fake scenarios and suites.
2. Add a fake-mode preflight that checks both the current CLI binary and private daemon reexec binary before dispatch.
3. Ship/repair migration for existing live DBs whose `external_reviews.runner` CHECK constraint excludes `fake`.
4. Make `stall-no-heartbeat` visibly classify as liveness/watchdog in task status, blocked reason, and telemetry.
5. Add an inverse sentinel for `STORES_LLM_OFF=1 + fake_external_review: false` to prove explicit real-review opt-back-in is the only path to real runners.
