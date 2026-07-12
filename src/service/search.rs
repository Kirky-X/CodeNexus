// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Search command: search for symbols by name or content.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, QueryModule, StorageModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::query::structured::{SearchEngine, SearchMode, SearchParams};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::query::SearchResult;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::service_api;

/// JSON-serializable search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchOutput {
    pub count: usize,
    pub results: Vec<Value>,
}

/// Converts a [`SearchResult`] into a JSON object for API output.
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn search_result_to_json(r: &SearchResult) -> Value {
    json!({
        "name": r.name,
        "label": r.label,
        "filePath": r.file_path,
        "startLine": r.start_line,
        "qualifiedName": r.qualified_name,
        "score": r.score,
        "matchReason": r.match_reason,
    })
}

/// Parses a search mode string into a [`SearchMode`].
///
/// Accepts case-insensitive mode names:
/// - `exact` → `Exact`
/// - `regex` → `Regex`
/// - `fuzzy` → `Fuzzy`
/// - `graph`, `graph_enhanced`, `graph-enhanced` → `GraphEnhanced`
/// - `multi`, `multi_signal`, `multi-signal` → `MultiSignal`
///
/// Returns `None` for empty or unrecognized strings (empty = legacy mode).
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn parse_search_mode(mode: &str) -> Option<SearchMode> {
    let lower = mode.trim().to_ascii_lowercase();
    match lower.as_str() {
        "" => None,
        "exact" => Some(SearchMode::Exact),
        "regex" => Some(SearchMode::Regex),
        "fuzzy" => Some(SearchMode::Fuzzy),
        "graph" | "graph_enhanced" | "graph-enhanced" => Some(SearchMode::GraphEnhanced),
        "multi" | "multi_signal" | "multi-signal" => Some(SearchMode::MultiSignal),
        _ => None,
    }
}

