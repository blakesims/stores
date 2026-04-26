#!/usr/bin/env bash
# tests/tasks_e2e.sh — End-to-end smoke test for the tasks workflow store.
#
# Drives a real T001 task through the full lifecycle via `stores tasks` CLI:
#   Steps 1-5:   setup, init, install, add, next-action/brief assertions
#   Step 6-7:    submit-plan + submit-plan-review READY → executing
#   Steps 8-10:  3 REVISE cycles (submit-execute + submit-review REVISE)
#   Step 11:     4th REVISE → BLOCKED (marquee assertion: AC9.4)
#   Step 12:     resume recovery: status=executing, current_cycle=1, current_phase unchanged
#   Step 12e:    PASS phase 1 → executing phase 2
#   Step 13:     PASS phase 2 → complete
#   Step 14:     render idempotency (two renders → byte-identical files)
#   Step 15:     SQLite final state assertion
#   Step 16:     cargo test --test submit_atomicity (Rust integration test suite for AC5.11)
#
# Allowed CLI verbs (AC9.6): next-action, brief, submit-plan, submit-plan-review,
#   submit-execute, submit-review, render, add, list, show, resume
#
# Invoker notes:
#   - CLAUDECODE is unset throughout this script to avoid session-inherited ai_autonomous.
#   - Actor-checked verbs use explicit --invoker flags:
#       ai_autonomous: submit-plan, submit-plan-review, submit-execute, submit-review
#       ai_with_human: add, resume
#       human: (none in this script)
#
# Usage: bash tests/tasks_e2e.sh
# Requires: stores binary on PATH (cargo install --path .), sqlite3, cargo

set -euo pipefail

# Unset CLAUDECODE for the whole script — ensures deterministic actor detection
unset CLAUDECODE 2>/dev/null || true

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_PLAN="$STORES_ROOT/tests/fixtures/smoke_plan.json"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

echo "=== stores tasks e2e ==="
echo "stores binary: $(command -v stores)"
echo "stores root: $STORES_ROOT"
echo ""

# ---------------------------------------------------------------------------
# Step 16 (pre-check): Rust atomicity/AC5.11 tests
# ---------------------------------------------------------------------------
echo "--- Step 16: cargo test (atomicity unit tests for AC5.11/AC5.13/AC5.14)"
# Run ac5_11b tests
cargo test --manifest-path "$STORES_ROOT/Cargo.toml" ac5_11b 2>&1 | \
    grep -q "test result: ok" || fail "ac5_11b atomicity test failed"
# Run ac5_13 tests
cargo test --manifest-path "$STORES_ROOT/Cargo.toml" ac5_13 2>&1 | \
    grep -q "test result: ok" || fail "ac5_13 lock-held test failed"
# Run ac5_14 tests
cargo test --manifest-path "$STORES_ROOT/Cargo.toml" ac5_14 2>&1 | \
    grep -q "test result: ok" || fail "ac5_14 blocked-recovery test failed"
pass "atomicity tests (AC5.11/AC5.13/AC5.14) pass"

# ---------------------------------------------------------------------------
# Step 1: Setup — fresh tmp dir with git repo
# ---------------------------------------------------------------------------
echo "--- Step 1: mktemp + git init"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

cd "$TMP"
git init -q
[[ -d .git ]] || fail "git init failed"
pass "fresh tmp dir with git repo: $TMP"

# ---------------------------------------------------------------------------
# Step 2: stores init + install observations + gate + tasks
# ---------------------------------------------------------------------------
echo "--- Step 2: stores init + install stores"
stores init
[[ -f .stores/db.sqlite ]] || fail "db.sqlite not created"
stores install "$STORES_ROOT/stores/observations"
stores install "$STORES_ROOT/stores/gate"
stores install "$STORES_ROOT/stores/tasks"
sqlite3 .stores/db.sqlite ".tables" | grep -q "tasks" || fail "tasks table not present"
pass "init + install observations + gate + tasks"

# ---------------------------------------------------------------------------
# Step 3: tasks add → T001
# ---------------------------------------------------------------------------
echo "--- Step 3: stores tasks add → T001"
OUT=$(stores tasks add \
    --title "Smoke test task" \
    --slug "smoke-test" \
    --capability "test" \
    --done-when "smoke passes" \
    --scope-in "x" \
    --scope-out "y" \
    --invoker ai_with_human)
