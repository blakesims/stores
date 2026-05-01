#!/usr/bin/env bash
# new-note.sh — create a worklog note in the project's documentation system.
#
# Reads .notes-config.yml at the project root (walks up from CWD to find it).
# Supports flat / daily / weekly granularity configured via .notes-config.yml.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  new-note.sh <slug>          Create a detailed note (auto-increments sequence)
  new-note.sh --summary       Create or locate today's daily summary
  new-note.sh --tomorrow      Pre-create tomorrow's daily summary
  new-note.sh --weekly        Create or locate this week's weekly summary

After creating, the script prints the file path and a reminder to use the
Read tool before editing. Slugs must be kebab-case.
EOF
    exit 1
}

[[ $# -lt 1 ]] && usage

find_root() {
    local dir="$PWD"
    while [[ "$dir" != "/" ]]; do
        [[ -f "$dir/.notes-config.yml" ]] && { echo "$dir"; return 0; }
        dir="$(dirname "$dir")"
    done
    return 1
}

PROJECT_ROOT="$(find_root)" || {
    echo "Error: .notes-config.yml not found in any parent directory." >&2
    echo "Run /doc:init to set up the documentation system first." >&2
    exit 1
}

cd "$PROJECT_ROOT"

cfg() {
    local key="$1" default="${2:-}" val
    val="$(grep -E "^${key}:" .notes-config.yml 2>/dev/null \
        | head -1 \
        | sed -E "s/^${key}:[[:space:]]*//" \
        | sed 's/[[:space:]]*#.*$//' \
        | sed -E 's/^"(.*)"$/\1/' \
        | sed -E "s/^'(.*)'$/\1/")"
    echo "${val:-$default}"
}

WORKLOG_DIR="$(cfg worklog_dir docs/worklog)"
GRANULARITY="$(cfg granularity flat)"
CADENCE="$(cfg summary_cadence daily)"

TODAY="$(date +%Y-%m-%d)"
ISO_WEEK="$(date +%G-W%V)"

# Portable tomorrow (BSD/macOS or GNU/Linux date)
if date -v+1d +%Y-%m-%d >/dev/null 2>&1; then
    TOMORROW="$(date -v+1d +%Y-%m-%d)"
    TOMORROW_WEEK="$(date -v+1d +%G-W%V)"
else
    TOMORROW="$(date -d tomorrow +%Y-%m-%d)"
    TOMORROW_WEEK="$(date -d tomorrow +%G-W%V)"
fi

target_dir() {
    local date="$1" week="$2"
    case "$GRANULARITY" in
        flat)   echo "$WORKLOG_DIR" ;;
        daily)  echo "$WORKLOG_DIR/$date" ;;
        weekly) echo "$WORKLOG_DIR/$week" ;;
        *)      echo "Error: unknown granularity '$GRANULARITY' in .notes-config.yml" >&2; exit 1 ;;
    esac
}

filename_prefix() {
    local date="$1"
    case "$GRANULARITY" in
        flat|weekly) echo "$date-" ;;
        daily)       echo "" ;;
    esac
}

emit_note() {
    local title="$1" date="$2"
    cat <<EOF
# $title

**Date:** $date
**Type:** note

## Summary

## Details

## Follow-ups
EOF
}

emit_summary() {
    local date="$1"
    cat <<EOF
# Daily Summary — $date

## Overview

## Work Completed

## Notes Today

| # | Note | Topic |
|---|------|-------|

## Open Threads

## Tomorrow
EOF
}

emit_weekly() {
    local week="$1" ending="$2"
    cat <<EOF
# Weekly Summary — $week

**Week ending:** $ending

## Highlights

## Key Decisions

## Open Threads

## Next Week
EOF
}

print_path_and_reminder() {
    local path="$1"
    echo "$path"
    echo ""
    echo "IMPORTANT: Use the Read tool on the path above before calling Edit."
    echo "Claude Code tools require a file to be read before it can be edited."
}

