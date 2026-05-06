# L116 + L117 fixes; second Pi-runner dogfood (T037 / L049)

**Date:** 2026-05-06
**Type:** note / engine-fix + dogfood

## Summary

Two interlocking substrate bugs surfaced while attempting a Pi-runner E2E on a freshly-ratified observation: (1) **L117** — `auto-promote` skipped the planning state's on-entry hooks, so T1 rows were stuck at `planning` with `next-action` returning no agent; (2) **L116** — the starting-line seeder claimed any matching transition_history row on every fresh `agents run --once`, so user verbs that fired between two daemon runs were silently swallowed as `skip-historical`. Both fixed via direct code edits per the new session doctrine; verified end-to-end (substrate-only verbs) by ratifying L049 → T037 promoted at `executing` (T1 cascade) with no SQL workarounds.

Also added a session doctrine to `CLAUDE.md` codifying: dogfood when the substrate works, escape to direct code edits when it doesn't, and **never raw-SQL the substrate DB**.

## Details

### Engine fixes

**L117 — `auto-promote` does not fire on-entry actions** (commit `6f869fb`)

`stores/tasks/schema.yaml`'s `on_state.planning` declares two tier-conditional actions:
- `{dispatch_agent: planner, when: "tier_hint != 'T1'"}`
- `{transition_to: ready, when: "tier_hint == 'T1'"}`

These fire via `submit::fire_on_entry_follow_ons`, which all submit handlers already call. `auto_promote::promote()` inserted the tasks row + synthetic create transition directly inside its own transaction without ever calling the helper, so for T1 rows the cascade never ran. Drive then errored with `next-action returned no agent for status 'planning'`.

