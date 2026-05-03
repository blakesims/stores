#!/usr/bin/env bash
# tests/agents_e2e.sh — End-to-end coverage for the autonomous-flow daemon
# and the `stores agents backfill` one-off verb (T014 P7).
#
# Test (a): live daemon path —
#   init substrate + install bundled stores
#   drop fixture agents.yaml + policies.yaml into .stores/
#   seed a tasks row at in_review with branch + workspace_path pointing at
#     a temp git repo (non-conflicting branch)
#   accept the row (stores tasks accept --invoker human)
#   run `stores agents run --max-iters=3 --poll-interval=1` (foreground)
#   assert: HEAD on main is a merge commit, row stayed accepted
#
# Test (m): backfill verb —
#   seed a SECOND tasks row at status=accepted, branch=feat/y, no merge yet
#   run `stores agents backfill`
#   assert: branch feat/y now merged into main
#   assert: backfill did NOT write a transition_history row for the seed
#           (backfill is side-effect catchup, not a state transition)
#
# Usage: bash tests/agents_e2e.sh
# Requires: `stores` binary on PATH (cargo install --path .), git, sqlite3.

set -euo pipefail

unset CLAUDECODE 2>/dev/null || true

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_AGENTS="$STORES_ROOT/tests/fixtures/agents.yaml"
FIXTURE_POLICIES="$STORES_ROOT/tests/fixtures/policies.yaml"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

[[ -f "$FIXTURE_AGENTS" ]] || fail "missing fixture: $FIXTURE_AGENTS"
[[ -f "$FIXTURE_POLICIES" ]] || fail "missing fixture: $FIXTURE_POLICIES"

TMP=$(mktemp -d /tmp/stores-agents-e2e-XXXXXX)
trap 'rm -rf "$TMP"' EXIT

REPO="$TMP/repo"
mkdir -p "$REPO"

echo "=== stores agents-e2e ==="
echo "tmp dir: $TMP"
echo "stores binary: $(command -v stores)"
echo ""

# ---------------------------------------------------------------------------
# AC7.1: backfill --help works; empty substrate → "0 rows scanned"
# ---------------------------------------------------------------------------
echo "--- AC7.1: backfill --help + empty-substrate exit 0"
EMPTY=$(mktemp -d /tmp/stores-agents-empty-XXXXXX)
(
    cd "$EMPTY"
    git init -q
    stores setup > /dev/null
    stores agents backfill --help > /dev/null \
        || fail "AC7.1: backfill --help failed"
    OUT=$(stores agents backfill 2>&1)
    echo "$OUT" | grep -q "0 rows scanned" \
        || fail "AC7.1: empty-substrate backfill must say '0 rows scanned'; got: $OUT"
)
rm -rf "$EMPTY"
pass "AC7.1: backfill --help works; empty substrate → '0 rows scanned'"

# ---------------------------------------------------------------------------
# Setup: temp git repo + branches; substrate init in the repo
# ---------------------------------------------------------------------------
echo "--- setup: temp git repo + substrate"
cd "$REPO"
git init -q -b main
git config user.email "agents-e2e@example.com"
git config user.name "agents-e2e"
echo "main-base" > base.txt
git add base.txt
git commit -q -m "init"

# Branch feat/x: non-conflicting addition (clean merge target).
git checkout -q -b feat/x
echo "feat-x" > feat-x.txt
git add feat-x.txt
git commit -q -m "feat x"
git checkout -q main

# Branch feat/y: another non-conflicting addition (for backfill test).
git checkout -q -b feat/y
echo "feat-y" > feat-y.txt
git add feat-y.txt
git commit -q -m "feat y"
git checkout -q main

stores setup > /dev/null
[[ -f .stores/db.sqlite ]] || fail "setup: .stores/db.sqlite not created"
cp "$FIXTURE_AGENTS" .stores/agents.yaml
cp "$FIXTURE_POLICIES" .stores/policies.yaml

NOW="2026-05-03T00:00:00Z"
CONTRACT='{"done_when":"x","scope_in":"y","scope_out":"z"}'

# ---------------------------------------------------------------------------
# Test (a): seed in_review row → accept → daemon dispatches accept-merge
# ---------------------------------------------------------------------------
echo "--- test (a): in_review → accept → daemon merges feat/x into main"

sqlite3 .stores/db.sqlite <<SQL
INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by)
VALUES ('T001', 'in_review', 'agents e2e (a)', 'agents-e2e-a', 'feat/x', '$REPO', '$CONTRACT', '$NOW', '$NOW', 'human', 'human');
SQL

