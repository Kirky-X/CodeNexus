#!/usr/bin/env bash
# fetch_samples.sh — clone missing sample repos and check out the pinned commit.
#
# Reads tools/verification/samples.json and for each entry:
#   1. If `repo_path` exists and matches `commit`, skip.
#   2. If `repo_path` exists but commit differs, fetch + checkout.
#   3. If `repo_path` does not exist, clone from `source` then checkout.
#
# Entries whose `source` is "self" or "gitnexus-indexed" are assumed to exist
# locally already; this script will only verify their commit and warn if absent.
#
# Usage:
#   bash tools/verification/fetch_samples.sh
#   bash tools/verification/fetch_samples.sh --only redis,LAPACK
#
# Requires: git, jq, curl (for cloning).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SAMPLES_JSON="${SCRIPT_DIR}/samples.json"

if [[ ! -f "${SAMPLES_JSON}" ]]; then
  echo "ERROR: samples.json not found at ${SAMPLES_JSON}" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq is required but not installed" >&2
  exit 1
fi

ONLY_FILTER="${1:-}"
if [[ "${ONLY_FILTER}" == "--only" ]]; then
  shift
  ONLY_NAMES="${1:-}"
  shift || true
else
  ONLY_NAMES=""
fi

# Read sample entries.
SAMPLE_COUNT=$(jq '.samples | length' "${SAMPLES_JSON}")
echo "[info] Processing ${SAMPLE_COUNT} samples from ${SAMPLES_JSON}"

for i in $(seq 0 $((SAMPLE_COUNT - 1))); do
  NAME=$(jq -r ".samples[${i}].name" "${SAMPLES_JSON}")
  REPO_PATH=$(jq -r ".samples[${i}].repo_path" "${SAMPLES_JSON}")
  COMMIT=$(jq -r ".samples[${i}].commit" "${SAMPLES_JSON}")
  SOURCE=$(jq -r ".samples[${i}].source" "${SAMPLES_JSON}")

  # Apply --only filter if set.
  if [[ -n "${ONLY_NAMES}" ]]; then
    case ",${ONLY_NAMES}," in
      *",${NAME},"*) ;;
      *) continue ;;
    esac
  fi

  echo "---"
  echo "[${NAME}] repo_path=${REPO_PATH} commit=${COMMIT} source=${SOURCE}"

  # Skip entries that are local-only (self or gitnexus-indexed).
  if [[ "${SOURCE}" == "self" || "${SOURCE}" == "gitnexus-indexed" ]]; then
    if [[ ! -d "${REPO_PATH}" ]]; then
      echo "[warn] ${NAME}: repo_path does not exist locally; skipping (source=${SOURCE})"
      continue
    fi
    CURRENT_COMMIT=$(cd "${REPO_PATH}" && git rev-parse --short HEAD 2>/dev/null || echo "unknown")
    if [[ "${CURRENT_COMMIT}" != "${COMMIT}" ]]; then
      echo "[warn] ${NAME}: local commit ${CURRENT_COMMIT} != pinned ${COMMIT}; leave as-is (local-only sample)"
    else
      echo "[ok] ${NAME}: commit matches ${COMMIT}"
    fi
    continue
  fi

  # External clone required.
  if [[ ! -d "${REPO_PATH}" ]]; then
    echo "[clone] ${NAME}: cloning ${SOURCE} → ${REPO_PATH}"
    PARENT_DIR=$(dirname "${REPO_PATH}")
    mkdir -p "${PARENT_DIR}"
    git clone --depth 50 "${SOURCE}" "${REPO_PATH}"
  fi

  # Checkout the pinned commit.
  echo "[checkout] ${NAME}: fetching ${COMMIT}"
  cd "${REPO_PATH}"
  git fetch --tags --depth 50 origin "${COMMIT}" 2>/dev/null || git fetch --tags --depth 50 origin
  git checkout "${COMMIT}" 2>/dev/null || {
    echo "[warn] ${NAME}: could not checkout ${COMMIT}; trying default branch"
    git checkout main 2>/dev/null || git checkout master 2>/dev/null || true
  }
  CURRENT_COMMIT=$(git rev-parse --short HEAD)
  echo "[ok] ${NAME}: now at ${CURRENT_COMMIT}"
  cd - >/dev/null
done

echo "---"
echo "[done] sample fetch complete"
