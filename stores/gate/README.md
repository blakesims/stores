# gate store

Decision gates — points where human judgment is required before work continues.

## Fields

- `type` (enum: decision|script, required) — gate kind
- `question` (text, required) — the question posed at this gate
- `options` (list<text>, optional) — possible answers; pipe-separated on CLI: `"soft|hard"`
- `answer` (text, optional, **actor: human**) — the chosen answer; only a human invoker may write this field
- `task_ref` (text, optional) — cross-store reference to a task display_id (e.g. `L001`); no FK enforced at SQL level

## Lifecycle

`pending` → `answered` (verb: `answer`, actor: `human`)
`pending` → `cancelled` (verb: `cancel`, actor: `ai_autonomous`)

## Quick start

```bash
stores install ./stores/gate
stores gate add --type decision --question "Soft or hard delete?" --options "soft|hard" --task-ref L001
# returns G001

stores gate answer G001 --answer hard --invoker human   # SUCCEEDS
CLAUDECODE=1 stores gate answer G001 --answer hard      # FAILS: actor mismatch on field 'answer'

stores gate add --type decision --question "Q2?" --options "a|b"   # G002
CLAUDECODE=1 stores gate cancel G002                               # SUCCEEDS (autonomous actor)
```
