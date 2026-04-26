# stores

`stores` is a schema-driven store framework with a single binary CLI. v0.2 ships with three built-in stores (`observations`, `gate`, `tasks`) demonstrating the schema → CLI → SQLite → enforcement chain: define a YAML schema, `stores install` it, and every field rule (required, enum, pattern, `required_when`, per-field actor authority) is enforced at write time. The `tasks` store adds a full multi-agent workflow engine with state machine, cycle guards, briefing templates, and deterministic main.md rendering.

## Install

```bash
cargo install --path .
stores init
stores install observations    # v0.1 bundled store
stores install gate            # v0.1 bundled store
stores install tasks           # v0.2 workflow store
```

Requires: Rust toolchain (stable). SQLite is bundled via `rusqlite-bundled` — no system SQLite dependency.

## 13-step demo walk

Run these commands in any empty directory. Each step closes a numbered verification point.

**Step 1** — Initialize the store database and manifest.

```bash
stores init
```

Creates `.stores/db.sqlite` (SQLite 3, WAL mode) and `.stores/manifest.yaml` in the current directory.

**Step 2** — Install the bundled `observations` store.

```bash
stores install ./stores/observations
```

Generates and applies the DDL for the `observations` table.

**Step 3** — Install the bundled `gate` store (proves multi-store coexistence in one DB).

```bash
stores install ./stores/gate
```

Both `observations` and `gate` tables now live in `.stores/db.sqlite`.

**Step 4** — Add an observation. Returns `OBS001`.

```bash
stores observations add --summary "thing broke"
```

**Step 5** — Triage to T3 without the required contract fields. **Fails** — the `required_when: triage.verdict == 'T3'` rule fires on all three contract sub-fields.

```bash
stores observations triage OBS001 --verdict T3
```

Expected error (all three violations in one pass):
```
Error: validation failed:
- contract.done_when: required (because triage.verdict == 'T3')
- contract.scope_in: required (because triage.verdict == 'T3')
- contract.scope_out: required (because triage.verdict == 'T3')
```

**Step 6** — Triage again with the full contract. Succeeds.

```bash
stores observations triage OBS001 --verdict T3 \
    --done-when "X works after fix" \
    --scope-in "backend handler" \
    --scope-out "frontend"
```

**Step 7** — Show OBS001. Entry includes nested `triage` and `contract` records.

```bash
stores observations show OBS001
```

Add `--json` for machine-readable output with fully nested objects (not escaped strings).

**Step 8** — List all observations.

```bash
stores observations list
```

Add `--json` for a JSON array.

**Step 9** — Add a gate decision linked to OBS001. Returns `G001`. (`task_ref = OBS001` makes the cross-store JOIN in step 12 return a real match.)

```bash
stores gate add --type decision \
    --question "Soft or hard delete on cleanup?" \
    --options "soft|hard" \
    --task-ref OBS001
```

**Step 10** — Answer the gate as a human. The `answer` field carries `actor: human`; `--invoker human` satisfies the constraint.

```bash
stores gate answer G001 --answer hard --invoker human
```

**Step 11** — Demonstrate actor-mismatch rejection. G001 is already answered, so we add G002 as a fresh pending gate, then attempt to answer it as `ai_autonomous` (auto-detected from `$CLAUDECODE`).

```bash
CLAUDECODE=1 stores gate add --type decision \
    --question "Actor check demo gate" \
    --options "yes|no"
```

This returns `G002`. Now attempt to answer without `--invoker`:

```bash
CLAUDECODE=1 stores gate answer G002 --answer hard
```

**Fails** — expected error citing the field and required actor:
```
Error: validation failed:
- <transition:answer>: transition 'answer' requires actor 'human'; invoker is 'ai_autonomous'
  (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)
- answer: field 'answer' requires actor 'human'; invoker is 'ai_autonomous'
  (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)
```

The `--invoker human` override clears it:

```bash
stores gate answer G002 --answer yes --invoker human
```

