## Dogfooding: use the system to build the system

**Rule.** This repo's task workflow runs THROUGH the `stores` substrate, not alongside it. Tasks live as rows in `.stores/db.sqlite`. Agents are spawned by `stores tasks drive`. Reviews submit via `stores tasks submit-*`. Markdown files in `tasks/` are projections written by `stores tasks render`, not hand-edited.

**Why.** Twelve tasks of careful planning and building taught us a lot. The thirteenth, where the orchestrator first tried to scaffold via `stores tasks add`, hit an obvious gap in 30 seconds: the substrate auto-mints `T001` with no `--display-id` override, so substrate IDs and filesystem-scanned IDs diverged immediately. No amount of design review caught that. **One real attempt to use the system did.** That ratio is the rule. Real use surfaces what real use surfaces; planning, code review, and tests do not replace it. The substrate's quality ceiling is set by what surfaces when actual work runs through it.

### The verbs you'll actually use

- `stores tasks add --invoker ai_with_human --title ... --slug ... --done-when ... --scope-in ... --scope-out ...` — scaffold a new task. Substrate auto-mints the ID.
- `stores tasks render <id>` — write the readable `main.md` projection of the row. Use this for diff/PR/CodeRabbit consumption.
- `stores tasks drive <id>` — spawn the planner → plan-reviewer → executor → code-reviewer → wrap cycle. Substrate-driven, not orchestrator-driven.
- `stores tasks status <id>` and `stores tasks next-action <id>` — observe the workflow without driving it.
- `stores tasks brief <id> <agent-role>` — preview the brief drive would dispatch (debugging without spawning).

### `--invoker ai_with_human`

The substrate detects `$CLAUDECODE` and treats writes as `ai_autonomous` by default. Fields marked `actor: ai_with_human` (e.g. `title`, `slug`) reject autonomous writes. **Pass `--invoker ai_with_human` whenever you (Claude) are operating in a session with the user actively in the loop** — which is true any time you're conversing with them. This is the wrapper boundary in action: the wrapping agent uses the CLI like any other client and does not get a privileged channel. See `docs/philosophy.md` § *What's outside the substrate*.

### Bugs are observations, not blockers

When the substrate hurts mid-task, **do not retreat to hand-editing markdown**. File the friction in the observations store via `stores observations add --invoker ai_with_human ...` (run `--help` for the exact field names your schema requires). The `tasks` schema has `linked_observations: list_fk, ref: observations` — link the observation to the task that surfaced it so the bug is discoverable next to the work that found it. The hurt IS the data we wanted; working around it silently throws that data away.

If the substrate is so broken you genuinely cannot proceed: file the observation, fall back to markdown for that one task, and open a fresh task to fix the substrate. Don't let the recursion stall you, but don't give up the dogfood without first paying the observation tax.

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
