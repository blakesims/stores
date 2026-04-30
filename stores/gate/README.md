# gate store

Decision gates — points where human judgment is required before work continues.

## Fields

- `type` (enum: decision|script, required) — gate kind
- `one_liner` (text, required) — the question or action posed at this gate
- `options` (list<text>, optional) — possible answers; pipe-separated on CLI: `"soft|hard"`
- `answer` (text, optional, **actor: human**) — the chosen answer; only a human invoker may write this field
- `task_ref` (text, optional) — cross-store reference to a task display_id (e.g. `L001`); no FK enforced at SQL level
- `priority` (enum: high|normal|low, optional) — filing priority
- `priority_rank` (integer, optional) — numeric rank 1-5 set by /focus; lower is more urgent
- `priority_rank_at` (timestamp, optional) — when priority_rank was last written
- `defer_until` (text, optional) — ISO date string (YYYY-MM-DD); gate re-surfaces on or after this date
- `filed_by` (text, required) — skill slug or agent name that filed this gate (e.g. `morning-check`, `task:wrap`). Named `filed_by` to avoid collision with the framework-reserved `created_by` audit column.
- `source` (enum: dashboard|qa|dev|converge|wrap|intake, required) — origin context where this gate was raised
- `business_reason` (text, optional) — proto-contract: why this gate matters from a business perspective
- `technical_detail` (text, optional) — proto-contract: technical context for the gate
- `command` (text, optional) — for type=script: the shell command to run
- `implications` (text, optional) — proto-contract: downstream consequences of the gate decision

## Lifecycle

States: `pending` → `answered` | `deferred` | `cancelled`

| From | To | Verb | Actor |
|------|----|------|-------|
| `pending` | `answered` | `answer` | `human` |
| `pending` | `cancelled` | `cancel` | `ai_autonomous` |
| `pending` | `deferred` | `defer` | `ai_with_human` |
| `deferred` | `pending` | `resume` | `ai_with_human` |
| `pending` | `pending` | `resume` | `ai_with_human` (idempotent self-loop) |
| `deferred` | `cancelled` | `cancel` | `ai_autonomous` |

**Defer transition:** `stores gate defer <ID> --defer-until <YYYY-MM-DD>` — transitions `pending → deferred` and records the date. `defer_until` is operator-hygiene: not schema-enforced (validate runs pre-merge; see T007 R1).

**Resume transition:** `stores gate resume <ID>` — transitions `deferred → pending`. Idempotent: running on an already-`pending` gate is a no-op self-loop (`pending → pending`).

## Quick start

```bash
stores install ./stores/gate

stores gate add --type decision \
    --one-liner "Soft or hard delete?" \
    --options "soft|hard" \
    --task-ref L001 \
    --filed-by quickstart \
    --source dev
# returns G001

stores gate answer G001 --answer hard --invoker human   # SUCCEEDS
CLAUDECODE=1 stores gate answer G001 --answer hard      # FAILS: actor mismatch on field 'answer'

stores gate add --type decision \
    --one-liner "Q2?" \
    --options "a|b" \
    --filed-by quickstart \
    --source dev
# returns G002
CLAUDECODE=1 stores gate cancel G002                               # SUCCEEDS (autonomous actor)
```
