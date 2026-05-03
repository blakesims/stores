# Stores → 10.06: design discussion

**Date:** 2026-05-03
**Type:** note

## Summary

Mid-session design discussion grounding `stores`' autonomy roadmap in a real client workflow (the 10.06 project's `./dev` + skill stack). The trigger was a felt friction with stores' two soft spots: (1) reviewing a completed task at the back gate is awkward — the wrap envelope is structured but renders as a one-line stub, so the human ends up reading JSON; (2) merging is still a manual bottleneck even after T019 ships post-accept ceremony, because failure-handling escalates straight to the human instead of being agentically owned. The conversation widened into: how should stores absorb 10.06's tier-driven dispatch, gate taxonomy, and pre-merge gate without locking into 10.06-specific assumptions (Fly, alembic, capability YAML).

The substantive moves:

- Conceded that the orchestrator-AI on main IS a conflict source for inline 1-LOC fixes. Proposed rule: in stores, **inline-on-main is human-only** — every `ai_autonomous` task gets its own `T###` branch via `./dev new`, even for tiny fixes. The tradeoff (overhead of a worktree for 5 LOC) is worth a clean serialization model: merge-lock-on-main becomes the only contention point.
- 10.06 is the **realistic pull** keeping stores honest. Without it, stores risks building generalizations that only fit its self-build (e.g. `cargo install` as the deploy verb — already a leak). The principle: design moves get pressure-tested against 10.06 before they ship.
- Stores' job is to **extract config, retain orchestration semantics**. The phase shape (Phase 0 contract → Stage 0 size → tier dispatch → execute loop → wrap → deploy chain) and lock primitives (per-worktree namespace) generalize. The substrate (alembic, Fly, capability YAML, postgres feat DBs) is project YAML.
- The "deployment blocker agent" the user wants is **not a new agent role** — it's a regular T1 task auto-created by a builtin on accept-merge / cargo-install / schema-migrate / deploy failure. The novelty is three small things: (a) builtin-triggered task creation, (b) a priority lane in the queue, (c) auto-retry of the parent deploy on accept of the fix task.

T019 (post-accept ceremony) was accepted, merged, and shipped during this conversation. The chain `accept-merge → cargo-install → schema-migrate` is now live in the binary. The merge surfaced topology snapshot conflicts (T018 changed renderer formatting; T019 added new states) — resolved by regenerating fixtures. One test (`h_ntfy_halt_event_body`) flakes under parallel execution but passes serially; this is a test-isolation bug, not a regression.

## Details

### What 10.06 already does (pulled by background subagent investigation)

The 10.06 project (`/home/blake/repos/clients/10.06-wt/10.06-main`) is a real client codebase running:

- **Filing rubric** (`research/refs/filing-rubric.md`): three-question decision tree.
  - Q1: Can I fix this turn? → fix, no filing.
  - Q2: Blake's hand or human decision? → file gate (one of 7 categories: prod DB write, critical decision, external info, inaccessible action, Fly secret, coordination wait, live-env policy).
  - Q3: Bug needs investigation? → file observation.
  - Co-file (both Q2 + Q3) only when both are true. Never co-file otherwise.
- **Tier-driven dispatch** in `/task:open` skill:
  - **T1** (≤2 files, ≤50 LOC, no migrations/secrets/config): inline fix on main, commit, close observation. No worktree, no plan, no PR. ~5-10 min round-trip.
  - **T2** (≤5 files, ≤200 LOC, single subsystem): in-session executor + code-reviewer subagent loop on main. No worktree, no plan stage. Commit on PASS.
  - **T3** (anything beyond, OR forced by `touches: migration|secret|capability|cross_system`): `./dev new` worktree + full planner/review/execute/CR cycle + `/task:wrap` to merge & deploy.
  - Forced step-up rule: `final_tier = MAX(tier_hint, touches_floor)`. Tier hint is the agent's guess; `touches` is the arbiter.
