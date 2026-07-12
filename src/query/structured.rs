// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Structured search by name / type / file (PRD §4.4.2), plus multi-mode
//! search (exact / regex / fuzzy / graph-enhanced / multi-signal).
//!
//! [`StructuredSearcher`] runs Cypher `MATCH` queries against specific node
//! tables, returning [`SearchResult`] lists. Because LadybugDB stores each
//! [`NodeLabel`] in a distinct table, "search all symbols by name" is
//! implemented as a fan-out across the relevant tables followed by a merge.
//!
//! [`SearchEngine`] provides the multi-mode search dispatcher (T019–T023).

use super::error::{QueryError, Result};
use super::SearchResult;
use crate::model::NodeLabel;
use crate::storage::capability::Storage;
use crate::storage::schema::{escape_cypher_string, escape_identifier};
use crate::storage::StorageConnection;
use serde::{Deserialize, Serialize};

/// Maximum value accepted for [`SearchParams::limit`].
pub const MAX_LIMIT: usize = 500;
/// Default limit used by [`SearchParams::default`].
pub const DEFAULT_LIMIT: usize = 50;
/// Maximum Levenshtein distance accepted by fuzzy search.
pub const MAX_FUZZY_DISTANCE: usize = 3;

/// Selects which search algorithm [`SearchEngine::search`] dispatches to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMode {
    /// Exact case-insensitive name match.
    Exact,
    /// Regular-expression match on name / qualifiedName.
    Regex,
    /// Levenshtein-distance fuzzy match.
    Fuzzy,
    /// Name match combined with degree / label graph filters.
    GraphEnhanced,
    /// Multi-signal scored search (name + degree + module + test coverage).
    MultiSignal,
}

/// Parameters for [`SearchEngine::search`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchParams {
    /// Search query (exact text, regex pattern, or fuzzy needle).
    pub query: String,
    /// Match strategy.
    pub mode: SearchMode,
    /// Restrict results to these node labels (e.g. `["Function"]`).
    pub label_filter: Option<Vec<String>>,
    /// `(min, max)` inclusive degree filter for graph-enhanced mode.
    pub degree_filter: Option<(u32, u32)>,
    /// Glob-style file path filter (currently informational).
    pub file_pattern: Option<String>,
    /// Maximum results to return (clamped to [`MAX_LIMIT`]).
    pub limit: usize,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::Exact,
            label_filter: None,
            degree_filter: None,
            file_pattern: None,
            limit: DEFAULT_LIMIT,
        }
    }
}

impl SearchParams {
    /// Clamps `limit` to `[0, MAX_LIMIT]`.
    pub fn clamped_limit(&self) -> usize {
        self.limit.min(MAX_LIMIT)
    }
}

/// Multi-mode search engine backed by a [`Storage`] capability.
///
/// Dispatches to mode-specific implementations via [`SearchEngine::search`]:
/// - [`SearchMode::Exact`] → name CONTAINS (case-insensitive)
/// - [`SearchMode::Regex`] → [`SearchEngine::search_regex`]
/// - [`SearchMode::Fuzzy`] → [`SearchEngine::search_fuzzy`]
/// - [`SearchMode::GraphEnhanced`] → [`SearchEngine::search_graph_enhanced`]
/// - [`SearchMode::MultiSignal`] → graph-enhanced + [`SearchEngine::score_multi_signal`]
pub struct SearchEngine<'a> {
    storage: &'a dyn Storage,
}

