# Handover 2026 05 08 Cleanup

**Date:** 2026-05-08
**Type:** note

## Summary

Manual no-dogfood cleanup of the overnight cascade, completed. Daemon stopped; all six stuck branches (T084/T085/T088/T093/T095/T096) sequentially merged into main with re-rebase between each, tests green throughout (1230 lib tests pass on final main); seven substrate rows closed-out-of-band (T032 historical leftover + T086 meta-substrate + the six from today); engine-health.md refreshed for 27 shipped tasks; origin/main in sync (`13e6c78..0c864f1` pushed). **Zero blocked tasks remain.** Daemon stays OFF pending Blake's explicit call to restart.

The cascade root was a single missed step from yesterday's session: T086 (the meta-substrate external_reviews reconciler) was hand-merged at `8bd21b6` per Pi msg_ccfb6b59 (chicken-and-egg: T086 fixes its own deploy gate), but the planned `tasks close-out-of-band T086` never ran. With main moved underneath, six in-flight branches inherited rebase debt; runner crashed on stale base; T041's retry-on-failure rescheduler ran cycle counts to 27/35/50/55/93. Codex/pi rate limits amplified the loop — `runner_crash exit_code=3` is leaking through what T029/L071 was supposed to type as `blocked:rate_limit`.

## Details

### What was done this session

1. **Daemon stopped.** `kill -TERM 391252` (was running foreground without `--detach` so `stores agents stop` didn't apply). Process gone.
2. **T086 closed-out-of-band** with `--commit 8bd21b6 --invoker human`. Status: blocked → closed_out_of_band.
3. **T085 investigated, verdict salvage.** 93 cycles decompose to ~18 substantive commits (P1-P4 + early P5 + rebase) building a real ~2.5k-LOC TUI cockpit; ~74 commits were `P5: resubmit verified watch cockpit fixtures` thrash on a tail-end `.gitignore`/runtime-state stability issue, NOT confused architecture.
4. **engine-health.md refreshed** (commit `b6b960e`): promoted ~20 obs/tasks to ✅, rewrote summary + priority ladder + next-picks, added 19 new "Recently shipped" rows, filed four new GAPs inline (rate-limit, cascade-dedup, log-fd-drift, stop-foreground).
5. **Six branches sequentially merged into main**, with re-rebase between each merge to surface conflicts cheaply:
   - T093 → main (`2ba503b`); 1185 lib tests
   - T095 → main (`7732c97`); 1187 lib tests
   - T088 → main (`b5cc0c5`); 1193 lib tests
   - T084 → main (`02687d1`); 1217 lib tests (one rerun for a flake)
   - T096 → main (`89adb0d`); 1217 lib tests — **finding: T096 was a duplicate task. All 26 commits were empty ("HEAD already contains the alignment, this empty commit provides a valid review SHA"). The actual L069 fix shipped via T061/L145 long ago. Merge is a no-op merge commit.**
   - T085 → main (`0c864f1`); 1230 lib tests
   - All re-rebases clean, zero conflicts.
6. **Seven rows closed-out-of-band** to match merged reality (matches T086 pattern):
   - T086 (`8bd21b6`), T093 (`2ba503b`), T095 (`7732c97`), T088 (`b5cc0c5`), T084 (`02687d1`), T096 (`89adb0d`), T085 (`0c864f1`)
   - Plus T032 (`bf2d388`) — historical leftover from May 5 that was never properly closed (drive_failed:silent_zombie_pid_dead from before close-out-of-band verb existed).
7. **Origin synced.** `git push origin main`: `eef4cc4..13e6c78` (after engine-health refresh) and then `13e6c78..0c864f1` (after the six merges). Origin is now caught up to local main.

### Six dangling auto-drive locks

`dispatch_locks` table has six rows for T084/T085/T086/T088/T093/T095 with `last_status='in_flight:pending_next'`, no `finished_at`, no `terminal_reason`, daemon_epoch from yesterday at `2026-05-07T17:28:17Z`. These should be auto-cleaned on daemon restart via T040 epoch-shift + T050 typed-lifecycle stale-detection. T096's lock is already properly closed (`terminal_reason=silent_zombie`, `attempts=6`, `finished_at` set). No raw-SQL writes performed.

### Daemon log fd drift (gap)

The daemon was launched with `--log-file logs/agents-daemon.log` but its fd 1/2 pointed at `/tmp/daemon2.log`. The on-disk log file went silent at 21:52 last night while the actual engine-runner activity flowed to `/tmp`. Filed inline as `GAP-log-fd-drift` in engine-health Layer 1.

### `stores agents stop` doesn't work for foreground daemons (gap)

The verb errors with "agents daemon pid file missing" if the daemon was started without `--detach`. We had to SIGTERM the PID directly. Filed inline as `GAP-stop-foreground` in engine-health Layer 1.

## Pickup priorities for next session

1. **Decide on daemon restart.** If yes: `stores agents run --detach --log-file logs/agents-daemon.log` (use `--detach` so the pidfile exists and `stores agents stop` works). Then verify the daemon picked up the T076 private install path: `ls -la /proc/$(cat .stores/agents.pid)/exe` should point at `~/.local/share/stores/bin/stores`. Tail `logs/agents-daemon.log` to confirm engine-runner ticks land in the configured file (would also verify the GAP-log-fd-drift behavior). Watch one or two ticks via `stores watch` to confirm the watchdog typed-closes the six dangling locks (epoch shift triggers T040+T050 stale-detection).
2. **No `tasks resume` needed** — all six rows are now `closed_out_of_band`, work merged into main. The substrate is consistent.
3. **Drop the pre-rebase stashes** in each worktree once the rebased state is verified clean: `git stash list` per worktree, `git stash drop` if it's clearly the projection-drift stash from today.
4. **File four gap observations** (autonomous via `stores intake add`):
   - Rate-limit-aware retry (T2): T041 doesn't distinguish flake from rate-limit-exhausted; T029/L071 was supposed to but exit_code=3 is leaking through. T085's 93-cycle thrash is the smoking gun.
   - Cascade-dedup subscriber (T1): dedup `deploy-blocked: merge conflict` obs on `(task_id, conflict_signature)`; L465–L479 are 15 dupes from today's cascade.
   - Daemon log-fd drift (T1): `--log-file` doesn't redirect fd 1/2 without `--detach`.
   - `stores agents stop` requires --detach (T1): foreground daemons can't be stopped via the verb; needs find-by-process-name fallback or pidfile in foreground mode too.
5. **Observation backlog hygiene sweep.** ~354 open obs; many are stale/dupes (the L465+ cascade rows are obvious starts). Classify/close before drafting more contracts. Let the gatekeeper Router (T053/L142) earn its keep here.
6. **Ratify priority + file-overlap scheduler** as the next contract. The cascade is the cost evidence; the doc's "Highest-leverage next picks" lists this as #1.
7. **Investigate T084's release-test flake.** During post-rebase verification, `cargo test --lib --release` had 1 fail on first run, clean on rerun (multiple times). Same pattern recurred on T095. Single-threaded passed cleanly. Worth filing as a flake observation; could be test-ordering or race in one of the new tests added by T084/T095.
8. **Investigate T096's empty-commit pattern.** The substrate produced 26 empty commits because the executor and reviewer disagreed about whether work was needed when HEAD already had the fix. Reviewer protocol gap: reviewer should accept "HEAD already satisfies DONE_WHEN — no commit needed" as a valid PASS state without requiring a SHA.

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