- **Pre-merge gate is non-skippable, runs on the feat worktree**: 11-step deployment gate (alembic, tsc, pytest, e2e, prod smoke, truth engine invariants, etc.). Phase 2.5 of `/task:wrap`.
- **Phase 1 capability tracking** (`app/backend/config/phase-1-capabilities.yaml`): rollup script recomputes capability status from completed tasks; reconciles at wrap-time, before the post-YAML test gate, so the YAML change rides the same deploy as the code.
- **Lock primitives**:
  - Phase-4 YAML lock (`flock 203` on `/tmp/carli-phase4.lock`) — serializes wrap's YAML rollup across concurrent wraps.
  - Per-worktree build/gate locks — `flock 202`/`201` on `/tmp/carli-{build,gate}-${worktree}.lock`. Concurrent across different worktrees.
  - Observation lock — JSON entry `lock` field with 2h TTL, prevents parallel `/task:open` collisions.
  - TID claim — implicit via `tasks/active/TXXX/` directory existence; `next-id` scans `max(active/, planning/, completed/, commits)`.
  - Port slot registry — append-only `worktree-ports.conf` mapping slugs to 100-offset port slots.

### Where stores already aligns with this

| 10.06 primitive | stores equivalent | Status |
|---|---|---|
| Filing rubric (Q1/Q2/Q3) | `observations.add` + `tasks.add --linked-observations` | Partially shipped; gate concept missing |
| Tier dispatch (T1/T2/T3) | `tier_hint` field; `--lock-contract`; planner brief includes tier | T013 shipped tier_hint field; L030 captures the planner-brief side; T1/T2 inline-on-main NOT yet shipped |
| Pre-merge gate | `accept-merge` builtin (T014) | Builtin runs `git merge` + `cargo build`; no test/lint gate |
| Post-accept ceremony | `cargo-install` + `schema-migrate` builtins (T019) | Shipped today |
| Lock primitives | `transition_history` audit + per-row claim | Per-row claim shipped (T014); merge-lock + deploy-lock rows NOT yet defined |
| Observation lock | not present | Gap |
| Gate concept | not present | Gap (filed below as observation candidate) |

### Where stores has leaks against 10.06

1. **`cargo install` as the deploy verb is too narrow.** T019 hard-wires it. Real client deploys are Fly, Vercel, Cloudflare Pages, AWS ECR, kubernetes, GitHub Action triggers. The right shape is YAML-defined: `agents.yaml` declares a `deploy` builtin with `command: <project-side script>`; for stores' self-build, `command: cargo install --path . --features runner-claude-code --quiet`; for 10.06, `command: ./dev deploy prod --from-wrap`.
2. **No gate concept.** 10.06 has 7-category gates (prod DB write, decision, external info, inaccessible action, Fly secret, coordination wait, live-env policy). Stores has only observations. Gates and observations are siblings, not parent/child — the difference is "Blake's hand needed" vs "investigation needed." Stores would benefit from absorbing gates as schema-typed rows with `actor: human` on the command/answer field, ntfy on creation, and a `category` enum.
3. **No pre-merge test gate.** `accept-merge` runs `git merge` + `cargo build` but doesn't run the test suite. 10.06's gate runs 11 steps. Stores' equivalent should be a configurable gate (project-defined test command set) that runs on the feat worktree before merging to main.
4. **No tier-driven inline-on-main.** Every stores task today goes through full plan/review/execute even for trivial fixes. 10.06's T1 path is "executor on main, commit, done" — no plan, no worktree, no review subagent. Stores' planner emits a one-phase plan for T1, but the worktree + plan-review overhead is still there.
5. **Wrap envelope renders as a stub.** The structured `wrap_log` JSON has executive_summary, recommended_sanity_checks, deviations, residual_risks. The markdown render projects a one-line stub (L021). 10.06's `/task:open` Stage 7 produces Executive summary → Deeper dive → Technical considerations → "To understand" with concrete verification commands.

### Multi-agent conflict model

The user's worry: 3+ agents on the same branch causes conflicts. Resolution:

