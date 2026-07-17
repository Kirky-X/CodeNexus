// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Trace command: trace a symbol's call and/or data-flow paths.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, TraceModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::trace::MAX_SUBGRAPH_NODES;
use crate::trace::{
    apply_path_filter, CallGraphTracer, PathFilter, TraceCycle, TraceEdge, TraceNode, TracePath,
    TraceResult, TraceType,
};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// JSON-serializable trace result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceOutput {
    pub symbol: String,
    pub paths: Vec<TracePathOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<TraceCycle>,
}

/// A single trace path — a sequence of nodes and edges at a given depth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TracePathOutput {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    pub depth: usize,
}

#[cfg(any(feature = "cli", feature = "mcp", test))]
fn trace_output(r: TraceResult) -> TraceOutput {
    let paths = r
        .paths
        .into_iter()
        .map(|p| TracePathOutput {
            nodes: p.nodes.iter().map(trace_node_to_json).collect(),
            edges: p.edges.iter().map(trace_edge_to_json).collect(),
            depth: p.depth,
        })
        .collect();
    TraceOutput {
        symbol: r.symbol,
        paths,
        cycles: r.cycles,
    }
}

#[cfg(any(feature = "cli", feature = "mcp", test))]
fn trace_node_to_json(n: &TraceNode) -> Value {
    json!({
        "name": n.name,
        "label": n.label,
        "filePath": n.file_path,
        "startLine": n.start_line,
    })
}

#[cfg(any(feature = "cli", feature = "mcp", test))]
fn trace_edge_to_json(e: &TraceEdge) -> Value {
    json!({
        "edgeType": e.edge_type,
        "reason": e.reason,
        "confidence": e.confidence,
    })
}

/// Builds a [`PathFilter`] from a comma-separated list of glob patterns.
///
/// Each pattern is matched against node `file_path` using glob semantics
/// (`*` = any sequence, `?` = single char). An empty string produces no
/// filter (returns `None`).
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn build_path_filter(path_filter: &str) -> Option<PathFilter> {
    let trimmed = path_filter.trim();
    if trimmed.is_empty() {
        return None;
    }
    let patterns: Vec<String> = trimmed
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if patterns.is_empty() {
        return None;
    }
    Some(PathFilter {
        include_files: Some(patterns),
        ..Default::default()
    })
}

/// Finds the start node id in the loaded graph by matching `name` or
/// `qualified_name`.
///
/// Returns `None` if no node matches. If multiple nodes match, the first
/// one is returned (sufficient for cross-service edge discovery — the
/// trace itself already resolved the symbol unambiguously).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub(crate) fn find_start_node_id(
    graph: &crate::model::Graph,
    symbol: &str,
) -> Option<crate::model::NodeId> {
    graph
        .nodes
        .values()
        .find(|n| n.name == symbol || n.qualified_name == symbol)
        .map(|n| n.id.clone())
}

/// Collects cross-service paths by finding `HttpCalls` edges from the start
/// node in the loaded graph.
///
/// Each HttpCalls edge produces a [`TracePath`] of depth 1 containing the
/// start node and the target node. This exposes outbound HTTP dependencies
/// that the standard `Calls`/`FfiCalls` tracers do not traverse.
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn collect_cross_service_paths(graph: &crate::model::Graph, start_id: &str) -> Vec<TracePath> {
    use crate::model::EdgeType;
    let node_id: String = start_id.to_string();
    let mut paths = Vec::new();
    let Some(start_node) = graph.get_node(&node_id) else {
        return paths;
    };
    let start_trace_node = TraceNode::from(start_node);
    for edge in graph.edges_from(&node_id) {
        if edge.edge_type != EdgeType::HttpCalls {
            continue;
        }
        let Some(target_node) = graph.get_node(&edge.target) else {
            continue;
        };
        paths.push(TracePath {
            nodes: vec![start_trace_node.clone(), TraceNode::from(target_node)],
            edges: vec![TraceEdge {
                edge_type: edge.edge_type.to_string(),
                reason: edge.reason.clone(),
                confidence: edge.confidence,
            }],
            depth: 1,
        });
    }
    paths
}

