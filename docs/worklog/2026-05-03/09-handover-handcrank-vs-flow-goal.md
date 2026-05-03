# Handover: hand-crank reality vs flow-through goal

**Date:** 2026-05-03
**Type:** handover

## Summary

**Read first**, in this order: `/CLAUDE.md` (doctrine, U-moments, token); `/docs/philosophy.md` v1.2 (schema-as-engine, two-gate frame, Pull-from-Real-Use); `/docs/worklog/2026-05-03/06-take-off-handover.md` (yesterday's take-off context); `/docs/worklog/2026-05-03/08-flow-observation-to-task-lifecycle.md` (the 10-step pipeline diagram and design seams — this handover assumes that picture).

**The big-picture frame.** The substrate's destination state is "two-gate flow": the human ratifies a contract (U1, front gate) and accepts finished work (U3, back gate); everything between flows through autonomous daemon edges. Today, only **steps 7–9 of the 10-step pipeline are wired** (post-accept ceremony from T014+T019: accept-merge → cargo-install → schema-migrate). Steps 4–5 (auto-promote + auto-scaffold) are **in flight as T020 right now**. Steps 2, 3, 6, 10 are filed-or-unfiled and unpromoted. Until those land, the orchestrator-on-main hand-cranks every step between U1 and U3 — typing `./dev new`, re-keying contract content, backfilling `linked_observations`, sequentially invoking `tasks drive`, surfacing wrap envelopes, all by hand, blocking the user thread on read-think latency.

**The key insight** the user named mid-session: when L046 (auto-promote) was about to be hand-cranked exactly the way the current orphan-prone workflow handles things, it became visible that the "hand-crank pattern itself" is the leak. The substrate's whole point is that the daemon moves things. The hand-crank we're tolerating right now is bootstrap-only — exactly one task (T020) gets the broken treatment so it can ship the unlock; everything after flows through it.

**Don't drift back into hand-cranking.** The temptation is real because there are 31 open observations and only one daemon edge wired today. Every observation in the queue could be hand-cranked through, and that *feels* like progress. It isn't — it's wasted work that the auto-promote subscriber will obsolete in 1–3 hours. The right work right now is on **the design substrate** (filing the missing observations; updating doctrine; cleaning up superseded items) — work that doesn't burn API time and that compounds when T020 lands.

## Details

### State of the world at handover (2026-05-03 ~14:55 UTC)

**Engine:**
- Daemon running, PID 1297522, polling every 5s, currently idle.
- `dispatch_locks` populated with starting-line (48 rows): 3 `ok` (T019), 4 `exit=1` (T014/T018 stuck on stale-worktree bug, observation L045), 41 `skip-historical` (synthetic).
- ntfy fires on policy halts; `transition_history` audits every automatic write.
- Daemon log: `/tmp/stores-daemon.log`. Stop with `kill -INT 1297522`.

**T020 in flight (background ID `bq7o4r7f4`):**
- Ships auto-promote + auto-scaffold subscribers (steps 4–5 of the pipeline).
- Currently re-driving after a substrate bug (eval_length on Null) was fixed inline (commit `476144a`) and T020 was surgically reset (`status=blocked → planning`, `plan=NULL`, `plan_review_log` preserved with prior NEEDS_WORK so the planner brief surfaces feedback).
- Worktree at `/home/blake/repos/experiments/stores-T020-upstream-autonomy-unlock`.
- Estimated wall-clock: 1–3 hours total.
- Linked to L046 (auto-promote) and L047 (auto-scaffold), both with bidirectional `task_id=T020` set.

**Pipeline status (mirrors worklog 08):**

| Step | State | Carrier |
|---|---|---|
| 1. File observation | shipped | `observations.add` |
| 2. Triage | partial | orchestrator discipline; L043 adds `needs_investigation` state |
| 3. Investigate | missing | L043 (filed, contract ratified, NOT promoted) |
| 4. Auto-promote | **in flight** | T020 |
| 5. Auto-scaffold | **in flight** | T020 |
| 6. Auto-drive | unfiled | needs filing → ratify → promote (Track A) |
| 7-9. Post-accept ceremony | shipped | T014+T019 |
| 10. Auto-resolve-observation | unfiled | needs filing → ratify → promote (Track A) |

### The hand-crank vs flow-through reality

**Today the orchestrator-on-main carries every transition between U1 and U3 manually.** Concretely, for one observation to ship, the orchestrator does roughly this — every time:

1. Files observation (autonomous; OK).
2. Triages by reading body + grep + a few file reads (autonomous, but **L043 anti-pattern**: the orchestrator dives deep instead of routing — see L043).
3. Drafts intent_contract via inline conversation, presents YAML to user (orchestrator burning tokens on something the investigator subagent should own).
4. **U1**: user types verb or pastes token, orchestrator runs `observations update LXXX --contract-state ready --approved-by blake --approved-at <now> --invoker ai_with_human --approve-token <T>`.
5. **Hand-crank promote** (the orphan-prone part): runs `./dev new --slug=<derived> --title=... --done-when=<re-typed contract.acceptance> --scope-in=<re-typed contract.in_scope> --scope-out=<re-typed contract.out_of_scope> --base main`. Re-keys contract content from observation into ./dev new flags by hand. Result: **task is created with no link to observation**, contract content drifted from source.
6. Backfills `tasks update T0XX --linked-observations LXXX --tier-hint TX --invoker ai_autonomous` (separate command).
7. Backfills bidirectional link: `observations update LXXX --task-id T0XX`.
8. Manually starts the drive: `tasks drive T0XX --claude-code --invoker ai_autonomous` — synchronous, blocks orchestrator thread for ~30–90 min per task unless run with `run_in_background: true`.
9. Watches the drive (or waits for completion notification).
10. **U3**: user types verb or pastes token, orchestrator runs `tasks accept T0XX --invoker ai_with_human --approve-token <T>`.
11. Daemon picks up the in_review→accepted transition; engine ceremony fires (T014+T019).

Steps 5–8 are orchestrator hand-crank. **The point of T020 is to obsolete steps 5–7; the point of an unfiled "auto-drive" subscriber (step 6 of the pipeline) is to obsolete step 8.** After both ship + we file step 10's auto-resolve, the orchestrator's role collapses to: triage routing, surfacing, and U-moment relay. That's the autonomy budget.

### What's queued behind T020 (do not hand-crank these)

These observations have ratified contracts but have **not been promoted to tasks**:

- **L045** (T1, ~50 LOC): accept-merge tolerates already-merged branches with cleaned worktrees. Surfaced from the engine's first run today (T014/T018 dispatch_locks stuck at `exit=1`).
- **L038** (T1, ~50 LOC): tasks drive refuses when `depends_on` is unmet (passive guard, Layer 1).
- **L043** (T2, ~150–250 LOC): investigator subagent + needs_investigation routing. Closes the L043 anti-pattern (the orchestrator-investigates-inline trap that bit us today on L042 + the eval_length bug).

**Do not hand-crank these now.** Once T020 ships and is accepted (back gate), every contract you ratify hits the auto-promote subscriber within ~5s and creates a task autonomously. Pre-ratifying these now and waiting for T020 is **the eat-our-own-dogfood test** of the new pipeline. Watch carefully: if auto-promote has a bug, only ONE observation cascades into a broken task at first. Verify it works on one before unleashing the rest.

### Today's surfaces and fixes (engine running surfaced these)

- **L042 closed (commit `f3ddc21`)** as a misdiagnosis: "run-log capture broken" was actually test pollution from four runner tests inheriting the manifest dir as cwd. Fix: `STORES_RUNS_DIR` env override + tests redirect to gitignored `target/test-runs/`. 135 historical leaked files cleaned. Real run-logs (in worktrees) were always healthy; we'd been looking at the wrong directory.

- **L045 filed** (the engine's first real find — accept-merge stale-worktree bug surfaced in the 1-second daemon test before pre-populating dispatch_locks).

- **L043 filed** (orchestrator routes inline instead of delegating to investigator subagent — the meta-anti-pattern named by user mid-L042).

- **eval_length Null bug fixed inline (commit `476144a`)** — surfaced by T020's first plan-review NEEDS_WORK landing in `blocked` instead of `planning`. Root cause: `tasks add`'s generic insert path doesn't initialize `plan_review_log` to `'[]'` (only `drive.rs`'s claim path does); `read_row` then maps NULL → `Value::Null`, and `eval_length` matched the catch-all and returned false. 1-line fix + regression test. **The deeper issue** (read_row should map list-typed NULL/empty cells to `Value::Array([])` to match the schema's declared field type) is left for a future observation if it surfaces elsewhere.

- **L046 + L047 filed** as the upstream-autonomy unlock (auto-promote + auto-scaffold). Both linked to T020 via `inputs`/`linked_observations`.

- **Philosophy v1.2 shipped (commit `e44f795`)**: substrate vs deployment system distinction (subscribers project-declared); failure recovery as ordinary-task pattern; "Pull from real use" doctrine.

### Operating environment

- **Branch**: `main`, ahead of `origin/main` by ~6 commits (this session's fixes + worklog notes + philosophy v1.2). Push when ready.
- **Worktrees**: T020 active at `../stores-T020-upstream-autonomy-unlock`. Teardown after accept via `./dev done T020 --force` (DO NOT do this until T020 ships and is accepted).
- **Token**: user pre-decrypted approval token is in this conversation's context. **Fresh agent has nothing.** Ask user to paste (`stores auth show` decrypts; needs passphrase or hardware tap).
- **Binary**: `~/.cargo/bin/stores 0.5.0`, post-eval-length-fix reinstall with `--features runner-claude-code`. Verify with `stores --version` and `stores tasks drive --help | grep claude-code`.
- **DB**: `.stores/db.sqlite`; T019's full schema (cargo_installed, schema_migrated, deploy_blocked states + transition_history table). T020 will add observations.status `ready` + framework auto-transition + auto-promote/auto-scaffold builtins.
- **agents.yaml**: `.stores/agents.yaml` exists (copied from `docs/agents-yaml-example.yaml`); declares accept-merge, cargo-install, schema-migrate, user-escalation. T020 will add auto-promote + auto-scaffold to it.
- **Daemon**: PID 1297522, log at `/tmp/stores-daemon.log`. SIGINT to stop.

### Operating discipline (don't slip)

- **L043 routing rule** (still missing as a substrate primitive but enforce as orchestrator discipline): ≤3 cheap tool calls to triangulate; if root cause isn't obvious, file an observation with status=needs_investigation (when L043 ships) OR halt and ask user. Do not dive into 15-tool-call inline investigations — that's exactly the anti-pattern this session burned hours on.
- **Inline-on-main is human-only.** Every `ai_autonomous` task gets its own T### branch via auto-scaffold (post-T020) or `./dev new` (today, last hand-crank). The orchestrator's edits to source files happen only when the user is at the keyboard authorizing each one (e.g. the eval_length fix today).
- **--invoker discipline:** default `ai_autonomous`. `ai_with_human` only at U1/U3/U4 with token attached or user typing the verb. Never silently upgrade.
- **Don't pre-ratify** the queued contracts (L045/L038/L043) until T020's auto-promote is verified on one observation first. Cascading-promotion-bug risk.
- **SQL surgery is recovery-of-last-resort.** It bypasses transition rules; the row's audit trail loses fidelity. Used today to unblock T020 (status=blocked → planning, plan=NULL, plan_review_log preserved). Justified once; not a habit.

### Reference: 10-step pipeline diagram

See `/docs/worklog/2026-05-03/08-flow-observation-to-task-lifecycle.md` for the full ASCII pipeline and 8 design seams. Summary: 2 human U-moments (U1 ratify, U3 accept), 8 autonomous edges. Today, 3 of 8 edges wired (steps 7–9). After T020 ships: 5 of 8. After auto-drive + auto-resolve land: 7 of 8 (step 2's triage routing rule lives in orchestrator discipline, not a substrate edge — though L043's investigator subagent counts as the deep-dive part of step 2/3).

## Follow-ups

For the next agent, in priority order:

1. **Wait for T020 to wrap.** Background ID `bq7o4r7f4`. Status: `stores tasks status T020 --invoker ai_autonomous`. Drive output: `/tmp/claude-1000/-home-blake-repos-experiments-stores/0949c4c0-6ca9-446f-ad64-29920ce82846/tasks/bq7o4r7f4.output`. When wrap lands, surface the brief to user, present `deviations[]` and `residual_risks[]` honestly. User accepts (token); engine fires accept-merge → cargo-install → schema-migrate.

2. **Verify auto-promote actually works on a fresh observation.** After T020 deploys, file a test observation, ratify its contract, watch the daemon log + dispatch_locks. Within ~5s the row should auto-create a task. If it doesn't: that's a real bug to investigate (file observation, halt; do not inline-debug — L043 discipline).

3. **Tracks A/B/C from this session, in order:**
   - **A (highest leverage):** file observations for auto-drive (step 6) + auto-resolve-observation (step 10). Body should reference worklog 08; contract draft inline.
   - **B (doctrine):** revise CLAUDE.md to collapse U1+U2 into "U1 ratify (auto-promotes)" + codify L043 routing rule (≤3 cheap tool calls then halt+route).
   - **C (cleanup):** triage superseded observations: L010 (resolved by T019, sha `1c8d02b`), L025 (superseded by L030), L027 (superseded by L030), L031 (deferral note; close as wont_fix-style), and bootstrap-era L002/L005/L006 if appropriate.

4. **Once auto-promote is verified, ratify L045 → L038 → L043 (in that order).** Each ratification cascades through auto-promote → auto-scaffold → planner. Watch the queue. The investigator subagent (L043) is the most consequential because it ships the routing primitive that frees the orchestrator from L043's anti-pattern.

5. **After auto-drive + auto-resolve ship**, the loop closes. The orchestrator's autonomy budget collapses to triage routing + U-moment surfacing + design discussion. Watch for the next class of friction that emerges from real use (per philosophy v1.2's Pull-from-Real-Use doctrine).

6. **Push to `origin/main`** when convenient. Local has the eval_length fix, philosophy v1.2, worklog 07/08/09, and the L042 cleanup. Nothing destructive; standard `git push origin main`.

### What NOT to do (this session's hard-won lessons, restated)

- **Don't dive deep on observations.** L043 anti-pattern. File needs_investigation (after L043 ships) or halt and ask user.
- **Don't hand-crank promotion of L045/L038/L043** even though their contracts are ratified. Wait for T020's auto-promote, eat your own dogfood.
- **Don't run multiple drives in parallel right now.** API cost; complexity tracking concurrent drives. After auto-drive ships, the daemon handles parallelism.
- **Don't re-key contract content into `./dev new` flags.** That's the orphan-prone hand-crank pattern T020 obsoletes.
- **Don't forget to push to origin** after the next batch lands.
- **Don't bypass U-moments.** Token-mediated tier-A writes are the substrate's grounding mechanism. The doctrine works because we don't fudge it.
