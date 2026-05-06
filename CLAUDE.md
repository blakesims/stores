## Dogfooding: use the system to build the system

**Rule.** This repo's task workflow runs THROUGH the `stores` substrate, not alongside it. Tasks live as rows in `.stores/db.sqlite`. Agents are spawned by `stores tasks drive`. Reviews submit via `stores tasks submit-*`. Markdown files in `tasks/` are projections written by `stores tasks render`, not hand-edited.

**Why.** Twelve tasks of careful planning and building taught us a lot. The thirteenth, where the orchestrator first tried to scaffold via `stores tasks add`, hit an obvious gap in 30 seconds: the substrate auto-mints `T001` with no `--display-id` override, so substrate IDs and filesystem-scanned IDs diverged immediately. No amount of design review caught that. **One real attempt to use the system did.** That ratio is the rule. Real use surfaces what real use surfaces; planning, code review, and tests do not replace it. The substrate's quality ceiling is set by what surfaces when actual work runs through it.

### The verbs you'll actually use

- `stores tasks add --invoker ai_with_human --title ... --slug ... --done-when ... --scope-in ... --scope-out ...` — scaffold a new task. Substrate auto-mints the ID.
- `stores tasks render <id>` — write the readable `main.md` projection of the row. Use this for diff/PR/CodeRabbit consumption.
- `stores tasks drive <id>` — spawn the planner → plan-reviewer → executor → code-reviewer → wrap cycle. Substrate-driven, not orchestrator-driven.
- `stores tasks status <id>` and `stores tasks next-action <id>` — observe the workflow without driving it.
- `stores tasks brief <id> <agent-role>` — preview the brief drive would dispatch (debugging without spawning).

### Cross-project filing (`--meta` / `STORES_META_PATH`)

When friction surfaces while doing client work, file it against the stores substrate without context-switching: pass `--meta=<PATH>` (or set `STORES_META_PATH=<PATH>` once and use bare `--meta`) to route a single CLI invocation at a META substrate. The META substrate is just another `.stores/` — by convention, the stores repo itself. The flag is global, so it works uniformly with `intake add`, `observations add`, `tasks add`, `tasks render`, etc. Bare `--meta` with `STORES_META_PATH` unset, or a path that doesn't contain a `.stores/` directory, errors fail-loud — no silent fallback to CWD.

### `--invoker` discipline (the strict rule)

The substrate detects `$CLAUDECODE` and treats writes as `ai_autonomous` by default. Fields marked `actor: ai_with_human` reject autonomous writes; fields marked `actor: human` reject any AI write. This is the wrapper boundary T011 documented, schema-enforced.

**Default is `ai_autonomous`. Never silently upgrade.** Most of the time (~80-90%) the AI operates this repo autonomously and the right invoker is `ai_autonomous`. Existing-of-a-session does NOT count as "human in loop." The session is just where the AI happens to be running; it is not consent to write rows the human hasn't seen.

**Use `--invoker ai_with_human` only when the human just authorized this exact row in this turn.** Concretely: you've shown the user the row you're about to write, they typed assent (a "yes" / "go" / equivalent to that proposal), and you're writing it now. Not "the user is presumably available." Not "the user kicked off this work an hour ago." This turn, this row, just-said-yes.

**The three user-authority moments (U1, U3, U4) are the only places `ai_with_human` is appropriate:**

- **U1 — Contract ratification.** The human approves an observation's `intent_contract` (the dominant path) or directly authors a task contract (legacy / escape hatch). Two forms:
  - **Observation-first (post-T020 — the path you should default to):** `observations update LXXX --contract-state ready --approved-by blake --approved-at <now> --invoker ai_with_human --approve-token <T>`. The auto-promote subscriber (L046) creates the task autonomously within ~5s; the human does **not** separately approve "promotion." U1 covers ratification; the framework-fired task creation is grounded transitively by the human-approved upstream contract.
  - **Direct-task (escape hatch):** `tasks add --invoker ai_with_human --title ... --slug ... --done-when ... --scope-in ... --scope-out ...` for tasks born without an observation (emergency hot-fix; infrastructure work where filing→ratify→promote is overkill). Prefer observation-first when an observation would do; the observation row is the durable surface and the dominant path.
- **U3 — Acceptance.** `tasks accept` / `tasks reject`. Tier-A (token-mediated): the user either types the verb (`--invoker human`) OR pre-authorizes via `--invoker ai_with_human --approve-token <T>` and the AI executes.
- **U4 — Resume / amend.** `tasks resume` (blocked → ready), `tasks amend` (rejected → planning). The human must have seen the blocker / rejection and authorized the unblock.

(The earlier "U2 — Promotion" moment is folded into U1: with auto-promote, ratifying the contract IS the act that produces the task. There is no longer a separate U2.)

Each U-moment now has two equivalent grounding paths:

