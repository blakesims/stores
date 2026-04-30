# Philosophy

## The problem

You have AI agents doing structured work alongside humans. The problem isn't "make the agent smarter" — it's that the **substrate** they share is unstructured. Markdown files, conventions in `CLAUDE.md`, "remember to fill out scope_in." When work is captured as prose, three things drift:

1. **Intent leaks.** A user creates a task with full context, then the AI picks it up two days later missing the done-criteria. So it asks the human to refresh context — defeating the autonomy.
2. **Authority is implicit.** A field that "should only be answered by a human" is enforced by hope. The AI can write it; nothing stops it.
3. **Source of truth splits.** The agent reads the markdown, decides what's true, writes back. Two write paths → drift → arguments over which file is real.

Most "AI workflow tools" build a process layer on top of these problems. `stores` builds a substrate that makes them structurally impossible.

## The move: schema is the contract; DB is the truth

You describe a workflow as a YAML schema — fields, types, lifecycle states, transitions, per-field actor, `required_when` predicates. The framework generates **everything**: the SQLite DDL, the CLI verbs (`stores tasks add`, `triage`, `submit-execute`), the insert-time validator, the briefing templates, the rendered `main.md`. There is exactly one write path: the CLI. Markdown becomes a **deterministic render** of DB rows on demand, not an authoritative document agents edit.

This is the β architecture: **DB-as-truth + framework-as-engine.** Workflow-shaped stores opt in to a `workflow:` block and get an orchestration engine for free — `next-action`, `brief`, `submit-*`, `render`, `drive`. Non-workflow stores stay pure data + enforcement.

## The three enforcement moments

### 1. `required_when` — capture intent at the moment of context

The `contract` record on `observations` has `required_when: "triage.verdict == 'T3'"` on `done_when`, `scope_in`, `scope_out`. When you triage to T3, you must supply all three together — the framework rejects the write otherwise, listing all violations in one error. The human is forced to bottle their context the moment they have it. Downstream, an AI drains a queue of T3 items without ever needing a context refresh, because the contract is already in the row.

### 2. Per-field `actor` — authority is a structural property of the field, not a convention

The `gate.answer` field carries `actor: human`. An AI invoker (auto-detected from `$CLAUDECODE`) attempting to write it is rejected — with the field name, the required actor, and the detection source in the error. The override (`--invoker human`) is explicit and audited. This is not "the AI agrees not to answer human-only questions"; it is the database refusing the row.

### 3. Lifecycle transitions as guarded edges

States and transitions are first-class in the schema. A transition declares its actor (`framework`, `human`, `ai_autonomous`, `ai_with_human`), its guard predicate, and its required gates. The state machine is enforced at write time. A "4th REVISE attempt" doesn't fail because an agent decided to give up — it's rejected by a schema-level guard, with status auto-set to `BLOCKED`. The thing you can't break in the database, you don't have to remember to enforce in process.

## What falls out

- **Agents become thin.** They don't track state, they don't decide what to do next, they don't render documents. The framework hands them a scoped briefing, they return a JSON envelope validated against a per-role schema (`agents/schemas/*.schema.json`), the framework writes it. v0.4 does this via `claude -p --json-schema` for SDK-native validation, with a Schema-Aligned Parser fallback. Postmortems are the full stream-json transcript in `.stores/runs/`.
- **Multi-runtime.** Because the engine is in the framework, you can swap the agent runtime — mock for tests, `claude-code` today, something else tomorrow — without touching the workflow.
- **Workflows compose.** Any new workflow-shaped problem (notes, runs, gates, reviews) is a YAML file away. You inherit `next-action`, `brief`, `submit-*`, `render`, `drive` for free.
- **Audit trail is mechanical.** The DB is the log. `main.md` is just a view. There is no "did the agent actually do X" — there's a row, with a timestamp, with the actor, with the diff.

## The deeper bet

Most agent frameworks treat the LLM as the cognitive center and the surrounding system as scaffolding. `stores` inverts that: **the schema is the cognitive center, and the LLM is a constrained worker that fills in slots the schema demands.** The framework doesn't ask "what does the agent want to do?" — it asks "what does the next row require, who is allowed to write it, and what predicate must hold?" The agent's job is to produce a value that satisfies the schema. The schema's job is to make sure no work happens without intent, and no intent goes uncaptured.

It's a bet that the durable assets in an AI-collaborative workflow are **not the prose**, but **the typed, validated, actor-attributed rows** — and that if you make those cheap to define and impossible to bypass, the prose can be rendered from them whenever anyone needs to read it.
