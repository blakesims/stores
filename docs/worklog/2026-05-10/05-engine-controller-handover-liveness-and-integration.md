# Engine Controller Handover Liveness And Integration

**Date:** 2026-05-10
**Type:** note

## Summary

Handover for the next agent acting as engine controller. The engine is now capable of shipping work end-to-end, but the operator still needs active babysitting around: stuck drives, stale binary / stale base recovery, external review timing, inactive activation gates, and post-accept integration ceremony. Today we shipped T141 and T143 through to `schema_migrated`, fixed one resume semantics bug directly on `main`, and created/landed the runner-liveness task from L121.

Current anchor commits:

- `6da2b9b` — direct fix on `main`: `resume` now restores `drive_failed:*` rows to the interrupted status instead of always rewinding to planning.
- `e0c495f` — T141 merged: watch cockpit rich store drilldowns.
- `cbbbc54` — T143 merged: runner liveness / heartbeat / no-output stall foundation.

## Details

### What to check first in a new session

Run these before touching anything:

```bash
stores engine plan-start
stores tasks status T143
stores tasks status T141
stores tasks status T142
stores observations show L121 --json | head
stores watch
```

Interpretation reminders:

- `schema_migrated` = terminal success for stores-repo tasks after merge + cargo-install + schema-migrate.
- `accepted` = human/token accepted but not necessarily integrated. If it stays accepted, run `enqueue-integration` and then `run-integration`.
- `integrated` = merged but post-integrated hooks may still be pending. Run `reconcile-accepted` if the daemon did not fire cargo-install/schema-migrate.
- `in_review` = wrap/external-review/acceptance gate. It is not merged. Usually run `stores tasks drive <id>` once to submit wrap if needed, then wait for latest ER to PASS, then `tasks accept` with token.
- `blocked:drive_failed:*` is usually infrastructure, not necessarily bad code. Inspect transition history and process tree before deciding whether to resume, abandon, or fix.

### Today's concrete lessons

#### T142: stuck executor not recognized

T142 was `executing` and `blocked=false`, but executor child `cargo build -p stores` had been running ~50 minutes in futex/artifact-lock wait. Stores saw the drive/executor PID alive and did not classify it as stuck. Manual recovery path:

1. Kill only wedged cargo first.
2. If executor respawns/leaves multiple task-owned cargo/cc children, kill only processes matching the task worktree / `tasks drive <id>`.
3. Preserve worktree.
4. Run build/tests manually if needed.
5. Commit safe work.
6. Resume/drive through substrate.

Filed intake: `I038` for alive-but-stuck child process visibility. This was later folded into L121/T143.

#### T141: stale binary + bad resume semantics

T141 hit `drive_failed:stale_binary_inode` after T142 replaced the installed stores binary. The watchdog was correct to block old drive processes, but `tasks resume` rewound to planning and discarded phase/cycle progress. That was too coarse.

Direct fix shipped on `main` in `6da2b9b`:

- For ordinary/user-level blockers, `resume` still returns to planning.
- For `drive_failed:*`, `resume` looks at latest `mark_drive_failed` transition and restores the interrupted status (`executing`, `code_review`, etc.).
- It preserves `current_cycle` for drive failures and still clears stale `drive_pid`, `drive_started_at`, and auto-drive locks.
- It also handles the T141 double-failure shape where an old bad resume already rewound to planning and then that fresh planner drive failed; in that case it recovers the pre-rewind interrupted status.

Important gotcha from testing: `which stores` may point at `/home/blake/.cargo/bin/stores` while daemon/private install uses `/home/blake/.local/share/stores/bin/stores`. After direct engine fixes, install both if you need the operator shell and daemon/private path aligned:

```bash
cargo install --path . --features runner-claude-code,runner-pi --bin stores
cargo install --path . --features runner-claude-code,runner-pi --root /home/blake/.local/share/stores --bin stores
```

#### T141 final path

T141 got to `in_review`, but latest ER was `tooling_held stale_base_requires_rebase` because the task branch had unstaged task projection noise. Recovery:

1. Stash tracked dirty task projection files only; leave unrelated untracked files alone.
2. Rebase task branch onto `main`.
3. `stores tasks recover-stale-base T141 --invoker ai_with_human --approve-token <token>`.
4. Wait for new ER PASS.
5. Accept, enqueue integration, activate if needed, run integration, reconcile.

Final: T141 `schema_migrated`, merge `e0c495f`.

#### L121 → T143 ratification UX trap

Watch showed L121 with `contract_state=ready` and `auto-promote eligible`, but it was still lifecycle `open`. Auto-promote did not fire until lifecycle moved through:

```bash
stores observations investigate L121 --invoker ai_autonomous --investigation-note '...'
stores observations confirm L121 --invoker ai_with_human --approve-token <token>
# framework auto-ratifies confirmed → ready
# auto-promote creates task
```

