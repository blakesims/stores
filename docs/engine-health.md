# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-06 (eighth pass) — Big batch day plus architecture turn, Pi-runner/load-balancing smoke, and pipe-fill cleanup. Shipped: T029/T035/T036/T037/T038/T039/T040/T041/T044/T045/T046/T047/T048/T050/T052/T054/T055/T056/T057/T058/T059/T060 plus L130 direct fix. Engine cleared the T1-drive gap, retry/watchdog/accept-merge/plan-persistence failures, close-out-of-band, investigator pull-shape, gatekeeper design, auto-resolve backfill, per-role runner config, minimal Pi-runner structured-output smoke, typed dispatch lifecycle, T1 canonical plan synthesis, observation risk metadata, and watch default rebucketing. Remaining pressure is concentrated in **gatekeeper implementation** (L142/T053 in flight), **stale/retirement/admin cleanup** (L124/L002/L145), and **deeper observability/queue-drain** (L057/L058/L151/L135).

## The picture in one sentence

**The engine now works end-to-end often enough that the main risk is operator trust and architectural coherence, not basic propulsion.** Runtime/deploy are much healthier; Pi can drive/write structured `final_output`; dispatch attempts and T1 contract-plans are typed enough for the current batch; and `stores watch` no longer has to show terminal exhaust as in-flight by default. The next geodesic is finishing the gatekeeper Router seam (L142/T053), then cleaning stale/retirement paths and deeper observability so the operator can tell current action, blocked reason, recovered terminal, and architectural risk at a glance.

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
*Drive-loop reliability now substantially solid. Watchdog catches silent zombies + ignores pre-deploy stale pids; retry-on-failure reschedules transient flakes. L134/T050 typed dispatch lifecycle shipped, folding L141/T049-style implicit lock-state concerns into postconditions + typed terminal reasons. Remaining drag is narrower: stale daemon binary detection, orphan cleanup, and per-project daemon control.*

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
| L134 | ✅ T050 | typed dispatch_locks lifecycle umbrella: postcondition_id+args, daemon_epoch, terminal_reason, next_retry_at; shipped via close-out-of-band after codex PASS/rebase reconciliation |
| L141 | ✅ T049/T050 | auto-drive `ok`-on-dispatch concern translated into L134 typed postcondition semantics; open-lock invariant superseded by `drive_pid_recorded_or_terminal` + watchdog on task pid |
| L149 | ⚪ T2 | daemon/on-disk binary drift after cargo-install kills fresh auto-drive subprocesses; restart workaround; folds into L011 + L134 observability |
| L150 | ⚪ T2 | halt/deploy-blocked subscriber mislabels blocked drive failures as deploy_blocked merge-conflict observations; another symptom of untyped terminal state / event postconditions, folded into L134/L135 |
| L068 | ⚪ — | cross-project daemon SIGTERM (other-repo `pkill 'stores agents run'` kills mine) |
| GAP | — | per-project daemon PID file + `stores agents status / stop` verbs |

### Layer 2 — State / idempotency

| obs | state | what hurts |
|---|---|---|
| L063 | ✅ T025 | auto-promote uses `linked_observations` (not surfacing-task `task_id`) for idempotency |
| L038 | ✅ T033 | `depends_on` pre-flight guard (T1, shipped after L109/T039 unblocked T1 drives) |
| L108 | ⚪ T2 | `fire_on_entry_follow_ons` fires only at add(); retroactive tier_hint update from T2→T1 doesn't re-trigger skip-plan |
| L130 | ✅ direct | resume routes blocked T2/T3 with plan=null to planning instead of ready (avoids "Phase 1 of 0" deadlock); fixed direct on main during T038 push |
| L132 | ✅ T057 | schema validator refuses unguarded transition shadowing a guarded one (silent override risk) |
| L133 | ✅ T054 | T1 execution shape normalized: synthesize a contract-derived single phase during skip-plan so plan is canonical rather than null/special-cased |
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
| L143 | ✅ T052 | observation risk metadata + approval_policy shipped; generic update cannot bypass direction-aware override-policy gate |
| L061 | ⚪ T2 | no pre-promotion acceptance precheck; tasks ship before discovering already-met |
| L069 | ⚪ — | `compute_resume` rejects `deploy_blocked` rows (related: L145, schema/handler drift on resume verb) |
| L070 | ⚪ — | accept-merge conflict path drops cargo-install + schema-migrate side effects |
| L144 | ✅ T051 | framework-DDL drift detection/audit shipped; existing DBs learn newer SUBSTRATE_DDL columns without manual ALTER |
| L145 | ⚪ T2 | resume handler hardcodes 'blocked' source; schema permits deploy_blocked→ready via resume but handler rejects pre-validator. Also semantic ambiguity: should resume from deploy_blocked re-cycle or just retry-deploy? |
| L149 | ⚪ T2 | **daemon's auto-drive spawn breaks silently after `cargo install` replaces `/home/blake/.cargo/bin/stores`** — current_exe()-based execvp loads new binary, but the daemon's in-memory image stays out of sync; spawn argv subtly mismatches new binary's CLI parsing → drive subprocess dies <5s with empty log. Workaround: restart daemon after each cargo-install. Surfaced today during pipe-fill (T048/T049 both died until daemon restart). Compounds with L011 (binary-version recording) — daemon should detect inode-replacement of own exe and self-restart. |
| L150 | ⚪ T2 | halt/deploy-blocked subscriber files merge-conflict-shaped observations for rows that are merely `blocked` by drive failure (e.g. T034 silent-zombie / Pi-smoke failures). Needs typed event/terminal reason before templating operator-facing halt observations. |
| GAP | — | acceptance-time precheck for "task touches files with uncommitted main-side changes → accept-merge will fail" |

