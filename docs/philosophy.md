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

The `intent_contract` record on `observations` has `required_when: "intent_contract.contract_state == 'ready'"` on `objective`, `acceptance`, `in_scope`, `out_of_scope`, `tier_hint`, `approved_by`, and `approved_at`. When you ratify to `ready`, you must supply all of them together — the framework rejects the write otherwise, listing all violations in one error. The human is forced to bottle their context the moment they have it. Downstream, an AI drains a queue of T3 items without ever needing a context refresh, because the contract is already in the row.

Ratification can happen at filing or at the investigation transition. `--lock-contract` on `observations add` lets a user-present session ratify at birth (when invoker is `human` or `ai_with_human` with token); `ai_autonomous` filings land with `contract_state: draft` and queue for review.

### 2. Per-field `actor` — authority is a structural property of the field, not a convention

The `gate.answer` field carries `actor: human`. An AI invoker (auto-detected from `$CLAUDECODE`) attempting to write it is rejected — with the field name, the required actor, and the detection source in the error. This is not "the AI agrees not to answer human-only questions"; it is the database refusing the row.

A field with `actor: human` has **two equivalent grounding paths**, both schema-verified:

- **`--invoker human`** — the user types the verb themselves. The CLI invocation is the human's signature.
- **`--invoker ai_with_human --approve-token <T>`** — the user has pre-decrypted an age-encrypted token (user-presence required: passphrase or hardware tap) and pasted it into the AI's chat context. The AI executes the write with the token attached; the substrate verifies via constant-time hash compare against `~/.config/stores/approve.token.hash`. The token IS the user's signature, transmitted through the AI as agent.

Both paths are equally valid grounding. The difference is cardinality: path one is per-row (the user sits at the keyboard for *this* verb); path two is per-session (the user pre-authorized a session of work and the AI executes individual writes within it). The substrate doesn't care which path produced the write; it cares only that the row is grounded.

The threat model is named explicitly. The token at rest is age-encrypted to a user-presence-required identity (passphrase-protected age key, hardware-backed key, etc.); a malicious or runaway AI cannot decrypt it without the user. The `auth init` verb refuses to bootstrap against a raw plaintext age key — that would defeat the encryption. Once the user decrypts the token into chat context, the AI possesses it for the remainder of the session; the AI's *ask-first* behavioral discipline (propose ONE action; await `go` before each U-moment) is the runtime protection during that window. Session-end → token leaves AI context.

### 3. Lifecycle transitions as guarded edges

States and transitions are first-class in the schema. A transition declares its actor (`framework`, `human`, `ai_autonomous`, `ai_with_human`), its guard predicate, and its required gates. The state machine is enforced at write time. A "4th REVISE attempt" doesn't fail because an agent decided to give up — it's rejected by a schema-level guard, with status auto-set to `BLOCKED`. The thing you can't break in the database, you don't have to remember to enforce in process.

## The two-gate operational frame

The three enforcement moments above collapse, in practice, to exactly two halts in any flow: a **front gate** (the contract must be locked — `contract_state: ready` — before work can promote past triage) and a **back gate** (a `actor: human` signature is required to accept finished work). Everything between the gates — investigation, planning, plan review, execution, code review, wrap, branch merge, deploy ceremony — flows under per-field actor + lifecycle guards without further halts. The human's role reduces to the two moments where authority must be present: capture intent, accept work.

## What falls out

- **Agents become thin.** They don't track state, they don't decide what to do next, they don't render documents. The framework hands them a scoped briefing, they return a JSON envelope validated against a per-role schema (`agents/schemas/*.schema.json`), the framework writes it. v0.4 does this via `claude -p --json-schema` for SDK-native validation, with a Schema-Aligned Parser fallback. Postmortems are the full stream-json transcript in `.stores/runs/`.
- **Multi-runtime.** Because the engine is in the framework, you can swap the agent runtime — mock for tests, `claude-code` today, something else tomorrow — without touching the workflow.
- **Workflows compose.** Any new workflow-shaped problem (notes, runs, gates, reviews) is a YAML file away. You inherit `next-action`, `brief`, `submit-*`, `render`, `drive` for free.
- **Audit trail is mechanical.** The DB is the log. `main.md` is just a view. There is no "did the agent actually do X" — there's a row, with a timestamp, with the actor, with the diff.
- **Autonomous flow is in-substrate.** A daemon (`stores agents run`) polls state transitions and dispatches builtin subscribers — `accept-merge`, `cargo-install`, `schema-migrate`, `user-escalation` — under a `policies.yaml` predicate layer. Default action: allow. NEVER policies are sacrosanct. Every automatic write records `policy_ref` and the policies-file hash on the row's `transition_history` audit trail. The engine consumes itself: it ships *inside* the substrate, not above it.

## What's outside the substrate

Worktree provisioning, project setup scripts, and observing wrappers — a Claude Code instance watching a long-running session, an outer orchestrator that spawned the whole thing — live outside the substrate. `stores` does not own these. They wrap `stores`; `stores` does not wrap them. This is not a gap to fill later; it is the correct boundary. The substrate's job is to enforce workflow structure. Everything above that layer is the caller's problem.

That boundary has one implication worth making explicit: the substrate has exactly one write path — the CLI — and **any wrapper shares the same authority surface as every other CLI client**. An autonomous outer agent is not `actor: ai_autonomous` at the schema level; it has no privileged row-write path; it cannot reach into the DB directly. If it wants to act on what it observes, it issues CLI commands, the same commands a human operator would type. The wrapper is just another client. The schema still enforces field-level actor constraints, required-when predicates, and lifecycle guards regardless of who is on the other end of the CLI invocation.

The trap to resist is giving that outer layer a back-channel: "let the wrapping orchestrator pause `drive`, or inject a state transition, or signal the substrate through some side path." The temptation is real — it feels like coordination. What it actually does is push orchestration up a level, outside the schema's reach; break the substrate's atomicity, since a pause is now an unverified second write path the DB does not know about; and introduce a class of failures the framework cannot detect or log. The answer is not a richer inter-layer protocol. The answer is that the outer layer drives everything through the CLI, same as anyone else, and the substrate's guarantees hold unconditionally.

The autonomous-flow daemon, the `agents.yaml` / `policies.yaml` files, and the builtin subscribers are *not* wrappers — they are substrate-internal services that share `stores`'s binary, schema, and write-path enforcement. What lives outside is genuinely external: project-side scripts (`./dev`), human-operator wrappers, observing orchestrators, deployment-specialist agents that aren't builtins. The line between "in" and "out" is whether the code ships in the `stores` binary itself.

## The deeper bet

Most agent frameworks treat the LLM as the cognitive center and the surrounding system as scaffolding. `stores` inverts that: **the schema is the cognitive center, and the LLM is a constrained worker that fills in slots the schema demands.** The framework doesn't ask "what does the agent want to do?" — it asks "what does the next row require, who is allowed to write it, and what predicate must hold?" The agent's job is to produce a value that satisfies the schema. The schema's job is to make sure no work happens without intent, and no intent goes uncaptured.

It's a bet that the durable assets in an AI-collaborative workflow are **not the prose**, but **the typed, validated, actor-attributed rows** — and that if you make those cheap to define and impossible to bypass, the prose can be rendered from them whenever anyone needs to read it.

## Revision history

- **v1.1** (2026-05-03) — added at-filing contract ratification (T013/L029); two-gate operational frame; autonomous-flow layer (T014/L018+L022+L026); daemon vs. wrapper boundary clarification.
- **v1.0** — initial draft.