[[ "$OUT" == "T001" ]] || fail "expected T001, got: $OUT"
pass "add returned T001"

# ---------------------------------------------------------------------------
# Step 4: next-action → assert planner + status: planning
# ---------------------------------------------------------------------------
echo "--- Step 4: stores tasks next-action T001 --json"
NA=$(stores tasks next-action T001 --json)
echo "$NA" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['id'] == 'T001', f'bad id: {d}'
assert d['status'] == 'planning', f'bad status: {d}'
assert d['next_agent'] == 'planner', f'bad next_agent: {d}'
assert d['current_phase'] in [0, None], f'bad current_phase: {d}'
assert d['blocked'] == False, f'bad blocked: {d}'
" || fail "next-action JSON check failed"
pass "next-action: next_agent=planner, status=planning"

# ---------------------------------------------------------------------------
# Step 5: brief → non-empty markdown containing title
# ---------------------------------------------------------------------------
echo "--- Step 5: stores tasks brief T001 --for planner"
BRIEF=$(stores tasks brief T001 --for planner)
[[ -n "$BRIEF" ]] || fail "brief returned empty output"
echo "$BRIEF" | grep -qi "smoke test task\|T001" || fail "brief missing task title"
pass "brief: non-empty markdown with title"

# ---------------------------------------------------------------------------
# Step 6: submit-plan → status: plan_review
# ---------------------------------------------------------------------------
echo "--- Step 6: stores tasks submit-plan T001 --plan-from-file"
stores tasks submit-plan T001 \
    --plan-from-file "$FIXTURE_PLAN" \
    --invoker ai_autonomous
STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
[[ "$STATUS" == "plan_review" ]] || fail "expected plan_review after submit-plan; got: $STATUS"
pass "submit-plan → status: plan_review"

# ---------------------------------------------------------------------------
# Step 7: submit-plan-review READY → status: executing, current_phase: 1
# ---------------------------------------------------------------------------
echo "--- Step 7: stores tasks submit-plan-review T001 --gate READY"
stores tasks submit-plan-review T001 \
    --gate READY \
    --summary "plan approved" \
    --invoker ai_autonomous
ROW=$(stores tasks show T001 --json)
echo "$ROW" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'executing', f'expected executing; got: {d[\"status\"]}'
assert d['current_phase'] == 1, f'expected current_phase=1; got: {d[\"current_phase\"]}'
assert d['current_cycle'] == 1, f'expected current_cycle=1; got: {d[\"current_cycle\"]}'
" || fail "submit-plan-review READY post-state check failed"
pass "submit-plan-review READY → status: executing, current_phase: 1, current_cycle: 1"

# ---------------------------------------------------------------------------
# Steps 8+9: 3 REVISE cycles (current_cycle advances: 1→2, 2→3, 3→4)
# ---------------------------------------------------------------------------
echo "--- Steps 8+9: 3 REVISE cycles"
for i in 1 2 3; do
    # submit-execute (AI autonomous)
    stores tasks submit-execute T001 \
        --summary "phase1 attempt $i" \
        --commit "abc$i" \
        --files-changed "src/foo.rs" \
        --invoker ai_autonomous
    STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
    [[ "$STATUS" == "code_review" ]] || fail "revise cycle $i: expected code_review after submit-execute; got: $STATUS"

    # submit-review REVISE (AI autonomous)
    stores tasks submit-review T001 \
        --gate REVISE \
        --critical 1 --major 0 --minor 0 \
        --summary "needs work: attempt $i" \
        --invoker ai_autonomous
    ROW=$(stores tasks show T001 --json)
    expected_cycle=$(( i + 1 ))
    echo "$ROW" | python3 -c "
import sys, json
d = json.load(sys.stdin)
expected_cycle = $i + 1
assert d['status'] == 'executing', f'revise cycle $i: expected executing; got: {d[\"status\"]}'
assert d['current_cycle'] == expected_cycle, f'revise cycle $i: expected current_cycle={expected_cycle}; got: {d[\"current_cycle\"]}'
" || fail "revise cycle $i: post-REVISE state check failed"
    pass "REVISE cycle $i: status=executing, current_cycle=$(( i + 1 ))"
done