(a) **`--invoker human`** — the user types the verb themselves (works for any U-moment, required where the field is `actor: human` and no token is presented).
(b) **`--invoker ai_with_human --approve-token <T>`** — the user pre-authorized the row by decrypting the approval token (passphrase / hardware tap) and pasting it into chat; the AI executes the write with the token attached. The substrate verifies the token via constant-time hash-equality and accepts the write under tier-A semantics.

Both paths are equally valid grounding. Pick (a) when the user is at the keyboard for this exact verb; pick (b) when the user has pre-authorized a session of work and wants the AI to execute without typing each verb.

**Everything else is `ai_autonomous`:** every `submit-*` during a drive cycle, every `observations add` for friction encountered mid-work, every `tasks render` / `tasks status` / `tasks next-action`, every read.

**Halting is a feature.** When autonomous work hits a moment that needs U1–U4 grounding, **halt and propose**, then write `ai_with_human` after assent. Do not pre-seek consent for autonomous moments; do not skip consent for grounding moments. The schema's rejection of an undergrounded write is the fail-loud signal we want — getting rejected is not an error to recover from, it's the substrate doing its job.

See `docs/philosophy.md` § *What's outside the substrate*.

## Approval-token doctrine (tier-A / tier-B)

The substrate distinguishes two grades of "human-grounded" writes:

- **Tier-A — `actor: human` gates (token-mediated).** Fields and transitions marked `actor: human` in `schema.yaml` are the highest-stakes assent moments: task acceptance/rejection, observation `confirm`, `intent_contract.approved_by/at`, and similar. They accept EITHER `--invoker human` (the user typed the verb) OR `--invoker ai_with_human --approve-token <T>` where `<T>` is the user's decrypted approval token. The token is generated at `stores auth init`, age-encrypted at rest under the user's age recipient at `~/.config/stores/approve.token.age` (gitignored, mode 0600), and can only be decrypted with passphrase or hardware tap. The substrate verifies the presented token by constant-time hash-equality. `ai_autonomous` is rejected even with a valid token — the token does not relax the AI-only case.

- **Tier-B — `actor: ai_with_human` gates (honor-system, no token).** Fields and transitions marked `actor: ai_with_human` accept `--invoker ai_with_human` without a token. These are lower-stakes "human in the loop" moments where the AI's ask-first discipline is the runtime protection. The schema does not enforce token possession here; it enforces only that the invoker is not `ai_autonomous`.

- **`ai_autonomous` is the autonomous default.** Everything not marked `actor: human` or `actor: ai_with_human` accepts `ai_autonomous` writes and is the AI's autonomous workspace.

### Threat model

The token mechanism's purpose is to make it **mechanically impossible for the AI to fabricate human assent on tier-A writes**. The threat is not malice; it's drift — an AI that gradually marks more rows `ai_with_human` because "the user is presumably available." The token closes that drift on tier-A:

- **At rest:** the token is age-encrypted to the user's recipient. The on-disk file does NOT contain the raw secret. `stores auth init` REFUSES to initialize with a raw-plaintext age key — only passphrase-protected or hardware-backed keys are accepted. This means decryption requires user-presence (passphrase entry or hardware tap); the AI cannot decrypt unattended.
- **Decryption is user-presence-bound.** `stores auth show` invokes `age` interactively — the user is prompted for the passphrase or asked to tap a hardware token. The AI cannot script around this.
- **Once decrypted into chat, the AI possesses it for the session.** This is the residual exposure: after the user pastes the token into chat, it lives in the conversation context until the session ends. The runtime protection during this window is the AI's ask-first behavioral discipline — propose the row, wait for assent, attach the token to the proposed write. Session-end clears the AI's context window, so the exposure is bounded to one session.
- **Pre-session and post-session, the AI cannot fabricate tier-A assent.** The cryptographic gate (decryption requires user-presence) plus the schema gate (constant-time hash-equality) means a tier-A write without a real, recently-decrypted token is rejected fail-loud.

The doctrine is: tier-A is cryptographically gated; tier-B is honor-system; `ai_autonomous` is autonomous. Drift across the tiers is what the schema fights.

### Bugs are observations, not blockers

When the substrate hurts mid-task, **do not retreat to hand-editing markdown**. Prefer the intake gate for autonomous local friction: `stores intake add --invoker ai_autonomous` lands raw signal in `draft` for gatekeeper classification before it becomes an observation. Filing friction is autonomous work, not a U-moment.

```bash
stores intake add --invoker ai_autonomous \
  --summary "<one-line>" \
  --source-agent "<planner|executor|code_reviewer|orchestrator|...>" \
  --captured-at "$(date -Iseconds)" \
  --captured-week "w$(date +%V)-d$(date +%u)" \
  --source-task "<surfacing-task-display-id>" \
  --body "<longer description; --body-from-file for multi-line>"
```

