# Handover 2026-05-07 Wind-Down

**Date:** 2026-05-07
**Type:** note (end-of-session handover after operational-trust drain + Heart-Architect ratification)

## Summary

Session shipped 4 operational-trust tasks (T075/L182, T072/L059, T067/L178, T070/L057) closing the engine-health priority 1-4 cluster, then ratified 3 follow-on tasks (T076/L184 private install path, T077/L171 architecture_reviews phase α, T078/L185 SOPS+age relaxation to plaintext+0600). Wind-down ratified SOP improvements across all three role skills.

**Main lesson (Pi's diagnosis, not buried under the ship cluster):** the engine still requires Blake/Pi to notice "ready work exists" and push dispatch. That's the wrong steady state. Filed L186 — engine-runner monitor primitive — as the durable fix; chat-heartbeat in the engine-controller SKILL is the friction-tax stopgap.

## Details

### Shipped this session

- **T075 / L182** (`da1d347`) — daemon candidate-binary validation before self-reexec. Validates fresh `~/.cargo/bin/stores` against the specific `Schema-driven store framework` Clap-about marker; bounded validation timeout (1500ms); typed `exit_status` field; first production proof landed during T075's own accept-merge ceremony. 14/14 tests pass.
- **T072 / L059** (`e250e5d`) — runs SQL VIEW + atomic backlink with dispatch_submit. New `runs` typed view `(display_id, phase, cycle, role, transcript_path)`; CLI `stores runs list/show`; transcript_path threaded into cycles JSON in same TX as `write_status_and_fields(...).commit()` — Pi-critical atomicity invariant.
- **T067 / L178** (`3784a6f`) — manual-drive ↔ daemon handoff fix (A1-strict). `wrap_log` is NOT a control sentinel; `next_agent` is source of truth. Discriminator: `last_status='ok:wrap_completed'` (free-text column; CHECK-constrained typed `terminal_reason='wrap_completed'` deferred). force-close ordering fix: invoked BEFORE post-submit max-iters bail.
- **T070 / L057** — agent_runs telemetry. Spawn-fail synthetic row with source-layer model_id (e.g. `claude_code:opus` not `claude_code:unknown`); fail-loud propagation on insert error (no silent swallow); tier T1/T3 tests assert non-NULL prompt_cache_hits + post-cycle agent_runs persistence; mock-defaults workspace under `target/test-workspaces` (not /tmp). 3-way merge against post-T067 + post-T072 main resolved cleanly.

### Ratified, in flight at handover

- **T076 / L184** — private substrate install path; move daemon binary off `~/.cargo/bin/stores` to a stores-private location. (Engine-health priority #2 post-T070; closes the L182/C corruption-surface concern.)
- **T077 / L171** — `architecture_reviews` typed store; phase α of Heart/Architect direction. Pi-blessed contract shape (msg_5cea147e revised + msg_69855431 merge-state addendum + msg_e4e619c8 final approval).
- **T078 / L185** — drop SOPS+age, plaintext+0600 approval token. Pi-blessed via msg_61f438cf with 6 codex-time guardrails pre-briefed to reviewer-runner (msg_9c840648). Doctrine relaxation: tier-A becomes host-bound + 0600 + AI honor-system instead of per-turn user-presence-bound.

### SOP / skill updates committed this wind-down

- `28fce9f docs(pi-architect): tighten active-thread and actionability SOP` (Pi-owned)
- `43c51db docs(skills): consolidate session SOP — heartbeat, codex-revise discipline, comms hygiene, dispatch shape` (substrate-agent-owned: engine-controller + reviewer-runner)
- `ad3fe55 docs(engine-health): mark T067/T072/T075 + L176/L058 ✅; rewrite next-picks for post-operator-trust phase`
- `be48309 docs(engine-health): split L182/C → L184 private install path; close Pi clarification`

Key SKILL deltas:
- **engine-controller:** heartbeat doctrine; first-pass full-shape contract drafting; spawn executor for ALL codex-revise; comms hygiene (terse acks, optional prefixes); RE-REBASE-ONLY-NO-CODEX; dispatch shape; CLI ergonomics gotchas.
- **pi-architect:** active thread path is parametric; terse yes/redirect; echo-only-what's-new; Pi reactive to engine-controller heartbeat silence (not a parallel poller); explicit park-on-architecture-rule.
- **reviewer-runner:** compressed PASS digest (~25 lines target); architecture/security/authority PASS keeps invariant-checked one-liner; PASS notes: block; TOOLING-FAILURE result bucket; codex stdin-hang fix (`</dev/null` always); RE-REBASE-ONLY-NO-CODEX scope-identity verification.

### Filed for next-session pickup

- **L186** — engine-runner monitor primitive (the durable fix replacing chat-heartbeat). Open; unratified. Coverage: actionable-row scan + dispatch + visible heartbeat/hold log + lane-cap enforcement; does NOT usurp U-moments. Cross-refs L151 (auto-investigator), T053/L142 (gatekeeper Router), L171 (architecture_reviews complement).
- **L187** — `stores observations update` CLI ergonomics gotchas. Silent failure on multi-bullet args; missing `--acceptance-from-file`; `approval_policy` requires separate `override-policy` verb; multi-status `tasks list` rejected. Workarounds documented in engine-controller SKILL.

### What NOT to do next session

- Do NOT raw-SQL the substrate DB (reads OK).
- Do NOT run `cargo install` from any subagent or test path; use `target/release/stores` from the worktree.
- Do NOT delete `tasks/active|paused|planning/*/main.md` projections from feature branches (residual but durable).
- Do NOT inline-edit codex-revise findings; spawn `task-workflow:executor`.
- Do NOT ratify L186 (engine-runner monitor) without Pi review of the contract shape — even though direction is established, this primitive crosses subscriber/lifecycle territory.

## Follow-ups

### Tomorrow's pickup priorities (in order)

1. **Drain the in-flight queue:** T076, T077, T078 will be in `in_review` (or further along) by next session start. Codex + accept (or revise → re-codex) each. Token-mediated accepts (Blake's session token preserved in chat: `<redacted-approval-token>` — note the L185 doctrine relaxation means this is host-local-bearer, not per-turn-user-presence-bound, going forward).
2. **L186 engine-runner monitor** — Pi-review contract shape; ratify if aligned; promote to T079+. The durable fix for the engine-stall pattern this session surfaced.
3. **L184 follow-up if T076 surfaces a doctrinal question** about what "stores-private location" means structurally (per-workspace `.stores/bin/` vs `~/.local/share/stores/bin/`).

### State at handoff

- **Daemon:** PID changed twice this session (post-T075 + post-T072 + post-T067 + post-T070 self-reexec ceremonies). Current daemon at `/home/blake/.cargo/bin/stores`. Verify with `ls -la /proc/$(pgrep -f 'stores agents run' | head -1)/exe` next session.
- **Pipeline:** 3 in_review/executing (T076/T077/T078). Operational-trust cluster fully closed.
- **Token:** in chat memory; under L185's new framing it'll be plaintext+0600 once T078 ships.
- **Worktrees:** all 4 shipped tasks merged; their feat/ branches remain as before. New worktrees: stores-T076/-T077/-T078.
- **Stashes:** preserved across worktrees as worked through this session; many are projection-noise stashes safely droppable, but verify before drop.
- **Agent-comm thread:** `/home/blake/repos/.agent-comm/threads/2026-05-07-01-stores-review-session.md` is the canonical record (~2900 lines). Next session likely needs a fresh thread.

### Reading order for next session

1. `.claude/skills/pi-architect/SKILL.md` (updated this wind-down)
2. `.claude/skills/engine-controller/SKILL.md` (updated this wind-down)
3. `.claude/skills/reviewer-runner/SKILL.md` (updated this wind-down)
4. `docs/engine-health.md` (refreshed; reflects shipped state + L184/L186 next picks)
5. `docs/heart-and-architect.md` (Pi's prior session output; L171 phase α direction; T077 contract aligns)
6. This handover note (head-first context for the wind-down + the 4 ships + the 3 ratified)
7. The agent-comm thread for SOP retrospective + Pi's wind-down architectural synthesis (msg_185bd255)

### Suggested next-session prompts

- **Pi:** "You are the Pi architecture/design governor for stores. Read `.claude/skills/pi-architect/SKILL.md`, `docs/engine-health.md`, and `docs/heart-and-architect.md`. Join a fresh agent-comm thread as `pi`. Confirm engine health, current top priorities (T076/T077/T078 in flight; L186 next), and your role boundaries before directing substrate-agent."
- **Substrate-agent (engine-controller):** invoke `/engine-controller`. Confirm heartbeat cadence is active. Inspect T076/T077/T078 readiness (executing → code_review → in_review trajectory).
- **Reviewer-runner:** invoke `/reviewer-runner`. Confirm PASS-digest compression in effect. Stand by for codex pings on T076/T077/T078.

### Substrate-agent's session-end reflection

The session's actual lesson is not "we shipped 4 + ratified 3." It is that the system needed Blake to push twice ("why is nothing in action?", "how much easier is this approach?") to convert ready state into action. Engine-controller chat-heartbeat is a stopgap; the durable fix (L186 engine-runner monitor) is the next architecturally-honest step.