**Step 12** — Cross-store SQL JOIN in the single DB. Returns a row with non-NULL `g.display_id` (`G001`) joined to observation `OBS001` via `task_ref`.

```bash
sqlite3 .stores/db.sqlite \
  "select o.display_id, o.status, json_extract(o.triage,'$.verdict'), g.display_id \
   from observations o left join gate g on g.task_ref = o.display_id"
```

Expected output: `OBS001|triaged|T3|G001`

**Step 13** — Invoker resolution is demonstrated throughout:
- No `--invoker` + `$CLAUDECODE=1` → `ai_autonomous` (auto-detected)
- No `--invoker` + no `$CLAUDECODE` → `human` (default)
- `--invoker human|ai_autonomous|ai_with_human` → explicit override
- Writes whose actor does not match the field's `actor:` constraint are rejected with the field name, required actor, and detection source in the error.

## What this demonstrates

Two key enforcement moments:

**Required-when contract (#5 / #6):** The `contract` Record fields (`done_when`, `scope_in`, `scope_out`) each carry `required_when: "triage.verdict == 'T3'"`. Triaging to T3 without them fails with all three violations aggregated in one error. All three must be supplied together. This models the "work item needs a clear definition of done before AI takes it on" pattern.

**Per-field actor on `gate.answer` (#10 / #11):** The `answer` field in the `gate` schema carries `actor: human`. An AI invoker (auto-detected from `$CLAUDECODE`) attempting to write it is rejected with a message naming the field, the required actor, and the `$CLAUDECODE` detection source. The `--invoker human` flag overrides the auto-detection for cases where a human is running the CLI in an AI-flagged environment.

## Where the data lives

Everything lives in `.stores/db.sqlite` in the working directory. Both tables are visible:

```bash
sqlite3 .stores/db.sqlite ".tables"
# gate  observations

sqlite3 .stores/db.sqlite ".schema observations"
sqlite3 .stores/db.sqlite ".schema gate"
```

The `manifest.yaml` at `.stores/manifest.yaml` records installed stores with their schema path and install timestamp.

## How to test

```bash
# Run all unit tests (298+)
cargo test

# Run the v0.1 13-step demo (observations + gate)
bash tests/e2e.sh

# Run the tasks workflow smoke test (full lifecycle: plan → revise → BLOCKED → resume → complete)
bash tests/tasks_e2e.sh
```

Both e2e scripts require the `stores` binary on `PATH`. Run `cargo install --path .` first.

## Workflow stores

The `tasks` store ships as a bundled workflow store (`stores list-installable`):

```bash
stores install tasks
stores tasks add --title "My task" --slug "my-task" --done-when "..." --scope-in "..." --scope-out "..."
stores tasks next-action T001 --json   # which agent acts next?
stores tasks brief T001               # get the agent's briefing
stores tasks submit-plan T001 --plan-from-file plan.json
stores tasks submit-plan-review T001 --gate READY --summary "approved"
stores tasks submit-execute T001 --summary "done" --commit abc --files-changed "f.rs"
stores tasks submit-review T001 --gate PASS --critical 0 --major 0 --minor 0 --summary "ok"
stores tasks render T001              # write tasks/active/T001-slug/main.md from DB
```

The `tasks:start` skill (`stores skills install tasks:start`) is an orchestrator that drives this loop automatically via Claude subagents.

## Next steps / not in v0.1

- **Provenance log (`runs` store)** — per-operation log for AI audit trails
- **Schema migrations** — `stores upgrade` to apply schema changes to existing tables
- **`ask_user` integration** — block transitions on human confirmation via `pi-ask-user`
- **Cross-repo identity** — shared `display_id` namespace across repos
- **Distribution** — `cargo install --git <url>`; published to crates.io
- **Store templates** — `stores new <name>` scaffolds a schema from a template
- **HTTP API** — JSON over HTTP for tool-use integration
- **Reserved-column-name install check** — install-time rejection when a user field name collides with a reserved column (`status`, `display_id`, `created_at`, etc.)
