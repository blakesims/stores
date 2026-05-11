#!/usr/bin/env bash
# Guard agents against broad, incidental formatter churn in this repo.
# Used by Claude Code PreToolUse hooks and pi shellCommandPrefix.
set -euo pipefail

mode="${1:-shell}"
input="${2:-}"

if [[ "$mode" == "claude-pretool" ]]; then
  input="$(cat)"
  command="$(printf '%s' "$input" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("tool_input",{}).get("command", ""))' 2>/dev/null || true)"
else
  command="$input"
fi

# Explicit escape hatch for intentional formatting tasks.
if [[ "${ALLOW_AGENT_FORMAT:-}" == "1" || "${STORES_ALLOW_FORMAT_CHURN:-}" == "1" ]]; then
  exit 0
fi

# Normalize newlines so simple regexes catch multi-command bash snippets.
one_line="$(printf '%s' "$command" | tr '\n' ' ')"

block() {
  cat <<MSG
BLOCKED: broad formatter command is disabled for agents in stores.

This repo keeps getting polluted by incidental rustfmt/prettier churn across dozens of files.
Do not run whole-repo formatters unless the task is explicitly a formatting task.

If formatting is intentional and scoped, rerun with one of:
  ALLOW_AGENT_FORMAT=1 <command>
  STORES_ALLOW_FORMAT_CHURN=1 <command>

Prefer targeted edits over cargo fmt / rustfmt / prettier --write.
MSG
  exit 2
}

# cargo fmt, including toolchain-prefixed cargo +nightly fmt. Allow --check.
if [[ "$one_line" =~ (^|[;&|[:space:]])cargo([[:space:]]+\+[A-Za-z0-9._-]+)?[[:space:]]+fmt([[:space:]]|$) ]]; then
  if [[ ! "$one_line" =~ (^|[[:space:]])--check([[:space:]]|$) ]]; then
    block
  fi
fi

# direct rustfmt. Allow --check.
if [[ "$one_line" =~ (^|[;&|[:space:]])rustfmt([[:space:]]|$) ]]; then
  if [[ ! "$one_line" =~ (^|[[:space:]])--check([[:space:]]|$) ]]; then
    block
  fi
fi

# Common JS/doc formatters in write mode. Allow check/list modes.
if [[ "$one_line" =~ (^|[;&|[:space:]])((npx|pnpm|yarn|bun)[[:space:]]+)?prettier([[:space:]]|$) ]]; then
  if [[ "$one_line" =~ (^|[[:space:]])(--write|-w)([[:space:]]|$) ]] || [[ ! "$one_line" =~ (^|[[:space:]])(--check|--list-different)([[:space:]]|$) ]]; then
    block
  fi
fi

exit 0
