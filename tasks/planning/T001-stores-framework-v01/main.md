# T001: Stores Framework v0.1

## Meta
- **Status:** PLANNING
- **Created:** 2026-04-26
- **Last Updated:** 2026-04-26
- **Blocked Reason:** —

## Task

Build a schema-driven store framework in Rust as a single-binary CLI (`stores`). The framework lets you install "stores" — typed bags of data, each declaring its schema, lifecycle, and write authority — and provides auto-generated CLI subcommands, a single SQLite backend, and insert-time enforcement of the schema rules.

### Motivating problem

In 10.06 today, when an observation gets triaged as T3 ("needs full task workflow"), the workflow stops there: the user has to open a fresh Claude Code session and run `/task:open` to do the Intent Contract work. The user is present at triage time with full context, but that context is thrown away; `/task:open` re-acquires it later, in a fresh shell, sometimes hours later. The bottleneck is the lack of a way to **lock in the Intent Contract at the moment of capture** — schema-enforced, not vibes-enforced.

A schema-driven store framework lets us declare: *"a triage verdict of T3 requires a contract record (done_when, scope_in, scope_out)."* The CLI refuses to commit a T3 triage without the contract. The contract gets captured exactly when the user has context. T2/T3 work then drains from a queue without needing to redo the user-blocking parts.

This task is the framework, with **observations** as the worked example store. Once it works, 10.06 can adopt it; pi-extension can layer on top of it; future stores (gate, runs, capabilities, notes, tasks) all share the same shape.

### Intent Contract

**Executive intent.** Build the smallest end-to-end Rust binary that proves the schema → CLI → SQLite → enforcement chain works for one real store, demonstrating the contract-at-T3 enforcement pattern. Tracer-bullet scope, real architecture — not toy code.

