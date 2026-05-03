#!/usr/bin/env bash
# tests/migrate_e2e.sh — End-to-end scripted trace of `stores migrate`.
#
# Covers T017 contract clauses (7)(a)-(e):
#   (a) stale DB         — drop a column → dry-run prints ALTER → --apply
#                          executes it → re-run is no-op.
#   (b) orphaned column  — DB has an extra column → stderr warning, no SQL,
#                          exit 0.
#   (c) type mismatch    — DB column type differs from schema → stderr
#                          warning, no SQL, exit 0.
#   (d) multi-store      — drop one column from observations and one from
#                          tasks → both ALTER statements emitted in one
#                          invocation.
#   (e) rollback         — engineer a partial-failure (case-collision: DB
#                          already has SOURCE, schema expects source) so the
#                          second ALTER inside the transaction fails →
#                          --apply exits non-zero and the first ALTER (body)
#                          is rolled back (PRAGMA shows no body column).
#
# Usage: bash tests/migrate_e2e.sh
# Requires: stores binary on PATH (cargo install --path .), sqlite3.

set -euo pipefail

unset CLAUDECODE 2>/dev/null || true

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

echo "=== stores migrate e2e ==="
echo "stores binary: $(command -v stores)"

# Each scenario gets its own fresh tmpdir so failures are isolated and
# diagnosable.
mk_fresh() {
    local d
    d=$(mktemp -d /tmp/stores-migrate-e2e-XXXXXX)
    (cd "$d" && git init -q && stores setup > /dev/null 2>&1)
    echo "$d"
}

# ---------------------------------------------------------------------------
# (a) stale DB → ALTER emitted, --apply executes, re-run no-op.
# ---------------------------------------------------------------------------
echo "--- (a) stale DB: drop observations.body"
A_TMP=$(mk_fresh); trap 'rm -rf "$A_TMP"' EXIT
cd "$A_TMP"
sqlite3 .stores/db.sqlite "ALTER TABLE observations DROP COLUMN body;"

DRY_OUT=$(stores migrate 2>/dev/null)
echo "$DRY_OUT" | grep -q 'ALTER TABLE "observations" ADD COLUMN body' \
    || fail "(a) dry-run did not emit ALTER for body; got: $DRY_OUT"
pass "(a) dry-run prints ALTER TABLE for missing body column"

# Confirm dry-run did NOT mutate the DB.
sqlite3 .stores/db.sqlite "PRAGMA table_info(observations);" | grep -q '|body|' \
    && fail "(a) dry-run unexpectedly added body column"
pass "(a) dry-run did not mutate DB"

stores migrate --apply > /dev/null 2>&1 || fail "(a) --apply exited non-zero"
sqlite3 .stores/db.sqlite "PRAGMA table_info(observations);" | grep -q '|body|TEXT|' \
    || fail "(a) --apply did not add body column"
pass "(a) --apply added body column (TEXT)"

RERUN=$(stores migrate 2>&1)
[[ -z "$RERUN" ]] || fail "(a) re-run not a clean no-op; got: $RERUN"
pass "(a) re-run after --apply is a clean no-op (idempotent)"

# ---------------------------------------------------------------------------
# (b) orphaned column → stderr warning, no stdout, exit 0.
# ---------------------------------------------------------------------------
echo "--- (b) orphaned column: observations.foo_orphan"
B_TMP=$(mk_fresh); trap 'rm -rf "$A_TMP" "$B_TMP"' EXIT
cd "$B_TMP"
sqlite3 .stores/db.sqlite "ALTER TABLE observations ADD COLUMN foo_orphan TEXT;"

B_STDOUT=$(stores migrate 2>/dev/null) || fail "(b) migrate exited non-zero"
B_STDERR=$(stores migrate 2>&1 >/dev/null) || fail "(b) migrate exited non-zero"
[[ -z "$B_STDOUT" ]] || fail "(b) stdout not empty: $B_STDOUT"
echo "$B_STDERR" | grep -q "orphaned column 'foo_orphan'" \
    || fail "(b) stderr missing orphan warning; got: $B_STDERR"
pass "(b) orphan warning on stderr, no SQL on stdout, exit 0"

# ---------------------------------------------------------------------------
# (c) type mismatch → stderr warning, no stdout, exit 0.
# ---------------------------------------------------------------------------
echo "--- (c) type mismatch: observations.body (TEXT → INTEGER)"
C_TMP=$(mk_fresh); trap 'rm -rf "$A_TMP" "$B_TMP" "$C_TMP"' EXIT
cd "$C_TMP"
sqlite3 .stores/db.sqlite "ALTER TABLE observations DROP COLUMN body; \
                           ALTER TABLE observations ADD COLUMN body INTEGER;"