# ---------------------------------------------------------------------------
# Step 11: 4th REVISE → BLOCKED (AC9.4 marquee assertion)
# ---------------------------------------------------------------------------
echo "--- Step 11: 4th REVISE → BLOCKED (marquee assertion)"
stores tasks submit-execute T001 \
    --summary "phase1 attempt 4" \
    --commit "abcd" \
    --files-changed "src/foo.rs" \
    --invoker ai_autonomous

# The 4th submit-review REVISE MUST return non-zero exit code
REVISE_OUT=$(stores tasks submit-review T001 \
    --gate REVISE \
    --critical 1 --major 0 --minor 0 \
    --summary "still broken after 3 revises" \
    --invoker ai_autonomous 2>&1) && \
    fail "4th REVISE should return non-zero exit code" || REVISE_RC=$?

[[ "${REVISE_RC:-0}" -ne 0 ]] || fail "4th REVISE: exit code was 0 (expected non-zero)"
pass "4th REVISE: non-zero exit code ($REVISE_RC)"

# Assert error output mentions the guard expression
echo "$REVISE_OUT" | grep -q "guard" || \
    fail "4th REVISE error missing 'guard'; got: $REVISE_OUT"
echo "$REVISE_OUT" | grep -q "current_cycle" || \
    fail "4th REVISE error missing 'current_cycle'; got: $REVISE_OUT"
pass "4th REVISE: error mentions 'guard' and 'current_cycle'"

# Assert status is blocked and blocked_reason is populated
ROW=$(stores tasks show T001 --json)
echo "$ROW" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'blocked', f'expected blocked; got: {d[\"status\"]}'
br = d.get('blocked_reason', '')
assert br and len(br) > 0, f'blocked_reason is empty; got: {d}'
assert 'guard' in br, f'blocked_reason missing guard; got: {br}'
assert 'current_cycle' in br, f'blocked_reason missing current_cycle; got: {br}'
assert '1' in br, f'blocked_reason missing phase context; got: {br}'
" || fail "4th REVISE: status/blocked_reason assertion failed"
pass "4th REVISE: status=blocked, blocked_reason populated with guard+phase+cycle context"

# ---------------------------------------------------------------------------
# Step 12: resume recovery (actor: ai_with_human)
# ---------------------------------------------------------------------------
echo "--- Step 12: resume recovery"
stores tasks resume T001 --invoker ai_with_human
ROW=$(stores tasks show T001 --json)
echo "$ROW" | python3 -c "
import sys, json
d = json.load(sys.stdin)
# 12a: status = executing
assert d['status'] == 'executing', f'expected executing after resume; got: {d[\"status\"]}'
# 12b: current_cycle == 1 (reset)
assert d['current_cycle'] == 1, f'expected current_cycle=1 after resume; got: {d[\"current_cycle\"]}'
# 12c: current_phase == 1 (UNCHANGED)
assert d['current_phase'] == 1, f'expected current_phase=1 (unchanged); got: {d[\"current_phase\"]}'
# 12d: cycles audit trail preserved (4 entries from the 3+1 revise cycles)
cycles = d.get('cycles', [])
assert len(cycles) == 4, f'expected 4 audit cycles preserved; got: {len(cycles)}'
# blocked_reason cleared
br = d.get('blocked_reason', 'x')
assert not br or br == '', f'expected blocked_reason cleared; got: {br}'
" || fail "resume: post-resume state check failed"
pass "resume: status=executing, current_cycle=1, current_phase=1, cycles=4, blocked_reason cleared"

# ---------------------------------------------------------------------------
# Step 12e: PASS phase 1 → executing phase 2
# ---------------------------------------------------------------------------
echo "--- Step 12e: PASS phase 1 → executing phase 2"
stores tasks submit-execute T001 \
    --summary "post-unblock phase 1 fix" \
    --commit "fixed" \
    --files-changed "src/foo.rs" \
    --invoker ai_autonomous
stores tasks submit-review T001 \
    --gate PASS \
    --critical 0 --major 0 --minor 0 \
    --summary "phase 1 approved after unblock" \
    --invoker ai_autonomous
ROW=$(stores tasks show T001 --json)
echo "$ROW" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'executing', f'expected executing; got: {d[\"status\"]}'
assert d['current_phase'] == 2, f'expected current_phase=2 (PASS-non-last); got: {d[\"current_phase\"]}'
assert d['current_cycle'] == 1, f'expected current_cycle=1 (reset on phase advance); got: {d[\"current_cycle\"]}'
" || fail "PASS phase 1: post-state check failed"
pass "PASS phase 1: status=executing, current_phase=2, current_cycle=1"

