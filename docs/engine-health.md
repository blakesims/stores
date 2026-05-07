# Engine health & shape

A long-standing snapshot of where the substrate engine bleeds, what's filed against each weakness, and what's already shipped. Refreshed by hand at significant inflection points (a batch of fixes lands; a new bug class surfaces; an architectural shift is proposed). For session-by-session detail, see `docs/worklog/`.

**Last updated:** 2026-05-07 (T077/L171 phase α docs refresh) — The engine is now running a three-role operating model: substrate-agent as engine controller, Pi as architect, and reviewer-runner as read-only codex sensor. This improved throughput and surfaced the next trust boundary: shared global binary corruption (`~/.cargo/bin/stores` overwritten by subagent-side installs) can make the CLI fail-silent and can trick T066-style self-reexec into execing a stub. Session SOP now forbids subagents/reviewer-runner from running `cargo install`; filed **L181** (fail-silent CLI) and **L182** (recurring binary corruption). Current stabilization queue: **T067/L178** handoff semantics, **T069/L011** daemon_starts (codex PASS, awaiting accept), **T070/L057** agent_runs telemetry, **T071/L058** metrics, plus next urgent follow-up **candidate-binary validation before self-reexec** (D) and then **private substrate install path** (C). Constitutional direction is captured in **`docs/heart-and-architect.md`**: **L171 phase α shipped via T077** (`architecture_reviews` with interpret/amend split, pending-architecture-review gate, and no typed Heart yet).

## The picture in one sentence

**The engine is productive but still operationally hot: throughput works, but operator trust now depends on hardening shared binary/runtime boundaries and making review/health signals first-class.** The next geodesic is candidate-binary validation before self-reexec, private install-path doctrine, handoff/review automation, concise watch/actionability, then the post-T077 Heart/Architect follow-ups (`architecture_reviews` phase α shipped; no typed Heart store yet).

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
| L150 | ⚪ T2 | halt/deploy-blocked subscriber files merge-conflict-shaped observations for rows that are merely `blocked` by drive failure (e.g. T034 silent-zombie / Pi-smoke failures). Needs typed event/terminal reason before templating operator-facing halt observations. |
| L176 | ✅ T066 | self-reexec follow-up to L149: preserve daemon process by execing fresh binary after stale detection. Candidate-binary validation now lands via T075/L182 — the corrupted-stub-exec failure mode is closed. |
| L181 | ⚪ T2 | `stores` CLI fail-silent: corrupted/stub binary returns exit 0 with empty stdout/stderr, violating substrate trust. |
| L182 | ✅ T075 (D) | candidate-binary validation before self-reexec shipped (T075). The (C) half — private install path — split out as **L184** (open, ratifiable). Session SOP forbids subagent cargo-install. (D) closes the recurring stub-corruption-tricks-self-reexec class; (C)/L184 closes the corruption surface itself. |
| L184 | ⚪ T2 | Private substrate install path: move daemon's runtime-owned binary off `~/.cargo/bin/stores` to a stores-specific location so subagent / operator `cargo install` cannot corrupt the daemon's launch path. (C) follow-up to L182. |
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

### Layer 6 — Auth / security

| obs | state | what hurts |
|---|---|---|
| L013 | ✅ L185/T078 | Superseded by host-bound plaintext `~/.config/stores/approve.token` (0600) plus `approve.token.hash`; `auth init` no longer discovers SOPS/age keys. |
| L014 | ⚪ T2 | `auth init` UX gaps (opaque binary-format error; 7-line shell ritual) |
| L015 | ⚪ T1 | `auth show` missing `--identity` flag (asymmetric with `init`) |
| L044 | ✅ L185/T078 | Superseded by removing the SOPS/age identity path and the L015 symlink workaround; `auth show` now reads plaintext directly. |
| L053 | ⚪ — | tier-A actor check bypass (cross-listed from Layer 2) |

### Layer 7 — Schema / contract substrate

