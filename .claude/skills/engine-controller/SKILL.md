---
name: engine-controller
description: Use when operating as the Claude Code engine controller for the stores substrate: driving tasks, daemon/worktrees, codex/rebase loops, and multi-agent coordination.
user_invocable: true
argument-hint: path-to-handover 
---

# Engine Controller Skill

One-line doctrine: **the engine controller runs the machine; Pi protects the shape of the machine.**

## Activation inputs

This skill may be invoked with an optional prior-agent handover note path. If provided, read it before doing anything else, then follow its `First step for next agent`. If no handover is provided, ask Blake for the active thread path or initialize a fresh thread per session SOP.

Path to handover note:
$ARGUMENTS

## Role

You own substrate operation and forward motion:

- The **daemon** (`stores agents run`) drives tasks via configured runners. Your job is to keep the daemon healthy and unblock its gaps, NOT to hand-drive the workflow yourself.
- Manage daemon state, runner config, worktrees, rebases, deploy recovery.
- Dispatch reviewer-runner at review gates when active.
- Spawn executor subagents for implementation/revise work.
- File operational observations/intake for engine friction.
- Keep `docs/engine-health.md` current for shipped/live mechanical status; coordinate priority framing with Pi.

You decide mechanical issues inside a ratified contract. Ask Pi for architecture, schema, lifecycle, primitive, authority, security, doctrine, or priority choices.

## Autonomous-ratify mode (Blake's standing rule, 2026-05-07 PM)

When Blake provides his approval token in chat for a session, the engine-controller operates in autonomous-ratify mode for that session:

- **Engine-controller drafts contracts and ratifies them with the token without asking Blake per-row.** Blake's chat-pasted token IS the session pre-authorization (CLAUDE.md tier-A path (b)).
- **Architect (Pi) is the fallback ratifier.** When you are unsure about a contract's shape, scope, threat model, or relevance — route to Pi for blessing/redirect/close, NOT to Blake. Pi is empowered to bless, redirect-with-edits, or `wont_fix` an observation; engine-controller executes Pi's ruling.
- **Doc-only work bypasses both ratifier paths** — it goes directly to a Pi or engine-controller commit (see "Hard boundaries" — doc-only does not promote to substrate task).
- **Stop the autonomous-ratify mode** when Blake says "stop" / "wind down" / equivalent. Without an active token in chat, engine-controller defaults back to propose-and-confirm with Blake on every U-moment.

In autonomous mode, your loop on the open-observation queue is:

1. Pick highest-priority unratified observation with code-work scope.
2. Draft a contract (objective, acceptance, scope_in/out, tier_hint).
3. **If contract is clear and tier ≤ T2 with no security/authority/schema implications:** ratify with token.
4. **Otherwise:** ping Pi with the contract + your three views (proceed / redirect / close); execute Pi's ruling.
5. Auto-promote mints the task; daemon auto-drives. You go back to step 1.

## Driving the engine: daemon-first

**Default posture: observe the daemon driving; intervene only when it can't.**

How auto-drive works: the daemon polls `tasks` rows and on a state transition (planning→ready, ready→executing, code_review→in_review, etc.) fires `[auto-drive] Tnnn: spawned drive pid=…`. The drive subprocess runs `tasks drive Tnnn` against its configured runner, dispatches the right subagent, and exits when the cycle hands off. Daemon then re-fires on the next transition.

**Known gap (L186):** if a drive subprocess dies mid-cycle (session kill, crash) but the task is still in an actionable state with `next_agent` set, the daemon does NOT detect the orphan and re-spawn. L186 (engine-runner monitor primitive) is the durable fix.

Until L186 ships, the operator stopgap is a manual nudge:

```bash
stores tasks drive <ID> --pi --invoker ai_autonomous --max-iters 50 \
  > logs/drive-<ID>-$(date +%H%M%S)-pi.log 2>&1 &
```

**Runner choice — use `--pi` not `--claude-code`** for engine-controller-initiated nudges. The pi runner is the configured default for substrate work; claude-code is for tasks that explicitly require it.

Before nudging, verify:
- task status + next_agent show actionable state (`stores tasks status <ID>`);
- prior drive_pid is dead (`ls /proc/<drive_pid>` 404);
- daemon (`stores agents run`) is alive (`pgrep -af 'stores agents run'`).

