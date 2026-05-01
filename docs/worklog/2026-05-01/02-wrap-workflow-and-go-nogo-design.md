# Wrap Workflow And Go Nogo Design

**Date:** 2026-05-01
**Type:** note

## Summary

Design conversation about the **last 10%** of a task's lifecycle: the moment it transitions from "agent says complete" to "human reviewer says GO/NO_GO." The user's mental model is **first 10% / last 10%**: high-touch human involvement on intent-contract authoring (collaborative, multi-turn, non-deterministic) AND on post-completion review (executive summary + Q&A + GO/NO_GO), with **fully autonomous deterministic workflow in between** (the schema-enforced phase/cycle loop the substrate already does).

The middle is solved (T002 + T003 + T005 → drive). The first 10% is mostly solved (intent_contract + required_when + actor: human enforcement → T009). **The last 10% is the next big gap** and the user said this is "really what will make this useable for me."

Key architectural decisions surfaced:

1. **Wrap needs a synthesis artifact, not a log dump.** Promise (contract) + Reality (cycles + commits) + Synthesis (the wrap-agent-produced executive summary) + Receipts (diff, runs/jsonl). Synthesis can't be cheap-templated — needs a new wrap agent, same shape as planner/executor/reviewer, with its own brief and SDK transcript.
2. **GO/NO_GO is a first-class state transition.** Recommendation: Option A — extend lifecycle `complete → in_review → accepted | rejected` with `actor: human` on the gating transitions. Don't conflate with the gate store (that's for orthogonal questions surfaced during review).
3. **Build ≠ ship.** Different intent contracts. Ship belongs in a separate task (parented to the build task), not as more phases on the same row. Don't build a "post-completion workflow engine" until ~5 ship cycles have happened by hand and the patterns are visible.
4. **`stores tasks guide` already exists in v0.3 stub form** — same agent, two entry points (gate mode = full write-back; task mode = read-only context). The v0.4 expansion that the agent header references is exactly the wrap shape. So it's not "build a new thing" — it's "graduate the existing stub into a wrap-mode."

## Details

### The lifecycle the user wants

```
                 [first 10%]              [middle 80%: deterministic, schema-enforced]              [last 10%]
                     ↓                                       ↓                                          ↓
human + agent → intent_contract ratified → drive (planner→exec→reviewer cycles) → complete → in_review → accepted | rejected
   collaborative              ↑                              autonomous                       ↑                        ↑
   multi-turn                 |                              deterministic                    |                        |
   non-deterministic          |                              actor:framework                  |                        |
                              |                                                               |                        |
                  T009 substrate enforces this                                       wrap agent + exec_summary    actor:human
                  (required_when on contract sub-fields,                             (NEW — this batch)           gates these
                   actor:human on approved_*)                                                                     transitions
```

The middle is the part the substrate already enforces structurally. The two ends are the parts that *need* to be high-touch — and right now the last end has neither a synthesis artifact nor a state transition.

### What the wrap brief should contain

The reviewer needs a two-page brief, NOT a log:

- **Promise** — `tasks.contract` frozen at open. Already in the row.
- **Reality** — `tasks.cycles[].executor.summary` + `.commit` + `.files_changed` per phase, plus reviewer verdicts. Already in the row.
- **Synthesis** — what was built, what it means, deviations from the contract, residual risk, what the reviewer should sanity-check first. **Not present.** This is the new thing, and it's load-bearing — string-concat of cycle summaries is useless as an executive read.
- **Receipts** — git diff, modified files, links to `runs/*.jsonl` for "show me the thinking."

Synthesis lands in a new column — `executive_summary` on the tasks row — produced by a new **wrap agent** (sibling to planner/executor/reviewer/guide). Brief input: contract + cycles + diff. Output: 150-word executive summary written for Blake-the-reviewer.

### GO/NO_GO as a state transition — Option A vs Option B

**Option A — extend the tasks state machine.** Add states: `complete → in_review → accepted | rejected`. `accepted` is the new terminal for the build workflow. `rejected` reopens (back to `executing` with an amended contract, OR to `blocked` if the reviewer wants to pause and rethink). The transitions carry `actor: human` so the AI can't self-accept.

| | Pros | Cons |
|---|---|---|
| **A** | One row, one timeline; schema enforces the loop; fits the philosophy thesis exactly | Muddles "build done" with "ship readiness" |
| **B** (gate at completion) | Reuses gate store's defer/resume/answer + actor:human | Two rows to look at; gate's `answer` doesn't feed back into the task row |

**Recommendation: A for GO/NO_GO itself** (intrinsic to task lifecycle), **B for orthogonal questions** that come up during review ("should we backfill old rows? — defer 2 weeks").

### Build vs ship are different intent contracts

This was the gentle pushback against adding `phase=2: ship` to the same row.

- **Build contract:** "checkout returns 2xx for null email" — acceptance is functional.
- **Ship contract:** "merge to main, deploy to prod behind LD flag billing_email_guard, smoke test, ramp 10/50/100, rollback if Sentry BILLING-4711 reappears" — acceptance is operational, with rollback plan and observability requirements.

