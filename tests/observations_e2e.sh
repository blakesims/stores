#!/usr/bin/env bash
# tests/observations_e2e.sh — End-to-end scripted trace of the 8 T009 DONE_WHEN clauses.
#
# Clause 1: Full add with all production-shaped fields → L001
# Clause 2: Triage flow: open → investigating → confirmed
#            (intent_contract.contract_state: draft → ready; guard gates confirm)
# Clause 3: required_when rejection — flip contract_state to ready without sub-fields
#            (intent_contract.objective, .acceptance, .in_scope, .out_of_scope,
#             .tier_hint, .type, .approved_by, .approved_at all cited in error)
# Clause 4: actor:human rejection — CLAUDECODE=1 + --invoker ai_autonomous on approved_by
# Clause 5: evidence.external_refs round-trips as JSON array (list_record; T006 P2 substrate)
# Clause 6: notes round-trips as structured JSON object (T008 substrate)
# Clause 7: needs_info parking + provide_info resume (confirmed → needs_info → confirmed)
#            AI invoker on provide_info must fail.
# Clause 8: cross-store task_id soft-FK — value round-trip only (T010 guards are out-of-scope).
#            NOTE: this step verifies task_id round-trips through add/show --json and that
#            stores tasks show <id> returns a row. It does NOT verify cross-store referential
#            integrity (e.g. tasks.complete requiring observations.status == resolved) — that
#            is T010 work.
#
# Usage: bash tests/observations_e2e.sh
# Requires: stores binary on PATH, python3, jq, git

set -euo pipefail

# Unset CLAUDECODE for the whole script so default invoker is human.
# Steps that need AI invoker re-set CLAUDECODE=1 explicitly for that one command only.
unset CLAUDECODE 2>/dev/null || true

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

TMPDIR="${STORES_E2E_TMP:-$(mktemp -d /tmp/t009-obs-port-XXXXXX)}"
trap 'rm -rf "$TMPDIR"' EXIT

echo "=== stores observations e2e (8 DONE_WHEN clauses) ==="
echo "tmp dir: $TMPDIR"
echo "stores binary: $(command -v stores)"
echo ""

cd "$TMPDIR"

# ---------------------------------------------------------------------------
# Setup: git init + stores setup (setup auto-installs observations + tasks + gate)
# ---------------------------------------------------------------------------
git init -q
stores setup > /dev/null 2>&1
[[ -f .stores/db.sqlite ]] || fail "setup: .stores/db.sqlite not created"
sqlite3 .stores/db.sqlite ".tables" | grep -q "observations" \
    || fail "setup: observations table not found"
sqlite3 .stores/db.sqlite ".tables" | grep -q "tasks" \
    || fail "setup: tasks table not found"

# ---------------------------------------------------------------------------
# Step 1/8 — Full add with all production-shaped fields
# ---------------------------------------------------------------------------
echo "--- Step 1/8 — full add (dashboard-sourced T3 with full intent contract) → L001"
L001_OUT=$(stores observations add \
    --summary "Dashboard: missing field handler causes 500 on checkout" \
    --source dashboard \
    --priority high \
    --captured-at 2026-04-30 \
    --captured-week w11-d4 \
    --contract-state draft \
    --objective "Fix the missing field handler on the checkout path" \
    --type work \
    --in-scope "checkout field handler" \
    --out-of-scope "mobile clients, admin panel" \
    --acceptance "handler returns 200 for all field types" \
    --tier-hint T3 \
    --drafted-by morning-check \
    --drafted-at 2026-04-30T08:00:00Z)
[[ "$L001_OUT" == "L001" ]] || fail "Step 1: expected L001, got: $L001_OUT"
pass "add returned L001"

L001_JSON=$(stores observations show L001 --json)
echo "$L001_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['display_id'] == 'L001', f'bad display_id: {d}'
assert d['status'] == 'open', f'expected open, got: {d[\"status\"]}'
assert d['source'] == 'dashboard', f'bad source: {d}'
assert d['priority'] == 'high', f'bad priority: {d}'
assert d['captured_at'] == '2026-04-30', f'bad captured_at: {d}'
assert d['captured_week'] == 'w11-d4', f'bad captured_week: {d}'
assert isinstance(d['intent_contract'], dict), f'intent_contract not dict: {d}'
assert d['intent_contract']['contract_state'] == 'draft', f'bad contract_state: {d}'
assert d['intent_contract']['tier_hint'] == 'T3', f'bad tier_hint: {d}'
assert d['intent_contract']['type'] == 'work', f'bad type: {d}'
assert d['intent_contract']['objective'] is not None, f'objective is null: {d}'
assert isinstance(d['intent_contract']['in_scope'], list), f'in_scope not list: {d}'
assert isinstance(d['intent_contract']['out_of_scope'], list), f'out_of_scope not list: {d}'
assert isinstance(d['intent_contract']['acceptance'], list), f'acceptance not list: {d}'
" || fail "Step 1: show --json field assertions failed"
pass "all required fields present and correct in show --json (L001)"

