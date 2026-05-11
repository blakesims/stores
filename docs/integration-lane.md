# Integration lane

**Status:** Current architecture contract (T146 / ADR 0001 complete).
**Owner:** substrate (engine-controller + Pi).
**Companion:** `docs/agents-yaml-example.yaml`, `docs/architecture-coherence.md` § *Client adapter boundary*.

The integration lane is the substrate-owned, repo-agnostic primitive that
serializes how accepted task candidates land on `main`. It replaces the
prior direct `accepted → cargo_installed` edge with a queued, capacity-1
mutation of `main` that records typed provenance for every attempt.

This doc is the operator-facing contract: the lifecycle, the configuration
shape, the freshness rules, the typed failure outcomes, the provenance
schema, and the substrate-vs-adapter boundary that keeps client repos out
of the queueing business.

## Lifecycle

A row that was just accepted enters `lifecycle='integration'` and advances by `integration_step`:

```
integration/queued
   │ refresh without main_branch lock
   ▼
integration/refreshing
   │ task review / tests without main_branch lock
   ▼
integration/task_review → integration/testing
   │ acquire main_branch ResourceLock only for truth mutation
   ▼
integration/merging
   │
   ├──► done/none with post_integration_step for repo-specific subscribers
   └──► integration/none blocked=true blocker_kind=<typed outcome>
```

- `integration_step='queued'` — enqueued, awaiting capacity.
- `refreshing`, `task_review`, and `testing` — slow freshness/gate work; these steps do not hold `main_branch`.
- `merging` — the only step that holds the `main_branch` ResourceLock.
- `deploying` / `verifying` — optional post-merge integration substeps before generic `done`.
- `blocked=true` with `blocker_kind` records recoverable failures without inventing blocked lifecycle states.

The retry edge re-traverses `integration_queued → integrating → integrated`
so post-integrated subscribers fire on the recovery path. There is no
`integration_blocked → integrated` direct edge by design.

## Capacity-1

At most one row may perform `integration_step='merging'` substrate-wide. Legacy compatibility may still project this as `status='integrating'`; the primary capacity semantics are lifecycle/step based. This is schema-enforced, not advisory:

```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_integration_singleton
  ON tasks((1)) WHERE status='integrating';
```

The partial UNIQUE index reduces to "at most one row may match the
predicate" — atomic at the SQLite level. Concurrent
`start-integration` attempts surface as a `ConstraintViolation`, which
the integrate builtin treats as capacity-busy and returns `Ok(0)`. The
queued row stays in `integration_queued` and is retried on the next
dispatch tick. No application-level lock or scheduler is involved.

This is why client repos must not implement competing queues: the
substrate already serializes integration at the schema layer, and any
client-side queue would either duplicate or contradict it.

## Refresh strategy

Configured per-agent on the `integrate` agent's `command_args.refresh_strategy`:

- `rebase` — `git rebase <main_branch>` against the candidate worktree
  (default).
- `merge_main` — `git merge <main_branch>` into the candidate.

Either strategy runs from the candidate's `workspace_path` after the lane
verifies the worktree is checked out to the candidate branch. Neither
strategy advances `main`; both produce a refreshed candidate head that
the lane then validates and fast-merges into `main`.

Failure modes:

- Conflict during refresh → `rebase_conflict` outcome; rebase/merge is
  aborted; lane routes to `integration_blocked`.
- Reviewed base no longer reachable from main during refresh →
  `stale_base` outcome (see *Stale-base detection* below).

## Pre-land check

Configured per-agent on `command_args.pre_land_check` (the command
string) and `command_args.pre_land_check_timeout_secs` (default 600s).

The lane runs the configured command from the candidate worktree
**after** refresh and ER head re-check, **before** the merge into main.
The command is the gate for "is this candidate landable?" — typically a
fast `cargo check`, `cargo test --no-run`, or repo-equivalent. Exit
status 0 means proceed; non-zero or timeout means
`pre_land_check_failed` and routes to `integration_blocked`.

The lane is repo-agnostic: it does not assume cargo, npm, make, or any
specific tool. Each repo wires its own pre-land check via its
`agents.yaml`. **Missing `pre_land_check` is treated as a
configuration error**, recorded as `pre_land_check_failed` provenance,
so the gap surfaces fail-loud rather than silently landing.

## External-review freshness

When a passed external_review row exists for the candidate, the lane
re-checks freshness **after refresh** (the candidate head may have
moved during rebase / merge_main):

- If the latest non-superseded ER `head_sha` matches the post-refresh
  `candidate_head_after`, freshness holds and the lane proceeds.
