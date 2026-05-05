# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-05 — after the first sustained dogfood-throughput session post-T022 (auto-drive). 4 engine fixes shipped (T024/T025/T026/T027); 5 new high-priority bugs filed via real use (L067/L068/L071/L072 + the L063 found pre-T027).

## The picture in one sentence

**The engine can ratify and drive, but it can't yet (1) reliably restart, (2) reliably deploy, or (3) reliably refill its own input queue.** Layer 1 (runtime) and Layer 4 (deploy) are the brittlest surfaces; Layer 8's auto-investigator gap is the strategic ceiling on dogfood velocity.

## Status legend

| icon | meaning |
|---|---|
| ✅ | shipped (task accepted + deployed) |
| 🟢 | in flight (drive running) |
| 🟡 | queued (cap-blocked) |
| 🟠 | contract ready, ratifiable |
| ⚪ | open (no contract drafted) |
| ❌ | wont_fix |
| GAP | not filed yet |

## Layer-by-layer state

### Layer 1 — Runtime / dispatch reliability
*Today's #1 pain. Every restart is a risk; every drive subprocess is a potential silent zombie.*

| obs | state | what hurts |
|---|---|---|
| L045 | ✅ T024 | accept-merge tolerates already-merged + stale workspace |
| L055 | ✅ T026 | daemon seeds starting-line-marker locks at startup (no retroactive fires) |
| L062 | ✅ T030 | watchdog catches post-spawn silent zombies via tasks-table scan + grace window (`tests/drive_silent_zombie_e2e.rs`) |
| L071 | ⚪ T2 | drive aborts gracefully on runner exit=1 (rate limit) but doesn't notify substrate; row stuck at executing |
| L039 | ⚪ T2 | daemon retry-on-failure unimplemented; transient flakes strand rows |
| L067 | ⚪ — | auto-drive spawns from worktree without `.stores/`; subcommand discovery fails |
| L068 | ⚪ — | cross-project daemon SIGTERM (other-repo `pkill 'stores agents run'` kills mine) |
| GAP | — | per-project daemon PID file + `stores agents status / stop` verbs |

### Layer 2 — State / idempotency

| obs | state | what hurts |
|---|---|---|
| L063 | ✅ T025 | auto-promote uses `linked_observations` (not surfacing-task `task_id`) for idempotency |
| L038 | 🟠 T1 | `depends_on` field exists but unenforced (no chain auto-firing) |
| L011 | ⚪ T2 | rows don't record `stores` binary version |
| L053 | ⚪ — | tier-A actor check bypassable via `--invoker human` from `$CLAUDECODE`-detected processes |

### Layer 3 — Drive economics

