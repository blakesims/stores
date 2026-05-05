# T017: Schema migrations on binary upgrade - stores migrate verb (additive-only)

## Meta
- **Status:** accepted
- **Created:** 2026-05-03T10:55:27Z
- **Last Updated:** 2026-05-03T11:57:27Z
- **Current Phase:** 3
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T017-schema-migrations

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** New CLI verb &#x60;stores migrate&#x60; (src/cli/migrate.rs or addition to existing CLI dispatcher); new handler (src/handlers/migrate.rs); schema-diff logic comparing compiled-in schemas against live DB introspection (PRAGMA table_info on each registered store table); ALTER TABLE ADD COLUMN emission for additive changes; STDERR warning emission for orphaned columns and type mismatches; transaction wrapping for --apply mode; unit + integration tests covering all (7)(a)-(e) scenarios; README docs section.
- **Out:** Destructive migrations (column drops, type coercions, table renames) — out of scope; backup-and-recreate strategy is a follow-up task. Migration history table — out of scope; additive migrations are deterministic from schema diff. Auto-on-startup migration — explicit verb only for safety. L010 (cargo install on accept) — separate task; once L010 ships, the daemon subscriber can chain &#x60;stores migrate --apply&#x60; after install. Multi-DB / multi-host coordination — out of scope; single-DB only. Schema-defined indexes / constraints / triggers — out of scope unless trivially additive (default: skip). Auth UX cluster (L013/L014/L015), L020 stale dirs, L021 wrap_log render, L033 brief plumbing (separate parallel task).

### Done When
(1) New verb &#x60;stores migrate&#x60; that diffs the live &#x60;.stores/db.sqlite&#x60; schema against the substrate&#x27;s compiled-in schema.yaml definitions for every registered store (tasks, observations, gate, and any future workflow-shaped stores) and emits SQL migration statements for ADDITIVE changes (ADD COLUMN).

(2) Default mode is DRY-RUN: &#x60;stores migrate&#x60; prints the SQL it would execute and exits 0 with no DB changes. Operator opts in to mutation via &#x60;stores migrate --apply&#x60;, which executes the SQL inside a transaction.

(3) Diff logic: for each registered store, compare the live table&#x27;s columns against the schema&#x27;s expected fields. Columns in schema.yaml but not in DB → emit &#x60;ALTER TABLE &lt;store&gt; ADD COLUMN &lt;name&gt; &lt;type&gt;&#x60; with NULL default. Columns in DB but not in schema.yaml → emit a STDERR warning naming the column (&quot;orphaned column; not auto-dropped&quot;) and skip — destructive operations are out of scope.

(4) Type-change detection: if a column exists in both DB and schema.yaml but with different types (e.g. TEXT in DB, INTEGER in schema), emit a STDERR warning and skip; do not auto-coerce.

(5) Idempotency: running &#x60;stores migrate&#x60; against an already-in-sync DB is a clean no-op (exit 0, no SQL emitted, no warnings). Running &#x60;stores migrate --apply&#x60; twice in a row produces the same result as running it once.

(6) Audit: each applied migration prints the SQL to stdout. No persistent migrations history table yet (additive-only migrations are deterministic from the schema diff; track later if/when destructive migrations land).

(7) Tests cover: (a) stale DB with one missing column → dry-run prints ADD COLUMN, --apply executes it, post-apply re-run is no-op; (b) DB ahead of schema (orphaned column) → warning emitted, no SQL; (c) type mismatch → warning emitted, no SQL; (d) multiple stores migrated in one invocation; (e) transaction rollback on partial failure during --apply.

(8) Docs: README section &quot;Schema migrations&quot; with example flow (cargo install + stores migrate --apply); brief runbook noting &quot;run stores migrate --apply after every cargo install / binary upgrade&quot; until the L010 daemon-subscriber automates it.

### Phases

