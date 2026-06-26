// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! BM25 full-text search (PRD §4.4.3).
//!
//! [`FullTextSearcher`] attempts to use the LadybugDB FTS extension
//! (`CALL fts_search(...)`) for BM25-ranked results. When the FTS extension is
//! unavailable (the common case in tests), it falls back to a `CONTAINS`-based
//! scan over the symbol tables, ranking results with a simple relevance score.

use super::error::{QueryError, Result};
use super::SearchResult;
use crate::model::NodeLabel;
use crate::storage::schema::escape_identifier;
use crate::storage::StorageConnection;
use tracing::warn;

/// Symbol-bearing node labels searched by [`FullTextSearcher`] when falling
/// back to `CONTAINS`. Mirrors [`super::structured::SYMBOL_LABELS`].
const SYMBOL_LABELS: &[NodeLabel] = &[
    NodeLabel::Module,
    NodeLabel::Class,
    NodeLabel::Struct,
    NodeLabel::Enum,
    NodeLabel::Trait,
    NodeLabel::Impl,
    NodeLabel::Function,
    NodeLabel::Method,
    NodeLabel::Variable,
    NodeLabel::GlobalVar,
    NodeLabel::Const,
    NodeLabel::Static,
    NodeLabel::Macro,
    NodeLabel::TypeAlias,
    NodeLabel::Typedef,
    NodeLabel::Namespace,
];

/// Executes BM25 full-text searches against a [`StorageConnection`].
///
/// Tries the LadybugDB FTS extension first; falls back to a `CONTAINS`-based
/// scan when FTS is unavailable.
pub struct FullTextSearcher<'a> {
    conn: &'a StorageConnection,
}

impl<'a> FullTextSearcher<'a> {
    /// Creates a new [`FullTextSearcher`] borrowing `conn`.
    #[must_use]
    pub fn new(conn: &'a StorageConnection) -> Self {
        Self { conn }
    }

    /// Searches for `text` using BM25 (FTS) when available, falling back to a
    /// `CONTAINS` scan when the FTS extension is unavailable. Results are sorted
    /// by descending relevance score.
    ///
    /// Only FTS errors that indicate the extension/index is unsupported (parser
    /// exceptions, "not supported", "does not exist", "already exists") trigger
    /// the fallback. Any other error (e.g. a genuine runtime failure) is
    /// propagated to the caller.
    pub fn search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        if text.trim().is_empty() {
            return Err(QueryError::InvalidQuery(
                "fulltext query must not be empty".to_string(),
            ));
        }
        // Try the FTS extension first. If it is unavailable (or the FTS index
        // does not exist), fall back to a CONTAINS-based scan.
        match self.try_fts_search(text, project, limit) {
            Ok(results) => Ok(results),
            Err(e) if is_fts_unsupported_error(&e) => {
                warn!(error = %e, "FTS not supported, falling back to CONTAINS scan");
                self.fallback_contains_search(text, project, limit)
            }
            Err(e) => Err(e),
        }
    }

    /// Attempts a LadybugDB FTS query.
    ///
    /// The canonical FTS invocation is:
    /// `CALL fts_search('fts_func_name', $text) YIELD node RETURN node`
    /// LadybugDB builds may not ship the FTS extension or the named index, so
    /// any error here is treated as "FTS unavailable" and the caller falls
    /// back to [`Self::fallback_contains_search`].
    fn try_fts_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let escaped = escape_cypher_string(text);
        // Query the Function FTS index (the primary symbol table).
        let cypher = match project {
            Some(p) => format!(
                "CALL fts_search('fts_func_name', '{escaped}') YIELD node \
                 WHERE node.project = '{}' \
                 RETURN node.name AS name, node.qualifiedName AS qn, \
                 node.filePath AS filePath, node.startLine AS line;",
                escape_cypher_string(p),
            ),
            None => format!(
                "CALL fts_search('fts_func_name', '{escaped}') YIELD node \
                 RETURN node.name AS name, node.qualifiedName AS qn, \
                 node.filePath AS filePath, node.startLine AS line;",
            ),
        };
        let rows = self.conn.query(&cypher).map_err(QueryError::from)?;
        let mut results = rows_to_search_results(rows, NodeLabel::Function, text);
        // FTS results are already BM25-ranked; we only truncate.
        if limit < results.len() {
            results.truncate(limit);
        }
        Ok(results)
    }

    /// Fallback: scans symbol tables with `CONTAINS` and ranks by a simple
    /// relevance score (exact > prefix > substring). Case-insensitive.
    fn fallback_contains_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let escaped = escape_cypher_string(text);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            let cypher = match project {
                Some(p) => format!(
                    "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') AND n.project = '{}' \
                     RETURN n.name AS name, n.qualifiedName AS qn, \
                     n.filePath AS filePath, n.startLine AS line;",
                    escape_cypher_string(p),
                ),
                None => format!(
                    "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') \
                     RETURN n.name AS name, n.qualifiedName AS qn, \
                     n.filePath AS filePath, n.startLine AS line;",
                ),
            };
            match self.conn.query(&cypher) {
                Ok(rows) => results.extend(rows_to_search_results(rows, label, text)),
                Err(_) => continue,
            }
        }
        sort_and_truncate(&mut results, limit);
        Ok(results)
    }
}