/// Runs trace against an injected Kit (testable core).
///
/// When `path_filter` is non-empty, trace paths are filtered by glob
/// patterns against node `file_path`. When `detect_cycles` is true, the
/// loaded subgraph is scanned for call-graph cycles via DFS coloring.
/// When `cross_service` is true, `HttpCalls` edges from the start node
/// are appended as additional paths.
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_trace(
    kit: &AsyncKit<AsyncReady>,
    symbol: &str,
    trace_type: &str,
    depth: u32,
    path_filter: &str,
    detect_cycles: bool,
    cross_service: bool,
) -> Result<TraceOutput, CodeNexusError> {
    let tt = TraceType::from_cli_str(trace_type).ok_or_else(|| {
        CodeNexusError::InvalidInput(format!(
            "invalid trace_type: {trace_type} (expected calls|dataflow|all)"
        ))
    })?;
    let trace_engine = kit.require::<TraceModule>()?;
    let mut result = trace_engine.trace(symbol, tt, depth as usize)?;

    let needs_graph = detect_cycles || cross_service;
    let graph = if needs_graph {
        let (g, _truncated) =
            trace_engine.load_graph(symbol, depth as usize, MAX_SUBGRAPH_NODES)?;
        Some(g)
    } else {
        None
    };

    if cross_service {
        if let Some(ref graph) = graph {
            if let Some(start_id) = find_start_node_id(graph, symbol) {
                let cross_paths = collect_cross_service_paths(graph, &start_id);
                result.paths.extend(cross_paths);
            }
        }
    }

    if let Some(filter) = build_path_filter(path_filter) {
        result.paths = apply_path_filter(result.paths, &filter);
    }

    if detect_cycles {
        if let Some(ref graph) = graph {
            let tracer = CallGraphTracer::new(graph);
            result.cycles = tracer.detect_cycles();
        }
    }

    Ok(trace_output(result))
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "trace",
    version = "0.3.5",
    description = "Trace a symbol's call and/or data-flow paths.",
    cli = true
)]
async fn trace(
    symbol: String,
    trace_type: String,
    depth: u32,
    path_filter: String,
    detect_cycles: bool,
    cross_service: bool,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result = run_trace(
        &kit,
        &symbol,
        &trace_type,
        depth,
        &path_filter,
        detect_cycles,
        cross_service,
    )
    .map_err(|e| to_api_error(e, "trace_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "trace_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[forge(
    name = "trace",
    version = "0.3.5",
    tool_name = "trace",
    description = "Trace a symbol's call and/or data-flow paths."
)]
async fn trace_mcp(
    symbol: String,
    trace_type: String,
    depth: u32,
    path_filter: String,
    detect_cycles: bool,
    cross_service: bool,
) -> Result<TraceOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_trace(
        &kit,
        &symbol,
        &trace_type,
        depth,
        &path_filter,
        detect_cycles,
        cross_service,
    )
    .map_err(|e| to_api_error(e, "trace_error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_trace_testdb");
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
    fn run_trace_returns_trace_error_for_unknown_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_trace(&kit, "demo.foo", "calls", 3, "", false, false)
            .expect_err("unknown symbol on empty DB should error");
        match err {
            CodeNexusError::Trace(crate::trace::TraceError::SymbolNotFound(s)) => {
                assert_eq!(s, "demo.foo");
            }
            other => panic!("expected Trace(SymbolNotFound), got {other:?}"),
        }
    }

    #[test]
    fn run_trace_returns_invalid_input_for_bad_trace_type() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_trace(&kit, "demo.foo", "bogus", 3, "", false, false)
            .expect_err("invalid trace_type should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("bogus"),
            "error should mention bad value: {msg}"
        );
        assert!(
            msg.contains("calls|dataflow|all"),
            "error should hint valid values: {msg}"
        );
    }

    #[test]
    fn trace_type_from_cli_str_accepts_known_values() {
        assert_eq!(TraceType::from_cli_str("calls"), Some(TraceType::Calls));
        assert_eq!(
            TraceType::from_cli_str("dataflow"),
            Some(TraceType::DataFlow)
        );
        assert_eq!(TraceType::from_cli_str("all"), Some(TraceType::All));
        assert_eq!(TraceType::from_cli_str("CALLS"), Some(TraceType::Calls));
    }

    #[test]
    fn trace_type_from_cli_str_rejects_unknown() {
        assert!(TraceType::from_cli_str("bogus").is_none());
        assert!(TraceType::from_cli_str("").is_none());
    }

    #[test]
    fn trace_output_maps_empty_result() {
        let result = TraceResult {
            symbol: "demo.foo".into(),
            paths: vec![],
            cycles: vec![],
        };
        let output = trace_output(result);
        assert_eq!(output.symbol, "demo.foo");
        assert!(output.paths.is_empty());
        assert!(output.cycles.is_empty());
    }

    #[test]
    fn trace_node_to_json_produces_expected_shape() {
        let node = TraceNode {
            name: "foo".into(),
            label: "Function".into(),
            file_path: Some("/demo.rs".into()),
            start_line: Some(42),
        };
        let v = trace_node_to_json(&node);
        assert_eq!(v["name"], "foo");
        assert_eq!(v["label"], "Function");
        assert_eq!(v["filePath"], "/demo.rs");
        assert_eq!(v["startLine"], 42);
    }

    #[test]
    fn trace_node_to_json_handles_missing_optional_fields() {
        let node = TraceNode {
            name: "foo".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
        };
        let v = trace_node_to_json(&node);
        assert_eq!(v["name"], "foo");
        assert!(v["filePath"].is_null(), "missing file_path should be null");
        assert!(
            v["startLine"].is_null(),
            "missing start_line should be null"
        );
    }

    #[test]
    fn trace_edge_to_json_produces_expected_shape() {
        let edge = TraceEdge {
            edge_type: "Calls".into(),
            reason: Some("direct call".into()),
            confidence: 0.9,
        };
        let v = trace_edge_to_json(&edge);
        assert_eq!(v["edgeType"], "Calls");
        assert_eq!(v["reason"], "direct call");
        let conf = v["confidence"]
            .as_f64()
            .expect("confidence should be a number");
        assert!((conf - 0.9).abs() < 1e-6, "confidence ~0.9, got {conf}");
    }

    #[test]
    fn trace_edge_to_json_handles_null_reason() {
        let edge = TraceEdge {
            edge_type: "Calls".into(),
            reason: None,
            confidence: 0.5,
        };
        let v = trace_edge_to_json(&edge);
        assert!(v["reason"].is_null(), "missing reason should be null");
    }

    #[test]
    fn trace_output_serializes_to_json() {
        let output = TraceOutput {
            symbol: "demo.foo".into(),
            paths: vec![TracePathOutput {
                nodes: vec![json!({"name": "foo"})],
                edges: vec![],
                depth: 1,
            }],
            cycles: vec![],
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"symbol\":\"demo.foo\""));
        assert!(json.contains("\"depth\":1"));
        assert!(json.contains("\"nodes\""));
    }

    #[test]
    fn trace_output_round_trips_through_json() {
        let output = TraceOutput {
            symbol: "demo.foo".into(),
            paths: vec![],
            cycles: vec![],
        };
        let json = serde_json::to_string(&output).unwrap();
        let parsed: TraceOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, parsed);
    }

    #[test]
    fn run_trace_succeeds_for_known_symbol_with_calls() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'callee', qualifiedName: 'demo.callee', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create callee");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: 'direct call', startLine: 2, project: 'demo'});").expect("create edge");

        let output = run_trace(&kit, "demo.caller", "calls", 3, "", false, false)
            .expect("trace on known symbol should succeed");
        assert_eq!(output.symbol, "demo.caller");
    }

    #[test]
    fn run_trace_succeeds_for_dataflow_trace_type() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_src', project: 'demo', name: 'src', qualifiedName: 'demo.src', filePath: '/src/s.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create src");
        storage.execute("CREATE (:Function {id: 'f_dst', project: 'demo', name: 'dst', qualifiedName: 'demo.dst', filePath: '/src/d.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create dst");
        storage.execute("CREATE (:CodeRelation {id: 'e_df', source: 'f_src', target: 'f_dst', type: 'DATAFLOWS', confidence: 1.0, confidenceTier: 'High', reason: 'assignment', startLine: 2, project: 'demo'});").expect("create dataflow edge");

        let output = run_trace(&kit, "demo.src", "dataflow", 3, "", false, false)
            .expect("dataflow trace should succeed");
        assert_eq!(output.symbol, "demo.src");
    }

    #[test]
    fn run_trace_succeeds_for_all_trace_type() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_root', project: 'demo', name: 'root', qualifiedName: 'demo.root', filePath: '/src/r.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create root");

        let output = run_trace(&kit, "demo.root", "all", 2, "", false, false)
            .expect("all trace type should succeed");
        assert_eq!(output.symbol, "demo.root");
    }

    // ===== T042: Enhanced trace tests =====

    #[test]
    fn build_path_filter_empty_returns_none() {
        assert!(build_path_filter("").is_none());
        assert!(build_path_filter("   ").is_none());
        assert!(build_path_filter(",").is_none());
        assert!(build_path_filter(" , , ").is_none());
    }

    #[test]
    fn build_path_filter_parses_single_glob() {
        let pf = build_path_filter("/src/*.rs").expect("should parse");
        assert_eq!(
            pf.include_files.as_deref(),
            Some(&["/src/*.rs".to_string()][..])
        );
        assert!(pf.exclude_files.is_none());
        assert!(pf.include_modules.is_none());
        assert!(pf.symbol_pattern.is_none());
    }

    #[test]
    fn build_path_filter_parses_multiple_globs() {
        let pf = build_path_filter("/src/*.rs, /lib/*.rs").expect("should parse");
        let patterns = pf.include_files.expect("include_files should be set");
        assert_eq!(patterns.len(), 2);
        assert!(patterns.contains(&"/src/*.rs".to_string()));
        assert!(patterns.contains(&"/lib/*.rs".to_string()));
    }

    #[test]
    fn run_trace_with_detect_cycles_finds_cycle() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // A→B→C→A (cycle)
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create c");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("a->b");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b', target: 'f_c', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("b->c");
        storage.execute("CREATE (:CodeRelation {id: 'e3', source: 'f_c', target: 'f_a', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("c->a");

        let output = run_trace(&kit, "demo.a", "calls", 5, "", true, false)
            .expect("trace with detect_cycles should succeed");
        assert!(
            !output.cycles.is_empty(),
            "should detect at least one cycle, got: {:?}",
            output.cycles
        );
    }

    #[test]
    fn run_trace_with_detect_cycles_no_cycle_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // A→B (no cycle)
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("a->b");

        let output = run_trace(&kit, "demo.a", "calls", 3, "", true, false)
            .expect("trace with detect_cycles should succeed");
        assert!(
            output.cycles.is_empty(),
            "no cycle should be detected, got: {:?}",
            output.cycles
        );
    }

    #[test]
    fn run_trace_with_cross_service_finds_http_calls() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'api_handler', qualifiedName: 'demo.api_handler', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:CodeRelation {id: 'e_http', source: 'f_a', target: 'f_b', type: 'HTTP_CALLS', confidence: 0.9, confidenceTier: 'High', reason: 'http request', startLine: 2, project: 'demo'});").expect("create http_calls edge");

        let output = run_trace(&kit, "demo.caller", "calls", 3, "", false, true)
            .expect("trace with cross_service should succeed");
        // The standard Calls tracer does not traverse HttpCalls, so
        // cross_service adds the HttpCalls edge as an additional path.
        let has_http = output.paths.iter().any(|p| {
            p.edges.iter().any(|e| {
                e.get("edgeType")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s == "HTTP_CALLS")
            })
        });
        assert!(
            has_http,
            "cross_service should expose HttpCalls edge, paths: {:?}",
            output.paths
        );
    }

    #[test]
    fn run_trace_with_cross_service_no_http_calls_returns_base() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'callee', qualifiedName: 'demo.callee', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create callee");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create calls edge");

        let base = run_trace(&kit, "demo.caller", "calls", 3, "", false, false)
            .expect("base trace should succeed");
        let cross = run_trace(&kit, "demo.caller", "calls", 3, "", false, true)
            .expect("cross_service trace should succeed");
        assert_eq!(
            base.paths.len(),
            cross.paths.len(),
            "no HttpCalls edges → same path count"
        );
    }

    #[test]
    fn run_trace_with_path_filter_filters_paths() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'callee', qualifiedName: 'demo.callee', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create callee");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create edge");

        // Filter to nonexistent file → all paths should be dropped
        let output = run_trace(
            &kit,
            "demo.caller",
            "calls",
            3,
            "/nonexistent.rs",
            false,
            false,
        )
        .expect("trace with path_filter should succeed");
        assert!(
            output.paths.is_empty(),
            "path_filter with no match should drop all paths"
        );
    }

    #[test]
    fn run_trace_with_path_filter_glob_keeps_matching() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'callee', qualifiedName: 'demo.callee', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create callee");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create edge");

        // Glob /src/*.rs matches both files → paths preserved
        let output = run_trace(&kit, "demo.caller", "calls", 3, "/src/*.rs", false, false)
            .expect("trace with path_filter should succeed");
        assert!(
            !output.paths.is_empty(),
            "glob matching both files should keep paths"
        );
    }

    #[test]
    fn run_trace_combined_detect_cycles_and_cross_service() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // A→B→A (cycle) + A -HTTP_CALLS-> C
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create c");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("a->b");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b', target: 'f_a', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("b->a");
        storage.execute("CREATE (:CodeRelation {id: 'e3', source: 'f_a', target: 'f_c', type: 'HTTP_CALLS', confidence: 0.9, confidenceTier: 'High', reason: 'http', startLine: 3, project: 'demo'});").expect("a->c http");

        let output = run_trace(&kit, "demo.a", "calls", 5, "", true, true)
            .expect("combined trace should succeed");
        assert!(
            !output.cycles.is_empty(),
            "should detect cycle, got: {:?}",
            output.cycles
        );
        let has_http = output.paths.iter().any(|p| {
            p.edges.iter().any(|e| {
                e.get("edgeType")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s == "HTTP_CALLS")
            })
        });
        assert!(
            has_http,
            "should expose HttpCalls edge, paths: {:?}",
            output.paths
        );
    }

    #[test]
    fn trace_output_serializes_with_cycles() {
        let output = TraceOutput {
            symbol: "demo.a".into(),
            paths: vec![],
            cycles: vec![TraceCycle {
                nodes: vec!["a".into(), "b".into(), "a".into()],
                edge_types: vec![crate::model::EdgeType::Calls, crate::model::EdgeType::Calls],
            }],
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"cycles\""));
        assert!(json.contains("\"nodes\""));
        assert!(json.contains("\"edge_types\""));
        assert!(json.contains("\"Calls\""));
    }

    #[test]
    fn trace_output_serializes_without_cycles_when_empty() {
        let output = TraceOutput {
            symbol: "demo.a".into(),
            paths: vec![],
            cycles: vec![],
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            !json.contains("cycles"),
            "empty cycles should be skipped in JSON, got: {json}"
        );
    }

    #[test]
    fn trace_output_deserializes_without_cycles_field() {
        // Legacy JSON without "cycles" field should deserialize correctly
        let legacy_json = r#"{"symbol":"demo.foo","paths":[]}"#;
        let parsed: TraceOutput = serde_json::from_str(legacy_json)
            .expect("legacy JSON without cycles should deserialize");
        assert_eq!(parsed.symbol, "demo.foo");
        assert!(parsed.paths.is_empty());
        assert!(
            parsed.cycles.is_empty(),
            "missing cycles should default to empty"
        );
    }

    // Covers collect_cross_service_paths early return when start node
    // not found in graph (line 139).
    #[test]
    fn collect_cross_service_paths_returns_empty_for_nonexistent_start() {
        use crate::model::Graph;
        let graph = Graph::new();
        let paths = collect_cross_service_paths(&graph, "nonexistent_id");
        assert!(paths.is_empty(), "nonexistent start_id → empty paths");
    }

    // Covers collect_cross_service_paths skipping HttpCalls edges whose
    // target node is missing from the graph (lines 146-147).
    #[test]
    fn collect_cross_service_paths_skips_missing_target_node() {
        use crate::model::{Edge, EdgeType, Graph, Node, NodeLabel};
        let mut graph = Graph::new();
        let start_node = Node::builder(NodeLabel::Function, "caller", "demo.caller")
            .id("f_a")
            .file_path("/src/a.rs")
            .build();
        graph.add_node(start_node);
        let edge = Edge::builder("f_a", "f_missing", EdgeType::HttpCalls, "demo")
            .confidence(0.9)
            .build();
        graph.add_edge(edge);
        let paths = collect_cross_service_paths(&graph, "f_a");
        assert!(
            paths.is_empty(),
            "HttpCalls edge with missing target → skip"
        );
    }

    // Covers collect_cross_service_paths creating a path for a valid
    // HttpCalls edge (lines 149-157).
    #[test]
    fn collect_cross_service_paths_creates_path_for_valid_http_edge() {
        use crate::model::{Edge, EdgeType, Graph, Node, NodeLabel};
        let mut graph = Graph::new();
        let caller = Node::builder(NodeLabel::Function, "caller", "demo.caller")
            .id("f_a")
            .file_path("/src/a.rs")
            .build();
        let handler = Node::builder(NodeLabel::Function, "handler", "demo.handler")
            .id("f_b")
            .file_path("/src/b.rs")
            .build();
        graph.add_node(caller);
        graph.add_node(handler);
        let edge = Edge::builder("f_a", "f_b", EdgeType::HttpCalls, "demo")
            .confidence(0.9)
            .reason("http request")
            .build();
        graph.add_edge(edge);
        let paths = collect_cross_service_paths(&graph, "f_a");
        assert_eq!(
            paths.len(),
            1,
            "should create 1 path for valid HttpCalls edge"
        );
        assert_eq!(paths[0].depth, 1);
        assert_eq!(paths[0].nodes.len(), 2);
        assert_eq!(paths[0].edges.len(), 1);
    }

    // ===== run_trace: additional combined mode tests =====

    #[test]
    fn run_trace_dataflow_with_detect_cycles_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'DATAFLOWS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create dataflow edge");

        let output = run_trace(&kit, "demo.a", "dataflow", 3, "", true, false)
            .expect("dataflow + detect_cycles should succeed");
        assert_eq!(output.symbol, "demo.a");
    }

    #[test]
    fn run_trace_all_type_with_cross_service_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e_http', source: 'f_a', target: 'f_b', type: 'HTTP_CALLS', confidence: 0.9, confidenceTier: 'High', reason: 'http', startLine: 2, project: 'demo'});").expect("create http edge");

        let output = run_trace(&kit, "demo.a", "all", 3, "", false, true)
            .expect("all + cross_service should succeed");
        assert_eq!(output.symbol, "demo.a");
    }

    #[test]
    fn run_trace_all_type_with_all_features_combined() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create calls edge");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b', target: 'f_a', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 3, project: 'demo'});").expect("create cycle edge");
        storage.execute("CREATE (:CodeRelation {id: 'e_http', source: 'f_a', target: 'f_b', type: 'HTTP_CALLS', confidence: 0.9, confidenceTier: 'High', reason: 'http', startLine: 4, project: 'demo'});").expect("create http edge");

        let output = run_trace(&kit, "demo.a", "all", 5, "/src/*.rs", true, true)
            .expect("all features combined should succeed");
        assert_eq!(output.symbol, "demo.a");
        assert!(!output.cycles.is_empty(), "should detect cycle");
    }

    #[test]
    fn run_trace_with_path_filter_and_cross_service_combined() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e_http', source: 'f_a', target: 'f_b', type: 'HTTP_CALLS', confidence: 0.9, confidenceTier: 'High', reason: 'http', startLine: 2, project: 'demo'});").expect("create http edge");

        let output = run_trace(&kit, "demo.a", "calls", 3, "/src/*.rs", false, true)
            .expect("path_filter + cross_service should succeed");
        assert_eq!(output.symbol, "demo.a");
    }

    #[test]
    fn run_trace_cross_service_missing_start_node_in_graph() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // Create a node with a different name than the query symbol
        storage.execute("CREATE (:Function {id: 'f_x', project: 'demo', name: 'x', qualifiedName: 'demo.x', filePath: '/src/x.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create x");

        // trace resolves the symbol, but load_graph might not find it by
        // the queried name in the graph → find_start_node_id returns None
        // → cross_service adds no paths
        let result = run_trace(&kit, "demo.x", "calls", 3, "", false, true);
        // Should succeed (no panic even if start_id not found for cross_service)
        if let Ok(output) = result {
            assert_eq!(output.symbol, "demo.x");
        }
    }

    #[test]
    fn build_path_filter_trims_whitespace_in_patterns() {
        let pf = build_path_filter("  /src/*.rs  ,  /lib/*.rs  ").expect("should parse");
        let patterns = pf.include_files.expect("include_files should be set");
        assert_eq!(patterns.len(), 2);
        assert!(patterns.contains(&"/src/*.rs".to_string()));
        assert!(patterns.contains(&"/lib/*.rs".to_string()));
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn trace_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'f1', qualifiedName: 'demo.f1', filePath: '/src/f1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f1");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(trace(
            "demo.f1".to_string(),
            "calls".to_string(),
            3,
            "".to_string(),
            false,
            false,
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn trace_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(trace(
            "demo.f1".to_string(),
            "calls".to_string(),
            3,
            "".to_string(),
            false,
            false,
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
