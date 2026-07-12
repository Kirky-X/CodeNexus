//! gitnexus reference client (tasks 4.1-4.4).
//!
//! Invokes `gitnexus cypher --repo <name> <query>` as a subprocess, parses the
//! JSON response (markdown table or `[]` or `{"error": "..."}`), and exposes
//! `fetch_reference()` to obtain node/edge/file counts for comparison.
//!
//! # Error model (Rule 12: Fail Loud)
//!
//! - **Repo not indexed**: subprocess exits non-zero with stderr containing
//!   `Repository "<name>" not found`. We return an explicit error instructing
//!   the user to run `gitnexus analyze` first.
//! - **DB corruption**: subprocess exits 0 but the JSON has an `"error"` field
//!   matching known corruption patterns (e.g. `UNREACHABLE_CODE`,
//!   `Mmap for size ... failed`, `WAL`). We return `ReferenceUnavailable`
//!   so the caller can mark the sample as skipped, not crashed.
//! - **Binder exception** (e.g. `Table X does not exist`): treated as a query
//!   error. `fetch_reference` catches these per-label and records 0 rather
//!   than aborting the whole fetch (a label may simply not exist for a given
//!   repo's language).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Stats fetched from gitnexus for a single repo. Mirrors `CodeNexusStats`
/// shape so the report generator can diff them side-by-side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitnexusStats {
    pub name: String,
    /// Node counts grouped by gitnexus label (e.g. `Function`, `File`).
    pub node_counts_by_label: BTreeMap<String, u64>,
    /// Edge counts grouped by `CodeRelation.type` (e.g. `CALLS`, `CONTAINS`).
    pub edge_counts_by_type: BTreeMap<String, u64>,
    /// Total file count (`MATCH (f:File) RETURN count(*)`).
    pub file_count: u64,
}

