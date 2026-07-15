// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Context command: show a 360-degree view of a symbol.

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, StorageModule, TraceModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
use crate::trace::context::{
    collect_incoming, collect_outgoing, collect_processes, resolve_start_id, ContextCollector,
    SymbolContext,
};
use crate::trace::types::{ContextOutput, SymbolNodeOutput};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

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

/// Runs enhanced context using [`ContextCollector`], returning [`SymbolContext`]
/// with type context, module context, test context, and data flow.
///
/// Unlike [`run_context`] which uses BFS graph expansion, this method queries
/// the storage directly for multi-dimensional context.
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_context_enhanced(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    symbol: &str,
) -> Result<SymbolContext, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let collector = ContextCollector::new(&*storage);
    collector
        .collect(project, symbol)
        .map_err(|e| CodeNexusError::Internal(format!("context collect failed: {e}")))
}

/// CLI wrapper — prints result to stdout as JSON.
///
/// When `enhanced=true`, uses [`ContextCollector`] to return [`SymbolContext`]
/// JSON with type/module/test context and data flow. Otherwise uses the
/// original BFS-based [`run_context`].
#[cfg(feature = "cli")]
#[forge(
    name = "context",
    version = "0.3.3",
    description = "Show a 360-degree view of a symbol (callers, callees, processes).",
    cli = true
)]
async fn context(
    symbol: String,
    depth: u32,
    project: String,
    enhanced: bool,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    if enhanced {
        let result = run_context_enhanced(&kit, &project, &symbol)
            .map_err(|e| to_api_error(e, "context_error"))?;
        let json = serde_json::to_string(&result)
            .map_err(|e| to_api_error(CodeNexusError::from(e), "context_error"))?;
        println!("{json}");
    } else {
        let result =
            run_context(&kit, &symbol, depth).map_err(|e| to_api_error(e, "context_error"))?;
        let json = serde_json::to_string(&result)
            .map_err(|e| to_api_error(CodeNexusError::from(e), "context_error"))?;
        println!("{json}");
    }
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[forge(
    name = "context",
    version = "0.3.3",
    tool_name = "context",
    description = "Show a 360-degree view of a symbol (callers, callees, processes)."
)]
#[allow(unused_variables)]
async fn context_mcp(
    symbol: String,
    depth: u32,
    project: String,
    enhanced: bool,
) -> Result<ContextOutput, ApiError> {
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
        let err =
            run_context(&kit, "nonexistent.symbol", 3).expect_err("unknown symbol should error");
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

    // ===== T038: run_context_enhanced with ContextCollector =====

    #[test]
    fn run_context_enhanced_fails_on_unknown_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context_enhanced(&kit, "demo", "nonexistent.symbol")
            .expect_err("unknown symbol should error");
        let msg = err.to_string();
        assert!(
            msg.contains("context collect failed"),
            "error should mention collect failure: {msg}"
        );
    }

    #[test]
    fn run_context_enhanced_returns_symbol_context() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // File node required by collect_module_context
        storage.execute("CREATE (:File {id: 'file1', project: 'demo', filePath: '/src/target.rs', language: 'rust'});").expect("create file");
        // Target function
        storage.execute("CREATE (:Function {id: 'f_target', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/target.rs', startLine: 10, endLine: 20, signature: 'fn do_thing()', returnType: 'void', isExported: true, docstring: 'does a thing', content: 'fn do_thing() {}', parentQn: ''});").expect("create target");

        let ctx = run_context_enhanced(&kit, "demo", "demo.do_thing")
            .expect("enhanced context should succeed");
        assert_eq!(ctx.symbol.name, "do_thing");
        assert_eq!(ctx.symbol.qualified_name, "demo.do_thing");
        assert_eq!(ctx.symbol.signature, "fn do_thing()");
        assert_eq!(ctx.symbol.file_path, "/src/target.rs");
        assert_eq!(ctx.symbol.start_line, 10);
        assert_eq!(ctx.symbol.end_line, 20);
        // type_context
        assert_eq!(ctx.type_context.return_type, "void");
        // module_context
        assert_eq!(ctx.module_context.file_path, "/src/target.rs");
        assert_eq!(ctx.module_context.package, "demo");
    }

    #[test]
    fn run_context_enhanced_serializes_to_json() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:File {id: 'file1', project: 'demo', filePath: '/src/mod.rs', language: 'rust'});").expect("create file");
        storage.execute("CREATE (:Function {id: 'f_mod', project: 'demo', name: 'mod_fn', qualifiedName: 'demo.mod_fn', filePath: '/src/mod.rs', startLine: 1, endLine: 5, signature: 'fn mod_fn()', returnType: '()', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");

        let ctx = run_context_enhanced(&kit, "demo", "demo.mod_fn")
            .expect("enhanced context should succeed");
        let json = serde_json::to_string(&ctx).expect("should serialize");
        assert!(json.contains("\"symbol\""));
        assert!(json.contains("\"qualified_name\":\"demo.mod_fn\""));
        assert!(json.contains("\"type_context\""));
        assert!(json.contains("\"module_context\""));
        assert!(json.contains("\"test_context\""));
        assert!(json.contains("\"data_flow\""));
        assert!(json.contains("\"callers\""));
        assert!(json.contains("\"callees\""));
    }

    // ===== run_context: process edges via HANDLES_ROUTE =====

    #[test]
    fn run_context_returns_processes_with_handles_route_edges() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_handler', project: 'demo', name: 'handler', qualifiedName: 'demo.handler', filePath: '/src/h.rs', startLine: 1, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:CodeRelation {id: 'e_hr', source: 'f_handler', target: 'r1', type: 'HANDLES_ROUTE', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create HANDLES_ROUTE edge");

        let output = run_context(&kit, "demo.handler", 3).expect("context should succeed");
        assert_eq!(output.symbol, "demo.handler");
        assert!(!output.processes.is_empty(), "should have process edges");
    }

    #[test]
    fn run_context_with_multiple_incoming_and_outgoing() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_target', project: 'demo', name: 'target', qualifiedName: 'demo.target', filePath: '/src/t.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create target");
        storage.execute("CREATE (:Function {id: 'f_c1', project: 'demo', name: 'caller1', qualifiedName: 'demo.caller1', filePath: '/src/c1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller1");
        storage.execute("CREATE (:Function {id: 'f_c2', project: 'demo', name: 'caller2', qualifiedName: 'demo.caller2', filePath: '/src/c2.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller2");
        storage.execute("CREATE (:Function {id: 'f_d1', project: 'demo', name: 'callee1', qualifiedName: 'demo.callee1', filePath: '/src/d1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create callee1");
        storage.execute("CREATE (:Function {id: 'f_d2', project: 'demo', name: 'callee2', qualifiedName: 'demo.callee2', filePath: '/src/d2.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create callee2");
        storage.execute("CREATE (:CodeRelation {id: 'e_in1', source: 'f_c1', target: 'f_target', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create in1");
        storage.execute("CREATE (:CodeRelation {id: 'e_in2', source: 'f_c2', target: 'f_target', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create in2");
        storage.execute("CREATE (:CodeRelation {id: 'e_out1', source: 'f_target', target: 'f_d1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 3, project: 'demo'});").expect("create out1");
        storage.execute("CREATE (:CodeRelation {id: 'e_out2', source: 'f_target', target: 'f_d2', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 4, project: 'demo'});").expect("create out2");

        let output = run_context(&kit, "demo.target", 3).expect("context should succeed");
        assert_eq!(output.incoming.len(), 2, "should have 2 incoming callers");
        assert_eq!(output.outgoing.len(), 2, "should have 2 outgoing callees");
    }

    #[test]
    fn run_context_serializes_output_to_json() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_solo', project: 'demo', name: 'solo', qualifiedName: 'demo.solo', filePath: '/src/s.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create solo");

        let output = run_context(&kit, "demo.solo", 1).expect("context should succeed");
        let json = serde_json::to_string(&output).expect("should serialize");
        assert!(json.contains("\"symbol\":\"demo.solo\""));
        assert!(json.contains("\"node\""));
        assert!(json.contains("\"incoming\""));
        assert!(json.contains("\"outgoing\""));
        assert!(json.contains("\"processes\""));
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn context_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'f1', qualifiedName: 'demo.f1', filePath: '/src/f1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f1");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(context("demo.f1".to_string(), 3, "demo".to_string(), false));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn context_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(context("demo.f1".to_string(), 3, "demo".to_string(), false));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }

    // Covers the enhanced=true branch (lines 88-93) in the wrapper:
    // run_context_enhanced → serde_json::to_string → println.
    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn context_wrapper_succeeds_with_enhanced() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:File {id: 'file1', project: 'demo', filePath: '/src/f1.rs', language: 'rust'});").expect("create file");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'f1', qualifiedName: 'demo.f1', filePath: '/src/f1.rs', startLine: 1, endLine: 5, signature: 'fn f1()', returnType: '()', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f1");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(context(
            "demo.f1".to_string(),
            3,
            "demo".to_string(),
            true, // enhanced=true
        ));
        assert!(
            result.is_ok(),
            "enhanced wrapper should succeed: {:?}",
            result.err()
        );

        reset_kit_for_testing();
    }

    // Covers the wrapper failing with an unknown symbol (enhanced=false path,
    // lines 95-96): run_context returns InvalidInput → ApiError.
    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn context_wrapper_fails_with_unknown_symbol() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(context(
            "nonexistent.symbol".to_string(),
            3,
            "demo".to_string(),
            false,
        ));
        let err = result.expect_err("unknown symbol should error");
        assert!(
            matches!(err, ApiError::InvalidInput { .. }),
            "expected InvalidInput, got {err:?}"
        );

        reset_kit_for_testing();
    }

    // Covers the wrapper failing with an unknown symbol (enhanced=true path,
    // lines 89-90): run_context_enhanced returns Internal error → ApiError.
    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn context_wrapper_fails_with_enhanced_unknown_symbol() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(context(
            "nonexistent.symbol".to_string(),
            3,
            "demo".to_string(),
            true, // enhanced=true
        ));
        assert!(
            result.is_err(),
            "enhanced wrapper should fail for unknown symbol: {:?}",
            result.err()
        );

        reset_kit_for_testing();
    }

    // Covers the wrapper with depth=0 (line 96 run_context with depth 0).
    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn context_wrapper_succeeds_with_depth_zero() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'solo', qualifiedName: 'demo.solo', filePath: '/src/s.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create solo");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(context(
            "demo.solo".to_string(),
            0, // depth=0
            "demo".to_string(),
            false,
        ));
        assert!(
            result.is_ok(),
            "depth=0 wrapper should succeed: {:?}",
            result.err()
        );

        reset_kit_for_testing();
    }
}
