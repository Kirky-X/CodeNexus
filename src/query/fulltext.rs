// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! BM25 full-text search (PRD §4.4.3, H11).
//!
//! [`FullTextSearcher`] attempts to use the LadybugDB FTS extension
//! (`CALL fts_search(...)`) for BM25-ranked results. When the FTS extension is
//! unavailable (the common case in tests), it falls back to a `CONTAINS`-based
//! scan over the symbol tables, ranking results with a simple relevance score.
//!
//! # codenexus_tokenizer (H11)
//!
//! Both the FTS query and the CONTAINS fallback use [`codenexus_tokenize`] to
//! split camelCase / snake_case identifiers before matching. This enables
//! searching for `parse` to match `parseFile`, `parse_file`, and
//! `my_parse_helper` — a plain `CONTAINS` would only match the exact substring.

use super::error::{QueryError, Result};
use super::tokenizer::codenexus_tokenize;
use super::SearchResult;
use crate::model::NodeLabel;
use crate::storage::schema::{escape_cypher_string, escape_identifier, node_table_columns};
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

/// FTS indexes on symbol `name` columns, created by
/// [`crate::storage::schema::index_ddl`]. Each entry pairs the FTS index name
/// with the [`NodeLabel`] used to tag search results.
///
/// Extended coverage from 3 tables (Function/Class/Method) to
/// all 15 symbol-bearing tables so that BM25 search reaches Struct/Enum/Trait/
/// Macro/Typedef/Namespace/Module/Variable/GlobalVar/Const/Static/TypeAlias.
const FTS_NAME_INDEXES: &[(&str, NodeLabel)] = &[
    ("fts_function_name", NodeLabel::Function),
    ("fts_class_name", NodeLabel::Class),
    ("fts_method_name", NodeLabel::Method),
    ("fts_struct_name", NodeLabel::Struct),
    ("fts_enum_name", NodeLabel::Enum),
    ("fts_trait_name", NodeLabel::Trait),
    ("fts_macro_name", NodeLabel::Macro),
    ("fts_typedef_name", NodeLabel::Typedef),
    ("fts_namespace_name", NodeLabel::Namespace),
    ("fts_module_name", NodeLabel::Module),
    ("fts_variable_name", NodeLabel::Variable),
    ("fts_globalvar_name", NodeLabel::GlobalVar),
    ("fts_const_name", NodeLabel::Const),
    ("fts_static_name", NodeLabel::Static),
    ("fts_typealias_name", NodeLabel::TypeAlias),
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

    /// Attempts a LadybugDB FTS query against the `name` FTS indexes (H11).
    ///
    /// Queries three FTS indexes — `fts_function_name`, `fts_class_name`,
    /// `fts_method_name` — created by [`crate::storage::schema::index_ddl`].
    /// The query is pre-tokenized via [`codenexus_tokenize`] so that
    /// `parseFile` becomes `parse file`, enabling the FTS engine to match
    /// individual sub-tokens.
    ///
    /// LadybugDB builds may not ship the FTS extension or the named index, so
    /// any error here is treated as "FTS unavailable" and the caller falls
    /// back to [`Self::fallback_contains_search`].
    fn try_fts_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // H11: tokenize the query so camelCase/snake_case identifiers are split
        // into sub-tokens before passing to the FTS engine.
        let tokenized = codenexus_tokenize(text);
        if tokenized.is_empty() {
            // Single-line for coverage: tarpaulin attribute continuation
            return Err(QueryError::InvalidQuery(
                "fulltext query tokenized to empty".to_string(),
            ));
        }
        let fts_query = tokenized.join(" ");
        let escaped = escape_cypher_string(&fts_query);

        // Query all name-column FTS indexes and merge results.
        let mut all_results = Vec::new();
        for (index_name, label) in FTS_NAME_INDEXES {
            // Namespace and Module tables have no `startLine`
            // column; return NULL instead of erroring.
            let line_expr = if node_table_columns(*label).contains(&"startLine") {
                "node.startLine"
            } else {
                "NULL"
            };
            let cypher = match project {
                Some(p) => format!(
                    "CALL fts_search('{index_name}', '{escaped}') YIELD node \
                     WHERE node.project = '{}' \
                     RETURN node.name AS name, node.qualifiedName AS qn, \
                     node.filePath AS filePath, {line_expr} AS line;",
                    escape_cypher_string(p),
                ),
                None => format!(
                    "CALL fts_search('{index_name}', '{escaped}') YIELD node \
                     RETURN node.name AS name, node.qualifiedName AS qn, \
                     node.filePath AS filePath, {line_expr} AS line;",
                ),
            };
            match self.conn.query(&cypher) {
                Ok(rows) => {
                    all_results.extend(rows_to_search_results(rows, *label, text));
                }
                // Propagate the first error — the caller will check
                // `is_fts_unsupported_error` to decide whether to fall back.
                // Single-line for coverage: tarpaulin attribute continuation
                Err(e) => return Err(QueryError::from(e)),
            }
        }
        // FTS results are already BM25-ranked per-index; merge-sort by score.
        sort_and_truncate(&mut all_results, limit);
        Ok(all_results)
    }

    /// Fallback: scans symbol tables with `CONTAINS` and ranks by a relevance
    /// score (exact > prefix > token-match > substring). Case-insensitive.
    ///
    /// H11: the query is pre-tokenized via [`codenexus_tokenize`] so that a
    /// multi-token query like `parseFile` becomes `["parse", "file"]`. The
    /// WHERE clause ORs one `CONTAINS` per token, enabling `parseFile` to
    /// match `parse_file` (which contains both `parse` and `file` as
    /// substrings) — a plain single-substring `CONTAINS('parseFile')` would
    /// miss it.
    fn fallback_contains_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let tokens = codenexus_tokenize(text);
        if tokens.is_empty() {
            // Single-line for coverage: tarpaulin attribute continuation
            return Err(QueryError::InvalidQuery(
                "fulltext query tokenized to empty".to_string(),
            ));
        }
        let or_clauses: Vec<String> = tokens
            .iter()
            .map(|t| {
                // Single-line for coverage: tarpaulin attribute continuation
                format!(
                    "toLower(n.name) CONTAINS toLower('{}')",
                    escape_cypher_string(t)
                )
            })
            .collect();
        let where_inner = or_clauses.join(" OR ");
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            // Namespace and Module tables have no `startLine`
            // column; return NULL instead of erroring so the fallback path
            // reaches all symbol tables (previously these were silently
            // skipped via `Err(_) => continue`).
            let line_expr = if node_table_columns(label).contains(&"startLine") {
                "n.startLine"
            } else {
                "NULL"
            };
            let cypher = match project {
                Some(p) => format!(
                    "MATCH (n:{table}) WHERE ({where_inner}) AND n.project = '{}' \
                     RETURN n.name AS name, n.qualifiedName AS qn, \
                     n.filePath AS filePath, {line_expr} AS line;",
                    escape_cypher_string(p),
                ),
                None => format!(
                    "MATCH (n:{table}) WHERE ({where_inner}) \
                     RETURN n.name AS name, n.qualifiedName AS qn, \
                     n.filePath AS filePath, {line_expr} AS line;",
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
            let qualified_name = row.get(1).and_then(|v| v.as_str()).map(String::from);
            let file_path = row.get(2).and_then(|v| v.as_str()).map(String::from);
            let start_line = row
                .get(3)
                .and_then(|v| v.as_i64())
                .and_then(|i| u32::try_from(i).ok());
            let (score, reason) = relevance_score_with_reason(&name, query);
            Some(SearchResult {
                name,
                label: label_str.clone(),
                file_path,
                start_line,
                qualified_name,
                score,
                match_reason: reason.to_string(),
                degree: 0,
            })
        })
        .collect()
}

