# Handover Three Agent Sop Eight Ships

**Date:** 2026-05-07
**Type:** note (mid-day handover after three-agent SOP introduction)

## Summary

Session introduced and proved the **three-agent SOP**: pi-architect (governor) + substrate-agent (engine controller, mine) + reviewer-runner (read-only codex sensor). The split decisively broke the in_review stall pattern that previously cost 25+ min per task to detect-and-process. Session shipped 8 tasks (T064/T065/T066/T068/T069/T071/T073/T074) plus 4 in-flight in_review (T067/T070/T072/T075) parked mid-revise.

Session also surfaced and addressed two substrate-trust bugs:
- **L181** (CLI fail-silent): `stores --help` returned exit 0 with empty output after binary corruption.
- **L182** (recurring binary corruption): subagent test execution invoked `cargo install` without `--features runner-claude-code,runner-pi`, producing a 429K stub that overwrote `/home/blake/.cargo/bin/stores` 3 times in the session. T075 (D) ratified to ship daemon-side candidate-binary validation as the targeted fix.

## Details

### Three-agent SOP shape

- **pi-architect:** rules architectural forks via agent-comm thread `2026-05-07-01-stores-thread.md`. Sub-5-min response on most rulings; cascading multi-message clarifications.
- **substrate-agent (me):** owns ratification, U3 acceptance with token, codex finding triage (mechanical → subagent; arch → pi route), daemon health, observation filing.
- **reviewer-runner:** read-only codex sensor, watches in_review, rebases worktrees, runs codex against branch diff, posts decision-surface digests with severity-tagged findings + `pi-needed: yes/no` + Path-A metadata block.

Doctrine clean: reviewer-runner cannot write substrate state, commit code, run cargo install, hold the token, or make architectural rulings. Sensor + spawn-codex + post-thread only.

### Eight ships (chronological)

1. **T064/L175** — watch surface actionability cleanup (NeedsTriage section + actionable counts).
2. **T065/L151** — auto-investigator subscriber V1 (operator-pull, T3, PASS first try).
3. **T066/L149-followup** — daemon self-reexec on stale-exe (replaces L149 P1 fail-loud). PROVED IN PRODUCTION on T064 ceremony — daemon stayed up, same PID, --detach correctly stripped per pi Option A.
4. **T068/L179** — required_when parser OR / IN[...] support (T1).
5. **T069/L011** — daemon_starts table (concurrency-safe display_id via __pending_PID_SEQ placeholder).
6. **T071/L058** — stores metrics CLI (windowed REVISE-rate, percentile interpolation, volatile_window flag for bare duration windows per pi Option B).
7. **T073/L005** — observations update list-typed fields (multi-value support).
8. **T074/L015** — stores auth show --identity flag.

### In-flight at handover (4 in_review, parked mid-revise)

- **T067/L178** — manual-drive↔daemon handoff fix. Currently r6 subagent in flight (a078fe5c1e3c88844). Pi clarified A1-strict needs lifecycle closure: wrap_log NOT control sentinel, but next_agent must advance after current-cycle wrap (option D if state-machine handles, else A explicit handler update).
- **T070/L057** — agent_runs telemetry. Currently r6 subagent in flight (adc056d6803580952). Pi: spawn-fail must create attempted-invocation telemetry (synthetic agent_runs row with exit_code=-1 + source-layer model_id + .stores/runs/ error transcript). Mock under workspace .stores/runs only — no /tmp, no target/test-mock-runs.
- **T072/L059** — runs SQL VIEW. Currently r6 subagent in flight (aa6502fdc6dc30caa). Pi: backlink MUST be in same TX as dispatch_submit; HALT if too invasive.
- **T075/L182** — daemon candidate-binary validation. r2 subagent done (commit 8b88284); awaiting reviewer-runner re-codex.

### Substrate-trust bugs surfaced + addressed

- **L181 (CLI fail-silent):** filed; awaiting future task ratification.
- **L182 (recurring binary corruption):** root-caused (subagent cargo install without features); SOP rule shipped this session ("no subagent cargo install ever"); durable substrate fix ratified as T075 (candidate-binary validation before T066 self-reexec); operator workaround (cp from target/release/stores) used 3 times.

### Rebase-race storm

When 5+ branches in flight, every accept advanced main → in-flight branches needed rebase. ~30% of session lost to rebase resolutions across T067/T070/T071/T072/T074/T075. Multiple rebase rounds per branch (some cycled 3+ times with new conflicts each round). Lessons:
- Pause new ratifications when in_review queue is large (≥3) to reduce churn.
- Use **local main** as rebase base, not origin/main (origin lags as accepts are local-only).
- Post-rebase MUST diff-scope-check (`git diff --name-only main..HEAD`) before pinging reviewer-runner. Heavy resolutions (10+ conflicts) tend to absorb unrelated work.

## Follow-ups

### Tomorrow's pickup priorities

1. **Drain the parked in_review queue:** T067 r6, T070 r6, T072 r6 will have committed by next session start. Codex + accept (or revise → re-codex) each.
2. **T075 accept** if PASS — ships L182 daemon candidate-binary validation. After this, the recurring corruption pattern stops biting (daemon refuses to exec stub binaries).
3. **Path A — codex-as-subscriber** (pi's medium-term: codex review becomes a substrate primitive on in_review entry). Reviewer-runner role is the prototype for this.
4. **Private install path doctrine** (pi's longer-term: substrate runtime decouples from operator's ~/.cargo/bin to eliminate L182 root cause structurally).

### State at handoff

- **Daemon:** running PID 753160, fresh exe at /home/blake/.cargo/bin/stores (post-T074 cargo install).
- **Pipeline:** 0 active drives. 4 in_review (T067/T070/T072/T075) with subagents in flight.
- **Token:** in conversation memory. Next session needs Blake to re-paste OR `--invoker human` for tier-A.
- **Stash:** carried over from prior session (one stash, content uncertain — review before drop).
- **Worktrees:** all 8 shipped tasks merged; their feat/ branches remain. Worktree dirs at /home/blake/repos/experiments/stores-T0XX-... still exist.
- **Agent-comm thread:** `/home/blake/repos/.agent-comm/threads/2026-05-07-01-stores-thread.md` is the canonical record. Pi consolidating session learnings into skill updates this turn.

### What NOT to do next session

- Do NOT raw-SQL the substrate DB.
- Do NOT delete any tasks/active|paused|planning/*/main.md projections from a feature branch (scope-creep — those are residual but durable).
- Do NOT run `cargo install` from any subagent or test path. Use `target/release/stores` from the worktree if a binary is needed.
- Do NOT accept tasks while ≥5 branches are in_review (rebase storm).
- Do NOT re-codex T071 with the bare-duration stability finding — pi adjudicated it as false positive (Option B + volatile_window=true is the accepted shape).

### Reading order for next CC session

1. This note (head-first context).
2. `docs/engine-health.md` (pending pi update with session learnings).
3. `.claude/skills/engine-controller/SKILL.md` (pending pi update with three-agent SOP + lane-cap doctrine).
4. `.claude/skills/reviewer-runner/SKILL.md` (new this session).
5. `.claude/skills/pi-architect/SKILL.md` (pending pi update).
6. `.claude/skills/session-wind-down/SKILL.md` (new this session — pi just shipped during wind-down).
7. The agent-comm thread for SOP retrospective + pi's consolidation messages.