# ---------------------------------------------------------------------------
# Step 13: PASS phase 2 → complete
# ---------------------------------------------------------------------------
echo "--- Step 13: PASS phase 2 → complete"
stores tasks submit-execute T001 \
    --summary "phase 2 done" \
    --commit "ph2" \
    --files-changed "src/bar.rs" \
    --invoker ai_autonomous
stores tasks submit-review T001 \
    --gate PASS \
    --critical 0 --major 0 --minor 0 \
    --summary "all phases complete" \
    --invoker ai_autonomous
STATUS=$(stores tasks show T001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
[[ "$STATUS" == "complete" ]] || fail "PASS phase 2: expected complete; got: $STATUS"
pass "PASS phase 2 (PASS-last): status=complete"

# ---------------------------------------------------------------------------
# Step 14: render idempotency
# ---------------------------------------------------------------------------
echo "--- Step 14: render idempotency"
stores tasks render T001
[[ -f "tasks/completed/T001-smoke-test/main.md" ]] || fail "render: main.md not created at tasks/completed/T001-smoke-test/main.md"
pass "render: main.md exists at tasks/completed/T001-smoke-test/"

sha1sum tasks/completed/T001-smoke-test/main.md > /tmp/render1.sha
stores tasks render T001
sha1sum tasks/completed/T001-smoke-test/main.md > /tmp/render2.sha
diff /tmp/render1.sha /tmp/render2.sha || fail "render: two renders not byte-identical"
pass "render: two consecutive renders are byte-identical (AC9.5)"

# ---------------------------------------------------------------------------
# Step 15: SQLite final state assertion
# ---------------------------------------------------------------------------
echo "--- Step 15: SQLite final state"
FINAL=$(sqlite3 .stores/db.sqlite "select status, current_phase from tasks where display_id = 'T001'")
echo "Final state: $FINAL"
echo "$FINAL" | grep -q "complete" || fail "final status not complete"
echo "$FINAL" | grep -q "2" || fail "final current_phase not 2"
pass "SQLite: final state is status=complete, current_phase=2"

# ---------------------------------------------------------------------------
# AC9.6: Verify no forbidden verbs used in this script
# ---------------------------------------------------------------------------
echo "--- AC9.6: verb allowlist check"
# Match lines where 'stores tasks' is invoked as a command (starts with optional whitespace then 'stores tasks')
# Exclude comment lines (starting with #) and the allowlist itself.
ALLOWED_RE="(next-action|brief|submit-plan-review|submit-plan|submit-execute|submit-review|render|add|list|show|resume)"
GREP_OUT=$(grep -E "^\s*stores tasks " "$STORES_ROOT/tests/tasks_e2e.sh" | \
    grep -v "^#" | \
    grep -vE "stores tasks $ALLOWED_RE" || true)
if [[ -n "$GREP_OUT" ]]; then
    fail "AC9.6: forbidden verb in script: $GREP_OUT"
fi
pass "AC9.6: all stores tasks verbs are in the allowed set"

echo ""
echo "=== All tasks e2e steps verified ==="
echo "  #1   mktemp + git init: PASS"
echo "  #2   stores init + install: PASS"
echo "  #3   tasks add → T001: PASS"
echo "  #4   next-action: planner, status=planning: PASS"
echo "  #5   brief: non-empty markdown: PASS"
echo "  #6   submit-plan → plan_review: PASS"
echo "  #7   submit-plan-review READY → executing, phase=1: PASS"
echo "  #8-10 3 REVISE cycles (current_cycle: 2,3,4): PASS"
echo "  #11  4th REVISE → BLOCKED (exit non-zero, mentions guard+current_cycle): PASS"
echo "  #12  resume → executing, current_cycle=1, current_phase=1, cycles=4: PASS"
echo "  #12e PASS phase 1 → executing phase 2, current_phase=2: PASS"
echo "  #13  PASS phase 2 (PASS-last) → complete: PASS"
echo "  #14  render idempotent (byte-identical): PASS"
echo "  #15  SQLite final state: complete/phase=2: PASS"
echo "  #16  atomicity unit tests (AC5.11/AC5.13/AC5.14): PASS"
echo "  AC9.6 verb allowlist: PASS"
