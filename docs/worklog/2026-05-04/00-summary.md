# Daily Summary — 2026-05-04

## Overview

T020 shipped (merge `bfa7a72`); the upstream-autonomy chain is live. Steps 4–5 of the 10-step pipeline are now wired through the daemon; combined with steps 7–9 (T014+T019), **5 of 8 autonomous edges are shipped**. The engine is "**ON DECK, NOT TURNED ON**" — the orchestrator was instructed not to ratify on Blake's behalf. The canary observation (L050, T1, single-file fixture regen) sits ready as the deliberate ignition.

The day's two long-form notes pulled in opposite-but-complementary directions: a realistic-pull friction analysis against the 10.06 client surfaced **6 substrate-shaping moves** (most importantly `touches_files` as a write-time conflict guard); a deeper primitives + engine-metaphor session named the substrate's six primitives we have words for, six we don't, and the architectural unlock to dissolve the drive cycle into transition-specialized subscribers in `agents.yaml`.

## Work Completed

- **T020 — upstream-autonomy unlock** (merge `bfa7a72`, 6-phase, reviewer PASS at every gate). Ships:
  - `observations.status` gets `ready` state + framework `confirmed→ready` "ratify" transition.
  - `maybe_auto_ratify_observation` post-confirm hook fires in-tx when `intent_contract.contract_state=ready` AND `approved_by`/`approved_at` set.
  - `add.rs` emits a synthetic `create` row in `transition_history` for every add (across all stores); under `--lock-contract` walks observation `open→investigating→confirmed` so the hook fires on filing.
  - `auto_promote.rs` builtin — subscribes `observations: confirmed→ready`; mints task at `planning` with `linked_observations` + contract content propagated; back-links `observation.task_id`. Idempotent.
  - `auto_scaffold.rs` builtin — subscribes `tasks: ''→planning` (the row-creation arrival convention); runs `scaffold.command` from `.stores/config.yaml`; writes `workspace_path`. Idempotent.
  - `AgentsYaml::validate` permits empty `from` (row-arrival convention; documented in philosophy.md v1.3).
  - `tests/flow_promote_scaffold_e2e.rs` — ratify→promote→scaffold E2E in <1s; idempotent third poll.
- **L009 fix** (`13f7dc1`) — Cargo.toml `default = ["runner-claude-code"]`. Bare `cargo install --path .` now produces a binary with `--claude-code`. Silent feature-drop closed.
- **L040 fix** (same commit) — `graph_easy_on_path()` does real spawn-and-pipe instead of `--version` (which exits non-zero on graph-easy 0.76 even when working). "Tests-skipped-as-pass" false negative closed.
- **L046, L047 → resolved** (resolution=`bfa7a72`); **L010 → resolved** (T019 `1c8d02b`); **L025, L027, L031 → wont_fix** (superseded/adopted-doctrine).
- **CLAUDE.md doctrine v2** (`250945f`) — U-moments collapsed from four to three (U2 folded into U1 because auto-promote makes ratify=produce-task atomic); U1 has two paths now (observation-first dominant; direct-task escape hatch). Added "Triage routing (the L043 rule)" subsection: ≤3 cheap tool calls then halt-or-route. Two new "What NOT to do" bullets (don't dive deep on observations; don't hand-crank ratified-but-unpromoted observations through `./dev new`).
- **Track A — observations filed for missing pipeline edges:**
  - **L048** — Auto-drive subscriber (T2, ~150–300 LOC, draft contract). Step 6 of the 10-step pipeline.
  - **L049** — Auto-resolve-observation subscriber (T1, ~50–100 LOC, draft contract). Step 10.
- **Bulk contract drafting:** 23 open observations now carry full draft `intent_contract` content (objective + type + in_scope + out_of_scope + acceptance + tier_hint). Stored in `.stores/db.sqlite` (NOT in git — wipe `.stores/` and you lose them).
- **6 commits pushed to `origin/main`**: `250945f` (doctrine), `13f7dc1` (L009/L040), `bfa7a72` (T020 merge) + T020's six in-branch commits via the merge.

## Notes Today

| # | Note | Topic |
|---|------|-------|
| 01 | [handover-engine-on-deck-for-blake.md](./01-handover-engine-on-deck-for-blake.md) | T020 shipped; engine on-deck. Documents what shipped, the "Pre-ratified queue trap" (L045/L038/L043 are NOT going to auto-promote spontaneously — `contract_state=ready` is a field, not a transition), the canary recipe (ratify L050 first, alone), the 23-observation drafted-contract backlog, and the operating-environment + decisions-pending. |
| 02 | [1006-friction-analysis-vs-stores.md](./02-1006-friction-analysis-vs-stores.md) | Realistic-pull analysis: maps 10.06's 6 friction roots to stores' state. 2 already structurally solved (mixed-bag stores; T2-on-main serialization). 1 half-solved (in-flight model — needs `touches_files`). 3 unaddressed (ratification cost uniformity; staleness decay; intake routing). Names the orchestrator-as-fleet-planner anti-pattern. Calls for write-time guards over read-time aggregators. |
| 03 | [primitives-and-engine-metaphor.md](./03-primitives-and-engine-metaphor.md) | Working session with Blake on the substrate's primitive set. Engine reframe: typed buffers connected by transitions, specialized subagents at each transition. 6 primitives we have words for (Buffer, Transition, Subscriber, Actor, Direction, Schema); 6 we don't (Loop, Aggregation, Decay, Notification, Capacity, Causality). Architectural unlock: dissolve `tasks drive` monolith into 5 transition-subscribers in agents.yaml; specialization-by-transition not by-role. Largest implication: stores as decision-routing substrate for *every* agent (unified pull-queue across 2-8 concurrent agents). |

