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

    #[test]
    fn extract_strips_inline_comments_within_block() {
        // `//` comments inside the block (after the marker) must be stripped,
        // leaving only the Cypher text. This covers the `line[..idx]` branch.
        let content = "\
// === CODENEXUS ===
MATCH (n) RETURN n // limit for safety
// explanatory note
LIMIT 10

// === GITNEXUS ===
MATCH (m) RETURN m
LIMIT 5
";
        let q = extract_query_for_side(content, Side::Codenexus).unwrap();
        assert!(q.contains("MATCH (n) RETURN n"));
        assert!(q.contains("LIMIT 10"));
        // The inline `// limit for safety` and the standalone `// explanatory note`
        // must NOT appear in the extracted block.
        assert!(!q.contains("limit for safety"));
        assert!(!q.contains("explanatory note"));
        // Markers themselves must not leak.
        assert!(!q.contains("==="));
    }

    #[test]
    fn extract_returns_error_when_block_is_empty() {
        // Marker is present but no Cypher between it and the next marker.
        // Covers the `bail!("empty Cypher block")` branch.
        let content = "\
// === CODENEXUS ===

// === GITNEXUS ===
MATCH (m) RETURN m
";
        let err = extract_query_for_side(content, Side::Codenexus).unwrap_err();
        assert!(
            err.to_string().contains("empty Cypher block"),
            "expected empty-block error, got: {err}"
        );
    }

    #[test]
    fn extract_gitnexus_block_at_eof_without_other_marker() {
        // When the gitnexus marker is the last one with no terminator,
        // the extractor must read to end-of-string. Covers the
        // `unwrap_or(content.len())` branch.
        let content = "\
// === CODENEXUS ===
MATCH (a) RETURN a

// === GITNEXUS ===
MATCH (b) RETURN b
LIMIT 50
";
        let q = extract_query_for_side(content, Side::Gitnexus).unwrap();
        assert!(q.contains("MATCH (b) RETURN b"));
        assert!(q.contains("LIMIT 50"));
        assert!(!q.contains("==="));
    }

    #[test]
    fn execute_codenexus_query_returns_rows_from_real_db() {
        // End-to-end: open a fresh DB, create a Project row, query it back via
        // execute_codenexus_query. Covers lines 102-126 (open + query + row
        // extraction). Project nodes carry `name` and `id` (string) columns;
        // both are returned as strings.
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("cov.lbug");
        let repo = codenexus::storage::repository::Repository::open(&db_path)
            .expect("Repository::open");
        // Insert a Project node so we have something to query.
        let project = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Project,
            "demo",
            "demo",
        )
        .id("proj-1")
        .build();
        repo.save_nodes(&[project], codenexus::model::NodeLabel::Project)
            .expect("save_nodes");
        drop(repo);

        let cql = "MATCH (p:Project) RETURN p.name AS name, p.id AS id";
        let rows = execute_codenexus_query(&db_path, cql).expect("execute_codenexus_query");
        assert_eq!(rows.len(), 1, "expected exactly one Project row");
        assert_eq!(rows[0].0, "demo");
        assert_eq!(rows[0].1, "proj-1");
    }

    #[test]
    fn execute_codenexus_query_propagates_open_error_for_missing_db() {
        // Pointing at a path whose parent doesn't exist must surface an error
        // rather than panicking or returning an empty vec. Covers the
        // `with_context` error branch.
        let bogus = std::path::Path::new("/nonexistent/dir/missing.lbug");
        let err = execute_codenexus_query(bogus, "MATCH (n) RETURN n").unwrap_err();
        assert!(
            err.to_string().contains("failed to open CodeNexus DB"),
            "expected open-DB error, got: {err}"
        );
    }

    #[test]
    fn execute_codenexus_query_returns_empty_for_empty_table() {
        // Querying an empty Project table must return an empty vec (not an
        // error). Covers the no-rows path of the row-mapping closure.
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("empty.lbug");
        let _ = codenexus::storage::repository::Repository::open(&db_path)
            .expect("Repository::open");
        let cql = "MATCH (p:Project) RETURN p.name AS name, p.id AS id";
        let rows = execute_codenexus_query(&db_path, cql).expect("execute_codenexus_query");
        assert!(rows.is_empty(), "expected zero rows for empty Project table");
    }

    #[test]
    fn execute_codenexus_query_falls_back_to_empty_when_column_value_is_null() {
        // Insert a Project with only `name` set; query `name` and a column
        // that exists in the schema but is null (`rootPath`). The row-mapping
        // closure must coerce null → "" (Rule 12: visible empty, not drop).
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("null.lbug");
        let repo = codenexus::storage::repository::Repository::open(&db_path)
            .expect("Repository::open");
        let project = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Project,
            "nullproj",
            "nullproj",
        )
        .id("np-1")
        .build();
        repo.save_nodes(&[project], codenexus::model::NodeLabel::Project)
            .expect("save_nodes");
        drop(repo);

        let cql = "MATCH (p:Project) RETURN p.name AS name, p.rootPath AS root";
        let rows = execute_codenexus_query(&db_path, cql).expect("execute_codenexus_query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "nullproj");
        // rootPath was never set → null → coerced to "" by the closure.
        assert_eq!(rows[0].1, "");
    }
}
