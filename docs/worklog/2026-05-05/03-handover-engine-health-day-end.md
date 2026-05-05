# Handover Engine Health Day End

**Date:** 2026-05-05
**Type:** note

## Summary

Heavy dogfood-throughput day. **8 obs closed, 3 tasks shipped (T030/T031/T032), 8 new obs filed via real use.** Engine-health summary moved from "ratify+drive but worktrees broken" → "ratify+drive+deploy+watchdog all reliable; remaining brittleness is metadata-flow surfaces (tier_hint, watchdog scope, dispatch_lock idempotency)."

The day's geodesic shifted mid-flight: T029 push surfaced **L109 — drive doesn't handle T1 cycle end-to-end** (T027 schema-ratified the T1 path, but no real T1 task had ever been pulled; the moment one was, the gap surfaced). Classic realistic-pull signal per `philosophy.md`.

**Key engine-health one-liners (chronological):**
- Morning: "engine ratifies and drives, but can't reliably restart, deploy, or refill its own input queue" — Layer 1+4 brittlest, Layer 8 strategic ceiling
- After T032: "engine ratifies, drives, and now provisions worktrees that work, but still can't catch silent-zombies or deploy schema across daemon-restart"
- After T031: "engine ratifies, drives, deploys schema cleanly across daemon-restart; silent-zombie watchdog is the next anchor"
- After T030 + T029 push: "engine catches silent-zombies; remaining brittleness is metadata-flow surfaces — T1 drive path (L109), watchdog scope (L107), retroactive on-entry triggers (L108)"

## Details

### Today's ✅ ships (chronological)

| time | task | obs | what changed |
|---|---|---|---|
| (earlier) | T024-T028 | L045/L055/L063/L066/L075 | accept-merge tolerance; daemon starting-line; auto-promote idempotency; tier-structural cycle (T027); ratatui TUI |
| ~09:30 | **T032** | **L032/L067/L080** | auto-scaffold symlinks `.stores/` artifacts + writes `tasks.branch` from worktree HEAD. Hand-cranked (the bug it fixed was blocking its own drive). Closes the worktree-discovery hole that was killing every auto-driven task |
| ~11:15 | **T031** | **L060** | `builtin:schema-migrate` spawns `stores migrate --apply` subprocess against on-disk binary (Fix Shape B). Full drive cycle (5 stages, 22.5 min, ~$5-7). Daemon's stale in-process schema bundle no longer blocks new schema additions on accept |
| ~12:00 | **T030** | **L062** | silent-zombie watchdog. `scan_zombie_tasks()` in `auto_drive.rs` checks tasks-table for in-cycle rows with dead/NULL drive_pid past 10s grace. New `actor_note` column on `transition_history` (DDL). Real-binary e2e test. **Hand-recovered through merge-conflict + manual ALTER TABLE.** Watchdog NOW LIVE in daemon (PID 2143066) |

### Today's ⚪ filed obs (in order)

- **L082** — observations source-shape leak (10.06 fields at top-level pollute universal schema) — high
- **L083** — temporal field hygiene (type inconsistency, derivable-required, missing parallels) — normal
- **L084** — severity vs priority conflation + reproduces/confidence missing — normal
- **L085** — first-class duplicate_of / merged_into (Aggregation prerequisite) — normal
- **L086** — capability + capability_ids denormalization — low
- **L087** — auto-promote silent-fails ~0% on rapid sequential ratifies (same dispatch-lock-shape as L062 on a different code path) — high
- **L092** — no out-of-band task close-out path (T032 surfaced this on its own ship) — normal
- **L093** — planner brief lacks tier_hint awareness (T2 planners over-decompose; rejected by submit-plan; demonstrated live on T031's first attempt) — normal
- **L107** — T030 watchdog reaps pre-existing dead drive_pids on first post-deploy sweep + drive-startup race window (false positives) — high
- **L108** — `fire_on_entry_follow_ons` fires only at add(); retroactive tier_hint update from T2→T1 doesn't re-trigger skip-plan — normal
- **L109** — **drive's next-action returns null for T1+ready+no-plan; T1 cycle never end-to-end pulled before today** — normal but important

### Workflow that worked (for next agent)

The pattern that produced the day's wins:

1. **Read engine-health.md first.** It's the curated map of "where the engine bleeds." The "Highest-leverage next picks" section is a deliberately-ranked geodesic.

2. **Pick the geodesic, not the local optimum.** Today: T031 (deploy) before T030 (watchdog) was correct, even though T030 was the bigger Layer 1 pain — because T030's eventual ceremony added a schema column that needed T031's fix already shipped. Order matters.

3. **Pre-empt known friction.** When firing T030 I pre-emptively re-tier'd to T3 (because T031 had just demonstrated T2's planner-rejection cycle, costing $1.50 of planner output). Dodging known bugs upstream is cheap.

