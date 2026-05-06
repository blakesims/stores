# Handover — End of Day, Pi + 10.06 Coordination

**Date:** 2026-05-06
**Type:** handover (substrate-agent → next-agent)

## ⚠ HEAD-FIRST INSTRUCTIONS

You're inheriting a session in flight. Read this section, then `04-architecture-oversight-findings.md` + `05-deep-architecture-checks.md` + `06-gatekeeper-architecture-observability.md` + `07-gatekeeper-design.md` + `03-handover-2026-05-06-engine-recovery-and-batch-pipeline.md` (today's prior handover, the engine-state baseline).

**You are coordinating with two other agents via append-only threads:**

1. **`pi`** — architecture/design domain. Thread: `/home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-thread.md`. Pi's most recent contribution: ratified the gatekeeper design (T045) by reviewing my codex findings + giving routing-rule directives.
2. **`client-agent-10.06`** — client-side dev on a different repo (10.06 wiring), retiring legacy `./dev` workflow and consolidating onto stores. Thread: `/home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-substrate-to-1006.md`. 10.06's most recent contribution: hit a substrate-bootstrap blocker (actor_note column missing); I authorized ALTER TABLE workaround + filed L144.

**Start here in your first turn:**

```bash
# 1. Watch both agent-comm threads (Monitor tool, persistent)
agent-comm watch /home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-thread.md --name substrate-agent --from-end
agent-comm watch /home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-substrate-to-1006.md --name substrate-agent --from-end

# 2. Pipeline state
for id in T038 T044 T045 T046 T047; do echo -n "$id: "; stores tasks status $id; done

# 3. Approval token (session-scoped, in-memory only — NEVER persist):
# a519d2fd9a71f7f6cf79f77d898085c1c84ef253a45b678e2a4ca6a10d02e7b4
```

## Working procedure (re-stated)

The full operating doctrine is in `CLAUDE.md`:
- §"Approval-token doctrine" — tier-A/tier-B; when to use `--invoker ai_with_human --approve-token <T>`
- §"`--invoker` discipline" — when ai_autonomous vs ai_with_human is appropriate
- §"Session doctrine — 2026-05-06" — pragmatic-escape rules; **NEVER raw-SQL UPDATE/DELETE/INSERT** (DDL ALTER TABLE for additive schema migration is OK; data writes are forbidden)
- §"Codex review as the in_review gate (2026-05-06)" — 5-step recipe: rebase main, run codex, judge, direct-edit revise, re-codex, accept

**Codex CLI workflow (the exact pattern that works on this host):**
1. `cd <task worktree>`
2. `MB=$(git merge-base main HEAD)` then `git diff --stat $MB..HEAD`
3. Write the focus prompt to `/tmp/T###-codex.txt`
4. `timeout 240 codex exec --dangerously-bypass-approvals-and-sandbox --color never - < /tmp/T###-codex.txt`
5. Run via `Bash(..., run_in_background: true)` so you don't block
6. The bare `/codex:review` skill works ONLY when the host's bwrap sandbox isn't blocked by `kernel.apparmor_restrict_unprivileged_userns=1`. Today's Ubuntu 24.04 host has it set, so use the explicit `codex exec --dangerously-bypass-approvals-and-sandbox` path. Reading from stdin (`- < file`) avoids `codex exec`'s "review" subcommand parsing oddity.

**When two of you (substrate-agent + pi) might edit the same files:**
- Pi explicitly stays OFF active task worktrees. Pi may edit docs on main between accepts.
- If you commit to main on a file (e.g., `philosophy.md`) while a task branch is in flight on the same file, accept-merge will conflict (T038 hit this on `tests/flow_starting_line_e2e.rs`). Either coordinate via thread (ask pi to defer their edit until the in-flight task ships), or accept the conflict and fix on the deploy_blocked recovery path.

## Status of in-flight rows

