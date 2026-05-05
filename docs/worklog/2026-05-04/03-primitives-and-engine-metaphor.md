# Primitives And Engine Metaphor

**Date:** 2026-05-04
**Type:** note

## Summary

Working session with Blake on the substrate's primitive set, prompted by his client-side friction (10.06: 27 uncontracted ledger entries, decision fatigue at 8/11 grill questions, 2–8 concurrent agents context-switching, contract drafts pushed at him in bundles). Blake reframed the substrate as **an engine**: typed buffers (stores) connected by transitions, with specialized subagents at each transition transforming information. The btop-style flow visualization is the right mental model. We surfaced six primitives we have words for (Buffer, Transition, Subscriber, Actor, Direction, Schema), six we don't yet (Loop, Aggregation, Decay, Notification, Capacity, Causality), and one architectural unlock: **the drive cycle is currently a monolith and should dissolve into N transition-subscribers in `agents.yaml`** (specialization-by-transition, not specialization-by-role). The doctrinal upgrade — **stores as decision-routing substrate for every agent**, not just for tasks-as-work — is the largest implication and reframes 2–8 concurrent agents from a context-switching nightmare into a typed pull-queue.

## Details

### Blake's reframe — the engine metaphor

The skill system today is "natural language plumbing" — sprawling collection of skills (`/intake`, `/focus walk`, `/hotfix:batch`, `/task:open`, `/converge`, `/qa:walk`, `/doc:question`, `/qa-agent-walk`, etc.) that route information through implicit conventions. Blake's experience working a real client repo with 5–8 concurrent agents: each agent often stops with an issue or decision, agents push their drafts at him bundled (5+ sub-questions per draft), context-switching cost is brutal, and there is no unified inbox.

His reframe (this session, his words approximately):

> "I keep feeling a kind of engine running. Highly specialised agents at each point to transition information from one stage to another. I should be able to *see* the flow of data through the system — something like btop but for the engine = ⋃ stores. If I can work out the primitives of each store at a very high level, I can design my engine by putting together various stores. They are like the valves, pistons, and carburetor of an engine."

The concrete example he reached for: an agent files a sloppy bug (quickly, while doing something else). A **triage-agent** (specialized for that one transition) decides whether to `wont_fix` / `needs_info` / `draft_contract`. The triage-agent IS the throughput. Right now Blake does that work himself; it exhausts him at scale.

This is **specialization-by-transition**, not specialization-by-role.

### Push vs pull, sharpened

Push/pull is a primitive of **buffers** — specifically, *which side of a producer→consumer interface holds the unread state*.

- **Push** = producer's queue; consumer faces a perpetual inbox (the 10.06 pattern: contract draft handed over with 5 sub-questions, decide now).
- **Pull** = shared buffer; consumer fetches when ready (the `/grill-me` pattern: one forced choice at a time, consumer paces).

Working memory is ~4 chunks. Push at scale shreds working memory; pull is the only sustainable shape when 5+ producers are active. The current substrate verbs (`tasks accept`, `observations confirm`) are still push-shaped. **Pull-direction has to be built into the verbs** — `stores observations triage L###` walking 5 forced choices with defaults preselected, not bundled. Schema knows what fields are needed; the verb's job is to ask one at a time.

### The candidate primitive set (working draft)

Six we have words for:

| Primitive | Definition | Where it lives today |
|---|---|---|
| **Buffer** | Typed store with a lifecycle | `observations`, `tasks`, `gate`, schema.yaml |
| **Transition** | Edge between states with guard + actor + side-effects | schema.yaml `transitions:` blocks |
| **Subscriber** | Consumer attached to a transition; often also a producer | `agents.yaml`; rust builtins in `src/flow/builtins/` |
| **Actor** | Typed identity with privileges (`framework`, `ai_autonomous`, `ai_with_human`, `human`) | per-field `actor:` in schema |
| **Direction** | Property of a buffer's interface to a consuming actor: push or pull | implicit in CLI verb shape; not yet a first-class concept |
| **Schema** | Type contract enforced at write-time (fields, required_when, actor) | schema.yaml |

Six we do not yet have words for, surfaced by Blake's client-pull pressure:

| Missing | What it would mean | Surfaced by |
|---|---|---|
| **Loop** | Transition that returns to a prior state (e.g. `needs_info` back to filer) | 10.06 triage flow |
| **Aggregation** | N rows → 1 summary row (batched dispositions, decision rollups) | 10.06 batch ratification |
| **Decay** | Automatic transition on time-since-X (staleness, expiry) | 10.06 friction #5 |
| **Notification** | Producer-initiated push to actor when their pull-queue has new content | implied by 2–8 agent inbox problem |
| **Capacity / Backpressure** | Per-transition load and routing-around behavior | implied by `/hotfix:batch` serialization |
| **Causality** | Queryable provenance: this T### caused by L### caused by L### upstream | implied by missing `--why` query surface |

These are **candidate** primitives. Some may collapse (Loop is arguably just a Transition with `to_state ≤ from_state`). Some may split (Buffer might be Store + LifecycleMachine separately). Don't declare the set finished — let realistic-pull surface more.

### The architectural unlock — dissolve the drive cycle

Today `tasks drive` is a monolithic command running planner → plan_reviewer → executor → code_reviewer → wrap in sequence. The agents are role-specialized and the orchestration is hardcoded.

