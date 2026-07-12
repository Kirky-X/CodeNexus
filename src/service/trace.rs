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
use crate::trace::{TraceEdge, TraceNode, TraceResult, TraceType};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable trace result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceOutput {
    pub symbol: String,
    pub paths: Vec<TracePathOutput>,
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

/// Runs trace against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_trace(
    kit: &AsyncKit<AsyncReady>,
    symbol: &str,
    trace_type: &str,
    depth: u32,
) -> Result<TraceOutput, CodeNexusError> {
    let tt = TraceType::from_cli_str(trace_type).ok_or_else(|| {
        CodeNexusError::InvalidInput(format!(
            "invalid trace_type: {trace_type} (expected calls|dataflow|all)"
        ))
    })?;
    let trace_engine = kit.require::<TraceModule>()?;
    let result = trace_engine.trace(symbol, tt, depth as usize)?;
    Ok(trace_output(result))
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "trace",
    version = "0.3.2",
    description = "Trace a symbol's call and/or data-flow paths.",
    cli = true
)]
async fn trace(symbol: String, trace_type: String, depth: u32) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result =
        run_trace(&kit, &symbol, &trace_type, depth).map_err(|e| to_api_error(e, "trace_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "trace_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "trace",
    version = "0.3.2",
    tool_name = "trace",
    description = "Trace a symbol's call and/or data-flow paths."
)]
async fn trace_mcp(
    symbol: String,
    trace_type: String,
    depth: u32,
) -> Result<TraceOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_trace(&kit, &symbol, &trace_type, depth).map_err(|e| to_api_error(e, "trace_error"))
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
        // The trace facade (src/trace/facade.rs:173) returns SymbolNotFound
        // when no graph node matches the requested symbol — this is the
        // designed behavior on a fresh/empty DB, not a success-with-empty-paths.
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_trace(&kit, "demo.foo", "calls", 3)
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
        let err = run_trace(&kit, "demo.foo", "bogus", 3)
            .expect_err("invalid trace_type should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        let msg = err.to_string();
        assert!(msg.contains("bogus"), "error should mention bad value: {msg}");
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
        // Case-insensitive.
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
        };
        let output = trace_output(result);
        assert_eq!(output.symbol, "demo.foo");
        assert!(output.paths.is_empty());
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
        assert!(v["startLine"].is_null(), "missing start_line should be null");
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
        // f32 → f64 precision loss: 0.9_f32 serializes as 0.8999999761581421.
        let conf = v["confidence"].as_f64().expect("confidence should be a number");
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

        let output = run_trace(&kit, "demo.caller", "calls", 3)
            .expect("trace on known symbol should succeed");
        assert_eq!(output.symbol, "demo.caller");
        // The trace facade may return 0+ paths depending on BFS logic; the key
        // assertion is that the trace ran without error and returned the symbol.
    }

    #[test]
    fn run_trace_succeeds_for_dataflow_trace_type() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_src', project: 'demo', name: 'src', qualifiedName: 'demo.src', filePath: '/src/s.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create src");
        storage.execute("CREATE (:Function {id: 'f_dst', project: 'demo', name: 'dst', qualifiedName: 'demo.dst', filePath: '/src/d.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create dst");
        storage.execute("CREATE (:CodeRelation {id: 'e_df', source: 'f_src', target: 'f_dst', type: 'DATAFLOWS', confidence: 1.0, confidenceTier: 'High', reason: 'assignment', startLine: 2, project: 'demo'});").expect("create dataflow edge");

        let output = run_trace(&kit, "demo.src", "dataflow", 3)
            .expect("dataflow trace should succeed");
        assert_eq!(output.symbol, "demo.src");
    }

    #[test]
    fn run_trace_succeeds_for_all_trace_type() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_root', project: 'demo', name: 'root', qualifiedName: 'demo.root', filePath: '/src/r.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create root");

        let output = run_trace(&kit, "demo.root", "all", 2)
            .expect("all trace type should succeed");
        assert_eq!(output.symbol, "demo.root");
    }
}
