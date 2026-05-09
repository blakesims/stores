# Handover — pi-architect

**Date:** 2026-05-09
**Type:** handover
**Role:** pi-architect

## Active thread

`/home/blake/repos/.agent-comm/threads/2026-05-08-01-2026-05-08-post-c0f45ff.md`

## Current responsibility

Pi is governing architecture and priority only. Engine-controller runs the daemon/tasks; queue-curator cleans/routes the queue. SOP source-of-truth was updated and committed at `aa895cf docs: codify manual meta-substrate escalation` — do not duplicate it here; read:

- `.claude/skills/pi-architect/SKILL.md`
- `.claude/skills/engine-controller/SKILL.md`
- `.claude/skills/queue-curator/SKILL.md`

Key current policy: normal task work can continue through the substrate, but small concrete **meta-substrate/control-plane blockers** should be packaged for Blake to fix manually on `main` rather than burned through full workflow cycles. During a pause, no new tasks/ratifications/resumes/accepts unless Blake explicitly authorizes a row.

## Current architecture/priorities

1. **Engine rescue / clean resume first.** Blake has manual rescue work in flight from:
   - `docs/worklog/2026-05-09/01-engine-rescue-sketch-plan.md`
   - `docs/worklog/2026-05-09/02-meta-substrate-rescue-plan.md`
   Resume broadly only after active rows are cleanly cleared/parked and latest guardrail binary is confirmed.
2. **Runner/dispatch stability.** Silent-zombie rate under WIP=5 is the immediate throughput limiter. I034/I035-class evidence says this is broader than one task or role.
3. **Resume/transition safety.** I033/L519 was the major correctness bug: blocked tasks with NEEDS_WORK plans could resume to executing on rejected plans. Manual rescue commits reportedly fixed guardrails; verify current binary/rows before resuming contaminated tasks.
4. **Integration lane MVP.** L528 was hardened into a substantial T3 and ratified by Blake; auto-promoted to **T123**. This is the major architectural direction: parallel candidate production, serialized/current-main-validated mutation of `main`.
5. **Watch/flowtop observability.** Blake may manually patch `stores watch`; substrate routing for watch work should stand aside while he does. L529 reportedly exists for the larger redesign.
6. **Queue cleanup after rescue.** Resolve duplicate/re-mint observations, abandoned/contaminated tasks, and dangling locks only after active row decisions are made.

## Active work / processes

Snapshot from `stores tasks status` around handover:

| item | status | why Pi may care | next action |
|---|---|---|---|
| T116 / L180 | `blocked`, phase 1/1 cycle 2 | T1 stress-test task hit silent_zombie; evidence for runner instability | Do not resume blindly during pause; inspect if Blake resumes. |
| T117 / L187 | `code_review`, phase 1/1 cycle 1 | T1 CLI ergonomics task recovered enough to reach code_review | Let engine-controller handle if lifecycle-clean; no Pi unless reviewer dies/semantics widen. |
| T120 / L518/L489 | `schema_migrated` | Stale-binary-alive detector shipped and self-validated by catching T121 stale inode | Mark as important evidence in next engine-health refresh. |
| T121 / L520 | `blocked` | I026 narrowed retest; blocked by T120 stale-binary detector / plan loops | Do not blindly resume; decide whether to keep as evidence, remint, or inspect manually. |
| T122 / L523 | `in_review`, phase 1/1 cycle 1 | Manual rescue path for paired I024/I025 subscriber-edge fix; repeated code-reviewer silent_zombies earlier | Per rescue plan, clear via human-grounded review decision if valid; do not retry blind code-reviewer loop. |
| T123 / L528 | `executing`, phase 1/5 cycle 1 | Integration Lane MVP T3 is active despite broader pause because Blake explicitly ratified it | Ensure this remains architecture-aligned; external_reviews canonical for T3. |
| T108 | `blocked` | Historical parked T108 / original I026 evidence | Keep parked unless Blake/Pi explicitly decides otherwise. |

## Important rulings from this Pi session

- `msg_db0c8d84` / SOP commit `aa895cf`: meta-substrate blockers with small concrete fixes go to Blake for manual-main repair by default; agents package evidence and stand aside.
- `msg_fe2487ca`: pause semantics — no new work/resumes/accepts unless explicitly authorized; preserve evidence; do not kill daemon/children unless told.
- `msg_0c3f0fba` then `msg_db86d9f1`: stress-test WIP=5 was authorized, then reduced after repeated silent_zombies and I033 contamination risk.
- `msg_e7a93ed2`: T118 contaminated by resume-on-rejected-plan; abandon and remint, route I033/L519.
- `msg_f2783660`: I026 narrowed — broad missing-plan-context drift not reproducing post-`c0f45ff`; mechanically ambiguous literal clauses still defeat planner. T114 abandoned/reminted as T121 with tightened Test 6.
- `msg_0598bc48`: T122 repeated code-reviewer silent_zombies — use manual code-review + submit-review, then substrate-native external_review as canonical gate; do not close-out-of-band by default.
- `msg_78777517`: L528 integration lane contract ratified/promoted to T123 after Blake explicit approval.

## Pending decisions / doctrine risks

- **Resume safety:** confirm rescue guardrail commits are installed in the daemon binary before any blocked row resume. The rescue notes mention `bca5fd7`, `86b0614`, `e9ba39e`, `8dfdad1`, plus later commits; next Pi should not rely on stale handover assumptions.
- **Silent-zombie root cause:** repeated deaths under stress are not fully explained. If another broad silent_zombie wave appears, lower WIP/ask Blake to inspect runner/resource layer rather than churning remints.
- **Manual rescue lane:** Blake wants to handle quick meta-substrate fixes himself. Agents should escalate with a crisp packet, not implement, unless Blake delegates.
- **T123 / integration lane:** large T3 is now active. Watch for scope drift into file-overlap scheduling, batching, feature flags, or broad conflict solving — all out of scope for L528/T123 MVP.
- **I025:** narrowed heavily; auto-promote worked for routed→ready and closed_out_of_band predecessor cases. Known suspect remains abandoned-task + re-ratify path.

## Do not do

- Do not blindly resume blocked rows, especially those with NEEDS_WORK/rejected plans or silent_zombie history.
- Do not start/ratify new work while Blake's pause/rescue context is active unless he explicitly authorizes a row.
- Do not route Blake's manual `stores watch` / control-plane repairs through normal workflow while he is patching them.
- Do not raw-SQL write the DB.
- Do not expand T123 beyond integration-lane MVP into scheduler/file-overlap/stacked-diff/batching work.

## First step for next agent

1. Join/watch the active thread above as `pi`.
2. Read the three SOP skill files listed in **Current responsibility** and the two rescue plans.
3. Ask engine-controller for the latest paused/rescue inventory if not already posted after the manual agent completes.
4. Before approving resume, verify: T122 disposition, T121/T117/T116 status, latest binary includes rescue guardrails, and no blocked row requires a blind resume.
5. If Blake asks to resume, recommend low WIP first unless runner stability has been explained.

## Notes

L528/T123 is the main architectural arc to protect: **parallel candidate production, serialized integration of `main`**. It is intentionally one substantial T3, not a pile of tiny doc/schema tasks.