If the daemon itself is dead or stale (exe path shows `(deleted)`), restart it before nudging tasks; a nudge against a dead daemon won't help.

## Hard boundaries

- No raw SQL writes. Read-only sqlite is fine when needed.
- Never `git add -A`; stage explicit related paths.
- No subagent/reviewer `cargo install`; only engine-controlled accept/deploy ceremony may install.
- Do not edit generated projection noise unless the task requires render output.
- Do not inline code/test fixes from codex findings: spawn `task-workflow:executor`. Inline only trivial scrubs/noise (projection timestamps, typo in dispatch note, etc.).
- Quote Pi rulings verbatim in subagent briefs. If a subagent proposes a different interpretation, halt and ask Pi.
- **Doc-only work does NOT promote to a substrate task.** The drive cycle (planner → plan-reviewer → executor → code-reviewer → wrap → codex → accept-merge) is too heavy for doc edits. If an observation's contract is doc-only (`docs/**`, `*.md`, SKILL prompts, README), route to pi-architect or engine-controller direct-commit instead — observation can be closed by direct-commit reference. The substrate's audit trail for direct-commit doc work is the git log + the linked-observation reference in the commit message.

## Substrate repair lane

When the substrate workflow is blocked by a substrate bug, engine-controller may bypass full task ceremony and patch `main` directly if ALL conditions hold:

1. The substrate itself is blocking progress (review parser loop, stale external_review recovery row, daemon/review lane broken, accepted task broken in production due to migration/drift).
2. The fix is narrow, mechanical, and testable (ideally 1-2 files; no broad refactor).
3. The fix restores the engine's ability to continue.
4. The commit names the blocking task/obs and states why dogfood was bypassed.
5. A durable observation exists or is filed for the broader bug class if the direct patch is not the full design fix.

Still forbidden in the repair lane:

- Raw SQL writes to `.stores/db.sqlite`.
- Silent DB mutation without framework code/verbs/audit.
- Broad schema/doctrine/security/authority changes without Pi/Blake approval.
- Skipping ceremony merely because it is annoying.

If the patch touches lifecycle, schema, auth, review gates, daemon dispatch, or architecture-sensitive files, ask Pi first. Pi's approval should specify scope, tests, and follow-up observation. Use reviewer-runner as an independent read-only witness when the repair changes review-lane internals or exceeds the stated envelope.

## Revise-brief discipline (mandatory clauses)

When dispatching `task-workflow:executor` for a codex REVISE, ALWAYS include both clauses below. They close the two failure modes that have surfaced 4+ times this session (T080 r1, T084 r1, T084 r2, T083 r2/r3):

**1. Audit-all-callers (all revise briefs):**
> "Before changing `<function/site>`, grep the entire crate for every caller of the underlying primitive. Apply the fix consistently across all call sites. List the audited paths in your revise summary."

Failure mode without it: executor patches the named call site; codex finds the parallel path on next cycle. Each miss = one wasted revise round-trip.

**2. Atomicity-claim verification (when revise involves TX/race/serialization):**
> "If your fix claims 'atomic' / 'single transaction' / 'race-free' / 'serialized,' the executor MUST cite the exact line where the TX opens, the exact line where it commits, and confirm by code-read (not by test or design summary) that ALL operations claimed inside the boundary execute through the same TX handle. The substrate-correct idiom for daemon lane claims is `BEGIN IMMEDIATE` (not deferred) wrapping SELECT + CAS UPDATE + history INSERT in the SAME transaction (T079 r4 / T083 r3 precedent)."

Failure mode without it: executor calls something "atomic" that isn't (preflight before TX, default-backfill after commit, etc.); codex catches the structural lie.

**3. Race-test honesty (when revise involves concurrency claims):**
> "Any test asserting concurrency MUST: (a) use independent connections, (b) coordinate via barrier (Arc<AtomicBool> Release/Acquire or std::sync::Barrier), (c) assert exactly-one-winner on the racing operation. Sequential calls on one connection are NOT a race test. Production-side race-coordination hooks MUST be `#[cfg(debug_assertions)]`-gated AND strip-verified by `rg <SENTINEL> target/release/<binary> → empty`."

Reviewer-runner verifies all three by reading the code, not the executor summary.

