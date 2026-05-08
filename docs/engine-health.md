# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-08 (post-overnight cascade cleanup) — Operator-trust + actionability layers are now substantially closed. The 3-agent overnight session shipped 27 tasks; the daemon now runs on the **T076 private install path** (`~/.local/share/stores/bin/stores`, confirmed live), with the **T079 engine-runner monitor** as its core loop and the **T083 substrate-native external review lane** in production. **T086/L193** (the ER reconciler — meta-substrate fix) was hand-merged at `8bd21b6` and closed-out-of-band today; that close was the missing step that triggered last night's cascade where T084/T085/T088/T093/T095/T096 inherited rebase debt and the runner crash-looped (amplified by codex/pi rate limits — see Layer 1's open gap). All six are now rebased clean; daemon currently OFF pending Phase-6 restart. The live priority is no longer "can the engine choose, review, integrate work?" but **priority + file-overlap scheduling** (so concurrent work doesn't create rebase debt by itself) and **metrics over runner/review outcomes**.

## The picture in one sentence

**The engine can now choose, review, and integrate work without chat-shaped glue, but raising concurrency without priority + file-overlap routing creates rebase debt faster than throughput.** The next geodesic is the priority+file-overlap scheduler, then metrics that turn runner/reviewer choice into evidence, then a rate-limit-aware retry that distinguishes transient flake from quota-exhausted.

## Read-this-first priority ladder

1. **Restart the daemon + resume rebased rows (T084/T085/T088/T093/T095/T096).** All six were caught in the T086 meta-merge cascade; locally rebased clean today. Daemon is OFF; Phase 6 of the overnight cleanup plan covers restart + resume. Watch one tick to confirm the watchdog typed-closes the 6 dangling `auto-drive` locks (epoch shift triggers T040 + T050 stale-detection).
2. **File the rate-limit-aware retry gap.** T085's 93 cycles + ~50 `P5: resubmit verified watch cockpit fixtures` commits are the smoking gun: T041's retry-on-failure rescheduler doesn't distinguish transient flake from rate-limit-exhausted. T029/L071 was supposed to type this case as `blocked:rate_limit` but `runner_crash exit_code=3` is leaking through. Tier T2.
3. **Run an observation backlog hygiene sweep.** ~354 open observations; the gatekeeper Router holds them as `needs_human` until ratified. Classify/close duplicates (L465–L479 are 15 dupe `deploy-blocked merge conflict` entries from the cascade), addressed rows, stale rows, and draft only the real contracts. Cascade-dedup follow-up should be filed alongside.
4. **Build priority + file-overlap scheduler.** Pick the highest-priority compatible row and hold conflicts with explicit reasons; do not raise WIP by blind active-count. The cascade is the cost evidence.
5. **Add runner/review metrics.** Use T070/T071/T072/T083 data to compare pi vs claude and codex/pi/claude review outcomes by role/tier/duration/cost.
6. **Then expand adapters/domains.** CodeRabbit, richer conflict domains, L006 observation runner asymmetry, and L035 typed inter-agent refs come after the scheduler + metrics substrate is legible.

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
| L068 | ✅ T080 | cross-project daemon SIGTERM scoped per-project (cross-listed in Layer 8) |
| GAP-stop-foreground | — | `stores agents stop` requires `--detach`-mode pidfile; foreground daemons can't be stopped via the verb. Hit during 2026-05-08 cleanup. |
| GAP-log-fd-drift | — | `--log-file` flag doesn't redirect fd 1/2 when the daemon runs without `--detach`; configured log file goes silent while activity flows to wherever the launching shell pointed stdout. Hit during 2026-05-08 cleanup. |

### Layer 2 — State / idempotency