- If the heads differ, the ER is invalidated and the lane routes to
  `integration_blocked` with outcome `stale_external_review`. The
  superseded ER row is flipped to `superseded` so the next acceptance
  cycle requires a fresh review.

This is the substrate's guarantee that external review PASS still
applies to whatever is about to land on main, not to whatever HEAD was
when the review fired.

## Stale-base detection

Distinct from `stale_external_review`. `stale_base` fires
**pre-rebase**, before any refresh runs:

```text
let er_base = latest_passed_external_review.base_sha;
if !git merge-base --is-ancestor er_base main:
    supersede ER, route to integration_blocked, outcome='stale_base'
```

The check uses `git merge-base --is-ancestor <er_base> <main_branch>`
to ask "is the reviewed base still reachable from current main?" If
no — typical of a force-push or history rewrite on main — the lane
**does not run rebase and does not advance main**. The ER is
superseded and the row blocks for fresh review.

### Stale-base vs stale-external-review

| outcome | when it fires | what changed | recovery |
|---|---|---|---|
| `stale_base` | pre-rebase | reviewed `base_sha` no longer reachable from current `main` (force-push / history rewrite) | fresh ER required against new base; `retry-integration` after re-review |
| `stale_external_review` | post-rebase | reviewed `head_sha` ≠ post-refresh `candidate_head_after` (rebase moved the head) | fresh ER required against new candidate head; `retry-integration` after re-review |

Both supersede the ER row and route to `integration_blocked`. They are
kept distinct in provenance and in operator-facing surfaces because the
recovery shape differs (history-rewrite suspicion vs. routine
post-rebase head divergence).

## Provenance

Every entry into `integrating` produces exactly one record in
`tasks.integration_attempts`. The invariant is **one single record per attempt**:
the in-progress entry is appended on `start-integration`,
then updated in place as the lane progresses (no per-step append). The
final update writes `completed_at` and the typed `outcome`.

### Storage model

`integration_attempts` is a `list_record` stored as a **JSON array
column** on the `tasks` row — not a separate SQL table. Query it via
SQLite JSON functions:

```sql
-- attempt count for a row
SELECT json_array_length(integration_attempts) FROM tasks WHERE display_id='T###';

-- last attempt's outcome
SELECT json_extract(integration_attempts, '$[#-1].outcome') FROM tasks WHERE display_id='T###';

-- specific attempt field
SELECT json_extract(integration_attempts, '$[0].pre_land_check_summary') FROM tasks WHERE display_id='T###';
```

Do **not** write `SELECT FROM integration_attempts` — there is no such
table. Consumers (status renderers, watch, TUI, reviewer tools) must
use `json_array_length` and `json_extract` against the `tasks.integration_attempts`
column.

### Per-attempt fields

| field | meaning |
|---|---|
| `attempt_no` | 1-indexed sequence within this row's history |
| `started_at` / `completed_at` | ISO-8601 timestamps |
| `base_main_sha` | main HEAD at lane entry |
| `candidate_head_before` / `candidate_head_after` | candidate HEAD before / after refresh |
| `landed_main_sha` | main HEAD after fast-merge (set only on `outcome='integrated'`) |
| `refresh_strategy` | `rebase` \| `merge_main` |
| `pre_land_check_summary` | last meaningful diagnostic (refresh msg, pre-land output, merge stderr — depends on outcome) |
| `reviewed_base_sha` | the ER `base_sha` checked for stale_base; null when no ER or when the divergence was head-side |
| `outcome` | typed enum (see below) |

## Outcome enum

`integration_attempts[].outcome` is a typed enum with seven values:

- `integrated` — candidate refreshed, ER re-check passed, pre-land
  passed, merge into main succeeded (and push, if `allow_push: true`).
- `rebase_conflict` — refresh hit a merge conflict; refresh was
  aborted; main was not touched.
- `stale_base` — pre-rebase: reviewed base no longer reachable from
  current main. ER superseded; main was not touched.
- `stale_external_review` — post-rebase: reviewed head ≠ refreshed
  candidate head. ER superseded; main was not touched.
- `pre_land_check_failed` — configured `pre_land_check` exited
  non-zero (or was missing from `command_args`). Main was not touched.
- `merge_failure` — post-check fast-merge into main failed (rare;
  typically transient or pre-existing main mutation). Lane attempts a
  best-effort `git merge --abort` to leave main on its prior tip.