The doctrinal generalization: **every transition has a subscriber, possibly LLM-backed.** The drive cycle becomes five entries in `agents.yaml`, each subscribing to one edge (`planning → plan_review`, `plan_review → ready`, `ready → executing`, etc.). The `tasks drive` command becomes superfluous — the daemon dispatches each transition's agent on its own, gated by `dispatch_locks`.

Today `agents.yaml` supports rust builtins (`builtin:auto-promote`, etc.) but does **not** support LLM-backed subscribers as a declarative shape. Adding that shape is the next architectural unlock:

```yaml
- name: triage-agent
  subscribes_to:
    - store: observations
      transition: { from: open, to: investigating }
  command: "claude-code:triage-agent.md"   # ← new shape
  claim_window_secs: 600
```

Once that exists, "specialized agent per transition" stops being a concept-level idea and becomes a configuration entry. Each transition gets a guardian. Some are pure rust (worktree provisioning, branch merge). Some are LLM-backed (triage, contract drafting, plan review). Some are humans (U1, U3). All read/write the same typed buffers.

### The largest implication — stores as decision-routing substrate for *every* agent

Currently when Claude in tmux pane #3 needs a decision, the decision lives inside that agent's context. Blake has to scroll, read, decide, switch back. The decision is invisible to a unified inbox.

If the agent instead **files an `observations` row** (or a future `decisions` store row) with `actor: human` blocked-on field, then any pull-shaped interface — `stores triage`, a status board, a notification — surfaces "Pane #3's agent needs you to pick A / B / C, full context here." The agent goes back to working on something else. Blake drains the typed decision queue at *his* pace, in *his* preferred direction.

This is the same architectural move as "DB is truth, not markdown" applied to the **human-in-the-loop interface itself**. Currently human-in-the-loop state lives inside agent contexts (push). It should live in a typed substrate the human pulls from. Stores already has the bones (`actor: human` fields, blocked transitions, ntfy hooks). What's missing is the **convention that agents must file decisions to the substrate before stalling**, not stall in-place expecting the human to come find them.

The 10.06 friction doc instinctively reached for this with "orchestrator-as-fleet-planner." That framing was wrong (back-channel anti-pattern) but the underlying need was right: **the human needs a unified queue of decisions across all in-flight work.** The substrate is where that queue lives. The orchestrator-AI is just another producer that writes to the queue.

When this lands, 2–8 concurrent agents stops being a context-switching nightmare and becomes throughput-with-a-typed-inbox.

### What we should NOT do

- **Don't declare the primitive set finished and start shipping based on it.** We're at "found the framework for finding the primitives," not at "found the primitives." Stay in the metaphor — engine, pipes, junctions, throughput — and let the metaphor stress-test each candidate.
- **Don't overload the orchestrator-AI with fleet-planning intelligence.** The substrate's `next-action` queries answer "what's next" for any consumer. Reasoning logic that should live in schema/predicates must not migrate into the orchestrator's prompt.
- **Don't treat stores as an issue tracker.** It's a generic typed substrate. Decisions, designs, qa-runs, gate-reviews, dependencies — all are stores. The engine is the *composition* of stores via transitions and subscribers.
- **Don't ship pull-direction as an external skill** wrapped around push verbs. Pull-shaped verbs need to be built into the substrate itself (`stores observations triage` walking 5 forced choices), not bolted on top.

### Realistic-pull confirming the doctrine

This session is the third realistic-pull moment in three days:

- **2026-05-03 worklog 07** — `cargo install` as deploy-verb generalization caught only because 10.06 doesn't deploy via cargo. `cargo-install` builtin was reframed as one specialization, not the type.
- **2026-05-03 worklog 07** — T2-direct-to-main retired before 10.06 surfaced it; auto-scaffold per-task worktree is the substrate-honest answer.
- **2026-05-04 (today)** — `touches_files` + tier-aware ratification + decisions-store + missing-primitives + the engine reframe. Six insights from one client-side pull.

Three rounds, six substrate-shaping moves. The realistic-pull doctrine is doing the work it was named for. Continue pulling on 10.06 as the default design pressure.

## Follow-ups

| # | Topic | Priority | Tier | Notes |
|---|---|---|---|---|
| 1 | Update `docs/philosophy.md` with concise primitives section (working draft + missing set) | high | T1 | Doctrine update; concise, not bloated. Ship alongside this worklog. |
| 2 | LLM-backed subscribers in `agents.yaml` (the `command: "claude-code:<agent>.md"` shape) | high | T2-T3 | The architectural unlock. Dissolves the drive cycle into transition-specialized subscribers. Bigger than L048. |
| 3 | `decisions` store (or `actor: human` queue convention on `observations`) | high | T2-T3 | The largest UX implication. Unified pull-queue across all in-flight agents. |
| 4 | `stores observations triage` pull-shaped verb (5 forced choices, defaults preselected, skip-to-defer) | high | T2 | Closes ratification cost cliff. Pairs with default-T1 + tier-aware required_when. |
| 5 | File observations for missing primitives: Loop, Aggregation, Decay, Notification, Capacity, Causality | normal | n/a | Discovery captured as substrate rows; pickup will route them. |
| 6 | `stores fleet` / `stores status` view reading `transition_history` for per-edge throughput | normal | T2 | The btop-style flow visualization. Data is there; surface is missing. |
| 7 | Continue pulling 10.06 friction into stores design before shipping new generalizations | meta | n/a | Doctrine, not a task. |
