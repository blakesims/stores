---
description: Observations store protocol — friction goes here, not into markdown.
---

## Observations under dogfood

Observations are how the system learns about itself. Filing one is autonomous (`ai_autonomous`); investigating, confirming, promoting, and won't-fix-ing are user-authority moments. The substrate enforces this via per-transition `actor:` declarations in `schema.yaml`.

For the dogfood rule, the verbs you'll use, and `--invoker` discipline, see the project root `CLAUDE.md`. This file assumes you've read that.

### When to file

File an observation **the moment** the substrate behaves in any way you didn't expect:

- A CLI flag is missing or named differently than you assumed.
- A required field has no obvious value (forcing the agent to invent one — that invention is the problem).
- An error message is unhelpful or misleading.
- A workflow step times out, hangs, or returns wrong data.
- A schema constraint rejects something that should plausibly be allowed.
- You found yourself reaching for `git mv`, `set_var`, or any other "I'll just bypass the substrate" shortcut.
- The next agent who sees this code would be confused and you can name why.

The bar is "you noticed friction." Don't filter for "is this important enough." Filter at triage, not at filing. Unfiled friction is data thrown away.

### The verb (the only one for filing)

```bash
stores observations add --invoker ai_autonomous \
  --summary "<one-line>" \
  --source dev \
  --priority high|normal|low \
  --captured-at "$(date -Iseconds)" \
  --captured-week "w$(date +%V)-d$(date +%u)" \
  --task-id "<surfacing-task-display-id>" \
  --body-from-file <(cat <<'EOF'
<longer description; what you tried; what you expected; what happened; reproducible?>
EOF
)
```

Required fields the agent must supply: `summary`, `source` (use `dev` for substrate-friction), `priority`, `captured_at` (ISO timestamp), `captured_week` (ops label like `w18-d5`). Optional but recommended: `task_id` (the task that surfaced the friction; soft-FK, just the display id), `body` (the longer description).

The substrate auto-mints an L-id (`L001`, `L002`, …) and lands the row in state `open`.

### The lifecycle (and where the user gates apply)

```
open ──investigate (U)──> investigating ──confirm (U, requires contract:ready)──> confirmed
                                            │
                                            └──request_info (auto)──> needs_info ──provide_info (human)──> confirmed
                                                                            │
                                                                            └──park (auto)──> needs_info
confirmed ──claim (auto)──> in_progress ──resolve (auto)──> resolved
confirmed ──wont_fix (U)──> wont_fix
open ──wont_fix (U)──> wont_fix
```

Legend: `(U)` = user-authority moment (`--invoker ai_with_human` only when human just assented to this exact transition this turn). `(auto)` = `ai_autonomous`. `(human)` = pure `actor: human`, the AI cannot do these.

The first U-moment is `investigate`. An open observation sits in the queue until the user (via `/pickup`) decides to triage it. Investigation produces a draft `intent_contract` on the observation; transitioning to `confirmed` requires the contract to be `ready` AND human-approved (`approved_by` / `approved_at` are `actor: human` on the contract).

### Triage tiers (encoded in the observation's intent_contract)

The observation's `intent_contract.tier_hint` records the triage decision:

- **T1 — Direct fix.** Single-file, ≤50 LOC, no migrations, no new deps. Handled inside the observation lifecycle: investigate → confirm → claim → resolve. No separate task.
- **T2 — Scoped.** Up to 5 files, ≤200 LOC, single subsystem. Same observation lifecycle; resolution may include a brief subagent loop, but the work is bounded.
- **T3 — Full task.** Anything bigger, or anything touching schema, runner contract, or substrate API. Promoted to a substrate task: `stores tasks add --invoker ai_with_human --linked-observations L0XX ...`. The observation gets `resolved` with `resolution: "shipped via s/T0XX"` once the task accepts.

Promotion is U2 (a user-authority moment): the user must have just seen the proposed task contract and assented in this turn. Don't promote autonomously — the substrate would reject the `tasks add` write because `title`, `slug`, and the contract approval all require `ai_with_human` or `human`.

### Finding friction (read-only)

```bash
stores observations list --invoker ai_autonomous           # all rows
stores observations list --invoker ai_autonomous --json    # machine-readable
stores observations show L001 --invoker ai_autonomous      # full row
```

(Filtering by state — `--state open`, `--priority high` — is what `/pickup` uses to surface the next thing. If those filters don't exist, that's an observation to file: meta-recursion in action.)

### Substrate-down escape

If `stores observations add` itself fails (substrate uninitialized, schema incompatibility, runner crash):

1. Write a worklog note: `docs/worklog/<date>/NN-substrate-down-<short-slug>.md` with the same content you would have put in the observation (summary, body, what broke).
2. When the substrate recovers, file the observation properly and reference the worklog note in the body.

The worklog is git-tracked and timestamped — it's an acceptable fallback record. The discipline is: **never let friction die silently**, even when the substrate is the thing that's broken.

### Anti-patterns

- Filing observations as `--invoker ai_with_human` for filing-only. Filing is autonomous; only investigation/confirmation/promotion are U-moments.
- Filtering at filing time ("is this worth filing?"). File first, triage second.
- Treating `task_id` as required. It's a soft-FK; not all observations have a surfacing task (e.g., observations from `/pickup` itself).
- Hand-editing observation rows via `stores observations update` to change state. Use the lifecycle verbs (`investigate`, `confirm`, etc.); they enforce the state machine and the actor gates.
- Promoting an observation to a task without the user seeing the proposed contract. The schema will reject `tasks add` without `ai_with_human` invoker, but the bigger problem is you'd be inventing scope on the user's behalf.

### What this file does NOT contain

- The CLI flag reference (read `stores observations <verb> --help`).
- The schema field listings (read `schema.yaml`).
- The 10.06 source-system semantics (read `README.md`).
- Generic Claude orientation (read `/CLAUDE.md`).
