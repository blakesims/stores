#!/usr/bin/env bash
# tests/e2e.sh — End-to-end verification of all 13 DONE_WHEN steps.
#
# README command correspondence (auditable list — same commands, same order):
#   1.  stores init
#   2.  stores install ./stores/observations
#   3.  stores install ./stores/gate
#   4.  stores observations add --summary "thing broke"                    → L001
#   5.  stores observations triage L001 --verdict T3                       → fails (required_when)
#   6.  stores observations triage L001 --verdict T3 --done-when "X works after fix" \
#         --scope-in "backend handler" --scope-out "frontend"              → succeeds
#   7.  stores observations show L001                                       → entry with triage + contract
#   8.  stores observations list                                            → all entries
#   9.  stores gate add --type decision --question "Soft or hard delete on cleanup?" \
#         --options "soft|hard" --task-ref L001                            → G001
#   10. stores gate answer G001 --answer hard --invoker human              → succeeds
#   11. CLAUDECODE=1 stores gate add --type decision \
#         --question "Actor check demo gate" --options "yes|no"            → G002 (step 11a: fresh pending gate)
#       CLAUDECODE=1 stores gate answer G002 --answer hard                 → fails (actor-mismatch)
#   12. sqlite3 .stores/db.sqlite "select o.display_id, o.status,
#         json_extract(o.triage,'$.verdict'), g.display_id
#         from observations o left join gate g on g.task_ref = o.display_id" → G001 non-NULL join match
#   13. (verified throughout: $CLAUDECODE → ai_autonomous; --invoker overrides; schema violations rejected)
#
# Usage: bash tests/e2e.sh
# Requires: stores binary on PATH (cargo install --path .), sqlite3, jq (optional)

set -euo pipefail

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "=== stores e2e ==="
echo "tmp dir: $TMP"
echo "stores binary: $(command -v stores)"
echo ""

cd "$TMP"

# ---------------------------------------------------------------------------
# Step 1: stores init
# ---------------------------------------------------------------------------
echo "--- Step 1: stores init"
stores init
[[ -f .stores/db.sqlite ]] || fail "db.sqlite not created"
[[ -f .stores/manifest.yaml ]] || fail "manifest.yaml not created"
pass "init created .stores/db.sqlite and .stores/manifest.yaml"

# ---------------------------------------------------------------------------
# Step 2: stores install ./stores/observations
# ---------------------------------------------------------------------------
echo "--- Step 2: stores install observations"
stores install "$STORES_ROOT/stores/observations"
sqlite3 .stores/db.sqlite ".tables" | grep -q "observations" || fail "observations table not present"
pass "observations store installed"

# ---------------------------------------------------------------------------
# Step 3: stores install ./stores/gate (multi-store coexistence)
# ---------------------------------------------------------------------------
echo "--- Step 3: stores install gate"
stores install "$STORES_ROOT/stores/gate"
TABLES=$(sqlite3 .stores/db.sqlite ".tables")
echo "$TABLES" | grep -q "observations" || fail "observations table missing after gate install"
echo "$TABLES" | grep -q "gate" || fail "gate table not present"
pass "gate store installed; both tables coexist"

# ---------------------------------------------------------------------------
# Step 4: observations add → L001
# ---------------------------------------------------------------------------
echo "--- Step 4: stores observations add"
OUT=$(stores observations add --summary "thing broke")
[[ "$OUT" == "L001" ]] || fail "expected L001, got: $OUT"
pass "add returned L001"

# ---------------------------------------------------------------------------
# Step 5: triage without contract → fails citing required_when
# ---------------------------------------------------------------------------
echo "--- Step 5: triage T3 without contract fields (should fail)"
ERR_OUT=$(stores observations triage L001 --verdict T3 2>&1) && fail "expected non-zero exit" || true
echo "$ERR_OUT" | grep -q "contract.done_when" || fail "expected contract.done_when in error; got: $ERR_OUT"
echo "$ERR_OUT" | grep -q "contract.scope_in" || fail "expected contract.scope_in in error"
echo "$ERR_OUT" | grep -q "contract.scope_out" || fail "expected contract.scope_out in error"
pass "triage T3 without contract rejected with required_when errors"

# ---------------------------------------------------------------------------
# Step 6: triage with full contract → succeeds
# ---------------------------------------------------------------------------
echo "--- Step 6: triage T3 with full contract (should succeed)"
stores observations triage L001 --verdict T3 \
    --done-when "X works after fix" \
    --scope-in "backend handler" \
    --scope-out "frontend"
pass "triage with contract succeeded"

# ---------------------------------------------------------------------------
# Step 7: show L001 — entry with nested triage + contract
# ---------------------------------------------------------------------------
echo "--- Step 7: stores observations show L001"
SHOW_OUT=$(stores observations show L001)
echo "$SHOW_OUT" | grep -q "display_id: L001" || fail "show missing display_id"
echo "$SHOW_OUT" | grep -q "verdict: T3" || fail "show missing triage.verdict"
echo "$SHOW_OUT" | grep -q "done_when: X works after fix" || fail "show missing contract.done_when"

# JSON: nested triage + contract keys present
SHOW_JSON=$(stores observations show L001 --json)
echo "$SHOW_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['display_id'] == 'L001', f'bad display_id: {d}'
assert isinstance(d['triage'], dict), f'triage not nested dict: {d}'
assert d['triage']['verdict'] == 'T3', f'bad verdict: {d}'
assert isinstance(d['contract'], dict), f'contract not nested dict: {d}'
assert d['contract']['done_when'] == 'X works after fix', f'bad done_when: {d}'
" || fail "show --json failed structure check"
pass "show returns entry with nested triage and contract"

