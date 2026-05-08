# Handover 2026 05 08 Cleanup

**Date:** 2026-05-08
**Type:** note

## Summary

Manual no-dogfood cleanup of the overnight cascade. Daemon stopped, T086 closed-out-of-band, six stuck branches (T084/T085/T088/T093/T095/T096) rebased clean onto local main with tests green, engine-health.md refreshed for 27 shipped tasks. Daemon remains OFF pending Blake's explicit call to restart.

The cascade root was a single missed step from yesterday's session: T086 (the meta-substrate external_reviews reconciler) was hand-merged at `8bd21b6` per Pi msg_ccfb6b59 (chicken-and-egg: T086 fixes its own deploy gate), but the planned `tasks close-out-of-band T086` never ran. With main moved underneath, six in-flight branches inherited rebase debt; runner crashed on stale base; T041's retry-on-failure rescheduler ran cycle counts to 27/35/50/55/93. Codex/pi rate limits amplified the loop — `runner_crash exit_code=3` is leaking through what T029/L071 was supposed to type as `blocked:rate_limit`.

## Details

### What was done this session

1. **Daemon stopped.** `kill -TERM 391252` (was running foreground without `--detach` so `stores agents stop` didn't apply). Process gone.
2. **T086 closed-out-of-band** with `--commit 8bd21b6 --invoker human`. Status: blocked → closed_out_of_band. Substrate row state now matches on-main reality. Stale `blocked_reason` field is residual but harmless (terminal state).
3. **T085 investigated, verdict salvage.** 93 cycles decompose to ~18 substantive commits (P1-P4 + early P5 + rebase) building a real ~2.5k-LOC TUI cockpit; ~74 commits were `P5: resubmit verified watch cockpit fixtures` thrash on a tail-end `.gitignore`/runtime-state stability issue, NOT confused architecture. Code is intact.
4. **Six branches rebased onto local main, tests green:**
   - T085: 92 commits replayed clean, 1192 lib tests pass
   - T084: 10 commits replayed clean, 1203 lib tests pass (one flake on first run, clean on rerun)
   - T088: 4 commits replayed clean, 1185 lib tests pass
   - T093: 2 commits replayed clean, 1185 lib tests pass
   - T095: 3 commits replayed clean, 1181 lib tests pass
   - T096: 26 commits replayed clean, 1179 lib tests pass
   - All worktrees had projection drift (`tasks/active/T001-test-task/main.md` etc); stashed before rebase under `T0XX pre-rebase projection drift` labels.
5. **engine-health.md refreshed** (commit `b6b960e`):
   - Promoted ~20 obs/tasks from open/in-flight to ✅ in layer tables.
   - Rewrote one-sentence summary, priority ladder, next-picks for post-operator-trust + post-engine-monitor state.
   - Added 19 new "Recently shipped" rows for the overnight + 2026-05-07 PM batch.
   - Filed three new GAP entries inline (rate-limit, cascade-dedup, log-fd-drift, stop-foreground).

### What is on origin

`origin/main` is at `eef4cc4` (May 4) — yesterday's session never pushed. All 27 merges + today's commits are local-only. Worktrees rebased today were force-rebased locally; nothing pushed to origin.

### Six dangling auto-drive locks

`dispatch_locks` table has six rows for T084/T085/T086/T088/T093/T095 with `last_status='in_flight:pending_next'`, no `finished_at`, no `terminal_reason`, daemon_epoch from yesterday at `2026-05-07T17:28:17Z`. These should be auto-cleaned on daemon restart via the T040 epoch-shift + T050 typed-lifecycle stale-detection. T096's lock is already properly closed (`terminal_reason=silent_zombie`, `attempts=6`, `finished_at` set). No raw-SQL writes performed.

### Daemon log fd drift (gap)

The daemon was launched with `--log-file logs/agents-daemon.log` but its fd 1/2 pointed at `/tmp/daemon2.log`. The on-disk log file went silent at 21:52 last night while the actual engine-runner activity flowed to `/tmp`. Filed inline as `GAP-log-fd-drift` in engine-health Layer 1.

### `stores agents stop` doesn't work for foreground daemons (gap)

The verb errors with "agents daemon pid file missing" if the daemon was started without `--detach`. We had to SIGTERM the PID directly. Filed inline as `GAP-stop-foreground` in engine-health Layer 1.

## Pickup priorities for next session

1. **Decide on daemon restart.** If yes: `stores agents run --detach --log-file logs/agents-daemon.log` (use `--detach` so the pidfile exists and `stores agents stop` works). Then verify the daemon picked up the T076 private install path: `ls -la /proc/$(cat .stores/agents.pid)/exe` should point at `~/.local/share/stores/bin/stores`. Tail `logs/agents-daemon.log` to confirm engine-runner ticks land in the configured file (would also verify the GAP-log-fd-drift behavior).
2. **Resume the rebased rows** (after daemon back): `stores tasks resume T084 T085 T088 T093 T095 T096` (one verb per id). This drives blocked → ready and lets the daemon pick them up cleanly without the runner_crash loop. Watch one or two ticks via `stores watch` or daemon log to confirm clean dispatch and that the watchdog typed-closes the six dangling locks.
3. **Drop the pre-rebase stashes** in each worktree once the rebased state is verified clean: `git stash list` per worktree, `git stash drop` if it's clearly the projection-drift stash from today.
4. **File three gap observations** (autonomous via `stores intake add`):
   - Rate-limit-aware retry (T2): T041 doesn't distinguish flake from rate-limit-exhausted; T029/L071 was supposed to but exit_code=3 is leaking through. T085's 93-cycle thrash is the smoking gun.
   - Cascade-dedup subscriber (T1): dedup `deploy-blocked: merge conflict` obs on `(task_id, conflict_signature)`; L465–L479 are 15 dupes from today's cascade.
   - Daemon log-fd / stop-foreground (T1): `--log-file` doesn't redirect fd 1/2 without `--detach`; `stores agents stop` requires `--detach` pidfile.
5. **Observation backlog hygiene sweep.** ~354 open obs; many are stale/dupes (the L465+ cascade rows are obvious starts). Classify/close before drafting more contracts. Let the gatekeeper Router (T053/L142) earn its keep here.
6. **Ratify priority + file-overlap scheduler** as the next contract. The cascade is the cost evidence; the doc's "Highest-leverage next picks" lists this as #1.

## Things to avoid next session

- Do NOT raw-SQL the substrate DB (reads OK).
- Do NOT push the rebased branches to `origin` without a deliberate decision — origin/main is May 4 stale, so push semantics need thinking through (likely want to push origin/main to current local main first, then push the feature branches).
- Do NOT delete the `tasks/active|paused|planning/*/main.md` projection files (durable per CLAUDE.md doctrine).
- Do NOT spawn agents or auto-drive cycles until daemon restart is verified clean.

## Open items at handoff

- **`.claude/skills/handover-engine-controller/SKILL.md`** and **`.claude/skills/pi-architect/SKILL.md`** still show modified in `git status` from yesterday's wind-down. Not touched this session; needs review whether to commit or revert.
- Six worktrees' stashes pending evaluation/drop (one per: T084/T085/T088/T093/T095/T096, all labeled `<TID> pre-rebase projection drift`).

## Follow-ups

- engine-health.md inline GAPs (rate-limit / cascade-dedup / log-fd-drift / stop-foreground) need to become real observations once daemon restarts.
- Decide whether to push origin/main forward at some point — yesterday's 27 merges + today's cleanup are all local. Either push, or accept that origin is the "shared GitHub state" and main is the "true local state."
