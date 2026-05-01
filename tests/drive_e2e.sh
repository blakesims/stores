#!/usr/bin/env bash
# tests/drive_e2e.sh — End-to-end smoke test for `stores tasks drive` with mock runner.
#
# Drives a fresh T001 task through the full lifecycle via `stores tasks drive --mock`:
#   AC7.1:  N=2 phases, zero REVISE → status=in_review, current_phase=2, 2 total cycles (1/phase)
#   AC7.1b: N=2 phases, one REVISE on phase 2 → status=in_review, phase-2 has 2 cycles
#   AC7.5:  wrap-then-accept — drive → in_review, then `stores tasks accept T001` → accepted
#   AC7.6:  CLI actor enforcement — CLAUDECODE=1 rejects accept/reject; unset allows them
#
# Invoker notes:
#   - CLAUDECODE is unset throughout to avoid session-inherited ai_autonomous.
#   - drive uses ai_autonomous internally for all submit calls.
#   - AC7.5/AC7.6 use the real CLI binary — actor enforcement must be tested at subprocess level.
#
# IMPORTANT: After changes that affect the CLI, rebuild the installed binary first:
#   cargo install --path .
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
trap 'rm -rf "$TMP_HAPPY" "${TMP_REVISE:-}" "${TMP_ACCEPT:-}" "${TMP_REJECT:-}"' EXIT

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

    # Drive with mock runner using happy_2phase fixture; capture stderr for wrap assertion
    DRIVE_STDERR=$(stores tasks drive T001 --mock "$FIXTURE_HAPPY" 2>&1 || true)
    echo "$DRIVE_STDERR" | grep -v "^$" || true

    # Assert final state via show --json
    ROW=$(stores tasks show T001 --json)

    # status = in_review (drive exits after wrap dispatch; human must accept/reject)
    STATUS=$(echo "$ROW" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['status'])")
    [[ "$STATUS" == "in_review" ]] || fail "AC7.1: expected status=in_review; got: $STATUS"

    # AC4.3 eager-dispatch assertion: drive must have spawned the wrap agent.
    # If the status-only guard regression is present, drive exits at in_review before
    # dispatching wrap and "spawning wrap" never appears in stderr.
    echo "$DRIVE_STDERR" | grep -q "spawning wrap" || \
        fail "AC7.1: wrap agent was not dispatched (eager-wrap regression); stderr: $DRIVE_STDERR"

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
pass "AC7.1: status=in_review, current_phase=2, both phases have 1 cycle each with PASS gate"

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

    # Drive with mock runner using revise_once fixture; capture stderr for wrap assertion
    DRIVE_STDERR=$(stores tasks drive T001 --mock "$FIXTURE_REVISE" 2>&1 || true)
    echo "$DRIVE_STDERR" | grep -v "^$" || true

    # Assert final state via show --json
    ROW=$(stores tasks show T001 --json)

    # status = in_review (drive exits after wrap dispatch; human must accept/reject)
    STATUS=$(echo "$ROW" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['status'])")
    [[ "$STATUS" == "in_review" ]] || fail "AC7.1b: expected status=in_review; got: $STATUS"

    # AC4.3 eager-dispatch assertion: wrap agent must have been spawned.
    echo "$DRIVE_STDERR" | grep -q "spawning wrap" || \
        fail "AC7.1b: wrap agent was not dispatched (eager-wrap regression); stderr: $DRIVE_STDERR"

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
pass "AC7.1b: status=in_review, current_phase=2, phase-2 has 2 cycles (REVISE then PASS)"

# ---------------------------------------------------------------------------
# AC7.5: wrap-then-accept end-to-end
# ---------------------------------------------------------------------------
echo "--- AC7.5: wrap-then-accept end-to-end"

TMP_ACCEPT=$(mktemp -d)

(
    cd "$TMP_ACCEPT"
    git init -q
    [[ -d .git ]] || fail "AC7.5: git init failed"

    stores init
    stores install "$STORES_ROOT/stores/observations"
    stores install "$STORES_ROOT/stores/gate"
    stores install "$STORES_ROOT/stores/tasks"

    OUT=$(stores tasks add \
        --title "Accept e2e test" \
        --slug "accept-e2e" \
        --capability "test" \
        --done-when "task accepted by human" \
        --scope-in "wrap accept path" \
        --scope-out "real claude" \
        --invoker ai_with_human)
    [[ "$OUT" == "T001" ]] || fail "AC7.5: expected T001, got: $OUT"

    # Drive to in_review
    stores tasks drive T001 --mock "$FIXTURE_HAPPY" >/dev/null 2>&1 || true

    # Verify in_review before accept
    PRE_STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
    [[ "$PRE_STATUS" == "in_review" ]] || fail "AC7.5: expected in_review before accept; got: $PRE_STATUS"

    # Verify wrap_log non-empty
    WRAP_LEN=$(stores tasks show T001 --json | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('wrap_log', [])))")
    [[ "$WRAP_LEN" -ge 1 ]] || fail "AC7.5: expected wrap_log non-empty before accept; got: $WRAP_LEN"

    # Human invokes accept (CLAUDECODE is unset → human actor)
    stores tasks accept T001

    # Assert final status accepted
    POST_STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
    [[ "$POST_STATUS" == "accepted" ]] || fail "AC7.5: expected status=accepted after accept; got: $POST_STATUS"

    # Assert wrap_log preserved (accept does not modify wrap_log)
    POST_WRAP_LEN=$(stores tasks show T001 --json | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('wrap_log', [])))")
    [[ "$POST_WRAP_LEN" -eq "$WRAP_LEN" ]] || fail "AC7.5: wrap_log length changed on accept ($WRAP_LEN → $POST_WRAP_LEN); accept must not append to wrap_log"
)
pass "AC7.5: drive → in_review → accept → accepted; wrap_log preserved"