impl<'a> SearchEngine<'a> {
    /// Creates a new engine borrowing the given storage capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
    }

    /// Dispatches to the appropriate search implementation based on `params.mode`.
    ///
    /// Results are sorted by descending `score`, then ascending `name`.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] for empty queries, invalid regex,
    /// or `max_distance > MAX_FUZZY_DISTANCE`.
    pub fn search(
        &self,
        project: &str,
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>> {
        let limit = params.clamped_limit();
        let mut results = match params.mode {
            SearchMode::Exact => self.search_exact(project, &params.query, limit)?,
            SearchMode::Regex => self.search_regex(project, &params.query)?,
            SearchMode::Fuzzy => self.search_fuzzy(project, &params.query, MAX_FUZZY_DISTANCE)?,
            SearchMode::GraphEnhanced => self.search_graph_enhanced(project, params)?,
            SearchMode::MultiSignal => {
                let mut hits = self.search_graph_enhanced(project, params)?;
                for hit in &mut hits {
                    hit.score = self.score_multi_signal(hit, params);
                    hit.match_reason = "multi-signal".to_string();
                }
                hits
            }
        };
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        if limit < results.len() {
            results.truncate(limit);
        }
        Ok(results)
    }

    /// Exact case-insensitive substring search (delegates to storage-level CONTAINS).
    fn search_exact(
        &self,
        project: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        if query.trim().is_empty() {
            return Err(QueryError::InvalidQuery("query must not be empty".to_string()));
        }
        let escaped = escape_cypher_string(query);
        let project_esc = escape_cypher_string(project);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') \
                 AND n.project = '{project_esc}' \
                 RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, \
                 n.startLine AS line;"
            );
            match self.storage.query(&cypher) {
                Ok(rows) => results.extend(rows_to_search_results(rows, label, query)),
                Err(_) => continue,
            }
        }
        if limit < results.len() {
            results.truncate(limit);
        }
        Ok(results)
    }

    /// Regex search over Function/Method/Class name and qualifiedName.
    ///
    /// Compiles `pattern` as a Rust `regex::Regex` and matches it against
    /// every `Function`, `Method`, and `Class` node in `project`. Both `name`
    /// and `qualifiedName` are tested (case-sensitive, `is_match` semantics).
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] if `pattern` is not a valid regex.
    fn search_regex(&self, project: &str, pattern: &str) -> Result<Vec<SearchResult>> {
        let re = regex::Regex::new(pattern)
            .map_err(|e| QueryError::InvalidQuery(format!("invalid regex: {e}")))?;
        let project_esc = escape_cypher_string(project);
        let labels = [
            NodeLabel::Function,
            NodeLabel::Method,
            NodeLabel::Class,
        ];
        let mut results = Vec::new();
        for &label in &labels {
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{project_esc}' \
                 RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, \
                 n.startLine AS line;"
            );
            let rows = match self.storage.query(&cypher) {
                Ok(rows) => rows,
                Err(_) => continue,
            };
            for row in rows {
                let Some(name) = row.first().and_then(|v| v.as_str()) else {
                    continue;
                };
                let qn = row.get(1).and_then(|v| v.as_str()).map(String::from);
                let file_path = row.get(2).and_then(|v| v.as_str()).map(String::from);
                let start_line = row
                    .get(3)
                    .and_then(|v| v.as_i64())
                    .and_then(|i| u32::try_from(i).ok());
                let matched_on = if re.is_match(name) {
                    "name"
                } else if qn.as_deref().is_some_and(|q| re.is_match(q)) {
                    "qualifiedName"
                } else {
                    continue;
                };
                results.push(SearchResult {
                    name: name.to_string(),
                    label: label.to_string(),
                    file_path,
                    start_line,
                    qualified_name: qn,
                    score: 1.0,
                    match_reason: format!("regex match on {matched_on}"),
                });
            }
        }
        Ok(results)
    }

    /// Fuzzy search using Levenshtein distance.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] if `query` is empty or
    /// `max_distance > MAX_FUZZY_DISTANCE`.
    fn search_fuzzy(
        &self,
        project: &str,
        query: &str,
        max_distance: usize,
    ) -> Result<Vec<SearchResult>> {
        // T021 placeholder — implementation in next task.
        let _ = (project, query, max_distance);
        Ok(Vec::new())
    }

    /// Graph-enhanced search: name match + degree filter + label filter.
    fn search_graph_enhanced(
        &self,
        project: &str,
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>> {
        // T022 placeholder — implementation in next task.
        let _ = (project, params);
        Ok(Vec::new())
    }

    /// Multi-signal score in `[0.0, 1.0]`:
    /// - `name_relevance * 0.4`
    /// - `degree_centrality * 0.3`
    /// - `module_proximity * 0.2`
    /// - `test_coverage * 0.1`
    fn score_multi_signal(&self, candidate: &SearchResult, params: &SearchParams) -> f64 {
        // T023 placeholder — implementation in next task.
        let _ = (candidate, params);
        0.0
    }
}

