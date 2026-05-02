---
name: promote
description: Promote an observation to a substrate task. User-authority moment (U2) — the human must be present and assenting. Use when an observation is T3 (anything bigger than ≤5 files / ≤200 LOC, or anything touching schema, runner contract, or substrate API).
user_invocable: true
---

You are executing a **U2 (promotion)** moment: turning an observation into a substrate task. This requires `--invoker ai_with_human` and the user's just-in-this-turn assent to the proposed task contract.

For dogfood context, see `/CLAUDE.md`, `tasks/CLAUDE.md`, and `stores/observations/CLAUDE.md`.

## When to invoke

Invoke `/promote` when an observation's tier_hint is **T3** — meaning the work to address it is too big to handle inside the observation lifecycle:

- More than 5 files touched, OR
- More than ~200 LOC, OR
- Any change to schema, runner contract, or substrate API surface, OR
- Spans multiple subsystems, OR
- Adds or modifies user-facing capability.

T1/T2 observations are handled inside the observation lifecycle (`investigate → confirm → claim → resolve`). They do NOT need a separate task. Do not promote them — that just creates noise.

## The promotion flow

### Step 1 — read the observation

```bash
stores observations show L0XX --invoker ai_autonomous
```

Read the body, the `intent_contract` (which should already exist in `draft` state from `/pickup`'s investigation step), the `task_id` (the task that surfaced the original friction, if any), and any linked observations.

If the observation is not in `investigating` state with a draft contract, the prerequisite work hasn't been done. Halt and surface to the user: "L0XX is in state <X>; promotion requires prior investigation. Want to investigate first?"

### Step 2 — draft the substrate task contract

Translate the observation's `intent_contract` into a tasks `contract` (the schemas are similar but not identical; map field-by-field). The substrate task's contract has:

- **title** — a one-line task title (will be the row's `title`, `actor: ai_with_human`)
- **slug** — kebab-case identifier (will be the row's `slug`, `actor: ai_with_human`, pattern `^[a-z0-9-]+$`)
- **executive_intent** — why this task matters
- **done_when** — testable, observable completion criteria (1-2 lines, written like a test assertion)
- **scope_in** — what's in
- **scope_out** — what's out (name the creep vectors)
- **assumptions** — anything that could change the answer

Carry over from the observation:
- `linked_observations: [L0XX]` — the soft-FK link
- The triage tier (already T3 by definition of being promoted)

### Step 3 — show the user the proposed contract

Present the proposal compactly:

```
Promoting L0XX → new substrate task

Title:       <...>
Slug:        <...>
done_when:   <...>
scope_in:    <...>
scope_out:   <...>
assumptions: <...>
linked:      L0XX

Verb that will run:
  stores tasks add --invoker ai_with_human \
    --title "..." --slug "..." \
    --done-when "..." --scope-in "..." --scope-out "..." \
    --assumptions "..." \
    --linked-observations L0XX

Reply: go | revise <field> <new value> | cancel
```

### Step 4 — act on the user's reply

- **go** — execute the `stores tasks add ... --invoker ai_with_human` command. Capture the new T-id.
- **revise <field> <new value>** — update the field, re-show the proposal, loop step 3.
- **cancel** — exit. Print "Promotion cancelled. L0XX remains in state <X>." Do not modify the observation.

### Step 5 — link back from the observation (audit trail)

After successful `tasks add`, update the observation to record the link:

```bash
# Record the promotion in the observation's notes or a dedicated link field.
# The observation stays in 'investigating' until the task ships and the user
# manually resolves the observation (via /pickup → wont_fix or via observation
# resolve verb after the task is accepted).
stores observations update L0XX --invoker ai_autonomous \
  --notes-from-file <(cat <<EOF
{"promoted_to_task": "T0YY", "promoted_at": "$(date -Iseconds)"}
EOF
)
```

(If the substrate doesn't accept this update shape, file an observation about it. Do not invent a fallback that obscures the link.)

### Step 6 — render and report

```bash
stores tasks render T0YY
```

Print:

```
Promoted L0XX → s/T0YY.
  Task contract written; row in state 'planning'.
  Markdown projection at: tasks/planning/T0YY-<slug>/main.md
  Next: invoke /pickup to drive the task through its workflow.
```

Exit cleanly.

## Discipline

- **`--invoker ai_with_human` is required** for `tasks add` (`title` and `slug` reject autonomous writes; the contract approval is `actor: human` `required_when contract_state == 'ready'`). The substrate will reject the write if you skip this.
- **The user must have just assented in this turn.** Not "the user is presumably available." The user just typed `go` to the proposal in step 3.
- **One observation per promotion.** If multiple observations are related, the substrate task should reference all of them in `linked_observations` (`--linked-observations L001,L002,...`), but the conversation about the contract still goes through the user once.
- **Do NOT autopilot.** If the user revises the contract, re-show. If they cancel, stop. The grounding is the point.

## Anti-patterns

- **Promoting T1/T2 observations.** They don't need a task; the observation lifecycle handles them. Promoting just creates a row with no work proportional to its existence.
- **Inventing scope.** `scope_in` / `scope_out` come from the observation's `intent_contract` plus the user's revisions. Don't add scope items the user hasn't agreed to.
- **Skipping the user assent.** Even if you "know" the contract is right, the substrate's actor enforcement is the wall. Lean into it.
- **Forgetting the back-link.** Without the `notes` update (or whatever link the substrate supports), the observation is orphaned after promotion — the recursion loses traceability.

## Substrate-down escape

If `stores tasks add` fails for substrate reasons (not contract validation): write a worklog note describing what broke, leave the observation in `investigating` state, surface the error to the user, do not retry blindly.

If the contract validation fails (the schema rejects a field): show the validation errors to the user, take their fixes, retry. This is the schema doing its job, not friction.

## Output

After successful promotion, print:

```
Promoted L0XX → s/T0YY.
  Title: "<...>"
  State: planning
  Markdown: tasks/planning/T0YY-<slug>/main.md
  Linked observations: L0XX
```

Then exit. Do not auto-invoke `/pickup` or `tasks drive` — that's a separate user decision.