Fix: after the synthetic create transition + observation back-link, call `fire_on_entry_follow_ons(&tx, &tasks_schema, &new_display_id, rowid, "planning")`. T1 rows now cascade `planning → ready → executing` inside the auto-promote transaction (`ready`'s on-entry is unconditional `transition_to: executing`); T2/T3 stay at `planning` because their on-entry is `dispatch_agent: planner` which doesn't move state. Two regression tests added.

**L116 — starting-line seeder claims new transitions as `skip-historical`** (commits `31a9c42` + `7703608`)

The seeder ran on every `run_daemon` startup, INSERT-OR-IGNORE'ing a starting-line lock for every transition_history row matching any subscription's `(store, from, to)`. With `agents run --once` repeatedly invoked between user verbs, the seeder claimed each new transition before the dispatcher could try_claim it. The dispatcher lost the `UNIQUE(store, row_id, agent_name)` race and the new transition silently dropped.

First attempt (`31a9c42`) bounded the seeder to `id <= MAX(id) at startup`, but the actual race is verbs firing **before** the daemon starts, so the bound included them. The right semantic (commit `7703608`) is **per-agent**: an agent that already has any `dispatch_lock` has had its starting-line drawn; the seeder must skip it entirely on subsequent runs. New agents (the original L055 case — adding a subscriber to a running daemon) still get seeded against the full historical table because they have no locks yet. Belt-and-suspenders id-bound retained for the rare race-during-startup window.

Tests:
- `seed_starting_line_skips_when_agent_already_has_locks` — pre-existing agent + new transition between runs → seeder skips, dispatcher wins.
- `seed_starting_line_seeds_new_agent_even_when_others_have_locks` — newcomer subscriber alongside an incumbent → newcomer seeded, incumbent untouched.

### Session doctrine added to `CLAUDE.md`

Codified after raw-SQL'ing `dispatch_locks` and `tasks` rows around L116 earlier in the session. The user's correction: "stores is the interface, not direct sql updates."

The doctrine: file friction → try substrate verbs (≤3 budget) → if interlocking bugs block the dogfood path, escape to direct code edits → **never raw-SQL writes** (reads via `SELECT` are fine) → name the friction in commit messages. Working rule for 2026-05-06; revisit when L116/L117 ship and the dogfood path is restored.

### Second Pi-runner dogfood: L049 → T037

L021 and L034 were poisoned by the earlier broken-seeder runs (their auto-promote slots permanently locked as `skip-historical`). L049 — `auto-resolve-observation: close linked obs when task hits schema_migrated` — was the third candidate and survived.

Substrate-verbs-only sequence (after the fixes):
1. `observations update L049 --contract-state ready --type work --approved-by blake --approved-at <now> --invoker ai_with_human --approve-token <T>`
2. `observations investigate L049 --invoker ai_autonomous`
3. `observations confirm L049 --invoker ai_with_human --approve-token <T>` → framework auto-ratified `confirmed → ready`
4. `agents run --once` → `seeded 0` (L116 fix), `auto-promote: L049 → T037 (planning)`, `auto-scaffold: workspace_path = …-T037-auto-promoted-l049`, T037 status=`executing` tier_hint=`T1` (L117 fix cascaded planning → ready → executing inside the auto-promote txn)
5. `tasks drive T037 --pi --max-iters 8`

Auto-drive subscriber temporarily disabled in `.stores/agents.yaml` to keep the dogfood Pi-only (auto-drive hardcodes `--claude-code` — filed as L119).

### Drive outcome — Pi runner hung silently on quota exhaustion (L121)

Pi executor stalled for 7+ minutes with zero stdout / stderr / progress signal. Root cause: the user's openai-codex quota was exhausted; the Pi SDK appears to retry/hang on the API call rather than returning a rate-limit error. The Node helper has no wall-clock budget; pi.rs uses a blocking `Command::output()` with no timeout; nothing catches the alive-but-stuck case. Filed as **L121** with three fix shapes (helper-side wall-clock, Rust-side timeout, substrate-side liveness watchdog).

Key takeaway: the Pi runner contract works on the happy path but is brittle under realistic failure modes. The blocking-wait pattern is fine for processes that exit; catastrophic for processes that hang. Worth fixing before treating Pi as a production runner option — but for today's dogfood we move on.

### Phase 4 — pivot to claude-code for the parallel batch

Pi quota is gone. Pivoting to claude-code per the plan ("process up to 3 at a time"). T037 manually resumed with `tasks drive T037 --claude-code`. Ratified L043 + L093 to fill out the batch.

Daemon dispatched 3 auto-drives in one poll iteration:
- T037 (L049, T1) — auto-resolve linked obs on schema_migrated
- T038 (L043, T2) — orchestrator inline investigation (investigator subagent)
- T039 (L093, T1) — planner brief tier-aware

**Substrate friction surfaced during the pivot** (filed as **L122**): manual `tasks drive` does not set `tasks.drive_pid`, so it's invisible to auto-drive's idempotency check and the parallel cap. Auto-drive happily race-spawned a SECOND drive on T037 (which I was already manually driving). Cheap fix shape sketched: drive.rs sets/clears drive_pid like auto-drive does.

3 claude-code drives now in flight. Monitoring.

## Follow-ups

- L119 (filed) — `auto-drive` should respect a config knob (e.g. `drive.runner: pi|claude-code|mock`) instead of hardcoding `--claude-code`. Blocks Pi-only dogfood without a temporary `agents.yaml` edit.
- Restore `auto-drive` subscription in `.stores/agents.yaml` after the T037 dogfood completes.
- L021 + L034 auto-promote slots are now permanently locked as `skip-historical`. There's no substrate verb to clear a stuck `dispatch_lock` — file an obs for a `dispatch-lock cleanup` verb (or a re-promote-via-task-removal path).
- T036 (the L020 stuck-at-planning task from earlier) is still cruft — my L117 fix doesn't fire on-entry hooks retroactively (that's L108 territory). No substrate verb to advance a stale `planning` row.
- L107 / L108 / L039 / L087 / L093 contracts now drafted (autonomously) and ready for ratification in phase 4 (process 3 at a time).
- Engine-health.md needs an update at session end with shipped bullets for L116, L117, doctrine.