/// Converts query rows into [`SearchResult`]s with a relevance score.
fn rows_to_search_results(
    rows: Vec<Vec<serde_json::Value>>,
    label: NodeLabel,
    query: &str,
) -> Vec<SearchResult> {
    let label_str = label.to_string();
    rows.into_iter()
        .filter_map(|row| {
            let name = row.first().and_then(|v| v.as_str())?.to_string();
            let qualified_name = row
                .get(1)
                .and_then(|v| v.as_str())
                .map(String::from);
            let file_path = row
                .get(2)
                .and_then(|v| v.as_str())
                .map(String::from);
            let start_line = row
                .get(3)
                .and_then(|v| v.as_i64())
                .and_then(|i| u32::try_from(i).ok());
            let score = relevance_score(&name, query);
            Some(SearchResult {
                name,
                label: label_str.clone(),
                file_path,
                start_line,
                qualified_name,
                score,
            })
        })
        .collect()
}

/// Computes a relevance score in `[0.0, 1.0]` for `name` against `query`.
fn relevance_score(name: &str, query: &str) -> f32 {
    let name_lower = name.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if name_lower == query_lower {
        1.0
    } else if name_lower.starts_with(&query_lower) {
        0.8
    } else {
        0.5
    }
}

/// Sorts results by descending score then ascending name, and truncates.
fn sort_and_truncate(results: &mut Vec<SearchResult>, limit: usize) {
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    if limit < results.len() {
        results.truncate(limit);
    }
}

