# Engine Weakness Picture And Dogfood Recovery

**Date:** 2026-05-05
**Type:** note

## Summary

First sustained dogfood-throughput session after T022 (auto-drive) merged. Started with 65 obs in DB and a backlog "to push through the engine." Walked the full chain end-to-end live: ratify → auto-promote → auto-scaffold → auto-drive → planner → reviewer → executor → wrap, all on engine-self-fix tasks (T024 L045, T025 L063, T026 L055, T027 L066). Surfaced multiple high-priority engine bugs by RUNNING the engine — exactly the dogfood doctrine working.

**One sentence picture:** The engine can ratify and drive, but it can't yet (1) reliably restart, (2) reliably deploy, or (3) reliably refill its own input queue. Layer 1 (runtime) and Layer 4 (deploy) are the brittlest surfaces; Layer 8's auto-investigator gap is the strategic ceiling on dogfood velocity.

**4 new high-priority obs filed today (L063, L067, L068, L069)** — every one of them found by *trying to use the system*, not by review. None would have surfaced from staring at the code.

## Details

### Session arc

1. **Set `drive.max_parallel: 3`** in `.stores/config.yaml`. Config is read per-poll, no daemon restart needed.
2. **Phase A ride: L045 → T024.** First end-to-end ratification. Hit immediate substrate bug (auto-promote idempotency wrongly conflated `task_id` provenance with promotion back-link). Manual `task_id = NULL` workaround unblocked it. Filed as **L063**.
3. **Phase B parallel batch: L045 + L063 + L055 → T024/T025/T026.** Three engine-fix drives in flight at cap. Each ratified with token-mediated U1; auto-promote → auto-scaffold → auto-drive fired cleanly.
4. **/intent-harden L066** for the tier-structural drive cycle (T3, ~200-400 LOC). Hardened scope, codebase-grounded recommendations, ratified with token. Promoted to T027, queued behind cap.
5. **Multi-layer substrate failure mid-session.**
   - Daemon got SIGTERM (suspect: client-repo daemon's pkill cleanup) — filed as **L068**.
   - All 3 in-flight drives (T024/25/26) died at startup with empty logs because `./dev scaffold` doesn't init `.stores/` in the new worktree, and post-T022 auto-drive spawns drives with `cwd=worktree`. Dynamic subcommand discovery fails: `error: unrecognized subcommand 'tasks'`. Filed as **L067** (manifestation of the long-standing L032).
   - Plus stranger-things: **L069** filed by some other actor — `compute_resume` rejects `deploy_blocked` rows.
6. **Recovery (Option A):** symlinked `.stores/` from main repo into each broken worktree, deleted auto-drive `dispatch_locks` for T024/25/26, NULLed `drive_pid`, restarted daemon. L066 then auto-promoted to T027 (also symlinked proactively). All 3 drives respawned and reached planner phase within minutes.
7. **/doc:new-note** — this note.

### Where the engine bleeds — by layer

Status: 🟢 in flight · 🟡 queued · 🟠 contract ready · ⚪ open · GAP = not filed

**Layer 1 — Runtime / dispatch reliability** (today's #1 pain)
- L045 🟢 T024 — accept-merge fails when worktree gone
- L055 🟢 T026 — retroactive subscriber firing on restart
- L062 ⚪ T2 — watchdog can't catch post-spawn failures
- L039 ⚪ T2 — daemon retry-on-failure unimplemented
- **L067** ⚪ NEW — auto-drive spawns from worktree without `.stores/`
- **L068** ⚪ NEW — cross-project daemon pkill (other-repo killed mine)
- **L069** ⚪ NEW — `compute_resume` rejects `deploy_blocked` rows
- GAP — no per-project daemon PID file / `agents status` verb

**Layer 2 — State / idempotency**
- L063 🟢 T025 — auto-promote conflates surfacing `task_id` with promotion back-link
- L038 🟠 T1 — `depends_on` field exists but unenforced
- L011 ⚪ T2 — rows don't record `stores` binary version
- L053 ⚪ — tier-A actor check bypassable via `--invoker human` from `$CLAUDECODE`

**Layer 3 — Drive economics**
- L066 🟡 T027 — every tier pays full 5-stage cycle (queued behind cap)
- L030 ⚪ T2 — will be `wont_fix` as part of T027
- L028 ⚪ T2 — drive-spawned agents lack `/observe` skill access
- GAP — tier-aware code-reviewer brief modulation (deferred from L030)

**Layer 4 — Deploy ceremony** (T023 currently stuck `deploy_blocked`)
- L060 ⚪ T2 — schema-migrate runs from OLD daemon binary; new schema silently no-ops
- L061 ⚪ T2 — no pre-promotion acceptance precheck
- L020 ⚪ T1 — render leaves empty dirs across state transitions
- L064/L065 ⚪ — symptoms of T023's stuck merge (not separate bugs)

**Layer 5 — Discovery / observability**
- L032 ⚪ T2 — worktree lacks `.stores/` (parent of L067; symlink workaround in use)
- L054 ⚪ — no structured-read verbs for task review
- L057 ⚪ T2 — no per-agent metadata (model/tokens/duration)
- L058 ⚪ T2 — no read surface for throughput/fleet metrics
- L059 ⚪ T1 — `.stores/runs/` transcripts have no index, no row→transcript link
- L012 ⚪ T3 — no inspector for agent context

**Layer 6 — Auth / security**
- L013/L014/L015 ⚪ — auth init/show UX gaps; SOPS entanglement
- L044 ⚪ T1 — L015 symlink workaround broke sops globally
- L053 ⚪ — actor check bypass (cross-listed from Layer 2)

**Layer 7 — Schema / contract substrate**
- L005 ⚪ T1 — list-typed fields accept only single-string at update
- L035 ⚪ T3 — schema-enforced inter-agent context refs
- L019 ⚪ T3 — DockerRunner / standardized agent sandboxing

**Layer 8 — Orchestration discipline** (#2 strategic weakness)
- L043 🟠 T2 — orchestrator inline investigation (the L043 rule itself)
- L023 ⚪ T2 — observations missing `next-id` + JSON envelope inconsistency
- L049 ⚪ T1 — no auto-resolve of linked obs when task ships
- L002/L003/L006/L021/L034 ⚪ — assorted UX / asymmetry gaps
- **GAP — NO `open → investigating` subscriber**: pipeline is one-sided; 34 open obs sit forever without manual contract drafting. This is the auto-investigator gap that breaks the dogfood economic model.

### Standing-out themes

**1. Bugs are observations, not blockers (working).** Every substrate hurt this session became an obs (L063, L067, L068), not a paper-over. The recovery was manual but the trail was captured. The L042/L043 rule held even when the engine itself was the friction.

**2. Auto-promote idempotency was the silent killer.** Every standard-filed obs (with `--task-id <surfacing>`) was being filtered out of the dogfood pipeline. Of 65 obs, exactly ONE had a ready contract AND no `task_id` — and that one was already resolved. Without L063 ratified, we couldn't have moved at all. Manual workaround unblocked us; the fix is now in flight as T025.

**3. `./dev scaffold` doesn't init `.stores/` in the new worktree** — L032 was filed long ago (T013-era) but only became a hard blocker after T022 auto-drive started spawning drives with `cwd=worktree`. T021/T023 evidently survived because pre-T022 drives ran from main-repo cwd. T022 changed the cwd and didn't catch the regression. Tests didn't cover the symlink/discovery path. **Lesson: substrate changes that move execution context (cwd, env, paths) deserve a dedicated test suite for "runs from inside a worktree."**

**4. Cross-project daemon interference is real.** Two `stores agents run` daemons were live concurrently (this repo + 10.06-main client work). The other repo's startup script (suspected) used `pkill -f 'stores agents run'` which doesn't filter by project — it killed mine. Workaround: user alerted the other agent. Long-term: per-project PID file + `stores agents stop` verb.

**5. Cap-3 throughput model works.** With three concurrent drives, the cap held cleanly. T027 (L066) auto-promoted, scaffolded, and queued without spawning until a slot frees. The pre-claim cap check in `agents_run.rs:208-214` does its job; no broken claims, no wasted dispatches.

**6. /intent-harden is genuinely valuable for T3.** L066 was high-quality already, but the harden cycle:
   - **Cut nothing structural** (good sign — original obs was disciplined).
   - **Grounded the contract in the codebase** (`StateAction` enum, `transition_to` already exists, predicate language exists in `flow/predicate.rs`).
   - **Caught the predicate-language extension trap** for T2 phase-count guard (defaulted to handler code; filed as deferred follow-up).
   - **Surfaced the L030 fate decision** explicitly rather than leaving it for the planner to invent.
   - One alignment deviation (L030 → wont_fix vs amend); accepted as cleaner.

**7. The auto-investigator gap is the dogfood economic ceiling.** Today's session forced the human (me, plus token-mediated user) to draft contracts on 4 obs (L045 already had one; L063, L055, L066 needed drafting). Each contract is ~5-10 minutes of careful work. With 34 open obs still in the queue, that's hours of human/orchestrator time before the engine can drain. **Until a daemon-side auto-investigator subscriber drafts contracts on `open` rows, the substrate's input rate is bottlenecked at the human's drafting rate.** Filing this as an obs would be the single highest-leverage strategic move.

**8. The dogfood compounding loop is real and observable.** Three of today's in-flight tasks (T024/T025/T026) directly fix three of today's session pain points. Once they ship:
   - Future ratifications won't need manual `task_id = NULL` (L063 fix).
   - Daemon restarts won't retroactively damage rows (L055 fix).
   - Stranded-workspace deploys won't strand the chain (L045 fix).

   That's three engine bugs auto-fixing themselves via the engine, in parallel, in roughly the time of one cycle. **The compounding multiplier is what makes the doctrine non-trivially valuable.**

## Follow-ups

### Immediate (current session, monitoring)
- T024/T025/T026 currently in planner/executor phases. Watch for completion → U3 acceptance with token (per Option A policy).
- T027 (L066) queued; will spawn on first slot-free.
- T023 still `deploy_blocked`; needs L060 fix or hand-recovery.

### Highest-leverage next 3 ratifications (after current batch lands)
1. **L062** (watchdog catches post-spawn failures) — open T2, contract drafting needed. Closes silent-zombie failure mode that hurt today.
2. **L060** (schema-migrate from new binary) — open T2, contract drafting needed. Unblocks deploy ceremony for any task that adds schema (T026 will need this).
3. **L038** (`depends_on` enforcement) — already 🟠 ready, T1. Free chaining; lets us declare dependencies between fix tasks.

### Strategic (file as new obs)
- **Auto-investigator subscriber** for `observations: open → investigating`. Daemon-side drafting of `intent_contract` candidate fields (objective, in/out scope, acceptance, tier_hint) on every newly-filed `open` obs. Human reviews + ratifies; doesn't draft. This flips the substrate from human-pulls to engine-pulls on contract drafting and removes the dogfood velocity ceiling.
- **Per-project daemon discipline** — PID file at `.stores/agents.pid`; `stores agents run --detach` refuses to start if a live PID exists; `stores agents stop` reads the PID file. Project setup scripts use the verb instead of pkill. Closes L068 long-term.
- **L067 fix path** — choice between project-side (`./dev scaffold` symlinks `.stores/`) vs substrate-side (auto-drive spawns from main_repo cwd). Project-side is cheaper but per-project; substrate-side is general. Probably both eventually.

### Open questions
- Should the dogfood doctrine grow a **substrate-emergency runbook**? Today's recovery (symlink + DELETE locks + NULL pids + restart) was reasoned-from-first-principles. A second such session should not have to re-derive it.
- Is the `task_id` field overload (filing-provenance vs promotion back-link) worth a deeper schema fix beyond L063? L063 fixes the symptom (auto-promote uses `linked_observations` instead). The semantic overload remains. Filing a separate obs may be warranted once L063 lands.
- L032 → L067 latency was ~3 weeks (filed → manifested as a hard blocker). Any way to surface latent obs that match a recent change's risk profile? "L032 mentions `cwd from worktree`; you just shipped a change that runs commands from worktree" → orchestrator pings.
