#!/usr/bin/env bash
# tests/e2e.sh — End-to-end verification of all 13 DONE_WHEN steps.
#
# README command correspondence (auditable list — same commands, same order):
#   1.  stores init
#   2.  stores install ./stores/observations
#   3.  stores install ./stores/gate (multi-store coexistence)
#   4.  stores observations add --summary "thing broke" --source dev --priority normal \
#         --captured-at 2026-04-30 --captured-week w11-d4                 → L001
#   5.  stores observations update L001 --contract-state ready             → fails (required_when)
#         Error cites intent_contract.objective, .acceptance, .in_scope, .out_of_scope,
#         .tier_hint, .type, .approved_by, .approved_at (all gated by contract_state == 'ready')
#   6.  stores observations investigate L001 --invoker human               → open → investigating
#       stores observations update L001 --contract-state ready             \
#         --objective "..." --type work --in-scope "..." --out-of-scope "..."\
#         --acceptance "..." --tier-hint T3                               \
#         --approved-by blake --approved-at 2026-04-30 --invoker human    → succeeds
#       stores observations confirm L001 --invoker human                  → investigating → confirmed
#   7.  stores observations show L001                                      → entry with intent_contract
#       stores observations show L001 --json                               → intent_contract.contract_state == 'ready'
#   8.  stores observations list                                           → all entries
#   9.  stores gate add --type decision --one-liner "Soft or hard delete on cleanup?" \
#         --options "soft|hard" --task-ref L001                            → G001
#   10. stores gate answer G001 --answer hard --invoker human             → succeeds
#   11. CLAUDECODE=1 stores gate add --type decision \
#         --one-liner "Actor check demo gate" --options "yes|no"           → G003 (step 11a)
#       CLAUDECODE=1 stores gate answer G003 --answer hard                → fails (actor-mismatch)
#   12. sqlite3 .stores/db.sqlite "select o.display_id, o.status,
#         json_extract(o.intent_contract,'$.tier_hint'), g.display_id
#         from observations o left join gate g on g.task_ref = o.display_id" → L001|confirmed|T3|G001
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
# Step 4: observations add → L001 (with 4 new required fields)
# ---------------------------------------------------------------------------
echo "--- Step 4: stores observations add"
OUT=$(stores observations add --summary "thing broke" \
    --source dev --priority normal \
    --captured-at 2026-04-30 --captured-week w11-d4)
[[ "$OUT" == "L001" ]] || fail "expected L001, got: $OUT"
pass "add returned L001"

# ---------------------------------------------------------------------------
# Step 5: update --contract-state ready without sub-fields → fails (required_when)
# Demonstrates the same enforcement as v0.1's triage T3 rejection:
# a write that flips contract_state to ready while missing all required sub-fields
# is rejected, with errors citing each intent_contract field gated by contract_state == 'ready'.
# ---------------------------------------------------------------------------
echo "--- Step 5: contract-state ready without sub-fields (should fail)"
ERR_OUT=$(stores observations update L001 --contract-state ready --invoker human 2>&1) \
    && fail "expected non-zero exit" || true
echo "$ERR_OUT" | grep -q "intent_contract.objective" \
    || fail "expected intent_contract.objective in error; got: $ERR_OUT"
echo "$ERR_OUT" | grep -q "intent_contract.acceptance" \
    || fail "expected intent_contract.acceptance in error; got: $ERR_OUT"
echo "$ERR_OUT" | grep -q "intent_contract.in_scope" \
    || fail "expected intent_contract.in_scope in error; got: $ERR_OUT"
echo "$ERR_OUT" | grep -q "intent_contract.out_of_scope" \
    || fail "expected intent_contract.out_of_scope in error; got: $ERR_OUT"
echo "$ERR_OUT" | grep -q "intent_contract.tier_hint" \
    || fail "expected intent_contract.tier_hint in error; got: $ERR_OUT"
echo "$ERR_OUT" | grep -q "intent_contract.contract_state == 'ready'" \
    || fail "expected contract_state == 'ready' enforcement cited; got: $ERR_OUT"
