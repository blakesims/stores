# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-11 ADR 0002 completion pass after runner-safety partials. T148 makes ADR 0002 lifecycle/waiting/outcome/typed-reference fields the primary upstream model for inlet, observations, and architecture reviews; raw upstream `status` is compatibility-only. Installed batches now fix live-run selection, autonomous duplicate dispatch, runner safety partials, binary identity diagnostics, recovery-only resume, ER blocked-REVISE reconciliation, and manual ER PASS import. The top risk remains control-plane truth under live runner pressure: active work must show the right owner/PID/session, stale-exe drift must be advisory rather than destructive, and external-review/rebase policy must be visible and recoverable.

## The picture in one sentence

**The engine can ship real work, but operator trust is bounded by whether the control plane tells the truth under live runner pressure.** The T148 incident proved observability and dispatch safety must be treated as runtime invariants: one visible live owner, correct role/PID/session display, no duplicate autonomous spawn, and no stale completed marker masking active work. The next live pain is binary identity/stale-exe clarity, resume/ER recovery semantics, prioritization/ranking, and authoritative external review at integration time.

## Read-this-first priority ladder

1. **Keep live-runner/control-plane truth visible.** T148 proved `status`/`runs current` correctness is the first cockpit primitive: active lanes must not be masked by historical exhaust, duplicate dispatch, stale binaries, or heartbeat-only stalls.
2. **Move authoritative external review to the integration point.** T138 gave the substrate a real integration lane, but review timing can still be invalidated by refresh/rebase/main movement. Durable shape: candidate refreshes against current main → authoritative ER on the to-be-merged head → merge/post-land.
3. **Harden ER acceptance/reconciliation semantics in live use.** Blocked REVISE reconciliation and manual PASS import are now present; next risk is proving them through a full real integration/acceptance cycle and ensuring policy cannot be bypassed.
4. **Finish/land L540/T139 watch cockpit P1.** `stores watch` must become a store-flow cockpit, not a raw mixed dump. T139 should hide historical exhaust, surface live lanes, and make drill-down explicit.
5. **Use recovery-only resume where appropriate.** `resume --no-dispatch` now exists; update SOP/briefs and prove it during the next orphaned-result recovery instead of blind resume.
6. **Make prioritization real, not markdown-only.** `priority`, `priority_rank`, and `priority_rank_at` exist, and watch/list can read them, but current open observations have no `priority_rank` values. L084 names the severity-vs-scheduling conflation.
7. **Right-sized ceremony / T1 fast path.** Tiny safe repairs still pay too much ceremony. Preserve gates for risky work, but add a mechanically-audited fast path for small local fixes.
8. **Empirical runner/model selection.** Per-agent telemetry exists, but model/runner choice is still not a first-class experiment loop. Capture role×runner×model, prompt/config hash, duration, token/cost, and outcome before tuning by vibe.
9. **Auto-resolve / lifecycle residue hardening.** Recent cleanup routed the current intake residue, but L555 shows schema edges still strand obsolete observations (`investigating → resolved` lacks autonomous close-as-addressed). Prevent remint/residue recurrence instead of periodic sweeps.
10. **Manual narrow-control verbs.** The session proved narrow recovery verbs help (`enqueue-integration`, `run-integration`, `external_reviews create-pending`). Keep adding audited single-row controls where daemon startup sweeps are too broad.
11. **Priority + file-overlap scheduler.** Only after ranking/visibility is trustworthy; otherwise concurrency manufactures stale-base and rebase debt.