#### Phase 1: Phase 1: Expected-columns helper in codegen
- **Objective:** Expose a deterministic &#x60;expected_columns(schema)&#x60; function that returns the same column set ddl_for produces, so migrate.rs can compute diffs without re-implementing column logic.
- **Tasks:**
  - Task 1.1: In src/codegen/ddl.rs, extract a public function &#x60;expected_columns(schema: &amp;Schema) -&gt; Vec&lt;ExpectedColumn&gt;&#x60; where ExpectedColumn &#x3D; { name: String, sql_type: String, full_def: String, is_reserved: bool }. Reserved entries come from RESERVED_COLUMNS (parse the existing static strings into name+type+full_def). User fields use the same dispatch logic currently in scalar_col_def (Text/Integer/Bool/Timestamp/DisplayId/Enum) and the JSON-blob branch (Record/List/ListRecord/ListFk/Json → TEXT).
  - Task 1.2: Refactor &#x60;ddl_for&#x60; to call &#x60;expected_columns&#x60; and assemble the CREATE TABLE statement from the returned full_def fragments. Verify via the existing &#x60;ddl_snapshot&#x60; test that output is byte-identical.
  - Task 1.3: Add unit tests for &#x60;expected_columns&#x60;: (a) all reserved columns present in correct order with sql_type populated; (b) Text field → sql_type&#x3D;&quot;TEXT&quot;, full_def&#x3D;&quot;&lt;name&gt; TEXT&quot;; (c) Bool field → sql_type&#x3D;&quot;INTEGER&quot;, full_def includes CHECK clause; (d) Enum field → sql_type&#x3D;&quot;TEXT&quot;, full_def includes CHECK; (e) JSON-blob fields → sql_type&#x3D;&quot;TEXT&quot;, no CHECK.
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds.
  - [ ] AC1.2: Existing &#x60;ddl_snapshot&#x60; test (codegen::ddl::tests::ddl_snapshot) still passes — DDL output byte-identical.
  - [ ] AC1.3: New tests &#x60;expected_columns_*&#x60; pass (≥5 cases listed in Task 1.3).
  - [ ] AC1.4: &#x60;expected_columns&#x60; is &#x60;pub&#x60; and reachable from src/handlers/migrate.rs as &#x60;crate::codegen::ddl::expected_columns&#x60;.
