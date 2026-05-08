# Handover — engine-controller

**Date:** 2026-05-08
**Type:** handover
**Role:** engine-controller

## Active thread

`/home/blake/repos/.agent-comm/threads/2026-05-08-01-stores-2026-05-08-session.md`

Watching as `substrate-agent`. Reviewer-runner online (idle this session — substrate path-A auto-codex handled all reviews; see SOP gap below). Pi-architect online and active.

## Current responsibility

Driving T098 (T3, L480 watch cockpit) to acceptance. T098 is the last remaining WIP. All other engine work this session shipped or filed.

## Daemon / CLI health

- **Daemon:** PID 2082883, alive, `--detach`, `--log-file logs/agents-daemon.log`. Binary `/home/blake/.local/share/stores/bin/stores` (private install path). Self-reexec'd via cargo install during the session — exe symlink shows current binary (no `(deleted)`).
- **CLI:** `stores` on `~/.cargo/bin/stores`. Diverged from daemon's binary; that's fine — daemon uses private path.
- **Last cargo install:** commit `5519597` (legacy stale-base backfill).

## Active work / processes

| item | status | pid | worktree | branch | commit | next action |
|---|---|---|---|---|---|---|
| T098 (L480 cockpit, T3) | executing phase=5/5 cycle=2 next=executor | 4179281 | `stores-T098-auto-promoted-l480` | `feat/T098-auto-promoted-l480` | branch tip post-rebase #3 | drive cycle 2 addressing real REVISE on `mission_compact_window` (src/tui/render.rs:220-240). On next in_review, daemon's L488 rebase-before-codex will fire automatically. |
| ER330 (T098 attempt 3) | revise (verdict landed) | — | — | — | head_sha=4a828ce3 base_sha=5519597 | substrate progressed to executing on this REVISE; nothing to do. |

## Active Claude subagents

**None active.** Two subagents fired earlier in session and completed:
- Obs-backlog triage subagent → produced `docs/worklog/2026-05-08/04-obs-backlog-triage.md` (327 closures executed by a separate execution subagent; both completed)
- T098 cycle-3 revise subagent → committed test commits `68c5c32` + `84ec536`
- T103 head_sha-timing subagent → committed `4498637` (now in main)

No subagents to wait on.

## Today's main commits (high-context)

```
5519597 external_review: backfill next_retry_at on legacy stale_base_requires_rebase rows (unblock T098 ER330)
c8d993e external_review: bound stale_base_requires_rebase next_retry_at (direct unblock for T098)
e896817 Merge branch 'feat/T103-auto-promoted-l488'   ← L488 stale-base check live in production
6c5f13c parser: handle markdown-list revise markers (direct unblock for T103)
bf420c0 Merge branch 'feat/T104-auto-promoted-l496'   ← parser finding-marker inference
8294e68 Merge branch 'feat/T100-auto-promoted-l484'   ← rate-limit-aware retry
0d22adf Merge branch 'feat/T102-auto-promoted-l491'   ← ddl.rs emergency repair (T099 schema columns)
b06a644 Merge branch 'feat/T101-auto-promoted-l490'   ← parser leading-word-prose tolerance
cd137a1 Merge branch 'feat/T099-auto-promoted-l483'   ← cascade-dedup subscriber
```

Direct edits (`6c5f13c`, `c8d993e`, `5519597`) used the dogfood-escape doctrine per Pi rulings (msg_dbaab2dc, msg_3cf7c3af, msg_a423719b).

## Substrate state

- **Open observations:** 30 open + 18 ready + 1 investigating = 49 total non-terminal (down from 357 at session start; 327 closed via subagent triage cleanup using `close_as_addressed`).
- **Filed durable obs (NOT ratified):**
  - L489 (stale-binary-alive watchdog gap)
  - L492 (schema-yaml ↔ ddl.rs drift durability)
  - L497 (parser whack-a-mole — structured codex output / fixture-parser durability)
  - L498 (L488 recovery durability — operator-callable retry-after-rebase verb)
- **No external_reviews currently held.** ER330 transitioned in_review→executing on REVISE.

