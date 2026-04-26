# T001: Stores Framework v0.1

## Meta
- **Status:** CODE_REVIEW
- **Phase 4 Start:** 2026-04-26
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
9. `stores gate add --type decision --question "Soft or hard delete on cleanup?" --options "soft|hard" --task-ref L001` — succeeds; returns `G001`. (`task_ref` points at the observation just created — v0.1 has no task store yet; the field accepts any display_id from any installed store, so the demo uses L001 to make the JOIN in step #12 return rows.)
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

### Objective

Ship a single Rust binary `stores` that loads YAML schemas, generates SQLite DDL + a `clap` subcommand tree per installed store, and enforces required / required_when / regex / per-field actor rules at insert time. Two stores (`observations`, `gate`) ship in-tree as bundled fixtures and together prove all 13 DONE_WHEN items end-to-end. Record-typed fields project their leaf sub-fields as flat top-level CLI flags so the literal demo path (`--verdict`, `--done-when`, `--scope-in`, `--scope-out`) works without dotted args.

### Scope

- **In Scope:** Cargo binary crate `stores`; YAML schema parser; in-memory typed schema model shared by validator + DDL codegen + CLI codegen; `init` / `install` / `<store> {add,show,list,update,<transition>}` commands; insert-time validator (required, enum, regex, `required_when` with the restricted single-equality grammar, per-field actor); hybrid identity (PK + `id_format`); single SQLite at `.stores/db.sqlite` via `rusqlite` bundled; manifest at `.stores/manifest.yaml`; `$CLAUDECODE` env detection + `--invoker` override; bundled `observations` and `gate` stores as YAML schema files in repo; `--<field>-from-file` / stdin inputs for long markdown; `--json` output flag; `show`/`list` round-trip JSON columns back into nested structures; README walking the demo path.
- **Out of Scope:** third store (`runs`), skill-side dependency declarations, schema migrations, cross-repo identity, sync TTY ask_user, distribution beyond local folder paths, HTTP API, external read-direct-from-SQLite (reads via CLI only).

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Cargo scaffold + `stores init` (manifest + empty SQLite) | Low |
| 2 | Schema parser (YAML → typed Rust model, including Record sub-field semantics) | Medium |
| 3 | `stores install <path>`: DDL codegen + manifest registration | Medium |
| 4 | Dynamic CLI codegen + `add` / `show` / `list` / `update` verbs (Record-flattening + List parsing) | High |
| 5 | Enforcement engine: required / required_when (cross-Record paths) / regex / per-field actor | High |
| 6 | Lifecycle transitions + bundled `observations` store (T3 contract gate) | Medium |
| 7 | Bundled `gate` store + human-only actor enforcement demo | Medium |
| 8 | End-to-end demo verification, `--json` polish, README | Low |

### Phase Details

#### Phase 1: Cargo scaffold + `stores init`

- **Objective:** Stand up the binary crate, wire `clap`, get `stores init` creating `.stores/db.sqlite` and `.stores/manifest.yaml` in the cwd. No schemas yet.
- **Files to create:**
  - `Cargo.toml` (workspace-less single-binary; deps: `clap` v4 with `derive`, `serde` + `serde_yaml`, `rusqlite` with `bundled`, `regex`, `anyhow`, `thiserror`, `serde_json`)
  - `src/main.rs` (entry point; `clap::Command` with top-level `init` subcommand and a placeholder for dynamically added store subcommands)
  - `src/cli/mod.rs`, `src/cli/init.rs` (the `init` handler)
  - `src/manifest.rs` (`Manifest { stores: Vec<InstalledStore> }`, load/save to `.stores/manifest.yaml`)
  - `src/db.rs` (open/create connection, set WAL pragma)
  - `src/paths.rs` (resolve `.stores/` under cwd; helpers)
  - `.gitignore` (target/, .stores/)
- **Acceptance Criteria:**
  - [ ] `cargo build` succeeds.
  - [ ] `cargo install --path .` installs a `stores` binary.
  - [ ] **Re-running `cargo install --path .` after a code change replaces the binary cleanly without touching `.stores/`** (resolves minor m1).
  - [ ] `stores init` in an empty dir creates `.stores/db.sqlite` (valid SQLite file, WAL on) and `.stores/manifest.yaml` (empty `stores: []`). **(DONE_WHEN #1)**
  - [ ] Re-running `stores init` is idempotent (does not error, does not clobber existing manifest).

#### Phase 2: Schema parser (YAML → typed Rust model)

- **Objective:** Parse a store-schema YAML file into a single canonical in-memory model that downstream phases (DDL codegen, CLI codegen, validator) all consume. Decide field-type → SQLite-column-type mapping here. **Lock in the Record-sub-field semantics that drive both flat CLI args and required_when enforcement.**
- **Files to create:**
  - `src/schema/mod.rs` — public `Schema { name, id_format, fields, lifecycle, transitions }` plus `Field { name, ty, required, required_when, pattern, actor, enum_values, description }`. Crucially, when `ty == Record(Vec<Field>)`, **each inner `Field` is a full `Field` struct** that may carry its own `required`, `required_when`, `pattern`, and `actor` — Record sub-fields are first-class Fields, not opaque blobs.
  - `src/schema/types.rs` — `FieldType` enum: `Text`, `Integer`, `Bool`, `Enum(Vec<String>)`, `List(Box<FieldType>)`, `Record(Vec<Field>)`, `DisplayId` (FK), `Timestamp`
  - `src/schema/actor.rs` — `Actor` enum `{ Human, AiAutonomous, AiWithHuman }`, with `from_env() -> Actor` reading `$CLAUDECODE`
  - `src/schema/required_when.rs` — minimal AST: `Expr { lhs_path: Vec<String>, rhs_literal: String }`; parser accepts only `dotted.path == 'literal'` with single quotes; rejects everything else with a clear error. The `lhs_path` may cross Record boundaries (e.g. a `required_when` declared on `contract.done_when` whose `lhs_path = ["triage","verdict"]` resolves out of the `contract` Record into the sibling `triage` Record).
  - `src/schema/lifecycle.rs` — `Lifecycle { states: Vec<String>, initial_state: Option<String>, transitions: Vec<Transition { from, to, verb, actor }> }`. `initial_state` defaults to `states[0]` if omitted (resolves M3).
  - `src/schema/parse.rs` — `Schema::from_yaml(&str) -> Result<Schema>`; surface line numbers in errors via `serde_yaml::Error`
  - `src/schema/flatten.rs` — `fn leaf_args(schema: &Schema) -> Vec<LeafArg { cli_name: String, path: Vec<String>, field: &Field }>`; walks each top-level Field and, for Records, recurses to enumerate leaf sub-fields. Asserts uniqueness of `cli_name` across all leaves at install-time (errors if two leaves collide). The `cli_name` is the **leaf field's own name converted to kebab-case** (e.g. Record `contract` with sub-field `done_when` → `--done-when`; Record `triage` with sub-field `verdict` → `--verdict`). Parent Record name is NOT included in the flag; this matches the literal DONE_WHEN demo (`--verdict`, `--done-when`, not `--triage-verdict`).
  - `src/id_format.rs` (parser only — renderer lives in Phase 4): parse `id_format` template, validate that it contains exactly one `{:0Nd}` placeholder.
- **Acceptance Criteria:**
  - [ ] Unit tests parse a hand-written YAML covering every `FieldType`, `required_when`, an enum field, an actor tag, and a transition; assert the round-tripped struct.
  - [ ] **A YAML fixture defines a Record `contract` whose sub-field `done_when` carries `required_when: triage.verdict == 'T3'`; the parsed model exposes that `required_when` on the sub-`Field` instance, not on the parent Record** (resolves C3 model side).
  - [ ] `required_when: "triage.verdict == 'T3'"` parses to `Expr { lhs_path: ["triage","verdict"], rhs_literal: "T3" }`.
  - [ ] Malformed `required_when` (e.g. `a == b OR c == d`, `a != b`) returns an error naming the unsupported token.
  - [ ] Unknown field type or unknown actor value returns an error pointing at the offending key.
  - [ ] `leaf_args(schema)` for a fixture with Records `triage{verdict, notes}` and `contract{done_when, scope_in, scope_out}` returns 5 leaf args with `cli_name`s `verdict, notes, done-when, scope-in, scope-out` (resolves C2 model side).
  - [ ] `leaf_args` returns an error if two leaves collide (e.g. two Records both containing a sub-field named `notes`); error message names both parent paths.
  - [ ] **`id_format: "L{:03d}"` parses; rendering with `pk=1` yields `L001`** (resolves m3). Renderer impl lives in Phase 4 but the format-string validation lives here.
  - [ ] `Lifecycle::initial_state` defaults to `states[0]` when YAML omits the field; explicit value overrides.

#### Phase 3: `stores install <path>`: DDL codegen + manifest registration

- **Objective:** Given a folder containing `schema.yaml`, generate and apply SQLite DDL for the store, then append it to `.stores/manifest.yaml`. This phase introduces the field-type → column-type mapping concretely, plus a fixture exercise covering all field-types.
- **Files to create:**
  - `src/install.rs` — entry point: read `<path>/schema.yaml`, parse, run `leaf_args` uniqueness check, codegen DDL, execute against `.stores/db.sqlite`, update manifest
  - `src/codegen/ddl.rs` — `fn ddl_for(schema: &Schema) -> String`; mapping rules: scalar (`Text`→TEXT, `Integer`→INTEGER, `Bool`→INTEGER 0/1, `Timestamp`→TEXT ISO-8601, `Enum`→TEXT with CHECK constraint, `DisplayId`→TEXT) become real columns; `List(_)` and `Record(_)` collapse to a single TEXT column holding JSON (rusqlite reads/writes via `serde_json::Value::to_string`); reserved columns: `id INTEGER PRIMARY KEY AUTOINCREMENT`, `display_id TEXT UNIQUE NOT NULL`, `status TEXT NOT NULL`, `created_at TEXT`, `updated_at TEXT`, `created_by TEXT`, `updated_by TEXT`
  - Manifest gains a per-store entry: `{ name, schema_path (canonical absolute), installed_at, table_name }`
  - `tests/fixtures/all_types_store/schema.yaml` — synthetic fixture exercising every `FieldType` variant (Text, Integer, Bool, Enum, List<Text>, Record with sub-fields including a `required_when`, DisplayId, Timestamp). Used by Phase 3 + Phase 5 unit tests (resolves cross-cutting #3).
- **Acceptance Criteria:**
  - [ ] `stores install <path-to-all_types_store-fixture>` succeeds; resulting table's column list matches the expected DDL (snapshot test); CHECK constraints on `Enum` columns present; JSON columns are TEXT.
  - [ ] `stores install ./stores/gate` after the first install succeeds and produces a second table in the same `db.sqlite` (full validation in Phase 7). **(DONE_WHEN #3 contributes)**
  - [ ] Re-installing the same store path is rejected with a clear "already installed; v0.1 has no migrations" error.
  - [ ] **Installing a different folder whose `schema.yaml` declares the same `name:` as an installed store is rejected with a name-collision error in the same error class** (resolves m4).
  - [ ] DDL emitted is deterministic (snapshot test on the SQL string).

#### Phase 4: Dynamic CLI codegen + `add` / `show` / `list` / `update` verbs

- **Objective:** On every CLI invocation, after parsing the manifest, dynamically build the `clap::Command` tree by adding one `Command` per installed store with subcommands `add`, `show <display_id>`, `list`, `update <display_id>`. **Each generated subcommand exposes one `--<cli_name>` arg per leaf returned by `leaf_args(schema)` — Record sub-fields appear as flat top-level flags, not as `--<record>` taking JSON.** All args are optional at the clap level — `required` is enforced by the validator in Phase 5, which gives better error messages than clap's built-in required check.
- **Files to create:**
  - `src/cli/dynamic.rs` — `fn build_root(manifest: &Manifest, schemas: &HashMap<String, Schema>) -> clap::Command`; per store, a `clap::Command::new(store.name)` with the four base verbs; per verb, iterate `leaf_args(schema)` and emit `clap::Arg::new(leaf.cli_name).long(&leaf.cli_name)` plus the `--<cli-name>-from-file <path>` companion arg and stdin (`-`) handling for `Text` leaves; **for `List(_)` leaves, the arg accepts a single string and is split on `|` at parse time (e.g. `--options "soft|hard"` → `["soft", "hard"]`)** (resolves M1); `--json` flag at the top level.
  - `src/cli/dispatch.rs` — given the parsed `ArgMatches`, route to the right handler with the correct store schema in hand. **Reassembly:** as args are read off `ArgMatches`, leaf values are nested back into their parent Record paths to build the in-memory `EntryMap` the validator + writer consume (so the validator sees `entry["contract"]["done_when"]`, not a flat key).
  - `src/handlers/add.rs`, `src/handlers/show.rs`, `src/handlers/list.rs`, `src/handlers/update.rs` — each takes `(&Schema, &Connection, &ArgMatches, Actor)`, builds an entry map, calls the validator (Phase 5 stub returns Ok for now), writes the row, prints result. **`add` writes `status = lifecycle.initial_state` (default `states[0]`)** (resolves M3). **`add` and `update` populate `created_at`/`updated_at` (ISO-8601 UTC) and `created_by`/`updated_by` (`invoker.to_string()`); `update` only touches `updated_*`** (resolves m2). On `show`/`list`, `Record` and `List` columns are deserialized from their stored JSON string back into nested `serde_json::Value` so output preserves the nested shape (resolves M2).
  - `src/id_format.rs` — render `id_format: "L{:03d}"` template against the new PK to produce `display_id`; runs inside the same DB transaction as the INSERT to avoid races
  - `src/output.rs` — text + `--json` formatters; `--json` for `show` emits the entry as a single JSON object with Records nested under their parent keys; `--json` for `list` emits a JSON array of such objects.
- **Acceptance Criteria:**
  - [ ] After installing a fixture store, `stores <store> add --<field> value` writes a row, returns the rendered display_id (e.g. `L001`), and exits 0. **(DONE_WHEN #4 contributes)**
  - [ ] **`leaf_args` are emitted as flat `--<cli_name>` flags; a fixture with Record `contract{done_when, scope_in, scope_out}` accepts `--done-when X --scope-in Y --scope-out Z` (no dotted args, no `--contract <json>`)** (resolves C2 codegen side).
  - [ ] **`--options "soft|hard"` against a `list<text>` field deserializes to `["soft","hard"]`** (resolves M1).
  - [ ] `stores <store> show <display_id>` prints the entry; `--json` emits valid JSON. **`Record` and `List` columns are decoded back to nested structures** so output includes (e.g.) a `triage` parent key with `verdict='T3'` nested inside, and a `contract` parent key with the three sub-fields nested inside — verified end-to-end in Phase 6 against the real schema (resolves M2). **(DONE_WHEN #7 contributes)**
  - [ ] `stores <store> list` prints all rows; `--json` emits a JSON array. **(DONE_WHEN #8 contributes)**
  - [ ] `--summary-from-file path/to/text.md` and piping via `--summary -` both populate the field correctly.
  - [ ] **`stores <store> update <display_id> --<field> value` mutates the row through the validator and bumps `updated_at`/`updated_by`** (resolves M4).
  - [ ] On every `add`, `status = lifecycle.initial_state` (defaulting to `lifecycle.states[0]`); asserted in a unit test (resolves M3).
  - [ ] On every `add`, `created_at`/`updated_at`/`created_by`/`updated_by` are populated; on every `update`, `updated_at`/`updated_by` are bumped (resolves m2).
  - [ ] `stores --help` shows installed stores; `stores observations --help` shows verbs.

#### Phase 5: Enforcement engine

- **Objective:** Replace the Phase 4 validator stub with the real one. This is the load-bearing correctness phase. The validator owns all four rule types and produces error messages that cite the offending field and rule. **The validator runs against the in-memory typed `EntryMap` (nested) — never against the SQLite row — so dotted-path lookup in `required_when` traverses the typed entry-map representation, including descent into Record sub-fields and ascent back out to sibling Records** (resolves M5 third bullet).
- **Files to create:**
  - `src/validate/mod.rs` — `fn validate(schema: &Schema, entry: &EntryMap, op: Op, invoker: Actor) -> Result<(), Vec<ValidationError>>`; `Op` is `Add | Update | Transition(verb)`. The validator walks both top-level `Field`s and Record sub-`Field`s recursively so per-leaf rules fire.
  - `src/validate/required.rs` — required + `required_when` evaluation against the partially-built `EntryMap`; uses the AST from Phase 2; dotted-path lookup walks the typed nested structure (Record → sub-field → value). **A `required_when` declared on a Record sub-field whose `lhs_path` resolves into a sibling Record's leaf (e.g. `contract.done_when`'s `required_when: triage.verdict == 'T3'`) must evaluate correctly** (resolves C3 enforcement side).
  - `src/validate/enum_check.rs`, `src/validate/regex_check.rs`
  - `src/validate/actor.rs` — for each field present in the entry (and for the transition itself, on transition ops), check `invoker` is allowed by the field's / transition's declared actor; error message: `"field 'answer' requires actor 'human'; invoker is 'ai_autonomous' (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)"`
  - `src/validate/error.rs` — `ValidationError { field_path, rule, message }`; pretty-print as a bullet list
- **Acceptance Criteria:**
  - [ ] Unit tests cover each rule type with passing + failing fixtures.
  - [ ] **A dedicated `required_when` unit test uses a Record sub-field whose `lhs_path` crosses into a sibling Record (`contract.done_when` declared with `required_when: triage.verdict == 'T3'`); the test asserts the rule fires when `triage.verdict='T3'` and is silent otherwise** (resolves item #7 / C3 enforcement).
  - [ ] `stores observations triage L001 --verdict T3` (without contract fields) errors out citing the three missing fields and the `required_when` that triggered them. **(DONE_WHEN #5 — full integration in Phase 6)**
  - [ ] An `add` against a Text field with a `pattern` regex that doesn't match is rejected.
  - [ ] An invoker mismatch produces the actor-mismatch error format above. **(DONE_WHEN #11, #13 contribute)**
  - [ ] Errors aggregate (multiple violations reported in one pass, not one-at-a-time).

#### Phase 6: Lifecycle transitions + bundled `observations` store

- **Objective:** Generate one CLI verb per declared lifecycle transition (e.g. `triage`, `close`), wire transition validation (current state → declared `from`; transition actor; field updates from the same call go through the validator), and ship the real `observations` schema in-tree.
- **Files to create:**
  - `src/cli/dynamic.rs` — extended: per `Transition` in the schema, add a verb subcommand `<verb> <display_id> [--<leaf_arg> value ...]` (where `--<leaf_arg>` are the same flat leaf-args used by `add`/`update`); the verb has implicit semantics "set status to `to`, then run validator with `Op::Transition(verb)`, then write".
  - `src/handlers/transition.rs`
  - `stores/observations/schema.yaml` — fields: `summary` (text, required), `body` (text, optional, supports `--body-from-file`), `triage` (record: sub-fields `verdict` (enum: T1,T2,T3) and `notes` (text)), `contract` (record: sub-fields `done_when` (text), `scope_in` (text), `scope_out` (text), **each carrying `required_when: triage.verdict == 'T3'` at the sub-field level** — matching the C3 model), `tags` (list<text>); `lifecycle.states: [open, triaged, resolved, wont_fix]`; `initial_state` omitted (defaults to `open`); transitions `open→triaged` verb `triage` actor `ai_with_human`, `triaged→resolved` verb `resolve` actor `ai_autonomous`, `triaged→wont_fix` verb `wont_fix` actor `ai_with_human`; `id_format: "L{:03d}"`
  - `stores/observations/README.md` — the bundled-store mini-README
- **Acceptance Criteria:**
  - [ ] `stores install ./stores/observations` succeeds and the table has the expected schema. **(DONE_WHEN #2 fully)**
  - [ ] `stores observations add --summary "thing broke"` returns `L001` and writes `status='open'` (initial_state default applied). **(DONE_WHEN #4 fully)**
  - [ ] `stores observations triage L001 --verdict T3` fails with the contract-required error, citing `contract.done_when`, `contract.scope_in`, `contract.scope_out`, and the `required_when` rule. **(DONE_WHEN #5 fully)**
  - [ ] `stores observations triage L001 --verdict T3 --done-when "..." --scope-in "..." --scope-out "..."` succeeds; entry status moves to `triaged`. **(DONE_WHEN #6)**
  - [ ] `stores observations show L001` shows the entry with `triage` and `contract` Records nested under their parent keys; `--json` output validates as JSON and contains `triage.verdict='T3'` plus the three `contract.*` sub-fields under their parent keys (ties off M2). **(DONE_WHEN #7)**
  - [ ] `stores observations list` shows the row. **(DONE_WHEN #8)**

#### Phase 7: Bundled `gate` store + human-only actor enforcement demo

- **Objective:** Ship the `gate` schema, exercise multi-store coexistence in one DB, and prove the per-field actor enforcement on the `answer` field. **Demo uses `--task-ref L001` so the cross-store JOIN in DONE_WHEN #12 returns matched rows.**
- **Files to create:**
  - `stores/gate/schema.yaml` — fields: `type` (enum: `decision|script`, required), `question` (text, required), `options` (list<text>, optional), `answer` (text, optional, **actor: human**), `task_ref` (display_id, optional, no FK constraint at SQL level — cross-store reference by convention; accepts any display_id from any installed store); `lifecycle.states: [pending, answered, cancelled]`; transitions `pending→answered` verb `answer` actor `human`, `pending→cancelled` verb `cancel` actor `ai_autonomous`; `id_format: "G{:03d}"`
  - `stores/gate/README.md`
- **Acceptance Criteria:**
  - [ ] `stores install ./stores/gate` succeeds; both tables coexist in `.stores/db.sqlite`. **(DONE_WHEN #3 fully)**
  - [ ] **`stores gate add --type decision --question "..." --options "soft|hard" --task-ref L001` returns `G001`** (DONE_WHEN #9 — task-ref is `L001` per updated DONE_WHEN; resolves C1 user-side).
  - [ ] `stores gate answer G001 --answer hard --invoker human` succeeds. **(DONE_WHEN #10)**
  - [ ] Under `CLAUDECODE=1`, `stores gate answer G001 --answer hard` (no `--invoker`) is rejected with the actor-mismatch message naming `answer` and the required actor `human`. **(DONE_WHEN #11)**

#### Phase 8: End-to-end demo verification, `--json` polish, README

- **Objective:** Run every numbered DONE_WHEN step from a fresh shell against a fresh dir; fix gaps. Write the README. Validate the cross-store SQL JOIN returns real matches.
- **Files to create:**
  - `README.md` — install + the 13-step demo path verbatim, expected output for each
  - `tests/e2e.sh` — bash script that runs the 13 steps in a temp dir against the freshly-installed binary; `set -euo pipefail`; greps for expected display_ids and error substrings; **executes the exact `sqlite3` JOIN query from DONE_WHEN #12 and asserts the result contains a non-NULL gate `display_id` matching `G001` joined to observation `L001` via `g.task_ref = o.display_id`**.
- **Acceptance Criteria:**
  - [ ] `tests/e2e.sh` exits 0. **(DONE_WHEN #1–#13 all)**
  - [ ] **The `sqlite3` JOIN query returns ≥1 row matching the `L001` observation with a non-NULL gate `display_id` (`G001`) — i.e. a real JOIN match exists, not just a LEFT-JOIN row with NULL gate columns** (DONE_WHEN #12; resolves C1 verification side).
  - [ ] **`tests/e2e.sh` is a literal copy of the README's numbered command list — same commands, same order, no extra setup/seeding steps outside what the README shows. The script includes a top-of-file comment block listing the README's commands in order so README↔script correspondence is auditable at a glance** (resolves item #11 / cross-cutting #2).
  - [ ] README renders the install + demo flow with no missing steps; copy-paste from README into a fresh shell reproduces e2e success.
  - [ ] `--json` output validates as JSON for every read/write verb (script pipes to `jq .`); `show` and `list` JSON include nested `triage`/`contract` keys (ties off M2).

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| In-memory schema model: shared vs split per consumer (validator / DDL codegen / CLI codegen) | (a) one canonical `Schema` struct shared by all three; (b) per-consumer view structs derived from the YAML | (a) one shared `Schema` | The schema is small (≤ a few hundred fields total across all installed stores) and the three consumers all need the same data; splitting invites drift. Phase 2 produces it once at startup. |
| List / Record field storage in SQLite | (a) child tables with FKs; (b) JSON in a TEXT column, queried via `json_extract` | (b) JSON in TEXT | v0.1 has no migrations and reads via CLI only; child tables explode the codegen surface and force join logic into every read. The DONE_WHEN #12 SQL example already uses `json_extract`, signalling JSON is the intended shape. **Accepted technical debt:** any future structured query like `where contract.scope_in like '%backend%'` needs `json_extract` everywhere; tracked as a v0.2 friction point. |
| **Record sub-field treatment (CLI + validator + storage round-trip)** — covers C2 + C3 + M2 cohesively | (a) flatten leaves to top-level `--<cli_name>` args + sub-fields are first-class `Field`s with their own rules + nested round-trip on read; (b) dotted args (`--triage.verdict`) + Record-level rules only; (c) `--<record> <json>` opaque blob | **(a) flatten + first-class sub-fields + nested round-trip** | The literal DONE_WHEN demo uses `--verdict`, `--done-when`, `--scope-in`, `--scope-out` — only (a) matches. Sub-fields carrying their own `required_when` is the spine of the T3-contract enforcement (the motivating story). Nested round-trip on `show`/`list` is needed for `--json` to be useful. **Naming rule:** the CLI flag for a leaf is the leaf's own field name converted to kebab-case (e.g. `done_when` → `--done-when`); parent Record name is NOT prefixed. **Uniqueness rule:** all leaf names within a single store must be unique; install-time check (`leaf_args` in Phase 2) rejects collisions with a message naming both parent paths. (Resolves C2 + C3 + M2.) |
| **List CLI input format** | (a) `\|`-separated single string; (b) comma-separated; (c) repeated `--flag value --flag value` | **(a) `\|`-separated** | Matches DONE_WHEN #9 literally (`--options "soft\|hard"`). Parser splits on `\|` for `List(Text)` fields. Repeated-flag form deferred. (Resolves M1.) |
| **Initial-status convention** | (a) explicit `initial_state` field in lifecycle (optional); (b) always implicit `states[0]`; (c) require explicit `initial_state` always | **(a) optional explicit, defaults to `states[0]`** | Most stores want the obvious default; allowing override is cheap and forward-compatible. (Resolves M3.) |
| **`update` verb scope in v0.1** | (a) drop entirely; (b) keep with a real AC | **(b) keep with AC** | Already wired in Phase 4; cost to test is low; gives users a generic write path beyond transitions. Real AC in Phase 4 closes M4. |
| Building the `clap::Command` tree | (a) build statically with derive macros, hard-code each store; (b) build dynamically at runtime from manifest + parsed schemas | (b) dynamic | Stores are user-installed; static derive can't see them. `clap::Command::new(...).subcommand(...)` is fully dynamic and well-supported. **Accepted risk:** runtime construction bypasses derive macros' compile-time checks; argument-name collisions across Record sub-fields within one store are runtime-discoverable only — install-time `leaf_args` uniqueness check is the backstop. Cross-store collisions are scoped (store names are clap-level subcommand boundaries) so two stores each owning a `--notes` leaf is fine. |
| `required_when` AST scope | (a) full expression language (AND/OR/NOT, comparisons); (b) single equality only | (b) single equality | Locked by user; mirrored in the YAML grammar to fail-fast on unsupported expressions and avoid building a precedence parser. |
| Display-id generation timing | (a) compute outside the transaction from `last_insert_rowid`; (b) compute inside the same transaction as the INSERT, then `UPDATE display_id` | (b) same transaction | Concurrent writers under WAL could otherwise interleave. One transaction: INSERT stub row, read `rowid`, render template, UPDATE display_id, COMMIT. |
| Re-install handling | (a) silent no-op on path-match; (b) reject path-match with "no migrations in v0.1" + reject same-name-different-path collision | **(b) reject both** | Migrations are explicitly out of scope; silent skip hides schema drift. Same-name-different-path is even more dangerous — two folders racing the same table. Both rejected with clear errors. (Resolves m4.) |
| Validator returns first error vs all errors | (a) bail on first; (b) collect all violations and return them together | (b) collect all | The T3-without-contract case has 3 missing fields; users want them all in one shot, especially under AI-driven retries. |
| `Bool` storage | (a) INTEGER 0/1; (b) TEXT 'true'/'false' | (a) INTEGER | SQLite idiom; `json_extract` returns numbers cleanly; one less serialization branch. |
| **Record vs List update merge semantics** | (a) deep-merge sub-keys for Record, replace-wholesale for List and scalars; (b) always replace wholesale | **(a) deep-merge Record, replace-wholesale List/scalar** | Record sub-fields are independent leaves — a user updating `severity` should not lose `notes`. List replacement is intentional (e.g. correcting a tag list means the new value supersedes the old). Scalar replacement is unchanged behaviour. (Fixes M1 in Phase 4 Revise cycle 1.) |
| Where bundled store schemas live in the repo | (a) `stores/observations/`, `stores/gate/` at repo root; (b) `examples/`; (c) `assets/` embedded into the binary | (a) repo-root `stores/` | Matches the literal DONE_WHEN install commands (`stores install ./stores/observations`); discoverable; no embed-vs-disk divergence. |
| Error type | (a) `anyhow::Error` everywhere; (b) `thiserror`-derived enums at module boundaries, `anyhow` in CLI | (b) hybrid | The validator's errors are structured (`ValidationError { field_path, rule, message }`) and need to be programmatically aggregated; CLI top-level can flatten via `anyhow`. |
| Manifest format | (a) YAML (matches schema files); (b) JSON; (c) TOML | (a) YAML | One format for users to edit by hand; symmetric with schema files; serde_yaml already a dep. |

### Risks / Assumptions (expanded — resolves M5)

Carried from `## Task` (Cargo toolchain, rusqlite-bundled binary size, 2200–2800 LOC estimate, both stores in v0.1, `required_when` grammar restriction, GTM still tracks this task externally), plus three implementation-level risks the reviewer surfaced:

- **Dynamic clap construction surface area.** Building the `Command` tree at runtime means clap's derive-macro compile-time checks don't fire. Argument-name collisions across Record sub-fields within one store are runtime-discoverable only. **Mitigation:** the `leaf_args(schema)` uniqueness check in Phase 2 runs at install time and refuses to add a store whose leaf names would collide internally. Cross-store collisions are inherently scoped (store names are clap-level subcommand boundaries) so two stores each defining a `--notes` leaf is fine — the conflict only matters within a store.
- **JSON-in-TEXT vs structured columns for nested types.** Storing Record/List as JSON-in-TEXT (Decision Matrix row 2) means future structured queries like `where contract.scope_in like '%backend%'` need `json_extract` everywhere; aggregation, indexes, and FK enforcement on nested values are all friction points. **Accepted as known cost** for v0.1; promote to a v0.2 design choice if pain shows up.
- **`required_when` evaluator straddles two representations.** The validator runs against the in-memory typed `EntryMap` (Phase 5) — never the SQLite row. Dotted-path lookup must therefore traverse the typed entry-map representation, including nesting into Record sub-fields. Phase 5's `required.rs` is explicitly clarified to walk `EntryMap`, not `serde_json::Value` from a stored column. Re-validation on `update` re-reads the row, deserializes JSON columns back into the `EntryMap` shape, applies the diff, then runs the validator against the merged in-memory form.

### Open Questions

None. All nine locked decisions cover the load-bearing architectural choices; the eleven plan-review items are addressed in-phase or via Decision Matrix entries (see change-log below). No new user input required to start execution.

### Revise Cycle 1

Change-log mapping each numbered item from `plan-review.md` to its fix:

1. **JOIN-zero-rows (C1) →** DONE_WHEN #9 already updated to `--task-ref L001` (see `## Task`, line 34). Phase 7 AC #2 now uses `--task-ref L001`. Phase 8 AC #2 now requires `≥1 row with non-NULL gate display_id` (a real JOIN match), not merely "≥1 row" (which a LEFT JOIN with NULL gate columns would satisfy hollowly). Phase 8 AC #3 pins README↔script correspondence.
2. **Record sub-field treatment (C2 + C3 + M2) →** New Decision Matrix row "Record sub-field treatment" with explicit (a) flatten leaves to flat `--<kebab-name>` flags; (b) install-time uniqueness check; (c) sub-fields are first-class `Field`s with their own `required_when`/`pattern`/`actor`; (d) `show`/`list` round-trip JSON columns back to nested form. Phase 2 adds `src/schema/flatten.rs` + ACs for sub-field rules and uniqueness. Phase 4 ACs cover flat flag emission and JSON round-trip on read. Phase 5 walks Record sub-fields recursively.
3. **List CLI input format (M1) →** New Decision Matrix row "List CLI input format" choosing `|`-separated. Phase 4 AC asserts `--options "soft|hard"` parses to `["soft","hard"]`.
4. **Initial-status convention (M3) →** Phase 2 schema model adds `Lifecycle.initial_state: Option<String>` defaulting to `states[0]`; Phase 4 handler text + AC require `add` to write `status = lifecycle.initial_state`; Phase 6 AC explicitly checks `add` produces `status='open'`.
5. **`update` verb fate (M4) →** Kept; new Phase 4 AC exercises it (`update <display_id> --<field> value` mutates row through validator and bumps `updated_*`).
6. **Reserved-column population (m2) →** Phase 4 handler text + AC require `created_at`/`updated_at`/`created_by`/`updated_by` populated on every insert/update.
7. **Phase 5 `required_when` Record-sub-field test (C3 enforcement) →** New Phase 5 AC: dedicated unit test where a Record sub-field's `required_when` LHS path crosses into a sibling Record (`contract.done_when`'s `required_when: triage.verdict == 'T3'`), asserting the rule fires correctly when triggered and is silent otherwise.
8. **`id_format` round-trip test (m3) →** New Phase 2 AC: `id_format: "L{:03d}"` parses; rendering with `pk=1` yields `L001`.
9. **Same-name-different-path install rejection (m4) →** New Phase 3 AC: installing a different folder whose schema declares an existing store's `name:` is rejected with the same "already installed" error class. Decision Matrix "Re-install handling" updated to cover both cases.
10. **Risks expansion (M5) →** Risks/Assumptions section expanded with the three named risks (dynamic clap surface, JSON-vs-column tradeoff as accepted debt, `required_when` evaluator on `EntryMap` not the row).
11. **README-as-test pinning (cross-cutting #2) →** New Phase 8 AC: `tests/e2e.sh` is a literal copy of README's numbered command list — no extra setup steps; auditable via top-of-file correspondence comment.

Adjacent fixes not numbered but addressed alongside the above: m1 (subsequent `cargo install --path .` replaces binary cleanly without touching `.stores/`) added as a Phase 1 AC. Cross-cutting #3 (Phase 3 fixture covering all field-types) addressed by adding `tests/fixtures/all_types_store/schema.yaml` with the explicit AC that the install snapshot covers every `FieldType` variant.

No disagreements with the reviewer; all 11 items got in-phase fixes or new Decision Matrix entries.

---

## Plan Review

- **Gate:** `READY` (cycle 2 of 3)
- **Reviewed:** 2026-04-26 by `plan-reviewer`
- **Summary:** All 11 numbered cycle-1 items are genuinely addressed in the revised Plan, not just claimed in the change-log. The load-bearing new work — `src/schema/flatten.rs` (item 2) — solves C2 + C3 + M2 cohesively: leaf args flatten to flat `--<kebab>` flags with install-time uniqueness check; Record sub-fields are first-class `Field`s carrying their own rules; Phase 4 reassembles flat CLI input back into a nested `EntryMap`; Phase 5 walks that nested form so cross-Record `required_when` paths (`contract.done_when` ← `triage.verdict == 'T3'`) resolve correctly. The pieces fit. Fresh-eye pass turned up only minor edge cases (reserved-column-name vs leaf-name collision check is implicit; no `|`-escape in List parser; `update` doesn't explicitly reject status mutations) — none gate-blocking. Cycle-1's 3 critical / 5 major / 4 minor reduces to 0 / 0 / 3 nits, all deferrable. Advance to executor.
- **Issues (cycle 2):** 0 critical / 0 major / 3 minor (deferrable nits, see plan-review.md cycle 2)
- **Open Questions Finalized:** None.

### Per-item verification (cycle 1 → cycle 2)

| # | Cycle-1 item | Cycle-1 status | Cycle-2 verdict | Note |
|---|---|---|---|---|
| 1 | JOIN-zero-rows (C1) | Critical | PASS | Task line 34, Phase 7 AC, Phase 8 AC all use `--task-ref L001`; Phase 8 AC requires real (non-NULL) JOIN match |
| 2 | Record sub-field treatment (C2+C3+M2) | Critical | PASS | New `flatten.rs` + Decision Matrix row; sub-fields first-class `Field`s; Phase 4 reassembles nested EntryMap; Phase 5 walks recursively |
| 3 | List CLI input format (M1) | Major | PASS | Decision Matrix row + Phase 4 AC for `--options "soft\|hard"` → `["soft","hard"]` |
| 4 | Initial-status convention (M3) | Major | PASS | `Lifecycle.initial_state: Option<String>` defaults to `states[0]`; Phase 4 + Phase 6 ACs exercise it |
| 5 | `update` verb fate (M4) | Major | PASS | Kept; Phase 4 AC mutates row + bumps `updated_*` |
| 6 | Reserved-column population (m2) | Minor | PASS | Phase 4 handler text + AC for `created_at`/`updated_at`/`created_by`/`updated_by` |
| 7 | Phase 5 cross-Record `required_when` test (C3 enforcement) | Critical | PASS | Phase 5 AC line 209 — dedicated test for `contract.done_when` with sibling-Record LHS path |
| 8 | `id_format` round-trip (m3) | Minor | PASS | Phase 2 AC: `"L{:03d}"` parses + renders `pk=1` → `L001` |
| 9 | Same-name-different-path rejection (m4) | Minor | PASS | Phase 3 AC + Decision Matrix Re-install row covers both cases |
| 10 | Risks expansion (M5) | Major | PASS | All three named risks (dynamic clap, JSON-vs-column debt, EntryMap-not-row) added |
| 11 | README-as-test pinning | Major | PASS | Phase 8 AC: e2e.sh is literal copy of README; top-of-file correspondence comment |
| — | Adjacent: m1 (`cargo install` re-run) | Minor | PASS | Phase 1 AC added |
| — | Adjacent: cross-cutting #3 (all-types fixture) | Minor | PASS | Phase 3 `tests/fixtures/all_types_store/schema.yaml` added with snapshot AC |

> Details: plan-review.md (cycle 2 section appended; cycle 1 preserved)

---

## Execution Log

### Phase 1: Cargo scaffold + stores init

- **Status:** COMPLETE
- **Started:** 2026-04-26
- **Completed:** 2026-04-26

**Files Created:**
- `Cargo.toml` — single-binary crate; deps: clap v4 derive, serde + serde_yaml, serde_json, rusqlite bundled, regex, anyhow, thiserror
- `src/main.rs` — clap Parser with Init + Install (stub) subcommands; catch-all for unknown subcommands
- `src/cli/mod.rs` — mod declaration
- `src/cli/init.rs` — init handler; idempotency logic (full init → "Already initialized"; partial → completes missing files)
- `src/manifest.rs` — Manifest { stores: Vec<InstalledStore> }; atomic save via tmp+rename
- `src/db.rs` — open(path) → Connection with WAL pragma applied
- `src/paths.rs` — stores_dir(), db_path(), manifest_path() from cwd
- `.gitignore` — /target/ and /.stores/

**ACs:**
- [x] `cargo build` succeeds (1 dead_code warning on `Manifest::load()` — unused until Phase 3; not an error)
- [x] `cargo install --path .` installs `/home/blake/.cargo/bin/stores`
- [x] Re-running after code change replaces binary cleanly; `.stores/` dir in unrelated tmp dir untouched
- [x] `stores init` creates `.stores/db.sqlite` (SQLite 3.x, WAL on) and `.stores/manifest.yaml` (content: `stores: []`)
- [x] Re-running `stores init` prints "Already initialized at <path>", exits 0, manifest unchanged

**Deviations:**
- `src/main.rs` uses `clap::CommandFactory` import (required to call `Cli::command()` for help printing) — not called out in the plan but standard clap v4 pattern, no behavioral change.
- `Commands::Install` stub added (phase 3 will implement) so `stores install` gives a clear "not yet implemented" error instead of "unknown subcommand".
- The dead_code warning on `Manifest::load()` is expected; Phase 3 will consume it.

**Commits:** `6bcfc08` feat(T001 phase 1): cargo scaffold + stores init

---

### Phase 2: Schema parser

- **Status:** COMPLETE
- **Started:** 2026-04-26
- **Completed:** 2026-04-26

**Files Created:**
- `src/schema/mod.rs` — `Schema`, `Field`, `FieldType` (with custom Deserialize); `Schema::from_yaml`; full unit test suite
- `src/schema/actor.rs` — `Actor` enum; custom `Deserialize`; `from_env()`
- `src/schema/required_when.rs` — `Expr { lhs_path, rhs_literal }`; hand-rolled parser rejecting `!=`, `&&`, `||`, `OR`, double-quoted RHS
- `src/schema/lifecycle.rs` — `Lifecycle { states, initial_state, transitions }`; `resolved_initial_state()` defaulting to `states[0]`
- `src/schema/flatten.rs` — `LeafArg`, `leaf_args(schema)`; kebab-case leaf names; uniqueness check naming both paths on collision
- `src/schema/parse.rs` — re-export shim + error-message test
- `src/schema/types.rs` — re-export shim (FieldType lives in mod.rs to avoid circular dep)
- `src/id_format.rs` — `validate(template)`; accepts exactly one `{:0Nd}` placeholder

**Files Modified:**
- `src/main.rs` — added `pub mod schema;` and `pub mod id_format;`

**ACs:**
- [x] AC1: Unit tests parse YAML covering every FieldType, required_when, enum, actor, transition
- [x] AC2: Record `contract` sub-field `done_when` carries required_when on sub-Field, not parent
- [x] AC3: `"triage.verdict == 'T3'"` → `Expr { lhs_path: ["triage","verdict"], rhs_literal: "T3" }`
- [x] AC4: Malformed required_when (`a != b`, `a == b OR c == d`) returns error naming unsupported token
- [x] AC5: Unknown field type / actor returns error pointing at offending key
- [x] AC6: `leaf_args` for `triage{verdict,notes}` + `contract{done_when,scope_in,scope_out}` → 5 leaves with correct cli_names
- [x] AC7: `leaf_args` returns error naming both parent paths on collision
- [x] AC8: `id_format: "L{:03d}"` validates; format-string validation passes
- [x] AC9: `Lifecycle::initial_state` defaults to `states[0]`; explicit value overrides

**Test count:** 31 tests, all pass (dev + release)

**Deviations:**
- `FieldType` defined in `mod.rs` rather than `types.rs` to avoid the circular dependency `FieldType::Record(Vec<Field>)` ↔ `Field`. `types.rs` is a re-export shim. No behavioral change.
- `parse.rs` is a re-export shim; `Schema::from_yaml` lives in `mod.rs` for the same reason. No behavioral change.

**Commits:** `169480f` feat(T001 phase 2): YAML schema parser with Record sub-field flattening

---

### Phase 4: Dynamic CLI codegen + `add`/`show`/`list`/`update` verbs

- **Status:** COMPLETE
- **Started:** 2026-04-26
- **Completed:** 2026-04-26

**Files Created:**
- `src/cli/dynamic.rs` — `build_root(manifest, schemas) -> clap::Command`; builder API (no derive); per store: `Command::new(store.name)` with `add`/`show`/`list`/`update` verbs; per leaf: `Arg::new(cli_name).long(cli_name)` + `--<name>-from-file` companion for Text/Timestamp/DisplayId leaves; `--json` global flag; `is_reserved()` guard prevents collision with clap builtins
- `src/cli/dispatch.rs` — routes parsed `ArgMatches` to handler by store-name match; calls `detect_invoker` (reads `$CLAUDECODE` env var)
- `src/handlers/mod.rs` — declares `add`, `list`, `row`, `show`, `update` submodules
- `src/handlers/row.rs` — `build_entry_map(schema, get_arg)`: reads flat CLI args, nests Record sub-fields back into `entry["record"]["subfield"] = val`; `coerce_value(ty, raw)`: List splits on `|`; `now_iso8601()` UTC formatter; `read_row(schema, conn, display_id)` deserializes JSON columns back to nested `serde_json::Value`
- `src/handlers/add.rs` — builds entry, calls validator stub, inserts row in transaction: INSERT stub → `last_insert_rowid()` → `id_format::render()` → `UPDATE display_id`; prints display_id; 3 unit tests for status/created_*/display_id
- `src/handlers/show.rs` — reads row via `read_row`; prints text (nested) or `--json` (single object)
- `src/handlers/list.rs` — queries all rows; decodes JSON columns; prints text (one line per entry) or `--json` (array)
- `src/handlers/update.rs` — reads existing row, builds diff from args, merges, runs validator stub, executes SET for diff fields + `updated_at`/`updated_by`
- `src/validate/mod.rs` — `type EntryMap = BTreeMap<String, Value>`; `validate(schema, entry, invoker) -> Ok(())` stub
- `src/output.rs` — `print_entry_text`, `print_list_text`, `print_entry_json`, `print_list_json`

**Files Modified:**
- `src/main.rs` — replaced derive `Cli` struct with builder-API entry: load manifest + schemas (appending `/schema.yaml` to stored `schema_path`), call `dynamic::build_root`, parse, dispatch via `match subcommand()`; `init` and `install` work before manifest exists
- `src/cli/mod.rs` — added `dispatch` and `dynamic` module declarations
- `src/schema/actor.rs` — added `Copy` derive to `Actor` (no heap data; enables passing by value without clone noise)
- `Cargo.toml` — added `"string"` to clap features (required for `From<String> for clap::builder::Str`, which is `#[cfg(feature = "string")]` in clap 4.6)

**ACs verified:**
- [x] AC1: `stores kitchen_sink add --title "thing broke" --priority low` → `K001`, exits 0
- [x] AC2: Flat leaf args — `--title`, `--notes`, `--severity` (Record sub-fields) appear as flat flags; no `--details` arg
- [x] AC3: `--tags "alpha|beta|gamma"` → JSON `["alpha","beta","gamma"]` (List split on `|`)
- [x] AC4: `stores kitchen_sink show K001` prints entry; `--json` emits valid JSON; Record and List columns decoded back to nested structures (`details.notes`, `tags` array)
- [x] AC5: `stores kitchen_sink list` shows all rows; `--json` emits JSON array
- [x] AC6: `--title-from-file /tmp/test_note.txt` loads file content into `--title`
- [x] AC7: `stores kitchen_sink update K001 --title "edited"` mutates row; `updated_at`/`updated_by` bumped; `created_*` unchanged
- [x] AC8: `status = lifecycle.states[0]` on every `add` (unit tested + E2E)
- [x] AC9: `created_at`/`updated_at`/`created_by`/`updated_by` populated on add; `updated_*` bumped on update (unit tested)
- [x] AC10: `stores --help` shows `kitchen_sink`; `stores kitchen_sink --help` shows verbs

**Test count:** 41 tests (3 new in `handlers::add::tests`), all pass

**Deviations:**
- `ArgMatches::contains_id` panics in clap 4.6 for unregistered IDs; used `try_contains_id` instead (returns `Result<bool>`, safer API). No behavioral change.
- `build_write_cmd` split into `build_add_cmd`/`build_update_cmd`/`build_leaf_cmd` to avoid passing `&str` to `Command::new` (which requires `&'static str` without the `string` feature); lifetime issue was pre-existing clap 4.6 constraint. No behavioral change.
- ISO-8601 UTC math duplicated from `install.rs` into `handlers/row.rs` (both are stdlib-only). Minor code smell; deferred to Phase 5 or later.

**Commits:** (pending)

---

## Code Review Log

### Phase 1: Cargo scaffold + `stores init`

- **Gate:** PASS
- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Commit:** `6bcfc08`
- **Issues:** 0 critical / 0 major / 3 minor
- **Summary:** All five Phase 1 ACs verified end-to-end in a fresh tmp dir: `cargo build` clean (one acknowledged `dead_code` warning on `Manifest::load()` — genuine Phase-3-only consumption, not broken design); `cargo install --path .` replaces the binary cleanly without touching `.stores/`; `stores init` creates valid SQLite with WAL persisted in the file header (verified across reopen), and an empty `stores: []` manifest; re-init is idempotent and preserves manifest content even when polluted. Three minor findings: (1) the `args: Vec<String>` catch-all in `main.rs` is throwaway scaffolding that Phase 4 will discard when it switches to the `clap::Command` builder for dynamic subcommand injection — not a clean seam, just expected churn; (2) `db::open` uses `pragma_update` for `journal_mode` which works but silently swallows the return value (defer; file-backed DB exercises it correctly); (3) status messages go to stdout instead of stderr (defer to Phase 4 output-convention work). None gate-blocking. DONE_WHEN #1 fully satisfied. Status: advance to Phase 2.
- → Details: code-review-phase-1.md

### Phase 2: Schema parser (YAML → typed Rust model)

- **Gate:** PASS
- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Commit:** `169480f`
- **Issues:** 0 critical / 1 major / 4 minor
- **Summary:** All 9 Phase 2 ACs verified by re-running `cargo test` (31/31 pass, matches executor's claim). Spot-checks confirm the load-bearing pieces are real, not just test-name-shaped: `Field { ty: FieldType::Record(Vec<Field>) }` literally nests full `Field` structs so sub-fields are first-class with their own `required`/`required_when`/`pattern`/`actor` (C3 model side, fixture asserts `contract.required_when.is_none()` AND `done_when.required_when` carries the sibling-Record path); `flatten::to_kebab` strips parent prefix, producing `--done-when`/`--scope-in`/`--scope-out`/`--verdict` exactly as the marquee DONE_WHEN demo demands (C2 model side); `required_when` parser path-agnostic so cross-Record `lhs_path = ["triage","verdict"]` from inside `contract.done_when` parses cleanly (Phase 5 owns resolution); `Lifecycle.resolved_initial_state()` defaults to `states[0]` and explicit override works; custom `RawFieldType` Visitor cleanly handles both string-form (`type: text`) and map-form (`type: { list: text }`/`{ record: ... }`). The deviation putting `FieldType` + `Schema::from_yaml` in `mod.rs` instead of `types.rs`/`parse.rs` is defensible (genuine `FieldType::Record(Vec<Field>)` ↔ `Field` circular-dep) and the shims are tiny. Forward-compat for Phases 3/4/5 is clean — no awkward seams.
  - **1 Major (M1) — `OR`/`AND` substring rejection in `required_when::parse` produces false positives.** `s.contains("OR")` falsely rejects legitimate values like `'NORTH'`, `'AUTHORIZED'`, `'CONNECTOR'`; `s.contains("AND")` falsely rejects `'BRANDY'`. Verified empirically. v0.1 bundled stores (T1/T2/T3, decision/script, human, etc.) don't trigger this so the demo path is unaffected — but the foot-gun is live for any user-authored store. Fix is ≤10 lines (token-aware split or `\bOR\b` regex). **Not gate-blocking;** recommend rolling into Phase 5's `required_when` work block since that phase is already extending the parser surface.
  - **4 Minors:** (m1) AC8 promised "rendering pk=1 → L001" but renderer is Phase 4 territory; only validation tested here — Phase 4 must close. (m2) `RawField` lacks `#[serde(deny_unknown_fields)]` so YAML typos like `requried_when:` parse silently. (m3) `Schema.default_actor` field added without plan mention; unused, possible YAGNI — confirm Phase 5 plan consumes it or drop. (m4) `parse.rs` test has dead `let bad = ...; let _ = bad;` cosmetic leftover. (m5) Reserved-column-name collision (cycle-2 fresh-eye m1c2: a user leaf named `status`/`display_id`/`created_at` etc.) is NOT caught here; deferrable to Phase 3 where DDL emission owns the reserved-column list. v0.1 bundled stores don't trigger it (status is store-managed, not declared as a user field).
  - DONE_WHEN: Phase 2 enables, doesn't directly demo. Marquee `triage --verdict T3 --done-when X` enforcement is genuinely supported by this model — verified end-to-end at the type level. Status: advance to Phase 3.
- → Details: code-review-phase-2.md

### Phase 3: `stores install` + DDL codegen + manifest registration

- **Gate:** PASS
- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Commit:** `9469d77`
- **Issues:** 0 critical / 0 major / 4 minor
- **Summary:** All 6 Phase 3 ACs verified. `cargo test` 38/38 passes (matches executor claim). Live E2E in a fresh `mktemp -d`: `stores init` → `stores install …/all_types_store` → `sqlite3 ".schema kitchen_sink"` matches the snapshot byte-for-byte (reserved cols → user scalars → JSON cols, Enum CHECK present, Bool CHECK present, List/Record collapsed to TEXT). Manifest contains canonical absolute `schema_path` + ISO-8601 `installed_at` + correct `name`/`table_name`. Re-install same path → clear path-collision message; copying the fixture under a different folder name then installing → distinguishable name-collision message. Probed `canonicalize` round-trip via `…/foo/../foo` — collapses to the same absolute path so the same-path check fires correctly. Probed install-before-init → clean `run \`stores init\` first` error. DDL runs in a SQLite `BEGIN; … COMMIT;` transaction and the manifest write uses tmp+rename — both atomic individually but not jointly atomic with each other; the orphan failure mode (DDL committed, manifest write fails) is benign because `CREATE TABLE IF NOT EXISTS` makes a retry self-healing. Forward-compat for Phase 4/5 is clean — reserved cols are pre-populated for `created_by`/`updated_by` from the validator's invoker; the table layout (reserved → scalars → JSON) maps naturally to `add`/`show`/`list`/`update`. The `kitchen_sink` fixture exercises all 8 `FieldType` variants; the Record sub-field carries `required_when` for Phase 5's C3 unit test (LHS is a non-Record path; the cross-Record case will be exercised by the real `observations` schema in Phase 6).
  - **4 Minors:** (m1) DDL execute and manifest save are not jointly atomic — orphan state is self-healing via `IF NOT EXISTS` but undocumented; recommend a comment in `install::run`. (m2) Reserved-column-name collision (user declares a field named `status`/`id`/`created_at` etc.) surfaces only as SQLite's own `duplicate column name` error — Phase 2's m5 deferred this here, Phase 3 didn't pick it up. Recommend rolling into Phase 4 alongside the leaf-name vs CLI-flag collision check. (m3) `Schema.default_actor` still unused (carried from Phase 2 m3); confirm Phase 5 wires it or drop. (m4) `chrono_now` hand-rolls UTC calendar arithmetic to avoid a chrono dep — correct + tested but Phase 4 will need timestamps everywhere; recommend pulling in `time` (smaller than chrono) at that point and deleting the hand-rolled code.
  - DONE_WHEN: #2 fully (install path-side); #3 partially (multi-store coexistence empirically demonstrated by installing the same fixture into the same DB under a renamed copy; full verification with `gate` is Phase 7's job). Status: advance to Phase 4.
- → Details: code-review-phase-3.md

### Phase 4: Dynamic CLI codegen + add/show/list/update verbs

- **Gate:** REVISE (minor — one major data-loss bug + several minors; fix is small)
- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Commit:** `8beeb67`
- **Issues:** 0 critical / 1 major / 5 minor
- **Summary:** All 10 Phase 4 ACs verified end-to-end against the `kitchen_sink` fixture in a fresh tmp dir. `cargo test` 41/41 passes (matches executor claim — three new `handlers::add::tests::*` cover initial-status, created/updated population, and display_id render-from-rowid). Live E2E: `add` with flat leaves (`--title`, `--priority`, `--notes`, `--severity`, `--tags "a|b|c"`) writes `K001`; `show K001` text formatter renders Records nested under `details:` with sub-keys indented; `--json` emits the Record as a real nested object (`"details": {"notes": "...", "severity": "..."}`) and List as a real JSON array (`"tags": ["a","b","c"]`) — not escaped strings. `list` works for both text and `--json`. `--title-from-file /tmp/blob.txt` and `echo X | stores ... --title-from-file -` both populate. `update K001 --title "edited"` bumps `updated_at`/`updated_by` and leaves `created_at`/`created_by` untouched (verified by `sqlite3` query). `status` is `open` on every add (= `lifecycle.states[0]`). `stores --help` lists `kitchen_sink`; `stores kitchen_sink --help` lists the four verbs; `stores kitchen_sink add --help` shows the flat leaf args including the Record sub-fields `--notes` / `--severity` (no `--details` JSON arg). The reassembly of flat leaves into a nested `EntryMap` (`row::build_entry_map`) is correct — when a Record-bearing fixture is loaded, the validator-input map has `entry["details"]["severity"]` shape, not `entry["severity"]`, so Phase 5's `required_when` evaluator will get what it needs. Empty-manifest case (`stores --help` before any install) works — only `init`/`install`/`help` are listed. Both clap-version deviations (`try_contains_id` vs `contains_id` panic in 4.6; `clap "string"` feature for `From<String> for clap::builder::Str`) are defensible — the executor's notes match what the source shows. Reserved-name collision (m1c2 carried) is partially caught: SQLite errors at install with `duplicate column name: status` — not silent, but the dedicated `is_reserved` list in `dynamic.rs` is not mirrored at install time so the error message is SQLite's, not ours.
  - **1 Major (M1) — Record sub-field update silently destroys sibling sub-fields.** `handlers::update::run` builds a `diff` EntryMap from the args provided, then `merged.insert(k, v)` in the merge loop **replaces** the entire `details` Object with whatever sub-keys the diff has. Concretely reproduced: starting from `details = {"notes":"X","severity":"Y"}`, running `stores kitchen_sink update K001 --severity warning` writes `details = {"severity":"warning"}` to the DB — `notes` is gone. This is straight data loss on every partial Record update. **Direct impact on the demo path:** Phase 6 needs `stores observations triage L001 --verdict T3 --done-when "..." --scope-in "..." --scope-out "..."` to write the contract Record, then later (e.g.) `update --done-when "revised"` to refine it — under this bug, the second update wipes `scope_in` and `scope_out` from the contract. Also affects `triage`'s implicit pattern (a transition will likely route through the same diff path in Phase 6). **Fix:** when building the diff for a Record-typed top-level field, deep-merge sub-keys into the existing entry's Object instead of replacing the Object outright. The fix is local to `update.rs` lines 42–46 (~10 LOC); the read of `existing` already has the full nested Record so the data is on hand.
  - **5 Minors:** (m1) `update` silently coerces unparseable Integer args to `0` instead of NULL or rejecting them — `add` at least stores NULL; `update` falls through `Value::String(raw)` to `_ => 0` at update.rs:84-90. Tracker for Phase 5 — the validator should reject the unparseable value before this branch fires. (m2) `details` shows up in the text formatter as `details:` with sub-fields indented but with **no visible separator** between the parent line and the children — fine for now, mention if the README starts asserting layout. (m3) ISO-8601 timestamp math is duplicated between `install.rs::chrono_now` and `handlers::row::now_iso8601` — Phase 3 m4 carried this; recommend extracting to `paths::now_iso8601()` or pulling in `time` crate. (m4) `dispatch.rs::detect_invoker` ignores `--invoker` flag — the function comment acknowledges this ("not a clap arg yet; Phase 6+ adds it"); ensure Phase 6 wires it before any actor-mismatch demo runs. (m5) `coerce_value` for List splits on `|` with no escape; cycle-2 m2c2 already noted this — defer. Reserved-column-name install-time check (cycle-2 m1c2) is partially mitigated by SQLite's own duplicate-column error; recommend mirroring `is_reserved` into install-time check in a future phase but not gate-blocking.
  - **Forward-compat notes:**
    - **Phase 5 (validator):** signature `validate(&Schema, &EntryMap, Actor) -> Result<()>` is stable; the `EntryMap` is genuinely nested (verified empirically — Records reassemble correctly from flat CLI input); cross-Record `required_when` evaluation has the data shape it needs. **However** Phase 5 should consider whether `Op::Add | Update | Transition(verb)` belongs in the signature (the plan called for it in Phase 5; current stub elides). Recommend Phase 5 add the `Op` parameter at the same time the body lands.
    - **Phase 6 (lifecycle transitions):** `dynamic.rs::build_store_command` hardcodes the four base verbs — adding per-transition verbs is a clean extension via another loop after `add_cmd/update_cmd/show_cmd/list_cmd` are added. The transition handler will share most of `update`'s shape (read existing → merge diff → validate → write) and **must inherit the M1 fix** when it's made. Single seam; no rework needed.
  - **DONE_WHEN:** Phase 4 contributes #4 (`add` returns L001-shaped display_id), #7 (`show` with nested Records), #8 (`list`); sets up #6 (flat leaf args for `triage --verdict T3 --done-when X --scope-in Y --scope-out Z` — Phase 6 wires the lifecycle transition). All four are demonstrably present at this layer; M1 puts #6 at risk for any subsequent partial-Record update but does not block #4/#7/#8.
  - **Status:** Stay at `EXECUTING_PHASE_4` until M1 is fixed. Action items: (1) fix Record sub-field merge in `update.rs`; (2) add a unit test that explicitly asserts `update --severity X` preserves sibling `notes` in `details`; (3) recommit, then re-review.
- → Details: code-review-phase-4.md

**Revise cycle 1 (2026-04-26):**
- **Bug fixed (M1):** `src/handlers/update.rs` lines 42–46. Merge loop now deep-merges Record-typed fields: when both `existing[k]` and `diff[k]` are `Value::Object`, sub-keys are merged (existing sub-keys preserved unless the diff provides them). For all other types the existing replace-wholesale behaviour is unchanged. SQL write for Record fields was also updated to serialize the merged value (not the partial diff Object). Net change: +16 LOC to merge block + +1 LOC for `FieldType::Record` split in the SQL writer.
- **Regression test added:** `handlers::update::tests::update_record_subfield_preserves_siblings` — builds in-memory `rstore` schema with `details { notes: text, severity: text }`, INSERTs row with both sub-fields, UPDATEs only `--severity`, asserts `details.notes == "keep-me"` and `details.severity == "warning"` post-update.
- **Tests:** 42/42 pass (41 pre-existing + 1 new).
- **Live repro verified:** `stores kitchen_sink add --notes "keep-me" --severity "info"` → `K001`; `stores kitchen_sink update K001 --severity "warning"`; `stores kitchen_sink show K001` shows `details.notes: keep-me`, `details.severity: warning`.
- **Commit:** (see git log — fix(T001 phase 4): deep-merge Record updates to preserve sibling sub-fields)

---

### Phase 3: install + DDL

- **Status:** COMPLETE
- **Started:** 2026-04-26
- **Completed:** 2026-04-26

**Files Created:**
- `src/install.rs` — `install::run(path)`: canonical-path resolve → schema parse → leaf_args check → ensure_initialized → manifest collision checks (path + name) → DDL codegen → SQLite execute_batch → manifest append + atomic save → print success
- `src/codegen/mod.rs` — mod declaration
- `src/codegen/ddl.rs` — `ddl_for(&schema) -> String`; reserved columns first, scalars in schema order, JSON columns (List/Record) last; Enum→TEXT+CHECK; Bool→INTEGER+CHECK; List/Record→TEXT (JSON); deterministic output; snapshot test pinned
- `src/paths.rs` — added `ensure_initialized()` helper
- `tests/fixtures/all_types_store/schema.yaml` — `kitchen_sink` fixture covering all 8 FieldType variants (Text, Integer, Bool, Enum, List<Text>, Record with required_when sub-field, DisplayId, Timestamp)

**Files Modified:**
- `src/main.rs` — added `mod codegen; mod install;`; wired `Install { path }` to `install::run(path)`
- `src/paths.rs` — added `ensure_initialized()`

**ACs:**
- [x] AC1: `stores install <path-to-all_types_store>` succeeds; table column list matches DDL snapshot; Enum CHECK present; JSON columns TEXT (38/38 tests pass + E2E verified)
- [x] AC2: Second install into same DB succeeds (E2E confirmed with different path)
- [x] AC3: Re-installing same path rejected: "already installed; v0.1 has no migrations"
- [x] AC4: Same `name:` different path rejected with name-collision error (same error class)
- [x] AC5: DDL is deterministic — snapshot test in `codegen::ddl::tests::ddl_snapshot` pins exact SQL
- [x] AC6: manifest.yaml contains `name`, `schema_path`, `installed_at`, `table_name` after install

**Test count:** 38 tests (7 new), all pass (dev + release)

**Deviations:**
- ISO-8601 UTC timestamp implemented via minimal stdlib-only calendar math (no chrono dep); avoids adding a new dependency for a single formatting call. Correct for dates from 1970 onward; no leap-second handling (out of scope for v0.1).
- `col_defs_for_field` helper elided — logic inlined into `ddl_for` directly; fewer indirection layers, same result.

**Commits:** `9469d77` feat(T001 phase 3): stores install — DDL codegen + manifest registration

---

## Completion
_Final summary when task is complete._

- **Completed:** —
- **Summary:** —
- **Commits:** —
- **Lessons Learned:** —