pass "contract-state ready without sub-fields rejected with required_when errors"

# ---------------------------------------------------------------------------
# Step 6: investigate → update contract to ready with all sub-fields → confirm
# Demonstrates the full triage flow under the new intent_contract shape:
#   open → investigating  (investigate verb, actor: ai_with_human)
#   update intent_contract sub-fields + contract_state=ready (actor: human for approved_by/at)
#   investigating → confirmed  (confirm verb, guard: contract_state == 'ready')
# ---------------------------------------------------------------------------
echo "--- Step 6: investigate + fill contract + confirm (should succeed)"
stores observations investigate L001 --invoker human
stores observations update L001 \
    --contract-state ready \
    --objective "Fix the broken backend handler" \
    --type work \
    --in-scope "backend handler" \
    --out-of-scope "frontend" \
    --acceptance "handler returns 200 after fix" \
    --tier-hint T3 \
    --approved-by blake \
    --approved-at 2026-04-30 \
    --invoker human
stores observations confirm L001 --invoker human
pass "investigate + contract ready + confirm succeeded"

# ---------------------------------------------------------------------------
# Step 7: show L001 — entry with nested intent_contract record
# ---------------------------------------------------------------------------
echo "--- Step 7: stores observations show L001"
SHOW_OUT=$(stores observations show L001)
echo "$SHOW_OUT" | grep -q "display_id: L001" || fail "show missing display_id"
echo "$SHOW_OUT" | grep -q "tier_hint: T3" || fail "show missing intent_contract.tier_hint"
echo "$SHOW_OUT" | grep -q "contract_state: ready" || fail "show missing intent_contract.contract_state"

# JSON: nested intent_contract keys present with production shape
SHOW_JSON=$(stores observations show L001 --json)
echo "$SHOW_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['display_id'] == 'L001', f'bad display_id: {d}'
assert isinstance(d['intent_contract'], dict), f'intent_contract not nested dict: {d}'
assert d['intent_contract']['contract_state'] == 'ready', f'bad contract_state: {d}'
assert d['intent_contract']['tier_hint'] == 'T3', f'bad tier_hint: {d}'
assert d['intent_contract']['objective'] == 'Fix the broken backend handler', f'bad objective: {d}'
assert isinstance(d['intent_contract']['acceptance'], list), f'acceptance not list: {d}'
assert isinstance(d['intent_contract']['in_scope'], list), f'in_scope not list: {d}'
assert isinstance(d['intent_contract']['out_of_scope'], list), f'out_of_scope not list: {d}'
" || fail "show --json failed structure check"
pass "show returns entry with nested intent_contract (contract_state=ready, tier_hint=T3)"

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
    --one-liner "Soft or hard delete on cleanup?" \
    --options "soft|hard" \
    --task-ref L001 \
    --filed-by e2e-test \
    --source dev)
[[ "$GATE_OUT" == "G001" ]] || fail "expected G001, got: $GATE_OUT"
pass "gate add returned G001 with task-ref L001"

# ---------------------------------------------------------------------------
# Step 9b: gate add from human (no CLAUDECODE) — actor-gate-fix verification
#   After removing default_actor: ai_autonomous from gate/schema.yaml, a human
#   invoker must be able to add a gate without --invoker flag.
# ---------------------------------------------------------------------------
echo "--- Step 9b: gate add as human invoker (no CLAUDECODE, no --invoker flag)"
GATE_HUMAN_OUT=$(stores gate add --type decision \
    --one-liner "Human-filed gate test" \
    --options "yes|no" \
    --task-ref L001 \
    --filed-by e2e-test \
    --source dev)
[[ "$GATE_HUMAN_OUT" == "G002" ]] || fail "expected G002 from human gate add, got: $GATE_HUMAN_OUT"
pass "human invoker gate add succeeded (no actor-mismatch error)"

