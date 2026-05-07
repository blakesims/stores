#!/usr/bin/env bash
# T077 P5 architecture_reviews end-to-end scenarios.
set -euo pipefail
unset CLAUDECODE 2>/dev/null || true

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STORES_BIN="${STORES_BIN:-$STORES_ROOT/target/debug/stores}"
if [[ ! -x "$STORES_BIN" ]]; then
  STORES_BIN="$(command -v stores)"
fi

TMPDIR="${STORES_E2E_TMP:-$(mktemp -d /tmp/t077-arch-reviews-XXXXXX)}"
trap 'rm -rf "$TMPDIR"' EXIT
cd "$TMPDIR"

git init -q
export STORES_TOKEN_DIR="$TMPDIR/tokens"
mkdir -p "$STORES_TOKEN_DIR"
printf '%s' valid-token | sha256sum | awk '{print $1}' > "$STORES_TOKEN_DIR/approve.token.hash"

"$STORES_BIN" setup >/dev/null 2>&1

add_ready_observation() {
  local summary="$1"
  "$STORES_BIN" observations add \
    --summary "$summary" \
    --source dev \
    --priority normal \
    --captured-at "2026-05-07T10:00:00Z" \
    --captured-week "w19-d4" \
    --contract-state ready \
    --drafted-by e2e \
    --drafted-at "2026-05-07T10:00:00Z" \
    --objective "$summary" \
    --type work \
    --in-scope fix \
    --out-of-scope none \
    --acceptance pass \
    --tier-hint T1 \
    --approved-by blake \
    --approved-at "2026-05-07T10:00:00Z" \
    --invoker human >/dev/null
  sqlite3 .stores/db.sqlite "SELECT display_id FROM observations ORDER BY id DESC LIMIT 1"
}

add_triaging_item() {
  local summary="$1"
  CLAUDECODE=1 "$STORES_BIN" intake add --invoker ai_autonomous \
    --summary "$summary" \
    --source-agent executor \
    --captured-at "2026-05-07T10:00:00Z" \
    --captured-week "w19-d4" >/dev/null
  local id
  id=$(sqlite3 .stores/db.sqlite "SELECT display_id FROM intake ORDER BY id DESC LIMIT 1")
  CLAUDECODE=1 "$STORES_BIN" intake claim-triage "$id" --invoker ai_autonomous >/dev/null
  echo "$id"
}

# lifecycle-interpret + render idempotence
A001=$("$STORES_BIN" architecture-reviews add --kind interpret --summary "interpret local fix" --invoker ai_with_human)
[[ "$A001" == "A001" ]] || fail "lifecycle-interpret: expected A001, got $A001"
"$STORES_BIN" architecture-reviews claim-review A001 --invoker ai_with_human >/dev/null
"$STORES_BIN" architecture-reviews issue-verdict A001 --kind interpret --verdict allow_local_fix --rationale x --invoker ai_with_human >/dev/null
"$STORES_BIN" architecture-reviews show A001 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="verdict_issued" and d["verdict_issued_at"]'
"$STORES_BIN" architecture-reviews render A001 >/dev/null 2>&1
sha1_before=$(sha256sum architecture-reviews/A001/main.md | awk '{print $1}')
"$STORES_BIN" architecture-reviews render A001 >/dev/null 2>&1
sha1_after=$(sha256sum architecture-reviews/A001/main.md | awk '{print $1}')
[[ "$sha1_before" == "$sha1_after" ]] || fail "lifecycle-interpret: render was not idempotent"
pass "lifecycle-interpret"

# lifecycle-amend-ratify + cascade-decisions-schema
A002=$("$STORES_BIN" architecture-reviews add --kind amend --summary "amend doctrine" --cascade-decisions '[{"target":"docs/heart-and-architect.md","decision":"update"}]' --invoker ai_with_human)
"$STORES_BIN" architecture-reviews claim-review A002 --invoker ai_with_human >/dev/null
if "$STORES_BIN" architecture-reviews issue-verdict A002 --kind amend --verdict propose_doctrine_update --rationale x --cascade-decisions '[{"target":"docs/heart-and-architect.md","decision":"bogus"}]' --invoker ai_with_human 2>err; then
  fail "cascade-decisions-schema: invalid decision unexpectedly succeeded"
fi
grep -q "cascade_decisions" err || fail "cascade-decisions-schema: missing cascade_decisions error"
pass "cascade-decisions-schema"
"$STORES_BIN" architecture-reviews issue-verdict A002 --kind amend --verdict propose_doctrine_update --rationale x --cascade-decisions '[{"target":"docs/heart-and-architect.md","decision":"update"}]' --invoker ai_with_human >/dev/null
"$STORES_BIN" architecture-reviews show A002 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="awaiting_human_ratification" and not d.get("ratified_at")'
if "$STORES_BIN" architecture-reviews ratify-amend A002 --invoker ai_with_human --approve-token valid-token 2>err; then
  fail "lifecycle-amend-ratify: ai_with_human ratify-amend unexpectedly succeeded"