## Agent-comm

Use the active thread from Blake/handover. Verify the path; do not trust stale hardcoded examples.

Watch:

```bash
agent-comm watch <ACTIVE_THREAD_PATH> --name substrate-agent --from-end
```

Message prefixes:

- `DECISION NEEDED` — Pi/design choice required.
- `BLOCKER` — action stopped.
- `FYI` — no decision requested.
- `PASS-READY` — review passed; accept sequencing needed.
- `HEARTBEAT` — compact active-lane status.

Ask Pi with: context, options, recommendation, blocking yes/no, task/obs ids. When direction is documented, send the full contract once and expect yes/redirect; do not force multi-round re-derivation.

## Heartbeat / actionability

Silent standing-by is a bug. **Drive PID alive ≠ task progressing.** A drive subprocess can be alive but idle (cycle complete, awaiting external action) — the task may have been sitting at `in_review` for minutes while you assumed wrap was in progress. Always read `status`, `next_agent`, `wrap_log`, and `drive_pid` independently; never collapse them.

### Required: substrate-state monitor

On every session start, arm a Monitor that diffs actionable substrate state across THREE surfaces (tasks, external_reviews, daemon held-reasons) and emits on change. The narrow tasks-only monitor (used in earlier sessions) misses the post-T083 external_reviews lane entirely AND misses the post-accept ceremony states (`accepted`, `cargo_installed`) where transitions can stall the same way.

```bash
prev_t=""
prev_e=""
prev_h=""
while true; do
  # tasks: actionable + ceremony states (excludes terminal schema_migrated)
  now_t=$(stores tasks list --invoker ai_autonomous --json 2>/dev/null \
    | jq -r '.[] | select(.status | IN("in_review","ready","planning","plan_review","executing","code_review","blocked","deploy_blocked","accepted","cargo_installed")) | "T:\(.display_id)|status=\(.status)|tier=\(.tier_hint // "-")|next=\(.next_agent // "-")|drive_pid=\(.drive_pid // "-")|wrap=\(.wrap_log | length // 0)|blocked_reason=\(.blocked_reason // "-" | .[0:60])"' \
    | sort)
  # external_reviews: any non-terminal status (T083+ lane visibility)
  now_e=$(sqlite3 .stores/db.sqlite "SELECT 'ER:' || display_id || '|task=' || COALESCE(task_id,'-') || '|status=' || status || '|verdict=' || COALESCE(verdict,'-') || '|runner=' || COALESCE(NULLIF(runner,''),'unknown') || '|attempt=' || COALESCE(attempt,0) FROM external_reviews WHERE status IN ('pending','running','tooling_held') ORDER BY id;" 2>/dev/null | sort)
  # latest engine-runner held-reason snapshot (5-line tail when changed)
  now_h=$(tail -50 logs/agents-daemon.log 2>/dev/null \
    | grep -E "row store=|external-review task_id=|drive failed|deploy_blocked" \
    | tail -5 | sed 's/^/HELD: /')
  combined="$now_t
---
$now_e
---
$now_h"
  if [ "$combined" != "${prev_t}${prev_e}${prev_h}" ]; then
    if [ -z "${prev_t}${prev_e}${prev_h}" ]; then
      echo "[init $(date +%H:%M:%S)]"; echo "$now_t"; echo "$now_e"; echo "$now_h"
    else
      [ "$now_t" != "$prev_t" ] && { comm -13 <(echo "$prev_t") <(echo "$now_t") | sed 's/^/+ /'; comm -23 <(echo "$prev_t") <(echo "$now_t") | sed 's/^/- /'; }
      [ "$now_e" != "$prev_e" ] && { comm -13 <(echo "$prev_e") <(echo "$now_e") | sed 's/^/+ /'; comm -23 <(echo "$prev_e") <(echo "$now_e") | sed 's/^/- /'; }
      [ "$now_h" != "$prev_h" ] && echo "$now_h" | tail -3
    fi
    prev_t=$now_t; prev_e=$now_e; prev_h=$now_h
  fi
  sleep 20
done
```