### Layer 5 — Discovery / observability

| obs | state | what hurts |
|---|---|---|
| L032 | ✅ T032 | auto-scaffold symlinks `.stores/` artifacts into provisioned worktrees (closes L067 transitively) |
| L165 | ✅ T059 | `stores watch` rebuckets task rows for actionable defaults: terminal/recovered/rejected exhaust no longer belongs in in-flight by default; blocked reason parsing improved |
| L057 | ⚪ T2 | no per-agent-invocation metadata on rows (model / tokens / duration / transcript-ref) — usage analytics gap; data exists in `.stores/runs/*.jsonl` but not aggregated |
| L161/T056 | ✅ T056 | minimal Pi-runner smoke target: T1 task driven via Pi produced structured `final_output`, committed one marker file, passed review/wrap, accepted, and merged. This proves Pi runner basics but not full T034 acceptance bookkeeping. |
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
| L142 | 🟢 T053 | implement intake_items store + gatekeeper subscriber (P1 of T045 design); in flight, guarded to Router seam only |
| L143 | ✅ T052 | risk_class + approval_policy fields to observations schema; canonical enums + direction-aware override-policy gate |
| L134 | ✅ T050 | formalize dispatch_locks as typed lifecycle buffer (compounds with L039/L087/L107/L116/L122/L141/L149) |
| L135 | ⚪ T2/T3 | promote Check primitive (cross-cutting validator surface for `submit-*` flows) |
| L072 | ⚪ — | code-reviewer REPLAN gate dead-ends as `blocked` instead of routing back to planning |
| L023 | ⚪ T2 | observations missing `next-id` verb + JSON envelope inconsistency |
| L002 | ⚪ T2 | no admin rollback verb |
| L003 | ⚪ T2 | observations list output unscannable for >2 rows |
| L006 | ⚪ T2 | observations runner asymmetry (no drive cycle for obs) |
| L021 | ✅ T058 | render template pulls `wrap_log` into Completion section |
| L034 | ⚪ T1 | wrap misattributes main-ahead commits as 'rides on this branch' |
| GAP | — | **auto-investigator subscriber** — fires investigator on `open → needs_investigation` automatically; partial machinery exists (L043 investigator agent) but no subscriber wires it. Engine still can't drain its own input queue. |

## Highest-leverage next picks

Layers 1–4 are substantially solid after the batch. Re-ranked picks (the geodesic now points at completing the gatekeeper seam, then cleanup/observability):

1. **L142 / T053 (gatekeeper Router seam)** — current strategic ceiling. Keep P1 narrow: intake_items + Router classifications; preserve direct observations path; no fast-track execution, architecture_reviews store, cluster registry, or watch observability yet.
2. **L145 (resume-from-deploy_blocked semantics)** — design question: does resume re-cycle execution or retry deploy ceremony? This remains a visible recovery-path sharp edge.
3. **L124 + L002 (retirement/admin cleanup)** — watch is better after L165, but stale/rejected/duplicate/deploy-blocked rows still need a real terminal/retirement/admin path.
4. **L135 (Check primitive)** — unifies deterministic pre/post-condition gates; pairs naturally with L134 postconditions and future fast-track audit.
5. **Auto-investigator subscriber (GAP/L151)** — turns L043 investigator into queue-drain automation.
6. **L149 / L011 (daemon binary/version visibility)** — stale-exe restarts remain operational friction.
7. **L057/L058/L059 (run/edge observability)** — aggregate transcript refs, per-edge throughput, and invocation metadata.
8. **L108 (on-entry retroactive trigger)** — rare but real metadata-flow gap.

Current picture: propulsion is healthy enough to run a full pipe. The operator trust surface improved with L165/T059, but not all stale-row lifecycle problems are solved. Next work should finish L142/T053, then clean recovery/retirement semantics so watch output and task state feel boring rather than mysterious.


## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
| 2026-05-06 | T060 | L169 | tier-aware executor/code-reviewer briefs skip phase decomposition for T1 |
| 2026-05-06 | T059 | L165 | `stores watch` rebuckets rows for actionable defaults; terminal/recovered/rejected exhaust no longer appears as in-flight by default; blocked reason parsing improved |
| 2026-05-06 | T058 | L021 | task render includes wrap_log in Completion section |
| 2026-05-06 | T057 | L132 | schema validator refuses unguarded transition shadowing a guarded one |
| 2026-05-06 | T054 | L133 | T1 canonical-plan synthesis removes plan-null special casing |
| 2026-05-06 | T052 | L143 | observation risk_class + approval_policy shipped; generic update locked out of approval_policy bypass |
| 2026-05-06 | T050 | L134 | typed dispatch_locks lifecycle shipped with postconditions, terminal reasons, next_retry_at, and framework_migrations marker |
| 2026-05-06 | T056 | L161 | minimal Pi-runner smoke target: T1 docs-only marker task driven via Pi, structured `final_output`, accepted + merged (`1b1e93b`) |
| 2026-05-06 | T055 | — | per-role runner/model config for `stores tasks drive`: config-driven runner selection, Pi runner in default build, `--claude-code-model`, auto-drive respects runner config; closed out-of-band after direct main merge (`bff3c34`) |
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
