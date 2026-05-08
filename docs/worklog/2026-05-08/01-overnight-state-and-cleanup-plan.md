# Overnight State And Cleanup Plan

**Date:** 2026-05-08
**Type:** note

## Summary

The 3-agent (substrate-agent / pi-architect / reviewer-runner) session ran ~13:00 → ~23:46 yesterday and shipped 27 tasks. Daemon kept running solo since ~00:01 today on the new T076 private install path (`~/.local/share/stores/bin/stores`) with the T079 engine-runner monitor live. Engine-health.md and the 04-handover note are now significantly stale.

The session ended in a **single cascade**, not seven independent failures: T086 (the meta-substrate external_reviews reconciler) was manually merged into main at `8bd21b6` per Pi msg_ccfb6b59 because it fixes its own deploy gate, but the planned `tasks close-out-of-band T086` step never happened. With main moved underneath them, six other in-flight branches (T084/T085/T088/T093/T095/T096) hit merge conflicts, the runner kept crashing on stale base, and T041's retry-on-failure rescheduler ran the cycle counter into the dozens (T085 is at 93 cycles).

The runner_crash exit_code=3 pattern is **also being amplified by codex/pi rate limits** Blake hit during the session — the runner subprocess exits non-zero on rate-limit, T041 retries with backoff, the rate limit is still active, repeat. T029/L071's "rate limit → blocked with structured reason" appears to not be catching this class of exit; that's a gap to file once cleanup is done.

## Details

### What shipped (27 tasks since 2026-05-07)

| group | tasks |
|---|---|
| Operator trust / runtime | T066 (self-reexec), T067 (auto-drive idle), T075 (candidate-binary), T076 (private install path), T078 (plaintext+0600), T080 (cross-project daemon), T081 (tier-A bypass) |
| Engine telemetry / read surface | T069 (binary version), T070 (agent_runs), T071 (`stores metrics`), T072 (runs VIEW + atomic backlink), T074 (auth show --identity) |
| Engine substrate (this is the big one) | **T079 (engine-runner monitor — LIVE)**, **T083 (substrate-native external review lane)**, T065 (auto-investigator subscriber), T077 (architecture_reviews typed store), T082 (derivation-token persistence) |
| Schema / observation hygiene | T064 (watch noise filter), T068 (required_when OR/IN), T073 (list-typed array input), T087 (DOT snapshot flake), T089 (watchdog terminal-state filter), T090 (scaffold stderr), T091 (topology line width), T092 (observations next-id), T094 (T086 elapsed retry), T097 (verdict parser tolerance) |

26 reached `schema_migrated`; T081 sits at `accepted` (cargo-install/migrate not yet run, or row drift).

### What's stuck

| task | obs | cycles | blocked_reason | actual cause |
|---|---|---|---|---|
| T084 | L082 | 1 | `runner_crash` exit=3 | rebase debt + likely rate limit |
| T085 | L192 | **93** | `runner_crash` exit=3 | thrash; ~50 `T085 P5: resubmit verified watch cockpit fixtures` commits in branch log |
| T086 | L193 | 50 | `runner_crash` exit=3 | **already merged at 8bd21b6**; row needs close-out-of-band |
| T088 | L003 | 35 | `runner_crash` exit=3 | rebase debt + rate limit |
| T093 | L083 | 55 | `runner_crash` exit=3 | rebase debt + rate limit |
| T095 | L054 | 36 | `runner_crash` exit=3 | rebase debt + rate limit |
| T096 | L069 | 27 | `silent_zombie_pid_dead` | different mode — drive PID died before recording itself; ~30 `T096 P1: reconfirm deploy-blocked resume guidance` commits |

L465–L479 are 15 auto-filed `deploy-blocked: task TXXX merge conflict on branch …` observations from the daemon — confirmation the cascade root is stale base, not task logic.

### Engine state at 10:00

- Daemon PID 391252, exec from `~/.local/share/stores/bin/stores` (T076 confirmed live).
- Engine-runner monitor ticking: `tasks:0 intake:0 obs:372 actionable=0 held=372 dispatched=0`. All 372 obs held with `needs_human` (gatekeeper Router doing its job; 354 `open`).
- **Daemon log fd drift:** configured `--log-file=logs/agents-daemon.log` but fd 1/2 → `/tmp/daemon2.log`. On-disk log stopped at 21:52 last night; current activity is in /tmp. Minor but worth a fix later.

## Mode

**Manual cleanup, no dogfooding** until the cascade is clear and the substrate row state matches reality. Daemon stopped (SIGTERM 391252 at 2026-05-08T~10:05 — process gone). Auto-drive will not pick up rebased branches mid-cleanup. We turn the daemon back on only after the engine-health one-sentence summary is honest again.

## Plan

### Phase 1 — substrate hygiene (manual, immediate)

1. **Close T086 out-of-band** with merge SHA `8bd21b6` and a reference to Pi msg_ccfb6b59. The branch is already on main; the substrate row is the only thing stuck. This also stops new "deploy-blocked merge conflict" observations being filed against T086 once the daemon comes back.

