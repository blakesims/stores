# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-06 (fifth pass) — Big batch day. Eleven tasks shipped (T029, T035, T036, T037, T038, T039, T040, T041, T044, T045, T046, T047) plus L130 direct fix. Engine cleared the T1-drive gap (L109/T039), watchdog scope (L107/T040), retry-on-failure (L039/T041), out-of-band close-out (L092/T044), accept-merge exit-code routing (L131/T046), and plan-persistence+watchdog actor_note (L120/T047). Investigator subagent pull-shape shipped (T038, accepted then closed_out_of_band via merge-commit recovery). Gatekeeper design ratified (T045 docs-only). 10.06 dogfood reached full migration smoke ship (their L026/T017, 809 LOC). 14 substrate wrinkles surfaced; ~half fixed today, the rest filed (L132–L145).

## The picture in one sentence

**The engine ratifies, drives, deploys, recovers, and watchdogs cleanly end-to-end across all three tier shapes — the remaining drag is metadata-flow ergonomics (resume-from-deploy_blocked semantics, T1 execution-shape consolidation, framework-DDL drift on bootstrap, dispatch_lock primitive sprawl) and the still-unstarted gatekeeper/intake layer.** Layers 1–4 are now substantially solid; the next geodesic is Layer 7 (typed-primitive coherence — Router shipped, Check + intake_items pending) and the auto-investigator queue-drain primitive (Layer 8) that turns the substrate from human-pulls to engine-pulls.

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
*Drive-loop reliability now substantially solid. Watchdog catches silent zombies + ignores pre-deploy stale pids; retry-on-failure reschedules transient flakes. Remaining drag: dispatch_lock primitive sprawl across L087/L116/L122/L141 (typed lifecycle pending — see L134).*

