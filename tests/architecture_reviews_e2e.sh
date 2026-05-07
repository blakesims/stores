#!/usr/bin/env bash
# T077 P2 architecture_reviews lifecycle smoke.
set -euo pipefail
unset CLAUDECODE 2>/dev/null || true

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

TMPDIR="${STORES_E2E_TMP:-$(mktemp -d /tmp/t077-arch-reviews-XXXXXX)}"
trap 'rm -rf "$TMPDIR"' EXIT
cd "$TMPDIR"

git init -q
export STORES_TOKEN_DIR="$TMPDIR/tokens"
mkdir -p "$STORES_TOKEN_DIR"
printf '%s' valid-token | sha256sum | awk '{print $1}' > "$STORES_TOKEN_DIR/approve.token.hash"

stores setup >/dev/null 2>&1

A001=$(stores architecture_reviews add --kind interpret --summary "interpret local fix" --invoker ai_with_human)
[[ "$A001" == "A001" ]] || fail "expected A001, got $A001"
stores architecture_reviews claim-review A001 --invoker ai_with_human >/dev/null
stores architecture_reviews issue-verdict A001 --kind interpret --verdict allow_local_fix --rationale x --invoker ai_with_human >/dev/null
stores architecture_reviews show A001 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="verdict_issued" and d["verdict_issued_at"]'
pass "interpret issue-verdict finalizes"

A002=$(stores architecture_reviews add --kind amend --summary "amend doctrine" --cascade-decisions '[{"target":"docs/heart-and-architect.md","decision":"update"}]' --invoker ai_with_human)
[[ "$A002" == "A002" ]] || fail "expected A002, got $A002"
stores architecture_reviews claim-review A002 --invoker ai_with_human >/dev/null
if stores architecture_reviews issue-verdict A002 --kind amend --verdict propose_doctrine_update --rationale x --invoker ai_with_human 2>err; then
  fail "amend without cascade_decisions unexpectedly succeeded"
fi
grep -q cascade_decisions err || fail "missing cascade_decisions error"
stores architecture_reviews issue-verdict A002 --kind amend --verdict propose_doctrine_update --rationale x --cascade-decisions '[{"target":"docs/heart-and-architect.md","decision":"update"}]' --invoker ai_with_human >/dev/null
stores architecture_reviews show A002 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="awaiting_human_ratification" and not d.get("ratified_at")'
if stores architecture_reviews ratify-amend A002 --invoker ai_with_human --approve-token valid-token 2>err; then
  fail "ai_with_human ratify-amend unexpectedly succeeded"
fi
grep -q "requires invoker actor human" err || fail "non-human rejection message missing"
stores architecture_reviews ratify-amend A002 --invoker human --approve-token valid-token >/dev/null
stores architecture_reviews show A002 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="verdict_issued" and d["ratified_at"]'
pass "amend ratification requires human token"

A003=$(stores architecture_reviews add --kind interpret --summary "superseding ruling" --invoker ai_with_human)
[[ "$A003" == "A003" ]] || fail "expected A003, got $A003"
stores architecture_reviews claim-review A003 --invoker ai_with_human >/dev/null
stores architecture_reviews issue-verdict A003 --kind interpret --verdict allow_local_fix --rationale x --supersedes A001 --invoker ai_with_human >/dev/null
stores architecture_reviews show A001 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="superseded"'
stores architecture_reviews show A003 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="verdict_issued"'
pass "supersedes marks prior only"

sqlite3 .stores/db.sqlite "INSERT INTO observations (id, display_id, status, created_at, updated_at, created_by, updated_by, summary, source, priority, captured_at, captured_week, tags, pending_architecture_review) VALUES (901, 'L901', 'open', 'now', 'now', 'ai_with_human', 'ai_with_human', 'legacy tagged arch candidate', 'dev', 'normal', 'now', 'w19-d2', '[\"arch-review-candidate\"]', 0)"
BEFORE=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews WHERE source_observation='L901'")
[[ "$BEFORE" == "0" ]] || fail "legacy seed unexpectedly had architecture review"
stores migrate --apply >/dev/null
AFTER_ONE=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews WHERE source_observation='L901'")
PENDING_ONE=$(sqlite3 .stores/db.sqlite "SELECT pending_architecture_review FROM observations WHERE display_id='L901'")
stores migrate --apply >/dev/null
AFTER_TWO=$(sqlite3 .stores/db.sqlite "SELECT COUNT(*) FROM architecture_reviews WHERE source_observation='L901'")
[[ "$AFTER_ONE" == "1" ]] || fail "backfill first run should create one A###; got $AFTER_ONE"
[[ "$PENDING_ONE" == "1" ]] || fail "backfill should set pending_architecture_review; got $PENDING_ONE"
[[ "$AFTER_TWO" == "1" ]] || fail "backfill second run should not duplicate; got $AFTER_TWO"
pass "idempotent backfill rerun creates one A### total"