- **Files:** `src/codegen/ddl.rs`, `src/codegen/mod.rs`
#### Phase 2: Phase 2: Diff engine + migrate handler + CLI wiring
- **Objective:** Implement &#x60;stores migrate&#x60; end-to-end: schema-vs-DB diff, SQL emission, STDERR warnings, --apply with one global transaction, idempotency.
- **Tasks:**
  - Task 2.1: Create src/handlers/migrate.rs with &#x60;pub struct MigrationPlan { pub additive: Vec&lt;(String /*store*/, ExpectedColumn)&gt;, pub orphaned: Vec&lt;(String, String /*col*/)&gt;, pub type_mismatches: Vec&lt;(String, String /*col*/, String /*db_type*/, String /*expected_type*/)&gt; }&#x60; and a function &#x60;pub fn compute_plan(conn: &amp;Connection, schemas: &amp;HashMap&lt;String, Schema&gt;, manifest: &amp;Manifest) -&gt; Result&lt;MigrationPlan&gt;&#x60;.
  - Task 2.2: For each installed store, run &#x60;PRAGMA table_info(&lt;quoted_table&gt;)&#x60; and collect (name, type) pairs. Compare against &#x60;expected_columns(schema)&#x60; by name (case-sensitive); classify into additive / orphaned / type_mismatch. Type comparison: case-insensitive equality on the bare sql_type token (e.g. &quot;TEXT&quot; &#x3D;&#x3D; &quot;text&quot;); ignore CHECK clauses. Skip reserved-column comparison if any reserved column is missing — instead surface a hard error &quot;corrupt schema for store &#x27;&lt;name&gt;&#x27;: reserved column &#x27;&lt;col&gt;&#x27; is absent; cannot auto-recover&quot;.
  - Task 2.3: Add &#x60;pub fn run_migrate(apply: bool) -&gt; Result&lt;()&gt;&#x60; in migrate.rs. Loads manifest+schemas (mirror main.rs&#x27;s load logic — extract a small helper if convenient), opens DB, calls compute_plan. For each additive entry, prints &#x60;ALTER TABLE &lt;quoted&gt; ADD COLUMN &lt;full_def&gt;;&#x60; to stdout. For each orphaned: &#x60;eprintln!(&quot;warning: store &#x27;{store}&#x27;: orphaned column &#x27;{col}&#x27; present in DB but not in schema; not auto-dropped (additive-only)&quot;);&#x60;. For each type_mismatch: &#x60;eprintln!(&quot;warning: store &#x27;{store}&#x27;: column &#x27;{col}&#x27; type mismatch — DB has &#x27;{db_type}&#x27;, schema expects &#x27;{expected_type}&#x27;; not auto-coerced (additive-only)&quot;);&#x60;. If &#x60;apply&#x60; is true, wrap all ALTER TABLE statements in a single &#x60;BEGIN; ... COMMIT;&#x60; execute_batch (rollback on any failure). Idempotent: if &#x60;additive&#x60; is empty, emit no SQL and exit 0.
  - Task 2.4: Register &#x60;migrate&#x60; as a top-level subcommand in src/cli/dynamic.rs &#x60;build_root&#x60; (alongside &#x60;init&#x60;, &#x60;install&#x60;, &#x60;setup&#x60;): &#x60;Command::new(&quot;migrate&quot;).about(&quot;Diff installed-store schemas against the live DB and emit additive ALTER TABLE statements (DRY-RUN by default)&quot;).arg(Arg::new(&quot;apply&quot;).long(&quot;apply&quot;).action(ArgAction::SetTrue).help(&quot;Execute the emitted SQL inside a transaction (default is DRY-RUN)&quot;))&#x60;.
  - Task 2.5: Dispatch in src/main.rs: add &#x60;Some((&quot;migrate&quot;, sub)) &#x3D;&gt; { handlers::migrate::run_migrate(sub.get_flag(&quot;apply&quot;))?; }&#x60; arm before the store-subcommand branch. Register &#x60;&quot;migrate&quot;&#x60; so it does not fall into the &quot;unknown subcommand&quot; branch.
  - Task 2.6: Add migrate to src/handlers/mod.rs (&#x60;pub mod migrate;&#x60;).
  - Task 2.7: Unit tests in migrate.rs using in-memory connections + bundled schemas (observations, gate, tasks): (a) compute_plan against an in-sync DB returns empty plan; (b) drop a column from CREATE TABLE before installation, then compute_plan reports it as additive; (c) introduce an orphaned column via raw ALTER, compute_plan reports it as orphaned; (d) introduce a column with mismatched type, compute_plan reports type_mismatch.
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;cargo build&#x60; succeeds.
  - [ ] AC2.2: &#x60;stores migrate --help&#x60; lists &#x60;--apply&#x60; and the about text.
  - [ ] AC2.3: All new unit tests in handlers::migrate pass (≥4 cases per Task 2.7).
  - [ ] AC2.4: With a freshly &#x60;stores setup&#x60;&#x27;d DB, &#x60;stores migrate&#x60; exits 0, prints nothing to stdout, prints nothing to stderr.
  - [ ] AC2.5: With &#x60;stores setup&#x60; followed by manually dropping (via sqlite3) a non-reserved scalar column from one store, &#x60;stores migrate&#x60; prints exactly one &#x60;ALTER TABLE ... ADD COLUMN ...;&#x60; line and exits 0; &#x60;stores migrate --apply&#x60; then executes it and a re-run is silent (idempotent).
  - [ ] AC2.6: Multi-store: with two stores each missing one column, &#x60;stores migrate&#x60; prints two ALTER statements (one per store), and &#x60;--apply&#x60; succeeds atomically.
  - [ ] AC2.7: Orphaned column: a table with an extra column not in schema causes &#x60;stores migrate&#x60; to emit the matching warning to stderr, no stdout SQL, exit 0.
  - [ ] AC2.8: Type mismatch: a column with a differing type produces the documented stderr warning, no stdout SQL, exit 0.
- **Files:** `src/handlers/migrate.rs`, `src/handlers/mod.rs`, `src/cli/dynamic.rs`, `src/main.rs`
- **Dependencies:** Phase 1: expected_columns must be available.
#### Phase 3: Phase 3: Integration tests, transaction-rollback test, README docs
- **Objective:** Add an end-to-end shell test mirroring tasks_e2e.sh patterns to cover the (7)(a)-(e) scenarios from the contract, prove rollback behaviour, and document the verb.
- **Tasks:**
  - Task 3.1: Create tests/migrate_e2e.sh modelled on tests/e2e.sh: (a) stale DB scenario — install bundled stores, drop a known column with sqlite3, run &#x60;stores migrate&#x60; and assert ALTER TABLE appears in stdout; run &#x60;stores migrate --apply&#x60;; assert column now exists via PRAGMA; re-run &#x60;stores migrate&#x60; and assert no output. (b) orphaned-column scenario — &#x60;ALTER TABLE observations ADD COLUMN foo_orphan TEXT&#x60;; assert &#x60;stores migrate&#x60; emits the orphan warning to stderr, no SQL on stdout, exit 0. (c) type-mismatch scenario — install a store, drop and re-create one column with a different type via temp-table swap; assert &#x60;stores migrate&#x60; emits the type-mismatch warning, no SQL on stdout, exit 0. (d) multi-store scenario — drop one column from observations and one from tasks, assert both ALTER statements emitted in one invocation. (e) rollback scenario — temporarily inject a column whose ALTER TABLE will fail (e.g. duplicate the ADD by running ALTER once outside, then &#x60;stores migrate --apply&#x60; — second ADD on the same column fails); assert exit non-zero, the prior already-applied changes (if any) are rolled back (verify by inspecting PRAGMA before and after).
  - Task 3.2: Wire migrate_e2e.sh into the test corpus: reference it from CI/dev script if such a manifest exists (mirror how tasks_e2e.sh is referenced; if no central runner, it stands on its own — match existing convention).
  - Task 3.3: Add a Rust integration test tests/migrate_rollback.rs that creates a temp .stores/, installs bundled stores via the public &#x60;install::run&#x60; path, manually mutates the DB to a partial-failure state, invokes &#x60;migrate::run_migrate(true)&#x60;, and asserts: (i) returns Err; (ii) the DB state is unchanged (no partial column added).
  - Task 3.4: Add a &#x60;## Schema migrations&#x60; section to README.md (after &#x60;## Install (manual)&#x60;, before &#x60;## Manual workflow walk-through&#x60;). Content: (i) when to run (&quot;after every cargo install / binary upgrade until the L010 daemon-subscriber automates it&quot;); (ii) example flow with three commands (&#x60;cargo install --path .&#x60;, &#x60;stores migrate&#x60; to preview, &#x60;stores migrate --apply&#x60; to execute); (iii) the additive-only contract (drops/type changes warn but do not act); (iv) the idempotency guarantee.
- **Acceptance Criteria:**
  - [ ] AC3.1: &#x60;bash tests/migrate_e2e.sh&#x60; exits 0 and prints PASS for each of scenarios (a)-(e).
  - [ ] AC3.2: &#x60;cargo test --test migrate_rollback&#x60; passes; rollback test asserts both Err return and unchanged PRAGMA output.
  - [ ] AC3.3: README.md contains a &#x60;## Schema migrations&#x60; heading at the documented position; the section names &#x60;stores migrate&#x60; and &#x60;stores migrate --apply&#x60; and states the additive-only and idempotency guarantees.
  - [ ] AC3.4: &#x60;cargo build&#x60; and &#x60;cargo test&#x60; (full workspace) both pass.
- **Files:** `tests/migrate_e2e.sh`, `tests/migrate_rollback.rs`, `README.md`
- **Dependencies:** Phase 2: handler and CLI verb implemented.

---

## Plan Review

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable. Phase 1 extracts expected_columns with a byte-identical DDL guard; Phase 2 wires diff + handler + CLI verb with mechanical ACs (--help text, exit codes, PRAGMA-verifiable outcomes); Phase 3 covers contract scenarios (a)-(e) plus README. File coverage and ordering check out against the live codebase (ddl.rs, dynamic.rs build_root, install::run).
- **Open Questions:**
  - Heads-up for executor: rollback tests (Task 3.1e and migrate_rollback.rs) must set up ≥2 pending ALTERs where a later one fails, otherwise there is nothing to roll back — a single failing ALTER leaves the DB unchanged trivially and does not exercise the BEGIN/COMMIT semantics.
- **At:** 2026-05-03T10:59:11Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Extracted pub expected_columns(schema) -&gt; Vec&lt;ExpectedColumn&gt; in src/codegen/ddl.rs (ExpectedColumn &#x3D; {name, sql_type, full_def, is_reserved}). Reserved entries parsed from RESERVED_COLUMNS via splitn(&#x27; &#x27;); user fields reuse scalar_col_def for full_def with sql_type derived as INTEGER for Integer/Bool, TEXT otherwise; JSON-blob fields → TEXT. ddl_for now assembles CREATE TABLE from expected_columns; ddl_snapshot remains byte-identical. Added 5 expected_columns_* tests (reserved order+types, text, bool CHECK, enum CHECK, json no-CHECK). All 16 codegen::ddl tests pass; cargo build clean; full suite shows one pre-existing unrelated graphviz/utf8 test failure (tests/topology_dot_render.rs:ac3_1) untouched by this phase. src/codegen/mod.rs unchanged — expected_columns is reachable as crate::codegen::ddl::expected_columns since the ddl module is already pub.
- **Commit:** `9d41eab`
- **Files:**
  - `src/codegen/ddl.rs`
- **At:** 2026-05-03T11:00:58Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Implemented Phase 2: src/handlers/migrate.rs with MigrationPlan { additive, orphaned, type_mismatches } and pub fns compute_plan + run_migrate. compute_plan diffs every installed store via PRAGMA table_info against expected_columns; missing reserved column is a hard error. run_migrate prints ALTER TABLE statements to stdout (DRY-RUN), emits stderr warnings for orphaned/mismatched columns, and with --apply wraps all ALTERs in one BEGIN/COMMIT. Wired migrate as top-level subcommand in cli/dynamic.rs and dispatched in main.rs. Six unit tests pass (in-sync no-op, missing column → additive, orphaned column, type mismatch, missing reserved column hard error, type_eq case-insensitive). End-to-end verified all ACs (2.1–2.8) against a real &#x60;stores setup&#x60; DB. Pre-existing topology_dot_render test failure is environmental (graphviz) and unrelated.
- **Commit:** `4e10ace`
- **Files:**
  - `src/handlers/migrate.rs`
  - `src/handlers/mod.rs`
  - `src/cli/dynamic.rs`
  - `src/main.rs`
- **At:** 2026-05-03T11:05:18Z
### Phase 3 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 3 complete. tests/migrate_e2e.sh covers contract clauses (a) stale DB + --apply + idempotent re-run, (b) orphaned-column stderr warning, (c) type-mismatch stderr warning, (d) multi-store ALTER in one invocation, (e) rollback engineered via case-collision (pre-add SOURCE; ALTER source fails as duplicate; verify PRAGMA before&#x3D;&#x3D;after). tests/migrate_rollback.rs is the Rust analogue using public install::run + migrate::run_migrate(true), asserts Err and unchanged columns. README adds &#x27;Schema migrations&#x27; section between &#x27;Install (manual)&#x27; and &#x27;Manual workflow walk-through&#x27; covering when-to-run, dry-run/--apply flow, additive-only contract, idempotency guarantee. cargo build clean; cargo test --test migrate_rollback passes; bash tests/migrate_e2e.sh prints PASS for all 5 scenarios. Pre-existing tests/topology_dot_render.rs failure (local graphviz lacks utf8 format) is environmental, reproduces on HEAD before this phase, unrelated to T017.
- **Commit:** `3380b2f`
- **Files:**
  - `tests/migrate_e2e.sh`
  - `tests/migrate_rollback.rs`
  - `README.md`
- **At:** 2026-05-03T11:52:36Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. AC1.1 cargo build clean; AC1.2 ddl_snapshot still byte-identical; AC1.3 all 5 new expected_columns_* tests pass (16/16 in codegen::ddl); AC1.4 expected_columns is pub and reachable as crate::codegen::ddl::expected_columns (lib.rs: pub mod codegen; codegen/mod.rs: pub mod ddl). Extraction is faithful — ddl_for now consumes expected_columns, eliminating column-logic duplication for Phase 2 migrate.rs. Three minor observations documented.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Duplicated type-derivation logic in expected_columns.
File: src/codegen/ddl.rs:127-131
Evidence: sql_type is derived by a second match on FieldType (Integer/Bool → INTEGER, _ → TEXT) parallel to the embedded type strings produced by scalar_col_def. If a future FieldType variant has INTEGER affinity but is added only to scalar_col_def, the parallel match will silently default it to TEXT.
Suggestion: Either parse sql_type out of full_def (split on first whitespace after name), or have scalar_col_def return a small struct (sql_type, full_def) so there&#x27;s one source of truth. Not blocking for Phase 1.

[MINOR] parse_reserved uses splitn(3, &#x27; &#x27;) but only consumes the first two parts.
File: src/codegen/ddl.rs:83
Evidence: &#x60;let mut parts &#x3D; def.splitn(3, &#x27; &#x27;);&#x60; then only &#x60;parts.next()&#x60; is called twice. The third element (e.g. &quot;PRIMARY KEY AUTOINCREMENT&quot;) is dropped — fine because full_def retains the original string, but the splitn(3,...) is misleading; splitn(2,...) would express intent more clearly.
Suggestion: change to &#x60;def.splitn(2, &#x27; &#x27;)&#x60; (still safe — second next() yields the rest, which we discard anyway since we only want the type token; arguably split_whitespace().take(2) is even clearer).

[MINOR] sql_type for reserved &#x27;id&#x27; is just &quot;INTEGER&quot; — does not capture PRIMARY KEY constraint.
File: src/codegen/ddl.rs:82-92
Evidence: parse_reserved returns sql_type&#x3D;&quot;INTEGER&quot; for &#x60;id INTEGER PRIMARY KEY AUTOINCREMENT&#x60;. For migrate diff this is correct (column-add comparisons key off type affinity), but a future caller using sql_type to recreate a column would lose the PRIMARY KEY clause. Not a defect for Phase 2 migrate (additive ADD COLUMN never recreates id), but worth a brief doc-comment on ExpectedColumn.sql_type clarifying &quot;affinity-only; constraints live in full_def&quot;.
Suggestion: Add a doc-comment on the sql_type field noting it is the SQLite type token only, with constraints carried in full_def.

[INFORMATIONAL] AC1.3 minimum was &quot;≥5&quot;; exactly 5 added (reserved-order, text, bool-CHECK, enum-CHECK, json-no-CHECK). Meets bar; coverage of Integer/Timestamp/DisplayId/ListFk variants is implicit via reuse of scalar_col_def but not directly asserted on ExpectedColumn — Phase 2 migrate tests will likely cover those paths.
- **At:** 2026-05-03T11:02:00Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 8 ACs verified: cargo build clean, migrate --help present with --apply, 6 unit tests pass (in-sync no-op, missing column→additive, orphaned, type mismatch, missing-reserved hard error, type_eq case-insensitive). compute_plan logic correctly classifies via PRAGMA table_info; run_migrate emits stderr warnings, prints SQL, and wraps --apply in a single BEGIN/COMMIT. 0 critical, 0 major, 3 minor (deferrable).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] AC2.5/2.6 end-to-end --apply path not directly covered by an automated test.
File: src/handlers/migrate.rs:231-358 (tests module)
Evidence: Tests exercise compute_plan thoroughly but no test calls run_migrate / executes the BEGIN+ALTER+COMMIT batch and re-runs to assert idempotence. The executor reports manual verification, which is acceptable per AC2.3 (≥4 unit cases satisfied with 5 substantive tests) but leaves a coverage gap.
Suggestion: Add a future test that uses a temp file-backed Connection, calls compute_plan → builds the same batch run_migrate would, executes it, then re-runs compute_plan and asserts plan.additive.is_empty(). Not blocking — defer to Phase 3 or follow-up.

[MINOR] Output ordering of additive/orphaned entries is HashMap-iteration-dependent (non-deterministic).
File: src/handlers/migrate.rs:64-87
Evidence: live_cols is HashMap&lt;String,String&gt;; iterating it for orphaned classification produces nondeterministic order across runs. expected (Vec) ordering is deterministic so additive is fine, but orphaned ordering may flap and tests comparing &gt;1 orphaned column would be flaky.
Suggestion: Sort plan.orphaned and plan.type_mismatches by (store, column) before returning, or use BTreeMap in read_table_info. Single-orphan test today is unaffected.

[MINOR] MigrationPlan is &#x60;pub&#x60; but only consumed in-crate.
File: src/handlers/migrate.rs:11-18
Evidence: No external caller; pub fields broaden surface unnecessarily.
Suggestion: Downgrade to pub(crate) when integration tests don&#x27;t need it. Not blocking.

[INFORMATIONAL] Pre-existing failure in tests/topology_dot_render.rs is environmental (graphviz &#x60;dot&#x60; rejects &#x60;-Tutf8&#x60;); unrelated to T017.
[INFORMATIONAL] Manifest+schema loader in load_schemas duplicates main.rs logic — acceptable for this phase, but a follow-up could consolidate.
- **At:** 2026-05-03T11:45:51Z

### Phase 3 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. AC3.1 e2e script structure covers all 5 scenarios (a)-(e) correctly with isolated tmpdirs and clear PASS/FAIL output (could not exec the script in this sandbox without binary install permission, but the rollback Rust analogue passes and the script logic is sound). AC3.2 cargo test --test migrate_rollback passes. AC3.3 README.md has &#x27;## Schema migrations&#x27; section between &#x27;Install (manual)&#x27; (line 140) and &#x27;Manual workflow walk-through&#x27; (line 184), names both verbs and states the additive-only + idempotency guarantees. AC3.4 cargo build clean; cargo test full workspace 554 unit tests + migrate_rollback + schemas_validate_fixtures pass; only failure is pre-existing topology_dot_render env issue (graphviz lacks &#x27;utf8&#x27; format on this host) which reproduces on prior commit and is unrelated to T017. 0 critical, 0 major, 3 minor.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] tests/migrate_e2e.sh — &#x60;mk_fresh()&#x60; swallows stderr from &#x60;stores setup&#x60; via &#x60;&gt; /dev/null 2&gt;&amp;1&#x60;; if setup fails the failure surfaces only obliquely (later sqlite3 missing-file). Suggestion: capture and echo on failure, e.g. &#x60;stores setup &gt; /tmp/setup.log 2&gt;&amp;1 || { cat /tmp/setup.log &gt;&amp;2; exit 1; }&#x60;. Non-blocking — diagnosability nit only.

[MINOR] tests/migrate_e2e.sh — trap chain pattern (&#x60;trap &#x27;rm -rf &quot;$A_TMP&quot;&#x27; EXIT&#x60;, then &#x60;trap &#x27;rm -rf &quot;$A_TMP&quot; &quot;$B_TMP&quot;&#x27; EXIT&#x60; …) is correct as written but fragile: each trap fully replaces the prior one, so a typo dropping an earlier var would silently leak its tmpdir. Suggestion: build the cleanup list in a single array and re-register, or use a single &#x60;CLEANUP&#x3D;()&#x60; accumulator + one &#x60;trap&#x60; registered up-front. Non-blocking.

[MINOR] tests/migrate_e2e.sh (e) — fixture-broken precondition only checks &#x60;body&#x60; was actually dropped; does not assert that &#x60;source&#x60; is gone or that uppercase &#x60;SOURCE&#x60; is present. If the DROP COLUMN for &#x60;source&#x60; silently failed (e.g. SQLite version regression), the scenario would degrade to a single-ALTER success and the rollback assertion would not actually be exercised. Suggestion: add &#x60;echo &quot;$PRE_PRAGMA&quot; | grep -q &#x27;|source|&#x27; &amp;&amp; fail &quot;...&quot;&#x60; and a positive grep for &#x60;|SOURCE|&#x60; on the pre-pragma. Non-blocking — current SQLite handles this fine.

[INFORMATIONAL] tests/migrate_rollback.rs uses std::env::set_current_dir which is process-wide, but this binary contains a single test so parallelism is not a risk. Acceptable.

[INFORMATIONAL] Pre-existing tests/topology_dot_render.rs failure verified independent of this phase: the file&#x27;s last touch was b10f58f (T005 P3) / d37a3e2 (cargo fmt); environmental graphviz format limitation, not a T017 regression.
- **At:** 2026-05-03T11:54:26Z

---

## Completion
- **Accepted:** 2026-05-03T11:57:27Z
- **Branch:** feat/T017-schema-migrations

