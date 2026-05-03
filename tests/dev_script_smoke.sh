#!/usr/bin/env bash
# tests/dev_script_smoke.sh — Smoke test for the ./dev worktree script (T003).
#
# Strategy: run from a self-contained tempdir.
#   - Create a tempdir, `git init`, single empty commit (so worktrees can branch).
#   - `stores setup` to install bundled stores (tasks, observations, gate).
#   - Copy the repo's ./dev script into the tempdir.
#   - Invoke `./dev new --slug=smoke ...`, assert exit 0.
#   - Read LAST line of stdout = printed worktree path; assert it exists.
#   - Assert substrate row T001 exists with workspace_path == printed path.
#   - Tear down via `./dev done T001 --force`; assert worktree gone.
#
# The test does NOT touch the host repo's substrate or worktrees.
#
# Usage: bash tests/dev_script_smoke.sh
# Requires: stores binary built (target/debug/stores) or on PATH; git.

set -euo pipefail

# Ensure deterministic actor detection (mirror tasks_e2e.sh).
unset CLAUDECODE 2>/dev/null || true

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEV_SCRIPT="$REPO_ROOT/dev"

# Locate stores binary: prefer target/debug, fall back to PATH.
if [[ -x "$REPO_ROOT/target/debug/stores" ]]; then
    STORES_BIN="$REPO_ROOT/target/debug/stores"
elif command -v stores >/dev/null 2>&1; then
    STORES_BIN="$(command -v stores)"
else
    echo "FAIL: stores binary not found (build with 'cargo build' or install to PATH)" >&2
    exit 1
fi
# Make `stores` resolvable inside the dev script's locate_stores helper
# regardless of which path we picked above.
export PATH="$(dirname "$STORES_BIN"):$PATH"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

[[ -x "$DEV_SCRIPT" ]] || fail "./dev script not executable at $DEV_SCRIPT"

TMP_PARENT="$(mktemp -d -t dev-smoke-XXXXXX)"
TMP_REPO="$TMP_PARENT/repo"
mkdir -p "$TMP_REPO"

EXPECTED_WORKTREE="$TMP_PARENT/repo-T001-smoke"

cleanup() {
    # Best-effort: remove the tempdir + any worktree that escaped --force teardown.
    rm -rf "$EXPECTED_WORKTREE" 2>/dev/null || true
    rm -rf "$TMP_PARENT" 2>/dev/null || true
}
trap cleanup EXIT

echo "=== dev script smoke ==="
echo "repo root:   $REPO_ROOT"
echo "stores bin:  $STORES_BIN"
echo "tmp repo:    $TMP_REPO"
echo ""

# --- Step 1: bootstrap a clean repo with installed substrate ----------------
( cd "$TMP_REPO" \
    && git init -q \
    && git config user.email "smoke@example.com" \
    && git config user.name "smoke" \
    && git commit -q --allow-empty -m "init" )
pass "tempdir initialized as git repo with one commit"

( cd "$TMP_REPO" && "$STORES_BIN" setup >/dev/null )
[[ -f "$TMP_REPO/.stores/db.sqlite" ]] || fail ".stores/db.sqlite missing after setup"
pass "stores setup installed bundled stores"

cp "$DEV_SCRIPT" "$TMP_REPO/dev"
chmod +x "$TMP_REPO/dev"

# --- Step 2: ./dev new ------------------------------------------------------
NEW_OUT="$TMP_PARENT/new.stdout"
NEW_ERR="$TMP_PARENT/new.stderr"
set +e
( cd "$TMP_REPO" && ./dev new \
    --slug=smoke \
    --title=smoke \
    --done-when=x \
    --scope-in=x \
    --scope-out=x \
    >"$NEW_OUT" 2>"$NEW_ERR" )
RC=$?
set -e
if [[ $RC -ne 0 ]]; then
    echo "--- ./dev new stdout ---" >&2; cat "$NEW_OUT" >&2
    echo "--- ./dev new stderr ---" >&2; cat "$NEW_ERR" >&2
    fail "./dev new exited $RC (expected 0)"
fi
pass "./dev new exited 0"

PRINTED_PATH="$(tail -n 1 "$NEW_OUT")"
[[ -n "$PRINTED_PATH" ]] || fail "./dev new printed no path on stdout"
[[ -d "$PRINTED_PATH" ]] || fail "printed worktree path does not exist: $PRINTED_PATH"
pass "printed worktree path exists: $PRINTED_PATH"

# --- Step 3: substrate row assertions --------------------------------------
SHOW_JSON="$( cd "$TMP_REPO" && "$STORES_BIN" tasks show T001 --json )" \
    || fail "stores tasks show T001 --json failed"

ROW_WS="$(printf '%s' "$SHOW_JSON" \
    | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d.get("workspace_path") or "")')"
[[ -n "$ROW_WS" ]] || fail "T001 row has empty workspace_path"
[[ "$ROW_WS" == "$PRINTED_PATH" ]] \
    || fail "workspace_path mismatch: row=$ROW_WS printed=$PRINTED_PATH"
pass "T001 row workspace_path matches printed path"

ROW_BRANCH="$(printf '%s' "$SHOW_JSON" \
    | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d.get("branch") or "")')"
[[ "$ROW_BRANCH" == "feat/T001-smoke" ]] \
    || fail "expected branch=feat/T001-smoke, got: $ROW_BRANCH"
pass "T001 row branch == feat/T001-smoke"

# --- Step 4: ./dev done <id> --force ---------------------------------------
DONE_OUT="$TMP_PARENT/done.stdout"
DONE_ERR="$TMP_PARENT/done.stderr"
set +e
( cd "$TMP_REPO" && ./dev done T001 --force \
    >"$DONE_OUT" 2>"$DONE_ERR" )
RC=$?
set -e
if [[ $RC -ne 0 ]]; then
    echo "--- ./dev done stdout ---" >&2; cat "$DONE_OUT" >&2
    echo "--- ./dev done stderr ---" >&2; cat "$DONE_ERR" >&2
    fail "./dev done T001 --force exited $RC (expected 0)"
fi
pass "./dev done T001 --force exited 0"

[[ ! -d "$PRINTED_PATH" ]] || fail "worktree still present after ./dev done: $PRINTED_PATH"
pass "worktree removed: $PRINTED_PATH"

WT_LIST="$( cd "$TMP_REPO" && git worktree list --porcelain )"
if printf '%s\n' "$WT_LIST" | grep -q "$PRINTED_PATH"; then
    fail "git worktree list still references removed path"
fi
pass "git worktree list no longer references the smoke worktree"

echo ""
echo "=== smoke OK ==="