fi
grep -q "requires invoker actor human" err || fail "lifecycle-amend-ratify: non-human rejection message missing"
"$STORES_BIN" architecture-reviews ratify-amend A002 --invoker human --approve-token valid-token >/dev/null
"$STORES_BIN" architecture-reviews show A002 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="verdict_issued" and d["ratified_at"]'
pass "lifecycle-amend-ratify"

# supersedes-terminal
A003=$("$STORES_BIN" architecture-reviews add --kind interpret --summary "superseding ruling" --invoker ai_with_human)
"$STORES_BIN" architecture-reviews claim-review A003 --invoker ai_with_human >/dev/null
"$STORES_BIN" architecture-reviews issue-verdict A003 --kind interpret --verdict allow_local_fix --rationale x --supersedes A001 --invoker ai_with_human >/dev/null
[[ "$(sqlite3 .stores/db.sqlite "SELECT status FROM architecture_reviews WHERE display_id='A001'")" == "superseded" ]] || fail "supersedes-terminal: A001 not superseded"
pass "supersedes-terminal"

# pre-ratification-gate
L_GATE=$(add_ready_observation "pending gate observation")
python3 - <<PY
import sqlite3
conn=sqlite3.connect('.stores/db.sqlite')
conn.execute("UPDATE observations SET pending_architecture_review=1 WHERE display_id=?", ('$L_GATE',))
conn.commit()
PY
"$STORES_BIN" observations investigate "$L_GATE" --invoker ai_autonomous >/dev/null
python3 - <<PY
import sqlite3
conn=sqlite3.connect('.stores/db.sqlite')
conn.execute("UPDATE observations SET pending_architecture_review=1 WHERE display_id=?", ('$L_GATE',))
conn.commit()
PY
if "$STORES_BIN" observations confirm "$L_GATE" --invoker ai_with_human 2>err; then
  fail "pre-ratification-gate: confirm unexpectedly cleared pending architecture gate"
fi
grep -q "pending_architecture_review=true blocks U1 ratification" err || fail "pre-ratification-gate: missing gate error"
pass "pre-ratification-gate"

# gatekeeper-router-migration
AR_ID=$(add_triaging_item "proposed actor authority change")
DEC_JSON_AR='{"decision":"arch_review_candidate","confidence":"high","rationale":"Touches actor authority.","tier_hint":"T3","risk_flags":["touches_actor_authority"],"cluster_key":"actor-authority"}'
CLAUDECODE=1 "$STORES_BIN" intake route "$AR_ID" --invoker ai_autonomous --decision arch_review_candidate --gatekeeper-decision-json "$DEC_JSON_AR" >/dev/null
AR_OBS_ID=$(sqlite3 .stores/db.sqlite "SELECT routed_to_observation FROM intake WHERE display_id='$AR_ID'")
AR_ARCH_ID=$(sqlite3 .stores/db.sqlite "SELECT routed_to_arch_review FROM intake WHERE display_id='$AR_ID'")
AR_ROW=$(sqlite3 .stores/db.sqlite "SELECT status || '|' || kind || '|' || source_observation || '|' || source_intake || '|' || cluster_key FROM architecture_reviews WHERE display_id='$AR_ARCH_ID'")
[[ "$(sqlite3 .stores/db.sqlite "SELECT pending_architecture_review FROM observations WHERE display_id='$AR_OBS_ID'")" == "1" ]] || fail "gatekeeper-router-migration: observation not pending"
[[ "$AR_ROW" == "pending|interpret|$AR_OBS_ID|$AR_ID|actor-authority" ]] || fail "gatekeeper-router-migration: architecture review mismatch: $AR_ROW"
pass "gatekeeper-router-migration"

# reframe-reconciliation
L_REF=$(add_ready_observation "reframe contract observation")
A_REF=$("$STORES_BIN" architecture-reviews add --kind interpret --source-observation "$L_REF" --summary "reframe contract" --invoker ai_with_human)
"$STORES_BIN" architecture-reviews claim-review "$A_REF" --invoker ai_with_human >/dev/null
"$STORES_BIN" architecture-reviews issue-verdict "$A_REF" --kind interpret --verdict reframe_contract --rationale x --invoker ai_with_human >/dev/null
python3 - <<PY
import sqlite3
conn=sqlite3.connect('.stores/db.sqlite')
conn.execute("UPDATE observations SET pending_architecture_review=1, clearable_by_ruling=? WHERE display_id=?", ('$A_REF','$L_REF'))
conn.commit()
PY
"$STORES_BIN" observations investigate "$L_REF" --invoker ai_autonomous >/dev/null
python3 - <<PY
import sqlite3
conn=sqlite3.connect('.stores/db.sqlite')
conn.execute("UPDATE observations SET pending_architecture_review=1, clearable_by_ruling=? WHERE display_id=?", ('$A_REF','$L_REF'))
conn.commit()
PY
if "$STORES_BIN" observations confirm "$L_REF" --invoker ai_with_human 2>err; then
  fail "reframe-reconciliation: confirm unexpectedly succeeded without acknowledgement"
