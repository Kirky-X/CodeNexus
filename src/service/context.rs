// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Context command: show a 360-degree view of a symbol.

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, TraceModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
use crate::trace::context::{
    collect_incoming, collect_outgoing, collect_processes, resolve_start_id,
};
use crate::trace::types::{ContextOutput, SymbolNodeOutput};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::service_api;

/// Runs context against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_context(
    kit: &AsyncKit<AsyncReady>,
    symbol: &str,
    depth: u32,
) -> Result<ContextOutput, CodeNexusError> {
    let trace_engine = kit.require::<TraceModule>()?;
    let graph = trace_engine.load_graph(symbol, depth as usize)?;
    let start_id = resolve_start_id(&graph, symbol)
        .ok_or_else(|| CodeNexusError::InvalidInput(format!("symbol not found: {symbol}")))?;
    let symbol_node = graph.get_node(&start_id).ok_or_else(|| {
        CodeNexusError::Internal(format!("symbol node resolved but not in graph: {symbol}"))
    })?;
    let incoming = collect_incoming(&graph, &start_id);
    let outgoing = collect_outgoing(&graph, &start_id);
    let processes = collect_processes(&graph, &start_id);
    Ok(ContextOutput {
        symbol: symbol.to_string(),
        node: SymbolNodeOutput::from(symbol_node),
        incoming,
        outgoing,
        processes,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "context",
    version = "0.3.2",
    description = "Show a 360-degree view of a symbol (callers, callees, processes).",
    cli = true
)]
async fn context(symbol: String, depth: u32) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result =
        run_context(&kit, &symbol, depth).map_err(|e| to_api_error(e, "context_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "context_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "context",
    version = "0.3.2",
    tool_name = "context",
    description = "Show a 360-degree view of a symbol (callers, callees, processes)."
)]
async fn context_mcp(symbol: String, depth: u32) -> Result<ContextOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_context(&kit, &symbol, depth).map_err(|e| to_api_error(e, "context_error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_context_testdb");
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
    fn run_context_returns_invalid_input_for_unknown_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "nonexistent.symbol", 3)
            .expect_err("unknown symbol should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("symbol not found"),
            "error should mention 'symbol not found': {msg}"
        );
        assert!(
            msg.contains("nonexistent.symbol"),
            "error should mention the missing symbol: {msg}"
        );
    }

    #[test]
    fn run_context_returns_invalid_input_for_empty_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "", 3).expect_err("empty symbol should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn run_context_returns_invalid_input_at_depth_zero() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "missing.symbol", 0).expect_err("missing symbol should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn run_context_error_message_format() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "foo.bar", 2).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.starts_with("invalid input: symbol not found: foo.bar"),
            "unexpected message: {msg}"
        );
    }

    #[test]
    fn run_context_returns_symbol_with_incoming_and_outgoing() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // Target symbol: demo.do_thing
        storage.execute("CREATE (:Function {id: 'f_target', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/target.rs', startLine: 10, endLine: 20, signature: 'fn do_thing()', returnType: 'void', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create target");
        // Caller: demo.caller → CALLS → demo.do_thing (incoming edge)
        storage.execute("CREATE (:Function {id: 'f_caller', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: 'fn caller()', returnType: 'void', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:CodeRelation {id: 'e_in', source: 'f_caller', target: 'f_target', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: 'direct call', startLine: 2, project: 'demo'});").expect("create incoming edge");
        // Callee: demo.do_thing → CALLS → demo.callee (outgoing edge)
        storage.execute("CREATE (:Function {id: 'f_callee', project: 'demo', name: 'callee', qualifiedName: 'demo.callee', filePath: '/src/callee.rs', startLine: 1, endLine: 5, signature: 'fn callee()', returnType: 'void', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create callee");
        storage.execute("CREATE (:CodeRelation {id: 'e_out', source: 'f_target', target: 'f_callee', type: 'CALLS', confidence: 0.9, confidenceTier: 'High', reason: 'direct call', startLine: 15, project: 'demo'});").expect("create outgoing edge");

        let output = run_context(&kit, "demo.do_thing", 3).expect("context should succeed");
        assert_eq!(output.symbol, "demo.do_thing");
        assert_eq!(output.node.qualified_name, "demo.do_thing");
        assert_eq!(output.node.name, "do_thing");
        assert_eq!(output.node.label, "Function");
        assert_eq!(output.node.file_path.as_deref(), Some("/src/target.rs"));
        assert_eq!(output.node.start_line, Some(10));
        assert_eq!(output.node.end_line, Some(20));
        // incoming: demo.caller → demo.do_thing
        assert_eq!(output.incoming.len(), 1, "should have 1 incoming caller");
        assert_eq!(output.incoming[0].name, "caller");
        assert_eq!(output.incoming[0].qualified_name, "demo.caller");
        assert_eq!(output.incoming[0].edge_type, "CALLS");
        // outgoing: demo.do_thing → demo.callee
        assert_eq!(output.outgoing.len(), 1, "should have 1 outgoing callee");
        assert_eq!(output.outgoing[0].name, "callee");
        assert_eq!(output.outgoing[0].qualified_name, "demo.callee");
        assert_eq!(output.outgoing[0].edge_type, "CALLS");
        // no process edges seeded
        assert!(output.processes.is_empty(), "no process edges expected");
    }

    #[test]
    fn run_context_returns_node_only_at_depth_zero() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_solo', project: 'demo', name: 'solo', qualifiedName: 'demo.solo', filePath: '/src/solo.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create solo");
        storage.execute("CREATE (:Function {id: 'f_other', project: 'demo', name: 'other', qualifiedName: 'demo.other', filePath: '/src/other.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create other");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_solo', target: 'f_other', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create edge");

        // depth=0: load only the start node, no BFS expansion.
        let output = run_context(&kit, "demo.solo", 0).expect("context depth 0 should succeed");
        assert_eq!(output.symbol, "demo.solo");
        assert_eq!(output.node.qualified_name, "demo.solo");
        // At depth 0 the BFS loop doesn't run, so no edges are collected.
        assert!(output.incoming.is_empty(), "no incoming at depth 0");
        assert!(output.outgoing.is_empty(), "no outgoing at depth 0");
    }

    #[test]
    fn run_context_resolves_by_short_name_when_unique() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_unique', project: 'demo', name: 'unique_name', qualifiedName: 'demo.unique_name', filePath: '/src/u.rs', startLine: 1, endLine: 2, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create unique");

        // Query by short name (not qualifiedName) — resolve_start_id matches by name.
        let output = run_context(&kit, "unique_name", 1).expect("context by name should succeed");
        assert_eq!(output.node.qualified_name, "demo.unique_name");
        assert_eq!(output.node.name, "unique_name");
    }
}