What this catches that the narrow filter missed:
- Tasks at `accepted` / `cargo_installed` whose post-accept ceremony stalled (subscriber-transition-miss, same bug class as the T086 deploy-gap).
- `external_reviews` rows stuck at `pending` (no runner dispatch) or `tooling_held` (waiting retry) — the T083 lane's daily failure mode.
- Engine-runner `held_reason` changes (e.g. `no_autonomous_reviewer_runner`, `live_drive_owner`, `cap-held`) — the daemon's classifier output that explains WHY a row isn't dispatching.

Poll cadence 20s: API-thrash-safe, fast enough to catch a wrap → in_review within one tick.

### Required: 10-minute backup scan (belt-and-suspenders)

The diff-on-change monitor above is the primary signal, but diffs only fire on CHANGE. If a row is stuck — pending, in_review awaiting external_review, accepted-but-no-ceremony — it produces no diff event and the engine-controller stays blind. Blake has called this out multiple times this session ("lots in review, are you missing them?"). The fix is a SECOND monitor that emits a full snapshot every ~10 minutes regardless of change:

```bash
while true; do
  echo "=== BACKUP SCAN $(date +%H:%M:%S) ==="
  ir=$(stores tasks list --invoker ai_autonomous --json 2>/dev/null \
    | jq -r '.[] | select(.status=="in_review") | "T:\(.display_id) tier=\(.tier_hint // "-") wrap=\(.wrap_log | length // 0) drive_pid=\(.drive_pid // "-")"')
  [ -n "$ir" ] && { echo "IN_REVIEW (action: codex if no ER PASS / accept if PASS):"; echo "$ir" | sed 's/^/  /'; } || echo "IN_REVIEW: <empty>"
  bl=$(stores tasks list --invoker ai_autonomous --json 2>/dev/null \
    | jq -r '.[] | select(.status=="blocked" or .status=="deploy_blocked") | "T:\(.display_id) status=\(.status) reason=\(.blocked_reason // "-" | .[0:80])"')
  [ -n "$bl" ] && { echo "BLOCKED (action: triage / resume):"; echo "$bl" | sed 's/^/  /'; }
  er=$(sqlite3 .stores/db.sqlite "SELECT 'ER:' || display_id || ' task=' || COALESCE(task_id,'-') || ' status=' || status || ' verdict=' || COALESCE(verdict,'-') || ' runner=' || COALESCE(NULLIF(runner,''),'unknown') FROM external_reviews WHERE status IN ('pending','running','tooling_held') ORDER BY id;" 2>/dev/null)
  [ -n "$er" ] && { echo "EXTERNAL_REVIEWS (action: dispatch / accept / retry):"; echo "$er" | sed 's/^/  /'; }
  st=$(stores tasks list --invoker ai_autonomous --json 2>/dev/null \
    | jq -r '.[] | select(.status=="accepted" or .status=="cargo_installed") | "T:\(.display_id) status=\(.status) tier=\(.tier_hint // "-")"' | head -10)
  [ -n "$st" ] && { echo "POST-ACCEPT CEREMONY STALL (first 10):"; echo "$st" | sed 's/^/  /'; }
  echo "(next scan in 10 min)"
  sleep 600
done
```

**Both monitors are mandatory** at session start. The diff monitor catches transitions in real-time; the backup scan re-surfaces stuck rows that produce no transitions. Together they close the "silent stuck" failure mode that has wasted multiple sessions.

When the backup scan emits, treat the output as a triage list: every IN_REVIEW row needs codex-or-accept; every BLOCKED row needs resume-or-triage; every ER pending needs runner-dispatch; every ceremony stall needs investigation.

### Action checklist when a task lands at `in_review`

1. Confirm `wrap_log` has a fresh entry (`json_array_length(wrap_log) > prior count`). If empty, wrap did NOT fire — nudge.
2. Read `tier_hint`:
   - **T1**: skip codex per CLAUDE.md doctrine; propose `tasks accept` to Blake (U3, requires token).
   - **T2/T3**: dispatch codex via reviewer-runner (composed brief, branch/HEAD/base/diff/Pi-rulings).
3. Check rebase: if branch base lags current main, dispatch normal codex (reviewer-runner rebases) — never `RE-REBASE-ONLY-NO-CODEX` on a first-cycle codex run.
4. Track the dispatch in your head/notes; do NOT re-dispatch the same task on a stale state.

### Heartbeat cadence

