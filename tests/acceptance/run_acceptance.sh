#!/usr/bin/env bash
# run_acceptance.sh — 8-language real-project acceptance test harness.
#
# Clones 8 open-source projects (one per language), indexes each with
# CodeNexus, cross-validates against gitnexus, and produces a summary
# diff report.
#
# Usage:
#   bash tests/acceptance/run_acceptance.sh [--dry-run] [--gitnexus-binary <path>]
#
# --dry-run            Echo commands without executing (validates syntax only).
# --gitnexus-binary P  Path to gitnexus binary (default: search PATH).
#
# Requires: git, jq, cargo. Run from the CodeNexus project root.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures"
RESULTS_DIR="${SCRIPT_DIR}/results"
VERIFY_RESULTS_DIR="${PROJECT_ROOT}/tools/verification/results"
SUMMARY_FILE="${RESULTS_DIR}/summary.md"

# --- Parse arguments ---
DRY_RUN=false
GITNEXUS_BINARY=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    --gitnexus-binary)
      [[ $# -ge 2 ]] || { echo "ERROR: --gitnexus-binary requires a path argument" >&2; exit 1; }
      GITNEXUS_BINARY="$2"
      shift 2
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      echo "Usage: $0 [--dry-run] [--gitnexus-binary <path>]" >&2
      exit 1
      ;;
  esac
done

# --- Project manifest: name|github_repo|tag|language ---
PROJECTS=(
  "serde|serde-rs/serde|v1.0.219|rust"
  "requests|psf/requests|v2.32.3|python"
  "vscode-uri|microsoft/vscode-uri|v3.0.2|typescript"
  "redis|redis/redis|7.4.2|c"
  "gin|gin-gonic/gin|v1.10.0|go"
  "jackson-databind|FasterXML/jackson-databind|jackson-databind-2.18.2|java"
  "fmt|fmtlib/fmt|11.0.2|cpp"
  "OpenBLAS|OpenMathLib/OpenBLAS|v0.3.28|fortran"
)

# --- Helpers ---

# Echo a command in dry-run mode, execute it otherwise.
run_or_echo() {
  if $DRY_RUN; then
    echo "[dry-run] $*"
  else
    "$@"
  fi
}

# Verify gitnexus binary is available (skipped in dry-run).
check_gitnexus() {
  if [[ -n "$GITNEXUS_BINARY" ]]; then
    if [[ ! -x "$GITNEXUS_BINARY" ]]; then
      echo "ERROR: gitnexus binary not found or not executable: $GITNEXUS_BINARY" >&2
      exit 1
    fi
  else
    if ! command -v gitnexus >/dev/null 2>&1; then
      echo "ERROR: gitnexus not found in PATH. Use --gitnexus-binary <path>." >&2
      exit 1
    fi
  fi
}

# Sum all values in a JSON object field.
# Rule 12: surface jq errors explicitly — do NOT silence with 2>/dev/null.
sum_json_values() {
  local json_file="$1"
  local key="$2"
  if [[ ! -f "$json_file" ]]; then
    echo "N/A"
    return
  fi
  local result
  if ! result=$(jq "[.${key}[]] | add" "$json_file" 2>&1); then
    echo "ERROR: jq failed on $json_file for key '$key': $result" >&2
    echo "N/A"
    return
  fi
  echo "$result"
}

# --- Main ---

# cd to project root so codenexus-verify's relative paths resolve.
cd "$PROJECT_ROOT"

# Pre-flight checks (jq needed for summary even in some dry-run paths).
if ! $DRY_RUN; then
  check_gitnexus
  if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: jq is required but not installed" >&2
    exit 1
  fi
fi

# Create output directories.
mkdir -p "$FIXTURES_DIR" "$RESULTS_DIR"

# Initialize summary file.
{
  echo "# Acceptance Test Summary"
  echo ""
  echo "| Project | Language | CodeNexus Nodes | gitnexus Nodes | Diff | Diff Reason |"
  echo "|---------|----------|-----------------|----------------|------|--------------|"
} > "$SUMMARY_FILE"

# Process each project.
for project in "${PROJECTS[@]}"; do
  IFS='|' read -r name repo tag language <<< "$project"
  fixture_path="${FIXTURES_DIR}/${name}"
  report_src="${VERIFY_RESULTS_DIR}/${name}.report.md"
  report_dst="${RESULTS_DIR}/${name}.md"

  echo ""
  echo "=== ${name} (${language}) ==="

  # 1. Clone (skip if fixture already exists).
  if [[ -d "$fixture_path" ]]; then
    echo "[skip] clone (fixture exists: ${fixture_path})"
  else
    run_or_echo git clone --depth 1 --branch "$tag" "https://github.com/${repo}.git" "$fixture_path"
  fi

  # 2. Index with CodeNexus.
  run_or_echo cargo run --release --bin codenexus -- index "$fixture_path" --name "$name"

  # 3. Index with gitnexus (reference index for cross-validation).
  #    --skip-agents-md avoids polluting the fixture's AGENTS.md.
  echo "[gitnexus] analyzing ${name}..."
  if ! $DRY_RUN; then
    gitnexus analyze --skip-agents-md "$fixture_path" 2>&1 | tail -3 || true
  else
    echo "[dry-run] gitnexus analyze --skip-agents-md $fixture_path"
  fi

  # 4. Cross-validate with codenexus-verify.
  #    --gitnexus-binary is a global flag (before the subcommand).
  VERIFY_CMD=(cargo run --release --bin codenexus-verify --)
  if [[ -n "$GITNEXUS_BINARY" ]]; then
    VERIFY_CMD+=("--gitnexus-binary" "$GITNEXUS_BINARY")
  fi
  VERIFY_CMD+=(single --repo "$fixture_path" --name "$name" --language "$language" --resume)
  run_or_echo "${VERIFY_CMD[@]}"

  # 4. Copy per-project report.
  if $DRY_RUN; then
    echo "[dry-run] cp ${report_src} ${report_dst}"
  elif [[ -f "$report_src" ]]; then
    cp "$report_src" "$report_dst"
    echo "[ok] copied report to ${report_dst}"
  else
    echo "[warn] no report at ${report_src}"
  fi

  # 5. Append summary row.
  if $DRY_RUN; then
    echo "| ${name} | ${language} | (dry-run) | (dry-run) | (dry-run) | (dry-run) |" >> "$SUMMARY_FILE"
  else
    cn_nodes=$(sum_json_values "${VERIFY_RESULTS_DIR}/${name}.codenexus.json" "node_counts_by_type")
    gn_nodes=$(sum_json_values "${VERIFY_RESULTS_DIR}/${name}.gitnexus.json" "node_counts_by_label")
    if [[ "$cn_nodes" == "N/A" || "$gn_nodes" == "N/A" ]]; then
      diff_val="N/A"
      reason="missing JSON"
    else
      diff_val=$((cn_nodes - gn_nodes))
      if [[ "$diff_val" -eq 0 ]]; then
        reason="match"
      else
        reason="see ${name}.md"
      fi
    fi
    echo "| ${name} | ${language} | ${cn_nodes} | ${gn_nodes} | ${diff_val} | ${reason} |" >> "$SUMMARY_FILE"
  fi
done

echo ""
echo "[done] Summary written to ${SUMMARY_FILE}"
