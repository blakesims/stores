---
name: gate:walk
description: Walk Blake through pending gate items (decisions + scripts) one at a time, with AI assistance. Resolves the human-only inbox without context-switching.
requires_stores: [gate]
default_invoker: human
user_invocable: true
---

# Walk gate items

Blake has a queue of human-only items in the gate store: decisions only Blake
can answer (business / scope / design) and scripts only Blake can run (sudo
deploys, secret rotations). This skill walks them sequentially, helping with
context but never answering on Blake's behalf.

## Why default_invoker is `human`

The gate's `answer` field has `actor: human` in its schema. The CLI auto-
detects `$CLAUDECODE` and would otherwise infer `ai_autonomous`, which the
schema rejects. Pass `--invoker human` explicitly on every `answer` call.
Without it, the answer write fails with a clear actor-mismatch error.

## Discover

```bash
stores gate --help
stores gate schema --json
stores gate list --status pending --json
```

## The walk

For each pending item:

1. **Print it** — the question, options (if decision) or command (if script),
   any `task_ref` link.
2. **Add context** — read the linked observation/task if `task_ref` is set:
   `stores observations show <ref-id>` or equivalent. Surface the relevant
   bits of context Blake needs to decide. Don't editorialize.
3. **Pause for Blake.** This is the load-bearing moment: you do not answer.
   Wait for Blake's choice (if decision) or for Blake to confirm they ran
   the script (if script).
4. **Record the answer:**

```bash
# Decision Blake answered:
stores gate answer <gate-id> --answer "<choice>" --invoker human

# Decision Blake wants to defer:
stores gate defer <gate-id> --until "<date>" --reason "<why>" --invoker human

# Decision Blake wants to cancel:
stores gate cancel <gate-id> --reason "<why>" --invoker human
```

5. Move to the next pending item.

## When NOT to use this skill

- Single gate item: just answer it directly via `stores gate answer ...`. The
  walk overhead isn't worth it.
- Item needs deeper investigation: stop the walk, hand context to a separate
  research session, come back later.

## Rules

- Never call `stores gate answer` without `--invoker human`. The CLI rejects
  it; you'd be working around schema enforcement.
- Surface context, do not decide.
- One item at a time. No batching.
- If you spot follow-up work for Blake while walking (e.g. a decision implies
  a new task), file it as a fresh gate item or task entry — don't cram it
  into the current item's resolution.