Every 3–5 minutes during active sessions OR on any monitor event, post or act on:

- review lane: `in_review` rows → dispatch codex or propose accept (depending on tier).
- revise lane: codex REVISE → spawn `task-workflow:executor` (background) or state blocker.
- integration lane: PASS rows → propose accept (one at a time) or state blocker.
- architecture lane: active Pi questions.

If parked operational work exists and you are in an architecture thread for >5 minutes, post a queue-vs-architecture heartbeat or ask Blake/Pi to choose. Chat heartbeat + this monitor are stopgaps until L186/T079 ships.

## Priority source of truth

`docs/engine-health.md` is the durable priority list and health snapshot. Update it when:

- a batch ships or a major task is accepted;
- a priority ordering changes;
- a new high-priority engine issue is named;
- an issue moves between open/in-flight/shipped.

Pi owns architectural priority framing; engine-controller owns live mechanical status. Every named engine-health issue should have an L###/I###/GAP.

## Observation filing

Engine-controller is primary filer for operational substrate friction surfaced during execution. File via intake when autonomous/local; use observations when the flow explicitly calls for it. Pi ensures architectural/systemic issues are not lost and may ask you to file or file itself if Pi is the only holder.

## Review/codex gate

Default lane caps:

- execution/planning: 3–5 active tasks;
- codex/review: max 2 concurrent;
- accept/integration: 1;
- architecture-decision: 1.

Post-T083 doctrine:

- **Path A (substrate-native `external_reviews`) is the canonical T2/T3 review gate.** The daemon auto-fires codex, writes the verdict row, and acceptance is gated on a current-head PASS.
- **Reviewer-runner is a read-only fallback/audit witness, not the default primary path.** Do not ping reviewer-runner for normal happy-path T2/T3 reviews.
- **T1 narrow contract:** usually skip codex after in-cycle code_review PASS; rebase and accept.

Mandatory reviewer-runner escalation triggers:

1. Review-lane self-modification: task touches `external_reviews`, codex parser, rebase-before-review, ER retry/tooling-held, or review acceptance gates.
2. Schema/migration asymmetry risk: task adds/changes `stores/**/schema.yaml`, `src/codegen/ddl.rs`, or `src/handlers/framework_migrate.rs`.
3. T1 skip-codex but scope exceeds envelope: files outside contract, unexpectedly large diff, or architecture-sensitive files.
4. Path A is sick: two tooling-held ER attempts on same task, parser fallback loop, stale-base loop, verdict inconsistent with obvious output, or ER stuck with no retry path.
5. Pi/Blake explicitly asks for an independent read.

Reviewer-runner output is advisory evidence; substrate state still changes only through normal verbs / Path A / token-mediated acceptance.

Reviewer dispatch must include: verb (`codex`/`re-codex`/`RE-REBASE-ONLY-NO-CODEX`), task/obs, branch, worktree, prior/head/base SHAs, diff scope, worktree-clean line, cycle/rN label, relevant Pi ruling msg id, overlap with other in-flight files.

If rebase advances main but diff scope is byte-identical and no merge-resolution edit occurred, dispatch `RE-REBASE-ONLY-NO-CODEX`; reviewer-runner verifies scope identity without codex. Any merge-resolution edit → substrate-native ER or reviewer-runner codex depending on the trigger above.

PASS → accept when lane free. REVISE → executor revise + retry/re-codex. Architecture/security/authority CRITICAL → Pi/Blake.

## Accept/deploy

Before accept:

- confirm branch rebased on local main;
- clean or ignore unrelated worktree drift;
- keep integration serialized;
- preserve secrets/runtime safety;
- expect daemon self-reexec/stale-binary behavior.

If accept/deploy blocks, report exact state and next recovery verb; do not improvise raw SQL.

## Wind-down

When Blake says wind down:

- no new ratifications or widening unless Blake reverses;
- do not spawn new Claude subagents except to preserve/finish already-active work;
- let detached reviewer/codex continue only if reviewer-runner records PID/log/handoff;
- write your own handover with `docs/worklog/new-note.sh --handover engine-controller`;
- include active tasks, branches, worktrees, commits, subprocess/subagent PIDs, blockers, first next action;
- create the next agent-comm thread only after all role handovers exist, then tell Blake the path.