The UI text is misleading. Contract field readiness is not lifecycle ratification. Future UI fix should explicitly say `contract fields ready; lifecycle open; next: investigate → confirm; auto-promote waits for status=ready`.

T143 initially blocked with `drive_failed:silent_zombie_pid_dead` before planning produced output. Activation/resume then worked:

```bash
stores tasks activate T143 --invoker ai_with_human --approve-token <token> --reason 'Start ratified L121 liveness task'
stores tasks resume T143 --invoker ai_with_human --approve-token <token> --summary 'Resume initial silent-zombie drive failure before planner produced output.'
stores tasks drive T143 --invoker ai_autonomous
```

During T143 drive, planner took ~350s despite log saying 30–90s; this is exactly the liveness pressure T143 was meant to improve. T143 eventually reached `in_review`, ER revised once, cycle 4 fixed it, ER384 passed, then it was accepted and integrated.

Final: T143 `schema_migrated`, merge `cbbbc54`.

### What T143 changed

T143 shipped a liveness foundation:

- New shared `src/runner/liveness.rs` streaming/liveness helper.
- Pi runner wired through liveness.
- Cargo install path bounded by liveness.
- Drive heartbeat integration.
- Auto-drive watchdog liveness classification.
- Watch engine-health rendering for liveness/heartbeat data.
- Tests in `tests/tui_watch_drilldowns.rs` and runner/liveness modules.

Task commits:

- `38eb89f` — bound runner liveness and surface heartbeats.
- `281db67` — centralize runner heartbeat writes.
- `5b38f71` — isolate heartbeat pump paths per runner thread.
- `397cddb` — drain killed liveness streams before reader joins.

Expected operator-visible improvement: hangs should become more visible and bounded. This is a foundation, not complete self-healing.

### Engine-control operating procedure

When a task looks stuck:

1. Check substrate state:

   ```bash
   stores tasks status <TID>
   stores tasks next-action <TID>
   stores tasks show <TID> --json > /tmp/<TID>.json
   sqlite3 .stores/db.sqlite "select id,from_status,to_status,verb,invoker,occurred_at,actor_note from transition_history where display_id='<TID>' order by id desc limit 20;"
   ```

2. Check processes:

   ```bash
   ps -ef | rg '<TID>|stores tasks drive|cargo|codex|claude|pi_runner' | rg -v rg
   pstree -ap <drive-pid>
   readlink /proc/<pid>/exe 2>/dev/null
   ```

3. Check artifacts/logs:

   ```bash
   rg -l '<TID>|branch-slug|linked-observation' .stores/runs .stores/logs 2>/dev/null | tail -50
   find /tmp/claude-1000 -path '*<TID>*' -type f 2>/dev/null | tail -50
   ```

4. If infra-only block (`drive_failed:*`), prefer resume with token after understanding latest transition. With `6da2b9b`, resume should preserve interrupted status for drive failures.

5. If `in_review`, do not assume accepted/merged. Drive wrap if needed, wait for latest external review to PASS, then accept.

6. If `accepted`, do not assume integration happened. If still accepted:

   ```bash
   stores tasks enqueue-integration <TID> --invoker ai_with_human --approve-token <token>
   stores tasks run-integration <TID> --invoker ai_autonomous
   stores tasks reconcile-accepted <TID> --invoker ai_with_human --approve-token <token>
   ```

7. If ER is `stale_base_requires_rebase`, rebase the task branch, then:

   ```bash
   stores tasks recover-stale-base <TID> --invoker ai_with_human --approve-token <token>
   ```

8. Never raw-SQL write the DB. Read-only SQL is fine.

### Current watch/UI UX gaps seen live

- Observation detail conflates `intent_contract.contract_state=ready` with lifecycle ratification. It should distinguish field readiness from `status=ready` and show exact next command.
- `in_review` is still unclear to the operator. Watch should explain: `wrap/ER/accept gate; latest ER status; next command`.
- Accepted active tasks can be easy to miss. Watch should make `Awaiting integration (active)` prominent and suggest enqueue/run integration if daemon did not fire.
- Activation/inactive rows can hide important work. `plan-start` helps, but watch should surface inactive-but-ready/accepted rows as operator valves.

## Follow-ups

High-leverage next work after T141/T143:

1. **Integration-point external review.** Make the authoritative ER run after branch refresh/rebase immediately before merge. Today stale-base/recover-stale-base still appears after wrap/accept attempts.
2. **UI fix for observation ratification state.** Contract-ready but lifecycle-open rows should say `investigate → confirm` and not imply auto-promote already happened.
3. **UI fix for `in_review` and accepted/integration states.** Show latest ER and exact next command.
4. **Formalize direct resume fix.** `6da2b9b` is a direct main commit; if needed, file/close an observation so doctrine history points to it.
5. **Priority/ranking primitive (L084).** Engine-health says this is the next backlog control surface once cockpit/liveness are usable.
6. **Keep an eye on T143 liveness behavior in real use.** Verify future stuck Pi/cargo cases now surface as liveness classifications rather than invisible hangs.
