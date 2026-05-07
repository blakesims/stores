#!/usr/bin/env bash
# tests/external_review_e2e.sh — substrate-native external review lane E2E.
# Runs hermetic Rust E2E tests with codex shim runners.

set -euo pipefail

STORES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$STORES_ROOT"

cargo test --test external_review_acceptance external_review_e2e_ -- --nocapture

echo "external_review_e2e: PASS"