## Dirty worktrees / stashes

- **Three pre-rebase stashes** in repo (visible from any worktree's `git stash list`; stash list is global):
  - `stash@{0}: T098 pre-rebase #3 projection drift` (T001 test-task md only — safe to drop)
  - `stash@{1}: T103 pre-rebase #2 projection drift` (same — safe to drop)
  - `stash@{2}: T098 pre-rebase #2 projection drift` (same — safe to drop)
- Do NOT drop until verified clean — recommend next agent inspect once T098 lands.
- **Working tree drift:** `.claude/skills/handover-engine-controller/SKILL.md` and `.claude/skills/pi-architect/SKILL.md` still show modified from yesterday's wind-down (untouched this session). `tasks/active/T001-test-task/main.md` and `tasks/planning/T001-test-task/main.md` are persistent projection drift. Untracked: `logs/`, `T801-T803` test-task scaffolds, `T053-auto-promoted-l142` paused dir, `T043-auto-promoted-l124` planning dir.

## Do not do

- Do NOT raw-SQL writes against `.stores/db.sqlite` (CLAUDE.md doctrine; `sqlite3 ... SELECT` reads are fine).
- Do NOT `git add -A` / `git add .` — name paths explicitly. Several untracked test-task scaffolds and planning dirs would sweep in.
- Do NOT push origin/main forward without Blake's deliberate decision (origin diverged since 2026-05-04; many merges local-only).
- Do NOT ratify any of L489, L492, L497, L498 without Blake/Pi explicit blessing — those are durable follow-ups, not next-task fodder.
- Do NOT drop the three stashes without inspection.
- Do NOT spawn a new T098 executor subagent unless T098 hits the same REVISE 3+ times in a row (per the SKILL revise-brief discipline). The current cycle 2 should address the `mission_compact_window` finding naturally.
- Do NOT widen WIP until T098 lands or Blake explicitly opens the gate.

## SOP gap to surface in next session

**Reviewer-runner has been idle the entire session.** Substrate path-A (daemon-spawned codex via T083 external_reviews lane) handled every T2/T3 review automatically. Reviewer-runner waited for explicit pings that never came. The engine-controller SKILL still tells me to dispatch reviewer-runner with structured pings — that pre-dates T083's substrate-native lane. Either (a) update the SKILL to mark reviewer-runner as observability-optional / cross-check-only, or (b) disable daemon path-A and route everything through reviewer-runner (path-B). Blake's call.

## First step for next agent

1. **Watch T098.** Check `stores tasks status T098`. If still cycling, monitor:
   - `executing → code_review → in_review` (ER spawn, this time with L488 auto-rebase)
   - On in_review, ER will fire automatically; codex returns PASS/REVISE on real findings (rebase-before-codex now in production)
   - If REVISE: drive cycle continues; if PASS: verify scope (`git diff --stat $(git merge-base feat/T098-auto-promoted-l480 main)..feat/T098-auto-promoted-l480` should show TUI-scoped files only) then `stores tasks accept T098 --invoker ai_with_human --approve-token <T>`.
2. **Re-arm monitors.** The diff-on-change Monitor and 15-min backup-scan Monitor in the prior session won't carry over. Re-arm using the patterns in `.claude/skills/engine-controller/SKILL.md`.
3. **Verify daemon health** before any nudges: `pgrep -af 'stores agents run'` + `ls -la /proc/$(grep ^PID= .stores/agents.pid | cut -d= -f2)/exe`.

## Notes

- Token is durable per-host at `~/.config/stores/approve.token`. Read via `stores auth show` if needed.
- Pi was responsive and authoritative this session — every architectural decision routed through her cleanly with quick acks. Use `agent-comm send ... --to pi --priority high --needs-ack --response-requested` for design calls.
- The stale-base + stale-binary + parser whack-a-mole pattern dominated the session. T103 (L488) closes stale-base going forward; L489 (stale-binary watchdog) and L497 (parser durability) are the next two engine-health priorities once T098 lands.
- 6 substrate tasks shipped + 3 direct-edits + 327-row obs cleanup + 4 durable obs filed. Big day.
