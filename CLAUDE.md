## Dogfooding: use the system to build the system

**Rule.** This repo's task workflow runs THROUGH the `stores` substrate, not alongside it. Tasks live as rows in `.stores/db.sqlite`. Agents are spawned by `stores tasks drive`. Reviews submit via `stores tasks submit-*`. Markdown files in `tasks/` are projections written by `stores tasks render`, not hand-edited.

**Why.** Twelve tasks of careful planning and building taught us a lot. The thirteenth, where the orchestrator first tried to scaffold via `stores tasks add`, hit an obvious gap in 30 seconds: the substrate auto-mints `T001` with no `--display-id` override, so substrate IDs and filesystem-scanned IDs diverged immediately. No amount of design review caught that. **One real attempt to use the system did.** That ratio is the rule. Real use surfaces what real use surfaces; planning, code review, and tests do not replace it. The substrate's quality ceiling is set by what surfaces when actual work runs through it.

### The verbs you'll actually use

- `stores tasks add --invoker ai_with_human --title ... --slug ... --done-when ... --scope-in ... --scope-out ...` — scaffold a new task. Substrate auto-mints the ID.
- `stores tasks render <id>` — write the readable `main.md` projection of the row. Use this for diff/PR/CodeRabbit consumption.
- `stores tasks drive <id>` — spawn the planner → plan-reviewer → executor → code-reviewer → wrap cycle. Substrate-driven, not orchestrator-driven.
- `stores tasks status <id>` and `stores tasks next-action <id>` — observe the workflow without driving it.
- `stores tasks brief <id> <agent-role>` — preview the brief drive would dispatch (debugging without spawning).

### `--invoker` discipline (the strict rule)

The substrate detects `$CLAUDECODE` and treats writes as `ai_autonomous` by default. Fields marked `actor: ai_with_human` reject autonomous writes; fields marked `actor: human` reject any AI write. This is the wrapper boundary T011 documented, schema-enforced.

**Default is `ai_autonomous`. Never silently upgrade.** Most of the time (~80-90%) the AI operates this repo autonomously and the right invoker is `ai_autonomous`. Existing-of-a-session does NOT count as "human in loop." The session is just where the AI happens to be running; it is not consent to write rows the human hasn't seen.

**Use `--invoker ai_with_human` only when the human just authorized this exact row in this turn.** Concretely: you've shown the user the row you're about to write, they typed assent (a "yes" / "go" / equivalent to that proposal), and you're writing it now. Not "the user is presumably available." Not "the user kicked off this work an hour ago." This turn, this row, just-said-yes.

**The four user-authority moments (U1–U4) are the only places `ai_with_human` is appropriate:**

- **U1 — Scope ratification.** `tasks add` (the row is born; the contract — `done_when` / `scope_in` / `scope_out` — is born with it). The human must have just approved the contract you're about to write.
- **U2 — Promotion.** Observation → task. `tasks add ... --linked-observations <obs-id>`. The user has seen the observation and assented to it becoming a task.
- **U3 — Acceptance.** `tasks accept` / `tasks reject`. These are pure `actor: human` — the AI cannot do them at all. The user types the verb.
- **U4 — Resume / amend.** `tasks resume` (blocked → ready), `tasks amend` (rejected → planning). The human must have seen the blocker / rejection and authorized the unblock.

**Everything else is `ai_autonomous`:** every `submit-*` during a drive cycle, every `observations add` for friction encountered mid-work, every `tasks render` / `tasks status` / `tasks next-action`, every read.

**Halting is a feature.** When autonomous work hits a moment that needs U1–U4 grounding, **halt and propose**, then write `ai_with_human` after assent. Do not pre-seek consent for autonomous moments; do not skip consent for grounding moments. The schema's rejection of an undergrounded write is the fail-loud signal we want — getting rejected is not an error to recover from, it's the substrate doing its job.

See `docs/philosophy.md` § *What's outside the substrate*.

### Bugs are observations, not blockers

When the substrate hurts mid-task, **do not retreat to hand-editing markdown**. File the friction in the observations store with `--invoker ai_autonomous` — filing friction is autonomous work, not a U-moment. The observation lands in the `open` state and shows up in the next `/pickup` queue.

```bash
stores observations add --invoker ai_autonomous \
  --summary "<one-line>" \
  --source dev \
  --priority high|normal|low \
  --captured-at "$(date -Iseconds)" \
  --captured-week "$(date +w%V)" \
  --task-id "<surfacing-task-display-id>" \
  --body "<longer description; --body-from-file for multi-line>"
```

Observations get `L{:03d}` IDs (`L001`, `L002`, …) — distinct from tasks' `T###`. The `task_id` field is a soft-FK (plain text, no referential guard) — set it to the display id of the task that surfaced the friction.

**Observations carry their own triage tier** via their `intent_contract.tier_hint` (T1 / T2 / T3):
- **T1 / T2** — handled inside the observation lifecycle (`investigate` → draft contract → `confirm` (U-moment) → `claim` → `resolve`). No separate task row.
- **T3** — promoted to a full task: `stores tasks add --invoker ai_with_human --linked-observations L00X ...`. The observation gets `resolved` with `resolution` referencing the task once the task ships.

If the substrate is so broken you cannot even file an observation: write a worklog note (`docs/worklog/<date>/NN-substrate-down-<slug>.md`) describing what broke and what you hand-edited, then open a fresh task (or observation, when substrate recovers) to address it. The worklog note IS the audit trail for the substrate-down period — git-tracked, timestamped, discoverable.

### The great divide on IDs

Tasks `fs/T001`–`fs/T012` lived only in the filesystem (`tasks/completed/`). The substrate database starts empty and counts up from `T001` again. Substrate-`T001` is "the first task done the new way" — it is not the same as `fs/T001`. **Don't try to reconcile.** Don't backfill placeholder rows. If you need to reference a pre-substrate task in writing, prefix it `fs/` (e.g. `fs/T012`) to disambiguate. The filesystem T001–T012 are the historical record; the substrate is the source of truth from substrate-`T001` onward.

### What NOT to do

- Don't retreat to hand-editing markdown when the substrate hurts. The pain is the data.
- Don't paper over a substrate bug with a workaround in the task content. File the observation; then either fix the substrate (in this same task or a fresh one) or work around it explicitly so the next reader sees the friction.
- Don't backfill placeholder rows to "align" filesystem and substrate IDs. The great divide is a feature.
- Don't give the orchestrator agent privileged channels into the substrate (e.g. "let me pause drive"). Re-read `docs/philosophy.md` if tempted. The answer is no.

### Pointers

- `tasks/CLAUDE.md` — task lifecycle protocol (status state machine, section ownership, orchestrator rules). Still applies — the DB is just the new source of truth.
- `docs/philosophy.md` — the substrate's design principles. § *What's outside the substrate* is the doctrine that grounds `--invoker` enforcement and the wrapper boundary.
- `docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md` — the design discussion behind the dogfood decision.
- `docs/worklog/2026-05-02/03-t012-workspace-path-and-next-id.md` — the substrate hooks (`workspace_path`, `next-id`) shipped in T012 to make multi-worktree dogfooding safe.

---

## Docs

See `.notes-config.yml` for the worklog / refs / sweep system.
