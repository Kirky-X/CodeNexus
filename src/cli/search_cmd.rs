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
use super::disambiguation;
use super::error::Result;
use crate::kit::{Kit, QueryKey};
#[cfg(feature = "embed")]
use crate::kit::EmbedKey;
use crate::model::NodeLabel;
use crate::query::capability::QueryEngine;
use crate::query::SearchResult;

/// Runs the `search` subcommand.
///
/// Resolves the [`QueryEngine`](crate::query::capability::QueryEngine)
/// capability from `kit`, runs the search, and prints the results as a JSON
/// array of [`SearchResultOutput`] objects.
///
/// # Narrowing flags (H14)
///
/// `--uid`/`--file`/`--kind` filter the results. When `--uid` is supplied,
/// the command looks up the node by id directly (bypassing text search) and
/// returns it as a single result, further filtered by `--file`/`--kind` if
/// also supplied. Without `--uid`, the normal text search runs and results
/// are filtered by `--file`/`--kind`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Query capability is
/// not registered. Returns [`crate::cli::error::CliError::Query`] for search
/// failures. Returns [`crate::cli::error::CliError::InvalidInput`] for an
/// invalid `--kind` value.
pub fn run(kit: &Kit, args: &SearchArgs) -> Result<()> {
    let kind_filter: Option<NodeLabel> = args
        .kind
        .as_deref()
        .map(disambiguation::parse_kind)
        .transpose()?;

    let results = if let Some(uid) = &args.uid {
        // --uid mode: direct node lookup, bypassing text search.
        let candidate = disambiguation::find_by_uid(kit, uid)?;
        match candidate {
            Some(c) => {
                let label_match = kind_filter
                    .map(|k| c.label == k.to_string())
                    .unwrap_or(true);
                let file_match = args
                    .file
                    .as_ref()
                    .map(|f| c.file_path.as_deref() == Some(f.as_str()))
                    .unwrap_or(true);
                if label_match && file_match {
                    vec![SearchResult {
                        name: c.name,
                        label: c.label,
                        file_path: c.file_path,
                        start_line: c.start_line,
                        qualified_name: if c.qualified_name.is_empty() {
                            None
                        } else {
                            Some(c.qualified_name)
                        },
                        score: 1.0,
                    }]
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        }
    } else {
        // Normal text search, then filter by --kind/--file.
        let query = kit.require::<QueryKey>()?;
        let raw = if args.semantic {
            semantic_search(kit, &*query, &args.text, args.limit)?
        } else {
            query.search(&args.text, None, args.limit)?
        };
        filter_results(raw, kind_filter, &args.file)
    };

    let output: Vec<SearchResultOutput> =
        results.into_iter().map(SearchResultOutput::from).collect();
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Filters search results by `--kind` and `--file`.
fn filter_results(
    results: Vec<SearchResult>,
    kind: Option<NodeLabel>,
    file: &Option<String>,
) -> Vec<SearchResult> {
    results
        .into_iter()
        .filter(|r| {
            if let Some(k) = kind {
                if r.label != k.to_string() {
                    return false;
                }
            }
            if let Some(ref f) = file {
                if r.file_path.as_deref() != Some(f.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect()
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
            uid: None,
            file: None,
            kind: None,
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

    // --- H14: narrowing flags ---

    #[test]
    fn run_search_uid_looks_up_node_directly() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = SearchArgs {
            text: String::new(), // --uid bypasses text search
            semantic: false,
            limit: 10,
            db: db.to_str().unwrap().to_string(),
            uid: Some("f1".to_string()),
            file: None,
            kind: None,
        };
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "uid search should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_search_kind_filter_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = SearchArgs {
            text: "parse".to_string(),
            semantic: false,
            limit: 10,
            db: db.to_str().unwrap().to_string(),
            uid: None,
            file: None,
            kind: Some("Function".to_string()),
        };
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "kind-filtered search should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_search_invalid_kind_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = SearchArgs {
            text: "parse".to_string(),
            semantic: false,
            limit: 10,
            db: db.to_str().unwrap().to_string(),
            uid: None,
            file: None,
            kind: Some("BogusLabel".to_string()),
        };
        let err = run(&kit, &args).expect_err("invalid kind should error");
        assert_eq!(err.exit_code(), 1, "invalid kind → exit 1");
    }

    #[test]
    fn run_search_file_filter_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_search_fixture(&kit);
        let args = SearchArgs {
            text: "parse".to_string(),
            semantic: false,
            limit: 10,
            db: db.to_str().unwrap().to_string(),
            uid: None,
            file: Some("/src/main.rs".to_string()),
            kind: None,
        };
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "file-filtered search should succeed: {:?}",
            result.err()
        );
    }

    // --- filter_results direct unit tests ---

    fn make_result(name: &str, label: &str, file: Option<&str>) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            label: label.to_string(),
            file_path: file.map(|s| s.to_string()),
            start_line: Some(1),
            qualified_name: None,
            score: 1.0,
        }
    }

    #[test]
    fn filter_results_no_filter_passes_all_through() {
        let results = vec![
            make_result("a", "Function", Some("/x.rs")),
            make_result("b", "Class", Some("/y.rs")),
        ];
        let out = filter_results(results, None, &None);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn filter_results_kind_filter_keeps_only_matching_label() {
        let results = vec![
            make_result("a", "Function", Some("/x.rs")),
            make_result("b", "Class", Some("/y.rs")),
            make_result("c", "Function", Some("/z.rs")),
        ];
        let out = filter_results(results, Some(NodeLabel::Function), &None);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|r| r.label == "Function"));
    }

    #[test]
    fn filter_results_file_filter_keeps_only_matching_file() {
        let results = vec![
            make_result("a", "Function", Some("/x.rs")),
            make_result("b", "Class", Some("/y.rs")),
        ];
        let out = filter_results(results, None, &Some("/x.rs".to_string()));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "a");
    }

    #[test]
    fn filter_results_both_filters_require_both_matches() {
        let results = vec![
            make_result("a", "Function", Some("/x.rs")),
            make_result("b", "Function", Some("/y.rs")),
            make_result("c", "Class", Some("/x.rs")),
        ];
        let out = filter_results(results, Some(NodeLabel::Function), &Some("/x.rs".to_string()));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "a");
    }

    #[test]
    fn filter_results_empty_input_returns_empty() {
        let out = filter_results(Vec::new(), Some(NodeLabel::Function), &None);
        assert!(out.is_empty());
    }

    #[test]
    fn filter_results_kind_filter_no_matches_returns_empty() {
        let results = vec![make_result("a", "Class", Some("/x.rs"))];
        let out = filter_results(results, Some(NodeLabel::Function), &None);
        assert!(out.is_empty());
    }

    #[test]
    fn filter_results_file_filter_none_file_path_does_not_match() {
        // A result with file_path=None should not match a file filter.
        let results = vec![make_result("a", "Function", None)];
        let out = filter_results(results, None, &Some("/x.rs".to_string()));
        assert!(out.is_empty());
    }
}