**Not current priorities:** the inactive accepted rows `T002`, `T005`, `T015`, and `T018` are historical/awaiting-integration residue, not the next strategic queue. `T015` may be superseded by T139; `T002` is useful later after telemetry; `T005`/`T018` are low unless topology becomes the active operator bottleneck. Prefer derived visibility over bulk status mutation.

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
*Drive-loop reliability is stronger and T140 reduced the ignition hazard, but runtime trust still depends on visible identity and liveness. Watchdog catches silent zombies; retry-on-failure reschedules transient flakes; the remaining question for the operator is whether a given daemon/binary/workspace/lock condition is live, stale, harmless, or blocking.*

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
| L543 | ⚪ high | daemon/binary/schema identity remains a high-priority class: stale or half-migrated binaries must fail loudly and be visible. The acute T140 integration recovery is fixed/installed, but the observation remains open and should be re-triaged against current code before promotion. |
| GAP-drive-worktree-required | ✅ T140/direct | manual drive/worktree ignition hazard was contained during T140; T140 landed installed and `plan-start`/activation now expose safer operator state. Keep any remaining fail-closed workspace-path checks verified before broad daemon automation. |
| L150 | ⚪ T2 | halt/deploy-blocked subscriber mislabels blocked drive failures as deploy_blocked merge-conflict observations; another symptom of untyped terminal state / event postconditions, folded into L134/L135 |
| L068 | ✅ T080 | cross-project daemon SIGTERM scoped per-project (cross-listed in Layer 8) |
| GAP-stop-foreground | — | `stores agents stop` requires `--detach`-mode pidfile; foreground daemons can't be stopped via the verb. Hit during 2026-05-08 cleanup. |
| GAP-log-fd-drift | — | `--log-file` flag doesn't redirect fd 1/2 when the daemon runs without `--detach`; configured log file goes silent while activity flows to wherever the launching shell pointed stdout. Hit during 2026-05-08 cleanup. |
| T148-dup-dispatch | ✅ direct | 2026-05-11 T148 live incident: duplicate `stores tasks drive T148` + duplicate `pi_runner executor` spawned against one worktree. Fixed by central autonomous dispatch ownership checks plus manual drive singleton guard: inactive rows, live `drive_pid`, unfinished live dispatch lock, fresh running marker, and same-worktree live owners now hold/refuse instead of spawning. Merges `dbc45cb`, `a957ab9`, `c27c341`; validation notes `04-...repair-plan.md` and `05-...partials-plan.md`. |
| T148-stale-exe-worker | ✅ direct | Updating/installing main/private binary while a worktree worker is running no longer blocks the task as `drive_failed:stale_binary_inode`. Post-spawn stale exe drift is advisory; normal no-output liveness still applies. Live validation updated binaries during T148 executor run: runner survived, no duplicate, no new stale_binary_inode transition. Commits `ec7f67f`, `e194249`. |
| binary-identity-diagnostics | ✅ direct | Stale/reexec diagnostics and `stores --version` now include version, git SHA, build timestamp, launch path, current exe path, and startup dev/inode identity. This keeps behavior unchanged but makes stale-binary debugging explainable. Merge `f44145b`. |

### Layer 2 — State / idempotency