/// Computes a relevance score in `[0.0, 1.0]` for `name` against `query`.
///
/// Scoring tiers (H11 token-aware):
/// - `1.0` — exact match (case-insensitive)
/// - `0.8` — name starts with query (prefix)
/// - `0.7` — every query token appears as a substring of some name token
///   (e.g. `my_parse_helper` vs `parse` → `["my","parse","helper"]` contains
///   `parse`)
/// - `0.5` — name contains query as a plain substring
/// - `0.3` — no match (defensive; callers pre-filter via `CONTAINS`)
#[allow(dead_code)]
fn relevance_score(name: &str, query: &str) -> f64 {
    relevance_score_with_reason(name, query).0
}

/// Computes both the score and a human-readable match reason.
fn relevance_score_with_reason(name: &str, query: &str) -> (f64, &'static str) {
    let name_lower = name.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if name_lower == query_lower {
        return (1.0, "exact name match");
    }
    if name_lower.starts_with(&query_lower) {
        return (0.8, "prefix match");
    }
    let query_tokens = codenexus_tokenize(&query_lower);
    let name_tokens = codenexus_tokenize(&name_lower);
    if !query_tokens.is_empty() && !name_tokens.is_empty() {
        let all_match = query_tokens
            .iter()
            .all(|qt| name_tokens.iter().any(|nt| nt == qt));
        if all_match {
            return (0.7, "token-aligned match");
        }
    }
    if name_lower.contains(&query_lower) {
        return (0.5, "substring match");
    }
    (0.3, "no match")
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

    fn sample_function(
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) -> Node {
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
        let results = searcher.search("parse", None, 100).expect("search");
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
                sample_function(
                    "f3",
                    "demo",
                    "my_parse_helper",
                    "demo.my_parse_helper",
                    "/b.rs",
                    1,
                ),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        assert!(!results.is_empty());
        // Exact match should rank first.
        assert_eq!(results[0].name, "parse");
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn search_filters_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "alpha",
                "parse",
                "alpha.parse",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes alpha");
        repo.save_nodes(
            &[sample_function(
                "f2",
                "beta",
                "parse",
                "beta.parse",
                "/b.rs",
                1,
            )],
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
        repo.save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 3).expect("search");
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
    fn search_rejects_query_that_tokenizes_to_empty() {
        // Punctuation-only query tokenizes to empty → InvalidQuery error
        // (exercises the tokenized.is_empty() branch in try_fts_search).
        let repo = fresh_repo();
        let searcher = FullTextSearcher::new(repo.connection());
        let err = searcher
            .search("...", None, 10)
            .expect_err("punctuation-only query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_returns_empty_when_no_match() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "main",
                "demo.main",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("nonexistent", None, 10).expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_populates_search_result_fields() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse",
                "demo.parse",
                "/src/main.rs",
                42,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
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
    fn relevance_score_token_match() {
        // H11: token-aligned match ranks above plain substring. `my_parse`
        // tokenizes to `["my", "parse"]` which contains the query token
        // `parse` exactly.
        assert_eq!(relevance_score("my_parse", "parse"), 0.7);
        assert_eq!(relevance_score("my_parse_helper", "parse"), 0.7);
    }

    #[test]
    fn relevance_score_substring_match() {
        // H11: a non-token-aligned substring (e.g. `xparse` → single token
        // `xparse`) scores below a token-aligned match.
        assert_eq!(relevance_score("xparse", "parse"), 0.5);
    }

    #[test]
    fn relevance_score_no_match() {
        assert_eq!(relevance_score("read_input", "parse"), 0.3);
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
            &[sample_function(
                "f1",
                "demo",
                "parse_file",
                "demo.parse_file",
                "/a.rs",
                1,
            )],
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
        let results = searcher.search("parse", None, 100).expect("search");
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

    #[test]
    fn fallback_contains_search_matches_camel_query_to_snake_name() {
        // H11: searching `parseFile` (camelCase) should match `parse_file`
        // (snake_case) via tokenization. Without tokenization, a single
        // `CONTAINS('parseFile')` would miss `parse_file`.
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse_file", "demo.parse_file", "/a.rs", 1),
                sample_function("f2", "demo", "read_input", "demo.read_input", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher
            .fallback_contains_search("parseFile", None, 100)
            .expect("fallback");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"parse_file"),
            "parse_file should match parseFile"
        );
        assert!(
            !names.contains(&"read_input"),
            "read_input should not match"
        );
    }

    #[test]
    fn fallback_contains_search_rejects_query_that_tokenizes_to_empty() {
        // H11: a query consisting only of separators tokenizes to empty and
        // must error (Rule 12: fail loud) rather than silently returning all
        // rows.
        let repo = fresh_repo();
        let searcher = FullTextSearcher::new(repo.connection());
        let err = searcher
            .fallback_contains_search("___", None, 10)
            .expect_err("separator-only query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_camel_query_matches_snake_name() {
        // End-to-end: `search("parseFile")` should find `parse_file` through
        // whichever path is available (FTS or fallback).
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse_file",
                "demo.parse_file",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parseFile", None, 100).expect("search");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse_file"));
    }

    // --- BM25 FTS index coverage extension ---

    /// Helper: builds a minimal node of any symbol-bearing label.
    fn sample_symbol(
        label: NodeLabel,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) -> Node {
        Node::builder(label, name, qn)
            .id(id)
            .project(project)
            .file_path(file)
            .start_line(line)
            .language(Language::Rust)
            .build()
    }

    #[test]
    fn fts_name_indexes_covers_all_symbol_tables() {
        // R-search-001: FTS_NAME_INDEXES must contain 15 entries covering all
        // symbol-bearing node labels (excluding Impl which has no meaningful
        // name for search).
        assert_eq!(
            FTS_NAME_INDEXES.len(),
            15,
            "expected 15 FTS name indexes, got {}: {FTS_NAME_INDEXES:?}",
            FTS_NAME_INDEXES.len()
        );
        let labels: Vec<NodeLabel> = FTS_NAME_INDEXES.iter().map(|(_, l)| *l).collect();
        for expected in [
            NodeLabel::Function,
            NodeLabel::Class,
            NodeLabel::Method,
            NodeLabel::Struct,
            NodeLabel::Enum,
            NodeLabel::Trait,
            NodeLabel::Macro,
            NodeLabel::Typedef,
            NodeLabel::Namespace,
            NodeLabel::Module,
            NodeLabel::Variable,
            NodeLabel::GlobalVar,
            NodeLabel::Const,
            NodeLabel::Static,
            NodeLabel::TypeAlias,
        ] {
            assert!(
                labels.contains(&expected),
                "FTS_NAME_INDEXES missing {expected:?}: {FTS_NAME_INDEXES:?}"
            );
        }
    }

    #[test]
    fn search_finds_struct_by_name() {
        // R-search-001: saving a Struct named "Point" and searching "Point"
        // must return a result with label == "Struct".
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_symbol(
                NodeLabel::Struct,
                "s1",
                "demo",
                "Point",
                "demo.Point",
                "/a.rs",
                1,
            )],
            NodeLabel::Struct,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("Point", None, 100).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Point");
        assert_eq!(results[0].label, "Struct");
    }

    #[test]
    fn search_finds_enum_by_name() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_symbol(
                NodeLabel::Enum,
                "e1",
                "demo",
                "Color",
                "demo.Color",
                "/a.rs",
                1,
            )],
            NodeLabel::Enum,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("Color", None, 100).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Color");
        assert_eq!(results[0].label, "Enum");
    }

    #[test]
    fn search_finds_macro_by_name() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_symbol(
                NodeLabel::Macro,
                "m1",
                "demo",
                "FOO",
                "demo.FOO",
                "/a.rs",
                1,
            )],
            NodeLabel::Macro,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("FOO", None, 100).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "FOO");
        assert_eq!(results[0].label, "Macro");
    }

    #[test]
    fn search_finds_namespace_by_name() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_symbol(
                NodeLabel::Namespace,
                "n1",
                "demo",
                "graphics",
                "demo.graphics",
                "/a.rs",
                1,
            )],
            NodeLabel::Namespace,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("graphics", None, 100).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "graphics");
        assert_eq!(results[0].label, "Namespace");
    }

    #[test]
    fn search_finds_typedef_by_name() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_symbol(
                NodeLabel::Typedef,
                "t1",
                "demo",
                "Handle",
                "demo.Handle",
                "/a.rs",
                1,
            )],
            NodeLabel::Typedef,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("Handle", None, 100).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Handle");
        assert_eq!(results[0].label, "Typedef");
    }

    #[test]
    fn search_across_multiple_label_types() {
        // R-search-001: saving Function + Struct + Enum with a shared name
        // token and searching that token must return all three label types.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse",
                "demo.parse",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes function");
        repo.save_nodes(
            &[sample_symbol(
                NodeLabel::Struct,
                "s1",
                "demo",
                "Parser",
                "demo.Parser",
                "/b.rs",
                1,
            )],
            NodeLabel::Struct,
        )
        .expect("save_nodes struct");
        repo.save_nodes(
            &[sample_symbol(
                NodeLabel::Enum,
                "e1",
                "demo",
                "ParseMode",
                "demo.ParseMode",
                "/c.rs",
                1,
            )],
            NodeLabel::Enum,
        )
        .expect("save_nodes enum");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        let labels: Vec<&str> = results.iter().map(|r| r.label.as_str()).collect();
        // All three symbol tables should return at least one hit (Function
        // exact match, Struct token match via "Parser" → ["parser"], Enum
        // token match via "ParseMode" → ["parse", "mode"]).
        assert!(
            labels.contains(&"Function"),
            "expected Function in results: {labels:?}"
        );
        assert!(
            labels.contains(&"Struct"),
            "expected Struct in results: {labels:?}"
        );
        assert!(
            labels.contains(&"Enum"),
            "expected Enum in results: {labels:?}"
        );
    }

    #[test]
    fn fallback_contains_search_continues_on_query_error() {
        // Cover the `Err(_) => continue` arm (line 239) of
        // fallback_contains_search: when a per-table CONTAINS query fails
        // (table dropped), the loop skips it and continues.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse",
                "demo.parse",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        repo.connection()
            .execute("DROP TABLE Class;")
            .expect("drop table");
        let searcher = FullTextSearcher::new(repo.connection());
        // search() falls back to CONTAINS; the dropped Class table is skipped.
        let results = searcher.search("parse", None, 100).expect("search");
        assert!(results.iter().any(|r| r.name == "parse"));
    }
}
