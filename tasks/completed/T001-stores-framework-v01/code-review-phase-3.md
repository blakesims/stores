# Phase 3 Code Review — `stores install` + DDL codegen + manifest registration

- **Commit:** `9469d77`
- **Reviewed:** 2026-04-26 by `code-reviewer`
- **Verdict:** PASS
- **Issues:** 0 critical / 0 major / 4 minor
- **DONE_WHEN delta:** #2 fully (install path-side); #3 partially (multi-store coexistence proven via second-install path test).

## What I did

1. Read commit diff (`git show 9469d77 --stat` + full diff inspection).
2. Re-read `src/install.rs`, `src/codegen/ddl.rs`, `src/paths.rs`, `src/main.rs`, `src/manifest.rs`, the `kitchen_sink` fixture.
3. Ran `cargo test` → 38/38 pass (matches executor claim).
4. `cargo install --path .` (warm-cache) and ran the prescribed E2E sanity in a fresh `mktemp -d`:
   - `stores init` — created db + manifest cleanly.
   - `stores install …/all_types_store` — succeeded; printed `Installed store 'kitchen_sink' (table: kitchen_sink)`.
   - `sqlite3 ".schema kitchen_sink"` — schema matches snapshot byte-for-byte (reserved cols → user scalars → JSON cols).
   - `sqlite3 ".tables"` — `kitchen_sink` present; only one table per install (no orphan `*_*` child tables, confirming JSON-in-TEXT decision).
   - `cat .stores/manifest.yaml` — `name`, `schema_path` (canonical absolute), `installed_at` (ISO-8601 UTC `Z`), `table_name` all present and well-formed.
   - Re-install same path — rejected with the path-collision message; exit 1.
5. Ran additional probes the prescribed E2E didn't cover:
   - **Install before `init`**: clean `Error: .stores/ is not initialized in '<cwd>'; run `stores init` first` — exit 1.
   - **Name collision (different path, same `name:`)**: copied fixture to `./other_store_dir`, installed → rejected with `a store named 'kitchen_sink' is already installed (from <orig path>); v0.1 has no migrations — store names must be unique`. Distinguishable from the path-collision class (different verb tense, different "from" path printed).
   - **Canonical-path equivalence**: `stores install /…/all_types_store/../all_types_store` (relative round-trip) → `canonicalize()` collapses both to the same absolute path; the path-collision check fires correctly. PASS for question #8.
   - **Reserved-column-name collision** (e.g. user declares `name: status`): NOT caught at codegen; surfaces as SQLite's own `duplicate column name: status` error. See m4 below.

## AC verification

| AC | Verdict | Notes |
|---|---|---|
| 1 — install snapshot match, Enum CHECK, JSON cols TEXT | PASS | `ddl_snapshot` test pins exact SQL; live `.schema` matches; `priority TEXT CHECK (priority IN ('low', 'medium', 'high'))` present. |
| 2 — second install coexists in same DB | PASS (partial — Phase 7 proves observations + gate end-to-end) | Empirically verified by installing a renamed second copy of the same fixture into the same DB; both rows present in `manifest.yaml`, both tables in `.tables`. |
| 3 — re-install same path rejected | PASS | Clear, actionable message; exit 1. |
| 4 — same-name-different-path rejected | PASS | Distinct error class from #3 — names the existing path, not the new one. |
| 5 — DDL deterministic | PASS | `ddl_is_deterministic` test compares two parses of the same fixture; snapshot pinned in `ddl_snapshot`. |
| 6 — manifest entry has `name`/`schema_path`/`installed_at`/`table_name` | PASS | Empirically verified in `cat .stores/manifest.yaml`. |

All six ACs satisfied.

## Findings

### m1 — Manifest write happens AFTER DDL execute; no compensating rollback if manifest save fails

`install::run` runs DDL inside a SQLite `BEGIN; … COMMIT;`, then separately calls `manifest.save()`. If the DDL succeeds and the manifest save fails (disk full, perms, race), the table exists in the DB but the manifest doesn't list it. **Recovery is benign** because `CREATE TABLE IF NOT EXISTS` swallows the duplicate on retry — re-running `stores install <same path>` will simply re-create-no-op the table and write the manifest entry. So the orphan state is self-healing. But:
- The behavior is undocumented.
- If the user instead uninstalls (manually), the manifest will be clean while the table remains. v0.1 has no `uninstall`, so this is theoretical for now.
- If, between the DDL commit and the manifest save, a concurrent process tried to install a different store with the same name, neither would see the other in the manifest. Two-process install is out of scope for v0.1 (single-user CLI), but worth a tracking note.

**Recommendation:** drop a comment in `install::run` noting the ordering and the self-healing property. Optional defer to v0.2: invert the order (write manifest first, then DDL — but then a DDL failure would leave a manifest entry pointing at a non-existent table, which is the worse direction). Current order is the right pick; just document it.