| obs | state | what hurts |
|---|---|---|
| L063 | ✅ T025 | auto-promote uses `linked_observations` (not surfacing-task `task_id`) for idempotency |
| L038 | ✅ T033 | `depends_on` pre-flight guard (T1, shipped after L109/T039 unblocked T1 drives) |
| L108 | ⚪ T2 | `fire_on_entry_follow_ons` fires only at add(); retroactive tier_hint update from T2→T1 doesn't re-trigger skip-plan |
| L130 | ✅ direct | resume routes blocked T2/T3 with plan=null to planning instead of ready (avoids "Phase 1 of 0" deadlock); fixed direct on main during T038 push |
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
| L150 | ⚪ T2 | halt/deploy-blocked subscriber files merge-conflict-shaped observations for rows that are merely `blocked` by drive failure (e.g. T034 silent-zombie / Pi-smoke failures). T089/L196 narrowed the watchdog's terminal-state filter; the dedup-on-(task_id, conflict_signature) gap remains and was hit hard overnight (L465–L479 = 15 dupe rows from the T086 cascade). Needs typed event/terminal reason before templating operator-facing halt observations. |
| L176 | ✅ T066 | self-reexec follow-up to L149: preserve daemon process by execing fresh binary after stale detection. Candidate-binary validation now lands via T075/L182 — the corrupted-stub-exec failure mode is closed. |
| L181 | ⚪ T2 | `stores` CLI fail-silent: corrupted/stub binary returns exit 0 with empty stdout/stderr, violating substrate trust. |
| L182 | ✅ T075 (D) | candidate-binary validation before self-reexec shipped (T075). The (C) half — private install path — split out as **L184** (open, ratifiable). Session SOP forbids subagent cargo-install. (D) closes the recurring stub-corruption-tricks-self-reexec class; (C)/L184 closes the corruption surface itself. |
| L184 | ✅ T076 | Private substrate install path shipped: daemon's runtime-owned binary moved off `~/.cargo/bin/stores` to `~/.local/share/stores/bin/stores`. Confirmed live via `/proc/$(pidof stores)/exe`. Closes the corruption surface from L182(C); subagent / operator `cargo install` no longer touches the daemon's launch path. |
| GAP-rate-limit | ⚪ T2 | T041 retry-on-failure doesn't distinguish transient flake from rate-limit-exhausted. T029/L071 was supposed to type runner-exit-on-rate-limit as `blocked:rate_limit`, but `runner_crash exit_code=3` is leaking through. Surfaced overnight via T085's 93-cycle thrash; needs to be filed as an observation. |
| GAP | — | acceptance-time precheck for "task touches files with uncommitted main-side changes → accept-merge will fail" |

### Layer 5 — Discovery / observability

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
*Investigator pull-shape shipped (T038/L043) — orchestrator now has a substrate primitive for "spawn a fresh sandboxed dive on this question". Auto-resolve subscriber shipped (T037/L049). The auto-investigator-fires-on-unblock-of-open-obs primitive is still the #1 strategic weakness; ~30 open obs sit without ratified contracts and the engine can't draft them itself yet.*

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
| L173 | ⚪ T3 | **deferred** post-T053: curated cluster_key registry + watch/observability dashboards (P5 of T045 design) |
| L072 | ⚪ — | code-reviewer REPLAN gate dead-ends as `blocked` instead of routing back to planning |
| L023 | ✅ T092 | observations `next-id` verb + JSON envelope shape shipped |
| L124 | ✅ T043 | `tasks abandon <id> --reason <text>` verb retires stale/superseded/duplicate/misadd rows to terminal `abandoned` without burning a drive cycle; tier-A token-mediated; idempotent; watch/TUI bucket: terminal-history (not in-flight). Allowed from 9 non-terminal states incl. complete; refused from 5 successful terminals (accepted/rejected/cargo_installed/schema_migrated/closed_out_of_band). |
| L002 | ✅ T043 | `tasks abandon` verb provides non-destructive admin retirement (closed transitively via L124/T043 — abandoned terminal preserves row + audit reason, no .stores wipe needed) |
| L003 | ⚪ T2 | observations list output unscannable for >2 rows |
| L006 | ⚪ T2 | observations runner asymmetry (no drive cycle for obs) |
| L021 | ✅ T058 | render template pulls `wrap_log` into Completion section |
| L034 | ⚪ T1 | wrap misattributes main-ahead commits as 'rides on this branch' |
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
| GAP-cascade-dedup | — | Subscriber should dedup `deploy-blocked: merge conflict` observations on `(task_id, conflict_signature)` to avoid the L465–L479 dupe storm seen during the 2026-05-08 T086 cascade (one obs per tick × 6 affected branches × hours = 15+ duplicate rows). Tier T1. |

## Highest-leverage next picks

Operator-trust + actionability layers are now substantially closed: T076/L184 ✅, T077/L171 ✅, T078/L185 ✅, T079/L186 ✅, T080/L068 ✅, T081/L053 ✅, T082/L077 ✅, T083/L188 ✅, T086/L193 ✅ (closed-out-of-band). The bottleneck is no longer "can the engine choose, review, integrate work?" — it's **"how does the engine raise concurrency without making rebase debt faster than throughput?"** and **"how does the engine pick a runner / reviewer based on evidence rather than vibes?"**.

