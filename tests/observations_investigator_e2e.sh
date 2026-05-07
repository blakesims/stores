#!/usr/bin/env bash
# tests/observations_investigator_e2e.sh — T065 P4 operator-pull investigator e2e.
# Verifies request-investigation → daemon subscriber → builtin investigator with
# STORES_INVESTIGATOR_CMD stubs; no Claude/network dependency.

set -euo pipefail

unset CLAUDECODE 2>/dev/null || true

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_AGENTS="$STORES_ROOT/tests/fixtures/agents.yaml"
STORES_BIN="${STORES_BIN:-stores}"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

[[ -f "$FIXTURE_AGENTS" ]] || fail "missing fixture: $FIXTURE_AGENTS"
command -v "$STORES_BIN" >/dev/null 2>&1 || fail "stores binary not found: $STORES_BIN"
command -v sqlite3 >/dev/null 2>&1 || fail "sqlite3 required"

TMP="${STORES_E2E_TMP:-$(mktemp -d /tmp/stores-obs-investigator-e2e-XXXXXX)}"
trap 'rm -rf "$TMP"' EXIT

DB=".stores/db.sqlite"

echo "=== stores observations investigator e2e (T065 P4) ==="
echo "tmp dir: $TMP"
echo "stores binary: $(command -v "$STORES_BIN")"
echo ""

add_open_obs() {
    local summary="$1"
    local body="$2"
    "$STORES_BIN" observations add \
        --summary "$summary" \
        --body "$body" \
        --source dev \
        --priority normal \
        --captured-at 2026-05-06 \
        --captured-week w19-d3
}

init_case() {
    local name="$1"
    local dir="$TMP/$name"
    mkdir -p "$dir"
    cd "$dir"
    git init -q
    "$STORES_BIN" setup > /dev/null 2>&1
    cp "$FIXTURE_AGENTS" .stores/agents.yaml
    [[ -f "$DB" ]] || fail "$name: .stores/db.sqlite not created"

    # Current daemon semantics draw a starting-line for a brand-new subscriber.
    # Seed one historical investigator transition so subsequent requests in
    # this e2e are treated as live work rather than skip-historical backlog.
    local seed_id seed_lock
    seed_id=$(add_open_obs "$name starting-line seed" "seed body")
    "$STORES_BIN" observations request-investigation "$seed_id" --invoker ai_with_human > /dev/null
    STORES_INVESTIGATOR_CMD="printf '{}'" "$STORES_BIN" agents run --once --poll-interval 0.1 > "$TMP/$name-seed-daemon.log" 2>&1 \
        || fail "$name: starting-line seed daemon failed: $(cat "$TMP/$name-seed-daemon.log")"
    seed_lock=$(sqlite3 "$DB" "SELECT last_status FROM dispatch_locks WHERE display_id='$seed_id' AND agent_name='investigator'")
    [[ "$seed_lock" == "skip-historical" ]] || fail "$name: expected seed lock skip-historical; got $seed_lock"
}

transition_path() {
    local id="$1"
    sqlite3 "$DB" "SELECT from_status || '->' || to_status FROM transition_history WHERE store='observations' AND display_id='$id' ORDER BY id"
}

# ---------------------------------------------------------------------------
# Success branch: open → needs_investigation → investigating → investigated.
# ---------------------------------------------------------------------------
echo "--- success: request-investigation dispatches investigator stub"
init_case success

cat > "$TMP/success-stub.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat > "${STORES_DISPLAY_ID}.question"
case "$(cat "${STORES_DISPLAY_ID}.question")" in
  *"Summary:"*"success summary"*"Body:"*"success body details"*) ;;
  *) echo "question missing summary/body" >&2; exit 17 ;;
esac
cat <<'JSON'
{"evidence":["canned report text: checkout handler panic reproduced",{"file":"src/checkout.rs","line":42,"snippet":"unwrap on missing field"}],"duplicate_candidates":[],"confidence":"high","proposed_tier":"T2","grill_question":"Which field type is missing?"}
JSON
SH
chmod +x "$TMP/success-stub.sh"

SUCCESS_ID=$(add_open_obs "success summary" "success body details")
[[ -n "$SUCCESS_ID" ]] || fail "success: add returned empty id"
"$STORES_BIN" observations request-investigation "$SUCCESS_ID" --invoker ai_with_human
STATUS=$(sqlite3 "$DB" "SELECT status FROM observations WHERE display_id='$SUCCESS_ID'")
[[ "$STATUS" == "needs_investigation" ]] || fail "success: expected needs_investigation after request; got $STATUS"

STORES_INVESTIGATOR_CMD="$TMP/success-stub.sh" "$STORES_BIN" agents run --once --poll-interval 0.1 > "$TMP/success-daemon.log" 2>&1 \
    || fail "success: daemon failed: $(cat "$TMP/success-daemon.log")"

STATUS=$(sqlite3 "$DB" "SELECT status FROM observations WHERE display_id='$SUCCESS_ID'")
[[ "$STATUS" == "investigated" ]] || fail "success: expected investigated; got $STATUS"
PATH_ACTUAL=$(transition_path "$SUCCESS_ID" | paste -sd ',' -)
[[ "$PATH_ACTUAL" == *"open->needs_investigation,needs_investigation->investigating,investigating->investigated"* ]] \
    || fail "success: transition path missing ordered edges; got $PATH_ACTUAL"