# ---------------------------------------------------------------------------
# AC7.6: CLI-level actor enforcement (CLAUDECODE env)
# ---------------------------------------------------------------------------
echo "--- AC7.6: CLI actor enforcement via CLAUDECODE env"

TMP_REJECT=$(mktemp -d)

(
    cd "$TMP_REJECT"
    git init -q
    [[ -d .git ]] || fail "AC7.6: git init failed"

    stores init
    stores install "$STORES_ROOT/stores/observations"
    stores install "$STORES_ROOT/stores/gate"
    stores install "$STORES_ROOT/stores/tasks"

    stores tasks add \
        --title "Actor enforcement test" \
        --slug "actor-enf" \
        --capability "test" \
        --done-when "actor enforcement verified" \
        --scope-in "accept/reject actor gate" \
        --scope-out "real claude" \
        --invoker ai_with_human >/dev/null

    # Drive T001 to in_review
    stores tasks drive T001 --mock "$FIXTURE_HAPPY" >/dev/null 2>&1 || true

    PRE_STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
    [[ "$PRE_STATUS" == "in_review" ]] || fail "AC7.6: expected in_review before actor test; got: $PRE_STATUS"

    # With CLAUDECODE=1 → accept must fail (ai_autonomous not allowed for actor:human)
    ACCEPT_ERR=$(CLAUDECODE=1 stores tasks accept T001 2>&1) && fail "AC7.6: accept with CLAUDECODE=1 should have exited non-zero" || true
    echo "$ACCEPT_ERR" | grep -q "transition 'accept'" || \
        fail "AC7.6: accept error should mention transition 'accept'; got: $ACCEPT_ERR"
    echo "$ACCEPT_ERR" | grep -q "requires actor 'human'" || \
        fail "AC7.6: accept error should mention requires actor 'human'; got: $ACCEPT_ERR"

    # Status must still be in_review (accept was rejected)
    MID_STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
    [[ "$MID_STATUS" == "in_review" ]] || fail "AC7.6: status changed after failed accept; got: $MID_STATUS"

    # With CLAUDECODE=1 → reject must also fail (--reason required to get past clap parse)
    REJECT_ERR=$(CLAUDECODE=1 stores tasks reject T001 --reason "test rejection" 2>&1) && fail "AC7.6: reject with CLAUDECODE=1 should have exited non-zero" || true
    echo "$REJECT_ERR" | grep -q "transition 'reject'" || \
        fail "AC7.6: reject error should mention transition 'reject'; got: $REJECT_ERR"
    echo "$REJECT_ERR" | grep -q "requires actor 'human'" || \
        fail "AC7.6: reject error should mention requires actor 'human'; got: $REJECT_ERR"

    # Without CLAUDECODE → reject must succeed (human invoker) and persist reject_reason
    unset CLAUDECODE 2>/dev/null || true
    stores tasks reject T001 --reason "test rejection"

    # Assert final status rejected
    FINAL_STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
    [[ "$FINAL_STATUS" == "rejected" ]] || fail "AC7.6: expected status=rejected after human reject; got: $FINAL_STATUS"

    # Assert reject_reason was written to wrap_log[-1]
    REJECT_REASON=$(stores tasks show T001 --json | python3 -c "
import sys, json
row = json.load(sys.stdin)
wl = row.get('wrap_log') or []
if isinstance(wl, str): wl = json.loads(wl)
last = wl[-1] if wl else {}
print(last.get('reject_reason', ''))
")
    [[ "$REJECT_REASON" == "test rejection" ]] || fail "AC7.6: expected reject_reason='test rejection' in wrap_log[-1]; got: '$REJECT_REASON'"
)
pass "AC7.6: CLAUDECODE=1 rejects accept+reject; unset CLAUDECODE allows reject with --reason; status=rejected, reject_reason persisted"

echo ""
echo "=== All drive e2e scenarios passed ==="
echo "  AC7.1  happy path (N=2 phases, zero REVISE): PASS"
echo "  AC7.1b revise-once (one REVISE on phase 2):  PASS"
echo "  AC7.5  wrap-then-accept end-to-end:           PASS"
echo "  AC7.6  CLI actor enforcement (CLAUDECODE):    PASS (reject --reason persisted)"
