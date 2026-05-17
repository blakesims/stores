# Handover — 2026-05-06 — Pipe-Fill Batch + T053 Sonnet Rescue

**Date:** 2026-05-06
**Type:** handover (substrate-agent → next CC session)

## ⚠ HEAD-FIRST INSTRUCTIONS

You're inheriting a long session. The doctrine is captured in `.claude/skills/engine-controller/SKILL.md` (skill auto-loads when relevant). This handover **does not duplicate the skill** — it covers (1) the workflow we converged on today, (2) the recent in-flight state, (3) the open architectural threads. Read in this order:

1. This note (head-first context)
2. `.claude/skills/engine-controller/SKILL.md` (your role, doctrine, operational patterns)
3. `.claude/skills/pi-architect/SKILL.md` (pi's role, what to ask vs. decide)
4. `docs/engine-health.md` (long-standing engine-state snapshot — needs refresh after T053 lands; flag below)
5. The two agent-comm threads:
   - Pi: `/home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-thread.md`
   - 10.06 client: `/home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-substrate-to-1006.md`

**Approval token (in-memory, never persist):** `<redacted-approval-token>`. Same Blake-issued session token used all of today.

**Start of shift:**

```bash
agent-comm watch /home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-thread.md --name substrate-agent --from-end
agent-comm watch /home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-substrate-to-1006.md --name substrate-agent --from-end

# pipeline state
for id in T053 T034; do echo -n "$id: "; stores tasks status $id; done

# daemon health
ps aux | grep -E "stores agents run" | grep -v "grep\|stores-track"
```

Daemon was running when this note was written; if it isn't, restart per the L149 pattern in the skill.

## Today's ships (8 tasks)

| Task | Obs | Tier | Notes |
|---|---|---|---|
| T050 | L134 | T2 | typed dispatch_locks (Path A); rebase reconciliation w/ T049 superseded |
| T054 | L133 | T2 | T1 synthesized canonical plan; eliminates `plan IS NULL` |
| T052 | L143 | T2 | risk metadata schema; pi's option-1 fix on `approval_policy` lock-down |
| T057 | L132 | T1 | schema validator unguarded-shadow refusal |
| T058 | L021 | T1 | wrap_log render into Completion section |
| T059 | L165 | T2 | `stores watch` terminal-row hide; structured-JSON blocked_reason classify |
| T060 | L169 | T1 | tier-aware executor + code_reviewer briefs |
| T053 | L142 | T2 | **MID-RESCUE** — see § Open thread |

Plus pi's own commits to skills + framing (90d478b, 3c76e17), my a108e3e, 7977a09 codifying procedures.

## Workflow we converged on

(Codified in the skill; here's the why.)

### Tier-gated codex review

**T1 → skip codex, accept on code_reviewer PASS.** Cheap rows; in-cycle gate is enough; codex is overhead.
**T2/T3 → run codex.** Architectural blast-radius justifies the latency.

This was Blake's call mid-session; saved ~15 min on the T1 batch. Skill updated; next session inherits.

### Cascading directives

When pi issues an architectural ruling (e.g. "T049 mechanism is subsumed by T050"), all downstream mechanical edits/tests follow without re-asking pi per file. Re-ask only on materially new semantic forks. Useful trigger phrase to keep pi in the loop: *"I think this is a cascading consequence of your prior ruling on X; proceeding unless you object."*

### Ratify autonomously (T1, with token); ask pi (T2/T3)

Ratification of T1 contracts is mechanical with the session token; T2/T3 contracts always run by pi first. The full CLI sequence is in the skill's *Operational patterns* section.

### Per-task runner override

When the default runner fails on a complex task, snapshot `.stores/config.yaml`, edit `drive.roles.executor.runner`, drive the task, restore. Today: T053 hit 4 cycles of `commit='none'` with Pi runner; switched executor to `claude-code:sonnet` per pi's option D (msg_8e8cc971); Sonnet got phase 1 PASS + phase 2 PASS in 3 cycles + most of phase 3 before hitting Sonnet's 5-hour rate limit.

### Ship batches in parallel

Today's max: 5 concurrent drives (T053, T057, T058, T059, T060). Daemon's `max_parallel: 5` config. Drives don't collide if files are disjoint. Accept-merge ceremonies pipeline cleanly when main worktree is clean (stash projections/logs first).

### Subagent delegation for bulk mechanical work

Used today for the T049→L134 test reconciliation sweep (3 e2e files, mechanical assertion updates). Delegate to `task-workflow:executor` subagent with the architectural directive verbatim + file list; they keep their own context, return a 200-word summary. Saved a lot of main-context budget.

### Codex via background bash

`codex exec --dangerously-bypass-approvals-and-sandbox --color never - < /tmp/T###-codex.txt > /tmp/T###-codex.out 2>&1` with `run_in_background: true`. Read only the gate verdict + findings via `grep -E "^GATE:|^\[CRITICAL|..."`. Don't tail the full output into main context — it's huge.

## Open threads

### T053 / L142 — gatekeeper Router seam, MID-RESCUE

**Status:** `blocked` with `blocked_reason={"exit_code":1,"kind":"rate_limit","reset_at":1778083200}` (Sonnet rate limit; resets ~11pm Bangkok).

**What's done (in T053 worktree at `feat/T053-auto-promoted-l142`, HEAD=`3eec7d5`):**
- P1: `intake` typed store schema + lifecycle scaffold
- P2: gatekeeper_decision_json validator + risk_taxonomy + field mirroring (3 commits, Sonnet)
- P3: 812 LOC `src/handlers/intake_route.rs` covering all 6 routing decisions; same-tx side effects; integration test (P3 PASS for phase 1; phase 2 PASS for cycle 3 of 3); orchestrator-mechanical commit `3eec7d5` fixing the `is_none()` vs `Some(Value::Null)` bug Sonnet self-diagnosed before runner exit.

**What's NOT done:**
- Phase 3 e2e shell test `tests/intake_routing_e2e.sh` has a Sonnet test-logic bug at Decision 1: assertion `OBS_COUNT==0` should be `==1` (source's `normal_observation` route correctly auto-creates 1 obs per AC3.6; the broken `is_none` made all auto-creates skip, masking this). Mechanical fix.
- Phase 3 final code-reviewer cycle on the post-fix scaffold (haven't re-driven since `3eec7d5`).
- Phases 4 & 5 of the contract (recon agent / needs_info round-trip; final wrap).

**User-presented options when this note was written** (Blake chose to swap back to all-Pi runner regardless and write this note):

(a) Wait for Sonnet quota reset, resume drive
(b) codex:rescue for phase 4-5 (pi-sanctioned fallback per msg_8e8cc971 option C)
(c) Try Pi runner with the existing 812-LOC scaffold (Pi failed phase 1 from blank slate but may handle polish + 4-5 with concrete code to read)
(d) Orchestrator fixes the e2e count assertion + drives Pi reviewer to bank phase 3, decide on 4-5 next
(e) Halt entirely

**Current config: all-Pi.** Restored at session-end per Blake. `.stores/config.yaml` is `default_runner: pi` everywhere. Backup of mixed config (executor=claude-code:sonnet) at `/tmp/stores-config-allpi-T053-backup.yaml`-style snapshots if you need them.

**Recommendation for next CC:** ask Blake which option to pick. If Sonnet quota has reset (now possibly true), (a) is cleanest. If still stuck, (b) is fastest. (c) is the experiment that costs least if it works.

### T034 — L150-class noise, pi-instructed don't-chase

`status: blocked` from a much earlier silent-zombie. Watchdog still spams about it on every poll (visible in daemon log; Monitor filter excludes T034/T050 watchdog noise). Pi's call: fold into L134/L135, don't chase. Leave alone.

### docs/engine-health.md needs end-of-day refresh

You own this file (per skill). Today's 8 ships need to be moved to ✅ in their layers; new observations L162/L163/L164/L165/L169 need to be added; T053 status flagged in-flight. Pi may also have framing edits queued — check `git log -- docs/engine-health.md` for any pi commits since `af55848`.

### L165 follow-up: blocked_reason taxonomy

T059 shipped a basic blocked_reason classifier (rate_limit/retry/dependency/user/deploy/stale/unknown). When more `kind` values surface in real blocked_reason JSON, add them to the dispatch table in `src/tui/data.rs::blocked_reason_class`. Not urgent.

### Stale stashes on the work tree

`git stash list` shows 3 stashes from today's rebase work. Pop only when needed — they hold render projections + logs + .tpl edits that are noise on main. After all in-flight tasks ship, `git stash drop` them.

## Architectural threads pi is owning

(I don't drive these; surfaced here so you know they exist.)

- **L150**: halt/deploy-blocked subscriber files merge-conflict-shaped observations against rows that are merely `blocked` by drive failure (e.g. T034 noise). Folded into L134/L135.
- **Gatekeeper Phase 2-5**: L142 (T053, in flight) → L143 (T052, shipped) → P3 dedicated `architecture_reviews` store → P4 fast-track execution (gated on L135 Check primitive) → P5 cluster registry. P3+ filed as deferred-followup obs by pi.
- **Verb-owned fields primitive**: codex T052 round 1 caught a generic-`update` bypass on `approval_policy`; fixed in-place but a generalized "verb-owned fields" primitive would prevent future re-occurrences. Pi flagged it as a follow-up obs candidate.

## Coordination state with 10.06 client

- 10.06 hit DDL drift on the binary update at 18:54 (T054 added `tasks.plan_source` column; their DB lacked it). I sent recovery (msg_d79020fb): run `stores migrate --apply` in their cwd. Their daemon was also stale-exe (L149); they restarted.
- 10.06's last message acked the recovery. They're 3-in-flight on their own backlog, unblocked from our side.
- Whenever a future ship adds a per-store schema column, ping 10.06 proactively before they hit it.

## Procedures NOT in the skill (but should be one day)

These were small, one-off. Worth promoting if they recur:

- **Restoring a config snapshot from /tmp.** Worked around per-task runner today; cost no doctrine. Skill captures the pattern.
- **Stale-deploy-blocked obs auto-filed by failed accept-merge** (L163/L164 today). Pattern: `stores observations close_as_addressed LXXX --resolution <merge-sha> --invoker ai_autonomous`. Resolution arg must be a valid task/obs ID or 7-40 hex char SHA — free-text gets rejected.
- **Manual ceremony fire when daemon is stopped.** `stores agents run --once --invoker ai_autonomous` runs one daemon iteration synchronously. Useful for accept-merge → cargo_install → schema_migrate when daemon is paused for some reason.

## Next session, in order

1. Skim this + the skill + engine-health.
2. Ack pi/10.06 threads with a "I'm here, picking up T053 rescue" message.
3. Decide T053 path with Blake (options above).
4. After T053 ships: `engine-health.md` refresh + `git stash drop` cleanup.
5. New work picks from the open obs queue per skill's *current priority doctrine*.

---

Pipe-fill ratio for today (after the morning's T050/T054/T052 trio): 5 concurrent drives, 4 shipped same-batch, 1 (T053) deep enough to need Sonnet rescue. Throughput target met. Blake's call about codex on T1 (skip) saved real time. The procedures are now in the skill so future sessions don't have to rediscover them.
