#!/usr/bin/env bash
# tests/gate_e2e.sh — End-to-end scripted trace of the 6 T007 DONE_WHEN clauses.
#
# Clause 1: Full add with all production-shaped fields → G001
# Clause 2: Defer G001 --defer-until 2026-05-11 → status=deferred, defer_until set
# Clause 3: Resume G001 → status=pending; second resume (self-loop) → still pending
# Clause 4: CLAUDECODE=1 answer G001 (no --invoker) → non-zero exit, actor:human error
# Clause 5: answer G001 --invoker human → status=answered
# Clause 6: --options "yes" --options "no" (repeatable) and --options "yes|no" (pipe)
#            both produce JSON options=["yes","no"]
#
# Usage: bash tests/gate_e2e.sh
# Requires: stores binary on PATH, python3, jq

set -euo pipefail

# Unset CLAUDECODE for the whole script — steps that need it re-set it explicitly
unset CLAUDECODE 2>/dev/null || true

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; exit 1; }

TMPDIR="${STORES_E2E_TMP:-$(mktemp -d /tmp/t007-gate-port-XXXXXX)}"
trap 'rm -rf "$TMPDIR"' EXIT

echo "=== stores gate e2e (6 DONE_WHEN clauses) ==="
echo "tmp dir: $TMPDIR"
echo "stores binary: $(command -v stores)"
echo ""

cd "$TMPDIR"

# ---------------------------------------------------------------------------
# Setup: init + install gate store via setup (gate is a bundled store)
# ---------------------------------------------------------------------------
stores setup > /dev/null 2>&1
[[ -f .stores/db.sqlite ]] || fail "setup: .stores/db.sqlite not created"
sqlite3 .stores/db.sqlite ".tables" | grep -q "gate" || fail "setup: gate table not found"

# ---------------------------------------------------------------------------
# Step 1/6 — Full add with all production-shaped fields
# ---------------------------------------------------------------------------
echo "--- Step 1/6 — full add → G001"
G001_OUT=$(stores gate add \
    --type script \
    --one-liner "Backfill Stripe customer_ids on sub_subscriptions" \
    --task-ref T241 \
    --filed-by morning-check \
    --source converge \
    --command "psql -c \"UPDATE sub_subscriptions SET customer_id = ...\"" \
    --business-reason "Revenue tracking requires customer_id on every sub row" \
    --technical-detail "sub_subscriptions.customer_id was added post-migration; ~2400 rows NULL" \
    --implications "Stripe webhook matching fails for NULL rows; backfill is idempotent" \
    --priority high)
[[ "$G001_OUT" == "G001" ]] || fail "Step 1: expected G001, got: $G001_OUT"
pass "add returned G001"

G001_JSON=$(stores gate show G001 --json)
echo "$G001_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['display_id'] == 'G001', f'bad display_id: {d}'
assert d['status'] == 'pending', f'expected pending, got: {d[\"status\"]}'
assert d['type'] == 'script', f'expected script, got: {d[\"type\"]}'
assert d['one_liner'] == 'Backfill Stripe customer_ids on sub_subscriptions', f'bad one_liner: {d}'
assert d['task_ref'] == 'T241', f'bad task_ref: {d}'
assert d['filed_by'] == 'morning-check', f'bad filed_by: {d}'
assert d['source'] == 'converge', f'bad source: {d}'
assert d['priority'] == 'high', f'bad priority: {d}'
assert d['business_reason'] is not None, f'business_reason is null: {d}'
assert d['technical_detail'] is not None, f'technical_detail is null: {d}'
assert d['command'] is not None, f'command is null: {d}'
assert d['implications'] is not None, f'implications is null: {d}'
" || fail "Step 1: show --json field assertions failed"
pass "all 9 new fields present and correct in show --json"

# ---------------------------------------------------------------------------
# Step 2/6 — Defer G001 → status=deferred, defer_until=2026-05-11
# ---------------------------------------------------------------------------
echo "--- Step 2/6 — defer G001 --defer-until 2026-05-11"
stores gate defer G001 --defer-until 2026-05-11 --invoker ai_with_human \
    || fail "Step 2: defer command failed"

G001_DEFERRED=$(stores gate show G001 --json)
echo "$G001_DEFERRED" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'deferred', f'expected deferred, got: {d[\"status\"]}'
assert d['defer_until'] == '2026-05-11', f'expected defer_until=2026-05-11, got: {d[\"defer_until\"]}'
" || fail "Step 2: show --json post-defer assertions failed"
pass "status=deferred and defer_until=2026-05-11"

# ---------------------------------------------------------------------------
# Step 3/6 — Resume G001 → pending; second resume (self-loop) → still pending
# ---------------------------------------------------------------------------
echo "--- Step 3/6 — resume G001 (deferred → pending, then pending → pending)"
stores gate resume G001 --invoker ai_with_human \
    || fail "Step 3: first resume failed"