The 2026-05-08 cascade is the cost evidence: the T086 close-out-of-band step was missed → six in-flight branches inherited the meta-merge conflict → runner crashed on stale base → T041 retried 27/35/50/55/93 times → codex/pi rate-limit further amplified the loop → 15+ dupe `deploy-blocked merge conflict` observations filed before anyone noticed.

1. **Priority + file-overlap scheduler (GAP / file next)** — the cascade above is exactly what this prevents. Choose the highest-priority compatible row, not just the next active slot. Phase 1 uses expected/actual touched-file overlap plus current priority fields (`priority`, `priority_rank`, `scheduled_for`) and writes held reasons like `held: overlaps active TXXX on src/flow/external_review.rs`. Conflict domains (semantic, schema, doctrine) can come later.
2. **Rate-limit-aware retry (`GAP-rate-limit` filed in Layer 1)** — T041's retry-on-failure is a pure backoff; T029/L071 was supposed to type the rate-limit case as `blocked:rate_limit` but `runner_crash exit_code=3` is leaking through. The 93-cycle T085 thrash is the smoking gun. Tier T2.
3. **Cascade-dedup subscriber (`GAP-cascade-dedup` filed in Layer 8)** — when a substrate-changing task merges into main, every open branch can re-fire `deploy-blocked merge conflict` per tick. Dedup on `(task_id, conflict_signature)`. Tier T1.
4. **Metrics over runner/review outcomes** — T070/T071/T072/T083 store typed records; report pi-vs-claude planner/executor/reviewer effectiveness, codex/pi/claude review PASS/REVISE/TOOLING_FAILURE rates, duration, and cost. Runner-selection feedback, not vanity analytics.
5. **Observation backlog hygiene sweep** — ~354 open observations; many are stale, addressed, or duplicates (the L465+ cascade rows alone). Classify/close before drafting more contracts.
6. **Gatekeeper/Router autonomy follow-through** — T053/L142 classifies intake; it should not become the scheduler. Keep roles separate: Gatekeeper classifies new input, Scheduler chooses next compatible executable work, Architect governs coherence, Reviewer checks implementation.
7. **I002 — sqlite-raw-byte review false-positive doctrine fix** — forbid `grep`/`strings` against `.sqlite` files as evidence of live substrate state; use `sqlite3 SELECT` or substrate CLI verbs.
8. **L172/L173 / CodeRabbit / richer conflict domains** — follow after the scheduler + metrics prove the shape.

Current picture: the engine can do real work; the next throughput gains come from making concurrency safe (file-overlap scheduler), making retry honest (rate-limit awareness), and making runner choice evidence-driven (metrics).


## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
| 2026-05-08 | direct | L193 | T086 closed-out-of-band with merge SHA `8bd21b6` after manual main-merge ceremony. Closure of the missed step that triggered the overnight cascade; substrate row state now matches the on-main reality. |
| 2026-05-07/08 | T097 | L199 | external_review verdict parser tolerates codex prose: no longer requires leading PASS/REVISE/TOOLING_FAILURE token. |
| 2026-05-07/08 | T094 | L197 | T086 Layer 2 elapsed retry: rows stuck in tooling-held after first failure now retry on elapsed-tooling-held. |
| 2026-05-07/08 | T092 | L023 | observations `next-id` verb shipped (CLI symmetry with tasks); `list --json` envelope shape consistent. |
| 2026-05-07/08 | T091 | L041 | topology `--format auto` Z1 tasks line within 120-col contract. |
| 2026-05-07/08 | T090 | L079 | auto-scaffold builtin captures shim stderr; operator decisions (e.g. seed-decision rationale) no longer silently dropped. |
| 2026-05-07/08 | T089 | L196 | auto-drive-watchdog filters terminal task states (T034 abandoned, T050 closed_out_of_band, T053 schema_migrated) from `mark_drive_failed` spam. |
| 2026-05-07/08 | T087 | L194 | `topology_dot_snapshot::ac2_4_dot_snapshot_matches` no longer flakes on color/fontcolor DOT ordering. |
| 2026-05-07/08 | T086 | L193 | external_reviews lane reconciler shipped (meta-substrate fix): T2/T3 in_review reconciled state-driven, in-TX cap=running with BEGIN IMMEDIATE serialization, typed `DispatchOutcome::{Dispatched, CapHeld, RaceLost}`. Hand-merged at `8bd21b6` (chicken-and-egg: T086 fixes its own deploy gate). |
| 2026-05-07/08 | T083 | L188 | substrate-native external review lane shipped: typed `external_reviews` post-wrap records for T2/T3; configurable `review.runner` supports codex / pi / claude-code; `review.max_parallel` lane cap; `tooling_held` reasons visible in agents/watch; T2/T3 accept blocked until current-head PASS exists; T1 stays lightweight. |
| 2026-05-07/08 | T082 | L077 | substrate persistence for high-leverage derivation tokens: `intent_contract.hardened_*` shipped. |
| 2026-05-07/08 | T081 | L053 | tier-A actor check no longer bypassable via `--invoker human` from `$CLAUDECODE`-detected processes. |
| 2026-05-07/08 | T080 | L068 | cross-project daemon SIGTERM scoped per-project (no more blanket `pkill 'stores agents run'` cross-project kills). |
| 2026-05-07/08 | T079 | L186 | engine-runner actionability monitor LIVE in daemon: heartbeat per iteration, redispatch of orphaned autonomous edges, structural held-reason recording. Phase-1 narrow shape (visibility + redispatch only; no new policy semantics; U-moments preserved). Closes the chat-shaped engine-stall pattern. |
| 2026-05-07/08 | T078 | L185 | approval token simplified to host-bound plaintext+0600 (`~/.config/stores/approve.token`) plus `approve.token.hash` for constant-time verification; SOPS+age dropped. |
| 2026-05-07/08 | T076 | L184 | private substrate install path shipped: daemon's runtime-owned binary moved to `~/.local/share/stores/bin/stores`; `cargo install` no longer corrupts the daemon's launch path. |
| 2026-05-07/08 | T074 | L015 | `auth show --identity` flag shipped (symmetric with `auth init`). |
| 2026-05-07/08 | T073 | L005 | observations update accepts JSON-array input for list-typed fields. |
| 2026-05-07/08 | T071 | L058 | `stores metrics` CLI shipped: windowed REVISE-rate, percentile interpolation, volatile_window flag; per-task-type breakdowns. |
| 2026-05-07/08 | T070 | L057 | per-agent-invocation telemetry shipped: spawn-fail synthetic agent_runs row with source-layer model_id; fail-loud insert; tier T1/T3 tests assert non-NULL prompt_cache_hits + post-cycle agent_runs persistence. |
| 2026-05-07/08 | T069 | L011 | rows now record the `stores` binary version that wrote them; audit-gap during sessions closed. |
| 2026-05-07/08 | T068 | I001 | `required_when` parser supports `IN [...]` membership and `expr OR expr` composition. |
| 2026-05-07/08 | T066 | L176 | daemon self-reexec on stale-exe (with T075/L182 candidate validation; closes the corrupted-stub-exec failure mode). |
| 2026-05-07/08 | T065 | L151 | auto-investigator subscriber shipped: fires investigator on `open → needs_investigation` automatically. Engine can now drain its own input queue. |
| 2026-05-07/08 | T064 | L175 | `stores watch` rebuckets task rows so terminal/recovered/rejected exhaust no longer drowns actionable work; `--all` escape hatch preserved. |
| 2026-05-07 | T077 | L171 | dedicated `architecture_reviews` typed store shipped as Heart/Architect phase α: A### lifecycle; interpret vs amend authority split; seven typed verdict outcomes; amend `cascade_decisions`; pure-human token-mediated `ratify-amend`; flexible-precedent `supersedes`; gatekeeper `arch_review_candidate` same-TX A### + `pending_architecture_review=true`; U1 pre-ratification gate with reframe/merge clearing; idempotent backfill from T053/L142 historical tagged candidates; render projection to `architecture-reviews/A###/main.md`. Deferred explicitly: typed Heart store, typed `actor: architect`, doc-diff projection hook, and auto-fire subscribers. |
| 2026-05-07 | T067 | L178 | manual-drive ↔ daemon handoff fix shipped (A1-strict): `wrap_log` is NOT a control sentinel; `next_agent` is the source of truth for current-cycle wrap completion. Implementation via `force_close_auto_drive_lock_ok` writes `last_status='ok:wrap_completed'` (free-text column; CHECK-constrained typed `terminal_reason='wrap_completed'` deferred — table-recreation cost not justified for this slice). Watchdog SQL predicate distinguishes force-closed-this-invocation locks from old-handoff-still-pending-wrap locks (both have `terminal_reason='ok'` but only force-closed have `last_status='ok:wrap_completed'`). force-close ordering fix: now invoked BEFORE post-submit `--max-iters` bail (was after, allowing race to leave `in_flight:pending_next` for a legitimately-completed wrap). Two new e2e tests pin the invariants through real production paths (`watchdog_force_closed_wrap_lock_no_redispatch`, `max_iters_after_wrap_dispatch_force_closes_lock`). Codex r6 → r7: 4 findings (1 HIGH watchdog distinguisher + 2 MEDIUM ordering/test + 1 LOW projection-revert) all closed; r7 PASS at ecc68dd → 26d92a8 after post-T072 re-rebase. 1105 tests pass. Merge `3784a6f`. |
| 2026-05-07 | T072 | L059 | runs SQL VIEW + atomic backlink with dispatch_submit shipped: new typed `runs` SQL view exposes `(display_id, phase, cycle, role, transcript_path)` over `cycles` JSON; `stores runs list/show` CLI subcommand queries the VIEW directly (decouples read surface from physical layout); transcript_path threaded through `compute_submit_execute`/`compute_submit_review` and embedded into cycles JSON in same TX as `write_status_and_fields(...).commit()` — atomicity guaranteed (Pi-critical invariant). `session_id=None` bails explicit error pre-submit (no silent default). Idempotence test correctly re-named `executor_transcript_path_consistent_under_retry` (production is append-only-with-no-dedup; the no-double-write semantics is a separate Pi-architectural call deferred). Rebase resolved T071-metrics-CLI ↔ T072-runs-CLI collision in `src/cli/{dynamic,mod}.rs` + `src/main.rs` (both subcommands coexist). Codex r6 → r7: 1 MINOR test-fidelity gap; r7 PASS at c30828f. 1099/0 lib + 3/3 runs_cli. Merge `e250e5d`. |
| 2026-05-07 | T075 | L182 | daemon candidate-binary validation before self-reexec shipped (operational-trust geodesic top priority): when daemon detects stale launch_path, validates the new binary BEFORE `execv` by spawning it with `--help` and matching the specific `Schema-driven store framework` Clap-about marker (T075 r3 tightened from prior loose `OR contains "stores"` fallback that would've accepted any third-party binary mentioning the word `stores`). Bounded validation timeout (1500ms with SIGKILL on overrun via process group). `CandidateValidationFailure` carries typed `exit_status` field (numeric / `timeout` / `spawn_failed`) for diagnostics. Spawn-error path mapped explicitly. Missing launch-path test pins early-fail behavior at startup-identity-guard (stronger fail-loud than the validation-step rejection). 14/14 daemon_stale_exe tests pass. **First production proof of T075's own contract** — when T075's accept-merge ran cargo-install, the daemon detected its own staleness, validated the freshly-installed `~/.cargo/bin/stores` (which contained the validation logic itself), and self-reexec'd successfully. Codex r3 → r4: 1 HIGH (marker looseness) + 1 LOW (missing-candidate test); r4 PASS at fc260a3. Merge `da1d347`. |
| 2026-05-07 | T063 | L135 | Check primitive shipped (P1 narrow slice): `Check` trait + compile-time registry in `src/flow/checks.rs` (id + typed args + evaluator + structured `CheckResult { check_id, args, observed_at, outcome }`). Two existing ad-hoc check sites adapted: (a) L134/T050 dispatch postconditions (drive_pid_recorded_or_terminal et al — still write `postcondition_id` + `postcondition_args` to `dispatch_locks`, now produced by Check registry); (b) T053 gatekeeper_decision_json validator (route via Check registry; failure preserves structured `CheckResult` JSON in error message via `format_check_failure` helper alongside legacy diagnostic text). docs/primitives.md gets short Check entry distinguishing it from schema validators. Codex 2 rounds: R1 caught gatekeeper failure flattened to stringly + CheckOutcome JSON shape mismatch; R2 PASS after `format_check_failure` helper preserved structure + flat outcome shape documented as intentional. 1008 lib tests + dispatch_locks_typed_regression + gatekeeper_decision_validator + intake_routing_e2e all green. Merge `1a105ac`. **First proof of T062's stale detection in production:** the post-T062 daemon caught its own stale exe after T063's cargo-install and exited fail-loud with the documented operator message — exactly as L149's contract specified. |
| 2026-05-07 | T062 | L149 | daemon stale-exe detection + fail-loud first ship: dev/ino identity recorded at startup, tight pre-spawn guard before each `run_dispatch` for `builtin:auto-drive` (TOCTOU-tight; codex round 1 caught the gap), centralized one-shot fail-loud via `STALE_HALTED`+`Err(STALE_DAEMON_MESSAGE)` (codex round 1 also caught per-candidate dedup gap), real fixture-replace integration test using `fs::rename` of a copied executable (codex round 1 minor — env-var force-stale fallback wasn't enough). 5 daemon_stale_exe integration tests + 1004 lib tests pass. Codex 2 rounds. Self-reexec deferred to follow-up. Operator note added to `docs/CLAUDE.md`. Merge `4f7484f`. **First-ship caveat:** the daemon running pre-T062 didn't detect its own ceremony's stale state (chicken-and-egg); from the post-install daemon onward, every cargo-install fires the fail-loud event. |
| 2026-05-07 | T043 | L124 / L002 | `tasks abandon <id> --reason <text>` verb + `abandoned` terminal state shipped: tier-A token-mediated (actor: human; --invoker human OR --invoker ai_with_human + --approve-token; rejects ai_autonomous even with valid token); allowed from 9 non-terminal states (planning/plan_review/ready/executing/code_review/blocked/in_review/deploy_blocked/complete); refused from 5 terminal (accepted/rejected/cargo_installed/schema_migrated/closed_out_of_band); idempotent on already-abandoned (no-op success, reason not overwritten, audit count = 1); reason required (whitespace-trimmed); transition_history captures verb+invoker+reason. Watch/TUI: abandoned shown in terminal/history bucket, NOT in_flight. Doctrine doc clarifies 3-way distinction (rejected = reviewed-and-rejected; abandoned = intentionally-retired; closed_out_of_band = shipped-via-manual-commit). Framework fields (abandoned_reason/abandoned_at, plus claimed_by/drive_pid etc.) no longer surface on generic add/update CLI. Codex (2 rounds) caught: `complete` missing from allowed set + framework-field CLI surface. Originally driven earlier today as T043 PASS-then-rejected for "ship close-out-of-band first" sequencing; amended after T044/T053/T061 cleared the preconditions. Merge `3e42cf5` — first row to actually exercise schema-migrate's column-add path (2 cols applied). |
| 2026-05-07 | T061 | L145 | retry-deploy recovery edge for `deploy_blocked` shipped: new `tasks retry-deploy <id>` verb (deploy_blocked → accepted) re-fires accept-merge → cargo-install → schema-migrate without re-cycling planner/executor/code-reviewer. accept_merge gained a peer-detection guard (skips firing `mark_cargo_installed` when a cargo-install peer is on the same edge — preserves L045 solo-recovery contract via no-peer fallback). cargo_install gained an L045-symmetric stale-workspace cwd fallback that fails LOUDLY if cwd isn't the stores Cargo crate (Cargo.toml package-name validation). compute_resume now rejects deploy_blocked rows with operator guidance. Codex (5 rounds) caught: subscriber-chain ordering bug → resilience completion gap → cwd-validation laxness → TOML parser inline-comment intolerance. All four findings landed via subagent revise + one direct parser tweak. Merge `d41b6fd`. |
| 2026-05-07 | T053 | L142 | gatekeeper Router seam P1 shipped: `intake_items` typed store with five-state lifecycle (draft/triaging/needs_info/routed/dropped); single `route` verb covering all six decisions (duplicate/needs_info/fast_track/normal_observation/arch_review_candidate/reject_noise) with same-tx side effects; structured gatekeeper_decision_json validator; `arch_review_candidate` produces a tagged-observation stand-in (no dedicated `architecture_reviews` store); `SideEffectAuthority::GatekeeperRoute` typed enum for narrow per-callsite framework authority on observation L143 fields; direct `observations add` escape hatch preserved; recon-return ≤2-cap. Codex (3 rounds) caught 2 critical (escalated lifecycle violation + missing route-validation), 2 major (CLI/JSON decision equality + side-effect actor discipline), then doc-stragglers + cosmetic typo. Drive economy: 4 cycles all-Pi from blank slate failed (commit='none'); switched to Sonnet executor, landed phases 1-2 + most of 3 before rate limit; resumed all-Pi from concrete 812-LOC scaffold and completed 3-5 in 5 cycles; codex-revise 4 substantive fixes via subagent. Merge `ebfaaf2`. First real intake row: `I001` (FIX 5 follow-up). |
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
