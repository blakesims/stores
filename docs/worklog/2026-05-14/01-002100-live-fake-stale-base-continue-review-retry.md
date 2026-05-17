# Continue Review Retry — Live Fake Stale-Base Red Proof

## Verdict

PASS as a valid **red TDD candidate** matching the broader user intent.

FAIL only against the original green expectation that the system would already refuse with a stale/freshness reason. The important result is stronger for TDD: the harness now creates the real live preconditions with fake runners only, attempts the normal path, and leaves a real watchable system issue instead of mocking the outcome.

## Findings

- Correct: `stale-base-refuses` is a first-class built-in live test preset and is routed to a dedicated live flow, not the generic happy-path integration loop (`src/cli/test.rs:141-158`, `src/cli/test.rs:195-200`, `src/cli/test.rs:264-290`).
- Correct: fake/no-LLM mode is enforced before live execution: `STORES_LLM_OFF=1`, fake runner/case env, fake executor mode, and fake-review acceptance are set in the harness environment (`src/cli/test.rs:166-188`). The live artifact also verifies this: T018 has only fake task `agent_runs`, and ER015 has runner `fake`.
- Correct: the harness fabricates real preconditions instead of writing target states. It waits for a real workspace/branch, records `main`, waits for fake ER PASS, verifies ER `head_sha` matches the real worktree `HEAD`, advances `main` with only the fenced marker path staged, then attempts normal `tasks accept` / `tasks enqueue-integration` / daemon integration (`src/cli/test.rs:494-551`, `src/cli/test.rs:700-728`, `src/cli/test.rs:731-835`).
- Correct: the continuation fixed the earlier false ambiguity: `stale-base-refuses` tasks are created with `task_review_policy=none`, so the observed `integration_step=task_review` stall is not explained away by an intentionally authoritative task-review policy (`src/cli/test.rs:1389-1392`, `src/cli/test.rs:1420-1425`).
- Correct: the harness now detects and reports the real issue quickly: an unfinished integrate dispatch lock after `mark_refresh_done`, `integration_attempts=null`, and the task parked at `integration_step=task_review` (`src/cli/test.rs:753-835`, `src/cli/test.rs:1059-1091`).
- Correct: the live row remains visible/recoverable for operator inspection. No generic isolation/deactivation path is used in `run_stale_base_refuses`; the command prints watch/status guidance instead (`src/cli/test.rs:590-594`).
- Correct: the plan note was committed with the code in `f6fe2f9` and accurately records the oracle-reviewed scope and the possibility of a red proof rather than a green stale refusal.
- Blocker: none found in the harness for the current red-proof purpose.
- Note: `./target/debug/stores watch --all` requires an interactive terminal; when run through this non-TTY review channel it errors with `No such device or address`, but Blake can run it directly in a terminal.

## Live red-proof artifacts

Current watchable candidate: `T018`.

- Task: `T018`
- Worktree: `/home/blake/repos/experiments/stores-T018-stores-test-live-stale-base-refuses-1778692395`
- Branch: `feat/T018-stores-test-live-stale-base-refuses-1778692395`
- Base A: `545c67d0b4d50027e0a42ebf13ce63a8bdfc6e71`
- Fake task head X: `2f213c9f7f9ada2406efbabffdc93b72d31f5efa`
- Fake ER: `ER015`, `status=passed`, `verdict=PASS`, `runner=fake`, `base_sha=545c67d0b4d50027e0a42ebf13ce63a8bdfc6e71`, `head_sha=2f213c9f7f9ada2406efbabffdc93b72d31f5efa`
- Additive main marker commit: `7bc06547bd56653bbb04e5448998473c03b19e3c`
- Marker path: `fake-runner-markers/T018-stale-base-refuses/main-advance.txt`
- Current task state: `status=integrating`, `lifecycle=integration`, `active_step=none`, `integration_step=task_review`, `task_review_policy=none`, `integration_attempts=null`
- Current dispatch evidence: latest `integrate` lock for T018 is unfinished, `attempts=0`, `finished_at=NULL`, `postcondition_id=integrated_state`
- Transition evidence: T018 reached `integration_queued -> integrating` and then `integrating -> integrating` via `mark_refresh_done`; no stale/freshness refusal transition was recorded.

This is a valid issue to press on: after fake ER PASS and genuine main movement, normal accept/enqueue succeeds and the daemon parks integration at a task-review substep with no integration attempt record and an unfinished integrate lock, rather than producing a typed freshness refusal or a successful integration attempt.

## Commands for Blake

Inspect the live row:

```bash
./target/debug/stores tasks status T018
./target/debug/stores watch --all
```

Inspect the exact DB evidence:

```bash
sqlite3 .stores/db.sqlite "select display_id,status,lifecycle,active_step,integration_step,task_review_policy,human_acceptance_policy,integration_attempts,blocked_reason,blocker_kind,integration_blocked_reason from tasks where display_id='T018';"
sqlite3 .stores/db.sqlite "select display_id,task_id,status,verdict,runner,base_sha,head_sha from external_reviews where task_id='T018' order by id desc limit 3;"
sqlite3 .stores/db.sqlite "select agent_name,display_id,claimed_at,finished_at,attempts,last_status,terminal_reason,postcondition_id from dispatch_locks where display_id='T018' order by id desc limit 5;"
sqlite3 .stores/db.sqlite "select id,from_status,to_status,verb,invoker,occurred_at,substr(actor_note,1,180) from transition_history where store='tasks' and display_id='T018' order by id;"
```

Reproduce a fresh red case:

```bash
cargo build --bin stores --bin stores-fake-agent
./target/debug/stores test run stale-base-refuses --live --watch
```

Expected current behavior: command exits nonzero with a RED-proof message unless the underlying engine bug has been fixed.

## Validation run

```bash
cargo fmt --check
cargo test -q cli::test --lib
cargo check -q
cargo build --bin stores --bin stores-fake-agent -q
./target/debug/stores tasks status T018
```

Results:

- `cargo fmt --check`: passed
- `cargo test -q cli::test --lib`: 15 passed
- `cargo check -q`: passed
- `cargo build --bin stores --bin stores-fake-agent -q`: passed
- `tasks status T018`: confirms the watchable red state above

## Recommendation

Use T018 as the immediate TDD red candidate. The next fix should target the integration daemon/lock path after `mark_refresh_done`: why does a `task_review_policy=none` task park at `integration_step=task_review` with `integration_attempts=null` and an unfinished integrate lock instead of continuing to freshness/integration or recording a typed failure?
