#!/usr/bin/env bash
# tests/meta_routing_e2e.sh — T023 P3: end-to-end verification of cross-CWD
# routing via --meta / STORES_META_PATH.
#
# Asserts:
#   1) Happy path: from a CWD that has no .stores/, `stores --meta=<META>` (and
#      the env-var form) writes to <META>/.stores/db.sqlite and does NOT create
#      `.stores/` in the CWD.
#   2) Error path: STORES_META_PATH unset and no --meta given prints a clean
#      diagnostic and exits non-zero.
#   3) Error path: STORES_META_PATH points at a directory with no `.stores/`
#      prints a clean diagnostic and exits non-zero.
#
# Usage: bash tests/meta_routing_e2e.sh
# Requires: cargo, sqlite3, git on PATH.

set -euo pipefail

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

# Build release binary deterministically (cheap if already built).
( cd "$STORES_ROOT" && cargo build --release >/dev/null 2>&1 ) \
    || fail "cargo build --release failed"

STORES_BIN="$STORES_ROOT/target/release/stores"
[[ -x "$STORES_BIN" ]] || fail "stores binary not found at $STORES_BIN"

TMPDIR="${STORES_E2E_TMP:-$(mktemp -d /tmp/t023-meta-routing-XXXXXX)}"
trap 'rm -rf "$TMPDIR"' EXIT

echo "=== stores --meta routing e2e (T023 P3) ==="
echo "tmp dir: $TMPDIR"
echo "stores binary: $STORES_BIN"
echo ""

# Unset CLAUDECODE so default invoker is human; we pass --invoker explicitly
# anyway, but this matches the convention of the other e2e scripts.
unset CLAUDECODE 2>/dev/null || true
# Clear STORES_META_PATH between cases.
unset STORES_META_PATH 2>/dev/null || true

DIR_A="$TMPDIR/A"
DIR_B="$TMPDIR/B"
mkdir -p "$DIR_A" "$DIR_B"

# ---------------------------------------------------------------------------
# Setup: dir A is the META substrate; dir B is the unrelated CWD.
# ---------------------------------------------------------------------------
( cd "$DIR_A" && git init -q && "$STORES_BIN" setup >/dev/null 2>&1 ) \
    || fail "setup failed in A"
[[ -f "$DIR_A/.stores/db.sqlite" ]] || fail "A/.stores/db.sqlite missing after setup"
pass "A: stores setup created .stores/db.sqlite"

# Sanity: B has no .stores/ before the routed call.
[[ ! -e "$DIR_B/.stores" ]] || fail "B: .stores/ exists before routed call (precondition)"

# ---------------------------------------------------------------------------
# Step 1 — happy path: --meta=<PATH> from B routes to A
# ---------------------------------------------------------------------------
echo "--- Step 1 — --meta=<PATH> from B routes write to A/.stores/db.sqlite"
ROW_OUT=$( cd "$DIR_B" && "$STORES_BIN" --meta="$DIR_A" observations add \
    --invoker ai_autonomous \
    --summary "cross-CWD filing via --meta" \
    --source dev \
    --priority normal \
    --captured-at 2026-05-05 \
    --captured-week w18-d2 \
    --body "filed from B, routed to A" )
[[ "$ROW_OUT" == "L001" ]] || fail "Step 1: expected L001, got: $ROW_OUT"
pass "--meta=<PATH> add returned L001"

# Row exists in A.
A_ROW_COUNT=$(sqlite3 "$DIR_A/.stores/db.sqlite" "SELECT COUNT(*) FROM observations WHERE display_id='L001';")
[[ "$A_ROW_COUNT" == "1" ]] || fail "Step 1: expected 1 row in A, got: $A_ROW_COUNT"
pass "L001 row found in A/.stores/db.sqlite"

# B/.stores/ was NOT created.
[[ ! -e "$DIR_B/.stores" ]] || fail "Step 1: B/.stores/ was created — routing leaked"
pass "B/.stores/ was not created"

# ---------------------------------------------------------------------------
# Step 2 — env var form: STORES_META_PATH=A, bare --meta from B routes to A
# ---------------------------------------------------------------------------
echo "--- Step 2 — STORES_META_PATH=A + bare --meta from B routes to A"
ROW_OUT=$( cd "$DIR_B" && STORES_META_PATH="$DIR_A" "$STORES_BIN" --meta observations add \
    --invoker ai_autonomous \
    --summary "env-var routed filing" \
    --source dev \
    --priority normal \
    --captured-at 2026-05-05 \
    --captured-week w18-d2 \
    --body "filed from B via STORES_META_PATH" )
[[ "$ROW_OUT" == "L002" ]] || fail "Step 2: expected L002, got: $ROW_OUT"
pass "env-var form add returned L002"

A_ROW_COUNT=$(sqlite3 "$DIR_A/.stores/db.sqlite" "SELECT COUNT(*) FROM observations;")
[[ "$A_ROW_COUNT" == "2" ]] || fail "Step 2: expected 2 rows in A, got: $A_ROW_COUNT"
pass "two rows now in A/.stores/db.sqlite"

[[ ! -e "$DIR_B/.stores" ]] || fail "Step 2: B/.stores/ was created — routing leaked"
pass "B/.stores/ still not created"

# ---------------------------------------------------------------------------
# Step 3 — error: STORES_META_PATH unset + bare --meta → clean diagnostic
# ---------------------------------------------------------------------------
echo "--- Step 3 — bare --meta with STORES_META_PATH unset errors clean"
set +e
ERR_OUT=$( cd "$DIR_B" && unset STORES_META_PATH && "$STORES_BIN" --meta observations add \
    --invoker ai_autonomous \
    --summary "should fail" \
    --source dev \
    --priority normal \
    --captured-at 2026-05-05 \
    --captured-week w18-d2 \
    --body "should fail" 2>&1 )
RC=$?
set -e
[[ $RC -ne 0 ]] || fail "Step 3: expected non-zero exit, got 0"
echo "$ERR_OUT" | grep -q "STORES_META_PATH" \
    || fail "Step 3: error did not mention STORES_META_PATH; got: $ERR_OUT"
pass "bare --meta with env unset → non-zero exit + STORES_META_PATH in diagnostic"

# ---------------------------------------------------------------------------
# Step 4 — error: STORES_META_PATH points at dir without .stores/
# ---------------------------------------------------------------------------
echo "--- Step 4 — STORES_META_PATH points at non-store dir errors clean"
NO_STORE_DIR="$TMPDIR/C-no-stores"
mkdir -p "$NO_STORE_DIR"
set +e
ERR_OUT=$( cd "$DIR_B" && STORES_META_PATH="$NO_STORE_DIR" "$STORES_BIN" --meta observations add \
    --invoker ai_autonomous \
    --summary "should fail" \
    --source dev \
    --priority normal \
    --captured-at 2026-05-05 \
    --captured-week w18-d2 \
    --body "should fail" 2>&1 )
RC=$?
set -e
[[ $RC -ne 0 ]] || fail "Step 4: expected non-zero exit, got 0"
echo "$ERR_OUT" | grep -qE "\.stores|does not contain|not a stores" \
    || fail "Step 4: error did not mention missing .stores/; got: $ERR_OUT"
pass "STORES_META_PATH at non-store dir → non-zero exit + diagnostic mentions .stores"

echo ""
echo "=== meta routing e2e: ALL PASS ==="
