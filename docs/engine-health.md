# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-05 (second pass) — same-day refresh after the worktree-discovery hole closed. T032 shipped (L032/L067/L080 ✅). Four tasks ratified + scaffolded today (T029 L071, T030 L062, T031 L060, T033 L038); awaiting sequential drive. Two new self-demonstrations filed (L087 auto-promote silent-fail; L092 no out-of-band close-out).

## The picture in one sentence

**The engine ratifies, drives, and now provisions worktrees that work, but it still can't (1) reliably catch silent-zombie failures, (2) reliably deploy schema across daemon-restart, or (3) reliably refill its own input queue.** Layer 1's silent-zombie watchdog (L062, queued) and Layer 4's schema-migrate-from-new-binary (L060, queued) are the next two anchor fixes; once they ship, Layer 8's auto-investigator GAP becomes the dominant strategic ceiling. Auto-promote's own ~0% reliability today (L087) is the most surprising signal — the substrate's input pipeline is bottlenecked by the same silent-zombie shape its runtime fix is trying to close.

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
| L067 | ✅ T032 | auto-drive into worktree now finds `.stores/` (substrate-side symlink in auto-scaffold) |
| L062 | 🟡 T030 | watchdog can't catch post-spawn failures (ratified; awaiting drive) |
| L071 | 🟡 T029 | drive aborts on runner exit=1 (rate limit) but doesn't notify substrate (ratified; awaiting drive; downgraded T1) |
| L087 | ⚪ — | **auto-promote silent-fails ~0% success on rapid sequential ratifies** — same dispatch-lock-marks-ok-but-no-task pattern as L062, on a different code path; suggests L062's fix may need to widen scope to the dispatch_lock primitive |
| L039 | ⚪ T2 | daemon retry-on-failure unimplemented; transient flakes strand rows |
| L068 | ⚪ — | cross-project daemon SIGTERM (other-repo `pkill 'stores agents run'` kills mine) |
| GAP | — | per-project daemon PID file + `stores agents status / stop` verbs |

### Layer 2 — State / idempotency

| obs | state | what hurts |
|---|---|---|
| L063 | ✅ T025 | auto-promote uses `linked_observations` (not surfacing-task `task_id`) for idempotency |
| L038 | 🟡 T033 | `depends_on` pre-flight guard (ratified; awaiting drive; T1) |
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
| L060 | 🟡 T031 | post-accept schema-migrate runs from OLD daemon binary; new schema silently no-ops (ratified; awaiting drive) |
| L080 | ✅ T032 | auto-scaffold writes `tasks.branch` from worktree HEAD (closes 10.06's accept-merge punt) |
| L061 | ⚪ T2 | no pre-promotion acceptance precheck; tasks ship before discovering already-met |
| L020 | ⚪ T1 | render leaves empty dirs across state transitions |
| L069 | ⚪ — | `compute_resume` rejects `deploy_blocked` rows; recovery requires SQL hand-touch |
| L070 | ⚪ — | accept-merge conflict path drops cargo-install + schema-migrate side effects |
| L064 / L065 / L073 / L074 | ⚪ — | escalation symptoms of T023/T027 stuck merges (not separate bugs) |
| GAP | — | acceptance-time precheck for "task touches files with uncommitted main-side changes → accept-merge will fail" |

### Layer 5 — Discovery / observability

| obs | state | what hurts |
|---|---|---|
| L032 | ✅ T032 | auto-scaffold symlinks `.stores/` artifacts into provisioned worktrees (closes L067 transitively) |
| L057 | ⚪ T2 | no per-agent-invocation metadata on rows (model / tokens / duration / transcript-ref) — usage analytics gap; data exists in `.stores/runs/*.jsonl` but not aggregated |
| L054 | ⚪ — | no structured-read verbs for task review (orchestrator falls back to grep) |
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
| L092 | ⚪ T2 | no out-of-band task close-out path — hand-cranked work that's already merged + installed has no clean substrate verb to walk planning → accepted; T032 itself surfaced this on its own ship |
| GAP | — | **NO `open → investigating` subscriber** — pipeline is one-sided; engine cannot drain its own queue |

## Highest-leverage next picks (after current batch lands)

Four tasks are scaffolded and awaiting drive (T029/T030/T031/T033). Geodesic order:

1. **T031 (L060)** — schema-migrate from new binary. **Drive first.** Without this, T030's and T029's eventual accept ceremonies might silently no-op any schema additions they include. Layer 4 unblock.
2. **T030 (L062)** — silent-zombie watchdog. Top Layer 1 pain; demonstrated 4× today (T023, plus auto-promote silent-failing L062/L060/L038 ratifications). Drive after T031.
3. **T029 (L071)** — drive aborts on rate-limit notify substrate. Pairs with T030 to make Layer 1 substantially less brittle. Now T1 (downgraded from T2) — single function tweak in drive wrapper, ~half the spawn cost. Drive after T030.
4. **T033 (L038)** — `depends_on` pre-flight guard. T1, cheap. Drive any time; not blocking anything.
5. **Auto-investigator subscriber (GAP)** — strategic ceiling. Flips substrate from human-pulls to engine-pulls on contract drafting. **File this AFTER L087 investigation completes** — the auto-promote silent-fail (L087) is the same dispatch_lock-shape gap that an auto-investigator would hit on its own subscription, so investigating L087 + designing the auto-investigator are tightly coupled.

After this batch lands, the one-sentence summary changes again — "ratify, drive, restart, deploy, watchdog" all reliable; auto-investigator becomes the dominant remaining gap.

## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
| 2026-05-05 | T024 | L045 | accept-merge tolerates already-merged + stale workspace |
| 2026-05-05 | T025 | L063 | auto-promote idempotency uses `linked_observations` |
| 2026-05-05 | T026 | L055 | daemon seeds starting-line-marker locks at startup |
| 2026-05-05 | T027 | L066 | tier-structural drive cycle (T1 skip / T2 single-phase / T3 unchanged) |
| 2026-05-05 | T032 | L032 / L067 / L080 | auto-scaffold symlinks `.stores/` artifacts into provisioned worktrees + writes `tasks.branch` from worktree HEAD; closes the worktree-discovery hole that was killing every auto-driven task and the branch-writeback gap blocking 10.06's accept-merge. Hand-cranked (the bug it fixes was blocking its own drive); shipped via cargo install + daemon restart |
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
