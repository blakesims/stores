#!/usr/bin/env bash
# Minimal mock claude shim — records argv only, no validation.
# Test 5.7 (token round-trip) reads the argv to verify that the operator's
# approval token was passed via --message (not via env), proving that the
# token round-trips through chat context.

set -euo pipefail

if [[ -z "${MOCK_CLAUDE_OUTDIR:-}" ]]; then
  echo "MOCK_CLAUDE_OUTDIR not set" >&2
  exit 64
fi

mkdir -p "$MOCK_CLAUDE_OUTDIR"
printf '%s\0' "$@" > "$MOCK_CLAUDE_OUTDIR/argv.txt"
exit 0
