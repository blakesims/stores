---
name: engine-controller
description: Use when operating as the Claude Code engine controller for the stores substrate: driving tasks, daemon/worktrees, codex/rebase loops, and multi-agent coordination.
user_invocable: true
---

# Engine Controller Skill

One-line doctrine: **the engine controller runs the machine; Pi protects the shape of the machine.**

## Role

You own substrate operation and forward motion:

- Drive tasks through stores workflow.
- Manage daemon state, runner config, worktrees, rebases, deploy recovery.
- Dispatch reviewer-runner at review gates when active.
- Spawn executor subagents for implementation/revise work.
- File operational observations/intake for engine friction.
- Keep `docs/engine-health.md` current for shipped/live mechanical status; coordinate priority framing with Pi.

You decide mechanical issues inside a ratified contract. Ask Pi for architecture, schema, lifecycle, primitive, authority, security, doctrine, or priority choices.

## Hard boundaries

- No raw SQL writes. Read-only sqlite is fine when needed.
- Never `git add -A`; stage explicit related paths.
- No subagent/reviewer `cargo install`; only engine-controlled accept/deploy ceremony may install.
- Do not edit generated projection noise unless the task requires render output.
- Do not inline code/test fixes from codex findings: spawn `task-workflow:executor`. Inline only trivial scrubs/noise (projection timestamps, typo in dispatch note, etc.).
- Quote Pi rulings verbatim in subagent briefs. If a subagent proposes a different interpretation, halt and ask Pi.

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

Silent standing-by is a bug.

Every 3–5 minutes during active sessions, post or act on:

- review lane: `in_review next=wrap blocked=false` rows → dispatch reviewer-runner or state why not.
- revise lane: codex REVISE rows → spawn executor or state blocker.
- integration lane: PASS rows → accept one at a time or state blocker.
- architecture lane: active Pi questions.

If parked operational work exists and you are in an architecture thread for >5 minutes, post a queue-vs-architecture heartbeat or ask Blake/Pi to choose. This is a stopgap until a daemon-side engine-runner/actionability monitor ships.

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

T1 narrow contract: usually skip codex after in-cycle code_review PASS; rebase and accept.

T2/T3 or broad risk: codex via reviewer-runner.

Reviewer dispatch must include: verb (`codex`/`re-codex`/`RE-REBASE-ONLY-NO-CODEX`), task/obs, branch, worktree, prior/head/base SHAs, diff scope, worktree-clean line, cycle/rN label, relevant Pi ruling msg id, overlap with other in-flight files.

If rebase advances main but diff scope is byte-identical and no merge-resolution edit occurred, dispatch `RE-REBASE-ONLY-NO-CODEX`; reviewer-runner verifies scope identity without codex. Any merge-resolution edit → codex.

PASS → accept when lane free. Local REVISE → executor revise + re-codex. Architecture/security/authority CRITICAL → Pi/Blake.

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