# ---------------------------------------------------------------------------
# Step 8: list → all entries
# ---------------------------------------------------------------------------
echo "--- Step 8: stores observations list"
LIST_OUT=$(stores observations list)
echo "$LIST_OUT" | grep -q "L001" || fail "list output missing L001"

LIST_JSON=$(stores observations list --json)
COUNT=$(echo "$LIST_JSON" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))")
[[ "$COUNT" -ge 1 ]] || fail "list --json returned empty array"
pass "list shows L001"

# ---------------------------------------------------------------------------
# Step 9: gate add → G001 (task-ref = L001 for JOIN)
# ---------------------------------------------------------------------------
echo "--- Step 9: stores gate add → G001"
GATE_OUT=$(stores gate add --type decision \
    --question "Soft or hard delete on cleanup?" \
    --options "soft|hard" \
    --task-ref L001)
[[ "$GATE_OUT" == "G001" ]] || fail "expected G001, got: $GATE_OUT"
pass "gate add returned G001 with task-ref L001"

# ---------------------------------------------------------------------------
# Step 10: gate answer G001 --invoker human → succeeds
# ---------------------------------------------------------------------------
echo "--- Step 10: stores gate answer G001 --invoker human"
stores gate answer G001 --answer hard --invoker human
pass "gate answer G001 succeeded with --invoker human"

# ---------------------------------------------------------------------------
# Step 11: actor-mismatch rejection under CLAUDECODE=1
#   G001 is already answered, so we add G002 as a fresh pending gate.
# ---------------------------------------------------------------------------
echo "--- Step 11: actor-mismatch rejection (CLAUDECODE=1, no --invoker)"
GATE2_OUT=$(CLAUDECODE=1 stores gate add --type decision \
    --question "Actor check demo gate" \
    --options "yes|no")
[[ "$GATE2_OUT" == "G002" ]] || fail "expected G002, got: $GATE2_OUT"

ACTOR_ERR=$(CLAUDECODE=1 stores gate answer G002 --answer hard 2>&1) && fail "expected non-zero exit for actor mismatch" || true
echo "$ACTOR_ERR" | grep -q "actor" || fail "expected actor-related error; got: $ACTOR_ERR"
echo "$ACTOR_ERR" | grep -q "human" || fail "expected 'human' in error; got: $ACTOR_ERR"
pass "CLAUDECODE=1 gate answer without --invoker rejected with actor-mismatch error"

# Verify --invoker human override succeeds on G002
stores gate answer G002 --answer yes --invoker human
pass "gate answer G002 with --invoker human override succeeded"

# Verify JSON on gate show + list
GATE_JSON=$(stores gate show G001 --json)
echo "$GATE_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['display_id'] == 'G001', f'bad display_id: {d}'
assert isinstance(d['options'], list), f'options not list: {d}'
assert d['answer'] == 'hard', f'bad answer: {d}'
assert d['task_ref'] == 'L001', f'bad task_ref: {d}'
" || fail "gate show --json failed structure check"

GATE_LIST_JSON=$(stores gate list --json)
echo "$GATE_LIST_JSON" | python3 -c "
import sys, json
arr = json.load(sys.stdin)
assert len(arr) >= 1, f'expected >=1 gates; got {arr}'
" || fail "gate list --json returned empty"
pass "gate show/list --json valid"

# ---------------------------------------------------------------------------
# Step 12: cross-store SQL JOIN — non-NULL gate match
# ---------------------------------------------------------------------------
echo "--- Step 12: cross-store SQL JOIN"
JOIN_OUT=$(sqlite3 .stores/db.sqlite \
    "select o.display_id, o.status, json_extract(o.triage,'$.verdict'), g.display_id from observations o left join gate g on g.task_ref = o.display_id")

echo "JOIN output: $JOIN_OUT"

# Assert G001 (non-NULL gate display_id) appears in the output joined to L001
echo "$JOIN_OUT" | grep -q "L001" || fail "L001 not in JOIN output"
echo "$JOIN_OUT" | grep -q "G001" || fail "G001 not in JOIN output — join match is NULL"
# Confirm the row structure: L001|...|T3|G001
echo "$JOIN_OUT" | grep -q "T3" || fail "verdict T3 not in JOIN output"
pass "JOIN returns L001|...|T3|G001 — real non-NULL gate match confirmed"

# ---------------------------------------------------------------------------
# Step 13: Summary of enforcement verified throughout
# ---------------------------------------------------------------------------
echo ""
echo "=== All 13 DONE_WHEN steps verified ==="
echo "  #1  init: PASS"
echo "  #2  install observations: PASS"
echo "  #3  install gate (multi-store coexistence): PASS"
echo "  #4  observations add → L001: PASS"
echo "  #5  triage T3 without contract rejected: PASS"
echo "  #6  triage T3 with contract succeeds: PASS"
echo "  #7  show L001 nested triage+contract: PASS"
echo "  #8  list all entries: PASS"
echo "  #9  gate add → G001 with task-ref L001: PASS"
echo "  #10 gate answer G001 --invoker human: PASS"
echo "  #11 CLAUDECODE=1 gate answer without --invoker rejected: PASS"
echo "  #12 cross-store SQL JOIN returns non-NULL G001 match: PASS"
echo "  #13 invoker detection + --invoker override + schema enforcement: PASS (verified throughout)"