| obs | state | what hurts |
|---|---|---|
| L063 | ✅ T025 | auto-promote uses `linked_observations` (not surfacing-task `task_id`) for idempotency |
| L038 | ✅ T033 | `depends_on` pre-flight guard (T1, shipped after L109/T039 unblocked T1 drives) |
| L108 | ⚪ T2 | `fire_on_entry_follow_ons` fires only at add(); retroactive tier_hint update from T2→T1 doesn't re-trigger skip-plan |
| L130 | ✅ direct | resume routes blocked T2/T3 with plan=null to planning instead of ready (avoids "Phase 1 of 0" deadlock); fixed direct on main during T038 push |
| I033/L519 | ✅ direct | resume is now conservative for all rows: `blocked → planning` only (`9f11fc5`). T123 proved non-empty plans can be rejected/stale and must not route to ready/executing. |
| L132 | ✅ T057 | schema validator refuses unguarded transition shadowing a guarded one (silent override risk) |
| L133 | ✅ T054 | T1 execution shape normalized: synthesize a contract-derived single phase during skip-plan so plan is canonical rather than null/special-cased |
| L011 | ✅ T069 | rows now record the `stores` binary version that wrote them; audit-gap closed |
| L053 | ✅ T081 | tier-A actor check no longer bypassable via `--invoker human` from `$CLAUDECODE`-detected processes (cross-listed from Layer 6) |

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
*Substantially solid post-batch. Daemon merges-cargo-installs-schema-migrates cleanly when it runs (T046's exit-code routing made silent-zombies impossible). L145/T061 added retry-deploy + cargo-install cwd fallback so deploy_blocked recovery is operator-visible and complete. Remaining drag: accept-merge conflict-path side-effect drop (L070), and the merge-conflict recovery dance still uses manual close-out-of-band (T044's verb).*

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
| L069 | ✅ T061 | `compute_resume` now scopes exactly to 'blocked' with helpful guidance for deploy_blocked → use retry-deploy or close-out-of-band (closed transitively via L145/T061) |
| L070 | ⚪ — | accept-merge conflict path drops cargo-install + schema-migrate side effects |
| L144 | ✅ T051 | framework-DDL drift detection/audit shipped; existing DBs learn newer SUBSTRATE_DDL columns without manual ALTER |
| L145 | ✅ T061 | retry-deploy verb shipped: deploy_blocked → accepted re-fires accept-merge → cargo-install → schema-migrate without re-cycling work; accept_merge peer-detection guard preserves L045 solo-recovery; cargo_install cwd fallback validates stores Cargo crate (fails LOUDLY otherwise) |
| L149 | ✅ T062 | daemon stale-exe detection + fail-loud first ship: records dev/ino at startup, tight pre-spawn guard re-checks immediately before each `run_dispatch` for `builtin:auto-drive` (TOCTOU-tight), centralized one-shot fail-loud (`STALE_HALTED` exits `poll_once_with_guard` with `Err(STALE_DAEMON_MESSAGE)`, `run_daemon` bails). Self-reexec deferred to T066/L176. Operator note in `docs/CLAUDE.md`. |
| L150 | ✅ T099 | halt/deploy-blocked subscriber files merge-conflict-shaped observations for rows that are merely `blocked` by drive failure. T099/L483 cascade-dedup subscriber shipped at `cd137a1`: dedup framework-filed `deploy-blocked`/`merge-conflict` observations on (task_id, normalized summary signature) at the auto-file write path; cascade-N now lands as cascade-1+count. Closes the 2026-05-07 cascade's primary surface. |
| L483/T099 | ✅ T099 | cascade-dedup subscriber: when an observation is auto-filed by the runner with `summary_signature`, dedup against an existing open keeper for same (task_id, signature) and increment `dupe_count` + `last_seen` instead of inserting. Schema added `summary_signature`, `dupe_count`, `last_seen` framework-actor fields. |
| L488 | ⚪ T2 | auto-codex review runs on stale-base task branches when main has advanced; should rebase or hold before interpreting missing mainline code as regression. T100 stale-base REVISE → drive subprocess death is the canary. Pi-filed as concrete child of L486. |
| L538 | ✅ T138 | generic integration lane shipped: substrate-owned, repo-agnostic queueing of accepted candidates with capacity-1 partial UNIQUE index `idx_tasks_integration_singleton`; `builtin:integrate` runs configurable refresh (`rebase`/`merge_main`) + ER head re-check + pre-land check + fast-merge into main; pre-rebase `stale_base` check via `git merge-base --is-ancestor` distinct from post-rebase `stale_external_review`; typed outcome enum (integrated/rebase_conflict/stale_base/stale_external_review/pre_land_check_failed/merge_failure/push_failure) recorded in `tasks.integration_attempts` (JSON-array list_record column, single-record-per-attempt invariant); cargo-install + schema-migrate moved to repo-specific subscribers off `integrating → integrated`; status/watch/TUI surface integration_queued/integrating/integrated/integration_blocked separately; two-candidate e2e proves candidate two validates against the main head produced by candidate one. Doc: `docs/integration-lane.md`. |
| L176 | ✅ T066 | self-reexec follow-up to L149: preserve daemon process by execing fresh binary after stale detection. Candidate-binary validation now lands via T075/L182 — the corrupted-stub-exec failure mode is closed. |
| L181 | ⚪ T2 | `stores` CLI fail-silent: corrupted/stub binary returns exit 0 with empty stdout/stderr, violating substrate trust. |
| L182 | ✅ T075 (D) | candidate-binary validation before self-reexec shipped (T075). The (C) half — private install path — split out as **L184** (open, ratifiable). Session SOP forbids subagent cargo-install. (D) closes the recurring stub-corruption-tricks-self-reexec class; (C)/L184 closes the corruption surface itself. |
| L184 | ✅ T076 | Private substrate install path shipped: daemon's runtime-owned binary moved off `~/.cargo/bin/stores` to `~/.local/share/stores/bin/stores`. Confirmed live via `/proc/$(pidof stores)/exe`. Closes the corruption surface from L182(C); subagent / operator `cargo install` no longer touches the daemon's launch path. |
| L484/T100 | 🟢 T2 | rate-limit-aware retry filed (L484) and ratified; T100 in flight (post-rebase code_review). Detects rate-limit responses at the runner boundary, writes `blocked_reason='rate_limit:<provider>:<until>'` so T041 treats as cooldown not flake. Closes the GAP-rate-limit surface. |
| GAP | — | acceptance-time precheck for "task touches files with uncommitted main-side changes → accept-merge will fail" |

