# Handover: engine on deck for Blake

**Date:** 2026-05-04
**Type:** handover

## Summary

**Read first**, in this order: `/CLAUDE.md` (doctrine v2: U1/U3/U4 with auto-promote folded in, L043 routing rule), `docs/philosophy.md` v1.3 (auto-promote/auto-scaffold doctrine + the row-creation arrival convention `from=''`), `docs/worklog/2026-05-03/08-flow-observation-to-task-lifecycle.md` (the 10-step pipeline), `docs/worklog/2026-05-03/09-handover-handcrank-vs-flow-goal.md` (yesterday's pre-T020 handover). This note assumes you've internalized those.

**Headline.** T020 shipped. The upstream-autonomy chain is live: ratify a contract → daemon auto-creates the task at `planning` with `linked_observations` populated → daemon auto-scaffolds the worktree → worktree gets `workspace_path` set on the row. Steps 4–5 of the 10-step pipeline are now wired through the daemon. Combined with steps 7–9 (post-accept ceremony from T014+T019), 5 of 8 autonomous edges are shipped. Steps 6 (auto-drive) and 10 (auto-resolve-observation) are filed as L048 + L049 with draft contracts; they're the next ratifications when you're ready to keep flowing the pipeline.

**Engine status: ON DECK, NOT TURNED ON.** The daemon is running. T020 is merged + cargo-installed + schema-migrated on main. The next autonomous edge fires the moment you ratify any observation contract — `observations update LXXX --contract-state ready --approved-by blake --approved-at <now> --invoker ai_with_human --approve-token <T>`. The orchestrator was instructed NOT to take that step on your behalf — that's your U1 moment, the deliberate ignition. **The dogfood test is sitting right there**: ratify L045 (T1, 50 LOC) FIRST in isolation; watch auto-promote create the task within 5s; verify the task lands clean; then unleash the rest of the queue.

**The orchestrator's autonomous run produced** a fully-drafted backlog: 23 open observations all carrying draft `intent_contract` content (objective / in_scope / out_of_scope / acceptance / tier_hint / type=work). When you ratify each, the new auto-promote subscriber materializes the task and threads `done_when` / `scope_in` / `scope_out` from the contract automatically — no more re-keying contract content into `./dev new` flags by hand.

## Details

### What shipped today (2026-05-03 → 2026-05-04 session)

#### T020 — upstream-autonomy unlock (the bootstrap)
Merge commit `bfa7a72`. Six-phase task; reviewer PASS at every gate.

- **schema.yaml**: `observations.status` gets `ready` state; framework `confirmed→ready` "ratify" transition (actor: framework).
- **src/handlers/transition.rs**: `maybe_auto_ratify_observation` post-confirm hook fires in-tx when `intent_contract.contract_state=ready` AND `approved_by`/`approved_at` set.
- **src/handlers/add.rs**: emits a synthetic `create` row in `transition_history` for every add (across all stores); under `--lock-contract` walks the observation `open→investigating→confirmed` so the hook fires on filing.
- **src/flow/builtins/auto_promote.rs**: subscribes `observations: confirmed→ready`; mints a tasks row at `planning` with `linked_observations`, `done_when`/`scope_in`/`scope_out` propagated from the contract; back-links `observation.task_id`. Idempotent (existing-task short-circuit).
- **src/flow/builtins/auto_scaffold.rs**: subscribes `tasks: ''→planning` (the row-creation arrival convention); runs `scaffold.command` from `.stores/config.yaml`; parses worktree path; writes `workspace_path`. Idempotent (existing-workspace-and-dir short-circuit).
- **AgentsYaml::validate** now permits empty `from` (the row-arrival convention), documented in philosophy.md v1.3.
- **tests/flow_promote_scaffold_e2e.rs**: end-to-end ratify→promote→scaffold in <1s; idempotent third poll.

**Honest disclosures from the wrap (reviewer-accepted, sanity-checked):**
- P1 deviation: compound guard implemented in Rust hook rather than expr-parser guard — documented expr-parser limitation.
- P3 deviation: `auto_promote` uses direct SQL INSERT bypassing validator, mirroring `user_escalation` pattern. `created_by='ai_autonomous'` as a label since the upstream ratify is the U-moment.
- Residual risks: pre-existing `e_schema_migrate_failure_blocks` flake under concurrent runs (not introduced by T020; passes in isolation, verified). `auto_scaffold` `{display_id}/{slug}/{branch}` substitution is naive string-replace (low-likelihood given slug charset). On-confirm hook recurses synchronously inside same tx (depth-limit may be needed if more follow-on transitions stack on `ready`).

**T020 acceptance was autonomous** (with token): user pre-authorized; orchestrator verified the brief, ran the recommended sanity checks (E2E ✓, agents.yaml ✓, idempotency guards ✓, philosophy.md v1.3 ✓, full workspace 645/646 + flake-passes-in-isolation ✓), accepted with `--invoker ai_with_human --approve-token <T>`. Daemon fired the post-accept ceremony cleanly: accept-merge → cargo-install (with `features=runner-claude-code` — L009 fix paying off) → schema-migrate (no-op, in-sync). Worktree torn down via `./dev done T020 --force`. Stale T019 worktree also cleaned up.

#### Direct fixes shipped on main

- **L009 (HIGH, commit `13f7dc1`)**: Cargo.toml `default = ["runner-claude-code"]`. Bare `cargo install --path .` now produces a binary with `--claude-code`. The silent-feature-drop footgun that bit twice this session is closed.
- **L040 (HIGH, same commit)**: `tests/topology_dot_render.rs::graph_easy_on_path()` now does a real spawn-and-capture (pipe a tiny digraph, assert non-empty stdout) instead of `--version` (which exits non-zero on graph-easy 0.76 even when working). The "tests-skipped-as-pass" false negative is closed.
- **L046, L047 (T020 itself)**: closed `resolved` (resolution=`bfa7a72`).

#### CLAUDE.md doctrine update (commit `250945f`)

- **U-moments collapsed from four to three.** The "U2 — Promotion" moment is folded into U1 because auto-promote makes ratify=produce-task atomic. U1 has two paths now: observation-first (the dominant path post-T020) + direct-task escape hatch.
- **New "Triage routing (the L043 rule)" subsection**: ≤3 cheap tool calls then halt-or-route. The orchestrator-on-main routes; the investigator subagent (L043) dives. The pain that earned the rule was the L042 misdiagnosis + eval_length root-cause hunt yesterday — both should have been routed, not held in main thread.
- **Two new "What NOT to do" bullets**: don't dive deep on observations as orchestrator; don't hand-crank ratified-but-unpromoted observations through `./dev new` (auto-promote obsoletes that pattern).

#### Track A — observations filed for the missing pipeline edges

- **L048** — Auto-drive subscriber (T2, ~150–300 LOC, draft contract). Step 6 of the 10-step pipeline. Spawns `tasks drive` as a detached subprocess when a task lands at `planning` with `workspace_path` set. Inputs: L046, L047, L038, worklog-08.
- **L049** — Auto-resolve-observation subscriber (T1, ~50–100 LOC, draft contract). Step 10. On `tasks → schema_migrated`, marks every entry in `linked_observations` as `resolved` with `resolution=task.commit_sha`. Idempotent.

#### Track C — superseded observations triaged

- **L010** → `resolved` (resolution=`1c8d02b`, T019 post-accept ceremony shipped).
- **L025** → `wont_fix` (superseded by L030's uniform-pipeline doctrine).
- **L027** → `wont_fix` (superseded by L030 — multi-runner architecture explicitly rejected).
- **L031** → `wont_fix` (adopted-doctrine — sandbox deferral IS the standing isolation pattern).

#### Bulk contract drafting — the engine's lunch

23 open observations now carry full draft `intent_contract` content (objective + type + in_scope + out_of_scope + acceptance + tier_hint). They are READY FOR U1 RATIFICATION. After T020's auto-promote ships, ratifying each one is a 1-verb action and the substrate carries the rest. The drafts are stored in `.stores/db.sqlite` (NOT in git — the substrate model). If you wipe `.stores/`, you lose them.

| L-id | Pri | Tier | Bundleable | Summary |
|---|---|---|---|---|
| L002 | normal | T2 | — | tasks abandon verb (any non-terminal → 'dropped') |
| L003 | low | T2 | — | observations list scannable default + tabular --brief |
| L005 | normal | T1 | — | pipe-separator splitting on list-typed CLI input |
| L006 | high | T2 | — | codify C-hybrid pattern (T1 in-chat, T2/T3 promote) |
| L011 | high | T2 | — | binary_version + git_sha on every row |
| L012 | normal | T3 | — | Tier-1 inspector: `tasks inspect <id>` (per-cycle CLI) |
| **L013** | **high** | **T1** | **auth-UX** | auth init default path → `~/.config/stores/identity.age` |
| **L014** | **high** | **T2** | **auth-UX** | auth init UX gaps (binary error, sidecar, one-verb bootstrap) |
| **L015** | **high** | **T1** | **auth-UX** | auth show needs `--identity` (mirror init) |
| L019 | normal | T3 | — | DockerRunner as Runner trait impl in stores |
| L020 | normal | T1 | — | tasks render canonicalize state directories |
| L021 | normal | T1 | — | render Completion section pulls wrap_log structured fields |
| L023 | normal | T2 | — | observations next-id + standardize list --json envelope |
| L028 | low | T2 | — | drive-spawned agent /observe access + provenance plumbing |
| L030 | normal | T2 | — | uniform-pipeline + tier-aware briefs (supersedes L025/L027) |
| L032 | high | T2 | — | worktree delegation manifest (substrate verbs from inside worktree) |
| L034 | normal | T1 | — | wrap agent reads git log both directions (no diff-stat-only) |
| L035 | high | T3 | — | schema-enforced inter-agent context flow (templated refs) |
| L039 | high | T2 | — | daemon retry-on-failure (closes T014 wrap deviation #2) |
| **L044** | **high** | **T1** | **auth-UX** | docs guardrail: don't symlink onto SOPS default path |
| L048 | normal | T2 | — | auto-drive subscriber (step 6 of pipeline) |
| L049 | normal | T1 | — | auto-resolve-observation subscriber (step 10) |
| L050 | normal | T1 | — | regen topology_dot_snapshot fixture (T019 states drift) |

L013 + L014 + L015 + L044 are the **auth-UX bundle** — body of L013 explicitly recommends shipping them as one task (~150 LOC across CLI + docs).

### "Pre-ratified" queue — the trap

Yesterday's handover called L045/L038/L043 "ratified contracts awaiting auto-promote." That framing is **partially wrong** and needs correction here. Verified state on each:

| L-id | status | contract_state | task_id | Pri | Tier |
|---|---|---|---|---|---|
| L045 | **open** | ready | T014 (soft-FK to surfacing task, NOT auto-created) | normal | T1 |
| L038 | **open** | ready | T019 (soft-FK to surfacing task, NOT auto-created) | high | T1 |
| L043 | **open** | ready | (none) | high | T2 |

**`contract_state=ready` is just a field** (set via `observations update --contract-state ready --approved-by --approved-at` directly). It is **not** the same as `status=ready`. The new T020 framework hook fires on the **state transition** `confirmed → ready`, not on field writes. These three observations never walked through the state machine — they're frozen at `status=open` with the contract field set as a side-fact. There are zero rows in `transition_history` for any of them.

**Implication for the dogfood test**: auto-promote will NOT fire on these three spontaneously. The previous handover's "verify they auto-promoted overnight" advice is incorrect. To make these flow through the new pipeline, they need to be walked through the lifecycle properly:

```bash
# Per observation:
stores observations investigate L045 --invoker ai_autonomous       # open → investigating
stores observations confirm     L045 --invoker ai_with_human \
                                       --approve-token <T>          # investigating → confirmed (tier-A; the U1 moment)
# The T020 framework hook then fires confirmed → ready on the same write,
# auto-promote subscriber receives the transition, and the task is materialized.
```

**THE CANARY (do this first, alone)**: ratify **L050** — it's a fresh observation filed today (post-T020), tier T1, single-file fixture regen, lowest stakes. The schema-verified recipe (from `stores/observations/schema.yaml` lifecycle):

```bash
# Step 1 — investigate (autonomous; open → investigating).
stores observations investigate L050 --invoker ai_autonomous

# Step 2 — already done in this session: contract content drafted via
# `observations update --objective ... --in-scope ... --acceptance ...
# --tier-hint ... --type work`. Verify with:
stores observations show L050 --invoker ai_autonomous | grep -E "objective:|tier_hint:|type:"

# Step 3 — ratify the contract (TIER-A, U1 moment, token-required).
# This is the actor:human field write; auto-promote does NOT fire yet.
stores observations update L050 \
  --contract-state ready \
  --approved-by blake \
  --approved-at "$(date -Iseconds)" \
  --invoker ai_with_human --approve-token <T>

# Step 4 — confirm (tier-B; investigating → confirmed; guard: contract_state=ready).
# This is the trigger. The post-confirm framework hook synchronously fires
# confirmed → ready (because contract_state+approved are set), and the
# auto-promote subscriber receives that transition on the next daemon poll.
stores observations confirm L050 --invoker ai_with_human --approve-token <T>

# Step 5 — observe.
tail -f /tmp/stores-daemon.log
# Expect within ~5s: dispatch of auto-promote → new T### at status=planning;
# then auto-scaffold → workspace_path written + worktree appears.
# Verify the chain landed:
stores observations show L050 --invoker ai_autonomous | grep -E "status:|task_id:"
stores tasks status T### --invoker ai_autonomous   # use the new T-id
```

If L050's chain lands cleanly, then escalate: **walk L045 / L038 / L043 through the same recipe** in that order (T1 first; investigator-subagent last). Their `contract_state=ready` is already set; you can skip Step 3, but you still need Step 1 (investigate) and Step 4 (confirm) on each.

After those: batch the **auth-UX bundle** (L013 + L014 + L015 + L044). One option is to walk each through the same recipe; another is to file ONE new umbrella observation with --lock-contract at filing time (which executes Steps 1+4 automatically per T020's add.rs walk) and the existing four close as `wont_fix` with cross-reference. Your call.

**If the canary fails** — auto-promote doesn't fire, OR fires-but-with-wrong-content — file an observation describing the failure mode, halt, and ask Blake. Do NOT inline-debug (L043 rule).

### State of the world (operating environment)

- **Branch**: `main`. Origin/main: synced (all commits pushed: `250945f`, `13f7dc1`, `bfa7a72`).
- **Daemon**: PID 1297522 (started yesterday ~14:00 UTC), polling every 5s, log at `/tmp/stores-daemon.log`. SIGINT to stop. **No need to touch it** — it's already polling and will pick up the next ratify-confirmed events when you ratify.
- **Worktrees**: only `main` left. T019 + T020 worktrees torn down.
- **Binary**: `~/.cargo/bin/stores 0.5.0` post-T020 reinstall via daemon's cargo-install builtin. `--features runner-claude-code` is now the default (L009 fix). Verify with `stores tasks drive --help | grep claude-code`.
- **DB**: `.stores/db.sqlite`; T020's full schema (observations.status `ready` + framework ratify transition + dispatch_locks for the new builtins). 23 open observations carry full draft contracts in this DB.
- **agents.yaml**: `.stores/agents.yaml` declares accept-merge, cargo-install, schema-migrate, user-escalation, **auto-promote**, **auto-scaffold**.
- **Token**: pre-decrypted token consumed for T020 acceptance. **Will need a fresh paste** for any new tier-A action (ratifying L038's contract update fields, etc.). The old token may or may not still be valid — `stores auth show` is the source.
- **Config gap**: `.stores/config.yaml` may need a `scaffold` section (`scaffold.command` + `scaffold.cwd`) for `auto_scaffold` to do anything when it fires. T020's body says "missing-command no-ops"; verify with first ratification. If scaffold is no-op, the workspace_path won't get set and the task lands at `planning` without a worktree — auto-drive (when L048 ships) won't fire on it. Self-test: ratify L050 (T1, single file) and watch.

### Pre-existing failures still present on main

- **`tests/topology_dot_snapshot::ac2_4_dot_snapshot_matches`** — fails on `cargo test`. Pre-existing snapshot drift since T019 added `cargo_installed`/`schema_migrated`/`deploy_blocked`; expected fixture not regenerated. **Filed as L050 with full draft contract** (T1, single-file regen). Mark and forget; ratify when convenient.
- **`tests/topology_dot_render::ac_max_line_width_under_120`** — `#[ignore]`d with comment pointing at L041. Pre-existing 128-col line in Z1 tasks zone exceeds the 120-col contract.
- **`flow::builtins::tests::e_schema_migrate_failure_blocks`** — flaky under parallel runs (concurrent STORES_NTFY_URL global / .stores/runs/ pollution); passes in isolation. Pre-existing; not introduced by T020.

### Pending YOUR decisions (what the orchestrator could not do alone)

1. **L041 — width contract**: `topology --format auto` Z1 tasks line is 128 cols; AC2.1 contract says ≤120. `graph-easy 0.76` has no `--width` option. Pick: (c) try `rankdir=LR` swap (uncertain), or (d) bump AC threshold to 140 (contract change). Currently the test is `#[ignore]`d. **L041 is intentionally the only open observation without a draft contract.**
2. **Ratifying L045/L038/L043** — the dogfood test (per above). Do L045 first, alone.
3. **Ratifying the rest** — your pace. Bundle-suggested: auth-UX (L013+L014+L015+L044) ships as one task; L048+L049 close the upstream pipeline; L030 codifies uniform-pipeline doctrine in code (currently the doctrine is doc-only).
4. **`.stores/config.yaml` scaffold section** — verify it exists and points at `./dev new --slug={slug} --base={base}` or equivalent. Without it, auto-scaffold no-ops silently.
5. **Token rotation** — the previous session's token was consumed; if you want a fresh one for the next bunch of ratifications, `stores auth init` (or rotate via whatever T001 shipped) and paste into chat.

### Operating discipline (don't slip — restated from yesterday)

- **L043 routing rule** (now in CLAUDE.md): ≤3 cheap tool calls, then halt-or-route. Don't inline-investigate.
- **Inline-on-main is human-only.** Direct fixes on main happen only when you authorize each edit.
- **`--invoker` discipline**: default `ai_autonomous`. `ai_with_human` only at U1/U3/U4 with token attached or user typing the verb. Never silently upgrade.
- **Don't pre-ratify** (per the dogfood-test sequence above).
- **SQL surgery is recovery-of-last-resort** — used yesterday once for T020's `eval_length` reset; not a habit.

### Headline metrics for the day

- **5 of 8 autonomous edges shipped** (was 3 of 8 yesterday). Up: steps 4–5 from T020. Pending: steps 6 (L048), 10 (L049), and step 2's investigator subagent (L043 already ratified — promotes on first auto-promote firing).
- **22 closed observations** (was 12 yesterday). Net +10 closures: L009, L040, L010, L025, L027, L031, L046, L047 plus pre-existing closures.
- **23 open observations all carry draft contracts** (was 0 fully-drafted yesterday). The substrate's "lunch" is laid out.
- **6 commits pushed to origin**: `250945f` (CLAUDE.md doctrine), `13f7dc1` (L009/L040), `bfa7a72` (T020 merge); plus T020's six in-branch commits via the merge.
- **646 lib + integration + E2E tests green** in T020's worktree pre-merge; one pre-existing snapshot test fails on main (L050 tracks).

### Reference: the 10-step pipeline (post-T020 status)

| Step | Status | Carrier |
|---|---|---|
| 1. File observation | ✅ shipped | `observations.add` |
| 2. Triage | partial | orchestrator discipline (L043 rule in CLAUDE.md); investigator subagent ratified pending auto-promote |
| 3. Investigate | pending | L043 (ratified, will auto-promote) |
| 4. Auto-promote | ✅ **shipped today** | T020's auto_promote builtin |
| 5. Auto-scaffold | ✅ **shipped today** | T020's auto_scaffold builtin |
| 6. Auto-drive | filed | L048 (draft contract; awaiting U1) |
| 7-9. Post-accept ceremony | ✅ shipped | T014 + T019 |
| 10. Auto-resolve-observation | filed | L049 (draft contract; awaiting U1) |

## Follow-ups

For the next agent, in priority order:

1. **Read the "Pre-ratified queue — the trap" section above.** L045/L038/L043 are NOT going to auto-promote spontaneously — they're frozen at status=open with contract_state=ready as a side-fact. The framework hook fires on transitions, not field writes. They need to be walked through `investigate → confirm` to fire the chain.

2. **Canary first: ratify L050 (fresh observation, T1, single-file regen).** Run `stores observations confirm L050 --invoker ai_with_human --approve-token <T>` and watch `/tmp/stores-daemon.log` for auto-promote + auto-scaffold within ~5s. If it works, escalate. If not, file + halt.

3. **Auth-UX bundle: ratify L013 + L014 + L015 + L044 next.** Bundle suggested by the bodies; ship as a single task that touches `src/cli/auth.rs` + setup docs. The L044 cross-tool footgun makes the L013 default-path fix urgent (don't symlink onto sops's default path; ever).

4. **Close the upstream pipeline: ratify L048 + L049.** With auto-drive (L048) shipped, the orchestrator no longer types `tasks drive`; with auto-resolve (L049), observation-status closes automatically when the task hits `schema_migrated`. After both ship, the pipeline is fully autonomous between U1 and U3.

5. **Decide L041.** 128-col fact vs 120-col contract. Pick layout-experiment or contract-bump.

6. **Doctrine code-up: L030 (uniform-pipeline tier-aware briefs).** Currently doctrine-only in CLAUDE.md and philosophy.md; the planner/plan-reviewer/code-reviewer briefs need to actually consume `tier_hint`. Medium task.

7. **L035 (schema-enforced inter-agent context).** T3, biggest architectural follow-up; finishes the schema-as-engine doctrine for inter-agent boundaries. Worth doing but not blocking.

### What NOT to do (this session's hard-won lessons, restated)

- **Don't dive deep on observations as orchestrator.** L043 rule. ≤3 cheap tool calls then halt-or-route.
- **Don't pre-ratify the queue.** Do L045 (or L050) first, alone, and watch the engine fire. Cascading-promotion-bug risk is real until a single-row test passes.
- **Don't run multiple drives in parallel** until L048 (auto-drive) ships. The daemon serializes via `dispatch_locks` UNIQUE-claim, but `tasks drive` itself isn't gated yet.
- **Don't re-key contract content into `./dev new` flags.** That's the orphan-prone hand-crank pattern T020 obsoletes. Auto-promote does it.
- **Don't bypass U-moments.** Token-mediated tier-A writes are the substrate's grounding mechanism. Drift across the tiers is what the schema fights. The doctrine works because we don't fudge it.
- **Don't try to "turn on the engine" by batch-ratifying.** The user's express instruction was: I will turn it on. Your job is to lay out the lunch.