**DONE_WHEN.** From a fresh shell, a user can run `cargo install --path .` followed by:
1. `stores init` — creates `.stores/db.sqlite` and `.stores/manifest.yaml` in cwd.
2. `stores install ./stores/observations` — registers the bundled observations store; SQLite DDL is generated and applied.
3. `stores install ./stores/gate` — registers the bundled gate store; SQLite DDL is generated and applied (second store proves multi-store coexistence in single DB).
4. `stores observations add --summary "thing broke"` — succeeds; returns `L001`.
5. `stores observations triage L001 --verdict T3` — **fails** with an error citing the missing contract fields (verdict-T3-requires-contract rule).
6. `stores observations triage L001 --verdict T3 --done-when "X works after fix" --scope-in "backend handler" --scope-out "frontend"` — succeeds.
7. `stores observations show L001` — prints the entry, contract embedded.
8. `stores observations list` — prints all entries.
9. `stores gate add --type decision --question "Soft or hard delete on cleanup?" --options "soft|hard" --task-ref T042` — succeeds; returns `G001`.
10. `stores gate answer G001 --answer hard --invoker human` — succeeds (the `--invoker human` is required because the `answer` field's actor is `human`; without override it would be auto-detected as `ai_autonomous` from `$CLAUDECODE` and rejected).
11. `stores gate answer G001 --answer hard` (without `--invoker`) under `CLAUDECODE=1` — **fails** with a clear actor-mismatch error.
12. `sqlite3 .stores/db.sqlite "select o.display_id, o.status, json_extract(o.triage,'$.verdict'), g.display_id from observations o left join gate g on g.task_ref = o.display_id"` — returns rows, demonstrating both stores live in one DB and cross-store SQL JOIN works.
13. CLI auto-detects `$CLAUDECODE` env to set invoker to `ai_autonomous`; `--invoker {human|ai_autonomous|ai_with_human}` overrides; writes whose actor isn't permitted by the schema are rejected with a clear error citing the offending field/rule.

**Scope — In:**
- Rust workspace (cargo) with one binary crate `stores`
- YAML schema parsing (serde + serde_yaml) into a typed in-memory schema
- SQLite via rusqlite-bundled (no system SQLite dep)
- `clap` for CLI scaffolding; subcommands generated dynamically per installed store from its schema
- Insert-time schema enforcement: required fields, enum bounds, `required_when` (single-equality expressions only — `field.path == 'value'`), pattern (regex), per-field actor authority
- Hybrid identity: integer PK in DB, display name (`L001`) generated from PK via `id_format` template
- Single global DB at `.stores/db.sqlite`; manifest at `.stores/manifest.yaml`
- Two built-in stores (both ship in v0.1):
  - **`observations`** — minimal lifecycle: states `[open, triaged, resolved, wont_fix]`; transitions `open→triaged` (actor: ai_with_human), `triaged→resolved` (actor: ai_autonomous), `triaged→wont_fix` (actor: ai_with_human); contract record gated by `required_when: triage.verdict == 'T3'`.
  - **`gate`** — async question routing for mid-flow blockers. States `[pending, answered, cancelled]`; transitions `pending→answered` (actor: human only), `pending→cancelled` (actor: any). Fields: `type` (enum: decision|script), `question` (text), `options` (list<string>), `answer` (text, write-authority: human only), `task_ref` (fk to display_id of any task-shaped store). Demonstrates multi-store coexistence in single DB and the per-field human-only write-authority enforcement.
- Generated subcommands per store: `add`, `show`, `list`, `update`, plus one subcommand per declared lifecycle transition (e.g. `triage`, `close`, `answer`, `cancel`)
- Invoker detection: `$CLAUDECODE` → `ai_autonomous`; `--invoker` flag override; default `human`
- README explaining how to install + run the demo path above

**Scope — Out (deferred to v0.2+):**
- Third built-in store (`runs` / provenance log — comes in v0.2)
- Process wiring (skill-side declarations of store dependencies + dep-checking on install)
- Provenance / runs log store
- Schema migrations
- Cross-repo identity
- Synchronous ask_user (blocking the run with a TTY prompt) — async-via-store-write only in v0.1
- Distribution beyond local folder paths (cargo registry, git URLs)
- Read-direct-from-SQLite from external code (v0.1 = reads via CLI only)
- Store templates / scaffolds
- HTTP API; multi-process concurrency beyond what SQLite WAL handles natively

**Proposed approach.** Cargo workspace with one binary crate. Schema parsed at startup into a typed Rust struct hierarchy. Per installed store: dynamically build a `clap::Command` tree from the schema and dispatch by name. SQLite DDL generated from the schema on `install`. Each insert/update goes through a validator that walks the schema tree against the input map; `required_when` expressions are evaluated against the partially-built entry (via simple AST: left-hand `dot.path` extraction + equality compare to literal RHS). Invoker resolved from env + flag and checked against per-field actor list before any DB write.

**Risks / assumptions.**
- Cargo + Rust toolchain (1.70+) available on the target machine. `cargo install --path .` is the install method.
- `rusqlite` bundled SQLite means no system SQLite dep needed; binary size grows ~3MB.
- LOC estimate: 2200–2800 lines (with gate included), achievable in a multi-phase plan.
- Both `observations` and `gate` ship in v0.1; gate adds a second store both to demonstrate multi-store and to give the framework a real async-question-routing capability from day one.
- `required_when` in v0.1 supports only `lhs.path == 'rhs-literal'`; richer expressions (AND/OR, inequalities) are deferred. Most actual rules will fit this constraint; if not, a follow-up task expands the expression language.
- The GTM column in `tasks/global-task-manager.md` for THIS framework's task tracking remains driven by the existing task-workflow plugin — it does NOT eat its own dogfood yet. Self-hosting is a future task.

**Open decisions.** None remaining — all questions resolved before planner spawn (see signed-off table below).

### Locked design decisions (signed off via AskUserQuestion)

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| 1 | Actor / write-authority model | Per-field actor tag (`human \| ai_with_human \| ai_autonomous`) + env detect (`$CLAUDECODE` → ai_autonomous), `--invoker` flag override | Strongest enforcement; keeps the human/AI/AI+human distinction explicit; matches user's stated taxonomy |
| 2 | Entry identity | Hybrid: integer PK in DB + display name (`L001`) generated via `id_format` template | Concurrency-safe, human-friendly, FKs use PK internally |
| 3 | DB topology | Single SQLite at `.stores/db.sqlite` for all installed stores | Cross-store JOINs free; cross-store transactions atomic; one DB to back up |
| 4 | CLI shape | Verb-noun (`stores observations add ...`); subcommands generated per store | Matches `./dev` / `kubectl` precedent; auto-generated subcommands keep stores cheap to add |
| 5 | Async-first question routing | When AI hits a blocker mid-flow, write to a question-routing store + transition task to BLOCKED; resume on store update | Always works; sync TTY prompt is a layered UX wrapper for v0.2 |
| 6 | Language | Rust | Single-binary distribution; strong typing for schema-driven generation; infrastructure code that doesn't change often |
| 7 | Schema declaration format | YAML at the top level + Rust types internally | Human and agent readable; agents can author schemas without compiling Rust |
| 8 | Gate in v0.1 | **YES** — both `observations` and `gate` ship in v0.1 | Multi-store coexistence + async question-routing demo work at first cut; bigger scope but bigger payoff |
| 9 | Observations lifecycle | Minimal: `[open, triaged, resolved, wont_fix]` with 3 transitions | Demonstrates state-machine enforcement without dragging in every real-world status; expansion later is mechanical |

---

## Plan
_Planner agent fills this section._

---

## Plan Review
_Plan-reviewer agent fills this section._

- **Gate:** READY | NEEDS_WORK | NOT_READY
- **Open Questions Finalized:** —
- **Issues Found:** —

> Details: plan-review.md

---

## Execution Log
_Executor agent fills this section per phase._

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

---

## Completion
_Final summary when task is complete._

- **Completed:** —
- **Summary:** —
- **Commits:** —
- **Lessons Learned:** —