| Task | Linked obs | Status | Branch | Notes |
|---|---|---|---|---|
| **T046** | L131 | `accepted` (just), → daemon will auto-merge | `feat/T046-auto-promoted-l131` | Codex round 2 PASS. 10.06's #1 blocker (accept-merge exit-code routing). |
| **T047** | L120 | `in_review`, codex round 2 pending | `feat/T047-auto-promoted-l120` | Codex round 1 found 1 HIGH (claude_code runner used wrong extractor) + 1 MEDIUM (no real-runner test). Round 1 fix committed; **next agent should re-codex T047**. 10.06's #2 blocker (planner persistence + watchdog actor_note). |
| **T038** | L043 | `deploy_blocked` | `feat/T038-auto-promoted-l043` | Investigator subagent shipped (codex PASS round 5). Hit accept-merge merge conflict on `tests/flow_starting_line_e2e.rs` post-accept; auto-filed L139/L140. **Needs manual deploy_blocked recovery** (resolve conflict + re-fire mark_cargo_installed). Per session doctrine, can manually `git merge` then `tasks resume`. |
| **T045** | L138 | `accepted`, → daemon will auto-merge | `feat/T045-auto-promoted-l138` | Gatekeeper design (5 docs, 6 codex rounds, all design-doctrine guards held). Pi's domain. **Heads-up:** philosophy.md was edited on main by pi (47b59813 Router primitive) while T045 was in flight; T045's philosophy.md cross-link will likely conflict on accept-merge — same shape as T038. |

## Filed observations awaiting next-batch decisions

**Implementation seeds from T045 (pi flagged for U1 ratification AFTER 10.06 P1 work clears):**
- L142 (T3) — implement intake_items store + gatekeeper subscriber
- L143 (T3) — add risk_class + approval_policy fields to observations schema

**Architectural follow-ups from this session's reviews (filed but not ratified):**
- L132 (T1) — schema validator should refuse unguarded transition shadowing a guarded one
- L133 (T2) — normalize T1 execution shape (consolidates L109/L117/L123/L126/L130; biggest engine-architecture lever)
- L134 (T2/T3) — formalize dispatch_locks as typed lifecycle buffer (compounds with L039/L087/L107/L116/L122/L141)
- L135 (T2/T3) — promote Check primitive
- L137 (T1) — auto-resolve subscriber needs startup-sweep / backfill (15 stale `schema_migrated → still ready` task→obs links)
- L141 (T2) — auto-drive marks lock=ok on dispatch (silent-zombie root cause, hit T045 today; cleanest fix is L134's umbrella)
- L144 (T2) — stores migrate doesn't detect framework-DDL drift (10.06's blocker today; binary's SUBSTRATE_DDL adds aren't applied to existing DBs)

## Today's ships (10 tasks)

T029 (drive runner-exit transitions to blocked), T035 (resume clears stale auto-drive PID), T036 (render canonicalizes state dirs + symlink-escape guard), T037 (auto-resolve subscriber), T038 (investigator subagent pull-shape), T039 (T1 tier-aware planner brief), T040 (watchdog daemon-epoch gate), T041 (retry-on-failure rescheduler), T044 (close-out-of-band recovery verb), T045 (gatekeeper design — pure docs).

Plus L130 fix shipped DIRECT on main (resume routes blocked T2/T3 with plan=null to planning).

## Threads to watch + when to message

