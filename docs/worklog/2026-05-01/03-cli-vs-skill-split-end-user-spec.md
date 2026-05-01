# Cli Vs Skill Split End User Spec

**Date:** 2026-05-01
**Type:** note

## Summary

End-user spec for what `stores` actually feels like to use day-to-day, written from a UI/UX angle rather than an architecture angle. The motivating problem: the user's current client-repo skills (`/task:open` ~600 lines, `/day:open` ~270 lines) are **state machines inlined into prose** — the LLM is forced to interpret routing logic, lock semantics, tier recomputation, bucket filters, the (status × contract) matrix on each invocation. That's the engine living in the skill, exactly the substrate problem `philosophy.md` exists to eliminate.

The split the user articulated, in their own words: **"would I ever want to talk to the agent doing this?"** If no → CLI verb. If yes → skill. This is the cleanest formulation we've reached and it should anchor the v0.6+ design work.

The deeper goal is **one source of truth for "workflow," portable across harnesses.** The CLI is the engine; skills are slim chat wrappers. The same workflow runs in Claude Code, the Anthropic SDK directly, Cursor, or a homegrown harness — only the question primitive changes.

## Details

### The criterion

Apply this test to any phase in any current skill:

1. **Could a cron job do this?** → CLI verb. (Health checks, syncs, bucket counts, render, state transitions, queue-watchers, ratification validation.)
2. **Does it require reading prose and deciding what's actionable?** → Agent inside a skill. (Catch-net transcription, contract drafting at hot context, wrap synthesis, triage investigation.)
3. **Would I plausibly ask a follow-up afterward?** → Skill, not CLI. (Day:open, intake, wrap, triage.)

Executor / code-reviewer / plan-reviewer all fail #3 — you read the diff, you don't follow up with the agent that wrote it. They live inside `stores tasks drive --watch`, headless. Day:open passes #3 every morning, so it stays a skill — the value is the agent's hot context across all the inboxes, not the data itself.

### What runs as a CLI on the command line directly

The user runs these without a harness, in their terminal or a tmux pane:

| Command | Purpose |
|---|---|
| `stores observations add "..."` | Quick capture mid-terminal |
| `stores observations list` | Query |
| `stores observations contract <id> --ratify ...` | Validation, mechanical (required_when fires) |
| `stores tasks ls --status in_review` | Review queue |
| `stores tasks <id> show` | Inspect a row |
| `stores tasks <id> accept` / `reject --reason "..."` | State transitions (actor:human) |
| `stores tasks drive --watch` | The autonomous middle 80% — daemon pane |
| `stores day check` | Health + checklist |
| `stores day buckets` | Queue counts (B1-B5 from work-lifecycle) |
| `stores day report` | Deterministic render |
| `stores day status` | Router + reminders |
| `stores observations sync` | Pull/push inbox sync |
| `stores observations verify-sweep` | Auto-resolve typed assertions |

These are cron-able. None of them need an LLM in the loop.

### What runs as a skill inside a harness

Slim prompts, agent has the stores CLI as tools, harness provides chat + question primitive:

| Skill | Why it's a skill not a CLI |
|---|---|
| `/day:open` | Agent loaded the morning state — user follows up on bucket items, stale entries, FAILs |
| `/intake` (or `/observation:add` interactive) | Collaborative contract drafting at the moment of hot context |
| `/task:wrap` | Wrap-mode chat on an `in_review` task: brief + Q&A + accept/reject |
| `/observation:triage` | Agent investigates one item with the user |

The skills are ~30-40 lines each. The heavy lifting is in the CLI underneath.

### The collapse — `/day:open` worked example

Current `/day:open`: ~270 lines, 8 phases, prose state machine.

Post-stores: six CLI calls + ~40 lines of skill prose.

```
stores day check          # Phases 1-3: checklist + health + truth-engine logs
stores observations sync  # Phase 4
stores observations verify-sweep  # Phase 4.5 (typed cases auto)
stores day buckets        # Phase 5.5
stores day report         # Phase 6 (deterministic render)
stores day status         # Phase 7 (router + reminders)
```

The skill prompt becomes:

> Run the six commands above. Then do the **catch-net pass** (current Phase 5): scan yesterday's daily summary, wrap-task deploy notes, recent triage notes. For any prose action item not already covered in `stores observations list` or `stores gate list`, file it. Surface the rendered report. Stay loaded — the user may ask follow-ups about any bucket item. For top-3 high-priority decisions, ask one at a time using the harness's question primitive.

The agent only does what *requires judgment*: the catch-net transcription pass, and the conversational tail. Everything else is the CLI. The 270-line skill collapses by ~85%.

### The collapse — `/task:open` worked example

Current `/task:open`: ~600 lines, Phase 0a/0b/0c/0d + Stages 0-7, encoding lock semantics, tier recomputation, (status × contract) routing matrix, capability frontmatter writing, REVISE max-3-cycles, CodeRabbit gate, etc.

Post-stores: this skill **mostly disappears**. The pieces split as:

