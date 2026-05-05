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
- **Autonomous flow is in-substrate.** A daemon (`stores agents run`) polls state transitions and dispatches subscribers declared in `agents.yaml` under a `policies.yaml` predicate layer. Default action: allow. NEVER policies are sacrosanct. Every automatic write records `policy_ref` and the policies-file hash on the row's `transition_history` audit trail. Builtin subscribers (`accept-merge`, `user-escalation`, and the post-accept ceremony chain stores' self-build uses to ship — `cargo-install`, `schema-migrate`) are one specialization of the subscriber primitive, not its definition; client projects declare their own deploy ceremonies in `agents.yaml` (e.g. `command: ./dev deploy prod`). The substrate is workflow primitives + lifecycle + per-field actor; it is not a deployment system. The engine consumes itself: it ships *inside* the substrate, not above it. Failure recovery follows the same shape — when a deploy ceremony fails, the substrate auto-promotes the failure into a fix task and drives it through the existing planner → executor → reviewer cycle. There is no specialized "blocker agent" role; the engine handles its own failures with its own primitives.

## Primitives

The substrate's typed primitives, composition rules, and known gaps are tracked in `docs/primitives.md` (working draft; see changelog there).

## What the substrate is FOR (from the human's perspective)

Stores exists to help the human make better decisions, quicker, and only where they need to. The scarce resource is human attention; the substrate's job is to protect it.

Filing is cheap; refinement is the substrate's burden. The inlet accepts anything — a clear bug with file paths and line numbers, a vague idea, a half-formed observation that may or may not duplicate an existing row. The substrate cooks each input until it reaches the density needed to flow past the next gate. Some arrive near-ready; some need many rounds of distillation (router-led grill questions, dedup checks, scope clarification). Both cross the same threshold; the work to get there scales with the input's entropy, not with the human's time.

When the substrate must ask the human something, it asks one thing at a time, with options the substrate generated, and the human's role is pruning by judgment — not validation of an LLM's draft. High-signal, low-noise.

This frames every design move: does it protect human attention, or burn it? `Direction(pull)` protects. The two-gate frame protects. Refinement-as-substrate-burden protects. The grill-me pattern protects. A push-shaped 12-field contract draft does not.

## What's outside the substrate

Worktree provisioning, project setup scripts, and observing wrappers — a Claude Code instance watching a long-running session, an outer orchestrator that spawned the whole thing — live outside the substrate. `stores` does not own these. They wrap `stores`; `stores` does not wrap them. This is not a gap to fill later; it is the correct boundary. The substrate's job is to enforce workflow structure. Everything above that layer is the caller's problem.

That boundary has one implication worth making explicit: the substrate has exactly one write path — the CLI — and **any wrapper shares the same authority surface as every other CLI client**. An autonomous outer agent is not `actor: ai_autonomous` at the schema level; it has no privileged row-write path; it cannot reach into the DB directly. If it wants to act on what it observes, it issues CLI commands, the same commands a human operator would type. The wrapper is just another client. The schema still enforces field-level actor constraints, required-when predicates, and lifecycle guards regardless of who is on the other end of the CLI invocation.

The trap to resist is giving that outer layer a back-channel: "let the wrapping orchestrator pause `drive`, or inject a state transition, or signal the substrate through some side path." The temptation is real — it feels like coordination. What it actually does is push orchestration up a level, outside the schema's reach; break the substrate's atomicity, since a pause is now an unverified second write path the DB does not know about; and introduce a class of failures the framework cannot detect or log. The answer is not a richer inter-layer protocol. The answer is that the outer layer drives everything through the CLI, same as anyone else, and the substrate's guarantees hold unconditionally.

The autonomous-flow daemon, the `agents.yaml` / `policies.yaml` files, and the builtin subscribers are *not* wrappers — they are substrate-internal services that share `stores`'s binary, schema, and write-path enforcement. What lives outside is genuinely external: project-side scripts (`./dev`), human-operator wrappers, observing orchestrators, deployment-specialist agents that aren't builtins. The line between "in" and "out" is whether the code ships in the `stores` binary itself.

## The deeper bet

Most agent frameworks treat the LLM as the cognitive center and the surrounding system as scaffolding. `stores` inverts that: **the schema is the cognitive center, and the LLM is a constrained worker that fills in slots the schema demands.** The framework doesn't ask "what does the agent want to do?" — it asks "what does the next row require, who is allowed to write it, and what predicate must hold?" The agent's job is to produce a value that satisfies the schema. The schema's job is to make sure no work happens without intent, and no intent goes uncaptured.

It's a bet that the durable assets in an AI-collaborative workflow are **not the prose**, but **the typed, validated, actor-attributed rows** — and that if you make those cheap to define and impossible to bypass, the prose can be rendered from them whenever anyone needs to read it.

## Pull from real use

