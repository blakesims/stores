#!/usr/bin/env bash
# Mock claude shim for T028 P5 hand-off tests.
#
# Asserts that the argv shape conforms to the spawn module's contract:
#   --append-system-prompt @<file>
#   --message <text>
#   --session-id <uuid>
#   [--resume]
#
# Side effects: writes the priming + initial-message contents into a
# `claude.received` file inside MOCK_CLAUDE_OUTDIR (so tests can assert
# what the side-car actually saw), and appends a fake jsonl session line
# into the runs/sidecar/<scope>-<id>.jsonl file.
#
# Env vars consumed:
#   MOCK_CLAUDE_OUTDIR — required; tests set this to a tempdir.
#   MOCK_CLAUDE_RUNS_DIR — required; the runs/sidecar dir to touch.
#   MOCK_CLAUDE_OBS_DRAFT_BODY — optional; if set, write the obs-draft JSON.

set -euo pipefail

if [[ -z "${MOCK_CLAUDE_OUTDIR:-}" ]]; then
  echo "MOCK_CLAUDE_OUTDIR not set" >&2
  exit 64
fi
if [[ -z "${MOCK_CLAUDE_RUNS_DIR:-}" ]]; then
  echo "MOCK_CLAUDE_RUNS_DIR not set" >&2
  exit 64
fi

mkdir -p "$MOCK_CLAUDE_OUTDIR"

# Record the full argv for downstream argv-shape assertions.
# NUL-separated so multi-line args (e.g. --message body) round-trip cleanly.
printf '%s\0' "$@" > "$MOCK_CLAUDE_OUTDIR/argv.txt"

# Walk the args and pluck the ones we care about.
priming_arg=""
message=""
session_id=""
resume=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --append-system-prompt)
      priming_arg="$2"
      shift 2
      ;;
    --message)
      message="$2"
      shift 2
      ;;
    --session-id)
      session_id="$2"
      shift 2
      ;;
    --resume)
      resume=1
      shift
      ;;
    *)
      shift
      ;;
  esac
done

# AC: --append-system-prompt @<file> exists and points at a real file.
if [[ -z "$priming_arg" ]]; then
  echo "missing --append-system-prompt" >&2
  exit 65
fi
priming_file="${priming_arg#@}"
if [[ ! -f "$priming_file" ]]; then
  echo "priming file '$priming_file' missing" >&2
  exit 66
fi
cp "$priming_file" "$MOCK_CLAUDE_OUTDIR/priming.txt"

# AC: --message must be non-empty and contain APPROVE_TOKEN= (the priming
# pipeline always inlines the token into the initial chat message).
if [[ -z "$message" ]]; then
  echo "missing --message" >&2
  exit 67
fi
printf '%s' "$message" > "$MOCK_CLAUDE_OUTDIR/message.txt"
if ! printf '%s' "$message" | grep -q "APPROVE_TOKEN="; then
  echo "message did not contain APPROVE_TOKEN=" >&2
  exit 68
fi

# AC: --session-id must be a non-empty token.
if [[ -z "$session_id" ]]; then
  echo "missing --session-id" >&2
  exit 69
fi
printf '%s' "$session_id" > "$MOCK_CLAUDE_OUTDIR/session-id.txt"
printf '%s' "$resume" > "$MOCK_CLAUDE_OUTDIR/resume.txt"

# Append a fake jsonl line into the runs dir to simulate a real session.
mkdir -p "$MOCK_CLAUDE_RUNS_DIR"
# We don't know the scope_key here, so glob-find the touched session file.
matching="$(find "$MOCK_CLAUDE_RUNS_DIR" -maxdepth 1 -name "*-${session_id}.jsonl" -print -quit)"
if [[ -n "$matching" ]]; then
  echo '{"role":"mock","msg":"hello"}' >> "$matching"
fi

# Optionally write an obs-draft JSON for the obs-draft scope.
if [[ -n "${MOCK_CLAUDE_OBS_DRAFT_BODY:-}" ]]; then
  printf '%s' "$MOCK_CLAUDE_OBS_DRAFT_BODY" \
    > "$MOCK_CLAUDE_RUNS_DIR/obs-draft-${session_id}.json"
fi

exit 0
