# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-06 (sixth pass) — Big batch day plus architecture turn. Shipped: T029/T035/T036/T037/T038/T039/T040/T041/T044/T045/T046/T047/T048 plus L130 direct fix. Engine cleared the T1-drive gap, retry/watchdog/accept-merge/plan-persistence failures, close-out-of-band, investigator pull-shape, gatekeeper design, and auto-resolve backfill. Remaining pressure is now concentrated in three coherent clusters: **dispatch attempts as a typed lifecycle** (L134/L141/L149), **T1 execution-shape normalization** (L133), and **gatekeeper implementation seeds** (L142/L143, held for amendment).

## The picture in one sentence

**The engine now mostly works end-to-end; the danger has moved from "can it drive?" to "can it see and type its own control-plane state?"** Runtime/deploy are much healthier, but status is still confusing because dispatch attempts, T1 contract-plans, and intake/gatekeeper risk metadata are not yet first-class enough. The next geodesic is typed observability: dispatch_locks → typed lifecycle (L134), T1 contract → canonical plan row shape (L133), raw filings → intake/gatekeeper Router (L142/L143).

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
| L134 | 🟢 T050 | typed dispatch_locks lifecycle umbrella: postcondition_id+args, daemon_epoch, terminal_reason, next_retry_at; currently blocked/recovering but right abstraction |
| L141 | ⚪ T2 | auto-drive marks `last_status='ok'` on dispatch, not completion; symptom folded into L134 |
| L149 | ⚪ T2 | daemon/on-disk binary drift after cargo-install kills fresh auto-drive subprocesses; restart workaround; folds into L011 + L134 observability |
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
| L137 | ✅ T048 | auto-resolve startup-sweep / backfill; 15 stale schema_migrated→ready obs pairs cleaned by startup sweep |
| L142 | 🟠 T3 | implement intake_items store + gatekeeper subscriber (P1 of T045 design); held for amendment — preserve direct mature-observation path, defer fast-track execution / architecture_reviews store |
| L143 | 🟠 T3 | add risk_class + approval_policy fields to observations schema; held for amendment — enum must match canonical `{low, normal, architecture, security, authority}` |
| L134 | 🟢 T050 | formalize dispatch_locks as typed lifecycle buffer (compounds with L039/L087/L107/L116/L122/L141/L149; umbrella fix now drafted/amended) |
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

1. **L134 / T050 (typed dispatch_locks lifecycle)** — highest leverage runtime cleanup. One umbrella for L087/L116/L122/L141/L149: typed terminal reasons, daemon epoch, postcondition_id+args, next_retry_at.
2. **L133 (T1 execution shape normalization)** — biggest state-shape lever. Path B chosen: synthesize a canonical one-phase plan during skip-plan with provenance; retire plan-null branches.
3. **L144 / T051 (framework-DDL drift)** — bootstrap/release hygiene. Existing DBs must learn new SUBSTRATE_DDL columns without manual ALTER.
4. **L142 / L143 (gatekeeper P1/P2)** — strategic ceiling, but hold for amendment. L143 enum mismatch + L142 over-scope identified; ratify after narrowing.
5. **L132 (schema validator unguarded-shadow refusal)** — cheap defensive schema hygiene.
6. **L145 (resume-from-deploy_blocked semantics)** — design question: does resume re-cycle execution or retry deploy ceremony?
7. **L135 (Check primitive)** — unifies deterministic pre/post-condition gates; pairs naturally with L134 postconditions and future fast-track audit.
8. **Auto-investigator subscriber (GAP)** — turns L043 investigator into queue-drain automation.
9. **L108 (on-entry retroactive trigger)** — rare but real metadata-flow gap.

Current picture: runtime is usable; next work is making the control plane typed enough that `stores watch`/humans can distinguish quiet, stuck, retriable, escalated, and architecturally risky without reading 100 rows.


## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
| 2026-05-06 | T048 | L137 | auto-resolve startup-sweep/backfill closes historical schema_migrated→ready observation pairs |
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