- **Capture / ratify (first 10%)** → `/intake` skill (collaborative draft) + `stores observations contract --ratify` CLI (the boundary). `required_when` enforces complete contracts; no "amend 3× then abort" prose needed.
- **Routing by tier** → schema concern. `final_tier` is a generated column (`max(tier_hint, touches_floor)`). T1 inline / T2 mini-loop / T3 worktree are three transition paths in the same lifecycle, gated by `final_tier`. Schema picks; skill doesn't route.
- **Lock semantics** → `lock_holder` + `lock_acquired_at` columns + a guard predicate on the open transition. Two sessions can't pick the same row because the second insert fails.
- **Phase loop / REVISE / BLOCKED** → schema-enforced state transitions, run by `stores tasks drive --watch` (the daemon pane). Headless agents fill JSON envelopes; CLI validates and writes.
- **CodeRabbit gate** → either a transition hook (post-`complete`, pre-`in_review`) or a separate workflow store that gates on its own row. Repo-specific, lives in workflow YAML extension.
- **Capability declaration** → typed field with FK to capability table. No "infer + confirm + write frontmatter" prose.

What remains as a skill: nothing meaningful. `/task:open LNNN` becomes `stores tasks open LNNN` — a CLI call that binds the observation to a task, sets up the worktree via transition hook, queues for `drive`. The user types it directly; no chat needed.

### Harness portability — why this matters

Currently `/task:open` and `/day:open` are married to Claude Code (AskUserQuestion, slash-command syntax, `./dev` CLI integration, the Task subagent dispatch shape). Switching harnesses means rewriting them.

Post-stores:

- **CLI is portable already.** Stores runs anywhere; cron, shell, any harness, no harness.
- **Skills become "spawn a chat with these tools, this short prompt."** Tools = the stores CLI (any harness can shell out). Prompt = ~40 lines. The harness only needs:
  - LLM with tool use
  - A way to ask the user a question (Claude Code's AskUserQuestion, or just chat-blocking prompts)

If the user switched from Claude Code to the Anthropic Agent SDK directly, or Cursor, or a homegrown harness — the same `/day:open` skill works. They re-implement the question primitive (or block on stdin) and nothing else changes. The slash-command syntax, AskUserQuestion, the transcript view — those are CC conveniences, not load-bearing.

The test for "is this skill lite enough?": **could it run in tmux + bash + a Python script that calls the Claude API directly?** If yes, it's portable. If not, harness logic has leaked in.

### The 10-80-10 in commands

Mapping back to the morning's wrap-workflow design discussion (worklog 02):

```
[first 10%]                    [middle 80%]                          [last 10%]
/intake (skill, hot context)   stores tasks drive --watch (daemon)   /task:wrap (skill, review chat)
  ↓                              ↓                                     ↓
stores observations contract   schema-enforced phase loop            stores tasks accept/reject
  --ratify (CLI boundary)      (no human in the loop)                (CLI boundary)
```

Both 10%s are **skills** because the agent's hot context is the value. The middle 80% is a **daemon CLI** because the agent's context is irrelevant — only the diff matters.

### Day-in-the-life sketch

- **08:30** — `/day:open` (skill). Agent surfaces buckets, FAILs, stale items. User asks two follow-ups. Filed three previously-untracked items into the gate.
- **08:35** — `stores tasks ls --status in_review` (CLI). Two tasks waiting.
- **08:40** — `/task:wrap T042` (skill). Wrap agent has the brief; user reads, asks one question, `stores tasks T042 accept` (CLI).
- **09:10** — Mid-call with client. `/intake` (skill). Capture observation, ratify contract collaboratively. Two minutes.
- **09:15** — Watcher pane (running `stores tasks drive --watch` since morning) picks up the new T3, sets up worktree, runs the phase loop. User goes back to the call.
- **17:30** — `stores day close` (CLI). Renders the day. Pure data.

CLI for everything mechanical. Skills for the three moments where conversation is the value.

## Follow-ups

This is the v0.6+ design north star. To turn into a concrete next-task arc:

1. **Catalogue the CLI surface** the spec implies. Audit current stores verbs against the table above; gap-fill what's missing. Likely gaps: `stores day {check, buckets, report, status, close}`, `stores observations {sync, verify-sweep}`, `stores tasks {drive --watch}` if not already shipped, `stores observations contract --draft` (mechanical synthesis path).

2. **Specify the transition-hook system.** The worktree-setup-on-`executing` example is the canonical case. Minimal shape: workflow YAML declares `on_enter: <shell command>` per state; CLI runs it after the row write. Bigger questions: error handling (hook fails → row stays in prior state? rolls back?), idempotency (re-running a hook on a re-entry transition), repo-local vs global config.

3. **Specify the catch-net judgment surface.** What CLI primitives does an `/day:open` agent need to do the Phase 5 transcription pass without re-implementing state? Probably: `stores observations list --json`, `stores gate list --json`, plus a "find me prose action items not yet covered" helper or just rely on the agent's read of yesterday's summary.

4. **Worked-example skill rewrites.** Pick `/day:open` first (highest leverage, well-understood). Write the post-stores 40-line version against a mock CLI surface. Use the diff to drive what CLI verbs need to ship.

5. **Decide where slash-command syntax lives.** Today, Claude Code owns `/foo`. If skills are harness-portable, the invocation needs a harness-agnostic shape. Options: skills are just markdown files at well-known paths, harnesses each register their own invocation; or stores ships a `stores skill run <name>` CLI that any harness shells out to. Probably the latter — one more way the CLI absorbs orchestration responsibility.

6. **The ad-hoc free-text path on `/task:open`** (current Stage 1b) needs a home. Most of `/task:open` collapses, but ad-hoc tasks still exist. Likely answer: `stores tasks add --kind ad-hoc` (CLI) for direct entry, or fold into `/intake` (skill) for collaborative drafting. Don't lose this path.

The motivation throughout: **remove insane logic from skills into the stores CLI. One source of truth for "workflow" that is portable.** Every line of state-machine prose in a skill is a place where the schema isn't the contract — and the philosophy says the schema is the contract.
