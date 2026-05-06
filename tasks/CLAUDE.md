---
description: Task lifecycle protocol — substrate is truth, this file is explanatory.
---

## Tasks under dogfood

**The DB is the source of truth.** Rows in `.stores/db.sqlite` (table `tasks`) drive the workflow. Markdown files in `tasks/{planning,active,paused,completed}/` are projections written by `stores tasks render <id>`, not hand-edited. Status transitions happen via substrate verbs, not by `git mv`. This file is *explanatory* — it documents what the substrate enforces, so you can read what `stores tasks next-action <id>` will return next without reverse-engineering the engine.

For the dogfood rule, the verbs you'll use, and `--invoker` discipline, see the project root `CLAUDE.md`. This file assumes you've read that.

### Pun discipline

Two senses of "task" coexist forever:
- **`s/T###`** = substrate-task (a row in `.stores/db.sqlite`). Authoritative for everything from substrate-T001 onward.
- **`fs/T###`** = filesystem-task (a folder in `tasks/{planning,active,paused,completed}/`). Authoritative for the historical record `fs/T001`–`fs/T012`.

Bare `T013` is ambiguous; treat it like an unqualified pronoun in review. The substrate database starts empty and counts up from `T001`; substrate-`T001` is **not** the same as `fs/T001`. Don't reconcile.

For pre-substrate work the readable record is `tasks/completed/T0XX-*/main.md` (no substrate row). For post-substrate work the row is canonical and the markdown is its render. Don't hand-edit a rendered file — re-render it from the row.

### Lifecycle (state machine, substrate-enforced)

| State | Meaning | Markdown projection lives in |
|-------|---------|------------------------------|
| `planning` | Planner agent drafting the plan | `tasks/planning/` |
| `plan_review` | Plan-reviewer evaluating | `tasks/planning/` |
| `ready` | Plan approved, awaiting execution | `tasks/active/` |
| `executing` | Executor working a phase | `tasks/active/` |
| `code_review` | Code-reviewer evaluating phase output | `tasks/active/` |
| `blocked` | Substrate paused on guard failure or REVISE-exhaustion | `tasks/paused/` |
| `complete` | All phases done | `tasks/completed/` |
| `in_review` | Wrap synthesis ready for the user | `tasks/completed/` |
| `accepted` | User accepted the wrap | `tasks/completed/` |
| `rejected` | Reviewed and rejected on the merits; awaiting amend | `tasks/completed/` |
| `abandoned` | Intentionally retired as superseded, misadded, duplicate, or stale | `tasks/completed/` |

Transitions are state-machine-enforced by the schema (`stores/tasks/schema.yaml` § `lifecycle.transitions`). The substrate refuses transitions that don't match a defined edge or fail the edge's guard. **Don't try to bypass.** If you find yourself wanting to set a status manually, the right move is either (a) the substrate's transition verb (`submit-plan`, `submit-execute`, `submit-review`, `accept`, `reject`, `amend`, `resume`, `abandon`) or (b) file an observation about the missing transition you wished for.

Three terminal/history states mean different things: `rejected` = reviewed-and-rejected-on-merits; `abandoned` = intentionally-retired (superseded/misadd/duplicate/stale); `closed_out_of_band` = work-shipped-via-manual-commit. `abandoned` is the non-destructive L002 alternative to raw rollback/delete or wiping `.stores/db.sqlite` for stale or misadded task rows.

### How `stores tasks drive` works

`stores tasks drive <id>` is the engine. It loops:

1. Read row, compute `next_action` from current state + transitions.
2. If next action is `dispatch_agent: <role>`, render the brief from `templates/<role>-brief.md.tpl`, spawn the agent via the configured runner.
3. The spawned agent does its work and writes back via the appropriate `submit-*` verb (`ai_autonomous` invoker — these are mid-cycle writes, not U-moments).
4. Substrate validates the envelope against the role's JSON schema, applies the transition (subject to guards), and loops.
5. Drive exits when the row reaches a terminal/history pause (`complete`, `accepted`, `rejected`, `abandoned`, `blocked`).

The orchestrator (you, in the outer Claude Code session) does NOT spawn agents directly. Drive does. Your job is to invoke `drive` and observe — `stores tasks status <id>` and `stores tasks next-action <id>` are read-only telemetry. If drive errors or behaves surprisingly, file an observation; don't reach into the loop.

### The four user-authority moments (U1–U4)

The substrate defers to the user at exactly four kinds of moment, schema-enforced:

- **U1 — `tasks add`** (scope ratification). The contract — `done_when`, `scope_in`, `scope_out`, `assumptions` — is born with the row. `--invoker ai_with_human`. The user has just seen the proposed contract and assented in this turn.
- **U2 — `tasks add ... --linked-observations <L-id>`** (promotion). An observation has been promoted to a substrate task. Same invoker rule as U1; same just-assented requirement.
- **U3 — `tasks accept` / `tasks reject`** (terminal verdict). `actor: human` — the AI cannot do these at all. The user types the verb.
- **U4 — `tasks resume` (`blocked → ready`) / `tasks amend` (`rejected → planning`)** (unblock / re-open). `--invoker ai_with_human`. The user has reviewed the blocker or rejection and authorized the next move.

Everything else — every `submit-*` during a cycle, every `tasks render`, every read — runs `ai_autonomous`. **Halt and propose** when autonomous work hits a U-moment; do not silently upgrade your invoker.

### Iteration limits

| Situation | Cap | Substrate behavior |
|-----------|-----|--------------------|
| `REVISE` cycles per phase (code review) | 3 | 4th REVISE routes `code_review → blocked` (per schema guard `current_cycle <= 4`) |
| `NEEDS_WORK` cycles (plan review) | 3 | 4th NEEDS_WORK routes `plan_review → blocked` (per schema guard `plan_review_log.length < 3`) |

When the substrate transitions to `blocked`, it sets `blocked_reason`. Recovery is **U4** — the user reviews, authorizes resume, the substrate transitions `blocked → ready`.

### What this file does NOT contain

- Workflow narrative (read `docs/philosophy.md`)
- Schema field listings (read `stores/tasks/schema.yaml`)
- Skill-level operating instructions (read `.claude/skills/<skill>.md`)
- The dogfood rule itself (read `/CLAUDE.md`)
- Procedures for hand-editing markdown (deprecated — markdown is a projection)

### Task completion notes

When a task reaches `accepted`, the post-completion ritual is:

1. Run `docs/worklog/new-note.sh <task-slug>` to create a worklog note capturing what shipped, surprises, follow-ups, lessons.
2. Reference the worklog note from the row's `wrap_log` (or as a comment in the next observation, depending on convention).

The worklog is the *narrative* record, complementing the substrate's *structured* record. The substrate is for things with lifecycle; the worklog is for things with chronology. Different shape, different home.

### Pre-substrate (`fs/T001`–`fs/T012`)

These tasks lived in markdown only. They remain in `tasks/completed/` as the historical record. Don't touch them; don't backfill rows for them; don't try to "migrate" them. If you need to reference one in writing, prefix it `fs/`. The great divide is a feature.