- **Each task has its own worktree + branch + DB slot** (10.06's existing model; carries to stores via `./dev new`).
- **Inline-on-main is human-only** in stores (the new rule from this discussion).
- **Merge-to-main is the only serialization point** — solved by a single named lock (`merge-lock` row in DB), held by the daemon while accept-merge runs.
- **Deploy is a second serialization point** — `deploy-lock` row, held while the deploy ceremony runs.
- The orchestrator-AI on main writes only DB rows (not application code on the merge branch), so it's not a conflict source for autonomous tasks. For human-driven inline fixes, the rule above prevents collision.

### The deployment-blocker-agent shape

When accept-merge / cargo-install / schema-migrate / deploy fails:

1. Builtin records `transition_history` row with `policy_ref=<failed-builtin>`, `result=failed`, stderr tail.
2. Builtin auto-files an observation (`L0XX deploy-blocker on T0XX: cargo install failed, stderr=…`).
3. Builtin auto-creates a fix task (`T0XX-fix`, `tier_hint=T1`, `priority=urgent`, `depends_on=T0XX`, `linked_observations=L0XX`, contract pre-filled from stderr + diff).
4. Daemon picks it up off the priority queue, drives it through normal planner → executor → code-reviewer.
5. On accept, post-accept builtin chain re-runs the original deploy.
6. After N failed cycles → halt + ntfy human (genuine escalation; >1 fix attempt = stuck).

This reuses the existing engine. The "deployment blocker agent" is just a T1 task with a priority bump, an auto-fire on accept, and a halt-after-N policy.

### Open questions for next session

1. **Auto-accept tier for deploy-fix tasks?** If a T1-deploy-fix passes code-review, should it auto-accept (zero halt) or still require human assent (one halt per fix)? Pro auto-accept: zero friction. Con auto-accept: a bug that passes code-review but breaks production ships without human eyes. 10.06's discipline is "Blake always sees the deploy." User's dream is "auto-pass close-to-prod fixes." Real tradeoff; not yet decided.
2. **Gate categories** — adopt 10.06's 7-category model wholesale, or simplify to fewer? The user named several real-world examples that don't cleanly fit one category (API key gathering, client copy collection, configurable values).
3. **Test-gate command** — should stores' pre-merge gate run a project-defined test command from `agents.yaml`, or always default to `cargo test` for the self-build and require explicit project config for client work?
4. **Inline-on-main for T1** — do we make `tasks add --tier-hint T1 --invoker ai_autonomous` automatically dispatch the executor on main without a worktree (matching 10.06's T1), or always require `./dev new` for autonomous writes? The latter is simpler; the former matches 10.06.

## Follow-ups

Observations to file (autonomous, since filing is autonomous work):

1. **Generalize deploy verb beyond cargo install** — `agents.yaml` declares `deploy:` builtin with project-defined command. T019's `cargo-install` becomes one specialization, not the type. Priority: high. Tier: T2.
2. **Gate concept (schema-typed rows)** — absorb 10.06's gate taxonomy into stores: `gate` store with `type: script|decision`, `category: 1-7`, `actor: human` on command/answer. Priority: high. Tier: T3.
3. **Pre-merge test gate** — `accept-merge` builtin runs project-defined test command on feat worktree before merging. Priority: normal. Tier: T2.
4. **Inline-on-main for T1 autonomous fixes** — supersedes the current "every task gets a worktree" rule. Priority: normal. Tier: T2.
5. **Merge-lock + deploy-lock rows** — single named locks held by daemon during ceremony. Priority: normal. Tier: T2.
6. **Auto-promote deploy failure to T1-fix task** — builtin auto-creates fix task on accept-merge / cargo-install / schema-migrate / deploy failure with priority lane and auto-retry on accept. Priority: high. Tier: T2 (depends on L039 retry-on-failure landing first).
7. **Wrap render: project full envelope into markdown** — port 10.06's Stage 7 shape (Executive summary → Deeper dive → Technical considerations → "To understand"). Half the work is L021 (already filed). Priority: normal. Tier: T1.
8. **ntfy on awaiting_acceptance** — extend T014's ntfy hook to fire when a task lands in `in_review` status. Priority: normal. Tier: T1.
9. **h_ntfy_halt_event_body parallel-test flake** — global ntfy mock isn't isolated across parallel tests; passes with `--test-threads=1`. Priority: normal. Tier: T1. Surfaced by T019 merge.

Decisions to make next session (block on these before promoting follow-ups to tasks):

- Auto-accept tier for deploy-fix tasks (yes / no / configurable).
- Gate category taxonomy (adopt 10.06's 7-cat as-is / simplify / delay).
- Inline-on-main rule for T1 autonomous (always worktree / inline allowed).