fi
grep -q "reframe_contract requires reframe_acknowledged_against" err || fail "reframe-reconciliation: missing ack error"
python3 - <<PY
import json, sqlite3
conn=sqlite3.connect('.stores/db.sqlite')
raw=conn.execute("SELECT intent_contract FROM observations WHERE display_id=?", ('$L_REF',)).fetchone()[0]
contract=json.loads(raw)
contract['updated_at']='2099-01-01T00:00:00Z'
conn.execute("UPDATE observations SET reframe_acknowledged_against=?, intent_contract=? WHERE display_id=?", ('$A_REF', json.dumps(contract), '$L_REF'))
conn.commit()
PY
"$STORES_BIN" observations confirm "$L_REF" --invoker ai_with_human >/dev/null
[[ "$(sqlite3 .stores/db.sqlite "SELECT pending_architecture_review FROM observations WHERE display_id='$L_REF'")" == "0" ]] || fail "reframe-reconciliation: pending flag not cleared"
pass "reframe-reconciliation"

# merge-redirect
L_SRC=$("$STORES_BIN" observations add --summary "merge source" --source dev --priority normal --captured-at "2026-05-07T10:00:00Z" --captured-week "w19-d4" --invoker ai_with_human)
L_TGT=$("$STORES_BIN" observations add --summary "merge target" --source dev --priority normal --captured-at "2026-05-07T10:00:00Z" --captured-week "w19-d4" --invoker ai_with_human)
python3 - <<PY
import sqlite3
conn=sqlite3.connect('.stores/db.sqlite')
conn.execute("UPDATE observations SET pending_architecture_review=1 WHERE display_id=?", ('$L_SRC',))
conn.commit()
PY
A_MERGE=$("$STORES_BIN" architecture-reviews add --kind interpret --source-observation "$L_SRC" --merge-target-id "$L_TGT" --summary "merge source into target" --invoker ai_with_human)
"$STORES_BIN" architecture-reviews claim-review "$A_MERGE" --invoker ai_with_human >/dev/null
"$STORES_BIN" architecture-reviews issue-verdict "$A_MERGE" --kind interpret --verdict merge_with_cluster --rationale x --source-observation "$L_SRC" --merge-target-id "$L_TGT" --invoker ai_with_human >/dev/null
MERGE_ROW=$(sqlite3 .stores/db.sqlite "SELECT status || '|' || pending_architecture_review || '|' || resolved_by || '|' || merge_target_id || '|' || resolution_kind FROM observations WHERE display_id='$L_SRC'")
[[ "$MERGE_ROW" == "resolved|0|$A_MERGE|$L_TGT|merged_with_cluster" ]] || fail "merge-redirect: source row mismatch: $MERGE_ROW"
pass "merge-redirect"

# idempotent-backfill-rerun
sqlite3 .stores/db.sqlite "INSERT INTO observations (id, display_id, status, created_at, updated_at, created_by, updated_by, summary, source, priority, captured_at, captured_week, tags, pending_architecture_review) VALUES (901, 'L901', 'open', 'now', 'now', 'ai_with_human', 'ai_with_human', 'legacy tagged arch candidate', 'dev', 'normal', 'now', 'w19-d4', '[\"arch-review-candidate\"]', 0)"
"$STORES_BIN" migrate --apply >/dev/null
AFTER_ONE=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews WHERE source_observation='L901'")
PENDING_ONE=$(sqlite3 .stores/db.sqlite "SELECT pending_architecture_review FROM observations WHERE display_id='L901'")
"$STORES_BIN" migrate --apply >/dev/null
AFTER_TWO=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews WHERE source_observation='L901'")
[[ "$AFTER_ONE" == "1" ]] || fail "idempotent-backfill-rerun: first run count $AFTER_ONE"
[[ "$PENDING_ONE" == "1" ]] || fail "idempotent-backfill-rerun: pending flag $PENDING_ONE"
[[ "$AFTER_TWO" == "1" ]] || fail "idempotent-backfill-rerun: second run count $AFTER_TWO"
pass "idempotent-backfill-rerun"