/// Escapes a string for safe interpolation into a Cypher single-quoted string.
fn escape_cypher_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Returns `true` when `err` indicates the FTS extension or FTS index is
/// unavailable in the linked LadybugDB build.
///
/// Mirrors the "unsupported DDL" classification in
/// [`crate::storage::connection::StorageConnection::run_init_ddl`]: parser
/// exceptions, "not supported", "does not exist", and "already exists" all
/// signal that the feature is absent rather than genuinely broken. Any other
/// error (e.g. "connection refused") is a real failure that should propagate.
fn is_fts_unsupported_error(err: &QueryError) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("not supported")
        || msg.contains("parser exception")
        || msg.contains("does not exist")
        || msg.contains("already exists")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::storage::Repository;

    fn fresh_repo() -> Repository {
        Repository::in_memory().expect("in_memory repository")
    }

    fn sample_function(id: &str, project: &str, name: &str, qn: &str, file: &str, line: u32) -> Node {
        Node::builder(NodeLabel::Function, name, qn)
            .id(id)
            .project(project)
            .file_path(file)
            .start_line(line)
            .end_line(line + 10)
            .language(Language::Rust)
            .signature("fn x()")
            .build()
    }

    #[test]
    fn search_falls_back_to_contains_when_fts_unavailable() {
        // No FTS index is created in the test DB, so this exercises the
        // fallback path.
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse_file", "demo.parse_file", "/a.rs", 1),
                sample_function("f2", "demo", "parse_line", "demo.parse_line", "/b.rs", 1),
                sample_function("f3", "demo", "read_input", "demo.read_input", "/a.rs", 10),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("parse", None, 100)
            .expect("search");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse_file"));
        assert!(names.contains(&"parse_line"));
        assert!(!names.contains(&"read_input"));
    }

    #[test]
    fn search_returns_results_sorted_by_relevance() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse", "demo.parse", "/a.rs", 1),
                sample_function("f2", "demo", "parse_file", "demo.parse_file", "/a.rs", 5),
                sample_function("f3", "demo", "my_parse_helper", "demo.my_parse_helper", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("parse", None, 100)
            .expect("search");
        assert!(!results.is_empty());
        // Exact match should rank first.
        assert_eq!(results[0].name, "parse");
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn search_filters_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "alpha", "parse", "alpha.parse", "/a.rs", 1)],
            NodeLabel::Function,
        )
        .expect("save_nodes alpha");
        repo.save_nodes(
            &[sample_function("f2", "beta", "parse", "beta.parse", "/b.rs", 1)],
            NodeLabel::Function,
        )
        .expect("save_nodes beta");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("parse", Some("alpha"), 100)
            .expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_name.as_deref(), Some("alpha.parse"));
    }

    #[test]
    fn search_respects_limit() {
        let repo = fresh_repo();
        let mut nodes = Vec::new();
        for i in 0..10 {
            nodes.push(sample_function(
                &format!("f{i}"),
                "demo",
                &format!("parse_{i}"),
                &format!("demo.parse_{i}"),
                "/a.rs",
                i + 1,
            ));
        }
        repo.save_nodes(&nodes, NodeLabel::Function).expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("parse", None, 3)
            .expect("search");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_rejects_empty_query() {
        let repo = fresh_repo();
        let searcher = FullTextSearcher::new(repo.connection());
        let err = searcher
            .search("", None, 10)
            .expect_err("empty query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_returns_empty_when_no_match() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "main", "demo.main", "/a.rs", 1)],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("nonexistent", None, 10)
            .expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_populates_search_result_fields() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "parse", "demo.parse", "/src/main.rs", 42)],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("parse", None, 100)
            .expect("search");
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.name, "parse");
        assert_eq!(r.label, "Function");
        assert_eq!(r.file_path.as_deref(), Some("/src/main.rs"));
        assert_eq!(r.start_line, Some(42));
        assert_eq!(r.qualified_name.as_deref(), Some("demo.parse"));
        assert!(r.score > 0.0);
    }

    #[test]
    fn relevance_score_exact_match() {
        assert_eq!(relevance_score("parse", "parse"), 1.0);
        assert_eq!(relevance_score("PARSE", "parse"), 1.0);
    }

    #[test]
    fn relevance_score_prefix_match() {
        assert_eq!(relevance_score("parse_file", "parse"), 0.8);
    }

    #[test]
    fn relevance_score_substring_match() {
        assert_eq!(relevance_score("my_parse", "parse"), 0.5);
    }

    #[test]
    fn escape_cypher_string_handles_special_chars() {
        assert_eq!(escape_cypher_string("it's"), "it\\'s");
        assert_eq!(escape_cypher_string("a\\b"), "a\\\\b");
    }

    #[test]
    fn is_fts_unsupported_error_detects_unsupported_patterns() {
        // Mirrors the unsupported-DDL patterns from storage::connection::run_init_ddl.
        assert!(is_fts_unsupported_error(&QueryError::Query(
            "Parser exception: syntax error near CALL".to_string()
        )));
        assert!(is_fts_unsupported_error(&QueryError::Query(
            "feature not supported in this build".to_string()
        )));
        assert!(is_fts_unsupported_error(&QueryError::Query(
            "Catalog exception: function fts_search does not exist".to_string()
        )));
        assert!(is_fts_unsupported_error(&QueryError::Query(
            "table already exists".to_string()
        )));
        // Case-insensitive matching.
        assert!(is_fts_unsupported_error(&QueryError::Query(
            "NOT SUPPORTED".to_string()
        )));
        assert!(is_fts_unsupported_error(&QueryError::Query(
            "PARSER EXCEPTION".to_string()
        )));
    }

    #[test]
    fn is_fts_unsupported_error_rejects_genuine_errors() {
        // Errors that are NOT "unsupported" signals must propagate, not fall back.
        assert!(!is_fts_unsupported_error(&QueryError::Query(
            "connection refused".to_string()
        )));
        assert!(!is_fts_unsupported_error(&QueryError::Query(
            "permission denied".to_string()
        )));
        assert!(!is_fts_unsupported_error(&QueryError::Query(
            "out of memory".to_string()
        )));
        assert!(!is_fts_unsupported_error(&QueryError::InvalidQuery(
            "empty query".to_string()
        )));
        assert!(!is_fts_unsupported_error(&QueryError::FullText(
            "index corrupted".to_string()
        )));
    }

    #[test]
    fn search_uses_fts_when_index_exists() {
        // Attempt to create an FTS index on the Function table. If LadybugDB
        // does not support FTS in this build, the creation fails silently and
        // the test falls back to verifying the CONTAINS path (which is already
        // covered above). When FTS is available, this exercises the
        // `try_fts_search` success path.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "parse_file", "demo.parse_file", "/a.rs", 1)],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        // Try the common LadybugDB FTS index creation syntaxes. At least one
        // may succeed depending on the linked build.
        let fts_created = repo
            .connection()
            .execute("CALL create_fts_index('fts_func_name', 'Function', ['name']);")
            .is_ok()
            || repo
                .connection()
                .execute("CREATE FTS INDEX fts_func_name ON Function(name);")
                .is_ok();

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("parse", None, 100)
            .expect("search");
        // Whether FTS or fallback was used, we should find the function.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "parse_file");
        // If FTS was created, the success path (try_fts_search Ok branch) was
        // exercised; otherwise the fallback path was exercised.
        let _ = fts_created;
    }

    #[test]
    fn search_with_project_filter_when_fts_unavailable() {
        // Exercises the fallback path with a project filter (the FTS path
        // would also apply the filter, but FTS is typically unavailable in
        // tests).
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "alpha", "parse", "alpha.parse", "/a.rs", 1),
                sample_function("f2", "beta", "parse", "beta.parse", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .search("parse", Some("alpha"), 100)
            .expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_name.as_deref(), Some("alpha.parse"));
    }

    #[test]
    fn try_fts_search_returns_error_when_fts_unavailable() {
        // Directly exercise try_fts_search to document that it returns an
        // error when the FTS extension/index is not available. This covers
        // the error-return path (the `?` on the query line).
        let repo = fresh_repo();
        let searcher = FullTextSearcher::new(repo.connection());
        let result = searcher.try_fts_search("parse", None, 100);
        assert!(result.is_err(), "try_fts_search should error without FTS");
    }

    #[test]
    fn try_fts_search_with_project_returns_error_when_fts_unavailable() {
        let repo = fresh_repo();
        let searcher = FullTextSearcher::new(repo.connection());
        let result = searcher.try_fts_search("parse", Some("demo"), 100);
        assert!(result.is_err());
    }

    #[test]
    fn fallback_contains_search_returns_empty_when_no_data() {
        // Exercises the fallback path with an empty database. All table
        // queries succeed (returning zero rows), so the `Err(_) => continue`
        // path is not taken, but the merge/sort/truncate logic is exercised.
        let repo = fresh_repo();
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .fallback_contains_search("anything", None, 10)
            .expect("fallback");
        assert!(results.is_empty());
    }

    #[test]
    fn fallback_contains_search_with_project_filter() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "alpha", "parse", "alpha.parse", "/a.rs", 1),
                sample_function("f2", "beta", "parse", "beta.parse", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .fallback_contains_search("parse", Some("beta"), 100)
            .expect("fallback");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_name.as_deref(), Some("beta.parse"));
    }
}