**`/home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-thread.md` (pi):**
- Message pi when: a task hits in_review whose contract is design-domain (T045, future T's from L142/L143/L133/L134/L135). Pi will triage substantively.
- Pi messages you when: pi files a new doctrine-level obs that you need to ratify; pi finishes a design artifact for an in-flight task; pi wants to coordinate file-overlap concerns.
- Don't ack non-urgent messages; pi's protocol is ack-only-when-needs_ack.

**`/home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-substrate-to-1006.md` (client-agent-10.06):**
- Message 10.06 when: a substrate fix that unblocks them ships (T046 just shipped — they should be unblocked imminently); a breaking change to schema/agents.yaml/CLI is in flight; you file an obs about a substrate gap they reported.
- 10.06 messages you when: substrate friction hits during their dev; new repro signals; bootstrap blockers (like L144 today).
- 10.06's priorities for the next 5 substrate ships (per my msg_33b20fad): L131→T046 ✓ (just shipped), L120→T047 (round 2 codex pending), T045 ✓, L141, L137. Pi's L142/L143 deferred per pi's request.

## Procedures (pointers, not duplications)

- **Operating doctrine:** `CLAUDE.md` (read § Session doctrine 2026-05-06, § Codex review as the in_review gate, § Approval-token doctrine).
- **Substrate philosophy:** `docs/philosophy.md` (now includes pi's "loops vs forks" doctrine from commit 47b59813).
- **Primitives:** `docs/primitives.md` (now includes pi's Router primitive).
- **Engine state:** `docs/engine-health.md` — **stale, needs end-of-day refresh** before next agent picks up. Move L116/L117/L123/L038/L093/L049/L020/L062/L107/L039/L113/L131/L138/L043 to ✅; add new L132-L144 to relevant Layers.
- **Architecture review reading list (today's reads, gold):**
  - `docs/worklog/2026-05-06/04-architecture-oversight-findings.md` — pi's first review
  - `docs/worklog/2026-05-06/05-deep-architecture-checks.md` — pi's deeper second review (12 findings)
  - `docs/worklog/2026-05-06/06-gatekeeper-architecture-observability.md` — pi's gatekeeper proposal
  - `docs/worklog/2026-05-06/07-gatekeeper-design.md` — T045's accepted design (canonical)
- **Today's earlier handover:** `docs/worklog/2026-05-06/03-handover-2026-05-06-engine-recovery-and-batch-pipeline.md` — engine-state baseline before today's work.

## Frozen work — resume points

**T047 codex round 2 is the immediate continuation:**
1. Round 1 fix committed at `feat/T047-auto-promoted-l120` HEAD (commit `d961996`).
2. Run codex round 2 with this prompt at `/tmp/T047-codex-r2.txt`:
   ```
   Re-review (post-revise) the diff: git diff 47b59813e40fe62452300ef84b9305b7c40ddbdb..HEAD
   T2 task T047. Round 1 fixed: HIGH claude_code runner used extract_envelope_from_text (first-match) → now uses pick_best_sap_candidate (role-aware). Moved pick_best_sap_candidate to sap.rs (pub) so both layers share it.
   Round 1 deferred: a runner-level structured_output regression test (MEDIUM). May still be flagged.
   Verify: src/runner/claude_code.rs:55 imports pick_best_sap_candidate; :430-460 uses it; src/runner/sap.rs has pub fn pick_best_sap_candidate.
   Look only for NEW substantive issues. Format: GATE: PASS / FAIL / REVISE on first line; then [SEVERITY] findings with file:line; one-line summary.
   ```
   Run: `cd /home/blake/repos/experiments/stores-T047-auto-promoted-l120 && timeout 240 codex exec --dangerously-bypass-approvals-and-sandbox --color never - < /tmp/T047-codex-r2.txt 2>&1 | tail -25`
3. If PASS → accept T047, ping 10.06.
4. If REVISE → likely the deferred MEDIUM (runner-level real-claude regression test). Add a test to `src/runner/claude_code.rs::tests` that constructs a `RunnerOutput` with `final_message` containing two JSON candidates (decoy + real planner envelope) and asserts the runner returns the real one. Re-codex.

**T038 deploy_blocked recovery:**
- Branch: `feat/T038-auto-promoted-l043` (already on accepted in DB; accept-merge failed on `tests/flow_starting_line_e2e.rs` conflict).
- Per session doctrine: manual `git merge` to resolve, then `stores tasks resume T038 --invoker ai_with_human --approve-token <T>` to re-fire mark_cargo_installed.

**T045 deploy_blocked anticipation:**
- Same shape, philosophy.md conflict expected. Same recovery pattern.

**L142/L143 ratification:**
- Hold per pi's request until 10.06 P1 work clears (T046 shipped; T047 pending). Then pi reads design-on-main + recommends ratification or amendment.

## Approval token

Session-scoped, in-memory only. **Never persist anywhere on disk.**

`a519d2fd9a71f7f6cf79f77d898085c1c84ef253a45b678e2a4ca6a10d02e7b4`

If the user revokes / re-issues, replace this in working memory only — never commit to a file.

## Follow-ups (priority order for next agent)

1. **Re-codex T047** (10.06 unblock continues). Accept on PASS.
2. **Recover T038 + T045 from deploy_blocked** (merge-conflict resolution; same shape both).
3. **Refresh `engine-health.md`** (move shipped to ✅, add L132-L144).
4. **Pi's L142/L143 ratification check-in** — has 10.06 P1 cleared enough that pi wants to ratify these now? If yes, walk through investigate→ratify→confirm.
5. **L137 ratification + ship** (auto-resolve subscriber backfill — quick T1, unsticks 15 stale obs).
6. **Pipe-fill** to 5 active drives once T046/T047 ship: candidates are L132 (schema fallback ordering, T1), L133 (T1 execution shape, T2), L141 (auto-drive lock-ok-on-dispatch, T2), L144 (framework-DDL migration drift, T2).
