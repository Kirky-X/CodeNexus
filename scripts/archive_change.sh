#!/usr/bin/env bash
# Copyright (c) 2026 Kirky.X. All rights reserved.
# SPDX-License-Identifier: MIT
# Deterministic archive executor for specmark workflow.
#
# Creates .readonly sentinel, acquires change-level flock, syncs delta specs
# (if --sync), moves changes/<name> -> archive/<date>-<name>, writes meta.json
# anchored to current git HEAD commit SHA.
#
# Usage:
#   bash scripts/archive_change.sh <name> [--sync] [--date YYYY-MM-DD]
#
# Exit codes:
#   0 — success
#   1 — generic error (missing change, target exists, sync failure)
#   2 — flock timeout (another archive in progress for this change)

set -euo pipefail

CHANGE_NAME=""
SYNC=0
DATE_OVERRIDE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --sync)   SYNC=1; shift ;;
        --date)   DATE_OVERRIDE="$2"; shift 2 ;;
        --*)      echo "unknown flag: $1" >&2; exit 1 ;;
        *)        CHANGE_NAME="$1"; shift ;;
    esac
done

if [[ -z "$CHANGE_NAME" ]]; then
    echo "usage: bash scripts/archive_change.sh <name> [--sync] [--date YYYY-MM-DD]" >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CHANGES_DIR="specmark/changes"
ARCHIVE_DIR="specmark/archive"
LOCKS_DIR="specmark/.locks"
CHANGE_PATH="${CHANGES_DIR}/${CHANGE_NAME}"

if [[ ! -d "$CHANGE_PATH" ]]; then
    echo "error: change not found: $CHANGE_PATH" >&2
    exit 1
fi

# Date (UTC) — override or today
if [[ -n "$DATE_OVERRIDE" ]]; then
    ARCHIVE_DATE="$DATE_OVERRIDE"
else
    ARCHIVE_DATE="$(date -u +%Y-%m-%d)"
fi
ARCHIVE_TARGET="${ARCHIVE_DIR}/${ARCHIVE_DATE}-${CHANGE_NAME}"

# Readonly sentinel
mkdir -p "$ARCHIVE_DIR" "$LOCKS_DIR"
touch "${ARCHIVE_DIR}/.readonly"

# Refuse if target already exists (readonly enforcement)
if [[ -e "$ARCHIVE_TARGET" ]]; then
    echo "error: archive target already exists: $ARCHIVE_TARGET" >&2
    echo "  archived entries are readonly; create a new change for follow-up work" >&2
    exit 1
fi

LOCK_FILE="${LOCKS_DIR}/${CHANGE_NAME}.lock"

# Change-level flock (10s timeout)
exec 9>"$LOCK_FILE"
if ! flock -w 10 9; then
    echo "error: could not acquire lock for change '$CHANGE_NAME' (another archive in progress?)" >&2
    exit 2
fi

# Sync delta specs if requested
SYNC_STATUS="false"
if [[ "$SYNC" -eq 1 ]]; then
    DELTA_SPECS_DIR="${CHANGE_PATH}/specs"
    if [[ -d "$DELTA_SPECS_DIR" ]]; then
        MAIN_SPECS_DIR="specmark/specs"
        MERGE_SCRIPT="scripts/merge_delta_spec.py"
        if [[ ! -f "$MERGE_SCRIPT" ]]; then
            echo "error: merge script not found: $MERGE_SCRIPT" >&2
            exit 1
        fi
        while IFS= read -r -d '' delta_spec; do
            # derive capability name from path: specs/<cap>/spec.md
            cap="$(basename "$(dirname "$delta_spec")")"
            main_spec="${MAIN_SPECS_DIR}/${cap}/spec.md"
            echo "syncing delta spec: $delta_spec -> $main_spec" >&2
            if ! python3 "$MERGE_SCRIPT" --main "$main_spec" --delta "$delta_spec"; then
                echo "error: delta spec merge failed for $cap" >&2
                exit 1
            fi
        done < <(find "$DELTA_SPECS_DIR" -name "spec.md" -print0)
        SYNC_STATUS="true"
    else
        echo "note: --sync requested but no delta specs found in $DELTA_SPECS_DIR" >&2
    fi
fi

# Commit SHA anchor
COMMIT_SHA="$(git rev-parse HEAD 2>/dev/null || echo null)"

# Atomic move
mv "$CHANGE_PATH" "$ARCHIVE_TARGET"

# Write meta.json
META_FILE="${ARCHIVE_TARGET}/meta.json"
cat > "$META_FILE" <<EOF
{
  "change": "${CHANGE_NAME}",
  "archived_at": "${ARCHIVE_DATE}",
  "commit_sha": "${COMMIT_SHA}",
  "synced": ${SYNC_STATUS}
}
EOF

echo "archived: ${CHANGE_NAME} -> ${ARCHIVE_TARGET}" >&2
echo "commit_sha: ${COMMIT_SHA}" >&2
echo "synced: ${SYNC_STATUS}" >&2