`stores observations add` remains valid as the explicit escape hatch for human / `ai_with_human` filings and for gatekeeper routing side-effects. Direct observation add is not blocked by this rule; it is no longer the default autonomous-local-friction path.

Intake items get `I{:03d}` IDs (`I001`, `I002`, …). Routed observations get `L{:03d}` IDs (`L001`, `L002`, …) — distinct from tasks' `T###`. The `source_task` / observation `task_id` fields are soft-FKs (plain text, no referential guard) — set them to the display id of the task that surfaced the friction.

**Observations carry their own triage tier** via their `intent_contract.tier_hint` (T0 / T1 / T2 / T3):
- **T0** — doctrinal-only. **Do not file.** Edit `CLAUDE.md` (or the relevant doc) directly. T0 is the class of change too small or too implicit to deserve a substrate row; there is no observation, no task, no cycle.
- **T1 / T2** — handled inside the observation lifecycle (`investigate` → draft contract → `confirm` (U-moment) → `claim` → `resolve`). No separate task row.
- **T3** — promoted to a full task: `stores tasks add --invoker ai_with_human --linked-observations L00X ...`. The observation gets `resolved` with `resolution` referencing the task once the task ships.

**Per-tier drive-cycle shape (T027):** when an observation does promote to a task, the cycle bends to its tier — schema-enforced via `when:` predicates on `StateAction`s, not runtime branching:
- **T1 — contract-is-plan.** Skips planner + plan_reviewer entirely. The framework fires `skip-plan` on the `planning → ready` edge; zero plan-stage subagent spawns.
- **T2 — one-phase plan.** Planner + plan_reviewer run, but `submit-plan` rejects any plan whose `phases.length != 1`. Plan shape is schema-enforced.
- **T3 — full cycle.** Multi-phase planner → plan_reviewer → executor → code_reviewer → wrap, unchanged.

See `docs/philosophy.md` § *Tier-structural drive cycle (T027)*.

If the substrate is so broken you cannot even file an observation: write a worklog note (`docs/worklog/<date>/NN-substrate-down-<slug>.md`) describing what broke and what you hand-edited, then open a fresh task (or observation, when substrate recovers) to address it. The worklog note IS the audit trail for the substrate-down period — git-tracked, timestamped, discoverable.

### Triage routing (the L043 rule)

When friction surfaces, **the orchestrator's job is to route, not investigate**. The discipline:

1. **≤3 cheap tool calls** (a file read, a grep, a quick command) to triangulate.
2. If the root cause is obvious within that budget — fix it (or file an observation describing the fix shape) and move on.
3. If not — file the observation with `intent_contract.tier_hint` set, then **STOP**. Either flag `status=needs_investigation` (when L043 ships an investigator subagent) or halt and ask the user how to route.

**Never start a 15-tool-call inline investigation as the orchestrator-on-main.** That is the L043 anti-pattern: the user-facing thread blocks on multi-minute reads while a subagent could have carried the dive in parallel without holding the orchestrator's context. The pain that earned this rule was a long L042 misdiagnosis followed by a long `eval_length` root-cause hunt, both of which should have been routed to a fresh subagent (or filed and halted) instead of swallowing the main thread.

The L043 investigator subagent (filed; awaiting promotion) closes the substrate primitive for this. Until it ships, enforce the rule as orchestrator discipline.

### The great divide on IDs

Tasks `fs/T001`–`fs/T012` lived only in the filesystem (`tasks/completed/`). The substrate database starts empty and counts up from `T001` again. Substrate-`T001` is "the first task done the new way" — it is not the same as `fs/T001`. **Don't try to reconcile.** Don't backfill placeholder rows. If you need to reference a pre-substrate task in writing, prefix it `fs/` (e.g. `fs/T012`) to disambiguate. The filesystem T001–T012 are the historical record; the substrate is the source of truth from substrate-`T001` onward.

### Workspace hygiene