## Tensions

- **`Pre-ratified queue` framing in 2026-05-03 note 09 was carried forward as "ratified contracts awaiting auto-promote."** Note 01 today corrected it: `contract_state=ready` is a side-field write; the framework hook fires on the `confirmed→ready` state transition, which these three observations never walked through. Zero rows in `transition_history` for L045/L038/L043. **Resolution: today's correction wins.** Walk them through `investigate → confirm` (the latter is the U1 trigger) to fire the chain. The canary L050 should be ratified first, alone.

## Open Threads

- **Engine ON DECK, not turned on.** Daemon running PID 1297522, polling 5s, log `/tmp/stores-daemon.log`. Awaiting Blake's deliberate ignition.
- **Canary recipe:** ratify L050 (T1, single-file regen of `topology_dot_snapshot` fixture) first, alone, watch daemon log for auto-promote + auto-scaffold within ~5s. If clean, escalate.
- **`.stores/config.yaml` scaffold section** — verify it points at `./dev new --slug={slug} --base={base}` or equivalent. Without it, `auto_scaffold` no-ops silently and `workspace_path` doesn't get set; auto-drive (when L048 ships) won't fire on workspace_path-less rows.
- **Pre-existing failing tests on main:** `topology_dot_snapshot::ac2_4_dot_snapshot_matches` (drift after T019 added states; L050 tracks); `topology_dot_render::ac_max_line_width_under_120` `#[ignore]`d (L041); `e_schema_migrate_failure_blocks` flaky under parallel runs (passes in isolation).
- **L041 — width contract decision pending.** Z1 tasks zone is 128 cols; AC says ≤120. graph-easy 0.76 has no `--width`. Pick: try `rankdir=LR`, or bump AC to 140. Currently `#[ignore]`d.
- **Auth-UX bundle (L013 + L014 + L015 + L044, all high)** — recommended as a single ~150 LOC task touching `src/cli/auth.rs` + setup docs. L044 cross-tool-footgun makes L013 default-path fix urgent.
- **L048 + L049** ratification close the upstream pipeline (steps 6 + 10). After both, pipeline is fully autonomous between U1 and U3.
- **L030 (uniform-pipeline tier-aware briefs)** — currently doc-only; planner/code-reviewer briefs need to consume `tier_hint`. Medium task.
- **L035 (schema-enforced inter-agent context)** — T3, biggest architectural follow-up.
- **Token rotation** — previous session's token consumed; fresh paste needed for next ratifications. `stores auth show` is the source.

### Roadmap surfaced today (notes 02 + 03 — to be filed as observations)

From 10.06 friction analysis (note 02):
- **`touches_files: list<text>`** on `observations.intent_contract` and `tasks.contract` + planning-arrival transition guard rejecting overlap. Conflict caught at write-time, not by a pre-flight aggregator. **High / T2 — the deepest fix.**
- **Tier-aware `required_when` predicates** (`AND tier_hint != 'T1'`) + `stores observations bulk-disposition` verb. Closes ratification cost cliff. **High / T1-T2.**
- **Staleness auto-park policy** (low-prio + 14-day-stale → hidden state) via `policies.yaml` daemon subscriber. **Normal / T1.**
- **`stores intake <text>` skill** — runs Q1/Q2/Q3 rubric, dispatches to right `add` verb. **Normal / T2.**

From primitives session (note 03):
- **Update `docs/philosophy.md`** with concise primitives section (working draft + missing-set candidates: Loop, Aggregation, Decay, Notification, Capacity, Causality). **High / T1.**
- **LLM-backed subscribers in `agents.yaml`** (the `command: "claude-code:<agent>.md"` shape) — the architectural unlock; dissolves drive cycle into transition-specialized subscribers. **High / T2-T3, bigger than L048.**
- **`decisions` store / `actor: human` queue convention on `observations`** — unified pull-queue across all in-flight agents. The largest UX implication. **High / T2-T3.**
- **`stores observations triage` pull-shaped verb** (5 forced choices; defaults preselected; skip-to-defer). Pairs with default-T1 + tier-aware required_when. **High / T2.**
- **`stores fleet` / `stores status`** btop-style flow visualization reading `transition_history` for per-edge throughput. **Normal / T2.**

## Tomorrow

- **Canary first**: investigate + ratify-update + confirm L050 with token; watch `/tmp/stores-daemon.log` for auto-promote + auto-scaffold within ~5s. Verify task lands clean before escalating.
- **If canary clean**: walk L045 → L038 → L043 through `investigate → confirm` (their `contract_state=ready` is already set; skip the update step but still need investigate + confirm to fire the chain). T1 first; investigator subagent (L043) last.
- **Auth-UX bundle** (L013 + L014 + L015 + L044) next — file as one umbrella task or walk individually.
- **Decide L041** — layout-experiment vs contract-bump.
- **L048 + L049** ratification once the canary chain proves stable.
- **L030 code-up** — make planner/code-reviewer briefs consume `tier_hint`.
- **File the roadmap observations** from notes 02 + 03 (touches_files; tier-aware required_when; bulk-disposition verb; staleness; intake skill; primitives doc; LLM-backed subscribers; decisions store; pull-shaped triage verb; fleet/status visualization).
- **Don't pre-ratify the queue.** L050 first, alone. Cascading-promotion-bug risk.
- **Don't dive deep** as orchestrator (L043 rule). ≤3 cheap tool calls then halt-or-route.