/// Parsed subprocess response from `gitnexus cypher`.
#[derive(Debug, Clone)]
pub enum CypherResponse {
    /// Query returned rows. `headers` is the column list; `rows` is row-major.
    Rows {
        /// Column names from the Cypher RETURN clause; retained for structural
        /// completeness (currently only `rows` is read by consumers).
        #[allow(dead_code)]
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Query returned zero rows (`[]`).
    Empty,
    /// Query or DB errored. `corruption: true` if pattern-matched as DB
    /// corruption (so the caller can mark the reference unavailable).
    Error { message: String, corruption: bool },
}

/// Top-level JSON envelope from `gitnexus cypher`.
#[derive(Debug, Deserialize)]
struct CypherJson {
    #[serde(default)]
    markdown: Option<String>,
    /// Retained for JSON structural completeness; not read by consumers.
    #[serde(default)]
    #[allow(dead_code)]
    row_count: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

/// Substrings that indicate the gitnexus LadybugDB index is corrupt / unusable
/// rather than merely missing a label. Mirrors the design.md risk note about
/// 8 TiB mmap errors and the observed `UNREACHABLE_CODE` WAL assertion.
const CORRUPTION_PATTERNS: &[&str] = &[
    "mmap for size",
    "unreachable_code",
    "wal_record",
    "database disk image is malformed",
    "file is encrypted or is not a database",
];

/// Task 4.1: Execute a Cypher query against gitnexus via subprocess.
///
/// Returns the parsed response. Caller decides how to handle `Error`.
///
/// When `gitnexus_binary` is `Some(path)`, invokes that binary directly;
/// when `None`, searches PATH for `gitnexus`.
pub fn run_cypher(
    repo: &str,
    query: &str,
    gitnexus_binary: Option<&Path>,
) -> Result<CypherResponse> {
    let binary = gitnexus_binary.unwrap_or_else(|| Path::new("gitnexus"));
    let output = Command::new(binary)
        .args(["cypher", "--repo", repo, query])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("failed to spawn `gitnexus cypher`")?;

    // Repo-not-found surfaces as non-zero exit + stderr message.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found. Available:") {
            bail!(
                "gitnexus has no index for `{repo}`; run `gitnexus analyze` in the repo first.\n--- stderr ---\n{stderr}"
            );
        }
        bail!(
            "gitnexus cypher failed (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            stderr,
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // `[]` (empty array) is a valid zero-row response.
    if stdout.trim() == "[]" {
        return Ok(CypherResponse::Empty);
    }

    let parsed: CypherJson = serde_json::from_str(&stdout)
        .with_context(|| format!("failed to parse gitnexus JSON: {stdout}"))?;

    if let Some(msg) = parsed.error {
        let corruption = CORRUPTION_PATTERNS
            .iter()
            .any(|p| msg.to_lowercase().contains(p));
        return Ok(CypherResponse::Error {
            message: msg,
            corruption,
        });
    }

    let markdown = parsed.markdown.unwrap_or_default();
    let (headers, rows) = parse_markdown_table(&markdown);
    if headers.is_empty() {
        return Ok(CypherResponse::Empty);
    }
    Ok(CypherResponse::Rows { headers, rows })
}

/// Parse a GitHub-flavored markdown table into (headers, rows).
///
/// Expected layout (what `gitnexus cypher` emits):
/// ```text
/// | col1 | col2 |
/// | --- | --- |
/// | v1a | v1b |
/// | v2a | v2b |
/// ```
///
/// Cells are trimmed; surrounding pipes are stripped. The separator row
/// (containing only `-`, `:`, `|`, spaces) is skipped.
fn parse_markdown_table(markdown: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut lines = markdown.lines().filter(|l| !l.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return (Vec::new(), Vec::new());
    };
    let headers = split_row(header_line);
    if headers.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // Skip separator row (| --- | --- |).
    let _ = lines.next();
    let mut rows = Vec::new();
    for line in lines {
        // Belt-and-braces: skip any stray separator rows.
        if line
            .trim()
            .trim_matches(|c: char| c == '|' || c == '-' || c == ':' || c.is_whitespace())
            .is_empty()
        {
            continue;
        }
        rows.push(split_row(line));
    }
    (headers, rows)
}

/// Split a markdown table row into trimmed cells.
fn split_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Task 4.2: Fetch reference stats for `repo_name` from gitnexus.
///
/// Runs three queries:
/// 1. `MATCH (n) RETURN label(n), count(*)` → node_counts_by_label
/// 2. `MATCH ()-[r:CodeRelation]->() RETURN r.type, count(*)` → edge_counts_by_type
/// 3. `MATCH (f:File) RETURN count(*)` → file_count
///
/// Per-label binder errors (Table X does not exist) are recorded as 0, not
/// fatal. DB corruption aborts with `ReferenceUnavailable`.
pub fn fetch_reference(repo_name: &str, gitnexus_binary: Option<&Path>) -> Result<GitnexusStats> {
    let mut node_counts = BTreeMap::new();
    match run_cypher(
        repo_name,
        "MATCH (n) RETURN label(n) AS lbl, count(*) AS c ORDER BY lbl",
        gitnexus_binary,
    ) {
        Ok(CypherResponse::Rows { rows, .. }) => {
            for row in &rows {
                if row.len() < 2 {
                    continue;
                }
                let label = row[0].clone();
                let count: u64 = row[1].parse().unwrap_or(0);
                node_counts.insert(label, count);
            }
        }
        Ok(CypherResponse::Error {
            message,
            corruption: true,
        }) => {
            bail!("gitnexus reference unavailable for `{repo_name}`: {message}");
        }
        Ok(CypherResponse::Error { message, .. }) => {
            // Non-corruption error on the all-nodes query is fatal — there's
            // no meaningful reference to compare against.
            bail!("gitnexus node-count query failed for `{repo_name}`: {message}");
        }
        Ok(CypherResponse::Empty) => {
            // Repo indexed but zero nodes — record empty map, comparison will
            // surface the discrepancy.
            eprintln!("[warn] gitnexus reports 0 nodes for `{repo_name}`");
        }
        Err(e) => return Err(e),
    }

    let mut edge_counts = BTreeMap::new();
    match run_cypher(
        repo_name,
        "MATCH ()-[r:CodeRelation]->() RETURN r.type AS t, count(*) AS c ORDER BY t",
        gitnexus_binary,
    ) {
        Ok(CypherResponse::Rows { rows, .. }) => {
            for row in &rows {
                if row.len() < 2 {
                    continue;
                }
                let t = row[0].clone();
                let c: u64 = row[1].parse().unwrap_or(0);
                edge_counts.insert(t, c);
            }
        }
        Ok(CypherResponse::Error {
            message,
            corruption: true,
        }) => {
            bail!("gitnexus reference unavailable for `{repo_name}`: {message}");
        }
        Ok(CypherResponse::Error { message, .. }) => {
            eprintln!("[warn] gitnexus edge-count query failed for `{repo_name}`: {message}");
        }
        Ok(CypherResponse::Empty) => {
            eprintln!("[warn] gitnexus reports 0 edges for `{repo_name}`");
        }
        Err(e) => return Err(e),
    }

    let mut file_count = 0u64;
    match run_cypher(
        repo_name,
        "MATCH (f:File) RETURN count(*) AS c",
        gitnexus_binary,
    ) {
        Ok(CypherResponse::Rows { rows, .. }) => {
            if let Some(row) = rows.first() {
                if let Some(cell) = row.first() {
                    file_count = cell.parse().unwrap_or(0);
                }
            }
        }
        Ok(CypherResponse::Error {
            message,
            corruption: true,
        }) => {
            bail!("gitnexus reference unavailable for `{repo_name}`: {message}");
        }
        Ok(CypherResponse::Error { message, .. }) => {
            eprintln!("[warn] gitnexus file-count query failed for `{repo_name}`: {message}");
        }
        Ok(CypherResponse::Empty) | Err(_) => {
            // File count is informational; don't abort the whole fetch.
        }
    }

    Ok(GitnexusStats {
        name: repo_name.to_string(),
        node_counts_by_label: node_counts,
        edge_counts_by_type: edge_counts,
        file_count,
    })
}

/// Task 4.4: Write reference stats to `tools/verification/results/<name>.gitnexus.json`.
pub fn write_reference(name: &str, stats: &GitnexusStats) -> Result<PathBuf> {
    let dir = Path::new("tools/verification/results");
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{name}.gitnexus.json"));
    let json = serde_json::to_string_pretty(stats)?;
    std::fs::write(&path, json + "\n")
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Load a previously-written gitnexus reference from
/// `tools/verification/results/<name>.gitnexus.json`. Used by the
/// orchestrator's `--resume` path when a fresh `gitnexus cypher` fetch is
/// unavailable (e.g. gitnexus DB version mismatch between the indexed DB
/// and the installed binary). Returns an error if the file does not exist
/// or is not valid JSON (Rule 12: fail loud — silently treating a missing
/// reference as zero counts would produce a misleading PASS report).
pub fn load_reference(name: &str) -> Result<GitnexusStats> {
    let dir = Path::new("tools/verification/results");
    let path = dir.join(format!("{name}.gitnexus.json"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("failed to read gitnexus reference {}", path.display()))?;
    let stats: GitnexusStats = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse gitnexus reference {}", path.display()))?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_markdown_table_basic() {
        let md = "| lbl | c |\n| --- | --- |\n| Function | 100 |\n| File | 20 |";
        let (headers, rows) = parse_markdown_table(md);
        assert_eq!(headers, vec!["lbl".to_string(), "c".to_string()]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["Function".to_string(), "100".to_string()]);
        assert_eq!(rows[1], vec!["File".to_string(), "20".to_string()]);
    }

    #[test]
    fn parse_markdown_table_skips_extra_separators() {
        let md = "| a | b |\n| --- | --- |\n| --- | --- |\n| 1 | 2 |";
        let (headers, rows) = parse_markdown_table(md);
        assert_eq!(headers, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(rows, vec![vec!["1".to_string(), "2".to_string()]]);
    }

    #[test]
    fn parse_markdown_table_empty_input() {
        let (headers, rows) = parse_markdown_table("");
        assert!(headers.is_empty());
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_markdown_table_single_column() {
        let md = "| c |\n| --- |\n| 42 |";
        let (headers, rows) = parse_markdown_table(md);
        assert_eq!(headers, vec!["c".to_string()]);
        assert_eq!(rows, vec![vec!["42".to_string()]]);
    }

    #[test]
    fn corruption_patterns_match_known_errors() {
        assert!(CORRUPTION_PATTERNS.iter().any(|p| {
            "Mmap for size 8796093022208 failed"
                .to_lowercase()
                .contains(p)
        }));
        assert!(CORRUPTION_PATTERNS.iter().any(|p| {
            "Assertion failed ... UNREACHABLE_CODE"
                .to_lowercase()
                .contains(p)
        }));
        // Binder exception must NOT be classified as corruption.
        assert!(!CORRUPTION_PATTERNS.iter().any(|p| {
            "Binder exception: Table Foo does not exist"
                .to_lowercase()
                .contains(p)
        }));
    }

    #[test]
    fn gitnexus_stats_serialize_roundtrip() {
        let stats = GitnexusStats {
            name: "demo".to_string(),
            node_counts_by_label: {
                let mut m = BTreeMap::new();
                m.insert("Function".to_string(), 100);
                m.insert("File".to_string(), 20);
                m
            },
            edge_counts_by_type: {
                let mut m = BTreeMap::new();
                m.insert("CALLS".to_string(), 500);
                m
            },
            file_count: 20,
        };
        let json = serde_json::to_string_pretty(&stats).unwrap();
        let back: GitnexusStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stats);
    }

    // --- CypherJson parsing ---

    #[test]
    fn cypher_json_parses_markdown_response() {
        let json = r#"{"markdown":"| col1 | col2 |\n| --- | --- |\n| v1 | v2 |","row_count":1}"#;
        let parsed: CypherJson = serde_json::from_str(json).unwrap();
        assert!(parsed.markdown.is_some());
        assert_eq!(parsed.row_count, Some(1));
        assert!(parsed.error.is_none());
    }

    #[test]
    fn cypher_json_parses_error_response() {
        let json = r#"{"error":"Binder exception: Table Foo does not exist"}"#;
        let parsed: CypherJson = serde_json::from_str(json).unwrap();
        assert!(parsed.markdown.is_none());
        assert_eq!(
            parsed.error.as_deref(),
            Some("Binder exception: Table Foo does not exist")
        );
    }

    #[test]
    fn cypher_json_parses_empty_object() {
        let json = r#"{}"#;
        let parsed: CypherJson = serde_json::from_str(json).unwrap();
        assert!(parsed.markdown.is_none());
        assert!(parsed.error.is_none());
    }

    // --- split_row ---

    #[test]
    fn split_row_trims_cells() {
        let cells = split_row("|  hello  |  world  |");
        assert_eq!(cells, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn split_row_single_cell() {
        let cells = split_row("| only |");
        assert_eq!(cells, vec!["only".to_string()]);
    }

    // --- write_reference + load_reference roundtrip ---

    #[test]
    fn write_and_load_reference_roundtrip() {
        let stats = GitnexusStats {
            name: "_test_roundtrip".to_string(),
            node_counts_by_label: {
                let mut m = BTreeMap::new();
                m.insert("Function".to_string(), 42);
                m.insert("File".to_string(), 10);
                m
            },
            edge_counts_by_type: {
                let mut m = BTreeMap::new();
                m.insert("CALLS".to_string(), 100);
                m
            },
            file_count: 10,
        };
        let path = write_reference("_test_roundtrip", &stats).expect("write_reference");
        assert!(path.exists(), "reference file should be created");
        let loaded = load_reference("_test_roundtrip").expect("load_reference");
        assert_eq!(loaded, stats);
        std::fs::remove_file(&path).expect("cleanup test file");
    }

    #[test]
    fn load_reference_returns_err_for_nonexistent() {
        let result = load_reference("_nonexistent_repo_xyz");
        assert!(result.is_err(), "loading nonexistent reference should error");
    }
}