# `accept` requires actor=human. CLAUDECODE is unset; --invoker human passes.
stores tasks accept T001 --invoker human
STATUS_AFTER_ACCEPT=$(sqlite3 .stores/db.sqlite "SELECT status FROM tasks WHERE display_id='T001'")
[[ "$STATUS_AFTER_ACCEPT" == "accepted" ]] \
    || fail "(a): expected status=accepted after accept; got: $STATUS_AFTER_ACCEPT"

# Confirm transition_history captured in_review→accepted (the daemon's hook).
TH_COUNT=$(sqlite3 .stores/db.sqlite \
    "SELECT COUNT(*) FROM transition_history WHERE store='tasks' AND display_id='T001' AND from_status='in_review' AND to_status='accepted'")
[[ "$TH_COUNT" -ge 1 ]] || fail "(a): transition_history missing in_review→accepted row"

# Run the daemon with --max-iters=3 so it exits after a few polls. The first
# poll should claim + dispatch accept-merge, which merges feat/x into main.
stores agents run --max-iters=3 --poll-interval 1 > "$TMP/daemon.log" 2>&1 \
    || fail "(a): daemon exited non-zero; log: $(cat $TMP/daemon.log)"

# Assert merge commit on main.
MERGE_LOG=$(git -C "$REPO" log --oneline --merges -n 5)
echo "$MERGE_LOG" | grep -q "feat/x" \
    || fail "(a): expected merge commit naming feat/x on main; got: $MERGE_LOG"

# Row still accepted (clean merge does NOT flip to deploy_blocked).
STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM tasks WHERE display_id='T001'")
[[ "$STATUS" == "accepted" ]] \
    || fail "(a): row should remain accepted on clean merge; got: $STATUS"

# Daemon recorded a successful dispatch.
DISPATCH_OK=$(sqlite3 .stores/db.sqlite \
    "SELECT last_status FROM dispatch_locks WHERE display_id='T001' AND agent_name='accept-merge'")
[[ "$DISPATCH_OK" == "ok" ]] \
    || fail "(a): expected dispatch_locks last_status=ok; got: $DISPATCH_OK"

pass "(a): live daemon dispatched accept-merge; main has merge commit for feat/x"

# ---------------------------------------------------------------------------
# Test (m): backfill verb — pre-seeded accepted-but-unmerged row gets merged.
# ---------------------------------------------------------------------------
echo "--- test (m): backfill merges pre-seeded accepted-but-unmerged row"

sqlite3 .stores/db.sqlite <<SQL
INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by)
VALUES ('T002', 'accepted', 'agents e2e (m)', 'agents-e2e-m', 'feat/y', '$REPO', '$CONTRACT', '$NOW', '$NOW', 'framework', 'framework');
SQL

# Snapshot transition_history count before backfill — must NOT change.
TH_BEFORE=$(sqlite3 .stores/db.sqlite \
    "SELECT COUNT(*) FROM transition_history WHERE store='tasks' AND display_id='T002'")

BACKFILL_OUT=$(stores agents backfill 2>&1)
echo "$BACKFILL_OUT" | grep -q "1 rows scanned\|merged" \
    || fail "(m): backfill output should mention scan or merge; got: $BACKFILL_OUT"

# feat/y now merged into main.
git -C "$REPO" branch --merged main | grep -q 'feat/y' \
    || fail "(m): feat/y must be merged into main after backfill"

# AC7.2: transition_history did NOT gain a row from backfill (no state change).
TH_AFTER=$(sqlite3 .stores/db.sqlite \
    "SELECT COUNT(*) FROM transition_history WHERE store='tasks' AND display_id='T002'")
[[ "$TH_BEFORE" == "$TH_AFTER" ]] \
    || fail "(m): backfill must NOT write transition_history; was $TH_BEFORE → $TH_AFTER"

# Re-running backfill is idempotent: feat/y now in --merged list, so skipped.
BACKFILL_OUT2=$(stores agents backfill 2>&1)
echo "$BACKFILL_OUT2" | grep -q "already merged" \
    || fail "(m): second backfill should report 'already merged'; got: $BACKFILL_OUT2"

pass "(m): backfill merged feat/y; idempotent on re-run; no spurious transition_history write"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "=== agents-e2e: all checks passed ==="
echo "  AC7.1  backfill --help + empty-substrate    PASS"
echo "  (a)    daemon dispatches accept-merge        PASS"
echo "  (m)    backfill merges + idempotent          PASS"