4. **Always file when something surprises you.** L087 was filed mid-stream when auto-promote silent-failed for 2-of-3 rapid ratifications; that observation later compounded with L107 to suggest a dispatch_lock-primitive gap. The filing pattern:
   ```
   stores observations add --invoker ai_autonomous \
     --summary "<one-line>" --source dev --priority {high|normal} \
     --captured-at "$(date -Iseconds)" --captured-week "w$(date +%V)-d$(date +%u)" \
     --task-id <if any> --body-from-file <(cat <<'EOF' ... EOF)
   ```
   Filing is autonomous (`ai_autonomous`); ratifying and accepting are U-moments needing the token.

5. **Hand-crank when the engine breaks. In a worktree.** When auto-drive was dead from L067, I hand-cranked T032 (worktree edit + cargo install + daemon restart). When T030's accept-merge hit a conflict, I hand-cranked the merge resolution + cargo install + manual ALTER TABLE + status advance via SQL. The user's standing rule: hand-crank in a worktree, not on main, and file an obs about every workaround used (L092 came out of T032's hand-ship; the substrate-DDL migration gap should be filed tomorrow).

6. **Don't grind on bugs that surfaced from realistic-pull.** When T029 hit L107/L108/L109 in rapid succession, I stopped pushing. The bugs ARE the engine-health information; pushing harder would have been poor leverage.

### Errors to expect (with recovery patterns)

- **`error: no transition from 'planning' via verb 'accept'`** — task is at planning, you tried to accept directly. Need to walk submit-* envelopes, OR (preferred for hand-cranked work) leave it at planning and file L092 as the gap.
- **`error: unrecognized subcommand 'tasks'`** — cwd doesn't have `.stores/`. Either `cd` to a dir that does (main repo or a properly-symlinked worktree), or set up the symlinks: `ln -s /path/to/main/.stores/{db.sqlite,manifest.yaml,agents.yaml,config.yaml,runs} <worktree>/.stores/`.
- **`Error: submit-plan: tier T2 requires phases.length == 1, got 2`** — the L093 firing. Either re-tier the row to T3 and submit existing plan, or hand-merge phases. Re-tier is faster (saves a planner re-run).
- **`merge conflict on branch ... last attempt: merge failed`** — accept-merge hit a conflict (canonical traps: docs/CLAUDE.md, philosophy.md, engine-health.md). Recovery: `cd` main repo, `git merge --no-ff feat/T0xx --no-commit`, resolve, commit, then cargo install + manual ALTER TABLE if schema changed + SQL advance status to `schema_migrated` + daemon restart.
- **`drive_failed:silent_zombie_pid_dead` / `drive_failed:pid_never_recorded`** — L107 false-positive zone. To unblock a row that's actually NOT a zombie: `DELETE FROM dispatch_locks WHERE display_id='Txxx' AND agent_name='auto-drive'; UPDATE tasks SET status='planning', drive_pid=NULL, drive_started_at=NULL, blocked_reason=NULL WHERE display_id='Txxx';`
- **Auto-promote silent-failing** (L087) — ratifying an obs doesn't reliably produce a task row. Always check after ratify: `sqlite3 .stores/db.sqlite "SELECT display_id FROM tasks WHERE linked_observations LIKE '%Lxxx%'"`. If empty, manually create via `stores tasks add --invoker ai_with_human --approve-token <T> ...`.
- **`error: next-action returned no agent for status 'planning'`** — likely L109 (T1 drive gap). Until L109 fixed, T1 tasks can't be driven via the standard CLI flow. Workaround: re-tier to T2 + hand-author 1-phase plan + submit-plan it.

### State at handover

- **Daemon:** PID 2143066, on T030 code (watchdog active). Subprocess schema-migrate is in main but the daemon image already has T031's code from the cargo install at T031's accept ceremony.
- **Binary on disk** (`~/.cargo/bin/stores`): T030 code (rebuilt during T030 hand-recovery cargo install).
- **main HEAD:** `1aa9a2b` (engine-health fourth-pass commit).
- **Active worktrees:**
  - `stores-T029-auto-promoted-l071` — T1, planning, ready for re-attempt after L109 fixed
  - `stores-T030-auto-promoted-l062` — shipped, can be cleaned up
  - `stores-T031-auto-promoted-l060` — shipped, can be cleaned up
  - `stores-T032-auto-promoted-l032` — work shipped via merge but task row stuck at planning (L092)
  - `stores-T033-auto-promoted-l038` — T1, planning, blocked behind L109
- **Open dispatch_locks:** none (verified earlier).
- **Unfiled gap:** substrate-internal DDL migrations (T030's `actor_note` column required manual ALTER TABLE; `stores migrate --apply` only handles per-store schemas, not `SUBSTRATE_DDL`). File this tomorrow.

## Follow-ups

### Immediate next session priorities (geodesic)

1. **Fix L109 (T1-drive gap)** — single biggest unblock. Until this lands, T029 + T033 can't drive cleanly, and any future T1 work hits the same wall. Likely T2: src/handlers/drive.rs or next_action.rs needs the no-plan-executor-from-contract path. Add an end-to-end test that drives a T1 task end-to-end (which would have caught this before today).
2. **Fix L107 (watchdog scope)** — refine `scan_zombie_tasks` predicate. Suggested: daemon-startup-epoch column on dispatch_locks (watchdog ignores prior-epoch locks), OR parent-pid-liveness check (watchdog skips rows where the lock-claiming process is still alive). T2.
3. **Fix L108 (on-entry retroactive)** — composes with L109. T2.
4. **Ship L093 (planner brief tier-aware)** — cheapest engine-economy improvement. T1 template change. After L109 lands.
5. **File the substrate-DDL migration gap** — `actor_note` was a canary; future substrate-internal schema changes will hit the same bug.
6. **Drive T029 + T033** once L109 fixed — both are cheap T1 drives once the path works.
7. **L087 + auto-investigator GAP** — strategic. Both touch the dispatch_lock primitive; design together.

### Worktree cleanup (low priority)

- `stores-T030/T031` worktrees can be removed after their feat/* branches are deleted.
- T032's worktree should stay until L092 is fixed (the substrate row is the audit-trail's stuck point).

### Token / spend hygiene

- Today's burn: ~$87 (~$15 in this session). 96% cache hit ratio held throughout.
- Heavy hitters: T028 (TUI, $33), T027 (tier cycle, $22), T030 ($10ish), T031 ($5-7).
- L057 (per-row token metadata) + L058 (read surface for fleet metrics) are both T2; once they ship the engine-health doc gets a "$ per task" column derivable from the data already in `.stores/runs/*.jsonl`.

### What NOT to do (lessons from today)

- Don't drive multiple T2/T3 tasks concurrently — risks the L071 rate-limit shape that's still unfixed (T029 unshipped) and burns budget on retries.
- Don't update tier_hint mid-flight expecting the framework to adapt — it won't (L108).
- Don't edit engine-health.md while a drive is active on a branch that also touches it — guaranteed merge conflict (L070 reproduces every time).
- Don't reset stale state by clearing drive_pid alone — the watchdog re-fires on the dispatch_lock JOIN. You need to also `DELETE FROM dispatch_locks WHERE agent_name='auto-drive'`.
