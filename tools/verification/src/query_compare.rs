//! Query comparison module (tasks 5.1-5.4).
//!
//! Loads `.cql` query files from `tools/verification/queries/`, executes each
//! against both CodeNexus (via `Repository::connection().query()`) and gitnexus
//! (via `gitnexus_client::run_cypher`), then compares the result sets as
//! order-insensitive `(symbol_name, file_path)` tuple sets.
//!
//! # File format
//!
//! Each `.cql` file contains two Cypher queries delimited by markers:
//!
//! ```text
//! // === CODENEXUS ===
//! <cypher for codenexus>
//!
//! // === GITNEXUS ===
//! <cypher for gitnexus>
//! ```
//!
//! Comments (`// ...`) before the first marker are treated as query metadata
//! and ignored by the extractor.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;

use crate::gitnexus_client;

/// Marker delimiting the CodeNexus variant in a `.cql` file.
const CODENEXUS_MARKER: &str = "// === CODENEXUS ===";
/// Marker delimiting the gitnexus variant in a `.cql` file.
const GITNEXUS_MARKER: &str = "// === GITNEXUS ===";

/// Which side to execute a query against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Codenexus,
    Gitnexus,
}

/// Outcome of comparing two query result sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryDiff {
    /// Both sides returned the same set (order-insensitive).
    Match { count: usize },
    /// Sets differ; lists the tuples missing from each side.
    CriticalDiff {
        missing_in_codenexus: Vec<(String, String)>,
        missing_in_gitnexus: Vec<(String, String)>,
    },
}