### Phase 2 — investigate T085 before doing anything mechanical

T085 has 93 cycles, ~50 `T085 P5: resubmit verified watch cockpit fixtures` commits, and a watch-cockpit topic that may have been thrashing on a real design ambiguity, not just rate-limit/rebase debt. **Read first, decide second.** Concretely:

1. `cd` into `stores-T085-auto-promoted-l192` worktree.
2. Read the contract (`stores tasks show T085`), the most recent few cycles in `cycles[]`, and the actual diff vs main.
3. Look at the "P5 resubmit" commits — are they all the same content (= thrash on a transient flake) or is each one different (= the executor genuinely couldn't converge)?
4. Outcome A: code is fundamentally OK, just hit conflict + rate limit → rebase + resume in Phase 3 like the others.
5. Outcome B: code is in a confused half-state, or the contract scope drifted → `tasks abandon T085 --reason "<why>"` and re-file a fresh observation with a tighter contract; let the next session re-promote.
6. Outcome C: ambiguous → halt, surface to Blake, decide together.

Don't rebase or push T085 until this read is done.

### Phase 3 — unwedge the rest of the cascade (manual)

For T084 / T088 / T093 / T095 (the mechanical four):
1. `cd` into the worktree.
2. `git fetch && git rebase main`. Resolve conflicts. (Most should be projection-only or simple Cargo.lock collisions; if any are substantive code conflicts, halt and surface.)
3. `cargo build --release && cargo test --lib` to confirm no breakage.
4. `git push --force-with-lease`.
5. **Do NOT `tasks resume` yet** — daemon is off. Resume verbs queue them for the daemon's next tick. We resume in Phase 6.

For T086: skip — Phase 1 closes it. Branch can be deleted after the row is closed.

For T096: `silent_zombie_pid_dead` is a different failure mode (drive PID died before recording itself). After rebase, check `dispatch_locks` for stale T096 rows; may need explicit clearing via the typed-lifecycle close (T050's verbs), NOT raw SQL. Manual judgment call.

### Phase 4 — doc refresh (manual)

1. **Update `docs/engine-health.md`:**
   - Move T076/T077/T078/T079/T080/T082/T083 from "in flight" to ✅ in their layer tables.
   - Add the 27-task batch to the Recently-shipped table.
   - Re-rank "highest-leverage next picks" — actionability monitor + external review lane both shipped, so priority+file-overlap scheduler is now the top live next-pick.
   - Update the one-sentence summary to reflect post-operator-trust + post-engine-monitor state.
2. **No action on the wind-down handover note** — it's an artifact of yesterday, not a doc to keep current.

### Phase 5 — file gaps surfaced this session (after daemon is back; autonomous obs filings)

- **Rate-limit-aware retry**: T041's retry-on-failure doesn't distinguish transient flake from rate-limit-exhausted. T029/L071 was supposed to type the rate-limit case as `blocked:rate_limit`, but exit_code=3 is leaking through as generic `runner_crash`. The 93-cycle T085 thrash is the smoking gun. Tier-hint T2.
- **Cascade-on-meta-merge**: when a substrate-changing task merges into main, all open branches inherit conflict debt. The daemon currently files one observation per merge-conflict tick (L465–L479 = 15 dupes). Subscriber should dedup on `(task_id, conflict_signature)`. Tier-hint T1.
- **Daemon log fd drift**: `--log-file` flag doesn't actually redirect fd 1/2 when the daemon is run without `--detach`. The configured log file (`logs/agents-daemon.log`) was last written 21:52 last night; current activity went to `/tmp/daemon2.log` instead. Tier-hint T1.
- **`stores agents stop` requires `--detach`**: the verb errors with "pid file missing" if the daemon was started in foreground mode. Either the verb should also handle non-detached daemons (e.g. find-by-process-name fallback), or the foreground-run path should still write a pidfile so stop works uniformly. Tier-hint T1.

### Phase 6 — restart daemon + resume rebased rows

1. `stores agents run --detach --log-file logs/agents-daemon.log` (use `--detach` this time so the pid file exists and `stores agents stop` works).
2. Verify daemon is on the T076 private install path (`ls -la /proc/$(cat .stores/agents.pid)/exe`).
3. `stores tasks resume T084` (and same for T088/T093/T095 — and T085 if Phase 2 chose Outcome A).
4. Watch one or two ticks via `stores watch` or `tail -f logs/agents-daemon.log` to confirm the daemon picks them up cleanly without the runner_crash loop.

### Phase 7 — handover note

Write `02-…-handover.md` capturing: T086 closed-out-of-band, cascade resolved, engine-health refreshed, daemon back on the rails. Next-session priorities live in the refreshed engine-health, not the handover.

## Follow-ups

- After Phase 2 read of T085, may surface a sub-decision (abandon vs salvage) that needs Blake's call.
- Phase 4's re-rank of "highest-leverage next picks" may surface that priority+file-overlap scheduling should be the next ratifiable contract — worth Pi review when next session opens.