# --- Mode: --summary ---
if [[ "$1" == "--summary" ]]; then
    case "$CADENCE" in
        daily|daily+weekly)
            ;;
        weekly)
            echo "Error: this project uses summary_cadence=weekly — there is no daily summary." >&2
            echo "       Use: $0 --weekly" >&2
            exit 1
            ;;
        none)
            echo "Error: this project uses summary_cadence=none — summaries are disabled." >&2
            echo "       Create a detailed note instead: $0 <slug>" >&2
            exit 1
            ;;
        *)
            echo "Error: unknown summary_cadence '$CADENCE' in .notes-config.yml" >&2
            exit 1
            ;;
    esac
    DIR="$(target_dir "$TODAY" "$ISO_WEEK")"
    mkdir -p "$DIR"
    PREFIX="$(filename_prefix "$TODAY")"
    FILEPATH="$DIR/${PREFIX}00-summary.md"
    [[ -f "$FILEPATH" ]] || emit_summary "$TODAY" > "$FILEPATH"
    print_path_and_reminder "$FILEPATH"
    exit 0
fi

# --- Mode: --tomorrow ---
if [[ "$1" == "--tomorrow" ]]; then
    case "$CADENCE" in
        daily|daily+weekly) ;;
        weekly|none)
            echo "Error: --tomorrow creates a daily summary, but summary_cadence=$CADENCE doesn't use daily summaries." >&2
            exit 1
            ;;
    esac
    DIR="$(target_dir "$TOMORROW" "$TOMORROW_WEEK")"
    mkdir -p "$DIR"
    PREFIX="$(filename_prefix "$TOMORROW")"
    FILEPATH="$DIR/${PREFIX}00-summary.md"
    [[ -f "$FILEPATH" ]] || emit_summary "$TOMORROW" > "$FILEPATH"
    print_path_and_reminder "$FILEPATH"
    exit 0
fi

# --- Mode: --weekly ---
if [[ "$1" == "--weekly" ]]; then
    case "$CADENCE" in
        weekly|daily+weekly) ;;
        daily|none)
            echo "Error: summary_cadence=$CADENCE does not produce weekly summaries." >&2
            echo "       Change summary_cadence in .notes-config.yml first, or use --summary / <slug>." >&2
            exit 1
            ;;
    esac
    case "$GRANULARITY" in
        weekly)
            DIR="$WORKLOG_DIR/$ISO_WEEK"
            FILEPATH="$DIR/weekly-summary.md"
            ;;
        flat|daily)
            DIR="$WORKLOG_DIR"
            FILEPATH="$DIR/$ISO_WEEK-weekly-summary.md"
            ;;
    esac
    mkdir -p "$DIR"
    [[ -f "$FILEPATH" ]] || emit_weekly "$ISO_WEEK" "$TODAY" > "$FILEPATH"
    print_path_and_reminder "$FILEPATH"
    exit 0
fi

# --- Mode: detailed note (positional slug) ---
SLUG="$1"

if [[ ! "$SLUG" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
    echo "Error: slug must be kebab-case (lowercase letters, numbers, hyphens)" >&2
    echo "       Example: 'auth-bug-investigation'" >&2
    exit 1
fi

DIR="$(target_dir "$TODAY" "$ISO_WEEK")"
mkdir -p "$DIR"
PREFIX="$(filename_prefix "$TODAY")"

SEQ=1
while true; do
    SEQ_PAD="$(printf '%02d' "$SEQ")"
    # shellcheck disable=SC2086
    if ! compgen -G "$DIR/${PREFIX}${SEQ_PAD}-*.md" > /dev/null; then
        break
    fi
    SEQ=$((SEQ + 1))
done
SEQ_PAD="$(printf '%02d' "$SEQ")"

FILEPATH="$DIR/${PREFIX}${SEQ_PAD}-${SLUG}.md"
TITLE="$(echo "$SLUG" | tr '-' ' ' | awk '{for(i=1;i<=NF;i++) $i=toupper(substr($i,1,1)) substr($i,2)} 1')"

emit_note "$TITLE" "$TODAY" > "$FILEPATH"
print_path_and_reminder "$FILEPATH"
