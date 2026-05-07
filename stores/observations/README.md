# observations store

Captures and triages observations during AI-assisted work sessions. Production shape mirrors 10.06 ledger rows.

## Fields (selected)

**Required at add time:**
- `summary` (text, required) — one-line description
- `source` (enum: dashboard|qa|dev|sentry|intake|converge|wrap, required)
- `priority` (enum: high|normal|low, required)
- `captured_at` (timestamp, required) — ISO date/time the observation was captured
- `captured_week` (text, optional) — e.g. `w11-d4`; derived on show/list from `captured_at` when omitted, stored non-empty values are preserved

**Optional top-level fields (selection):**
- `body` (text) — extended notes; use `--body-from-file` for multi-line
- `tags` (list<text>) — pipe-separated: `"frontend|backend"`
- `task_id` (text) — soft-FK to a `tasks` display_id (e.g. `T170`)
- `pending_architecture_review` (bool) — set by gatekeeper `arch_review_candidate` routing; blocks U1 ratification until a clearing A### architecture-review verdict and any required reconciliation are present
- `resolved_by`, `merge_target_id`, `resolution_kind` — architecture-review merge/redirect resolution tracking
- `investigation_note`, `resolved_at`, `wont_fix_at`, `resolution` — terminal-state tracking
- `contact_id`, `field_name` — dashboard-sync dedup keys

**`intent_contract` record (15 sub-fields, D9 production names):**

Sub-fields required when `contract_state == 'ready'`:
- `objective` (text) — one-line goal statement
- `type` (enum: work|investigation)
- `in_scope` (list<text>) — what changes
- `out_of_scope` (list<text>) — what stays unchanged
- `acceptance` (list<text>) — completion criteria
- `tier_hint` (enum: T1|T2|T3)
- `approved_by` (text, actor: human) — human who ratified the contract
- `approved_at` (timestamp, actor: human)

Always-optional sub-fields: `inputs`, `touches`, `affects_capability`, `known_solution`, `drafted_by`, `drafted_at`, `contract_state` (enum: draft|ready).

**`evidence` record:** `external_refs` (list_record with `system`, `kind`, `id` sub-fields).

**`notes` (json):** structured JSON blob for operator notes.

## Lifecycle

```
open → investigating  (verb: investigate, actor: ai_with_human)
open → wont_fix       (verb: wont_fix,    actor: ai_with_human)

investigating → confirmed   (verb: confirm,       actor: ai_with_human,
                             guard: intent_contract.contract_state == 'ready')
investigating → needs_info  (verb: request_info,  actor: ai_autonomous)

confirmed → needs_info   (verb: park,         actor: ai_autonomous)
confirmed → in_progress  (verb: claim,        actor: ai_autonomous)
confirmed → wont_fix     (verb: wont_fix,     actor: ai_with_human)

needs_info → confirmed   (verb: provide_info, actor: human)

in_progress → resolved   (verb: resolve,      actor: ai_autonomous)
```

## Quick start

```bash
stores install ./stores/observations

# Add a fully-shaped observation
stores observations add \
    --summary "Dashboard: 500 on checkout" \
    --source dashboard --priority high \
    --captured-at 2026-04-30 --captured-week w11-d4

# Ratify flow: investigate → fill intent contract → confirm
stores observations investigate L001 --invoker human
stores observations update L001 \
    --contract-state ready \
    --objective "Fix the 500 handler" \
    --type work \
    --in-scope "backend handler" \
    --out-of-scope "frontend" \
    --acceptance "checkout succeeds" \
    --tier-hint T3 \
    --approved-by blake --approved-at 2026-04-30 \
    --invoker human
# If pending_architecture_review=true, confirm fails until the linked A### ruling clears it.
stores observations confirm L001 --invoker human

stores observations show L001
stores observations list
```

## Migrating from `./dev observation` (10.06)

If you're porting muscle memory from 10.06's bash CLI, the verb shape is the
same but most flag names align with the production schema rather than the
v0.1 short-form. Common mappings:

| `./dev observation …` | `stores observations …` | Notes |
|---|---|---|
| `log --foo bar` | `add --foo bar` | Verb rename: `log` → `add`. |
| `log --done-when "X"` | `update <id> --acceptance "X"` | Renamed to D9 production name. Belongs on the contract, not the initial add. |
| `log --scope-in "X"` | `update <id> --in-scope "X"` | D9 rename. |
| `log --scope-out "X"` | `update <id> --out-of-scope "X"` | D9 rename. |
| `log --verdict T3` | `update <id> --tier-hint T3` | D9 rename. |
| `update --evidence '{...}'` | `update <id> --observed-at … --env … --external-refs '{system,kind,id}'` | `evidence` was flattened: each sub-field is its own flag. `external_refs` is a `list_record` and accepts repeated `--external-refs '{...}'` flags or a single `'[{...},{...}]'` JSON array. |
| `update --notes '{...}'` | `update <id> --notes '{...}'` | Same — opaque JSON blob via `FieldType::Json`. |
| `update --investigation-note "..."` | `update <id> --investigation-note "..."` | Same. |
| `update --schedule today` | `update <id> --scheduled-for "$(date -I)"` | No `today/tomorrow/clear` shortcut yet — pass an ISO date. |
| `list --closed-today` | `list --status resolved --json \| jq '...'` | No shortcut flag yet; filter via JSON output. |
| `add --linked-observations '["L001"]'` | `add --linked-observations L001 --linked-observations L002` | Repeatable flags now work; bare single value auto-promotes to a 1-element array. (Both forms accepted.) |

Actor flags are uniformly `--invoker human|ai_autonomous|ai_with_human`. In a
`CLAUDECODE=1` shell the auto-detect yields `ai_autonomous`, which already
satisfies any `actor: ai_autonomous` verb (`claim`, `resolve`, `park`,
`request_info`) — so for those, **omit `--invoker`**. Pass it explicitly only
when you need to override the auto-detect (e.g. `--invoker human` for
`provide_info` or for fields like `approved_by`/`approved_at`).

Historical POC: `stores/observations_1006/` — frozen fixture, not maintained.