G001_RESUMED=$(stores gate show G001 --json)
echo "$G001_RESUMED" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'pending', f'expected pending after resume, got: {d[\"status\"]}'
" || fail "Step 3: status not pending after first resume"
pass "status=pending after deferred → pending resume"

# Second resume (self-loop: pending → pending) — must succeed (idempotent)
stores gate resume G001 --invoker ai_with_human \
    || fail "Step 3: second resume (self-loop) failed — idempotency broken"

G001_SELFLOOP=$(stores gate show G001 --json)
echo "$G001_SELFLOOP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'pending', f'expected pending after self-loop resume, got: {d[\"status\"]}'
" || fail "Step 3: status corrupted after self-loop resume"
pass "self-loop resume succeeded; status still pending"

# ---------------------------------------------------------------------------
# Step 4/6 — AI invoker rejected on answer (CLAUDECODE=1, no --invoker)
# ---------------------------------------------------------------------------
echo "--- Step 4/6 — CLAUDECODE=1 answer G001 (no --invoker) → non-zero exit"
set +e
ACTOR_ERR=$(CLAUDECODE=1 stores gate answer G001 --answer "yes" 2>&1)
ACTOR_EXIT=$?
set -e

[[ "$ACTOR_EXIT" -ne 0 ]] || fail "Step 4: expected non-zero exit for AI invoker on answer; got exit 0"
echo "$ACTOR_ERR" | grep -q "actor" || fail "Step 4: expected 'actor' in error; got: $ACTOR_ERR"
echo "$ACTOR_ERR" | grep -q "human" || fail "Step 4: expected 'human' in error; got: $ACTOR_ERR"
pass "CLAUDECODE=1 answer rejected (exit=$ACTOR_EXIT); error mentions actor and human"

# ---------------------------------------------------------------------------
# Step 5/6 — Human invoker accepts answer → status=answered
# ---------------------------------------------------------------------------
echo "--- Step 5/6 — answer G001 --invoker human → status=answered"
stores gate answer G001 --answer "yes" --invoker human \
    || fail "Step 5: human answer failed"

G001_ANSWERED=$(stores gate show G001 --json)
echo "$G001_ANSWERED" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['status'] == 'answered', f'expected answered, got: {d[\"status\"]}'
assert d['answer'] == 'yes', f'expected answer=yes, got: {d[\"answer\"]}'
" || fail "Step 5: show --json post-answer assertions failed"
pass "status=answered and answer=yes"

# ---------------------------------------------------------------------------
# Step 6/6 — Repeatable options: --options A --options B == --options A|B
# ---------------------------------------------------------------------------
echo "--- Step 6/6 — repeatable --options forms produce identical JSON arrays"

# G002: repeatable form --options "yes" --options "no"
G002_OUT=$(stores gate add \
    --type decision \
    --one-liner "Repeatable options test (multi-flag)" \
    --filed-by e2e-test \
    --source dev \
    --options "yes" \
    --options "no")
[[ "$G002_OUT" == "G002" ]] || fail "Step 6: expected G002 (repeatable), got: $G002_OUT"

G002_OPTIONS=$(stores gate show G002 --json | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin)['options']))")
[[ "$G002_OPTIONS" == '["yes", "no"]' ]] || fail "Step 6: G002 options expected [\"yes\",\"no\"], got: $G002_OPTIONS"
pass "G002 repeatable --options produced $G002_OPTIONS"

# G003: pipe form --options "yes|no"
G003_OUT=$(stores gate add \
    --type decision \
    --one-liner "Repeatable options test (pipe-form)" \
    --filed-by e2e-test \
    --source dev \
    --options "yes|no")
[[ "$G003_OUT" == "G003" ]] || fail "Step 6: expected G003 (pipe-form), got: $G003_OUT"

G003_OPTIONS=$(stores gate show G003 --json | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin)['options']))")
[[ "$G003_OPTIONS" == '["yes", "no"]' ]] || fail "Step 6: G003 options expected [\"yes\",\"no\"], got: $G003_OPTIONS"
pass "G003 pipe-form --options produced $G003_OPTIONS"

# Assert G002.options == G003.options via jq
G002_JQ=$(stores gate show G002 --json | jq -c '.options')
G003_JQ=$(stores gate show G003 --json | jq -c '.options')
[[ "$G002_JQ" == "$G003_JQ" ]] || fail "Step 6: options mismatch: G002=$G002_JQ G003=$G003_JQ"
pass "jq comparison: G002.options == G003.options ($G002_JQ)"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "=== All 6 DONE_WHEN clauses verified ==="
echo "  #1 full add (9 new fields + G001): PASS"
echo "  #2 defer G001 (deferred + defer_until=2026-05-11): PASS"
echo "  #3 resume (deferred→pending + self-loop idempotency): PASS"
echo "  #4 AI invoker rejected on answer: PASS"
echo "  #5 human invoker answer (answered): PASS"
echo "  #6 repeatable options both forms produce [\"yes\",\"no\"]: PASS"
