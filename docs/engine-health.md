# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-05 (fourth pass) — T030 (L062 silent-zombie watchdog) + T031 (L060 schema-migrate subprocess) both shipped today via full drive + accept ceremony (T030 hand-recovered through merge conflict + manual ALTER TABLE for substrate-internal DDL gap). Watchdog now LIVE in the daemon. T029/T033 push attempted; surfaced three new engine bugs (L107/L108/L109) — see below. Eight obs closed today: L032/L045/L055/L060/L062/L063/L066/L067/L080.

## The picture in one sentence

**The engine ratifies, drives, deploys schema cleanly across daemon-restart, provisions worktrees that work, AND now catches silent-zombie subagent failures — but it can't (1) reliably differentiate stale dead-pids from real zombies (L107 watchdog scope/epoch), (2) reliably handle T1 task drives (L109 — never end-to-end-pulled before today), (3) propagate retroactive metadata changes (L108 on-entry actions; L093 planner brief), or (4) refill its own input queue.** The T1-drive gap (L109) is genuinely new information — it took a *realistic-pull* (T029) to surface. Layer 1's autonomous machinery is now substantially better at runtime correctness; the remaining brittleness is in the metadata-flow surfaces around it.

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
| L062 | ✅ T030 | watchdog catches post-spawn silent zombies via tasks-table scan + grace window (`tests/drive_silent_zombie_e2e.rs`) |
| L071 | 🟡 T029 | drive aborts on runner exit=1 (rate limit) but doesn't notify substrate (ratified; awaiting drive; downgraded T1) |
| L087 | ⚪ — | **auto-promote silent-fails ~0% success on rapid sequential ratifies** — same dispatch-lock-marks-ok-but-no-task pattern as L062, on a different code path; T030's watchdog catches the runtime-side; auto-promote's spawn-without-task gap remains |
| L107 | ⚪ T2 | **T030 watchdog reaps pre-existing dead drive_pids on first post-deploy sweep** — false positives from prior daemon lifetime + drive-startup race window; needs lock-recency / daemon-epoch / parent-pid-liveness check |
| L109 | ⚪ T2 | **drive's next-action returns null for T1+ready+no-plan** — T1 path schema-ratified by T027 but never end-to-end-pulled; surfaced by T029 (first real T1 drive). Drive code likely missing the no-plan-executor-from-contract path |
| L039 | ⚪ T2 | daemon retry-on-failure unimplemented; transient flakes strand rows |
| L068 | ⚪ — | cross-project daemon SIGTERM (other-repo `pkill 'stores agents run'` kills mine) |
| GAP | — | per-project daemon PID file + `stores agents status / stop` verbs |

### Layer 2 — State / idempotency

| obs | state | what hurts |
|---|---|---|
| L063 | ✅ T025 | auto-promote uses `linked_observations` (not surfacing-task `task_id`) for idempotency |
| L038 | 🟡 T033 | `depends_on` pre-flight guard (ratified; awaiting drive; T1 — currently blocked on L109 T1-drive gap) |
| L108 | ⚪ T2 | `fire_on_entry_follow_ons` fires only at add(); retroactive tier_hint update from T2→T1 doesn't re-trigger skip-plan; T029 hit this when downgraded mid-flight |
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
| L060 | ✅ T031 | schema-migrate subscriber spawns `stores migrate --apply` subprocess against on-disk binary (Fix Shape B); freshly-installed schemas drive the diff, not the daemon's stale in-process bundle |
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
| L093 | ⚪ T1 | planner brief lacks tier_hint awareness — T2 planners produce multi-phase plans rejected by submit-plan; demonstrated live on T031's first drive attempt (213s of planner output discarded) |
| GAP | — | **NO `open → investigating` subscriber** — pipeline is one-sided; engine cannot drain its own queue |

## Highest-leverage next picks

T030 ✅ + T031 ✅ landed today. T029/T033 push surfaced three new engine bugs (L107/L108/L109). Remaining picks (re-ranked given the new surface):

1. **L109 (T1-drive gap)** — T1 task drive path missing or broken. **Blocks T029 + T033.** Should investigate / fix before pushing more T1 work. T2 estimate.
2. **L107 (watchdog scope)** — false positives at deploy-time + drive-startup race. T030's watchdog is currently slightly trigger-happy; doesn't break correctness but makes recovery from any stale state painful. T2 estimate.
3. **L108 (on-entry retroactive trigger)** — fix tier_hint mid-flight without losing skip-plan. T2 estimate. Compounds with L109 fix; both about T1 metadata-flow.
4. **L093 (planner brief tier-aware)** — T1 template change. Cheapest engine-economy improvement; saves ~$1-2 per T2 drive.
5. **T029 (L071)** — once L109 fixed, this becomes a T1 drive (~5 min, ~$1-2).
6. **T033 (L038)** — same as T029 once L109 fixed.
7. **L087 investigation** — auto-promote's spawn-without-task gap on rapid ratifies. Same dispatch-lock-shape as the watchdog issues; ideally folded into a single dispatch_lock-primitive refactor.
8. **Auto-investigator subscriber (GAP)** — strategic ceiling. Flips substrate from human-pulls to engine-pulls. Should be designed alongside L087 / L107 since they touch the same dispatch_lock primitive.
9. **L092** (out-of-band close-out) — T2 modest. Lets hand-cranked tasks close cleanly through the substrate.
10. **Substrate-internal DDL migration gap** (newly surfaced today during T030 ship — `actor_note` column added to `SUBSTRATE_DDL` but no migration path for existing DBs; required manual ALTER TABLE). Worth filing as fresh obs; not yet captured.

The geodesic shifted: instead of "drive remaining tasks → engine fixed," the realistic-pull on T029 surfaced that **the T1 cycle has never been pulled end-to-end before**. Fix L109 first; T029/T033/future T1 work all unblock together.


## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
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
