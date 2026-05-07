#!/usr/bin/env bash
set -euo pipefail
unset CLAUDECODE 2>/dev/null || true

fail() { echo "FAIL: $*" >&2; exit 1; }
TMPDIR="${STORES_E2E_TMP:-$(mktemp -d /tmp/t084-obs-source-XXXXXX)}"
trap 'rm -rf "$TMPDIR"' EXIT
cd "$TMPDIR"
git init -q
stores setup >/dev/null 2>&1

stores observations add --help | grep -q -- '--source-env' || fail 'add help missing --source-env'
stores observations add --help | grep -q -- 'DEPRECATED' || fail 'add help missing DEPRECATED legacy wording'
stores observations update --help | grep -q -- '--source-id' || fail 'update help missing --source-id'
stores observations update --help | grep -q -- 'DEPRECATED' || fail 'update help missing DEPRECATED legacy wording'

L1=$(stores observations add --summary canonical-prod --source dashboard --priority normal --captured-at 2026-05-01 --captured-week w18-d1 --source-env prod --source-id P123)
[[ "$L1" == L001 ]] || fail "canonical add returned $L1"
stores observations show L001 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["source_env"]=="prod" and d["source_id"]=="P123", d'

LEGACY_OUT=$(stores observations add --summary legacy-sandbox --source dashboard --priority normal --captured-at 2026-05-01 --captured-week w18-d1 --sandbox-source-id S456 2>&1)
echo "$LEGACY_OUT" | grep -q 'deprecated' || fail "legacy add missing deprecation warning: $LEGACY_OUT"
stores observations show L002 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["source_env"]=="sandbox" and d["source_id"]=="S456", d'

stores observations update L001 --source-env sandbox --source-id S789
stores observations show L001 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["source_env"]=="sandbox" and d["source_id"]=="S789", d'

stores observations update L001 --prod-source-id P789 >/tmp/t084_legacy_update.out 2>&1
cat /tmp/t084_legacy_update.out | grep -q 'deprecated' || fail 'legacy update missing deprecation warning'
stores observations show L001 --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["source_env"]=="prod" and d["source_id"]=="P789", d'

set +e
ERR=$(stores observations add --summary bad --source dashboard --priority normal --captured-at 2026-05-01 --captured-week w18-d1 --source-env prod --prod-source-id P 2>&1)
EC=$?
set -e
[[ $EC -ne 0 ]] || fail 'canonical+legacy add conflict succeeded'
echo "$ERR" | grep -q -- '--source-env/--source-id' || fail "conflict error missing canonical surface: $ERR"

set +e
ERR=$(stores observations update L001 --prod-source-id P --sandbox-source-id S 2>&1)
EC=$?
set -e
[[ $EC -ne 0 ]] || fail 'prod+sandbox update conflict succeeded'
echo "$ERR" | grep -qi 'ambiguous' || fail "prod+sandbox error missing ambiguous: $ERR"

stores observations add --summary prod-filter --source dashboard --priority normal --captured-at 2026-05-01 --captured-week w18-d1 --source-env prod --source-id P123 >/dev/null
stores observations add --summary sandbox-filter --source dashboard --priority normal --captured-at 2026-05-01 --captured-week w18-d1 --source-env sandbox --source-id P123 >/dev/null
LIST=$(stores observations list --json --source-env prod --source-id P123)
echo "$LIST" | python3 -c 'import json,sys; rows=json.load(sys.stdin); assert any(r["summary"]=="prod-filter" for r in rows), rows; assert all(r.get("source_env")!="sandbox" for r in rows), rows'
ALIAS_LIST=$(stores observations list --json --prod-source-id P123 2>/tmp/t084_list_warn)
grep -q 'deprecated' /tmp/t084_list_warn || fail 'legacy list missing deprecation warning'
echo "$ALIAS_LIST" | python3 -c 'import json,sys; rows=json.load(sys.stdin); assert any(r["summary"]=="prod-filter" for r in rows), rows; assert all(r.get("source_env")!="sandbox" for r in rows), rows'

echo 'PASS observations source env e2e'