### m2 — Reserved-column-name collision (e.g. user field named `status`, `id`, `display_id`, `created_at`) is NOT caught at codegen time

Verified: a fixture with `name: status` (user field) installs partway, then errors with SQLite's `duplicate column name: status` followed by `Error code 1: SQL error or missing database`. The DDL is rolled back (transaction), so the table doesn't exist post-failure, AND `manifest.save()` never runs (because the `?` propagates), so no orphan manifest entry — the cleanup path is clean. But the error message is unfriendly: the user sees a SQLite internal error, not a "field name `status` is reserved; reserved names are: id, display_id, status, created_at, updated_at, created_by, updated_by" error.

This was flagged in Phase 2's m5 as deferrable to Phase 3; Phase 3 didn't pick it up. Plan mentions it nowhere as an explicit Phase 3 AC, so technically not a missed AC. **Not gate-blocking.** Recommend rolling into Phase 4's CLI-codegen work (where the same collision matters for `--<flag>` names too — a leaf named `status` would emit `--status` which clashes with no built-in, but a leaf named `json` would clash with the top-level `--json` flag from the plan). Whoever picks it up should add a `RESERVED_NAMES` constant in `codegen::ddl` and check it in `install::run` before DDL execute.

### m3 — `default_actor` field on `Schema` still unused (carried from Phase 2 m3)

Re-verified: `grep default_actor src/` shows it's parsed and stored on `Schema` but never read by Phase 3's install path. Still YAGNI until Phase 5 actually wires it into the validator's per-field actor resolution. Not introduced by this commit (carried from Phase 2) — flagging only because it's still dead code at the end of Phase 3, and the longer it sits, the more likely it gets out of sync with whatever Phase 5 actually needs.

### m4 — `chrono_now` reinvents UTC calendar arithmetic to avoid a `chrono` dep

`install::run` calls a hand-rolled `unix_to_ymd_hms` → `days_to_ymd` → `is_leap` ladder to format `installed_at` as ISO-8601. Logic is correct (verified manually for 2026-04-26) and the deviation is acknowledged in the executor's notes. But:
- It duplicates work `chrono` (or `time`, or `jiff`) does in one line.
- Phase 4 will need timestamps for `created_at`/`updated_at` on every insert/update — same code will get re-needed.
- `chrono` adds ~200 KB to the binary; `time` is smaller; both are widely audited.

**Not gate-blocking** — the impl is correct and tested. **Recommend** picking up `time` (smaller than `chrono`) in Phase 4 when the same need recurs across `add`/`update` handlers, and replacing the hand-rolled code at that point. Don't bother in Phase 3.

## Forward-compat notes

- **Phase 4 (`add`/`show`/`list`/`update`):** the table layout (reserved cols → scalars in schema order → JSON cols) makes all four verbs natural. `add` writes to all reserved + user cols in one INSERT; `show`/`list` SELECT * then re-nest JSON cols using the in-memory schema; `update` SET only touches the user cols + bumps `updated_at`/`updated_by`. No friction.
- **Phase 5 validator (`created_by`/`updated_by` from invoker):** the schema reserves those as TEXT columns and Phase 4's plan populates them from `invoker.to_string()`. Clean seam.
- **`kitchen_sink` fixture coverage:** all 8 `FieldType` variants exercised (Text, Integer, Bool, Enum, List<Text>, Record-with-required_when sub-field, DisplayId, Timestamp). The `details.severity` Record sub-field carries `required_when: priority == 'high'` — same shape as the marquee `contract.done_when` ← `triage.verdict == 'T3'` cross-Record case, just with a non-Record LHS. Phase 5 will exercise the cross-Record case via the real `observations` schema; the fixture's job is to give Phase 5 a synthetic store to unit-test against, and it does.
- **Manifest schema_path canonical:** `canonicalize()` returns the same absolute path regardless of how the user spelled the input (`./foo`, `../bar/foo`, `/abs/foo`). Verified empirically. The path-collision check uses `PathBuf` equality on canonical paths, which is correct.

## Verdict

**PASS.** All 6 ACs verified by tests + E2E sanity. DDL is deterministic, snapshot-pinned, and matches reality byte-for-byte. Reserved cols, Enum CHECK, Bool CHECK, JSON-in-TEXT collapse, field ordering all correct. Both error paths (re-install vs name collision) are distinguishable to the user. Canonical-path equivalence handled correctly. Forward-compat for Phase 4/5 is clean.

The 4 minor findings are all deferrable: m1 is a documentation gap on a self-healing failure mode; m2 is the reserved-column-name collision Phase 2's review already flagged for deferral; m3 is dead code carried from Phase 2; m4 is a code-debt note for Phase 4 to address when timestamps recur.

**Status flip:** `EXECUTING_PHASE_4`.
