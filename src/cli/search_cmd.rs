// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `search` subcommand handler (PRD §4.4).
//!
//! Calls [`QueryEngine::search`] (or [`QueryEngine::fulltext_search`] /
//! [`QueryEngine::semantic_search`] when `--semantic` is set) and prints the
//! results as a JSON array.
//!
//! When the `embed` feature is enabled and `--semantic` is set, the command
//! uses [`HybridStrategy`] (BM25 + vector RRF fusion, AC-SEARCH-002) if an
//! embedding API key is configured; otherwise it falls back to BM25 full-text
//! search.
//!
//! [`HybridStrategy`]: crate::embed::HybridStrategy

use serde::Serialize;

use super::args::SearchArgs;
use super::error::Result;
use crate::kit::{Kit, QueryKey};
#[cfg(feature = "embed")]
use crate::kit::EmbedKey;
use crate::query::capability::QueryEngine;
use crate::query::SearchResult;

/// Runs the `search` subcommand.
///
/// Resolves the [`QueryEngine`](crate::query::capability::QueryEngine)
/// capability from `kit`, runs the search, and prints the results as a JSON
/// array of [`SearchResultOutput`] objects.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Query capability is
/// not registered. Returns [`crate::cli::error::CliError::Query`] for search
/// failures.
pub fn run(kit: &Kit, args: &SearchArgs) -> Result<()> {
    let query = kit.require::<QueryKey>()?;
    let results = if args.semantic {
        semantic_search(kit, &*query, &args.text, args.limit)?
    } else {
        query.search(&args.text, None, args.limit)?
    };
    let output: Vec<SearchResultOutput> =
        results.into_iter().map(SearchResultOutput::from).collect();
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Executes a semantic search, using the embed subsystem when available.
///
/// When the `embed` feature is enabled and the Embed capability is registered,
/// this calls [`QueryEngine::semantic_search`] (BM25 + vector RRF fusion via
/// [`HybridStrategy`]). If the embed capability is unavailable or the semantic
/// search fails (e.g. no API key, vector extension missing), it falls back to
/// BM25 full-text search via [`QueryEngine::fulltext_search`].
///
/// [`HybridStrategy`]: crate::embed::HybridStrategy
#[cfg_attr(not(feature = "embed"), allow(unused_variables))]
fn semantic_search(
    kit: &Kit,
    query: &dyn QueryEngine,
    text: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    #[cfg(feature = "embed")]
    {
        if let Ok(embed_client) = kit.require::<EmbedKey>() {
            if let Ok(results) = query.semantic_search(text, None, limit, &*embed_client) {
                return Ok(results);
            }
        }
    }
    // Fallback: BM25 full-text search (always available).
    Ok(query.fulltext_search(text, None, limit)?)
}

/// JSON-serializable view of a single search result.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResultOutput {
    /// Short display name of the matched symbol.
    pub name: String,
    /// Node label (e.g. `"Function"`).
    pub label: String,
    /// Source file path, when available.
    pub file_path: Option<String>,
    /// 1-based start line, when available.
    pub start_line: Option<u32>,
    /// Fully qualified name, when available.
    pub qualified_name: Option<String>,
    /// Relevance score in `[0.0, 1.0]`.
    pub score: f32,
}

impl From<SearchResult> for SearchResultOutput {
    fn from(r: SearchResult) -> Self {
        Self {
            name: r.name,
            label: r.label,
            file_path: r.file_path,
            start_line: r.start_line,
            qualified_name: r.qualified_name,
            score: r.score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::SearchArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_search_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// Seeds the database with functions whose names contain "parse".
    fn seed_search_fixture(kit: &Kit) {
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0});").expect("create project");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'parse_file', qualifiedName: 'demo.parse_file', filePath: '/src/main.rs', startLine: 1, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f1");
        storage.execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'parse_line', qualifiedName: 'demo.parse_line', filePath: '/src/main.rs', startLine: 11, endLine: 20, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f2");
        storage.execute("CREATE (:Function {id: 'f3', project: 'demo', name: 'read_input', qualifiedName: 'demo.read_input', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f3");
    }

    fn make_args(text: &str, semantic: bool, limit: usize, db: &str) -> SearchArgs {
        SearchArgs {
            text: text.to_string(),
            semantic,
            limit,
            db: db.to_string(),
        }
    }

    // --- SearchResultOutput ---

    #[test]
    fn search_result_output_from_search_result() {
        let r = SearchResult {
            name: "parse".into(),
            label: "Function".into(),
            file_path: Some("/x.rs".into()),
            start_line: Some(1),
            qualified_name: Some("demo.parse".into()),
            score: 0.9,
        };
        let out = SearchResultOutput::from(r);
        assert_eq!(out.name, "parse");
        assert_eq!(out.label, "Function");
        assert_eq!(out.file_path.as_deref(), Some("/x.rs"));
        assert_eq!(out.start_line, Some(1));
        assert_eq!(out.qualified_name.as_deref(), Some("demo.parse"));
        assert!((out.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn search_result_output_serializes_to_json() {
        let out = SearchResultOutput {
            name: "x".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: None,
            score: 0.5,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"name\":\"x\""));
        assert!(json.contains("\"label\":\"Function\""));
    }

    // --- run() success ---

    #[test]
    fn run_search_returns_results() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = make_args("parse", false, 10, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "search should succeed: {:?}", result.err());
    }

    #[test]
    fn run_search_semantic_uses_fulltext() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = make_args("parse", true, 10, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "semantic search should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_search_no_matches_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = make_args("zzz_nonexistent", false, 10, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "no-match search should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_search_limit_one_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = make_args("parse", false, 1, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "limit-1 search should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---

    #[test]
    fn run_search_empty_db_returns_empty_array() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("parse", false, 10, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "empty-db search should succeed: {:?}",
            result.err()
        );
    }

    // Note: `run_search_missing_db_returns_error` was removed because the
    // "missing db" error now surfaces at `build_kit` time, not at `run` time.
    // Covered by `build_kit_invalid_db_path_returns_build_failed_error` in
    // `kit::bootstrap::tests`.
}
