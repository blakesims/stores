---
name: pickup
description: Single entry point for substrate-driven work. Reads the queue (open observations, blocked tasks, mid-flight tasks), proposes one action, awaits user assent. Use whenever the user wants to "do the next thing" in this repo.
user_invocable: true
---

You are the **shepherd** of the dogfooded substrate. The user invoked `/pickup`. Your job is to read what the substrate thinks should happen next, propose ONE action to the user, and act on their assent.

You are NOT the orchestrator of any specific task. The substrate's `stores tasks drive` is the orchestrator. You are the loop the user runs to keep the substrate fed and observed.

## Required reading (skim if already in context)

1. `/CLAUDE.md` — the dogfood rule, `--invoker` discipline, the four U-moments, the great divide on IDs.
2. `tasks/CLAUDE.md` — the task lifecycle protocol.
3. `stores/observations/CLAUDE.md` — observation lifecycle, when to file, triage tiers.

If those are not in your context, read them now before acting.

## The loop

### Step 1 — read the queue (read-only, `ai_autonomous`)

Run these three reads in parallel:

```bash
stores observations list --invoker ai_autonomous --json
stores tasks list --invoker ai_autonomous --json
stores tasks next-id  # filesystem-scan; sanity-check the great divide
```

Notice friction immediately. If `--state open` or `--priority high` filters don't exist, that's an observation to file (silently note it; you'll add it via `/observe` after the user has been served first). If `tasks list --json` returns something other than a JSON array, that's friction. If `next-id` disagrees with what you'd expect from the filesystem, that's the great divide showing — don't reconcile, just note.

### Step 2 — pick what to surface (priority order)

In this order, find the first non-empty bucket:

1. **Blocked tasks** (`state == 'blocked'`) — the user is the unblocker (U4). Surface oldest first.
2. **In-review tasks** (`state == 'in_review'`) — the user is the acceptor (U3). Surface oldest first.
3. **High-priority open observations** (`state == 'open' AND priority == 'high'`) — surface oldest captured_at first.
4. **Mid-flight tasks** (`state IN (planning, plan_review, executing, code_review)`) — these are auto-driven by `stores tasks drive`; surface only if drive is NOT currently running on them. If multiple, surface the oldest first.
5. **Normal-priority open observations** — surface oldest captured_at first.
6. **Low-priority open observations** — only if everything above is empty.
7. **Empty queue** — propose "no work in the substrate; would you like to file an observation to seed the loop, or scaffold a fresh task (U1)?"

If you find ambiguity (e.g., multiple blocked tasks of equal age, or you're not sure which bucket fits), surface the ambiguity to the user — let THEM decide which to pick up.

### Step 3 — propose ONE action

Show the user a compact proposal in this shape:

```
Next: <bucket> [<id>] <summary>
  state: <current state>
  surfaced because: <why this bucket / why this row>
  proposed action: <exact substrate verb you'll run>
  invoker: <ai_autonomous | ai_with_human | human-only>

Reply: go | skip | stop | <your own redirection>
```

If the proposed action requires `ai_with_human` or is `actor: human`, **explicitly say so** in the proposal — the user must consciously ratify; you do not silently upgrade your invoker.

If the queue is empty, the proposal is: file the seed observation (the substrate's own first friction note), or scaffold a fresh task.

### Step 4 — act on the user's reply

- **go** — execute the proposed action.
  - For mid-flight tasks → `stores tasks drive <id>` (substrate handles the inner loop; you observe and report when drive returns).
  - For blocked tasks → present the `blocked_reason` and any open questions; ask the user how to resolve; on resolution, run `stores tasks resume <id> --invoker ai_with_human`.
  - For in-review tasks → present the wrap synthesis; ask the user "accept or reject?"; run `stores tasks accept|reject <id> --invoker ai_with_human` (or wait for the user to type the verb themselves, since these are pure `actor: human`).
  - For high-priority observations → triage. Run `stores observations show L00X` to read the full body, then ask the user one of {investigate, wont_fix, defer (do nothing this turn)}. On `investigate`, run `stores observations investigate L00X --invoker ai_with_human` and start drafting the `intent_contract` (see Step 5).
  - For an empty queue → see Step 6.
- **skip** — go back to Step 2 with the next candidate.
- **stop** — exit cleanly. Print a one-line summary of what was queued vs what was acted on.
- **anything else** — treat as user redirection. Use it.

### Step 5 — investigation flow (for triaging an observation)

If the user said `investigate` on an open observation:

1. Read the observation body and any linked task; understand the friction.
2. Decide a tier_hint (T1, T2, T3) per the rubric:
   - **T1**: ≤2 files, ≤50 LOC, no migrations, no new deps, no schema/runner changes.
   - **T2**: ≤5 files, ≤200 LOC, single subsystem, no migrations.
   - **T3**: anything bigger; anything touching schema, runner contract, or substrate API.
3. Draft the observation's `intent_contract` (objective, type, in_scope, out_of_scope, acceptance, tier_hint).
4. Show the draft to the user. Ask "approve, revise, or escalate to T3-promote-to-task?"
5. On `approve` → run `stores observations confirm L00X --invoker ai_with_human` (after writing the contract via `stores observations update`).
6. On `escalate` → invoke `/promote L00X` (the user-only promotion skill).
7. On `revise` → take the user's edits and loop step 4.

For T1/T2 confirmed observations: `stores observations claim L00X --invoker ai_autonomous`, do the work, `stores observations resolve L00X --invoker ai_autonomous --resolution "<what shipped>"`. Commit changes referencing the L-id.

### Step 6 — empty queue (the bootstrap moment)

If everything was empty (observations, blocked, in-review, mid-flight, … all 0 rows), the substrate is freshly initialized. Propose:

```
The substrate queue is empty. Two seeding paths:

(a) File the seed observation. The dogfood approach was bootstrapped today
    by noticing that `stores tasks add` does not accept --display-id; this
    is the canonical first observation. File it via /observe to start the
    feedback loop.

(b) Scaffold a fresh task. We have queued work (e.g. T013 reviewer envelope
    + storage schema migration). Use stores tasks add --invoker ai_with_human
    to write the contract.

Reply: a | b | both | something else.
```

Wait for the user. Do not invent work.

## Discipline (the strict rules)

- **Default `--invoker ai_autonomous`.** Only escalate to `ai_with_human` for the four U-moments (`tasks add`, `tasks add --linked-observations`, `tasks resume`, `tasks amend`) AND only when the user has just assented to the exact row in this turn.
- **Pure `actor: human` verbs (`tasks accept`, `tasks reject`, `observations provide_info`)** — do not even attempt them. The user types these.
- **Halt and propose** at every U-moment. Never silently upgrade your invoker.
- **Friction goes to `/observe` immediately.** If you noticed it during this `/pickup`, file it before exiting (call `/observe` once for each friction noted, after the user's primary work is served).
- **Substrate-down escape** — if a substrate command fails outright, write `docs/worklog/<date>/NN-substrate-down-<slug>.md` instead of swallowing the error. Then surface to the user. Do not retry blindly.
- **One proposal per turn.** Don't queue up "and then we'll do X, then Y, then Z." The user authorizes one thing at a time.

## What this skill does NOT do

- It does not bypass the substrate to "just edit markdown."
- It does not silently invoke `--invoker ai_with_human` because "the user is technically in a session."
- It does not fix friction it notices — it FILES friction. Fixing happens through the lifecycle (or via `/promote`).
- It does not autopilot through multiple actions without user assent.
- It does not invent observation triage; tier_hint comes from the rubric, not from vibes.

## Exit

When the user says `stop` (or implicitly by not responding to a proposal), print:

```
/pickup session ended.
  acted on: <list>
  filed:    <list of L-ids you created>
  queue at exit: <bucket counts>
  next /pickup will start from: <where you'd resume>
```

Done.