C_STDOUT=$(stores migrate 2>/dev/null) || fail "(c) migrate exited non-zero"
C_STDERR=$(stores migrate 2>&1 >/dev/null) || fail "(c) migrate exited non-zero"
[[ -z "$C_STDOUT" ]] || fail "(c) stdout not empty: $C_STDOUT"
echo "$C_STDERR" | grep -q "type mismatch" \
    || fail "(c) stderr missing type-mismatch warning; got: $C_STDERR"
echo "$C_STDERR" | grep -q "'body'" \
    || fail "(c) stderr missing column name 'body'; got: $C_STDERR"
pass "(c) type-mismatch warning on stderr, no SQL on stdout, exit 0"

# ---------------------------------------------------------------------------
# (d) multi-store: drop one column from observations and one from tasks.
# ---------------------------------------------------------------------------
echo "--- (d) multi-store: drop observations.body + tasks.title"
D_TMP=$(mk_fresh); trap 'rm -rf "$A_TMP" "$B_TMP" "$C_TMP" "$D_TMP"' EXIT
cd "$D_TMP"
sqlite3 .stores/db.sqlite "ALTER TABLE observations DROP COLUMN body;"
sqlite3 .stores/db.sqlite "ALTER TABLE tasks DROP COLUMN title;"

D_OUT=$(stores migrate 2>/dev/null)
echo "$D_OUT" | grep -q 'ALTER TABLE "observations" ADD COLUMN body' \
    || fail "(d) missing observations.body ALTER; got: $D_OUT"
echo "$D_OUT" | grep -q 'ALTER TABLE "tasks" ADD COLUMN title' \
    || fail "(d) missing tasks.title ALTER; got: $D_OUT"
pass "(d) one invocation emits ALTER for both stores"

stores migrate --apply > /dev/null 2>&1 || fail "(d) --apply exited non-zero"
sqlite3 .stores/db.sqlite "PRAGMA table_info(observations);" | grep -q '|body|TEXT|' \
    || fail "(d) --apply did not add observations.body"
sqlite3 .stores/db.sqlite "PRAGMA table_info(tasks);" | grep -q '|title|TEXT|' \
    || fail "(d) --apply did not add tasks.title"
pass "(d) --apply added both columns"

# ---------------------------------------------------------------------------
# (e) rollback: engineer a partial-failure via case-collision.
#
# Both `body` and `source` are dropped → compute_plan emits two ALTERs in
# order [body, source]. We then pre-add `SOURCE` (uppercase). compute_plan
# uses case-sensitive HashMap lookup on PRAGMA names so `source` is still
# reported as missing; but SQLite is case-insensitive about column-name
# uniqueness, so the second ALTER (`ADD COLUMN source ...`) fails with
# "duplicate column name". The first ALTER (body) had already executed
# inside the transaction; if BEGIN/COMMIT works, body must be gone after
# the failure (rolled back).
# ---------------------------------------------------------------------------
echo "--- (e) rollback: pre-collide on 'source' so 2nd ALTER fails"
E_TMP=$(mk_fresh); trap 'rm -rf "$A_TMP" "$B_TMP" "$C_TMP" "$D_TMP" "$E_TMP"' EXIT
cd "$E_TMP"
sqlite3 .stores/db.sqlite "ALTER TABLE observations DROP COLUMN body; \
                           ALTER TABLE observations DROP COLUMN source; \
                           ALTER TABLE observations ADD COLUMN SOURCE TEXT;"

# Capture pre-apply state for diff assertion.
PRE_PRAGMA=$(sqlite3 .stores/db.sqlite "PRAGMA table_info(observations);")
echo "$PRE_PRAGMA" | grep -q '|body|' \
    && fail "(e) pre-state already has body — fixture broken"

# --apply must exit non-zero.
set +e
stores migrate --apply > /dev/null 2>&1
APPLY_RC=$?
set -e
[[ "$APPLY_RC" -ne 0 ]] || fail "(e) --apply exited 0 (expected non-zero on partial failure)"
pass "(e) --apply exited non-zero ($APPLY_RC) on partial failure"

# Post-state must equal pre-state (transaction rolled back).
POST_PRAGMA=$(sqlite3 .stores/db.sqlite "PRAGMA table_info(observations);")
[[ "$PRE_PRAGMA" == "$POST_PRAGMA" ]] \
    || fail "(e) PRAGMA changed across failed --apply; rollback did not restore state"
pass "(e) PRAGMA before == after (transaction rolled back; body NOT partially applied)"

echo ""
echo "=== All migrate e2e scenarios verified ==="
echo "  (a) stale DB → ALTER emitted, --apply executes, re-run no-op:    PASS"
echo "  (b) orphaned column → stderr warning, no SQL, exit 0:            PASS"
echo "  (c) type mismatch → stderr warning, no SQL, exit 0:              PASS"
echo "  (d) multi-store → both ALTERs emitted in one invocation:         PASS"
echo "  (e) rollback → --apply non-zero, PRAGMA unchanged after failure: PASS"