- `push_failure` — `allow_push: true` was configured and the post-merge
  `git push` failed. The lane attempts a best-effort
  `git reset --hard <base_main_sha>` rollback, but **only** when the
  current local HEAD's parent is exactly `base_main_sha` (i.e. the new
  merge commit hasn't been built upon).

`integration_blocked_reason` on the row carries the same outcome plus a
short summary, e.g. `stale_base: reviewed base 9def951 no longer
reachable from current main 8bd21b6; fresh external review required`.

## main_branch ResourceLock

`builtin:integrate` acquires the DB-backed `main_branch` ResourceLock immediately before checking out and mutating `main`, using `handlers::resource_locks` with `Actor::Framework`; see `docs/primitives.md` for the primitive contract and `src/handlers/resource_locks.rs` for the helper surface. Refresh, task review, and testing run before that lock window. The lock window sits wholly inside `lifecycle='integration'` and `integration_step='merging'`; probes observe the lock absent during queued/refresh/review/test work and present only during merge mutation. The lock is released after merge/push and `mark_integrated` complete; every error or early-return path in the merge/push window is covered by a guard `Drop`, so failed merge/push attempts release the lock too.

If acquisition finds an unexpired owner, the attempt records `outcome='merge_failure'` with `pre_land_check_summary='merge_failure: main_branch lock held by <owner>; will retry'`, then routes to `integration_blocked`. Phase 3's lifecycle-overlay table maps that `merge_failure:` prefix to `blocker_kind='main_red'`, leaving the row visible as `lifecycle='integration'`, `integration_step='none'`, and `blocked=true`.

Immediately before `git merge --no-ff`, the lane re-checks ownership of `main_branch` for the task display id. If the lock was fenced/stolen/rotated during merge prep, the lane blocks with `merge_failure: main_branch lock no longer owned (token rotated)` and the guard releases the stale token if still valid.

Stale ResourceLock rows are recovered by `resource_locks::acquire`: expired rows are deleted, a `transition_history` audit row is written with `verb='recover_stale'` and `invoker='framework'`, and the current integration attempt acquires a fresh fencing token before mutating main.

## Post-integrated subscribers

The integration lane fires `mark_integrated` and stops. It does **not**
invoke `cargo install`, schema migrations, deploy scripts, or any
repo-specific verb after `integrated`.

Repo-specific post-land work hangs off the `integrating → integrated`
transition as ordinary subscribers in that repo's `agents.yaml`. For
the stores repo:

- `builtin:cargo-install` subscribes to `integrating → integrated`,
  fires `mark_cargo_installed`.
- `builtin:schema-migrate` subscribes to `integrated → cargo_installed`,
  fires `mark_schema_migrated`.

A different repo wiring this surface would not include `cargo-install`
or `schema-migrate`; it would hang its own post-integrated chain off
`integrating → integrated`.

This boundary is what makes the integration lane portable: the substrate
owns the queue and the merge ceremony; the client owns whatever
post-land steps its release shape requires.

## Retry-integration

When a row lands in `integration_blocked`, recovery is a U4 verb:

```bash
stores tasks retry-integration <id> \
  --invoker ai_with_human \
  --approve-token <T>
```

This re-fires `integration_blocked → integration_queued`. The lane
re-traverses `integration_queued → integrating → integrated`, so
post-integrated subscribers fire on the recovery path. The retry verb
does NOT short-circuit to `integrated`; the lane re-runs refresh, ER
re-check, and pre-land check on the next attempt, producing a fresh
`integration_attempts` entry.

The verb is `actor: ai_with_human` (tier-B): the human must have seen
the typed blocked reason and authorized the retry, but no token is
schema-enforced for this transition. The retry assumes the underlying
issue (rebase conflict resolved, stale ER refreshed, pre-land flake
re-run, etc.) is genuinely fixed — the lane will re-route to
`integration_blocked` if it is not.

## Pointers

- `docs/agents-yaml-example.yaml` — reference `.stores/agents.yaml` showing the
  `integrate` agent + the stores-specific post-`integrated` chain.
- `docs/architecture-coherence.md` § *Client adapter boundary* — the
  doctrine that grounds the substrate-vs-adapter split.
- `stores/tasks/schema.yaml` — lifecycle states, transitions, and
  `integration_attempts` field definition.
- `src/flow/builtins/integrate.rs` — the implementation.
- `src/handlers/framework_migrate.rs` — `idx_tasks_integration_singleton`
  partial UNIQUE index DDL.
- `tests/integration_lane_e2e.rs` — two-candidate regression suite
  proving candidate two is validated against the main head produced by
  candidate one before it can land.
