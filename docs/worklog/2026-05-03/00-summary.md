# Daily Summary — 2026-05-03

## Overview

The day the substrate started building itself end-to-end. Across nine notes (likely multiple sessions), the dogfood doctrine was clarified, the token mechanism + per-role models + `./dev` worktree script bootstrapped, observability surfaces shipped (`stores watch` + `stores topology`), and the autonomous-flow engine (T014) plus its post-accept ceremony (T019) landed. By end of day the project was ~75% to autonomous-propulsion GO; T020 (auto-promote + auto-scaffold) was in flight as the bootstrap-only hand-crank that would obsolete the orchestrator's hand-cranking forever.

The throughline of the day was Blake's reframe (note 01): "I am not a flight controller telling pilots where to fly. I am merely an observer watching an engine run itself." The operational translation — autonomous moments do not ask, U-moments halt and propose — reshaped the rest of the session and surfaced the first 13 friction observations within hours.

## Work Completed

**Substrate tasks shipped (12 + ceremonies):**

- **T001** — approval-token mechanism (encrypted-at-rest age token + sha256 sidecar + constant-time verify; tier-A/tier-B doctrine codified in CLAUDE.md). Hot-fix `82501d3` shipped inline during deploy when L016 (random-bytes ≠ UTF-8) surfaced.
- **T002** — per-role model config (planner=opus, plan_reviewer=opus, executor=sonnet, code_reviewer=opus, wrap=opus); `(model=<m>)` printed on every spawn line.
- **T003** — `./dev` worktree script (`./dev new`, `./dev done`); merge-recovered into main as `8f6d451` after handover overstated "all merged."
- **T004** — observations close-from-open verb (T1; closes L017's lifecycle papercut).
- **T005** — `stores topology` static three-zone schematic; `--format dot|mermaid|auto`; auto-mode renders in-terminal via `graph-easy --as=boxart`.
- **T013** — filing primitives: `intent_contract.*` settable at `observations.add`; `--lock-contract`; `tasks.tier_hint` enum field with inheritance from linked observations.
- **T014** — autonomous flow engine: `agents.yaml` + `policies.yaml` parsers, predicate evaluator (default-ALLOW, NEVER-sacrosanct, SHA-256 audit), `stores agents run` daemon (5s polling, SQLite UNIQUE-claim idempotency, SIGTERM graceful shutdown), `accept-merge` + `user-escalation` builtins, `deploy_blocked` state, `transition_history` audit table, `stores agents backfill` one-off.
- **T015** — watch dashboard upgraded with phase-box / cycle-dot Design A glyphs (`▰▮◐▱` + `●·`).
- **T016** — plan-reviewer brief renders the planner's `decision_matrix`.
- **T017** — `stores migrate` verb (additive-only schema sync; dry-run default; `--apply` mutates inside a transaction; idempotent).
- **T018** — topology zones rendered separately (after 876-col blow-up); regenerated snapshot fixtures.
- **T019** — post-accept ceremony: `builtin:cargo-install` + `builtin:schema-migrate` chained after `builtin:accept-merge`. Shipped during note 07's discussion.

**Direct fixes on main:**

- **L016** hot-fix `82501d3` — hex-encode token bytes (random ≠ UTF-8); fixtures with `"test-secret-..."` shape had hidden the bug.
- **L036** fix `cdabd54` — `dot -Tutf8` (no such graphviz format) → `graph-easy --as=boxart`; reason-aware fallback.
- **L042 closed** as misdiagnosis (`f3ddc21`) — "run-log capture broken" was actually four runner tests inheriting manifest dir as cwd; `STORES_RUNS_DIR` env override + redirect to `target/test-runs/`. 135 leaked files cleaned. Real run-logs were always healthy.
- **eval_length Null bug** fixed inline (`476144a`) — `tasks add`'s generic insert path didn't initialize `plan_review_log` to `'[]'`; surfaced when T020's first plan-review NEEDS_WORK landed in `blocked` instead of `planning`. 1-line fix + regression test.
- **philosophy.md v1.2** shipped (`e44f795`) — substrate vs deployment system distinction (subscribers project-declared); failure recovery as ordinary-task pattern; "Pull from real use" doctrine.

**Repo published:** master renamed to main; pushed to private `github.com/blakesims/stores`; default branch updated; stale remote `master` deleted; 57 commits fast-forwarded.

**Doctrine clarifications captured (7):** two-gate model (front gate = contract lock, back gate = task accept); drafted contracts at filing (L029); uniform task-branch dispatch (L030 — tier modulates brief content, not pipeline shape); sandbox deferral (L031 — worktree + `permissions.deny` is the standing isolation pattern); default-allow policy; schema-enforced context flow (L035, T3, deepest); executor scope intentional (narrow code-writer; fix lives in plan-reviewer brief plumbing + L035).

**Friction observations filed (~30 across L007–L050):** L007–L019 in note 01; L020/L021 mid-handover audits; L022 (policy-pre-authorization); L023–L038 in note 04 (most surfaced live during real work, several closed same-day via T013/T016/T017); L036/L040/L041 in note 05; L042–L047 mid-T020 (L042 closed as misdiagnosis; L045/L046/L047 named the engine's first real finds); L048/L049 unfiled at end-of-day as Track A targets.

## Notes Today

| # | Note | Topic |
|---|------|-------|
| 01 | [dogfood-recursion-first-session.md](./01-dogfood-recursion-first-session.md) | First session where stores was used to build itself. Three chapters: doctrine reframe → infrastructure for self-drive → first deploy + L016 hotfix. T001/T002/T003 accepted; 13 obs filed; repo published. |
| 02 | [autonomous-flow-foundation-handover.md](./02-autonomous-flow-foundation-handover.md) | End-of-session design discussion on what un-clogs the substrate. 5-layer blockage analysis; Layer 2 = policy-based pre-authorization (`policies.yaml`); proposed unified "Autonomous Flow Foundation" task bundling L018+L022+L017. (Note: L022 was unfiled at write-time; observation IDs L020/L021 had been claimed by mid-handover audits.) |
| 03 | [stores-watch-poc-and-topology-discussion.md](./03-stores-watch-poc-and-topology-discussion.md) | `stores watch` POC (~240 LOC, ANSI-only, polls SQLite read-only). Topology question explored as A (static schema topology) + B (live count panel) + C (transition stream). Recommended sequencing; promote to substrate task only after C lands. |
| 04 | [dogfood-engine-day-three.md](./04-dogfood-engine-day-three.md) | Engine started building itself: T013/T016/T017 shipped clean; T014 in flight at P7/7. 16 new observations. Doctrine resolved: two-gate model, drafted contracts at filing, uniform task-branch, default-ALLOW, schema-enforced context flow. ~50% to GO. |
| 05 | [topology-and-watch-shipped-with-residual-friction.md](./05-topology-and-watch-shipped-with-residual-friction.md) | T005/T015/T018 shipped autonomously. L036 fixed. Surfaced "tests-skipped-as-passes" pattern (L036, L040 — silent skips reported as ok). Wrap-agent attribution drift (L034) caught at U3. |
| 06 | [take-off-handover.md](./06-take-off-handover.md) | ~75% to GO. T019 in flight P1/5. Lays out the GO checklist (accept T019 → merge → cargo install → migrate → fix L042 regression → start daemon). Operating cheat-sheet + landmines + doctrine restate. (Premise on L042 was later corrected by note 09 as misdiagnosis.) |
| 07 | [stores-to-10-06-design-discussion.md](./07-stores-to-10-06-design-discussion.md) | Mid-session realistic-pull session against 10.06. Conceded inline-on-main is human-only (every `ai_autonomous` task gets its own branch). 10.06 leaks identified: cargo-install too narrow, no gate concept, no pre-merge test gate, no T1-inline-on-main, wrap renders as stub. T019 accepted + merged during this conversation. |
| 08 | [flow-observation-to-task-lifecycle.md](./08-flow-observation-to-task-lifecycle.md) | The 10-step pipeline named: 2 human U-moments, 8 autonomous edges. Today: 3 of 8 wired (steps 7-9). T020 in flight on 4-5. Steps 6, 10 unfiled. 8 design seams; key call: tier modulates brief, not path; one `blocked` state with reason field, not many specific blocked-states. U1+U2 collapse explicit. |
| 09 | [handover-handcrank-vs-flow-goal.md](./09-handover-handcrank-vs-flow-goal.md) | End-of-day handover. Names the orchestrator-on-main hand-crank as the leak T020 obsoletes. Strict instruction: don't pre-ratify the queued contracts (L045/L038/L043) until auto-promote is verified on one observation first. T020 background id `bq7o4r7f4`; daemon PID 1297522. |

## Tensions

- **L022 / policies.yaml status:** note 02 framed it as "next agent's first action — file L022, then promote a unified task bundling L018+L022+L017." Note 04 reports L022 was filed and partially shipped via T013/T014's architecture; default-allow semantics decided ("when daemon sees a transition with no policy match, FLOWS"). **Resolution: later note wins** — L022's intent merged into T014's shipped engine; the unified-bundle proposal in note 02 was superseded.
- **"All branches merged" claim:** note 02 said T002/T003/T010/T011/T012/T013 were "all merged into main but not deleted." Note 04 found T003 unmerged on disk; merged inline as `8f6d451`. **Resolution: note 04 (later, evidence-grounded) wins.**
- **L042 framing:** note 06 listed L042 as a "regression that's BROKEN — every drive is a black box for debugging" requiring a hot-fix before serious propulsion. Note 09 closes L042 as a misdiagnosis (test pollution; real run-logs were always healthy). **Resolution: note 09 wins; 135 leaked files cleaned, no regression.**
- **L045/L038/L043 readiness:** note 09 framed these as "ratified contracts awaiting auto-promote." This was carried forward to 2026-05-04 note 01, which corrected it as **partially wrong**: `contract_state=ready` is a side-field, not a state-machine transition; auto-promote fires on the `confirmed→ready` transition, which never happened on these three. **Resolution: 5-04's correction wins (cross-day); see "Tomorrow" — they need to be walked through investigate→confirm to fire the chain.**

## Open Threads

- **T020 (auto-promote + auto-scaffold)** in flight at end of session. Background id `bq7o4r7f4`. Worktree at `../stores-T020-upstream-autonomy-unlock`.
- **Daemon running**, PID 1297522, polling 5s, log `/tmp/stores-daemon.log`.
- **Pre-ratified-but-unpromoted observations:** L045 (T1, accept-merge stale-worktree), L038 (T1, depends_on enforcement, Layer 1 ~30-50 LOC), L043 (T2, investigator subagent + needs_investigation routing). **DO NOT hand-crank** — wait for auto-promote, eat own dogfood.
- **Auto-drive (step 6) and auto-resolve-observation (step 10) unfiled.** Track A targets — file as observations after T020 lands.
- **L040** (test gate uses `graph-easy --version` exits non-zero — silent skip). Same anti-pattern as L036.
- **L041** (Z1 width 136 cols vs 120 contracted; tests `#[ignore]`d).
- **"tests-skipped-as-passes" pattern** worth a refs doc + substrate-level lint reporting skip count alongside pass count.
- **Wrap-agent attribution drift (L034)** — wrap reads diff stat without `git log <range>`; misattributes commits.
- **Auth-UX cluster (L013/L014/L015 + L016 informational)** — one ~50 LOC bundle; deferred.
- **L030 (uniform-pipeline tier-aware briefs)** — currently doc-only doctrine; planner/code-reviewer briefs need to consume `tier_hint`.
- **L035 (schema-enforced inter-agent context flow)** — T3, biggest architectural follow-up; finishes schema-as-engine doctrine.
- **L032** — worktree has no `.stores/` visibility; substrate verbs fail from inside worktree. Workaround: always run from main.
- **`./dev new` next-id collision** when scaffolding two tasks back-to-back; render between scaffolds as workaround.
- **L038 Layer 1** — small T1 task to make `tasks drive` honor `depends_on`.
- **Empty planner submission** observed once on T019's first attempt (transient SDK flake); retry succeeded.
- **6 commits queued for `origin/main` push** at end of session (eval-length fix, philosophy v1.2, worklogs 07/08/09, L042 cleanup).

## Tomorrow

- **Wait for T020 to wrap.** Background id `bq7o4r7f4`. When wrap lands, surface brief + deviations[] + residual_risks[] honestly. User accepts (token); engine fires accept-merge → cargo-install → schema-migrate.
- **Verify auto-promote on a fresh observation.** File a test obs, ratify its contract, watch daemon log + dispatch_locks. ~5s should auto-create a task. If not — file observation, halt; don't inline-debug (L043 rule).
- **Track A:** file observations for auto-drive (step 6) + auto-resolve-observation (step 10).
- **Track B (doctrine):** revise CLAUDE.md to collapse U1+U2 into "U1 ratify (auto-promotes)"; codify L043 routing rule (≤3 cheap tool calls then halt-or-route).
- **Track C (cleanup):** triage superseded observations — L010 (resolved by T019), L025/L027 (superseded by L030), L031 (deferral as wont_fix).
- **Once auto-promote verified:** ratify L045 → L038 → L043 (T1 first, investigator subagent last).
- **Push to `origin/main`** the queued 6 commits.
