#!/usr/bin/env bash
# tests/tasks_tier_hint_e2e.sh — T013 P3/P4: tier_hint inheritance on tasks add.
#
# Demonstrates:
#   - Two observations L001/L002 with intent_contract.tier_hint=T3
#   - `stores tasks add --linked-observations L001 --linked-observations L002`
#     auto-inherits tier_hint=T3 (no --tier-hint flag).
#   - A second task with --linked-observations spanning T2+T3 is rejected with
#     a clear error naming both ids.
#   - --tier-hint T3 explicitly overrides the disagreement.
#   - No linked obs and no --tier-hint → row created with tier_hint NULL.
#
# Usage: bash tests/tasks_tier_hint_e2e.sh

set -euo pipefail

unset CLAUDECODE 2>/dev/null || true

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

TMPDIR=$(mktemp -d /tmp/t013-tier-hint-XXXXXX)
trap 'rm -rf "$TMPDIR"' EXIT

cd "$TMPDIR"
git init -q
stores init > /dev/null
stores install "$STORES_ROOT/stores/observations" > /dev/null
stores install "$STORES_ROOT/stores/gate" > /dev/null
stores install "$STORES_ROOT/stores/tasks" > /dev/null

# ---------------------------------------------------------------------------
# Step 1 — seed two observations both with tier_hint=T3.
# ---------------------------------------------------------------------------
echo "--- Step 1: seed L001 and L002 (both tier_hint=T3)"

L001=$(stores observations add \
    --summary "tier-hint seed #1 (T3)" \
    --source dev \
    --priority normal \
    --captured-at 2026-05-03 \
    --captured-week w18-d7 \
    --tier-hint T3 \
    --invoker human)
[[ "$L001" == "L001" ]] || fail "expected L001, got: $L001"

L002=$(stores observations add \
    --summary "tier-hint seed #2 (T3)" \
    --source dev \
    --priority normal \
    --captured-at 2026-05-03 \
    --captured-week w18-d7 \
    --tier-hint T3 \
    --invoker human)
[[ "$L002" == "L002" ]] || fail "expected L002, got: $L002"
pass "seeded L001 and L002 with tier_hint=T3"

# ---------------------------------------------------------------------------
# Step 2 — tasks add with both linked observations, no --tier-hint → inherits T3.
# ---------------------------------------------------------------------------
echo "--- Step 2: tasks add --linked-observations L001 L002 → tier_hint=T3"

T001=$(stores tasks add \
    --title "Inherit T3 from linked observations" \
    --slug "inherit-t3" \
    --done-when "tier_hint=T3 inferred from L001+L002" \
    --scope-in "tasks add inheritance path" \
    --scope-out "everything else" \
    --linked-observations "$L001" \
    --linked-observations "$L002" \
    --invoker ai_with_human)
[[ "$T001" == "T001" ]] || fail "expected T001, got: $T001"

TIER=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin).get('tier_hint'))")
[[ "$TIER" == "T3" ]] || fail "expected tier_hint=T3, got: $TIER"
pass "T001 tier_hint=T3 (inherited from unanimous L001+L002)"

# ---------------------------------------------------------------------------
# Step 3 — disagreement: seed L003 (T2), then tasks add against L001 (T3) +
# L003 (T2) without --tier-hint → reject; error names both ids.
# ---------------------------------------------------------------------------
echo "--- Step 3: disagreement (T2 + T3) without --tier-hint → reject"

L003=$(stores observations add \
    --summary "tier-hint seed #3 (T2)" \
    --source dev \
    --priority normal \
    --captured-at 2026-05-03 \
    --captured-week w18-d7 \
    --tier-hint T2 \
    --invoker human)
[[ "$L003" == "L003" ]] || fail "expected L003, got: $L003"

set +e
DIS_OUT=$(stores tasks add \
    --title "Should fail on disagreement" \
    --slug "disagree-fail" \
    --done-when "n/a" \
    --scope-in "n/a" \
    --scope-out "n/a" \
    --linked-observations "$L001" \
    --linked-observations "$L003" \
    --invoker ai_with_human 2>&1)
DIS_RC=$?
set -e
[[ "$DIS_RC" -ne 0 ]] || fail "expected non-zero exit on tier disagreement; got 0"
echo "$DIS_OUT" | grep -q "$L001" || fail "error must name $L001; got: $DIS_OUT"
echo "$DIS_OUT" | grep -q "$L003" || fail "error must name $L003; got: $DIS_OUT"
echo "$DIS_OUT" | grep -q -- "--tier-hint" \
    || fail "error must instruct passing --tier-hint; got: $DIS_OUT"
pass "disagreement rejected; error names both observations and --tier-hint"

# ---------------------------------------------------------------------------
# Step 4 — same disagreement WITH --tier-hint T3 → succeeds, tier_hint=T3.
# ---------------------------------------------------------------------------
echo "--- Step 4: disagreement + explicit --tier-hint T3 → tier_hint=T3"

T002=$(stores tasks add \
    --title "Explicit override" \
    --slug "explicit-override" \
    --done-when "explicit wins" \
    --scope-in "x" \
    --scope-out "y" \
    --linked-observations "$L001" \
    --linked-observations "$L003" \
    --tier-hint T3 \
    --invoker ai_with_human)
[[ "$T002" == "T002" ]] || fail "expected T002, got: $T002"
TIER2=$(stores tasks show T002 --json | python3 -c "import sys,json; print(json.load(sys.stdin).get('tier_hint'))")
[[ "$TIER2" == "T3" ]] || fail "expected tier_hint=T3 (explicit), got: $TIER2"
pass "T002 tier_hint=T3 (explicit --tier-hint overrides disagreement)"

# ---------------------------------------------------------------------------
# Step 5 — no linked obs, no --tier-hint → succeeds with tier_hint NULL.
# ---------------------------------------------------------------------------
echo "--- Step 5: no linked obs, no --tier-hint → tier_hint NULL"

T003=$(stores tasks add \
    --title "No linkage, no flag" \
    --slug "no-linkage" \
    --done-when "tier_hint is null when nothing to infer from" \
    --scope-in "x" \
    --scope-out "y" \
    --invoker ai_with_human)
[[ "$T003" == "T003" ]] || fail "expected T003, got: $T003"
TIER3=$(stores tasks show T003 --json | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(d.get('tier_hint') if d.get('tier_hint') is not None else 'NULL')
")
[[ "$TIER3" == "NULL" ]] || fail "expected tier_hint NULL, got: $TIER3"
pass "T003 tier_hint NULL (no linked obs, no flag)"

echo ""
echo "=== T013 P3/P4 tier_hint inheritance e2e: all steps PASS ==="
