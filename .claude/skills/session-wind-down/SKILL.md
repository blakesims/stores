---
name: session-wind-down
description: Use when Blake says context is high, asks to wind down, pause/clear the engine, prepare handover, or end a multi-agent stores session cleanly. Coordinates substrate-agent/reviewer-runner feedback, updates SOP skills/docs, and produces next-session handover instructions.
user_invocable: true
---

# Session Wind-Down Skill

Use this when the stores session is nearing context limits or Blake asks to wind down.

Goal: **stop creating new WIP, finish or safely park current work, collect role/SOP feedback, update durable docs/skills, and hand Blake a clean next-session launch plan.**

## 1. Announce wind-down over agent-comm

Send a high-priority message to the active thread, addressed to all active agents.

Required content:

- No new task ratification/start unless Blake explicitly reverses.
- Finish or safely park active/in-review work.
- Keep accepts serialized; avoid new main churn except tasks already ready to land.
- Preserve worktrees/commits; no raw SQL; no broad cleanup unless required for active task completion.
- If a task is in a deep revise loop and not close to PASS, park it with a clear status/handoff rather than forcing late-session churn.

Ask each agent for concise SOP feedback.

For substrate-agent:

1. What worked in the engine-controller role?
2. What failed or caused drag?
3. What should the engine-controller skill say differently?
4. What should become substrate tasks/observations vs session SOP?
5. How should thread traffic be shaped to reduce noise while preserving decisions?

For reviewer-runner:

1. What worked in the read-only codex-sensor role?
2. What boundaries were unclear or inefficient?
3. What metadata/digest fields were useful vs too noisy?
4. How should re-codex triggers, base selection, and ERROR/REVISE taxonomy improve?
5. What should become substrate automation later, especially codex-as-subscriber / review artifact storage?

## 2. Let the engine drain

While agents respond:

- Do not initiate new architecture work.
- Continue answering blocking architectural questions for active tasks.
- Prefer parking over widening scope.
- Watch for unsafe accepts or main churn that violates quiescence.

If the queue is still large, ask substrate-agent for a concise final state table: active / in_review / accepted / parked / blocked.

## 3. Consolidate SOP feedback

Synthesize feedback into:

- Agreements/common ground.
- Role-boundary changes.
- Thread/noise changes.
- Substrate tasks/observations to file later.
- Immediate skill/doc updates.

Common improvements to consider from prior sessions:

- Keep the three-agent split when review volume is high: Pi architect, substrate-agent engine controller, reviewer-runner codex sensor.
- Use local main as review base; do not review noisy origin/main or merge-base diffs.
- Quiesce main while codex reviews a batch.
- First-pass codex should normally wait for substrate-agent's rebased-and-ready ping.
- Add/maintain `REVISE-FALSE-POSITIVE` for Pi-adjudicated codex findings.
- Require scope checks after heavy rebases.
- Require subagent briefs to quote Pi rulings verbatim.
- Verify codex-reported test failures before patching when cheap.

Keep this concise. Do not paste the whole thread into docs.

## 4. Update durable skills/docs

Pi owns SOP skills. Update as needed:

- `.claude/skills/pi-architect/SKILL.md`
- `.claude/skills/engine-controller/SKILL.md`
- `.claude/skills/reviewer-runner/SKILL.md`
- this skill, if the wind-down procedure changed.

Update `docs/engine-health.md` if priorities, live health, or next picks changed. Keep it glanceable.

If the session produced a major architectural direction, ensure it is linked from engine-health and the pi-architect skill. Do not duplicate large docs.

Commit SOP/doc updates promptly with explicit paths staged; never `git add -A`.

## 5. Prepare handover

Create a worklog handover note only when useful for the next agent. Use the worklog system (`docs/worklog/new-note.sh`), never manual filenames.

Handover should include:

- Current daemon/CLI health.
- Current task pipeline state.
- What shipped this session.
- What is parked or needs rescue.
- Current top priorities from `docs/engine-health.md`.
- Active agent-comm thread and whether a new thread should be created next session.
- Any special SOP still in force, especially no subagent `cargo install`.
- Stashes/worktrees that must not be dropped.

## 6. Tell Blake how to start next session

Final response to Blake should include:

- What was updated and committed.
- Whether the engine is drained or what remains parked.
- Recommended next reading order:
  1. `.claude/skills/pi-architect/SKILL.md`
  2. `docs/engine-health.md`
  3. key direction doc, currently `docs/heart-and-architect.md`
  4. latest handover note, if created
- Agent-comm instruction: create/use a fresh thread for the next session unless continuity requires the old one. Provide exact thread path if already created, or tell Blake to ask the next agent to initialize one.
- Suggested prompt for next Pi/architect agent.

## Suggested next-agent prompt

```text
You are the Pi architecture/design governor for stores. Read `.claude/skills/pi-architect/SKILL.md`, `docs/engine-health.md`, and `docs/heart-and-architect.md`. Join the current/new agent-comm thread as `pi`. Confirm current engine health, top priorities, and your role boundaries before directing substrate-agent.
```

## Rules

- Wind-down does not mean abandon work silently; every active item must either finish or be intentionally parked with status.
- Do not start new tasks to improve the handover unless Blake explicitly asks.
- Do not let SOP insights remain only in chat; update skills/docs if they should persist.
- Keep `docs/engine-health.md` concise and current; detailed churn belongs in worklog/agent-comm.
