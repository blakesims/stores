#!/usr/bin/env bash
# tests/drive_e2e.sh — End-to-end smoke test for `stores tasks drive` with mock runner.
#
# Drives a fresh T001 task through the full lifecycle via `stores tasks drive --mock`:
#   AC7.1:  N=2 phases, zero REVISE → status=complete, current_phase=2, 2 total cycles (1/phase)
#   AC7.1b: N=2 phases, one REVISE on phase 2 → status=complete, phase-2 has 2 cycles
#
# Invoker notes:
#   - CLAUDECODE is unset throughout to avoid session-inherited ai_autonomous.
#   - drive uses ai_autonomous internally for all submit calls.
#
# Usage: bash tests/drive_e2e.sh
# Requires: stores binary on PATH (cargo install --path . OR cargo build --release), sqlite3, jq

set -euo pipefail

# Unset CLAUDECODE for the whole script — ensures deterministic actor detection
unset CLAUDECODE 2>/dev/null || true

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_HAPPY="$STORES_ROOT/tests/fixtures/drive_e2e/happy_2phase.jsonl"
FIXTURE_REVISE="$STORES_ROOT/tests/fixtures/drive_e2e/revise_once.jsonl"

pass() { echo "  ✓ $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

echo "=== stores drive e2e ==="
echo "stores binary: $(command -v stores)"
echo "stores root: $STORES_ROOT"
echo ""

# ---------------------------------------------------------------------------
# Pre-check: fixtures exist
# ---------------------------------------------------------------------------
[[ -f "$FIXTURE_HAPPY" ]] || fail "happy_2phase.jsonl not found at $FIXTURE_HAPPY"
[[ -f "$FIXTURE_REVISE" ]] || fail "revise_once.jsonl not found at $FIXTURE_REVISE"
pass "fixtures present"

# ---------------------------------------------------------------------------
# AC7.1: happy path — N=2 phases, zero REVISE
# ---------------------------------------------------------------------------
echo "--- AC7.1: happy path (N=2 phases, zero REVISE)"

TMP_HAPPY=$(mktemp -d)
trap 'rm -rf "$TMP_HAPPY" "$TMP_REVISE"' EXIT

(
    cd "$TMP_HAPPY"
    git init -q
    [[ -d .git ]] || fail "AC7.1: git init failed"

    stores init
    [[ -f .stores/db.sqlite ]] || fail "AC7.1: db.sqlite not created"
    stores install "$STORES_ROOT/stores/observations"
    stores install "$STORES_ROOT/stores/gate"
    stores install "$STORES_ROOT/stores/tasks"
    sqlite3 .stores/db.sqlite ".tables" | grep -q "tasks" || fail "AC7.1: tasks table not present"

    OUT=$(stores tasks add \
        --title "Happy 2-phase drive test" \
        --slug "happy-drive" \
        --capability "test" \
        --done-when "both phases pass with no revise" \
        --scope-in "drive loop" \
        --scope-out "real claude" \
        --invoker ai_with_human)
    [[ "$OUT" == "T001" ]] || fail "AC7.1: expected T001, got: $OUT"

    # Drive with mock runner using happy_2phase fixture
    stores tasks drive T001 --mock "$FIXTURE_HAPPY" 2>&1 | \
        grep -v "^$" || true

    # Assert final state via show --json
    ROW=$(stores tasks show T001 --json)

    # status = complete
    STATUS=$(echo "$ROW" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['status'])")
    [[ "$STATUS" == "complete" ]] || fail "AC7.1: expected status=complete; got: $STATUS"

    # current_phase = 2
    PHASE=$(echo "$ROW" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['current_phase'])")
    [[ "$PHASE" == "2" ]] || fail "AC7.1: expected current_phase=2; got: $PHASE"

    # cycles: exactly 2 entries (one per phase), both with PASS gate
    echo "$ROW" | python3 -c "
import sys, json
d = json.load(sys.stdin)
cycles = d.get('cycles', [])
assert len(cycles) == 2, f'AC7.1: expected 2 cycles total (1/phase), got {len(cycles)}: {json.dumps(cycles, indent=2)}'

# Phase 1: cycle index 0
c0 = cycles[0]
assert c0.get('phase') == 1, f'AC7.1: cycles[0].phase should be 1; got {c0.get(\"phase\")}'
assert c0.get('cycle') == 1, f'AC7.1: cycles[0].cycle should be 1; got {c0.get(\"cycle\")}'
r0 = c0.get('review') or {}
assert r0.get('gate') == 'PASS', f'AC7.1: cycles[0].review.gate should be PASS; got {r0.get(\"gate\")}'

# Phase 2: cycle index 1
c1 = cycles[1]
assert c1.get('phase') == 2, f'AC7.1: cycles[1].phase should be 2; got {c1.get(\"phase\")}'
assert c1.get('cycle') == 1, f'AC7.1: cycles[1].cycle should be 1; got {c1.get(\"cycle\")}'
r1 = c1.get('review') or {}
assert r1.get('gate') == 'PASS', f'AC7.1: cycles[1].review.gate should be PASS; got {r1.get(\"gate\")}'

print('assertions ok')
" || fail "AC7.1: cycles assertion failed"

)
pass "AC7.1: status=complete, current_phase=2, both phases have 1 cycle each with PASS gate"

# ---------------------------------------------------------------------------
# AC7.1b: revise-once — N=2 phases, one REVISE cycle on phase 2
# ---------------------------------------------------------------------------
echo "--- AC7.1b: revise-once (one REVISE on phase 2)"

TMP_REVISE=$(mktemp -d)

(
    cd "$TMP_REVISE"
    git init -q
    [[ -d .git ]] || fail "AC7.1b: git init failed"

    stores init
    stores install "$STORES_ROOT/stores/observations"
    stores install "$STORES_ROOT/stores/gate"
    stores install "$STORES_ROOT/stores/tasks"

    OUT=$(stores tasks add \
        --title "Revise-once drive test" \
        --slug "revise-drive" \
        --capability "test" \
        --done-when "phase 2 passes after one revise cycle" \
        --scope-in "drive loop revise" \
        --scope-out "real claude" \
        --invoker ai_with_human)
    [[ "$OUT" == "T001" ]] || fail "AC7.1b: expected T001, got: $OUT"

    # Drive with mock runner using revise_once fixture
    stores tasks drive T001 --mock "$FIXTURE_REVISE" 2>&1 | \
        grep -v "^$" || true

    # Assert final state via show --json
    ROW=$(stores tasks show T001 --json)

    # status = complete
    STATUS=$(echo "$ROW" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['status'])")
    [[ "$STATUS" == "complete" ]] || fail "AC7.1b: expected status=complete; got: $STATUS"

    # current_phase = 2
    PHASE=$(echo "$ROW" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['current_phase'])")
    [[ "$PHASE" == "2" ]] || fail "AC7.1b: expected current_phase=2; got: $PHASE"

    # cycles: exactly 3 entries total
    #   index 0: phase=1, cycle=1, review.gate=PASS
    #   index 1: phase=2, cycle=1, review.gate=REVISE
    #   index 2: phase=2, cycle=2, review.gate=PASS
    echo "$ROW" | python3 -c "
import sys, json
d = json.load(sys.stdin)
cycles = d.get('cycles', [])
assert len(cycles) == 3, f'AC7.1b: expected 3 cycles total (p1c1 + p2c1-REVISE + p2c2-PASS), got {len(cycles)}: {json.dumps(cycles, indent=2)}'

# Phase 1: cycle 1 → PASS
c0 = cycles[0]
assert c0.get('phase') == 1, f'AC7.1b: cycles[0].phase should be 1; got {c0.get(\"phase\")}'
assert c0.get('cycle') == 1, f'AC7.1b: cycles[0].cycle should be 1; got {c0.get(\"cycle\")}'
r0 = c0.get('review') or {}
assert r0.get('gate') == 'PASS', f'AC7.1b: cycles[0].review.gate should be PASS; got {r0.get(\"gate\")}'

# Phase 2: cycle 1 → REVISE
c1 = cycles[1]
assert c1.get('phase') == 2, f'AC7.1b: cycles[1].phase should be 2; got {c1.get(\"phase\")}'
assert c1.get('cycle') == 1, f'AC7.1b: cycles[1].cycle should be 1; got {c1.get(\"cycle\")}'
r1 = c1.get('review') or {}
assert r1.get('gate') == 'REVISE', f'AC7.1b: cycles[1].review.gate should be REVISE; got {r1.get(\"gate\")}'

# Phase 2: cycle 2 → PASS (final)
c2 = cycles[2]
assert c2.get('phase') == 2, f'AC7.1b: cycles[2].phase should be 2; got {c2.get(\"phase\")}'
assert c2.get('cycle') == 2, f'AC7.1b: cycles[2].cycle should be 2; got {c2.get(\"cycle\")}'
r2 = c2.get('review') or {}
assert r2.get('gate') == 'PASS', f'AC7.1b: cycles[2].review.gate should be PASS; got {r2.get(\"gate\")}'

print('assertions ok')
" || fail "AC7.1b: cycles assertion failed"

)
pass "AC7.1b: status=complete, current_phase=2, phase-2 has 2 cycles (REVISE then PASS)"

echo ""
echo "=== All drive e2e scenarios passed ==="
echo "  AC7.1  happy path (N=2 phases, zero REVISE): PASS"
echo "  AC7.1b revise-once (one REVISE on phase 2):  PASS"