### Layer 5 — Discovery / observability
*This is the current strategic bottleneck. T139 is in flight to turn watch into a cockpit; T140 added derived activation/operator disposition; but prioritization is still mostly human-curated markdown. No open observations currently carry `priority_rank`, so the substrate can classify urgency but cannot yet rank next picks mechanically.*

| obs | state | what hurts |
|---|---|---|
| L032 | ✅ T032 | auto-scaffold symlinks `.stores/` artifacts into provisioned worktrees (closes L067 transitively) |
| L165 | ✅ T059 | `stores watch` rebuckets task rows for actionable defaults: terminal/recovered/rejected exhaust no longer belongs in in-flight by default; blocked reason parsing improved |
| L057 | ✅ T070 | per-agent-invocation telemetry shipped: spawn-fail synthetic agent_runs row with source-layer model_id; fail-loud insert; tier T1/T3 tests assert non-NULL prompt_cache_hits + post-cycle agent_runs persistence; mock-defaults under workspace `target/test-workspaces`. |
| L161/T056 | ✅ T056 | minimal Pi-runner smoke target: T1 task driven via Pi produced structured `final_output`, committed one marker file, passed review/wrap, accepted, and merged. This proves Pi runner basics but not full T034 acceptance bookkeeping. |
| L054 | ⚪ — | no structured-read verbs for task review (orchestrator falls back to grep) |
| L058 | ✅ T071 | `stores metrics` CLI shipped: windowed REVISE-rate, percentile interpolation, volatile_window flag for bare duration windows (per Pi Option B). Per-task-type breakdowns. |
| L059 | ✅ T072 | runs SQL VIEW + atomic-backlink-with-dispatch_submit shipped. `stores runs list/show` queries typed view `(display_id, phase, cycle, role, transcript_path)`; transcript_path embedded in cycles JSON in same TX as submit (Pi-critical atomicity invariant). |
| L012 | ⚪ T3 | no inspector for agent context (full graph view: aggregate, post-run, edit) |
| L188/T083 | ✅ T083 | final external review is substrate-native: T2/T3 wrap creates typed `external_reviews`; configurable `review.runner` supports codex, pi, and claude-code; `review.max_parallel` caps the lane; `tooling_held` held reasons are visible in agents/watch; T2/T3 accept requires a current-head PASS while T1 remains lightweight. |
| L529/L540/T139 | 🟢 T139 | watch/flowtop cockpit P1 is active: default surface should show store-flow, active lanes, recent terminal exhaust only by policy, and focused detail instead of raw task/observation/intake noise. |
| L084 | ⚪ T2 | priority conflates scheduling and severity; `priority_rank` exists but is unused (`0` ranked open observations as of 2026-05-10). Need a focus/ranking primitive before backlog ordering is trustworthy. |
| L554 | ⚪ T2 | cluster_key registry coverage gap: cleanup had to reject/drop legitimate friction because the curated key set was too narrow. This is a front-door fidelity issue, not just taxonomy polish. |
| T148-live-status | ✅ direct | `stores tasks status` / `stores runs current` selected completed planner `final_output` over the running executor because marker `updated_at` outranked semantic liveness. Fixed central selector in `src/cli/runs.rs`: running/live markers first, semantic `status.json.last_event_at`, marker `updated_at`; stale uncorroborated running markers are labeled `stale_marker` and do not outrank completed evidence forever. Merges `30f965d`, `669825e`, `3a30782`. |
| T148-payload-errors | ✅ direct | Malformed runner `final_output` / envelope parse failures with child exit 0 now become visible typed `runner_payload_error` blocked reasons, do not advance state, preserve real child exit code in `agent_runs`, and rewrite current-run marker to `status=failed` with `payload_error`. Commit `38bb6b2`. |
| resume-no-dispatch | ✅ direct | `stores tasks resume --no-dispatch` repairs blocked state without immediate follow-on dispatch, sets activation inactive, and leaves an audit note. Ordinary resume behavior remains. Merge `4c0806b`. |
| er-blocked-reconcile | ✅ direct | Terminal external-review REVISE can reconcile from `blocked` only for structured legacy drive-failure reasons (`drive_failed`, `drive_failed:*`, or JSON drive kind), current head, and idempotent already-applied transitions. Blocked PASS remains out of scope. Merge `30ffd7d`. |
| er-import-pass | ✅ direct | `stores external_reviews import-pass` creates an auditable manual PASS row with transcript/base/head/runner provenance and current-head/base checks; accept precheck recognizes it only at current head. Merge `a7682a9`. |

