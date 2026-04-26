# Code Review — T001 Phase 1

- **Phase:** 1 (Cargo scaffold + `stores init`)
- **Commit:** `6bcfc08` (`feat(T001 phase 1): cargo scaffold + stores init`)
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer (Opus 4.7)
- **Verdict:** **PASS** — advance to Phase 2

## Acceptance Criteria — Verification

| AC | Result | Notes |
|----|--------|-------|
| `cargo build` succeeds | PASS | One `dead_code` warning on `Manifest::load()` (acknowledged; Phase 3 consumes it) |
| `cargo install --path .` installs `stores` binary | PASS | Replaced cleanly into `~/.cargo/bin/stores` |
| Re-running `cargo install --path .` after a code change replaces the binary cleanly | PASS | `Replacing /home/blake/.cargo/bin/stores` confirmed; `.stores/` in tmp dir untouched |
| `stores init` creates `.stores/db.sqlite` (valid SQLite, WAL on) and `.stores/manifest.yaml` (empty `stores: []`) | PASS | `file db.sqlite` → SQLite 3.x; `pragma journal_mode` → `wal` (persists across reopen); manifest content `stores: []\n` |
| Re-running `stores init` is idempotent | PASS | Prints "Already initialized at <path>", manifest content unchanged even when polluted with fake entries |

Bonus verified beyond ACs:
- Partial recovery (delete only `manifest.yaml`, re-run → manifest restored, db preserved; same for db).
- WAL mode is persisted in DB header — the connection-drop after `pragma_update` is safe.
- `stores --help` shows clean clap output.

## Findings

### MINOR-1: `args: Vec<String>` catch-all on top-level `Cli` is throwaway scaffolding, not a Phase 4 seam

**File:** `src/main.rs:11-18`, `src/main.rs:39-52`

The `Cli` struct mixes `#[command(subcommand)] command: Option<Commands>` with a `trailing_var_arg` catch-all `args: Vec<String>`, then dispatches "unknown subcommand" via the `None` branch by inspecting `cli.args[0]`. This works for Phase 1, but Phase 4 (`build_root(manifest: &Manifest, schemas: &HashMap<String, Schema>) -> clap::Command`) will entirely replace this with the `clap::Command` builder API to inject store subcommands at runtime. The current derive setup will be discarded.

**Impact:** Wasted ~15 lines of code in Phase 4 (deleted then replaced). No correctness hazard. The executor's "clean seam" claim in the deviations section is optimistic — the seam is actually the `mod cli` boundary, not `Cli` itself.

**Recommendation:** No change required for Phase 1. Phase 4 executor should expect to rewrite `main.rs` using `clap::Command::new()` builder pattern, not extend the derive struct.

### MINOR-2: `db::open` uses `pragma_update` for `journal_mode` — works, but mildly non-idiomatic

**File:** `src/db.rs:7`

`pragma_update(None, "journal_mode", "WAL")` is normally for write-only pragmas. `journal_mode` actually returns the new mode as a row (which is why `sqlite3 .stores/db.sqlite "pragma journal_mode=WAL"` echoes `wal`). rusqlite's `pragma_update` swallows the return value silently here. Empirically it works (verified WAL persists in the header across reopens), but the more idiomatic approach is `conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))` to verify SQLite actually entered WAL (rather than silently fallback to DELETE/MEMORY on e.g. a memory DB).

**Impact:** Phase 1 has no observable bug (it's a file-backed DB). If a future phase opens an in-memory test DB and assumes WAL is on, the silent fallback would hide a bug.

**Recommendation:** Defer. Phase 3+ tests will catch any future regression because they actually exercise the DB.

### MINOR-3: Status messages go to stdout, not stderr

**File:** `src/cli/init.rs:15, 27, 32`

`println!("Created ...")` and `println!("Already initialized ...")` write status to stdout. By Unix convention, status/diagnostic messages belong on stderr; stdout is reserved for machine-readable output (relevant for Phase 4's `--json` flag and Phase 8's e2e test that pipes through `jq`). For `init` specifically there is no JSON output to compete with, but consistency starts here.

**Impact:** None today. If Phase 8's e2e script ever pipes `stores init` output to a parser (it shouldn't, but might via `set -x` debugging), the status text mixes with real output.

**Recommendation:** Defer to Phase 4 when output conventions are finalized (`src/output.rs`). Probably worth a one-line `eprintln!` swap then.

## Defer-Acknowledged: `dead_code` warning on `Manifest::load()`

The executor flagged this in their Execution Log. Verified:

- `Manifest::load()` is implemented at `src/manifest.rs:25-33`.
- Phase 3's `src/install.rs` per the plan: "read `<path>/schema.yaml`, parse, run `leaf_args` uniqueness check, codegen DDL, execute against `.stores/db.sqlite`, **update manifest**". Updating the manifest requires loading it first.
- This is genuinely Phase-3-only consumption; not dead code, just early-arrival code.

Acceptable. Not a finding.

## Forward-Compatibility — Phase 2/3/4 seams

- **Phase 2 (Schema parser):** Adds `src/schema/` tree alongside existing modules. `Cargo.toml` already has `serde_yaml`, `serde`, `serde_json`, `regex`. No conflict.
- **Phase 3 (`stores install`):** Will replace the `Install` stub in `main.rs` with real dispatch to `src/install.rs`. The `InstalledStore { name, schema_path, installed_at, table_name }` struct in `manifest.rs` matches Phase 3's "per-store entry" spec exactly. Clean handoff.
- **Phase 4 (Dynamic CLI codegen):** Will rewrite `main.rs`'s argument parsing from derive-form to builder-form to inject per-store subcommands. The `args: Vec<String>` catch-all is throwaway (see MINOR-1).
- **Phase 5+ (Validator, transitions):** Untouched by Phase 1.

## DONE_WHEN Alignment

Phase 1 contributes step #1 of the 13-step demo path:
> `stores init` — creates `.stores/db.sqlite` and `.stores/manifest.yaml` in cwd.

Verified end-to-end against the literal command. All sub-requirements (valid SQLite, WAL on, empty `stores: []` manifest, idempotent re-run) hold.

## Issue Summary

- **Critical:** 0
- **Major:** 0
- **Minor:** 3 (all deferrable; none gate-blocking)

## Code Quality Assessment

- Idiomatic Rust: yes. `anyhow::Result` at handler boundaries; no `unwrap()` in handlers.
- Error handling: `?` propagation throughout; no panics.
- Atomic manifest save: correct (tmp + rename in same directory).
- Module organization: clean. `paths.rs` / `db.rs` / `manifest.rs` / `cli/` separation is good.
- `.gitignore`: correct (`/target/` and `/.stores/`).
- `Cargo.lock` tracked: correct for binary crates.

## Verdict

**PASS** — advance to Phase 2.

The implementation is correct, idiomatic, and meets all five Phase 1 acceptance criteria. The three minor findings are deferrable nits with no impact on later phases. The `Manifest::load()` dead_code warning is genuine early-arrival, not broken design. Forward seams to Phases 2 and 3 are clean; Phase 4 will discard the `main.rs` derive struct, but that's expected per the plan.