| obs | state | what hurts |
|---|---|---|
| L066 | ✅ T027 | tier-structural drive cycle: T1 skips planner+plan_reviewer; T2 plans constrained to 1 phase |
| L030 | ❌ — | superseded by L066/T027 (T1 path); remaining tier-aware-brief scope deferred until pulled by use |
| L028 | ⚪ T2 | drive-spawned agents lack verified `/observe` skill access |
| GAP | — | tier-aware code-reviewer brief modulation (L030's deferred remainder) |

### Layer 4 — Deploy ceremony / release
*Currently has T023 stuck `deploy_blocked`; T027 hand-recovered today via SQL. Docs/CLAUDE.md/philosophy.md merge conflicts are the canonical trap.*

| obs | state | what hurts |
|---|---|---|
| L060 | ⚪ T2 | post-accept schema-migrate runs from OLD daemon binary; new schema silently no-ops |
| L061 | ⚪ T2 | no pre-promotion acceptance precheck; tasks ship before discovering already-met |
| L020 | ⚪ T1 | render leaves empty dirs across state transitions |
| L069 | ⚪ — | `compute_resume` rejects `deploy_blocked` rows; recovery requires SQL hand-touch |
| L070 | ⚪ — | accept-merge conflict path drops cargo-install + schema-migrate side effects |
| L064 / L065 / L073 / L074 | ⚪ — | escalation symptoms of T023/T027 stuck merges (not separate bugs) |
| GAP | — | acceptance-time precheck for "task touches files with uncommitted main-side changes → accept-merge will fail" |

### Layer 5 — Discovery / observability

| obs | state | what hurts |
|---|---|---|
| L032 | ⚪ T2 | worktree lacks `.stores/` (parent of L067; symlink workaround in use) |
| L054 | ⚪ — | no structured-read verbs for task review (orchestrator falls back to grep) |
| L057 | ⚪ T2 | no per-agent-invocation metadata on rows (model / tokens / duration / transcript-ref) |
| L058 | ⚪ T2 | no read surface for per-edge throughput / fleet metrics |
| L059 | ⚪ T1 | `.stores/runs/<task>/<role>.json` transcripts have no index, no row→transcript link |
| L012 | ⚪ T3 | no inspector for agent context (full graph view: aggregate, post-run, edit) |

### Layer 6 — Auth / security

| obs | state | what hurts |
|---|---|---|
| L013 | ⚪ T1 | `auth init` defaults to `~/.config/sops/age/keys.txt` (entanglement with SOPS) |
| L014 | ⚪ T2 | `auth init` UX gaps (opaque binary-format error; 7-line shell ritual) |
| L015 | ⚪ T1 | `auth show` missing `--identity` flag (asymmetric with `init`) |
| L044 | ⚪ T1 | L015 symlink workaround broke sops globally |
| L053 | ⚪ — | tier-A actor check bypass (cross-listed from Layer 2) |

### Layer 7 — Schema / contract substrate

| obs | state | what hurts |
|---|---|---|
| L005 | ⚪ T1 | list-typed fields accept only single-string at update (no JSON-array input) |
| L035 | ⚪ T3 | no schema-enforced inter-agent context refs (typed agents) |
| L019 | ⚪ T3 | no DockerRunner / standardized agent sandboxing |

### Layer 8 — Orchestration / triage discipline
*The auto-investigator gap is the #2 strategic weakness. As of this snapshot, ~34 open obs sit forever without manual contract drafting — the substrate's input rate is bottlenecked at the human's drafting rate.*

| obs | state | what hurts |
|---|---|---|
| L043 | 🟠 T2 | orchestrator inline investigation (the L043 rule itself; awaits investigator subagent) |
| L072 | ⚪ — | code-reviewer REPLAN gate dead-ends as `blocked` instead of routing back to planning |
| L023 | ⚪ T2 | observations missing `next-id` verb + JSON envelope inconsistency |
| L049 | ⚪ T1 | no auto-resolve of linked obs when task hits `schema_migrated` |
| L002 | ⚪ T2 | no admin rollback verb |
| L003 | ⚪ T2 | observations list output unscannable for >2 rows |
| L006 | ⚪ T2 | observations runner asymmetry (no drive cycle for obs) |
| L021 | ⚪ T1 | render template doesn't pull `wrap_log` into Completion section |
| L034 | ⚪ T1 | wrap misattributes main-ahead commits as 'rides on this branch' |
| GAP | — | **NO `open → investigating` subscriber** — pipeline is one-sided; engine cannot drain its own queue |

## Highest-leverage next picks (after current batch lands)

1. **L071** — close the remaining silent-zombie failure mode: drive aborts gracefully on runner exit=1 (rate limit) but doesn't notify substrate. L062's watchdog (T030) now catches drives that crash silently; L071 covers the cooperative-abort path.
2. **L060** — schema-migrate from new binary. Unblocks deploy ceremony for any future task that adds schema. Also opens the door to safer daemon restarts.
3. **L038** — `depends_on` enforcement. Already 🟠 ready. Lets us declare task chains without manual sequencing.
4. **Auto-investigator subscriber (GAP)** — the single biggest strategic move. Flips the substrate from human-pulls to engine-pulls on contract drafting. File this, then ratify.

## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
| 2026-05-05 | T030 | L062 | daemon detects post-spawn silent zombies (tasks-table scan + grace window; structured `drive_failed:silent_zombie_pid_dead` / `:pid_never_recorded` reasons) |
| 2026-05-05 | T024 | L045 | accept-merge tolerates already-merged + stale workspace |
| 2026-05-05 | T025 | L063 | auto-promote idempotency uses `linked_observations` |
| 2026-05-05 | T026 | L055 | daemon seeds starting-line-marker locks at startup |
| 2026-05-05 | T027 | L066 | tier-structural drive cycle (T1 skip / T2 single-phase / T3 unchanged) |
| 2026-05-05 | T028 | L075 | `stores watch` upgraded to ratatui TUI (section-grouped rows, sort/filter/search, daemon liveness, side-car spawn keys s/S/g/o); legacy ANSI POC behind `--legacy` |
| 2026-05-04 | T022 | L048 | auto-drive subscriber |
| 2026-05-03 | T021 | L050 | topology snapshot includes T019 states |
| 2026-05-03 | T020 | L046/L047 | auto-promote + auto-scaffold builtins |

## How to update this doc

This is a hand-curated snapshot, not a generated report. Refresh it at inflection points:

- **A batch of fixes lands.** Move shipped obs from open to ✅; add to "Recently shipped"; promote any newly-vacant Layer to "less brittle" wording in the one-sentence summary.
- **A new high-priority obs surfaces.** Add a row to the relevant Layer.
- **A bug class is named that wasn't previously visible.** Add a new Layer or a new GAP line.
- **The "highest-leverage next picks" section drifts.** Re-rank based on current ratifiable contracts and the day's pain.

To regenerate the obs status snapshot, query the DB:
```
sqlite3 .stores/db.sqlite "SELECT display_id, status, json_extract(intent_contract,'$.tier_hint') as tier, COALESCE(task_id,'') as tid, summary FROM observations WHERE status NOT IN ('resolved','wont_fix') ORDER BY display_id;"
```

For the deeper reasoning behind any single shipped item, the worklog under `docs/worklog/<date>/` has the session detail. Promote insights here when they become long-standing.