### Layer 6 — Auth / security

| obs | state | what hurts |
|---|---|---|
| L013 | ✅ L185/T078 | Superseded by host-bound plaintext `~/.config/stores/approve.token` (0600) plus `approve.token.hash`; `auth init` no longer discovers SOPS/age keys. |
| L014 | ⚪ T2 | `auth init` UX gaps (opaque binary-format error; 7-line shell ritual) |
| L015 | ✅ T074 | `auth show --identity` flag shipped (now symmetric with `init`) |
| L044 | ✅ L185/T078 | Superseded by removing the SOPS/age identity path and the L015 symlink workaround; `auth show` now reads plaintext directly. |
| L053 | ✅ T081 | tier-A actor check bypass closed (cross-listed from Layer 2) |

### Layer 7 — Schema / contract substrate

| obs | state | what hurts |
|---|---|---|
| L005 | ✅ T073 | list-typed fields on observations update now accept JSON-array input |
| L035 | ⚪ T3 | no schema-enforced inter-agent context refs (typed agents) |
| L019 | ⚪ T3 | no DockerRunner / standardized agent sandboxing |
| I001 | ✅ T068 | `required_when` parser now supports `IN [...]` membership and `expr OR expr` composition (the FIX 5 surface from T053 codex-revise — first real-world use of the gatekeeper Router seam) |

### Layer 8 — Orchestration / triage discipline
*The front door is structurally much better: intake exists, investigator pull-shape shipped, auto-investigator exists, and T140 drained the current intake residue. The live weakness is no longer raw draft volume; it is ranking, dedupe/cluster fidelity, and closure edges for obsolete rows.*

