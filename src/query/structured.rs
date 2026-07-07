// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Structured search by name / type / file (PRD §4.4.2).
//!
//! [`StructuredSearcher`] runs Cypher `MATCH` queries against specific node
//! tables, returning [`SearchResult`] lists. Because LadybugDB stores each
//! [`NodeLabel`] in a distinct table, "search all symbols by name" is
//! implemented as a fan-out across the relevant tables followed by a merge.

use super::error::{QueryError, Result};
use super::SearchResult;
use crate::model::NodeLabel;
use crate::storage::schema::{escape_cypher_string, escape_identifier};
use crate::storage::StorageConnection;

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
///
/// - Exact match → 1.0
/// - Prefix match → 0.8
/// - Substring match → 0.5
/// - No query (e.g. `search_by_type`) → 1.0 (neutral)
fn relevance_score(name: &str, query: &str) -> f32 {
    if query.is_empty() {
        return 1.0;
    }
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
            &[sample_function("f1", "demo", "parse", "demo.parse", "/a.rs", 1)],
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
        let results = searcher
            .search_by_name("parse", None, 100)
            .expect("search");
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
}