Different conversations, different stakes. The first-10% / last-10% sandwich applies to BOTH, separately. So when a reviewer says GO on a build, the right next thing is `tasks add --parent T002 --kind ship` — a new task with its own contract authoring round. The build task transitions to `accepted-pending-ship`; when the ship task hits its own `accepted`, the parent transitions to `shipped`.

Sounds bureaucratic; isn't, because (a) ship contracts are mostly templated for routine work, (b) build can be merged/reverted independently of ship decisions, (c) state machine stays small (each row has one job).

### Where `stores tasks guide` already fits

`stores tasks guide` exists in v0.3 stub form. Same agent definition (`agents/guide.md`), two modes:

| | `stores gate <id> guide` (v0.3 complete) | `stores tasks <id> guide` (v0.3 stub) |
|---|---|---|
| When | Mid-flow, on `blocked` | (currently) any state, read-only |
| Tools | Read + `gate answer` write-back | Read-only stores commands |
| Persona | "What's blocking us, and what answer unblocks it?" | (placeholder — "v0.4 expansion") |

Wrap is the natural third mode: post-`complete`, write-back permission for `executive_summary` + `accept`/`reject` transitions, persona "what was promised vs delivered, and is it shippable?"

So this isn't "build a new agent" — it's "graduate the existing stub into wrap-mode + add the executive_summary column + add the in_review/accepted/rejected transitions." The agent definition can stay shared; brief shapes diverge per mode (already the pattern with the gate-mode brief).

### The Q&A loop on the brief

The user wants to "naturally and flexibly talk about it" — the brief opens the conversation, then the reviewer asks questions. Two persistence options:

- **Ephemeral** — Q&A lives in chat scrollback only. Lightest; works for most cases.
- **`wrap_log` JSON column** — same shape as `plan_review_log`. Persists Q&A pairs the reviewer flags as load-bearing.

Recommended: start ephemeral; only persist the final GO/NO_GO note + flagged Q&A pairs. Don't over-engineer audit before knowing what gets re-read.

### The sequencing question

If picking least-regret order:

1. **Wrap agent + `executive_summary` column.** No state machine changes. `/task:wrap` becomes "spawn wrap agent, render brief, drop the user into chat." Mechanical; value visible on the very next task.
2. **`complete → in_review → accepted | rejected` transitions** with `actor: human` gating. Small schema bump. GO/NO_GO becomes a first-class fact, not vibes.
3. **Ship as a separate task kind.** Only after wrap-then-merge-by-hand happens 5+ times. Ship-contract template is something you discover by doing.

**Don't build yet:** a generic "post-completion workflow engine." Not enough ship-shaped data points to know what the deterministic part should enforce. Stay manual on ship until the patterns are visible.

## Follow-ups

This is the next major task arc — call it **T010** (cross-store guards bumps to T011, TUI bumps to T012). DONE_WHEN sketch:

1. New `wrap` agent definition at `agents/wrap.md` with its own JSON-schema in `agents/schemas/wrap.schema.json`. Brief input: contract + cycles + git diff summary. Output envelope: `{role: "wrap", executive_summary, deviations[], residual_risks[], recommended_sanity_checks[]}`.
2. New `executive_summary` text column on tasks (or under a new `wrap_log` record); written by the wrap agent's submit handler.
3. Lifecycle extension: `complete → in_review` (verb: `request_review`, actor: ai_autonomous, fires automatically on drive's "all phases done" hand-off); `in_review → accepted` (verb: `accept`, actor: human); `in_review → rejected` (verb: `reject`, actor: human, requires `--reason`).
4. `stores tasks <id> guide` graduates from v0.3 stub: when row status is `in_review`, guide spawns in wrap-mode (write-back permission for the two terminal transitions + the executive_summary, read-only otherwise).
5. `/task:wrap` skill rewrite: spawn `stores tasks <id> guide` in wrap-mode, render the brief, drop the user into chat with the agent. The `accept`/`reject` decision is the natural exit.

Open questions to resolve at planner-time:

- Where does `executive_summary` live — a top-level text column, or as a sub-field of a new `wrap_log` record (parallel to `plan_review_log`)? The latter scales better if Q&A persistence is added later.
- Does the wrap agent run automatically when drive hits `complete`, or only when `/task:wrap` is invoked? Pre-running it means the brief is ready when the human shows up; lazy-running saves Claude calls on tasks that get `accept`-rubber-stamped.
- The `rejected → executing` re-loop semantics: does it require contract amendment first (philosophically cleaner — the contract changed, so the work changed) or can it just re-run with the same contract (faster — for "executor missed a case")? Probably the former, but worth talking through.
- Ship-as-separate-task is filed for later (after 5 manual ships) — but the `accepted-pending-ship` parent state needs to exist now if we want the build → ship → shipped chain to be schema-clean from day one. Or do we just use `accepted` as terminal for v0.5 and add `shipped` later when we file the first ship task? The latter is honest — don't enforce structure that doesn't have data behind it yet.

The user said "this is really what will make this useable for me." — meaning the substrate work to date (T005–T009 + this morning's v0.5.0 batch) is the foundation, but the wrap loop is what closes the human-in-the-loop story. **This is the highest-leverage next task in the project.**