The substrate is dogfooded by being used to build itself — but self-build alone pulls toward generalizations that fit only the self-build. (`cargo install` as the deploy verb is the canonical example: shipping it as a builtin made it look general, when it was only ever the self-build's deploy step.) The substrate stays honest by being driven by real client work in parallel, where the schema, the gates, and the deploy chain meet failure modes the self-build cannot surface — different deploy targets, different test gates, different gate categories, different lock-contention shapes. The dogfood doctrine ("use the substrate to build the substrate") is necessary; the realistic-pull doctrine ("AND use it on work where the substrate doesn't choose its environment") is what keeps it from collapsing inward. Generalizations that survive both pulls are durable; generalizations that survive only the self-build are leaks waiting to be discovered.

## Upstream autonomy: row-creation arrival (T020)

The autonomous-flow daemon subscribes to `transition_history` rows. By default a subscription names a non-empty `from` and `to` state, matching a lifecycle edge. The substrate also writes a synthetic create-event for every successful `add` (across all stores) with `from_status = ''` and `to_status = <initial-state>`. A subscription whose `transition.from` is the empty string therefore fires once per row creation, before any further state movement. This is the "planning-arrival" hook the upstream-autonomy chain stands on: `auto-promote` (`observations: confirmed → ready`) creates a tasks row at `planning`, and `auto-scaffold` (`tasks: '' → planning`) catches that creation event and provisions a worktree. The empty-string `from` is a convention, not a special case — the daemon's SQL match (`WHERE from_status = ?`) treats it like any other state token. The validator accepts empty `from`, but `to` must remain non-empty: a subscription with no destination state has nothing to match.

## Tier-structural drive cycle (T027)

Tasks are not all the same shape, and the drive cycle should not pretend they are. The substrate carries a `tier_hint` (`T0` / `T1` / `T2` / `T3`) on every task, and the lifecycle bends to it — not via runtime branching in agent code, but via schema-declared `when:` predicates on `StateAction`s. The same predicate language that guards transitions also gates whether a state-entry action fires. Tier shape is therefore a structural property of the workflow, visible in `schema.yaml`, audited in `transition_history`, and impossible to bypass.

The four tiers, by cycle shape:

- **T0 — doctrinal.** Lives in `CLAUDE.md` only; never filed as a row. T0 is a class of "pure-doctrine" change too small or too implicit to deserve a substrate task. Edit the doc directly. The substrate has no T0 row, so there is no cycle to skip.
- **T1 — contract-is-plan.** The ratified `intent_contract` already names objective, acceptance, scope. Re-running planner + plan_reviewer to "produce a plan" would just rephrase the contract. T1 tasks therefore skip both stages: the framework fires a `skip-plan` verb on the `planning → ready` edge, gated by a `when: tier_hint == 'T1'` predicate on the planning-state action. `transition_history` records the skip with `verb=skip-plan` and zero planner / plan_reviewer subagent spawns.
- **T2 — schema-constrained one-phase plan.** A T2 task gets a planner + plan_reviewer cycle, but the plan must be exactly one phase. `submit-plan` carries a `when: tier_hint == 'T2' && phases.length != 1` predicate that rejects multi-phase submissions at the schema gate. The plan-shape constraint is enforced by the substrate, not by reviewer judgment.
- **T3 — full cycle.** Multi-phase plans, full planner → plan_reviewer → executor → code_reviewer → wrap loop per phase. No tier predicate fires; the cycle runs as it always has.

The mechanism is the optional `when:` field on `StateAction` (an extension of the existing transition-guard predicate language). Tier shape is therefore declared in the same place the rest of the lifecycle is declared, and the engine honours it through the same evaluator. There is no tier-aware Rust branching anywhere in `tasks/drive`; the substrate evaluates predicates, the cycle bends.

This closes L030 (the tier-aware-cycle observation): the durable surface is the `when:` predicate, not a tier-aware Runner trait or a tier-keyed dispatch map.

## Revision history

- **v1.6** (2026-05-05) — tier-structural drive cycle (T027): T0 / T1 / T2 / T3 cycle shapes via `when:` predicates on `StateAction`. T1 skips planner+plan_reviewer; T2 plans constrained to one phase; T3 unchanged. L030 superseded.
- **v1.5** (2026-05-04) — added "What the substrate is FOR (from the human's perspective)": operating principle (high-signal, low-noise; protect human attention) + the steam-engine inlet metaphor (filing is cheap, refinement is the substrate's burden; same inlet for clear bugs and vague ideas; refinement depth scales to input entropy).
- **v1.4** (2026-05-04) — primitives extracted to `docs/primitives.md` (single source of truth, with changelog). Philosophy now references it in one line.
- **v1.3** (2026-05-03) — upstream-autonomy section: row-creation arrival convention (`from_status=''`) and the auto-promote / auto-scaffold builtins (T020).
- **v1.2** (2026-05-03) — substrate-vs-deployment-system distinction (subscribers are project-declared; cargo-install is one specialization, not the type); failure recovery as ordinary-task pattern; "Pull from real use" doctrine as complement to dogfood.
- **v1.1** (2026-05-03) — added at-filing contract ratification (T013/L029); two-gate operational frame; autonomous-flow layer (T014/L018+L022+L026); daemon vs. wrapper boundary clarification.
- **v1.0** — initial draft.
