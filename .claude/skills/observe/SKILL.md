---
name: observe
description: File a substrate-friction observation correctly. Use the moment you notice anything unexpected about the substrate — a missing flag, a confusing error, a workflow surprise. Filing is autonomous; do not gate on the user.
user_invocable: true
---

You are filing an observation about substrate friction. **Filing is autonomous (`ai_autonomous`)** — you do not need user assent. Friction goes to a row immediately or it dies.

For dogfood context, see `/CLAUDE.md` and `stores/observations/CLAUDE.md`.

## When to invoke

You (the agent) invoke this skill any time you notice friction with the substrate during other work. The user can also invoke it directly to file something they noticed. Either way, the protocol is the same.

Bar for filing: **you noticed friction**. Don't filter at filing time. Triage happens in the observation lifecycle, not before the row exists.

Examples of file-worthy friction:
- A CLI flag is missing or named differently than you assumed.
- An error message doesn't tell you what went wrong.
- A required field has no obvious value (forcing you to invent one).
- A workflow step times out, hangs, or returns wrong data.
- A schema constraint rejects something that should plausibly be allowed.
- You found yourself reaching for `git mv`, `set_var`, or any other "I'll just bypass" shortcut.
- The substrate's own docs (CLAUDE.md, schema, READMEs) misled you.

## The shape of an observation

You need at minimum:

- **summary** (one line, ≤80 chars)
- **source** = `dev` (always, for substrate-friction; other source values are for production-system observations the user files)
- **priority** = `high | normal | low`
  - `high` = blocks current work or breaks the dogfood loop itself
  - `normal` = friction worth fixing, doesn't block
  - `low` = nit / cosmetic / wishlist
- **captured_at** = ISO timestamp (use `date -Iseconds`)
- **captured_week** = ops label like `w<isoweek>-d<weekday>` (use `w$(date +%V)-d$(date +%u)`)
- **task_id** (optional but recommended) = display id of the task that surfaced this friction (e.g. `T013` for substrate-T013, or empty if no task is in flight)
- **body** (recommended) = the longer description: what you tried, what you expected, what happened, whether it's reproducible

## The verb

```bash
stores observations add --invoker ai_autonomous \
  --summary "<one-line>" \
  --source dev \
  --priority <high|normal|low> \
  --captured-at "$(date -Iseconds)" \
  --captured-week "w$(date +%V)-d$(date +%u)" \
  --task-id "<surfacing-task-display-id-or-omit>" \
  --body-from-file <(cat <<'EOF'
<longer description here. Multi-line OK. State what you tried, what you expected, what happened, and whether it's reproducible. If a workaround exists, describe it. Reference any related observations or tasks by id.>
EOF
)
```

Returns the new L-id (e.g. `L004`). Capture it; reference it in commit messages and any related work.

## After filing

- If you filed during another piece of work: continue that work. The observation is in the queue; the next `/pickup` will surface it. Do NOT stop to investigate the observation now — that's a different turn, with the user, with a triage decision.
- If the user invoked `/observe` directly: confirm the L-id back to them and exit. Do not auto-investigate.
- Reference the L-id in the next commit message that touches related code, even if the commit doesn't fix the observation. This builds the audit trail.

## Substrate-down escape

If `stores observations add` itself fails (substrate uninitialized, schema error, runner crash), do NOT swallow the error. Write a worklog note instead:

```bash
# Make a worklog note via the project's note script:
docs/worklog/new-note.sh substrate-down-<short-slug>
```

Fill the worklog note with the same content the observation would have carried, plus what broke about the substrate itself. When the substrate recovers, file the observation properly and reference the worklog note in the body.

The discipline: **never let friction die silently**, even when the substrate is the thing that's broken.

## Anti-patterns

- **Filing as `--invoker ai_with_human`** — wrong; filing is autonomous. The user-authority moments are `investigate` / `confirm` / `wont_fix` / promotion, not filing.
- **Filtering at filing time** — "is this worth filing?" is a triage question, not a filing question. File first.
- **Investigating before exiting** — that's a different turn with the user. `/observe` files and exits.
- **Filing the same friction twice** — check `stores observations list --json | grep -i <keyword>` first; if a recent open observation already covers it, add a comment via `stores observations update` (with `--invoker ai_autonomous` if the field allows) instead of duplicating.
- **Inventing a `task_id` that doesn't exist** — leave it empty if no task surfaced this. The field is optional and a soft-FK; a wrong value is worse than no value.

## Output

After filing, print one line:

```
Filed L0XX (priority=<X>, summary="<...>", task_id=<id-or-none>).
```

Then return to whatever the surrounding work was.