| obs | state | what hurts |
|---|---|---|
| L005 | ⚪ T1 | list-typed fields accept only single-string at update (no JSON-array input) |
| L035 | ⚪ T3 | no schema-enforced inter-agent context refs (typed agents) |
| L019 | ⚪ T3 | no DockerRunner / standardized agent sandboxing |
| I001 | ⚪ T1 | `required_when` parser only supports `field == literal`; needs `IN [...]` membership or `expr OR expr` composition. Surfaced from T053 codex-revise FIX 5 (filed via `stores intake add` — first real-world use of the new gatekeeper Router seam). |

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
| L023 | ⚪ T2 | observations missing `next-id` verb + JSON envelope inconsistency |
| L124 | ✅ T043 | `tasks abandon <id> --reason <text>` verb retires stale/superseded/duplicate/misadd rows to terminal `abandoned` without burning a drive cycle; tier-A token-mediated; idempotent; watch/TUI bucket: terminal-history (not in-flight). Allowed from 9 non-terminal states incl. complete; refused from 5 successful terminals (accepted/rejected/cargo_installed/schema_migrated/closed_out_of_band). |
| L002 | ✅ T043 | `tasks abandon` verb provides non-destructive admin retirement (closed transitively via L124/T043 — abandoned terminal preserves row + audit reason, no .stores wipe needed) |
| L003 | ⚪ T2 | observations list output unscannable for >2 rows |
| L006 | ⚪ T2 | observations runner asymmetry (no drive cycle for obs) |
| L021 | ✅ T058 | render template pulls `wrap_log` into Completion section |
| L034 | ⚪ T1 | wrap misattributes main-ahead commits as 'rides on this branch' |
| L186 | 🟡 T079 | **engine-runner actionability monitor (in flight)** — daemon-side loop that scans substrate-visible rows, writes a heartbeat per iteration, redispatches orphaned existing autonomous edges (e.g. tasks with `next_agent` set + dead drive_pid), and structurally records held reasons. Pi-blessed phase-1 narrow shape: visibility + redispatch only, no new policy semantics for gatekeeper/investigator/reviewer-runner/architecture_reviews; U-moments preserved. Closes the "engine stalls when ec/ec-on-main is silent" pattern Pi diagnosed at this morning's wind-down. |
| I002 | ⚪ — | **code-reviewer / codex grepping `.sqlite` raw bytes treats stale page data as live row state** — burns review cycles on false-positive transitions (T078 c1-c3 burn). Filed via intake `I002`; doctrine fix pending: SOP edit + reviewer validator forbidding raw-byte inspection of sqlite files; substitute `sqlite3 SELECT` or substrate CLI verbs. |
| GAP | — | **auto-investigator subscriber** — fires investigator on `open → needs_investigation` automatically; partial machinery exists (L043 investigator agent) but no subscriber wires it. Engine still can't drain its own input queue. |

## Highest-leverage next picks

Operator-trust layer fully closed (T067 / T070 / T072 / T075 all shipped 2026-05-07 AM). Pipeline 2026-05-07 PM:

**In flight right now:**

1. **T076 / L184 — Private substrate install path** — daemon binary off `~/.cargo/bin/stores` to a private path (`STORES_DAEMON_BIN_PATH` or `~/.local/share/stores/bin/stores`); cargo-install validates/promotes into private; cold-start isolation test. Codex r1 REVISE on `ensure_private_daemon_binary` non-atomic seed — executor-revised to atomic temp+validate+rename + validate-on-shortcut; codex r2 dispatched. Pi: AC1.1 satisfied as written, do not widen (msg_ae95684b/msg_6e449b4e).
2. **T077 / L171 — `architecture_reviews` typed store (phase α)** — Heart/Architect direction. Drive PID alive at phase 6/6.
3. **T078 / L185 — SOPS+age → plaintext+0600** — auth doctrine relaxation. Resumed (U4) after a 3-cycle in-cycle code-reviewer FAIL turned out to be sqlite-raw-byte false positives (see I002 doctrine fix below). Executing.
4. **T079 / L186 — Engine-runner actionability monitor (NEW this session)** — daemon-side actionability loop + heartbeat + orphan re-drive on existing autonomous edges. Pi-blessed phase-1 shape: visibility + redispatch only; no new policy. Auto-driven (drive pid 326953).

**Queued behind in-flight:**

5. **L053 — tier-A actor check bypass** — security/authority gap; should jump to next-pick after T076–T079 land.
6. **I002 — sqlite-raw-byte review false-positive doctrine fix** — file the SOP edit + validator that forbids `grep`/`strings` against `.sqlite` files in code-reviewer/reviewer-runner output.
7. **L172/L173 gatekeeper P4/P5** — deferred follow-ups; surface after L171 phase α lands.
8. **I001 / L005 schema ergonomics** — small schema-language and list-input improvements as filler tasks.

Current picture: operator-trust layer fully closed; the architectural surface (Heart/Architect via T077/L171) and the engine's "drives itself without operator nudge" property (T079/L186) are in flight in the same session. After this PM cluster lands, the dominant remaining bottleneck is auto-investigator / contract-drafter (L151 successor) — the engine's ability to draft contracts on its own input queue.


## Recently shipped

| date | task | obs | what changed |
|---|---|---|---|
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