Commit docs as soon as you write them — uncommitted files in main block accept-merge (one dirty `CLAUDE.md` line stalled T023's deploy).

### Session doctrine — 2026-05-06: pragmatic escape from broken dogfood (NEVER raw-SQL the DB)

The pure-dogfood rule says: drive substrate work through substrate verbs. We've already proven the system end-to-end (T021 onward); we don't need to re-prove that ratio on every task. Today's working rule trades ceremony for throughput when the substrate is too broken to drive its own fix:

1. **File substrate friction as observations.** Always. The pain is the data, even when we work around it. Filing is `ai_autonomous`, not a U-moment.
2. **Try the substrate path first** with a cheap budget (≤3 verb calls). If verbs work, ship via verbs.
3. **If two or more substrate bugs interlock and block the fix path, escape to direct code edits.** Use Edit/Write, spawn subagents for parallel investigation/work, run normal `cargo test` / git cycles. The branch + commit + linked-observation reference in the commit message is the audit trail. Don't burn the session re-proving a ratio that's already been proven.
4. **Hard rule: NEVER raw-SQL the substrate DB.** No `sqlite3 .stores/db.sqlite UPDATE/DELETE/INSERT`, ever. Direct DB writes bypass actor gates, transition history, validators, and on-entry hooks — the entire safety surface that makes the substrate trustworthy. If you reach for `sqlite3 ... UPDATE`, that's the signal to fix the broken handler in code instead. Same cost, infinitely more correct. *Reading via `sqlite3 ... SELECT` is fine — read-only is not a substrate write.*
5. **Name the friction in the commit/PR.** "Couldn't dogfood because L116 + L117 interlock; direct fix tracked via L###" — so the next reader sees both the path taken and why.

**Why this rule today:** the orchestrator (me) hand-edited `dispatch_locks` and `tasks` rows via raw SQL while routing around L116 (seeder race) to attempt a Pi-runner E2E on T036. That was wrong. Hand-editing markdown was already off the table; this codifies that hand-editing the DB is also off the table, with a clear pragmatic escape (edit code) so the orchestrator doesn't get stuck choosing between "violate the doctrine" and "stall indefinitely on interlocking substrate bugs."

**When to revisit:** when L116 + L117 ship and the dogfood path is restored end-to-end, tighten the rule back toward "always dogfood unless [extreme circumstance]." This is a working rule for 2026-05-06, not a permanent relaxation.

### Codex review as the in_review gate (2026-05-06)

When a task hits `in_review`:

1. Rebase the task's branch onto current `main` (codex "deleted X" findings are often stale-base artifacts).
2. Run codex against the branch diff. If `/codex:review` fails with bwrap errors, fall back to `cd <worktree> && codex exec --dangerously-bypass-approvals-and-sandbox --color never "<focus prompt>"`.
3. PASS / cosmetic-only → `tasks accept <id> --invoker ai_with_human --approve-token <T>`.
4. Substantive findings, non-critical → direct-edit the worktree, commit as `<TID> codex-revise: <summary>`, re-run codex, loop until PASS.
5. Critical / architectural findings → halt and surface to the user.

### What NOT to do

- Don't retreat to hand-editing markdown when the substrate hurts. The pain is the data.
- Don't raw-SQL the substrate DB (see § *Session doctrine — 2026-05-06* above). Reads are fine; writes are forbidden.
- Don't paper over a substrate bug with a workaround in the task content. File the observation; then either fix the substrate (in this same task or a fresh one) or work around it explicitly so the next reader sees the friction.
- Don't backfill placeholder rows to "align" filesystem and substrate IDs. The great divide is a feature.
- Don't give the orchestrator agent privileged channels into the substrate (e.g. "let me pause drive"). Re-read `docs/philosophy.md` if tempted. The answer is no.
- Don't dive deep on observations as the orchestrator. See *Triage routing (the L043 rule)* above. ≤3 cheap tool calls then halt-or-route; never inline-investigate.
- Don't hand-crank ratified-but-unpromoted observations through `./dev new`. That's the orphan-prone pattern the auto-promote subscriber (L046, in T020) obsoletes — re-keying contract content into `./dev new` flags drifts the contract from the observation and leaves both rows un-linked. Ratifying the observation's contract is the only step the human does; auto-promote carries the rest.

### Pointers

- `tasks/CLAUDE.md` — task lifecycle protocol (status state machine, section ownership, orchestrator rules). Still applies — the DB is just the new source of truth.
- `docs/philosophy.md` — the substrate's design principles. § *What's outside the substrate* is the doctrine that grounds `--invoker` enforcement and the wrapper boundary.
- `docs/architecture-coherence.md` — doctrine that local correctness is not architectural coherence (T045); grounds the gatekeeper / intake / architecture-review layer.
- `docs/primitives.md` — the typed primitives the substrate composes from (working draft, with changelog). Read here before proposing schema-shape moves.
- `docs/engine-health.md` — long-standing snapshot of where the engine bleeds, what's filed against each weakness, and what's already shipped. **Keep this up to date** at inflection points: when a batch of fixes lands (move obs to ✅), when a new high-priority bug surfaces (add a row), or when a bug class is named that wasn't previously visible (add a Layer or GAP). The doc has a self-update section at the bottom; follow it. The worklog under `docs/worklog/<date>/` carries session detail; promote insights to engine-health when they become long-standing.
- `docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md` — the design discussion behind the dogfood decision.
- `docs/worklog/2026-05-02/03-t012-workspace-path-and-next-id.md` — the substrate hooks (`workspace_path`, `next-id`) shipped in T012 to make multi-worktree dogfooding safe.

---

## Docs

See `.notes-config.yml` for the worklog / refs / sweep system.