/// Symbol-bearing node labels searched by [`StructuredSearcher::search`] and
/// [`StructuredSearcher::search_by_name`]. Project/Folder/File/Parameter are
/// excluded: Project has no `project` column, Folder/File are structural, and
/// Parameter is too granular for general symbol search.
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

/// Executes structured (name/type/file) searches against a [`StorageConnection`].
pub struct StructuredSearcher<'a> {
    conn: &'a StorageConnection,
}

impl<'a> StructuredSearcher<'a> {
    /// Creates a new [`StructuredSearcher`] borrowing `conn`.
    #[must_use]
    pub fn new(conn: &'a StorageConnection) -> Self {
        Self { conn }
    }

    /// Searches all symbol tables for nodes whose `name` contains `name`.
    ///
    /// Results are merged across tables and sorted by descending relevance
    /// score (exact > prefix > substring), then by name. The `project` filter
    /// is applied when supplied. Matching is case-insensitive.
    pub fn search_by_name(
        &self,
        name: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        if name.trim().is_empty() {
            return Err(QueryError::InvalidQuery(
                "search name must not be empty".to_string(),
            ));
        }
        let escaped = escape_cypher_string(name);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            // Use toLower() for case-insensitive CONTAINS matching.
            let cypher = match project {
                Some(p) => format!(
                    "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') AND n.project = '{}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                    escape_cypher_string(p),
                ),
                None => format!(
                    "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                ),
            };
            // Some tables may lack a `qualifiedName` or `filePath` column, or
            // toLower() may be unsupported; skip those gracefully.
            match self.conn.query(&cypher) {
                Ok(rows) => results.extend(rows_to_search_results(rows, label, name)),
                Err(_) => continue,
            }
        }
        sort_and_truncate(&mut results, limit);
        Ok(results)
    }

    /// Returns all nodes of the given `label`, optionally filtered by project.
    pub fn search_by_type(
        &self,
        label: NodeLabel,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let table = escape_identifier(label.table_name());
        let cypher = match project {
            Some(p) => format!(
                "MATCH (n:{table}) WHERE n.project = '{}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                escape_cypher_string(p),
            ),
            None => format!(
                "MATCH (n:{table}) RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
            ),
        };
        let rows = self.conn.query(&cypher)?;
        let mut results = rows_to_search_results(rows, label, "");
        sort_and_truncate(&mut results, limit);
        Ok(results)
    }

    /// Returns all symbols located in the given `file_path`, optionally
    /// filtered by project.
    pub fn search_by_file(
        &self,
        file_path: &str,
        project: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        if file_path.trim().is_empty() {
            return Err(QueryError::InvalidQuery(
                "file path must not be empty".to_string(),
            ));
        }
        let escaped = escape_cypher_string(file_path);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            let cypher = match project {
                Some(p) => format!(
                    "MATCH (n:{table}) WHERE n.filePath = '{escaped}' AND n.project = '{}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                    escape_cypher_string(p),
                ),
                None => format!(
                    "MATCH (n:{table}) WHERE n.filePath = '{escaped}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                ),
            };
            match self.conn.query(&cypher) {
                Ok(rows) => results.extend(rows_to_search_results(rows, label, "")),
                Err(_) => continue,
            }
        }
        // Sort by start line for deterministic file-order output.
        results.sort_by(|a, b| {
            a.start_line
                .unwrap_or(0)
                .cmp(&b.start_line.unwrap_or(0))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(results)
    }

    /// General search: searches by name (CONTAINS) and returns results sorted
    /// by relevance. Equivalent to [`StructuredSearcher::search_by_name`] but
    /// provided as the default `search` entry point for the facade.
    pub fn search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        self.search_by_name(text, project, limit)
    }
}

/// Converts query rows into [`SearchResult`]s, computing a relevance score
/// based on how closely `name` matches each result's name.
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
            })
        })
        .collect()
}