| obs | state | what hurts |
|---|---|---|
| L045 | ✅ T024 | accept-merge tolerates already-merged + stale workspace |
| L055 | ✅ T026 | daemon seeds starting-line-marker locks at startup (no retroactive fires) |
| L067 | ✅ T032 | auto-drive into worktree now finds `.stores/` (substrate-side symlink in auto-scaffold) |
| L062 | ✅ T030 | watchdog catches post-spawn silent zombies via tasks-table scan + grace window (`tests/drive_silent_zombie_e2e.rs`) |
| L071 | ✅ T029 | drive aborts on runner exit=1 (rate limit) → substrate marked blocked with structured reason |
| L107 | ✅ T040 | watchdog daemon-epoch gate + lock-recency check (false positives from pre-deploy stale drive_pids fixed) |
| L039 | ✅ T041 | daemon retry-on-failure rescheduler ships (transient flakes auto-retry with backoff) |
| L113 | ✅ T035 | resume clears stale auto-drive PID before re-firing the cycle |
| L131 | ✅ T046 | accept-merge subscriber exit-code routing → accepted→deploy_blocked on shim failure (verified live in 10.06's binary) |
| L120 | ✅ T047 | planner plan persistence + watchdog actor_note column; claude_code runner SAP fallback uses role-aware `pick_best_sap_candidate` |
| L087 | ⚪ — | auto-promote silent-fails on rapid sequential ratifies (folded into L134's typed-lifecycle umbrella) |
| L116 | ⚪ T2 | seeder race during agents.yaml hot-reload (overlaps L141 dispatch primitive) |
| L122 | ⚪ T2 | dispatch_lock orphans on subagent kill |
| L141 | ⚪ T2 | auto-drive subscriber marks `last_status='ok'` on dispatch (not on completion); silent-zombie root cause cousin to L087 — cleanest fix is L134's umbrella |
| L068 | ⚪ — | cross-project daemon SIGTERM (other-repo `pkill 'stores agents run'` kills mine) |
| GAP | — | per-project daemon PID file + `stores agents status / stop` verbs |

### Layer 2 — State / idempotency

| obs | state | what hurts |
|---|---|---|
| L063 | ✅ T025 | auto-promote uses `linked_observations` (not surfacing-task `task_id`) for idempotency |
| L038 | ✅ T033 | `depends_on` pre-flight guard (T1, shipped after L109/T039 unblocked T1 drives) |
| L108 | ⚪ T2 | `fire_on_entry_follow_ons` fires only at add(); retroactive tier_hint update from T2→T1 doesn't re-trigger skip-plan |
| L130 | ✅ direct | resume routes blocked T2/T3 with plan=null to planning instead of ready (avoids "Phase 1 of 0" deadlock); fixed direct on main during T038 push |
| L132 | ⚪ T1 | schema validator should refuse unguarded transition shadowing a guarded one (silent override risk) |
| L133 | ⚪ T2 | normalize T1 execution shape: synthesize a contract-derived single phase during skip-plan so plan IS the phase rather than a parallel surface (consolidates L109/L117/L123/L126/L130 — biggest engine-architecture lever) |
| L011 | ⚪ T2 | rows don't record `stores` binary version |
| L053 | ⚪ — | tier-A actor check bypassable via `--invoker human` from `$CLAUDECODE`-detected processes |

### Layer 3 — Drive economics

| obs | state | what hurts |
|---|---|---|
| L066 | ✅ T027 | tier-structural drive cycle: T1 skips planner+plan_reviewer; T2 plans constrained to 1 phase |
| L093 | ✅ T039 | planner brief tier-aware (T1: skip-plan; T2: single-phase enforced; T3: multi-phase) — saves wasted planner output on misshaped tiers |
| L109 | ✅ T039 | T1 drive end-to-end pull (next-action returns the executor for ready+contract-only T1 rows) |
| L030 | ❌ — | superseded by L066/T027 (T1 path); remaining tier-aware-brief scope deferred until pulled by use |
| L028 | ⚪ T2 | drive-spawned agents lack verified `/observe` skill access |
| GAP | — | tier-aware code-reviewer brief modulation (L030's deferred remainder) |

### Layer 4 — Deploy ceremony / release
*Substantially solid post-batch. Daemon merges-cargo-installs-schema-migrates cleanly when it runs (T046's exit-code routing made silent-zombies impossible). Remaining drag: framework-DDL drift on bootstrap (L144), resume-from-deploy_blocked semantics (L145), and the merge-conflict recovery dance still requires manual close-out-of-band (T044's verb).*

| obs | state | what hurts |
|---|---|---|
| L060 | ✅ T031 | schema-migrate subscriber spawns `stores migrate --apply` subprocess against on-disk binary (Fix Shape B) |
| L080 | ✅ T032 | auto-scaffold writes `tasks.branch` from worktree HEAD |
| L020 | ✅ T036 | render canonicalizes state dirs + symlink-escape guard |
| L131 | ✅ T046 | accept-merge subscriber routes shim failure → accepted→deploy_blocked (cross-listed from Layer 1) |
| L136 | ✅ T044 | `tasks close-out-of-band` recovery-terminal verb (closes accepted/deploy_blocked rows whose work shipped via manual merge) |
| L138 | ✅ T045 | gatekeeper design doc + risk/cluster taxonomy ratified (docs-only; impl seeds in L142/L143) |
| L061 | ⚪ T2 | no pre-promotion acceptance precheck; tasks ship before discovering already-met |
| L069 | ⚪ — | `compute_resume` rejects `deploy_blocked` rows (related: L145, schema/handler drift on resume verb) |
| L070 | ⚪ — | accept-merge conflict path drops cargo-install + schema-migrate side effects |
| L144 | ⚪ T2 | `stores migrate` doesn't detect framework-DDL drift (SUBSTRATE_DDL columns added in newer binary aren't applied to existing DBs); 10.06 hit this on actor_note bootstrap, manual ALTER required |
| L145 | ⚪ T2 | resume handler hardcodes 'blocked' source; schema permits deploy_blocked→ready via resume but handler rejects pre-validator. Also semantic ambiguity: should resume from deploy_blocked re-cycle or just retry-deploy? |
| L149 | ⚪ T2 | **daemon's auto-drive spawn breaks silently after `cargo install` replaces `/home/blake/.cargo/bin/stores`** — current_exe()-based execvp loads new binary, but the daemon's in-memory image stays out of sync; spawn argv subtly mismatches new binary's CLI parsing → drive subprocess dies <5s with empty log. Workaround: restart daemon after each cargo-install. Surfaced today during pipe-fill (T048/T049 both died until daemon restart). Compounds with L011 (binary-version recording) — daemon should detect inode-replacement of own exe and self-restart. |
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
*Investigator pull-shape shipped (T038/L043) — orchestrator now has a substrate primitive for "spawn a fresh sandboxed dive on this question". Auto-resolve subscriber shipped (T037/L049). The auto-investigator-fires-on-unblock-of-open-obs primitive is still the #1 strategic weakness; ~30 open obs sit without ratified contracts and the engine can't draft them itself yet.*

| obs | state | what hurts |
|---|---|---|
| L043 | ✅ T038 | investigator subagent pull-shape (sandboxed dive, returns structured report) |
| L049 | ✅ T037 | auto-resolve subscriber on cargo_installed→schema_migrated transition |
| L092 | ✅ T044 | `tasks close-out-of-band` verb (cross-listed from Layer 4) |
| L093 | ✅ T039 | planner brief tier-aware (cross-listed from Layer 3) |
| L137 | 🟠 T1 | auto-resolve needs startup-sweep / backfill (15 stale schema_migrated→ready obs pairs) — contract drafted, awaiting ratify |
| L142 | 🟠 T3 | implement intake_items store + gatekeeper subscriber (P1 of T045 design); pi reviewing scope today |
| L143 | 🟠 T3 | add risk_class + approval_policy fields to observations schema (P2 of T045 design) |
| L134 | ⚪ T2/T3 | formalize dispatch_locks as typed lifecycle buffer (compounds with L039/L087/L107/L116/L122/L141; the umbrella that retires several Layer 1 rows) |
| L135 | ⚪ T2/T3 | promote Check primitive (cross-cutting validator surface for `submit-*` flows) |
| L072 | ⚪ — | code-reviewer REPLAN gate dead-ends as `blocked` instead of routing back to planning |
| L023 | ⚪ T2 | observations missing `next-id` verb + JSON envelope inconsistency |
| L002 | ⚪ T2 | no admin rollback verb |
| L003 | ⚪ T2 | observations list output unscannable for >2 rows |
| L006 | ⚪ T2 | observations runner asymmetry (no drive cycle for obs) |
| L021 | ⚪ T1 | render template doesn't pull `wrap_log` into Completion section |
| L034 | ⚪ T1 | wrap misattributes main-ahead commits as 'rides on this branch' |
| GAP | — | **auto-investigator subscriber** — fires investigator on `open → needs_investigation` automatically; partial machinery exists (L043 investigator agent) but no subscriber wires it. Engine still can't drain its own input queue. |

## Highest-leverage next picks

Layers 1–4 substantially solid post-batch. Re-ranked picks (the geodesic now points at typed-primitive coherence + queue-drain):

1. **L137 (auto-resolve startup-sweep)** — T1, ~80-150 LOC. Quick win: unsticks 15 historical task→obs pairs and codifies the backfill pattern. Contract drafted, awaiting U1.
2. **L141 (auto-drive lock-on-dispatch)** — T2. Silent-zombie root cause cousin of L087; cleanest fix is via L134's umbrella. Could ship standalone if L134 design needs more time.
3. **L132 (schema validator unguarded-shadow refusal)** — T1. Defensive substrate hygiene; cheap.
4. **L144 (framework-DDL drift)** — T2. 10.06's bootstrap blocker today; once shipped, fresh DB clones won't need manual ALTER. Compounds with L011 (binary-version recording).
5. **L133 (T1 execution shape normalization)** — T2 / biggest lever. Synthesize contract-derived single phase during skip-plan so plan IS the phase rather than a parallel surface; consolidates L109/L117/L123/L126/L130 into one structural fix.
6. **L134 (dispatch_locks typed lifecycle)** — T2/T3 umbrella. Retires L087/L116/L122/L141 (and unblocks L107's edge cases). Should be designed jointly with L135 (Check primitive) since both are cross-cutting validators.
7. **L142 / L143 (gatekeeper P1/P2)** — T3. Pi reviewing scope; intake_items store + risk_class/approval_policy fields. Strategic ceiling for the "filings drift becomes architecture drift" loop.
8. **Auto-investigator subscriber (GAP)** — strategic ceiling cousin to L142. Flips substrate from human-pulls to engine-pulls. T038's investigator agent is the primitive; needs the subscriber wiring.
9. **L145 (resume-from-deploy_blocked semantics)** — T2 design question; needs pi/architecture input on whether resume should re-cycle or retry-deploy.
10. **L108 (on-entry retroactive trigger)** — T2. Mid-flight tier_hint changes still don't re-fire skip-plan; rare but real.

The geodesic shifted again: the T1 + retry + watchdog drag is gone; the next bottleneck is **engine-shape primitives** (L133/L134/L135) and **input-queue auto-drain** (gatekeeper L142 + auto-investigator GAP).


## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
| 2026-05-06 | T047 | L120 | planner plan persistence + watchdog actor_note column; claude_code runner SAP fallback uses role-aware `pick_best_sap_candidate` (10.06's #2 blocker cleared) |
| 2026-05-06 | T046 | L131 | accept-merge subscriber routes shim failure → accepted→deploy_blocked with structured reason (10.06's #1 blocker cleared; verified live in their binary) |
| 2026-05-06 | T045 | L138 | gatekeeper design + risk/cluster taxonomy doc-only ratification (impl seeds: L142/L143) |
| 2026-05-06 | T044 | L136 | `tasks close-out-of-band` recovery-terminal verb with merge-commit provenance |
| 2026-05-06 | T041 | L039 | daemon retry-on-failure rescheduler with backoff |
| 2026-05-06 | T040 | L107 | watchdog daemon-epoch gate + lock-recency check (no more pre-deploy stale-pid false positives) |
| 2026-05-06 | T039 | L093/L109 | T1 tier-aware planner brief + T1-drive end-to-end pull (executor fires from contract-only ready rows) |
| 2026-05-06 | T038 | L043 | investigator subagent pull-shape (sandboxed dive; closed_out_of_band via merge-commit recovery) |
| 2026-05-06 | T037 | L049 | auto-resolve subscriber on cargo_installed→schema_migrated (live transition only — backfill pending in L137) |
| 2026-05-06 | T036 | L020 | render canonicalizes state dirs + symlink-escape guard |
| 2026-05-06 | T035 | L113 | resume clears stale auto-drive PID before re-firing |
| 2026-05-06 | T029 | L071 | drive runner-exit (rate limit / non-zero) → substrate marked blocked with structured reason |
| 2026-05-06 | direct | L130 | resume routes blocked T2/T3 with plan=null to planning instead of ready (avoids "Phase 1 of 0" deadlock) |
| 2026-05-05 | T030 | L062 | daemon detects post-spawn silent zombies (tasks-table scan + grace window; structured `drive_failed:silent_zombie_pid_dead` / `:pid_never_recorded` reasons) |
| 2026-05-05 | T024 | L045 | accept-merge tolerates already-merged + stale workspace |
| 2026-05-05 | T025 | L063 | auto-promote idempotency uses `linked_observations` |
| 2026-05-05 | T026 | L055 | daemon seeds starting-line-marker locks at startup |
| 2026-05-05 | T027 | L066 | tier-structural drive cycle (T1 skip / T2 single-phase / T3 unchanged) |
| 2026-05-05 | T031 | L060 | schema-migrate subscriber spawns `stores migrate --apply` subprocess against on-disk binary (Fix Shape B) — the freshly cargo-installed binary's bundled schemas drive the diff, not the daemon's stale in-process bundle. New `resolve_stores_bin()` helper + `tests/schema_migrate_post_accept_e2e.rs` (262 LOC, success + failure paths). Full drive cycle through 2 phases; accept-merge + cargo-install + schema-migrate ceremony all `ok` |
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
