# Autonomous Flow Engine — agents & policies

The autonomous-flow engine watches `transition_history`, gates each
candidate dispatch through the policy layer, and runs registered
subscribers. Two YAML files in `.stores/` configure it:

- `.stores/agents.yaml` — the agent registry (what subscribes to which
  transition; what command runs on dispatch).
- `.stores/policies.yaml` — the policy layer (allow/halt/never gates
  evaluated against each candidate transition before dispatch).

Substrate guards (`required_when`, `actor`, lifecycle) remain the floor.
Policies cannot relax them. The default action is **allow** — match the
"everything flows between the gates" doctrine. `never` policies are
sacrosanct (cannot be overridden).

---

## Example `.stores/agents.yaml`

```yaml
agents:
  # Builtin: fast-merges a task's branch into main when it transitions
  # in_review → accepted. On conflict the row is flipped to deploy_blocked
  # and dispatched to `deployment_specialist`.
  - name: accept-merge
    subscribes_to:
      - store: tasks
        transition: { from: in_review, to: accepted }
    command: "builtin:accept-merge"
    claim_window_secs: 300
    retry_policy: { max_attempts: 3, backoff: linear }

  # Sample shell subscriber: notify a Slack hook whenever a task is
  # accepted. Receives row context as STORES_* env vars.
  - name: slack-on-accept
    subscribes_to:
      - store: tasks
        transition: { from: in_review, to: accepted }
    command: 'curl -fsS -X POST -H content-type:application/json --data "{\"text\":\"task ${STORES_DISPLAY_ID} accepted\"}" "$SLACK_WEBHOOK_URL"'
    claim_window_secs: 60
    retry_policy: { max_attempts: 2, backoff: exponential }

# Default specialist agent dispatched when accept-merge hits a conflict.
# Either a name from the `agents:` list above, or a "builtin:<name>"
# sentinel. Defaults to "builtin:user-escalation" when omitted.
deployment_specialist: builtin:user-escalation
```

### Subscriber env vars

Every shell `command` is invoked with these env vars set so the script
can locate the row and record correct audit metadata on any follow-on
substrate write:

| Var                     | Meaning                                            |
| ----------------------- | -------------------------------------------------- |
| `STORES_STORE`          | store name (e.g. `tasks`)                          |
| `STORES_DISPLAY_ID`     | display id of the row (e.g. `T042`)                |
| `STORES_ROW_ID`         | substrate row id (integer)                         |
| `STORES_TRANSITION_FROM`| from-state of the transition that fired           |
| `STORES_TRANSITION_TO`  | to-state of the transition that fired             |
| `STORES_POLICY_REF`     | matched policy id (or `default-allow`)             |
| `STORES_POLICIES_HASH`  | sha256 of the policies.yaml that gated the dispatch|

---

## Example `.stores/policies.yaml`

```yaml
policies:
  # Allow: a T1 fast path that lets the daemon accept-merge T1 rows.
  - id: allow-T1-fast-path
    transition: { store: tasks, from: in_review, to: accepted }
    predicate: { op: "==", left: "$tier_hint", right: "T1" }
    action: allow

  # NEVER: empty branch must never auto-merge — the row would silently
  # go nowhere. NEVER halts even when an Allow rule also matches.
  - id: never-empty-branch
    transition: { store: tasks, from: in_review, to: accepted }
    predicate: { op: "==", left: "$branch", right: "" }
    action: never
```

### Predicate language

| Op       | Operands                          | Notes                                  |
| -------- | --------------------------------- | -------------------------------------- |
| `==`     | scalar, scalar                    | string equality                        |
| `!=`     | scalar, scalar                    |                                        |
| `in`     | scalar, list                      | membership                             |
| `not in` | scalar, list                      |                                        |
| `matches`| scalar, regex                     | regex must match the whole string     |

Operands that start with `$` reference row fields (e.g. `$tier_hint`,
`$branch`, `$linked_observation_count`). Literals are written inline.

### Decision precedence

1. Any matching `never` → **halt** (records `policy_ref` on
   `transition_history`; fires ntfy).
2. Any matching `halt` → **halt** (same as above).
3. Any matching `allow` → **allow**, records `policy_ref`.
4. No match → **default-allow**, records `policy_ref: "default-allow"`.

---

## External review lane

T2/T3 tasks create a typed `external_reviews` row after wrap. The
`external-review` subscriber consumes contract, plan, wrap log, diff
`base_sha`/`head_sha`, and prior external-review findings. `PASS` leaves the
task in `in_review` and satisfies the human-accept precheck; `REVISE` routes
the task back to `executing` for the executor; `TOOLING_FAILURE` marks the
review `tooling_held` with `held_reason`, `next_retry_at`, log/transcript refs,
and no acceptance bypass.

`.stores/config.yaml` examples:

```yaml
review:
  runner: codex
  max_parallel: 1
  timeout_secs: 1800
  model: gpt-5-codex
codex:
  command: codex
  args: ["review", "--stdin"]
```

```yaml
review:
  runner: pi
  max_parallel: 1
  timeout_secs: 1800
  model: pi-review
```

```yaml
review:
  runner: claude-code
  max_parallel: 1
  timeout_secs: 1800
  model: sonnet
```

`review.max_parallel` caps concurrent review rows. Cap-held rows stay pending
with `held_reason=cap-held`; tooling failures stay `tooling_held` until retried
(`tooling_held → pending`). Both states are printed by `stores agents run` and
surfaced in `stores watch` review rows.

---

## Runbook

### Start the daemon

```bash
# Foreground (logs to stdout). SIGINT / SIGTERM finishes the in-flight
# dispatch and exits cleanly.
stores agents run

# Custom poll interval (default: 5s)
stores agents run --poll-interval 2

# Detach: forks, sets up a new session, redirects fd1/fd2 to --log-file.
# Parent prints the child PID and exits 0.
stores agents run --detach --log-file /tmp/stores-agents.log
```

### Stop the daemon

```bash
# Foreground: Ctrl-C. Detached:
kill $(pgrep -f 'stores agents run')
```

### One-off backfill (catch up accepted-but-unmerged rows)

```bash
# Scans every accepted row whose branch is set but not yet merged into
# main, then runs accept-merge against each. Conflicts surface via
# ntfy + deploy_blocked, same as the live daemon.
stores agents backfill
```

This is **not** auto-fired on daemon startup — run it manually after
provisioning the daemon for the first time, or after a long outage.

### Inspect the audit trail

```bash
# Every transition records the gating policy id (or 'default-allow')
# and the policies.yaml hash so historical decisions can be re-verified.
sqlite3 .stores/db.sqlite "
  SELECT display_id, from_status, to_status, verb, invoker, policy_ref, policies_hash
  FROM transition_history
  WHERE store = 'tasks'
  ORDER BY id DESC
  LIMIT 20;
"
```

### Add a new subscriber

1. Append an entry under `agents:` in `.stores/agents.yaml`.
2. Optionally, add a matching policy to `.stores/policies.yaml`.
3. Restart the daemon (`agents.yaml` is read once at startup).

### Notification config

`ntfy` URL is read in this order:

1. `.stores/config.yaml` → `ntfy.url`
2. `STORES_NTFY_URL` env var
3. neither set → log to stderr, continue

`.stores/config.yaml` example:

```yaml
ntfy:
  url: "https://ntfy.sh/your-private-topic"
```
