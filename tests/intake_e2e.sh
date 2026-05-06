#!/usr/bin/env bash
# tests/intake_e2e.sh — T053 Phase 5 contract e2e.
# Covers: six routing decisions (delegates Phase 3 real-CLI script), direct
# observations add escape hatches, decision_metadata rationale persistence,
# reject_noise amend recovery, and L143 typed-column propagation.

set -euo pipefail

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STORES_BIN="${STORES_BIN:-$STORES_ROOT/target/debug/stores}"
if [[ ! -x "$STORES_BIN" ]]; then
    STORES_BIN="$(command -v stores)"
fi
export STORES_BIN

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

# (i), (iv), (v): exercise all six routing decisions, reject_noise→amend,
# and L143 typed columns with isolated .stores + real CLI invocations.
bash "$STORES_ROOT/tests/intake_routing_e2e.sh"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cd "$TMP"

"$STORES_BIN" init >/dev/null
"$STORES_BIN" install "$STORES_ROOT/stores/intake_items" >/dev/null
"$STORES_BIN" install "$STORES_ROOT/stores/observations" >/dev/null

# (ii): direct observations add remains available to both escape-hatch invokers.
AI_AUTO=$(CLAUDECODE=1 "$STORES_BIN" observations add --invoker ai_autonomous \
    --summary "direct autonomous observation escape hatch" \
    --source dev --priority normal \
    --captured-at "2026-05-06T10:00:00Z" --captured-week "w19-d2")
[[ "$AI_AUTO" =~ ^L[0-9]{3,}$ ]] || fail "ai_autonomous observations add returned: $AI_AUTO"

AI_WITH_HUMAN=$(CLAUDECODE=1 "$STORES_BIN" observations add --invoker ai_with_human \
    --summary "direct ai_with_human observation escape hatch" \
    --source dev --priority normal \
    --captured-at "2026-05-06T10:00:00Z" --captured-week "w19-d2")
[[ "$AI_WITH_HUMAN" =~ ^L[0-9]{3,}$ ]] || fail "ai_with_human observations add returned: $AI_WITH_HUMAN"
pass "direct observations add works for ai_autonomous and ai_with_human"

# Helper for a focused route row.
CLAUDECODE=1 "$STORES_BIN" intake add --invoker ai_autonomous \
    --summary "metadata and typed-column propagation probe" \
    --source-agent executor \
    --captured-at "2026-05-06T10:00:00Z" \
    --captured-week "w19-d2" >/dev/null
IID=$(sqlite3 .stores/db.sqlite "SELECT display_id FROM intake ORDER BY id DESC LIMIT 1")
CLAUDECODE=1 "$STORES_BIN" intake claim-triage "$IID" --invoker ai_autonomous >/dev/null

DEC_JSON='{"decision":"normal_observation","confidence":"medium","rationale":"Gatekeeper rationale persisted for audit.","tier_hint":"T2","risk_flags":["small_local_fix"],"cluster_key":"phase-five-probe"}'
CLAUDECODE=1 "$STORES_BIN" intake route "$IID" --invoker ai_autonomous \
    --decision normal_observation \
    --gatekeeper-decision-json "$DEC_JSON" >/dev/null

# (iii): decision_metadata captures rationale fields.
DM=$(sqlite3 .stores/db.sqlite "SELECT decision_metadata FROM intake WHERE display_id='$IID'")
echo "$DM" | grep -q "Gatekeeper rationale persisted for audit" || fail "decision_metadata missing rationale: $DM"
echo "$DM" | grep -q '"confidence":"medium"' || fail "decision_metadata missing confidence: $DM"
echo "$DM" | grep -q '"tier_hint":"T2"' || fail "decision_metadata missing tier_hint: $DM"
pass "decision_metadata captures rationale/confidence/tier_hint"

# (v): typed columns flow through to resulting observation.
OID=$(sqlite3 .stores/db.sqlite "SELECT routed_to_observation FROM intake WHERE display_id='$IID'")
RC=$(sqlite3 .stores/db.sqlite "SELECT risk_class FROM observations WHERE display_id='$OID'")
AP=$(sqlite3 .stores/db.sqlite "SELECT approval_policy FROM observations WHERE display_id='$OID'")
RF=$(sqlite3 .stores/db.sqlite "SELECT risk_flags FROM observations WHERE display_id='$OID'")
CK=$(sqlite3 .stores/db.sqlite "SELECT cluster_key FROM observations WHERE display_id='$OID'")
[[ "$RC" == "low" ]] || fail "risk_class expected low, got $RC"
[[ "$AP" == "auto" ]] || fail "approval_policy expected auto, got $AP"
echo "$RF" | grep -q "small_local_fix" || fail "risk_flags missing small_local_fix: $RF"
[[ "$CK" == "phase-five-probe" ]] || fail "cluster_key expected phase-five-probe, got $CK"
pass "L143 typed columns propagate to observation $OID"

echo "=== All T053 Phase 5 intake e2e checks passed ==="