/// Runs search against an injected Kit (testable core).
///
/// When `mode` is non-empty, uses [`SearchEngine`] with the corresponding
/// [`SearchMode`] for multi-mode search (exact/regex/fuzzy/graph/multi).
/// When `mode` is empty, falls back to the legacy `QueryModule` search
/// (structured or BM25 full-text based on `fulltext` flag).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_search(
    kit: &AsyncKit<AsyncReady>,
    text: &str,
    fulltext: bool,
    limit: u32,
    mode: &str,
    project: &str,
) -> Result<SearchOutput, CodeNexusError> {
    if let Some(search_mode) = parse_search_mode(mode) {
        let storage = kit.require::<StorageModule>()?;
        let engine = SearchEngine::new(&*storage);
        let params = SearchParams {
            query: text.to_string(),
            mode: search_mode,
            limit: limit as usize,
            ..Default::default()
        };
        let results = engine.search(project, &params)?;
        let results: Vec<Value> = results.iter().map(search_result_to_json).collect();
        Ok(SearchOutput {
            count: results.len(),
            results,
        })
    } else {
        let q = kit.require::<QueryModule>()?;
        let results = if fulltext {
            q.fulltext_search(text, None, limit as usize)
        } else {
            q.search(text, None, limit as usize)
        }?;
        let results: Vec<Value> = results.iter().map(search_result_to_json).collect();
        Ok(SearchOutput {
            count: results.len(),
            results,
        })
    }
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "search",
    version = "0.3.2",
    description = "Search for symbols by name (structured) or content (BM25 full-text).",
    cli = true
)]
async fn search(
    text: String,
    fulltext: bool,
    limit: u32,
    mode: String,
    project: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result = run_search(&kit, &text, fulltext, limit, &mode, &project)
        .map_err(|e| to_api_error(e, "search_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "search_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "search",
    version = "0.3.2",
    tool_name = "search",
    description = "Search for symbols by name (structured) or content (BM25 full-text)."
)]
async fn search_mcp(
    text: String,
    fulltext: bool,
    limit: u32,
    mode: String,
    project: String,
) -> Result<SearchOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_search(&kit, &text, fulltext, limit, &mode, &project)
        .map_err(|e| to_api_error(e, "search_error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_search_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    #[test]
    fn run_search_returns_empty_on_fresh_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_search(&kit, "foo", false, 10, "", "").expect("run should succeed");
        assert_eq!(output.count, 0);
        assert!(output.results.is_empty());
    }

    #[test]
    fn run_search_finds_symbol_by_name() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_thing", false, 10, "", "").expect("run should succeed");
        let _ = output;
    }

    #[test]
    fn run_search_empty_text_returns_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_search(&kit, "   ", false, 10, "", "").expect_err("empty text should error");
        assert!(matches!(err, CodeNexusError::Query(_)));
    }

    #[test]
    fn run_search_fulltext_empty_text_returns_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_search(&kit, "", true, 10, "", "").expect_err("empty fulltext should error");
        assert!(matches!(err, CodeNexusError::Query(_)));
    }

    #[test]
    fn search_output_serializes_to_json() {
        let output = SearchOutput {
            count: 1,
            results: vec![json!({
                "name": "foo",
                "label": "Function",
                "filePath": "/src/a.rs",
                "startLine": 1,
                "qualifiedName": "demo.foo",
                "score": 0.95,
            })],
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"count\":1"));
        assert!(json.contains("\"name\":\"foo\""));
        assert!(json.contains("\"label\":\"Function\""));
    }

    #[test]
    fn search_result_to_json_maps_all_fields() {
        let r = SearchResult {
            name: "foo".into(),
            label: "Function".into(),
            file_path: Some("/src/a.rs".into()),
            start_line: Some(42),
            qualified_name: Some("demo.foo".into()),
            score: 0.8,
            match_reason: "exact".into(),
            degree: 0,
        };
        let v = search_result_to_json(&r);
        assert_eq!(v["name"], "foo");
        assert_eq!(v["label"], "Function");
        assert_eq!(v["filePath"], "/src/a.rs");
        assert_eq!(v["startLine"], 42);
        assert_eq!(v["qualifiedName"], "demo.foo");
        let score = v["score"].as_f64().expect("score should be a number");
        assert!((score - 0.8).abs() < 1e-6, "score should be ~0.8, got {score}");
    }

    #[test]
    fn search_result_to_json_handles_none_fields() {
        let r = SearchResult {
            name: "bar".into(),
            label: "Class".into(),
            file_path: None,
            start_line: None,
            qualified_name: None,
            score: 0.0,
            match_reason: String::new(),
            degree: 0,
        };
        let v = search_result_to_json(&r);
        assert_eq!(v["name"], "bar");
        assert!(v["filePath"].is_null());
        assert!(v["startLine"].is_null());
        assert!(v["qualifiedName"].is_null());
    }

    // ===== T039: parse_search_mode unit tests =====

    #[test]
    fn parse_search_mode_empty_returns_none() {
        assert_eq!(parse_search_mode(""), None);
        assert_eq!(parse_search_mode("   "), None);
    }

    #[test]
    fn parse_search_mode_exact() {
        assert_eq!(parse_search_mode("exact"), Some(SearchMode::Exact));
        assert_eq!(parse_search_mode("EXACT"), Some(SearchMode::Exact));
    }

    #[test]
    fn parse_search_mode_regex() {
        assert_eq!(parse_search_mode("regex"), Some(SearchMode::Regex));
        assert_eq!(parse_search_mode("Regex"), Some(SearchMode::Regex));
    }

    #[test]
    fn parse_search_mode_fuzzy() {
        assert_eq!(parse_search_mode("fuzzy"), Some(SearchMode::Fuzzy));
        assert_eq!(parse_search_mode("FUZZY"), Some(SearchMode::Fuzzy));
    }

    #[test]
    fn parse_search_mode_graph_aliases() {
        assert_eq!(parse_search_mode("graph"), Some(SearchMode::GraphEnhanced));
        assert_eq!(parse_search_mode("graph_enhanced"), Some(SearchMode::GraphEnhanced));
        assert_eq!(parse_search_mode("graph-enhanced"), Some(SearchMode::GraphEnhanced));
    }

    #[test]
    fn parse_search_mode_multi_aliases() {
        assert_eq!(parse_search_mode("multi"), Some(SearchMode::MultiSignal));
        assert_eq!(parse_search_mode("multi_signal"), Some(SearchMode::MultiSignal));
        assert_eq!(parse_search_mode("multi-signal"), Some(SearchMode::MultiSignal));
    }

    #[test]
    fn parse_search_mode_invalid_returns_none() {
        assert_eq!(parse_search_mode("invalid"), None);
        assert_eq!(parse_search_mode("semantic"), None);
    }

    // ===== T039: run_search with mode parameter =====

    #[test]
    fn run_search_with_exact_mode_finds_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_thing", false, 10, "exact", "demo")
            .expect("exact mode search should succeed");
        assert!(output.count > 0, "should find do_thing in exact mode");
    }

    #[test]
    fn run_search_with_regex_mode_finds_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_.*", false, 10, "regex", "demo")
            .expect("regex mode search should succeed");
        assert!(output.count > 0, "should find do_thing with regex do_.*");
    }

    #[test]
    fn run_search_with_fuzzy_mode_finds_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_thng", false, 10, "fuzzy", "demo")
            .expect("fuzzy mode search should succeed");
        // Fuzzy search should find do_thing even with a typo
        assert!(output.count > 0, "should find do_thing with fuzzy do_thng");
    }

    #[test]
    fn run_search_with_invalid_mode_falls_back_to_legacy() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Invalid mode → None → legacy path (should succeed on empty DB)
        let output = run_search(&kit, "foo", false, 10, "invalid_mode", "")
            .expect("invalid mode should fall back to legacy");
        assert_eq!(output.count, 0);
    }

    #[test]
    fn run_search_with_empty_mode_uses_legacy_path() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_search(&kit, "foo", false, 10, "", "")
            .expect("empty mode should use legacy path");
        assert_eq!(output.count, 0);
    }

    // ===== run_search: graph_enhanced and multi_signal modes =====

    #[test]
    fn run_search_with_graph_enhanced_mode_finds_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_thing", false, 10, "graph", "demo")
            .expect("graph_enhanced mode should succeed");
        assert!(output.count > 0, "should find do_thing in graph mode");
    }

    #[test]
    fn run_search_with_graph_enhanced_alias_finds_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_thing", false, 10, "graph_enhanced", "demo")
            .expect("graph_enhanced alias should succeed");
        assert!(output.count > 0, "should find do_thing with graph_enhanced alias");
    }

    #[test]
    fn run_search_with_multi_signal_mode_finds_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_thing", false, 10, "multi", "demo")
            .expect("multi_signal mode should succeed");
        assert!(output.count > 0, "should find do_thing in multi mode");
    }

    #[test]
    fn run_search_with_multi_signal_alias_finds_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "do_thing", false, 10, "multi_signal", "demo")
            .expect("multi_signal alias should succeed");
        assert!(output.count > 0, "should find do_thing with multi_signal alias");
    }

    #[test]
    fn run_search_with_graph_enhanced_on_empty_db_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_search(&kit, "foo", false, 10, "graph", "demo")
            .expect("graph mode on empty DB should succeed");
        assert_eq!(output.count, 0);
    }

    #[test]
    fn run_search_with_multi_signal_on_empty_db_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_search(&kit, "foo", false, 10, "multi", "demo")
            .expect("multi mode on empty DB should succeed");
        assert_eq!(output.count, 0);
    }

    // ===== run_search: fulltext mode exercises legacy fulltext path =====

    #[test]
    fn run_search_fulltext_on_non_empty_db_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'fetch_data', qualifiedName: 'demo.fetch_data', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        let output = run_search(&kit, "fetch_data", true, 10, "", "")
            .expect("fulltext search should succeed");
        let _ = output;
    }

    #[test]
    fn run_search_with_exact_mode_on_empty_db_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_search(&kit, "foo", false, 10, "exact", "demo")
            .expect("exact mode on empty DB should succeed");
        assert_eq!(output.count, 0);
    }

    #[test]
    fn run_search_with_regex_mode_on_empty_db_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_search(&kit, "foo.*", false, 10, "regex", "demo")
            .expect("regex mode on empty DB should succeed");
        assert_eq!(output.count, 0);
    }

    #[test]
    fn run_search_with_fuzzy_mode_on_empty_db_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_search(&kit, "foo", false, 10, "fuzzy", "demo")
            .expect("fuzzy mode on empty DB should succeed");
        assert_eq!(output.count, 0);
    }
}