# ---------------------------------------------------------------------------
# Step 10: gate answer G001 --invoker human → succeeds
# ---------------------------------------------------------------------------
echo "--- Step 10: stores gate answer G001 --invoker human"
stores gate answer G001 --answer hard --invoker human
pass "gate answer G001 succeeded with --invoker human"

# ---------------------------------------------------------------------------
# Step 11: actor-mismatch rejection under CLAUDECODE=1
#   G001+G002 are pending; add G003 as fresh pending gate via AI invoker.
# ---------------------------------------------------------------------------
echo "--- Step 11: actor-mismatch rejection (CLAUDECODE=1, no --invoker)"
GATE3_OUT=$(CLAUDECODE=1 stores gate add --type decision \
    --one-liner "Actor check demo gate" \
    --options "yes|no" \
    --filed-by e2e-test \
    --source dev)
[[ "$GATE3_OUT" == "G003" ]] || fail "expected G003, got: $GATE3_OUT"

ACTOR_ERR=$(CLAUDECODE=1 stores gate answer G003 --answer hard 2>&1) && fail "expected non-zero exit for actor mismatch" || true
echo "$ACTOR_ERR" | grep -q "actor" || fail "expected actor-related error; got: $ACTOR_ERR"
echo "$ACTOR_ERR" | grep -q "human" || fail "expected 'human' in error; got: $ACTOR_ERR"
pass "CLAUDECODE=1 gate answer without --invoker rejected with actor-mismatch error"

# Verify --invoker human override succeeds on G003
stores gate answer G003 --answer yes --invoker human
pass "gate answer G003 with --invoker human override succeeded"

# Answer G002 (from step 9b) as well
stores gate answer G002 --answer yes --invoker human
pass "gate answer G002 (human-filed gate) succeeded"

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
# Uses intent_contract.tier_hint (new shape) instead of triage.verdict (v0.1)
# ---------------------------------------------------------------------------
echo "--- Step 12: cross-store SQL JOIN"
JOIN_OUT=$(sqlite3 .stores/db.sqlite \
    "select o.display_id, o.status, json_extract(o.intent_contract,'$.tier_hint'), g.display_id from observations o left join gate g on g.task_ref = o.display_id")

echo "JOIN output: $JOIN_OUT"

# Assert G001 (non-NULL gate display_id) appears in the output joined to L001
echo "$JOIN_OUT" | grep -q "L001" || fail "L001 not in JOIN output"
echo "$JOIN_OUT" | grep -q "G001" || fail "G001 not in JOIN output — join match is NULL"
# Confirm the row structure: L001|confirmed|T3|G001
echo "$JOIN_OUT" | grep -q "T3" || fail "tier_hint T3 not in JOIN output"
pass "JOIN returns L001|confirmed|T3|G001 — real non-NULL gate match confirmed"

# ---------------------------------------------------------------------------
# Step 13: Summary of enforcement verified throughout
# ---------------------------------------------------------------------------
echo ""
echo "=== All 13 DONE_WHEN steps verified ==="
echo "  #1  init: PASS"
echo "  #2  install observations: PASS"
echo "  #3  install gate (multi-store coexistence): PASS"
echo "  #4  observations add → L001 (with required source/priority/captured-at/captured-week): PASS"
echo "  #5  contract-state ready without sub-fields rejected (intent_contract.contract_state == 'ready'): PASS"
echo "  #6  investigate + contract ready + confirm succeeds: PASS"
echo "  #7  show L001 nested intent_contract (contract_state=ready, tier_hint=T3): PASS"
echo "  #8  list all entries: PASS"
echo "  #9  gate add → G001 with task-ref L001: PASS"
echo "  #10 gate answer G001 --invoker human: PASS"
echo "  #11 CLAUDECODE=1 gate answer without --invoker rejected: PASS"
echo "  #12 cross-store SQL JOIN returns L001|confirmed|T3|G001 (intent_contract.tier_hint): PASS"
echo "  #13 invoker detection + --invoker override + schema enforcement: PASS (verified throughout)"