/// Computes a relevance score in `[0.0, 1.0]` for `name` against `query`.
///
/// - Exact match → 1.0
/// - Prefix match → 0.8
/// - Substring match → 0.5
/// - No query (e.g. `search_by_type`) → 1.0 (neutral)
fn relevance_score(name: &str, query: &str) -> f64 {
    relevance_score_with_reason(name, query).0
}

/// Computes both the score and a human-readable match reason.
fn relevance_score_with_reason(name: &str, query: &str) -> (f64, &'static str) {
    if query.is_empty() {
        return (1.0, "neutral");
    }
    let name_lower = name.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if name_lower == query_lower {
        (1.0, "exact name match")
    } else if name_lower.starts_with(&query_lower) {
        (0.8, "prefix match")
    } else {
        (0.5, "substring match")
    }
}

/// Sorts results by descending score then ascending name, and truncates to `limit`.
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

    fn sample_class(id: &str, project: &str, name: &str, qn: &str, file: &str, line: u32) -> Node {
        Node::builder(NodeLabel::Class, name, qn)
            .id(id)
            .project(project)
            .file_path(file)
            .start_line(line)
            .end_line(line + 20)
            .language(Language::Rust)
            .build()
    }

    // --- search_by_name ---

    #[test]
    fn search_by_name_finds_substring_matches() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse_file", "demo.parse_file", "/a.rs", 1),
                sample_function("f2", "demo", "read_input", "demo.read_input", "/a.rs", 10),
                sample_function("f3", "demo", "parse_line", "demo.parse_line", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_name("parse", None, 100)
            .expect("search_by_name");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse_file"));
        assert!(names.contains(&"parse_line"));
        assert!(!names.contains(&"read_input"));
    }

    #[test]
    fn search_by_name_ac_search_001_returns_parse_symbols() {
        // AC-SEARCH-001: search "parse" returns symbols containing "parse".
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse", "demo.parse", "/a.rs", 1),
                sample_function("f2", "demo", "parse_token", "demo.parse_token", "/a.rs", 5),
                sample_function("f3", "demo", "tokenize", "demo.tokenize", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        assert!(results.iter().all(|r| r.name.contains("parse")));
        assert!(results.iter().any(|r| r.name == "parse"));
        assert!(results.iter().any(|r| r.name == "parse_token"));
    }

    #[test]
    fn search_by_name_filters_by_project() {
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

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_name("parse", Some("alpha"), 100)
            .expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_name.as_deref(), Some("alpha.parse"));
    }

    #[test]
    fn search_by_name_respects_limit() {
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

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 3).expect("search");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_by_name_rejects_empty_query() {
        let repo = fresh_repo();
        let searcher = StructuredSearcher::new(repo.connection());
        let err = searcher
            .search_by_name("", None, 10)
            .expect_err("empty name should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_by_name_returns_empty_when_no_match() {
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
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_name("nonexistent", None, 10)
            .expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_by_name_searches_across_multiple_labels() {
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
            &[sample_class(
                "c1",
                "demo",
                "Parser",
                "demo.Parser",
                "/a.rs",
                20,
            )],
            NodeLabel::Class,
        )
        .expect("save_nodes class");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        // Case-insensitive substring: "Parser" contains "parse" lowercased.
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse"));
        assert!(names.contains(&"Parser"));
    }

    #[test]
    fn search_by_name_assigns_higher_score_to_exact_match() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse", "demo.parse", "/a.rs", 1),
                sample_function("f2", "demo", "parse_file", "demo.parse_file", "/a.rs", 5),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        // Exact match should be first (highest score).
        assert_eq!(results[0].name, "parse");
        assert!(results[0].score > results[1].score);
    }

    // --- search_by_type ---

    #[test]
    fn search_by_type_returns_all_functions() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "alpha", "demo.alpha", "/a.rs", 1),
                sample_function("f2", "demo", "beta", "demo.beta", "/a.rs", 5),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        repo.save_nodes(
            &[sample_class(
                "c1",
                "demo",
                "Gamma",
                "demo.Gamma",
                "/a.rs",
                10,
            )],
            NodeLabel::Class,
        )
        .expect("save_nodes class");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, None, 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.label == "Function"));
    }

    #[test]
    fn search_by_type_returns_all_classes() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_class("c1", "demo", "Alpha", "demo.Alpha", "/a.rs", 1),
                sample_class("c2", "demo", "Beta", "demo.Beta", "/a.rs", 10),
            ],
            NodeLabel::Class,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Class, None, 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.label == "Class"));
    }

    #[test]
    fn search_by_type_filters_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "alpha",
                "main",
                "alpha.main",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes alpha");
        repo.save_nodes(
            &[
                sample_function("f2", "beta", "main", "beta.main", "/a.rs", 1),
                sample_function("f3", "beta", "util", "beta.util", "/a.rs", 5),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes beta");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, Some("beta"), 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|r| r.qualified_name.as_ref().unwrap().starts_with("beta.")));
    }

    #[test]
    fn search_by_type_respects_limit() {
        let repo = fresh_repo();
        let mut nodes = Vec::new();
        for i in 0..10 {
            nodes.push(sample_function(
                &format!("f{i}"),
                "demo",
                &format!("func_{i}"),
                &format!("demo.func_{i}"),
                "/a.rs",
                i + 1,
            ));
        }
        repo.save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, None, 3)
            .expect("search_by_type");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_by_type_returns_empty_when_none() {
        let repo = fresh_repo();
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, None, 100)
            .expect("search_by_type");
        assert!(results.is_empty());
    }

    // --- search_by_file ---

    #[test]
    fn search_by_file_returns_all_symbols_in_file() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "alpha", "demo.alpha", "/src/main.rs", 1),
                sample_function("f2", "demo", "beta", "demo.beta", "/src/main.rs", 10),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        repo.save_nodes(
            &[sample_class(
                "c1",
                "demo",
                "Gamma",
                "demo.Gamma",
                "/src/main.rs",
                20,
            )],
            NodeLabel::Class,
        )
        .expect("save_nodes class");
        // A symbol in a different file should not appear.
        repo.save_nodes(
            &[sample_function(
                "f3",
                "demo",
                "delta",
                "demo.delta",
                "/src/lib.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes other file");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/src/main.rs", None)
            .expect("search_by_file");
        assert_eq!(results.len(), 3);
        assert!(results
            .iter()
            .all(|r| r.file_path.as_deref() == Some("/src/main.rs")));
    }

    #[test]
    fn search_by_file_filters_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "alpha",
                "main",
                "alpha.main",
                "/src/main.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes alpha");
        repo.save_nodes(
            &[sample_function(
                "f2",
                "beta",
                "main",
                "beta.main",
                "/src/main.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes beta");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/src/main.rs", Some("alpha"))
            .expect("search_by_file");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_name.as_deref(), Some("alpha.main"));
    }

    #[test]
    fn search_by_file_rejects_empty_path() {
        let repo = fresh_repo();
        let searcher = StructuredSearcher::new(repo.connection());
        let err = searcher
            .search_by_file("", None)
            .expect_err("empty path should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_by_file_returns_empty_when_no_match() {
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
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/nonexistent.rs", None)
            .expect("search_by_file");
        assert!(results.is_empty());
    }

    // --- search (general) ---

    #[test]
    fn search_delegates_to_search_by_name() {
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

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "parse_file");
    }

    // --- helpers ---

    #[test]
    fn relevance_score_exact_match_is_highest() {
        assert_eq!(relevance_score("parse", "parse"), 1.0);
        assert_eq!(relevance_score("PARSE", "parse"), 1.0);
    }

    #[test]
    fn relevance_score_prefix_match_is_high() {
        assert_eq!(relevance_score("parse_file", "parse"), 0.8);
    }

    #[test]
    fn relevance_score_substring_match_is_low() {
        assert_eq!(relevance_score("my_parse_func", "parse"), 0.5);
    }

    #[test]
    fn relevance_score_empty_query_is_neutral() {
        assert_eq!(relevance_score("anything", ""), 1.0);
    }

    // --- error continuation coverage ---

    #[test]
    fn search_by_name_continues_on_query_error() {
        // Cover the `Err(_) => continue` arm (line 86) of search_by_name:
        // when a per-table MATCH query fails (table dropped), the loop
        // skips it and continues to the next table.
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
        // Drop the Class table so its MATCH query errors.
        repo.connection()
            .execute("DROP TABLE Class;")
            .expect("drop table");
        // Verify the table is actually gone.
        let check = repo
            .connection()
            .query("MATCH (n:Class) RETURN n.name AS name;");
        assert!(check.is_err(), "Class table should be gone after DROP");
        let searcher = StructuredSearcher::new(repo.connection());
        // Should still return results from the Function table.
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        assert!(results.iter().any(|r| r.name == "parse"));
    }

    #[test]
    fn search_by_file_continues_on_query_error() {
        // Cover the `Err(_) => continue` arm (line 143) of search_by_file.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse",
                "demo.parse",
                "/src/main.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        repo.connection()
            .execute("DROP TABLE Class;")
            .expect("drop table");
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/src/main.rs", None)
            .expect("search_by_file");
        assert!(results.iter().any(|r| r.name == "parse"));
    }

    // --- T019: multi-mode search types ---

    use crate::kit::StorageModule;
    use crate::storage::StorageConfig;

    fn build_storage() -> std::sync::Arc<dyn Storage> {
        StorageModule::build_cap(&StorageConfig::in_memory()).expect("StorageModule::build_cap")
    }

    #[test]
    fn search_mode_serializes_to_descriptive_strings() {
        let json = serde_json::to_string(&SearchMode::Exact).expect("serialize Exact");
        assert_eq!(json, "\"Exact\"");
        let json = serde_json::to_string(&SearchMode::Regex).expect("serialize Regex");
        assert_eq!(json, "\"Regex\"");
        let json = serde_json::to_string(&SearchMode::Fuzzy).expect("serialize Fuzzy");
        assert_eq!(json, "\"Fuzzy\"");
        let json =
            serde_json::to_string(&SearchMode::GraphEnhanced).expect("serialize GraphEnhanced");
        assert_eq!(json, "\"GraphEnhanced\"");
        let json =
            serde_json::to_string(&SearchMode::MultiSignal).expect("serialize MultiSignal");
        assert_eq!(json, "\"MultiSignal\"");
    }

    #[test]
    fn search_mode_round_trips_through_json() {
        for mode in [
            SearchMode::Exact,
            SearchMode::Regex,
            SearchMode::Fuzzy,
            SearchMode::GraphEnhanced,
            SearchMode::MultiSignal,
        ] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let back: SearchMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn search_params_default_has_limit_50_and_exact_mode() {
        let p = SearchParams::default();
        assert_eq!(p.limit, 50);
        assert_eq!(p.mode, SearchMode::Exact);
        assert!(p.label_filter.is_none());
        assert!(p.degree_filter.is_none());
        assert!(p.file_pattern.is_none());
    }

    #[test]
    fn search_params_clamps_limit_to_max_500() {
        let mut p = SearchParams::default();
        p.limit = 1000;
        assert_eq!(p.clamped_limit(), MAX_LIMIT);
        p.limit = 10;
        assert_eq!(p.clamped_limit(), 10);
    }

    #[test]
    fn search_params_round_trips_through_json() {
        let p = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            label_filter: Some(vec!["Function".to_string()]),
            degree_filter: Some((5, 100)),
            file_pattern: Some("/src/**/*.rs".to_string()),
            limit: 25,
        };
        let json = serde_json::to_string(&p).expect("serialize");
        let back: SearchParams = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn search_engine_dispatches_to_exact_mode() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "parse_file", "demo.parse_file")
            .id("f1")
            .project("demo")
            .file_path("/a.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "parse".to_string(),
            mode: SearchMode::Exact,
            limit: 50,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("exact search");
        assert!(results.iter().any(|r| r.name == "parse_file"));
        assert!(results.iter().all(|r| r.match_reason.contains("match")));
    }

    #[test]
    fn search_engine_exact_rejects_empty_query() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: String::new(),
            mode: SearchMode::Exact,
            ..SearchParams::default()
        };
        let err = engine.search("demo", &params).expect_err("empty query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_engine_regex_matches_pattern_on_name() {
        let storage = build_storage();
        let funcs = [
            Node::builder(NodeLabel::Function, "get_user_by_id", "demo.get_user_by_id")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "get_first_user", "demo.get_first_user")
                .id("f2")
                .project("demo")
                .file_path("/a.rs")
                .start_line(20)
                .end_line(30)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "delete_user", "demo.delete_user")
                .id("f3")
                .project("demo")
                .file_path("/a.rs")
                .start_line(40)
                .end_line(50)
                .language(Language::Rust)
                .build(),
        ];
        storage.save_nodes(&funcs, NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        // Pattern `get_.*_user` matches "get_first_user" but not "get_user_by_id"
        // (no "_user" substring after "get_"). Use `get_.*` to match both.
        let params = SearchParams {
            query: r"get_.*_user".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"get_first_user"), "should match get_first_user, got {names:?}");
        assert!(!names.contains(&"delete_user"));
        assert!(results.iter().all(|r| r.match_reason.starts_with("regex match")));

        // Broader pattern matches both get_* functions.
        let params2 = SearchParams {
            query: r"get_.*".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results2 = engine.search("demo", &params2).expect("regex search");
        let names2: Vec<&str> = results2.iter().map(|r| r.name.as_str()).collect();
        assert!(names2.contains(&"get_user_by_id"));
        assert!(names2.contains(&"get_first_user"));
        assert!(!names2.contains(&"delete_user"));
    }

    #[test]
    fn search_engine_regex_matches_suffix_pattern() {
        let storage = build_storage();
        let classes = [
            Node::builder(NodeLabel::Class, "RequestHandler", "demo.RequestHandler")
                .id("c1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(50)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Class, "BaseController", "demo.BaseController")
                .id("c2")
                .project("demo")
                .file_path("/a.rs")
                .start_line(60)
                .end_line(100)
                .language(Language::Rust)
                .build(),
        ];
        storage.save_nodes(&classes, NodeLabel::Class).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"Handler$".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "RequestHandler");
    }

    #[test]
    fn search_engine_regex_matches_on_qualified_name() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "do_work", "demo.handler.process")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"^demo\.handler".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "do_work");
        assert!(results[0].match_reason.contains("qualifiedName"));
    }

    #[test]
    fn search_engine_regex_rejects_invalid_pattern() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"[invalid".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let err = engine.search("demo", &params).expect_err("invalid regex should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_engine_regex_returns_empty_when_no_match() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "main", "demo.main")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"nonexistent_\d+".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_engine_fuzzy_mode_returns_empty_placeholder() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "getuser".to_string(),
            mode: SearchMode::Fuzzy,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("fuzzy dispatch");
        assert!(results.is_empty());
    }

    #[test]
    fn search_engine_graph_enhanced_returns_empty_placeholder() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("graph dispatch");
        assert!(results.is_empty());
    }

    #[test]
    fn search_engine_multi_signal_returns_empty_placeholder() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::MultiSignal,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("multi-signal dispatch");
        assert!(results.is_empty());
    }
}