/// Extract the Cypher block for `side` from a `.cql` file's contents.
///
/// Strips `//` line comments. Returns an error if the marker is missing or
/// the block is empty.
pub fn extract_query_for_side(content: &str, side: Side) -> Result<String> {
    let marker = match side {
        Side::Codenexus => CODENEXUS_MARKER,
        Side::Gitnexus => GITNEXUS_MARKER,
    };
    let other = match side {
        Side::Codenexus => GITNEXUS_MARKER,
        Side::Gitnexus => CODENEXUS_MARKER,
    };

    let start_idx = content
        .find(marker)
        .ok_or_else(|| anyhow::anyhow!("marker `{marker}` not found in .cql file"))?
        + marker.len();
    let end_idx = content[start_idx..]
        .find(other)
        .map(|i| start_idx + i)
        .unwrap_or(content.len());

    let block: String = content[start_idx..end_idx]
        .lines()
        .map(|line| {
            // Strip `//` line comments within the block (e.g. explanatory notes).
            if let Some(idx) = line.find("//") {
                &line[..idx]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if block.is_empty() {
        anyhow::bail!("empty Cypher block for side `{side:?}`");
    }
    Ok(block)
}

/// Execute a Cypher query against CodeNexus and return `(symbol_name, file_path)` tuples.
///
/// Uses the first two columns of each row. Missing values become empty
/// strings (Rule 12: visible, not silent — empty rows are still recorded
/// so a missing column shows up as `("","")` rather than being dropped).
pub fn execute_codenexus_query(
    db_path: &Path,
    cql: &str,
) -> Result<Vec<(String, String)>> {
    let repo = codenexus::storage::repository::Repository::open(db_path)
        .with_context(|| format!("failed to open CodeNexus DB at {}", db_path.display()))?;
    let rows = repo.connection().query(cql)?;
    let out = rows
        .into_iter()
        .map(|row| {
            let symbol = row
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let file = row
                .get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (symbol, file)
        })
        .collect();
    Ok(out)
}

/// Execute a Cypher query against gitnexus and return `(symbol_name, file_path)` tuples.
///
/// Delegates to `gitnexus_client::run_cypher` for subprocess invocation.
/// Errors (corruption, repo-not-found) propagate; binder exceptions are
/// surfaced as empty results with a warning (some labels don't exist for
/// every language).
pub fn execute_gitnexus_query(repo: &str, cql: &str) -> Result<Vec<(String, String)>> {
    let response = gitnexus_client::run_cypher(repo, cql)?;
    let rows = match response {
        gitnexus_client::CypherResponse::Rows { rows, .. } => rows,
        gitnexus_client::CypherResponse::Empty => Vec::new(),
        gitnexus_client::CypherResponse::Error { message, .. } => {
            eprintln!("[warn] gitnexus query failed for `{repo}`: {message}");
            Vec::new()
        }
    };
    let out = rows
        .into_iter()
        .map(|row| {
            let symbol = row.first().cloned().unwrap_or_default();
            let file = row.get(1).cloned().unwrap_or_default();
            (symbol, file)
        })
        .collect();
    Ok(out)
}

/// Compare two result sets as order-insensitive `(symbol_name, file_path)` tuple sets.
///
/// Returns `Match` if the sets are equal (regardless of order or duplicates),
/// otherwise `CriticalDiff` listing the tuples missing from each side.
pub fn compare_query_results(
    codenexus_results: &[(String, String)],
    gitnexus_results: &[(String, String)],
) -> QueryDiff {
    let cn: HashSet<&(String, String)> = codenexus_results.iter().collect();
    let gn: HashSet<&(String, String)> = gitnexus_results.iter().collect();
    if cn == gn {
        return QueryDiff::Match { count: cn.len() };
    }
    let missing_in_codenexus: Vec<(String, String)> = gn
        .difference(&cn)
        .map(|(s, f)| (s.clone(), f.clone()))
        .collect();
    let missing_in_gitnexus: Vec<(String, String)> = cn
        .difference(&gn)
        .map(|(s, f)| (s.clone(), f.clone()))
        .collect();
    QueryDiff::CriticalDiff {
        missing_in_codenexus,
        missing_in_gitnexus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CQL: &str = r#"// Query: callers_of_function
// Returns: (symbol_name, file_path) tuples

// === CODENEXUS ===
MATCH (caller:Function)-[:Calls]->(callee:Function)
RETURN caller.name AS symbol_name, caller.filePath AS file_path
LIMIT 200

// === GITNEXUS ===
MATCH (caller:Function)-[r:CodeRelation]->(callee:Function)
WHERE r.type = 'CALLS'
RETURN caller.name AS symbol_name, caller.filePath AS file_path
LIMIT 200
"#;

    #[test]
    fn extract_codenexus_block_strips_markers_and_comments() {
        let q = extract_query_for_side(SAMPLE_CQL, Side::Codenexus).unwrap();
        assert!(q.contains("MATCH (caller:Function)"));
        assert!(q.contains("LIMIT 200"));
        assert!(!q.contains("==="));
        // The leading `// Query:` header must NOT appear in the extracted block.
        assert!(!q.contains("Query: callers"));
    }

    #[test]
    fn extract_gitnexus_block_includes_where_clause() {
        let q = extract_query_for_side(SAMPLE_CQL, Side::Gitnexus).unwrap();
        assert!(q.contains("WHERE r.type = 'CALLS'"));
        assert!(!q.contains("==="));
    }

    #[test]
    fn extract_returns_error_when_marker_missing() {
        let content = "// no markers here\nMATCH (n) RETURN n";
        let err = extract_query_for_side(content, Side::Codenexus).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn compare_match_when_sets_equal_regardless_of_order() {
        let cn = vec![
            ("foo".to_string(), "a.rs".to_string()),
            ("bar".to_string(), "b.rs".to_string()),
            ("baz".to_string(), "c.rs".to_string()),
        ];
        // Same set, different order.
        let gn = vec![
            ("baz".to_string(), "c.rs".to_string()),
            ("foo".to_string(), "a.rs".to_string()),
            ("bar".to_string(), "b.rs".to_string()),
        ];
        let diff = compare_query_results(&cn, &gn);
        assert_eq!(diff, QueryDiff::Match { count: 3 });
    }

    #[test]
    fn compare_match_when_duplicates_present() {
        // Sets treat duplicates as one element.
        let cn = vec![
            ("foo".to_string(), "a.rs".to_string()),
            ("foo".to_string(), "a.rs".to_string()),
        ];
        let gn = vec![("foo".to_string(), "a.rs".to_string())];
        let diff = compare_query_results(&cn, &gn);
        assert_eq!(diff, QueryDiff::Match { count: 1 });
    }

    #[test]
    fn compare_critical_diff_when_gitnexus_has_extra_symbol() {
        let cn = vec![
            ("foo".to_string(), "a.rs".to_string()),
            ("bar".to_string(), "b.rs".to_string()),
        ];
        let gn = vec![
            ("foo".to_string(), "a.rs".to_string()),
            ("bar".to_string(), "b.rs".to_string()),
            ("baz".to_string(), "c.rs".to_string()), // extra in gitnexus
        ];
        let diff = compare_query_results(&cn, &gn);
        match diff {
            QueryDiff::CriticalDiff { missing_in_codenexus, missing_in_gitnexus } => {
                assert_eq!(missing_in_codenexus, vec![("baz".to_string(), "c.rs".to_string())]);
                assert!(missing_in_gitnexus.is_empty());
            }
            other => panic!("expected CriticalDiff, got {other:?}"),
        }
    }

    #[test]
    fn compare_critical_diff_when_codenexus_has_extra_symbol() {
        let cn = vec![
            ("foo".to_string(), "a.rs".to_string()),
            ("bar".to_string(), "b.rs".to_string()),
            ("qux".to_string(), "d.rs".to_string()), // extra in codenexus
        ];
        let gn = vec![
            ("foo".to_string(), "a.rs".to_string()),
            ("bar".to_string(), "b.rs".to_string()),
        ];
        let diff = compare_query_results(&cn, &gn);
        match diff {
            QueryDiff::CriticalDiff { missing_in_codenexus, missing_in_gitnexus } => {
                assert!(missing_in_codenexus.is_empty());
                assert_eq!(missing_in_gitnexus, vec![("qux".to_string(), "d.rs".to_string())]);
            }
            other => panic!("expected CriticalDiff, got {other:?}"),
        }
    }

    #[test]
    fn compare_both_sides_missing_is_critical_diff() {
        let cn = vec![("foo".to_string(), "a.rs".to_string())];
        let gn = vec![("bar".to_string(), "b.rs".to_string())];
        let diff = compare_query_results(&cn, &gn);
        match diff {
            QueryDiff::CriticalDiff { missing_in_codenexus, missing_in_gitnexus } => {
                assert_eq!(missing_in_codenexus, vec![("bar".to_string(), "b.rs".to_string())]);
                assert_eq!(missing_in_gitnexus, vec![("foo".to_string(), "a.rs".to_string())]);
            }
            other => panic!("expected CriticalDiff, got {other:?}"),
        }
    }

    #[test]
    fn compare_empty_sets_match() {
        let diff = compare_query_results(&[], &[]);
        assert_eq!(diff, QueryDiff::Match { count: 0 });
    }
}