| obs | state | what hurts |
|---|---|---|
| L043 | ✅ T038 | investigator subagent pull-shape (sandboxed dive, returns structured report) |
| L049 | ✅ T037 | auto-resolve subscriber on cargo_installed→schema_migrated transition |
| L092 | ✅ T044 | `tasks close-out-of-band` verb (cross-listed from Layer 4) |
| L093 | ✅ T039 | planner brief tier-aware (cross-listed from Layer 3) |
| L137 | ✅ T048 | auto-resolve startup-sweep / backfill; 15 stale schema_migrated→ready obs pairs cleaned by startup sweep |
| L142 | ✅ T053 | intake_items store + gatekeeper Router seam shipped (P1 of T045 design): six routing decisions, structured gatekeeper_decision_json validator, tagged-observation stand-in for arch_review_candidate, narrow `SideEffectAuthority::GatekeeperRoute` typed authority. Codex caught architectural drift (escalated state, missing route validation, decision-mismatch, side-effect actor); revise loop landed clean. Direct observations add escape hatch preserved. |
| L143 | ✅ T052 | risk_class + approval_policy fields to observations schema; canonical enums + direction-aware override-policy gate |
| L134 | ✅ T050 | formalize dispatch_locks as typed lifecycle buffer (compounds with L039/L087/L107/L116/L122/L141/L149) |
| L135 | ✅ T063 | Check primitive P1 shipped: trait + compile-time registry + structured CheckResult; two adapters (L134 dispatch postconditions; T053 gatekeeper validator) prove the shape; broader site adoption is follow-up; pairs with L172 fast-track auto-execution |
| L171 | ✅ T077 | dedicated `architecture_reviews` typed store (P3 of T045 design) shipped: A### namespace; interpret/amend split; seven typed verdict outcomes; amend `cascade_decisions` + human-token `ratify-amend`; flexible `supersedes`; gatekeeper `arch_review_candidate` writes A### plus `pending_architecture_review=true`; reframe/merge U1 gate clearing; idempotent backfill; architecture-reviews render projection |
| L172 | ⚪ T3 | **deferred** post-T053: fast-track auto-execution + L135 Check primitive (P4 of T045 design); deterministic check record audit shape |
| L173 | ✅ T107 | curated cluster_key registry + watch/observability dashboard work shipped enough to resolve L173, but L554 shows registry coverage is still too narrow in real cleanup. |
| L072 | ⚪ — | code-reviewer REPLAN gate dead-ends as `blocked` instead of routing back to planning |
| L023 | ✅ T092 | observations `next-id` verb + JSON envelope shape shipped |
| L124 | ✅ T043 | `tasks abandon <id> --reason <text>` verb retires stale/superseded/duplicate/misadd rows to terminal `abandoned` without burning a drive cycle; tier-A token-mediated; idempotent; watch/TUI bucket: terminal-history (not in-flight). Allowed from 9 non-terminal states incl. complete; refused from 5 successful terminals (accepted/rejected/cargo_installed/schema_migrated/closed_out_of_band). |
| L002 | ✅ T043 | `tasks abandon` verb provides non-destructive admin retirement (closed transitively via L124/T043 — abandoned terminal preserves row + audit reason, no .stores wipe needed) |
| L003 | ⚪ T2 | observations list output unscannable for >2 rows |
| L006 | ⚪ T2 | observations runner asymmetry (no drive cycle for obs) |
| L021 | ✅ T058 | render template pulls `wrap_log` into Completion section |
| L034 | ✅ T124 | wrap diff attribution is direction-aware; T124 shipped after codex fixes for master fallback and workspace-local diff computation. |
| L186 | ✅ T079 | engine-runner actionability monitor LIVE: daemon-side loop scans substrate-visible rows, writes a heartbeat per iteration, redispatches orphaned autonomous edges, and structurally records held reasons. Currently visible in `/tmp/daemon2.log` ticking `[engine-runner] iter=N saw=tasks:M intake:K obs:O actionable=A held=H dispatched=D`. Closes the "engine stalls when ec/ec-on-main is silent" pattern. |
| L068 | ✅ T080 | cross-project daemon SIGTERM no longer blanket-kills via `pkill 'stores agents run'` (T080 added per-project scoping; cross-listed from Layer 1's GAP) |
| L151 | ✅ T065 | auto-investigator subscriber shipped: fires investigator on `open → needs_investigation` automatically (was the GAP at the bottom of this layer; the substrate can now drain its own input queue) |
| L188 | ✅ T083 | substrate-native external review lane shipped (cross-listed from Layer 5) |
| L193 | ✅ T086 | external_reviews lane reconciler shipped: T2/T3 in_review tasks reconciled state-driven, not just verdict-driven; in-TX cap=running with BEGIN IMMEDIATE serialization; typed `DispatchOutcome::{Dispatched, CapHeld, RaceLost}`. Hand-merged at `8bd21b6` (chicken-and-egg meta fix) and closed-out-of-band 2026-05-08 — the missed close-out triggered the overnight cascade across T084/T085/T088/T093/T095/T096. |
| L194 | ✅ T087 | `topology_dot_snapshot::ac2_4_dot_snapshot_matches` no longer flakes on color/fontcolor DOT ordering |
| L196 | ✅ T089 | auto-drive-watchdog no longer spams `mark_drive_failed` on terminal tasks (T034 abandoned, T050 closed_out_of_band, T053 schema_migrated) |
| L079 | ✅ T090 | auto-scaffold builtin captures shim stderr; operator decisions (e.g. seed-decision rationale) no longer silently dropped |
| L041 | ✅ T091 | topology `--format auto` Z1 tasks line within 120-col contract |
| L197 | ✅ T094 | T086 Layer 2 elapsed retry shipped: rows stuck after first failure now retry on elapsed-tooling-held |
| L199 | ✅ T097 | external_review verdict parser tolerates codex prose (no longer requires leading PASS/REVISE/TOOLING_FAILURE token) |
| L077 | ✅ T082 | substrate persistence for high-leverage derivation tokens: `intent_contract.hardened_*` shipped |
| I002 | ⚪ — | **code-reviewer / codex grepping `.sqlite` raw bytes treats stale page data as live row state** — burns review cycles on false-positive transitions (T078 c1-c3 burn). Filed via intake `I002`; doctrine fix pending: SOP edit + reviewer validator forbidding raw-byte inspection of sqlite files; substitute `sqlite3 SELECT` or substrate CLI verbs. |
| L555 | ⚪ T2 | obsolete investigated/investigating observations can be structurally unreachable to autonomous cleanup because `close_as_addressed` lacks an `investigating → resolved` edge; add a safe close path or equivalent recovery verb. |
| GAP-cascade-dedup | — | Mostly covered by L483/T099 for framework-filed merge-conflict signatures, but keep an eye on new duplicate surfaces (L544) where subscriber cadence still emits repeated observations. |

## Architect big-picture priorities

As of 2026-05-10, the strategic map is:

1. **Truthful control surfaces before more autonomy:** the system can ship, but the operator must be able to see which rows are live, historical, blocked, stale, or merely noisy.
2. **Front-door fidelity and ranking:** intake/observations/watch should produce an ordered next-pick queue. Today `priority_rank` exists but is empty, so engine-health markdown is still doing too much work.
3. **Review/integration correctness:** external review must be current with the exact integration candidate after refresh/rebase, not merely current when wrap happened.
4. **Right-sized ceremony:** keep full ceremony for risky work; create a checked fast path for tiny deterministic repairs.
5. **Empirical engine telemetry:** use the shipped metrics/runs surfaces to measure runner/model/prompt outcomes before model-policy changes.
6. **Safe throughput:** only add priority/file-overlap scheduling once ranking and visibility are trustworthy.

## Highest-leverage next picks

1. **Finish T139.** It is the active row and directly repairs the operator cockpit. Do not distract it with legacy accepted rows.
2. **Design/ship integration-point external review.** Make ER authoritative immediately before merge after branch refresh; this reduces stale-review churn and false confidence.
3. **Add a real focus/ranking primitive.** Write `priority_rank`/`priority_rank_at`, separate severity from scheduling (L084), and render next-picks from live substrate rows instead of this markdown doc.
4. **Close the watch/backlog loop:** after T139 lands, verify `stores watch`, `stores engine plan-start`, and open observations agree on what is actionable; hide historical accepted residue by derivation, not DB mutation.
5. **Fix lifecycle residue edges:** especially L555 (`investigating → resolved` cleanup) and any remaining duplicate auto-file surfaces (L544/L554).
6. **T1 fast path.** Reduce ceremony cost for tiny safe fixes with deterministic checks.
7. **Telemetry/model policy.** Use observed role×runner×model outcomes before changing per-role model defaults.
8. **File-overlap scheduler.** Defer until the queue is ranked and stale-base/review timing is fixed.

Current picture: intake is clean, tasks have one active engine row, and observations are the main backlog. The backlog should be handled by cluster/rank, not oldest-first. Historical accepted tasks (`T002`, `T005`, `T015`, `T018`) are not strategic blockers; revisit only after T139 clarifies what UI/topology/model-config work remains.

## Doc size / archival policy

This file is now large enough that it should stop accumulating full history. Keep it as the **current health dashboard**, not the permanent changelog.

Keep here:
- the one-sentence picture, priority ladder, architect priorities, and highest-leverage next picks;
- open/high-risk rows and recently-shipped items from the last few days that still explain current decisions;
- shipped rows only when they define a current invariant or prevent re-litigation;
- **at most ~10 rows per table** in the main dashboard. If a table wants an 11th row, compress older entries into a one-line summary and link to worklog/archive detail.

Archive or compress:
- old ✅ rows whose invariant is now uncontroversial and covered by tests/docs;
- long implementation detail for shipped tasks (move to `docs/worklog/` or a dated archive such as `docs/archive/2026-05/engine-health-history.md`);
- stale “recently shipped” entries older than the current operational window;
- repeated same-day shipped rows once their only value is historical audit. Summarize the batch in one or two lines instead of preserving every row inline.

Do **not** delete history outright: it is useful for debugging doctrine drift. But the main `engine-health.md` should become a curated index with links to dated detail, otherwise the current priorities drown in solved incidents.

## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
| 2026-05-10 | T140/direct | — | ignition cleanup/activation gate landed and installed. `stores engine plan-start` reports derived activation/operator disposition; manual integration recovery verbs and external-review pending creation were added during recovery; T140 is `schema_migrated`. |
| 2026-05-10 | direct | L541/I036/I037 | cleanup pass closed stale L541 and routed intake residue into observations L554/L555; watch default now hides resolved/wont_fix observations, routed/dropped intake, and stale terminal exhaust by derived visibility. |
| 2026-05-09 | direct | I033/L519 | Resume safety repaired after T123 unsafe resume reproduced the contamination class. `tasks resume` now always routes `blocked → planning`; schema no longer has a fallthrough `blocked → ready` resume edge. Tests updated around conservative resume. Commit `9f11fc5`. |
| 2026-05-09 | T116 | L180 | T1 exact-token silent_zombie matcher shipped after manual-control drive. Final diff tightens `src/tui/data.rs`, updates watch classification tests and live-realistic fixture. Shows correctness benefit of review but excessive ceremony cost for tiny fixes. |
| 2026-05-09 | T124 | L034 | Direction-aware wrap diff attribution shipped: branch/base sections, workspace-local git diff computation, master fallback fix. Codex caught two substantive issues before accept. Merge `94357a1`; schema repair for ready-observation disposition `575615b`. |
| 2026-05-09 | cleanup | L034/L087/L110/L485/L489/L499/L513/L514/L515/L519/L520/L525/L527/L530/L532-L537 | Manual queue cleanup closed remint holes and residue rows. T014/T116 reached `schema_migrated`; T108/T126/T128/T123/T134-T137 retired; L538/T138 created as clean integration-lane replacement. |
| 2026-05-09 | direct | — | T140 main-cwd incident contained: old main drive resumed to a rate-limit block, T140 moved to `/home/blake/repos/experiments/stores-T140-engine-ignition-cleanup-activation-gate`, `workspace_path`/branch set via `tasks update`, and main cleaned after committing docs. Direct fail-closed fix implemented separately on `fix/drive-worktree-fail-closed` (`ec087fe`). |
| 2026-05-09 | direct/10.06 | L543 | Daemon binary identity regression surfaced: fresh PATH 0.6.0 daemon self-reexecs into stale canonical 0.5.0, so integrate lane cannot dispatch. 10.06 owns quick fix (sync canonical binary); this lane should not run cargo-install or daemon restart experiments that race it. |
| 2026-05-09 | direct | — | Narrow external review primitive `stores external_reviews run <ERID>` added (`06403b5`) so one ER row can run without daemon startup sweeps / auto-drive / watchdog side effects. |
| 2026-05-08 and earlier | archived batch | many | Earlier recovery work shipped external-review lane/reconciler, cascade dedup, topology/watch primitives, auth/token safety, auto-drive/scaffold/promote, metrics/runs, brief checks, retry/reconcile recovery, user-escalation/zombie fixes, and observation cleanup. Keep detailed audit in dated worklogs/archive; do not grow this table past the current operational window. |

## How to update this doc

This is a hand-curated snapshot, not a generated report. Refresh it at inflection points:

- **A batch of fixes lands.** Move shipped obs from open to ✅; add only the newest/high-signal rows to "Recently shipped"; compress older rows once a table exceeds ~10 entries.
- **A new high-priority obs surfaces.** Add a row to the relevant Layer, replacing or compressing a lower-signal row if needed.
- **A bug class is named that wasn't previously visible.** Add a new Layer or GAP line only if it changes current priorities.
- **The "highest-leverage next picks" section drifts.** Re-rank based on current ratifiable contracts and the day's pain.

To regenerate the observation lifecycle snapshot, query the DB:
```
sqlite3 .stores/db.sqlite "SELECT display_id, lifecycle, contract_state, json_extract(intent_contract,'$.tier_hint') as tier, COALESCE(task_id,'') as tid, summary FROM observations WHERE COALESCE(lifecycle,'') != 'closed' ORDER BY display_id;"
```

For the deeper reasoning behind any single shipped item, the worklog under `docs/worklog/<date>/` has the session detail. Promote insights here when they become long-standing; archive verbose history rather than growing this dashboard indefinitely.