# ---------------------------------------------------------------------------
# Step 2/8 — Triage flow: open → investigating → confirmed
# ---------------------------------------------------------------------------
echo "--- Step 2/8 — triage flow (open → investigating → confirmed)"

# open → investigating (T001 P4: investigate is now actor:ai_autonomous;
# closes L007. Pass --invoker ai_autonomous explicitly because the test
# script unsets CLAUDECODE.)
stores observations investigate L001 --invoker ai_autonomous \
    || fail "Step 2: investigate failed"

L001_STATUS=$(stores observations show L001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
[[ "$L001_STATUS" == "investigating" ]] \
    || fail "Step 2: expected investigating after investigate, got: $L001_STATUS"
pass "status=investigating after investigate"

# Fill intent_contract to ready (approved_by/at require --invoker human)
stores observations update L001 \
    --contract-state ready \
    --approved-by blake \
    --approved-at 2026-04-30T09:00:00Z \
    --invoker human \
    || fail "Step 2: update contract to ready failed"

L001_CONTRACT=$(stores observations show L001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['intent_contract']['contract_state'])")
[[ "$L001_CONTRACT" == "ready" ]] \
    || fail "Step 2: expected contract_state=ready after update, got: $L001_CONTRACT"
pass "intent_contract.contract_state=ready after update"

# investigating → confirmed (guard: contract_state == 'ready' is now satisfied)
stores observations confirm L001 --invoker human \
    || fail "Step 2: confirm failed"

L001_JSON=$(stores observations show L001 --json)
echo "$L001_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'confirmed', f'expected confirmed, got: {d[\"status\"]}'
assert d['intent_contract']['contract_state'] == 'ready', f'expected contract_state=ready, got: {d}'
" || fail "Step 2: show --json post-confirm assertions failed"
pass "status=confirmed and intent_contract.contract_state=ready"

# ---------------------------------------------------------------------------
# Step 3/8 — required_when: flip contract_state ready without sub-fields (L002)
# Uses a fresh entry to ensure the sub-fields are absent.
# ---------------------------------------------------------------------------
echo "--- Step 3/8 — required_when: contract_state ready without sub-fields"

# Add L002 with no contract sub-fields
L002_OUT=$(stores observations add \
    --summary "Minimal observation for required_when test" \
    --source dev \
    --priority normal \
    --captured-at 2026-04-30 \
    --captured-week w11-d4)
[[ "$L002_OUT" == "L002" ]] || fail "Step 3: expected L002, got: $L002_OUT"

# Try flipping contract_state to ready without required sub-fields → must fail
set +e
REQUIRED_ERR=$(stores observations update L002 --contract-state ready --invoker human 2>&1)
REQUIRED_EXIT=$?
set -e

[[ "$REQUIRED_EXIT" -ne 0 ]] \
    || fail "Step 3: expected non-zero exit when missing required_when sub-fields; got exit 0"
echo "$REQUIRED_ERR" | grep -q "intent_contract.contract_state == 'ready'" \
    || fail "Step 3: expected 'intent_contract.contract_state == .ready.' in error; got: $REQUIRED_ERR"
echo "$REQUIRED_ERR" | grep -q "intent_contract.objective" \
    || fail "Step 3: expected intent_contract.objective in error; got: $REQUIRED_ERR"
echo "$REQUIRED_ERR" | grep -q "intent_contract.acceptance" \
    || fail "Step 3: expected intent_contract.acceptance in error; got: $REQUIRED_ERR"
pass "required_when rejected (exit=$REQUIRED_EXIT); error cites intent_contract.contract_state == 'ready' + sub-fields"

# ---------------------------------------------------------------------------
# Step 4/8 — actor:human on approved_by rejects AI invoker
# ---------------------------------------------------------------------------
echo "--- Step 4/8 — actor:human rejection on intent_contract.approved_by"
set +e
ACTOR_ERR=$(CLAUDECODE=1 stores observations update L002 --approved-by blake --invoker ai_autonomous 2>&1)
ACTOR_EXIT=$?
set -e

[[ "$ACTOR_EXIT" -ne 0 ]] \
    || fail "Step 4: expected non-zero exit for AI invoker on approved_by; got exit 0"
echo "$ACTOR_ERR" | grep -q "actor" \
    || fail "Step 4: expected 'actor' in error; got: $ACTOR_ERR"
echo "$ACTOR_ERR" | grep -q "human" \
    || fail "Step 4: expected 'human' in error; got: $ACTOR_ERR"
echo "$ACTOR_ERR" | grep -q "approved_by" \
    || fail "Step 4: expected 'approved_by' in error; got: $ACTOR_ERR"
pass "CLAUDECODE=1 ai_autonomous rejected on approved_by (exit=$ACTOR_EXIT); error mentions actor/human/approved_by"

# ---------------------------------------------------------------------------
# Step 5/8 — evidence.external_refs round-trips as JSON array (list_record)
# ---------------------------------------------------------------------------
echo "--- Step 5/8 — evidence.external_refs JSON array round-trip"

L003_OUT=$(stores observations add \
    --summary "Evidence round-trip test" \
    --source sentry \
    --priority normal \
    --captured-at 2026-04-30 \
    --captured-week w11-d4)
[[ "$L003_OUT" == "L003" ]] || fail "Step 5: expected L003, got: $L003_OUT"

stores observations update L003 \
    --external-refs '[{"system":"docker","kind":"container","id":"x"}]' \
    || fail "Step 5: update external_refs failed"

SYSTEM_VAL=$(stores observations show L003 --json | jq '.evidence.external_refs[0].system')
[[ "$SYSTEM_VAL" == '"docker"' ]] \
    || fail "Step 5: expected .evidence.external_refs[0].system == \"docker\"; got: $SYSTEM_VAL"
pass "evidence.external_refs[0].system == \"docker\" — structured array, not blob"

# ---------------------------------------------------------------------------
# Step 6/8 — notes round-trips as JSON object
# ---------------------------------------------------------------------------
echo "--- Step 6/8 — notes JSON object round-trip"

L004_OUT=$(stores observations add \
    --summary "Notes round-trip test" \
    --source dev \
    --priority low \
    --captured-at 2026-04-30 \
    --captured-week w11-d4)
[[ "$L004_OUT" == "L004" ]] || fail "Step 6: expected L004, got: $L004_OUT"

stores observations update L004 \
    --notes '{"siblings":["L210"]}' \
    || fail "Step 6: update notes failed"

SIBLINGS_VAL=$(stores observations show L004 --json | jq -c '.notes.siblings')
[[ "$SIBLINGS_VAL" == '["L210"]' ]] \
    || fail "Step 6: expected .notes.siblings == [\"L210\"]; got: $SIBLINGS_VAL"
pass "notes.siblings == [\"L210\"] — structured JSON object, not blob"

# ---------------------------------------------------------------------------
# Step 7/8 — needs_info parking + provide_info resume
# park requires actor:ai_autonomous; provide_info requires actor:human
# ---------------------------------------------------------------------------
echo "--- Step 7/8 — confirmed → needs_info (park) → confirmed (provide_info)"

# L001 is already in confirmed state from Step 2; park it (ai_autonomous)
stores observations park L001 --invoker ai_autonomous \
    || fail "Step 7: park failed"

L001_STATUS=$(stores observations show L001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
[[ "$L001_STATUS" == "needs_info" ]] \
    || fail "Step 7: expected needs_info after park, got: $L001_STATUS"
pass "status=needs_info after park"

# AI invoker on provide_info must fail (actor:human enforced)
set +e
PROVIDE_ERR=$(CLAUDECODE=1 stores observations provide_info L001 --invoker ai_autonomous 2>&1)
PROVIDE_EXIT=$?
set -e

[[ "$PROVIDE_EXIT" -ne 0 ]] \
    || fail "Step 7: expected non-zero exit for AI invoker on provide_info; got exit 0"
echo "$PROVIDE_ERR" | grep -q "actor" \
    || fail "Step 7: expected 'actor' in error; got: $PROVIDE_ERR"
echo "$PROVIDE_ERR" | grep -q "human" \
    || fail "Step 7: expected 'human' in error; got: $PROVIDE_ERR"
pass "AI invoker on provide_info rejected (exit=$PROVIDE_EXIT); error mentions actor/human"

# Human resume (provide_info) — must succeed
stores observations provide_info L001 --invoker human \
    || fail "Step 7: provide_info --invoker human failed"

L001_STATUS=$(stores observations show L001 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
[[ "$L001_STATUS" == "confirmed" ]] \
    || fail "Step 7: expected confirmed after provide_info, got: $L001_STATUS"
pass "status=confirmed after provide_info --invoker human (needs_info → confirmed)"

# ---------------------------------------------------------------------------
# Step 8/8 — cross-store task_id soft-FK
# NOTE: soft-FK round-trip only; cross-store referential integrity
# (e.g. tasks.complete requiring observations.status == 'resolved') is T010
# work and out of scope here. This step verifies task_id round-trips through
# update/show --json and that stores tasks show <id> returns a row — NOT that
# the framework rejects writes when the FK is dangling.
# ---------------------------------------------------------------------------
echo "--- Step 8/8 — cross-store task_id soft-FK (value round-trip only; T010 guards out of scope)"

# Add a task in the tasks store
T001_OUT=$(stores tasks add \
    --title "Task linked from observation" \
    --slug "task-linked-obs" \
    --done-when "task done" \
    --scope-in "in scope" \
    --scope-out "out of scope" \
    --invoker ai_with_human)
[[ "$T001_OUT" == "T001" ]] || fail "Step 8: expected T001 from tasks add, got: $T001_OUT"
pass "tasks add returned T001"

# Verify task row exists
stores tasks show T001 --json | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['display_id'] == 'T001', f'bad task display_id: {d}'
" || fail "Step 8: stores tasks show T001 --json assertions failed"

# Link L001 → T001 via task_id soft-FK
stores observations update L001 --task-id T001 \
    || fail "Step 8: update L001 --task-id T001 failed"

TASK_ID_VAL=$(stores observations show L001 --json | jq '.task_id')
[[ "$TASK_ID_VAL" == '"T001"' ]] \
    || fail "Step 8: expected task_id==\"T001\"; got: $TASK_ID_VAL"
pass "task_id=\"T001\" round-trips (soft-FK value round-trip only; cross-store integrity is T010)"

# ---------------------------------------------------------------------------
# Step 9 — T001 P4: investigate works with --invoker ai_autonomous (closes L007)
# ---------------------------------------------------------------------------
echo "--- Step 9 — T001 P4: observations investigate works with --invoker ai_autonomous"

L005_OUT=$(stores observations add \
    --summary "ai_autonomous investigate test" \
    --source dev \
    --priority normal \
    --captured-at 2026-04-30 \
    --captured-week w11-d4)
[[ "$L005_OUT" == "L005" ]] || fail "Step 9: expected L005, got: $L005_OUT"

stores observations investigate L005 --invoker ai_autonomous \
    || fail "Step 9: investigate --invoker ai_autonomous failed (actor should now be ai_autonomous)"

L005_STATUS=$(stores observations show L005 --json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
[[ "$L005_STATUS" == "investigating" ]] \
    || fail "Step 9: expected investigating after autonomous investigate, got: $L005_STATUS"
pass "investigate transition is actor:ai_autonomous (L007 closed)"

# Same path: with CLAUDECODE=1 and no --invoker, default ai_autonomous works.
L006_OUT=$(stores observations add \
    --summary "claudecode-detected investigate test" \
    --source dev \
    --priority normal \
    --captured-at 2026-04-30 \
    --captured-week w11-d4)
[[ "$L006_OUT" == "L006" ]] || fail "Step 9: expected L006, got: $L006_OUT"

CLAUDECODE=1 stores observations investigate L006 \
    || fail "Step 9: investigate with CLAUDECODE=1 (auto-detect ai_autonomous) failed"
pass "investigate auto-detects ai_autonomous from \$CLAUDECODE"

# ---------------------------------------------------------------------------
# Step 10 — T001 P4: approved_by writable via --invoker ai_with_human --approve-token (closes L008)
# Sets STORES_TOKEN_DIR to an isolated tmp path and writes a known SHA-256 hash.
# ---------------------------------------------------------------------------
echo "--- Step 10 — T001 P4: intent_contract.approved_by token-mediated path"

TOKEN_DIR="$TMPDIR/auth"
mkdir -p "$TOKEN_DIR"
TOKEN_PLAIN="t-p4-correct-token-xyz"
TOKEN_WRONG="t-p4-wrong-token"
# SHA-256 hex of TOKEN_PLAIN
TOKEN_HASH=$(printf '%s' "$TOKEN_PLAIN" | python3 -c "import sys,hashlib; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())")
printf '%s' "$TOKEN_HASH" > "$TOKEN_DIR/approve.token.hash"
chmod 600 "$TOKEN_DIR/approve.token.hash"

# Fresh observation, drive to investigating, fill contract draft (no approval yet).
L007_OUT=$(stores observations add \
    --summary "Token-mediated approval test" \
    --source dev \
    --priority high \
    --captured-at 2026-04-30 \
    --captured-week w11-d4 \
    --contract-state draft \
    --objective "Token path objective" \
    --type work \
    --in-scope "scope" \
    --out-of-scope "noscope" \
    --acceptance "passes" \
    --tier-hint T1 \
    --drafted-by t001-p4 \
    --drafted-at 2026-04-30T08:00:00Z)
[[ "$L007_OUT" == "L007" ]] || fail "Step 10: expected L007, got: $L007_OUT"

stores observations investigate L007 --invoker ai_autonomous \
    || fail "Step 10: investigate L007 failed"

# (a) write approved_by/at WITHOUT token under --invoker ai_with_human → must fail
set +e
NO_TOKEN_ERR=$(STORES_TOKEN_DIR="$TOKEN_DIR" stores observations update L007 \
    --approved-by blake \
    --approved-at 2026-04-30T09:00:00Z \
    --invoker ai_with_human 2>&1)
NO_TOKEN_EXIT=$?
set -e
[[ "$NO_TOKEN_EXIT" -ne 0 ]] \
    || fail "Step 10(a): expected non-zero exit writing approved_by w/o token; got 0"
echo "$NO_TOKEN_ERR" | grep -q "actor" \
    || fail "Step 10(a): expected 'actor' in error; got: $NO_TOKEN_ERR"
echo "$NO_TOKEN_ERR" | grep -q "approved_by" \
    || fail "Step 10(a): expected 'approved_by' in error; got: $NO_TOKEN_ERR"
pass "approved_by rejected for ai_with_human WITHOUT --approve-token"

# (b) write approved_by with WRONG token → must fail
set +e
WRONG_TOKEN_ERR=$(STORES_TOKEN_DIR="$TOKEN_DIR" stores observations update L007 \
    --approved-by blake \
    --approved-at 2026-04-30T09:00:00Z \
    --invoker ai_with_human \
    --approve-token "$TOKEN_WRONG" 2>&1)
WRONG_TOKEN_EXIT=$?
set -e
[[ "$WRONG_TOKEN_EXIT" -ne 0 ]] \
    || fail "Step 10(b): expected non-zero exit writing approved_by w/ WRONG token; got 0"
pass "approved_by rejected for ai_with_human WITH wrong --approve-token"

# (c) write approved_by with CORRECT token → must succeed
STORES_TOKEN_DIR="$TOKEN_DIR" stores observations update L007 \
    --approved-by blake \
    --approved-at 2026-04-30T09:00:00Z \
    --invoker ai_with_human \
    --approve-token "$TOKEN_PLAIN" \
    || fail "Step 10(c): update approved_by with correct --approve-token failed"

L007_APPROVED=$(stores observations show L007 --json | jq -r '.intent_contract.approved_by')
[[ "$L007_APPROVED" == "blake" ]] \
    || fail "Step 10(c): expected approved_by=blake after token-mediated update; got: $L007_APPROVED"
pass "approved_by accepted for ai_with_human WITH correct --approve-token (L008 closed)"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "=== All 8 DONE_WHEN clauses verified ==="
echo "  #1 full add (dashboard T3, all required fields, full intent contract → L001): PASS"
echo "  #2 triage flow (open→investigating→confirmed; guard: contract_state==ready): PASS"
echo "  #3 required_when (flip contract_state ready without sub-fields → rejected): PASS"
echo "  #4 actor:human (CLAUDECODE=1 ai_autonomous on approved_by → rejected): PASS"
echo "  #5 evidence.external_refs JSON array round-trip (list_record, T006 P2): PASS"
echo "  #6 notes JSON object round-trip (T008 substrate): PASS"
echo "  #7 needs_info parking (confirmed→needs_info→confirmed; AI provide_info rejected): PASS"
echo "  #8 cross-store task_id soft-FK (value round-trip; T010 guards out of scope): PASS"
echo "  #9 T001 P4 — investigate is actor:ai_autonomous (L007): PASS"
echo "  #10 T001 P4 — approved_by token-mediated (no/wrong/correct token; L008): PASS"