NOTE=$(sqlite3 "$DB" "SELECT investigation_note FROM observations WHERE display_id='$SUCCESS_ID'")
echo "$NOTE" | grep -q "canned report text: checkout handler panic reproduced" \
    || fail "success: investigation_note missing canned report; got $NOTE"
echo "$NOTE" | grep -q "Evidence:" \
    || fail "success: investigation_note not human-readable; got $NOTE"
echo "$NOTE" | grep -q '^{"' && fail "success: investigation_note is raw JSON blob: $NOTE"
LOCK_STATUS=$(sqlite3 "$DB" "SELECT last_status FROM dispatch_locks WHERE store='observations' AND display_id='$SUCCESS_ID' AND agent_name='investigator'")
[[ "$LOCK_STATUS" == "ok" ]] || fail "success: expected dispatch_locks last_status=ok; got $LOCK_STATUS"
pass "success: transitions ordered; status=investigated; human-readable canned report persisted"

# ---------------------------------------------------------------------------
# Failure branch: nonzero stub → investigation_failed with visible reason + lock.
# ---------------------------------------------------------------------------
echo "--- failure: nonzero investigator stub is visible"
init_case failure

cat > "$TMP/fail-stub.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
printf 'rate_limit: canned failure from stub\n' >&2
exit 29
SH
chmod +x "$TMP/fail-stub.sh"

FAIL_ID=$(add_open_obs "failure summary" "failure body details")
[[ -n "$FAIL_ID" ]] || fail "failure: add returned empty id"
"$STORES_BIN" observations request-investigation "$FAIL_ID" --invoker ai_with_human

set +e
STORES_INVESTIGATOR_CMD="$TMP/fail-stub.sh" "$STORES_BIN" agents run --once --poll-interval 0.1 > "$TMP/failure-daemon.log" 2>&1
DAEMON_RC=$?
set -e
[[ "$DAEMON_RC" -eq 0 ]] || fail "failure: daemon should record subscriber failure and continue; rc=$DAEMON_RC log=$(cat "$TMP/failure-daemon.log")"
STATUS=$(sqlite3 "$DB" "SELECT status FROM observations WHERE display_id='$FAIL_ID'")
[[ "$STATUS" == "investigation_failed" ]] || fail "failure: expected investigation_failed; got $STATUS"
REASON=$(sqlite3 "$DB" "SELECT investigation_failure_reason FROM observations WHERE display_id='$FAIL_ID'")
[[ -n "$REASON" ]] || fail "failure: investigation_failure_reason empty"
echo "$REASON" | grep -q "rate_limit: canned failure from stub" \
    || fail "failure: reason missing stub stderr; got $REASON"
LOCK_STATUS=$(sqlite3 "$DB" "SELECT last_status FROM dispatch_locks WHERE store='observations' AND display_id='$FAIL_ID' AND agent_name='investigator'")
[[ -n "$LOCK_STATUS" && "$LOCK_STATUS" != "ok" ]] \
    || fail "failure: dispatch_locks must expose non-ok status; got $LOCK_STATUS"
echo "$LOCK_STATUS" | grep -q "rate_limit: canned failure from stub" \
    || fail "failure: dispatch_locks status missing failure detail; got $LOCK_STATUS"
pass "failure: status=investigation_failed; reason and non-ok dispatch_locks visible"

# ---------------------------------------------------------------------------
# Direct-add branch: add remains open; no lock until operator request.
# ---------------------------------------------------------------------------
echo "--- direct-add: add does not trigger investigator before request"
init_case direct_add

DIRECT_ID=$(add_open_obs "direct add summary" "direct add body")
[[ -n "$DIRECT_ID" ]] || fail "direct-add: add returned empty id"
STATUS=$(sqlite3 "$DB" "SELECT status FROM observations WHERE display_id='$DIRECT_ID'")
[[ "$STATUS" == "open" ]] || fail "direct-add: expected open; got $STATUS"
STORES_INVESTIGATOR_CMD="$TMP/success-stub.sh" "$STORES_BIN" agents run --once --poll-interval 0.1 > "$TMP/direct-pre-daemon.log" 2>&1 \
    || fail "direct-add: pre-request daemon failed: $(cat "$TMP/direct-pre-daemon.log")"
LOCK_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM dispatch_locks WHERE store='observations' AND display_id='$DIRECT_ID' AND agent_name='investigator'")
[[ "$LOCK_COUNT" == "0" ]] || fail "direct-add: expected no investigator lock before request; got $LOCK_COUNT"
"$STORES_BIN" observations request-investigation "$DIRECT_ID" --invoker ai_with_human
LOCK_COUNT_AFTER_REQUEST_BEFORE_DAEMON=$(sqlite3 "$DB" "SELECT COUNT(*) FROM dispatch_locks WHERE store='observations' AND display_id='$DIRECT_ID' AND agent_name='investigator'")
[[ "$LOCK_COUNT_AFTER_REQUEST_BEFORE_DAEMON" == "0" ]] \
    || fail "direct-add: request should not create lock before daemon claim; got $LOCK_COUNT_AFTER_REQUEST_BEFORE_DAEMON"
pass "direct-add: open add creates no investigator dispatch_locks row before request/daemon claim"

echo ""
echo "=== observations investigator e2e: all checks passed ==="
