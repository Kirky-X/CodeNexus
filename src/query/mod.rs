// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Query engine (Facade pattern).
//!
//! Provides Cypher execution, structured search, and BM25 full-text search
//! over the LadybugDB graph, unified behind [`QueryFacade`] (PRD §4.4).
//!
//! # Modules
//!
//! - [`error`]: [`QueryError`] and [`Result`](error::Result) alias.
//! - [`cypher`]: [`CypherExecutor`] for raw Cypher queries.
//! - [`structured`]: [`StructuredSearcher`] for name/type/file search.
//! - [`fulltext`]: [`FullTextSearcher`] for BM25 full-text search.
//! - [`facade`]: [`QueryFacade`] (Facade pattern) unifying the above.

pub mod capability;
pub mod cypher;
pub mod cypher_subset;
pub mod error;
pub mod facade;
pub mod fulltext;
pub mod module;
pub mod structured;
pub mod tokenizer;

pub use cypher::CypherExecutor;
pub use cypher_subset::validate_cypher_subset;
pub use error::{QueryError, Result};
pub use facade::QueryFacade;
pub use fulltext::FullTextSearcher;
pub use module::{QueryConfig, QueryModule};
pub use structured::StructuredSearcher;
pub use structured::{
    SearchEngine, SearchMode, SearchParams, DEFAULT_LIMIT, MAX_FUZZY_DISTANCE, MAX_LIMIT,
};
pub use tokenizer::{codenexus_tokenize, codenexus_tokenize_join};

/// A single search hit returned by the structured and full-text searchers.
///
/// Mirrors PRD §4.4.2: each result carries the symbol's display name, node
/// label, source location, qualified name, and a relevance score in `[0.0, 1.0]`.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// Short display name of the matched symbol.
    pub name: String,
    /// Node label (e.g. `"Function"`, `"Class"`).
    pub label: String,
    /// Source file path, when available.
    pub file_path: Option<String>,
    /// 1-based start line, when available.
    pub start_line: Option<u32>,
    /// Fully qualified name, when available.
    pub qualified_name: Option<String>,
    /// Relevance score in `[0.0, 1.0]` (higher is more relevant).
    pub score: f64,
    /// Human-readable reason explaining why this result matched (e.g.
    /// `"exact name match"`, `"regex match"`, `"fuzzy d=1"`).
    pub match_reason: String,
    /// In-degree (incoming CALLS count) for graph-enhanced scoring; 0 for
    /// non-graph search modes.
    pub degree: u32,
}

/// The outcome of a Cypher query execution.
///
/// Mirrors PRD §4.4.2: column names, row values (as JSON), and wall-clock
/// duration in milliseconds.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QueryResult {
    /// Column names returned by the query.
    pub columns: Vec<String>,
    /// Row values, one inner `Vec` per row, each element a JSON value.
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Wall-clock execution duration in milliseconds.
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_is_constructible() {
        let r = SearchResult {
            name: "parse".to_string(),
            label: "Function".to_string(),
            file_path: Some("/a.rs".to_string()),
            start_line: Some(1),
            qualified_name: Some("demo.parse".to_string()),
            score: 1.0,
            match_reason: "exact".to_string(),
            degree: 0,
        };
        assert_eq!(r.name, "parse");
        assert_eq!(r.label, "Function");
        assert_eq!(r.file_path.as_deref(), Some("/a.rs"));
        assert_eq!(r.start_line, Some(1));
        assert_eq!(r.qualified_name.as_deref(), Some("demo.parse"));
        assert_eq!(r.score, 1.0);
        assert_eq!(r.match_reason, "exact");
    }

    #[test]
    fn search_result_clone_is_equal() {
        let r = SearchResult {
            name: "x".to_string(),
            label: "Function".to_string(),
            file_path: None,
            start_line: None,
            qualified_name: None,
            score: 0.5,
            match_reason: String::new(),
            degree: 0,
        };
        assert_eq!(r, r.clone());
    }

    #[test]
    fn search_result_debug_contains_name() {
        let r = SearchResult {
            name: "parse".to_string(),
            label: "Function".to_string(),
            file_path: None,
            start_line: None,
            qualified_name: None,
            score: 1.0,
            match_reason: "regex".to_string(),
            degree: 0,
        };
        let s = format!("{r:?}");
        assert!(s.contains("parse"));
        assert!(s.contains("Function"));
    }

    #[test]
    fn query_result_is_constructible() {
        let r = QueryResult {
            columns: vec!["name".to_string()],
            rows: vec![vec![serde_json::json!("alpha")]],
            duration_ms: 5,
        };
        assert_eq!(r.columns, vec!["name"]);
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0][0], serde_json::json!("alpha"));
        assert_eq!(r.duration_ms, 5);
    }

    #[test]
    fn query_result_clone_is_equal() {
        let r = QueryResult {
            columns: vec!["a".to_string()],
            rows: vec![],
            duration_ms: 0,
        };
        assert_eq!(r, r.clone());
    }

    #[test]
    fn query_result_debug_contains_columns() {
        let r = QueryResult {
            columns: vec!["name".to_string()],
            rows: vec![],
            duration_ms: 0,
        };
        let s = format!("{r:?}");
        assert!(s.contains("name"));
        assert!(s.contains("QueryResult"));
    }

    #[test]
    fn query_result_with_empty_rows() {
        let r = QueryResult {
            columns: vec!["name".to_string(), "id".to_string()],
            rows: vec![],
            duration_ms: 1,
        };
        assert!(r.rows.is_empty());
        assert_eq!(r.columns.len(), 2);
    }
}
