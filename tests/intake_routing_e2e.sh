#!/usr/bin/env bash
# tests/intake_routing_e2e.sh — T053 Phase 3 end-to-end routing decision tests.
#
# Covers all six routing decisions (AC3.1):
#   1. duplicate        — sets duplicate_of; copies cluster_key; no obs created
#   2. fast_track       — creates obs with tags=[fast-track-eligible], state=open
#   3. normal_observation — creates obs with L143 columns (risk_class, approval_policy, etc.)
#   4. arch_review_candidate — creates obs with tags=[arch-review-candidate], pending flag
#   5. needs_info       — bumps recon_round via recon-return; enforces ≤ 2 cap
#   6. reject_noise     — lands in dropped (terminal); amend recovers to triaging
#
# AC3.2 (failed obs insert rolls back intake transition) is covered by the unit test
# handlers::intake_route::tests::failed_obs_insert_rolls_back_via_transaction.
#
# AC3.5 (amend invoker enforcement) is also covered in tests/intake_amend_recovery.rs.
#
# Usage: bash tests/intake_routing_e2e.sh
# Requires: stores binary on PATH, sqlite3

set -euo pipefail

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }
check_contains() { echo "$1" | grep -q "$2" || fail "$3: expected '$2' in: $1"; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "=== stores intake routing e2e (T053 Phase 3) ==="
echo "tmp dir: $TMP"
STORES_BIN="${STORES_BIN:-$STORES_ROOT/target/debug/stores}"
if [[ ! -x "$STORES_BIN" ]]; then
    STORES_BIN="$(command -v stores)"
fi
echo "stores binary: $STORES_BIN"
echo ""

cd "$TMP"

# ---------------------------------------------------------------------------
# Setup: init + install stores
# ---------------------------------------------------------------------------
echo "--- Setup: init + install stores"
"$STORES_BIN" init

"$STORES_BIN" install "$STORES_ROOT/stores/intake_items"
"$STORES_BIN" install "$STORES_ROOT/stores/observations"
"$STORES_BIN" install "$STORES_ROOT/stores/architecture_reviews"

sqlite3 .stores/db.sqlite ".tables" | grep -q "intake" || fail "intake table not created"
sqlite3 .stores/db.sqlite ".tables" | grep -q "observations" || fail "observations table not created"
sqlite3 .stores/db.sqlite ".tables" | grep -q "architecture_reviews" || fail "architecture_reviews table not created"
pass "stores installed"

# ---------------------------------------------------------------------------
# Helper: create + claim-triage an intake item
# ---------------------------------------------------------------------------
add_triaging_item() {
    local summary="$1"
    CLAUDECODE=1 "$STORES_BIN" intake add --invoker ai_autonomous \
        --summary "$summary" \
        --source-agent executor \
        --captured-at "2026-05-06T10:00:00Z" \
        --captured-week "w19-d2" \
        > /dev/null

    # Get the just-created item ID (highest I###)
    local id
    id=$(sqlite3 .stores/db.sqlite \
        "SELECT display_id FROM intake ORDER BY id DESC LIMIT 1")

    # Claim triage
    CLAUDECODE=1 "$STORES_BIN" intake claim-triage "$id" --invoker ai_autonomous > /dev/null

    echo "$id"
}

# ---------------------------------------------------------------------------
# Decision 1: duplicate
# ---------------------------------------------------------------------------
echo "--- Decision 1: duplicate"

# Create source item with a known cluster_key (set directly for test convenience)
CLAUDECODE=1 "$STORES_BIN" intake add --invoker ai_autonomous \
    --summary "source item for duplicate test" \
    --source-agent executor \
    --captured-at "2026-05-06T10:00:00Z" \
    --captured-week "w19-d2" \
    > /dev/null
SRC_ID=$(sqlite3 .stores/db.sqlite "SELECT display_id FROM intake ORDER BY id DESC LIMIT 1")
CLAUDECODE=1 "$STORES_BIN" intake claim-triage "$SRC_ID" --invoker ai_autonomous > /dev/null

# Route source as normal_observation so it gets a cluster_key
DEC_JSON_SRC='{"decision":"normal_observation","confidence":"high","rationale":"source","tier_hint":"T2","risk_flags":["small_local_fix"],"cluster_key":"dispatch-lifecycle"}'
CLAUDECODE=1 "$STORES_BIN" intake route "$SRC_ID" --invoker ai_autonomous \
    --decision normal_observation \
    --gatekeeper-decision-json "$DEC_JSON_SRC" \
    > /dev/null

SRC_CK=$(sqlite3 .stores/db.sqlite "SELECT cluster_key FROM intake WHERE display_id='$SRC_ID'")
[[ "$SRC_CK" == "dispatch-lifecycle" ]] || fail "source item cluster_key not set; got: $SRC_CK"

# Create duplicate item and route it as duplicate of the source
DUP_ID=$(add_triaging_item "duplicate of source item")

DEC_JSON_DUP='{"decision":"duplicate","confidence":"high","rationale":"already filed","cluster_key":"dispatch-lifecycle","duplicate_candidates":["'"$SRC_ID"'"]}'
CLAUDECODE=1 "$STORES_BIN" intake route "$DUP_ID" --invoker ai_autonomous \
    --decision duplicate \
    --duplicate-of "$SRC_ID" \
    --gatekeeper-decision-json "$DEC_JSON_DUP" \
    > /dev/null

DUP_STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$DUP_ID'")
DUP_CK=$(sqlite3 .stores/db.sqlite "SELECT cluster_key FROM intake WHERE display_id='$DUP_ID'")
DUP_OF=$(sqlite3 .stores/db.sqlite "SELECT duplicate_of FROM intake WHERE display_id='$DUP_ID'")

[[ "$DUP_STATUS" == "routed" ]] || fail "duplicate: expected status=routed; got: $DUP_STATUS"
[[ "$DUP_CK" == "dispatch-lifecycle" ]] || fail "duplicate: cluster_key not copied; got: $DUP_CK"
[[ "$DUP_OF" == "$SRC_ID" ]] || fail "duplicate: duplicate_of not set; got: $DUP_OF"
# Source item's normal_observation route auto-created 1 obs (per AC3.6); duplicate adds none
OBS_COUNT=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM observations")
[[ "$OBS_COUNT" == "1" ]] || fail "duplicate: only source's auto-obs should exist (count=1 expected); count=$OBS_COUNT"
pass "duplicate: status=routed, cluster_key copied, no new obs from duplicate route"

# ---------------------------------------------------------------------------
# Decision 2: fast_track
# ---------------------------------------------------------------------------
echo "--- Decision 2: fast_track"

FT_ID=$(add_triaging_item "fast track eligible item")

DEC_JSON_FT='{"decision":"fast_track","confidence":"high","rationale":"Tiny doc fix.","tier_hint":"T1","risk_flags":["docs_only"]}'
CLAUDECODE=1 "$STORES_BIN" intake route "$FT_ID" --invoker ai_autonomous \
    --decision fast_track \
    --gatekeeper-decision-json "$DEC_JSON_FT" \
    > /dev/null

FT_STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$FT_ID'")
FT_OBS_ID=$(sqlite3 .stores/db.sqlite "SELECT routed_to_observation FROM intake WHERE display_id='$FT_ID'")
[[ "$FT_STATUS" == "routed" ]] || fail "fast_track: expected status=routed; got: $FT_STATUS"
[[ -n "$FT_OBS_ID" ]] || fail "fast_track: routed_to_observation must be set"

# Verify obs row
FT_OBS_STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM observations WHERE display_id='$FT_OBS_ID'")
FT_OBS_TAGS=$(sqlite3 .stores/db.sqlite "SELECT tags FROM observations WHERE display_id='$FT_OBS_ID'")
FT_OBS_RES=$(sqlite3 .stores/db.sqlite "SELECT resolution FROM observations WHERE display_id='$FT_OBS_ID'")

[[ "$FT_OBS_STATUS" == "open" ]] || fail "fast_track obs: expected state=open; got: $FT_OBS_STATUS"
echo "$FT_OBS_TAGS" | grep -q "fast-track-eligible" || fail "fast_track obs: missing fast-track-eligible tag; got: $FT_OBS_TAGS"
[[ -z "$FT_OBS_RES" ]] || fail "fast_track obs: must NOT be resolved (no auto-execution); got: $FT_OBS_RES"
pass "fast_track: obs created with state=open, tag=fast-track-eligible, no resolution"

# ---------------------------------------------------------------------------
# Decision 3: normal_observation
# ---------------------------------------------------------------------------
echo "--- Decision 3: normal_observation"

NO_ID=$(add_triaging_item "stale pid files causing dispatch confusion")

DEC_JSON_NO='{"decision":"normal_observation","confidence":"medium","rationale":"Stale lock files.","tier_hint":"T2","risk_flags":["small_local_fix"],"cluster_key":"dispatch-lifecycle"}'
CLAUDECODE=1 "$STORES_BIN" intake route "$NO_ID" --invoker ai_autonomous \
    --decision normal_observation \
    --gatekeeper-decision-json "$DEC_JSON_NO" \
    > /dev/null

NO_STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$NO_ID'")
NO_OBS_ID=$(sqlite3 .stores/db.sqlite "SELECT routed_to_observation FROM intake WHERE display_id='$NO_ID'")
[[ "$NO_STATUS" == "routed" ]] || fail "normal_observation: expected status=routed; got: $NO_STATUS"
[[ -n "$NO_OBS_ID" ]] || fail "normal_observation: routed_to_observation must be set"

# AC3.6: verify L143 columns on the new observation
NO_RISK_CLASS=$(sqlite3 .stores/db.sqlite "SELECT risk_class FROM observations WHERE display_id='$NO_OBS_ID'")
NO_APPROVAL=$(sqlite3 .stores/db.sqlite "SELECT approval_policy FROM observations WHERE display_id='$NO_OBS_ID'")
NO_RISK_FLAGS=$(sqlite3 .stores/db.sqlite "SELECT risk_flags FROM observations WHERE display_id='$NO_OBS_ID'")
NO_CK=$(sqlite3 .stores/db.sqlite "SELECT cluster_key FROM observations WHERE display_id='$NO_OBS_ID'")

[[ "$NO_RISK_CLASS" == "low" ]] || fail "normal_observation: expected risk_class=low (small_local_fix); got: $NO_RISK_CLASS"
[[ "$NO_APPROVAL" == "auto" ]] || fail "normal_observation: expected approval_policy=auto (T2+low); got: $NO_APPROVAL"
echo "$NO_RISK_FLAGS" | grep -q "small_local_fix" || fail "normal_observation: risk_flags missing small_local_fix; got: $NO_RISK_FLAGS"
[[ "$NO_CK" == "dispatch-lifecycle" ]] || fail "normal_observation: cluster_key mismatch; got: $NO_CK"
pass "normal_observation: obs created, L143 columns populated (risk_class=$NO_RISK_CLASS, policy=$NO_APPROVAL)"

# ---------------------------------------------------------------------------
# Decision 4: arch_review_candidate
# ---------------------------------------------------------------------------
echo "--- Decision 4: arch_review_candidate"

AR_ID=$(add_triaging_item "proposed change to actor authority surface")

DEC_JSON_AR='{"decision":"arch_review_candidate","confidence":"high","rationale":"Touches actor authority — needs arch review.","tier_hint":"T3","risk_flags":["touches_actor_authority","authority_surface_drift"],"cluster_key":"actor-authority"}'

# arch_review_candidate is a Router outcome via the route verb; it creates L### + A###.
CLAUDECODE=1 "$STORES_BIN" intake route "$AR_ID" --invoker ai_autonomous \
    --decision arch_review_candidate \
    --gatekeeper-decision-json "$DEC_JSON_AR" \
    > /dev/null

AR_STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$AR_ID'")
AR_OBS_ID=$(sqlite3 .stores/db.sqlite "SELECT routed_to_observation FROM intake WHERE display_id='$AR_ID'")
AR_ARCH_ID=$(sqlite3 .stores/db.sqlite "SELECT routed_to_arch_review FROM intake WHERE display_id='$AR_ID'")
[[ "$AR_STATUS" == "routed" ]] || fail "arch_review_candidate: expected status=routed; got: $AR_STATUS"
[[ -n "$AR_OBS_ID" ]] || fail "arch_review_candidate: routed_to_observation must be set"
[[ -n "$AR_ARCH_ID" ]] || fail "arch_review_candidate: routed_to_arch_review must be set"

# AC4.1/AC4.3: verify obs pending flag and dedicated A### identity.
AR_OBS_PENDING=$(sqlite3 .stores/db.sqlite "SELECT pending_architecture_review FROM observations WHERE display_id='$AR_OBS_ID'")
AR_OBS_TAGS=$(sqlite3 .stores/db.sqlite "SELECT COALESCE(tags,'') FROM observations WHERE display_id='$AR_OBS_ID'")
AR_ARCH_ROW=$(sqlite3 .stores/db.sqlite "SELECT status || '|' || kind || '|' || source_observation || '|' || source_intake || '|' || cluster_key FROM architecture_reviews WHERE display_id='$AR_ARCH_ID'")
[[ "$AR_OBS_PENDING" == "1" ]] || fail "arch_review obs: pending_architecture_review not true; got: $AR_OBS_PENDING"
[[ "$AR_OBS_TAGS" != *"arch-review-candidate"* ]] || fail "arch_review obs: must not rely on legacy tag identity; got: $AR_OBS_TAGS"
[[ "$AR_ARCH_ROW" == "pending|interpret|$AR_OBS_ID|$AR_ID|actor-authority" ]] || fail "arch review row mismatch: $AR_ARCH_ROW"
pass "arch_review_candidate: L### source + A### review created in same route"

AR_BYPASS_ID=$(add_triaging_item "attempted caller supplied architecture review id")
ARCH_BEFORE=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews")
OBS_BEFORE=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM observations")
if CLAUDECODE=1 "$STORES_BIN" intake route "$AR_BYPASS_ID" --invoker ai_autonomous \
    --decision arch_review_candidate \
    --routed-to-arch-review A999 \
    --gatekeeper-decision-json "$DEC_JSON_AR" \
    >out 2>err; then
    fail "arch_review_candidate: caller-supplied routed_to_arch_review unexpectedly succeeded"
fi
grep -q "caller-supplied routed_to_arch_review" err || fail "arch_review_candidate: missing pre-supplied A### rejection"
AR_BYPASS_STATE=$(sqlite3 .stores/db.sqlite "SELECT status || '|' || COALESCE(routed_to_observation,'') || '|' || COALESCE(routed_to_arch_review,'') FROM intake WHERE display_id='$AR_BYPASS_ID'")
ARCH_AFTER=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews")
OBS_AFTER=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM observations")
[[ "$AR_BYPASS_STATE" == "triaging||" ]] || fail "arch_review_candidate: rejected bypass mutated intake: $AR_BYPASS_STATE"
[[ "$ARCH_AFTER" == "$ARCH_BEFORE" ]] || fail "arch_review_candidate: rejected bypass created A### rows"
[[ "$OBS_AFTER" == "$OBS_BEFORE" ]] || fail "arch_review_candidate: rejected bypass created L### rows"
pass "arch_review_candidate: caller-supplied A### rejected before side effects"

AR_OBS_BYPASS_ID=$(add_triaging_item "attempted caller supplied source observation id")
ARCH_BEFORE=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews")
OBS_BEFORE=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM observations")
FT_PENDING_BEFORE=$(sqlite3 .stores/db.sqlite "SELECT pending_architecture_review FROM observations WHERE display_id='$FT_OBS_ID'")
if CLAUDECODE=1 "$STORES_BIN" intake route "$AR_OBS_BYPASS_ID" --invoker ai_autonomous \
    --decision arch_review_candidate \
    --routed-to-observation "$FT_OBS_ID" \
    --gatekeeper-decision-json "$DEC_JSON_AR" \
    >out 2>err; then
    fail "arch_review_candidate: caller-supplied routed_to_observation unexpectedly succeeded"
fi
grep -q "caller-supplied routed_to_observation" err || fail "arch_review_candidate: missing pre-supplied L### rejection"
AR_OBS_BYPASS_STATE=$(sqlite3 .stores/db.sqlite "SELECT status || '|' || COALESCE(routed_to_observation,'') || '|' || COALESCE(routed_to_arch_review,'') FROM intake WHERE display_id='$AR_OBS_BYPASS_ID'")
ARCH_AFTER=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews")
OBS_AFTER=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM observations")
FT_PENDING_AFTER=$(sqlite3 .stores/db.sqlite "SELECT pending_architecture_review FROM observations WHERE display_id='$FT_OBS_ID'")
[[ "$AR_OBS_BYPASS_STATE" == "triaging||" ]] || fail "arch_review_candidate: rejected L### bypass mutated intake: $AR_OBS_BYPASS_STATE"
[[ "$ARCH_AFTER" == "$ARCH_BEFORE" ]] || fail "arch_review_candidate: rejected L### bypass created A### rows"
[[ "$OBS_AFTER" == "$OBS_BEFORE" ]] || fail "arch_review_candidate: rejected L### bypass created L### rows"
[[ "$FT_PENDING_AFTER" == "$FT_PENDING_BEFORE" ]] || fail "arch_review_candidate: rejected L### bypass marked preexisting observation pending"
pass "arch_review_candidate: caller-supplied L### rejected before side effects"

# ---------------------------------------------------------------------------
# Decision 5: needs_info (with recon-return loop)
# ---------------------------------------------------------------------------
echo "--- Decision 5: needs_info + recon-return"

NI_ID=$(add_triaging_item "ambiguous friction report")

DEC_JSON_NI='{"decision":"needs_info","confidence":"low","rationale":"Need reproduction steps.","missing_info_question":"What is the exact command that failed?"}'
CLAUDECODE=1 "$STORES_BIN" intake route "$NI_ID" --invoker ai_autonomous \
    --decision needs_info \
    --gatekeeper-decision-json "$DEC_JSON_NI" \
    > /dev/null

NI_STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$NI_ID'")
NI_MIQ=$(sqlite3 .stores/db.sqlite "SELECT missing_info_question FROM intake WHERE display_id='$NI_ID'")
[[ "$NI_STATUS" == "needs_info" ]] || fail "needs_info: expected status=needs_info; got: $NI_STATUS"
[[ "$NI_MIQ" == "What is the exact command that failed?" ]] || fail "needs_info: missing_info_question wrong; got: $NI_MIQ"

# Round 1: recon-return with evidence
CLAUDECODE=1 "$STORES_BIN" intake recon-return "$NI_ID" --invoker ai_autonomous \
    --evidence '{"kind":"repro","summary":"runs stores intake route","path":"src/handlers/transition.rs","line":460}' \
    > /dev/null

NI_ROUND=$(sqlite3 .stores/db.sqlite "SELECT recon_round FROM intake WHERE display_id='$NI_ID'")
NI_STATUS2=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$NI_ID'")
[[ "$NI_ROUND" == "1" ]] || fail "needs_info: recon_round must be 1 after first return; got: $NI_ROUND"
[[ "$NI_STATUS2" == "triaging" ]] || fail "needs_info: expected status=triaging after recon-return; got: $NI_STATUS2"
pass "needs_info: recon_round=1, status=triaging after first recon-return"

# Route needs_info again (second round)
CLAUDECODE=1 "$STORES_BIN" intake route "$NI_ID" --invoker ai_autonomous \
    --decision needs_info \
    --gatekeeper-decision-json "$DEC_JSON_NI" \
    > /dev/null

# Round 2: recon-return (recon_round should become 2 — the cap)
CLAUDECODE=1 "$STORES_BIN" intake recon-return "$NI_ID" --invoker ai_autonomous > /dev/null
NI_ROUND2=$(sqlite3 .stores/db.sqlite "SELECT recon_round FROM intake WHERE display_id='$NI_ID'")
[[ "$NI_ROUND2" == "2" ]] || fail "needs_info: recon_round must be 2 after second return; got: $NI_ROUND2"
pass "needs_info: recon_round=2 at cap limit"

# Route needs_info a third time, then recon-return should be REJECTED (cap=2 exceeded)
CLAUDECODE=1 "$STORES_BIN" intake route "$NI_ID" --invoker ai_autonomous \
    --decision needs_info \
    --gatekeeper-decision-json "$DEC_JSON_NI" \
    > /dev/null
NI_ERR_OUT=$(CLAUDECODE=1 "$STORES_BIN" intake recon-return "$NI_ID" --invoker ai_autonomous 2>&1 || true)
echo "$NI_ERR_OUT" | grep -qi "cap\|exceeded\|max" \
    || fail "needs_info: third recon-return should fail with cap error; got: $NI_ERR_OUT"
NI_STATUS_CAP=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$NI_ID'")
[[ "$NI_STATUS_CAP" == "needs_info" ]] || fail "needs_info: failed capped recon-return must roll back to needs_info; got: $NI_STATUS_CAP"
pass "needs_info: third recon-return correctly rejected (cap exceeded)"

# ---------------------------------------------------------------------------
# Decision 6: reject_noise → amend → triaging
# ---------------------------------------------------------------------------
echo "--- Decision 6: reject_noise + amend recovery"

RN_ID=$(add_triaging_item "duplicate log line appearing in output")

DEC_JSON_RN='{"decision":"reject_noise","confidence":"high","rationale":"Already-resolved symptom re-appearing."}'
CLAUDECODE=1 "$STORES_BIN" intake route "$RN_ID" --invoker ai_autonomous \
    --decision reject_noise \
    --gatekeeper-decision-json "$DEC_JSON_RN" \
    > /dev/null

RN_STATUS=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$RN_ID'")
[[ "$RN_STATUS" == "dropped" ]] || fail "reject_noise: expected status=dropped; got: $RN_STATUS"
pass "reject_noise: status=dropped (terminal)"

# AC3.5: ai_autonomous on amend must be rejected while the row is dropped
RN_AUTH_ERR=$(CLAUDECODE=1 "$STORES_BIN" intake amend "$RN_ID" --invoker ai_autonomous 2>&1 || true)
echo "$RN_AUTH_ERR" | grep -qi "actor\|autonomous\|permission\|unauthorized\|invoker" \
    || fail "amend with ai_autonomous should be rejected; got: $RN_AUTH_ERR"
RN_STILL_DROPPED=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$RN_ID'")
[[ "$RN_STILL_DROPPED" == "dropped" ]] || fail "failed ai_autonomous amend must leave status=dropped; got: $RN_STILL_DROPPED"
pass "amend --invoker ai_autonomous correctly rejected"

# AC3.5: amend with ai_with_human recovers dropped → triaging
"$STORES_BIN" intake amend "$RN_ID" --invoker ai_with_human > /dev/null

RN_AMENDED=$(sqlite3 .stores/db.sqlite "SELECT status FROM intake WHERE display_id='$RN_ID'")
[[ "$RN_AMENDED" == "triaging" ]] || fail "amend: expected status=triaging; got: $RN_AMENDED"
pass "reject_noise + amend: dropped → triaging recovered"

# ---------------------------------------------------------------------------
# AC3.6 recap: SELECT from the normal_observation obs row
# ---------------------------------------------------------------------------
echo "--- AC3.6 verify: L143 columns on routed observation"
AC36_RC=$(sqlite3 .stores/db.sqlite "SELECT risk_class FROM observations WHERE display_id='$NO_OBS_ID'")
AC36_AP=$(sqlite3 .stores/db.sqlite "SELECT approval_policy FROM observations WHERE display_id='$NO_OBS_ID'")
AC36_RF=$(sqlite3 .stores/db.sqlite "SELECT risk_flags FROM observations WHERE display_id='$NO_OBS_ID'")
AC36_CK=$(sqlite3 .stores/db.sqlite "SELECT cluster_key FROM observations WHERE display_id='$NO_OBS_ID'")

[[ -n "$AC36_RC" ]] || fail "AC3.6: risk_class must be populated; got empty"
[[ -n "$AC36_AP" ]] || fail "AC3.6: approval_policy must be populated; got empty"
[[ -n "$AC36_RF" ]] || fail "AC3.6: risk_flags must be populated; got empty"
[[ -n "$AC36_CK" ]] || fail "AC3.6: cluster_key must be populated; got empty"
pass "AC3.6: risk_class=$AC36_RC approval_policy=$AC36_AP cluster_key=$AC36_CK"

echo ""
echo "=== All intake routing e2e tests passed ==="
