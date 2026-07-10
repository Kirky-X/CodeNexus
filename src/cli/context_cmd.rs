// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `context` subcommand handler.

use super::args::ContextArgs;
use super::error::Result;
use crate::kit::{Kit, TraceKey};
use crate::trace::context::{
    collect_incoming, collect_outgoing, collect_processes, resolve_start_id,
};
use crate::trace::types::{ContextOutput, SymbolNodeOutput};
use crate::trace::TraceError;

pub fn run(kit: &Kit, args: &ContextArgs) -> Result<()> {
    let trace = kit.require::<TraceKey>()?;
    let graph = trace.load_graph(&args.symbol, args.depth)?;
    let start_id = resolve_start_id(&graph, &args.symbol)
        .ok_or_else(|| TraceError::SymbolNotFound(args.symbol.clone()))?;

    let symbol_node = graph
        .get_node(&start_id)
        .ok_or_else(|| TraceError::SymbolNotFound(args.symbol.clone()))?;

    let incoming = collect_incoming(&graph, &start_id);
    let outgoing = collect_outgoing(&graph, &start_id);
    let processes = collect_processes(&graph, &start_id);

    let output = ContextOutput {
        symbol: args.symbol.clone(),
        node: SymbolNodeOutput::from(symbol_node),
        incoming,
        outgoing,
        processes,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ContextArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_context_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    fn make_args(symbol: &str, depth: usize, db: &str) -> ContextArgs {
        ContextArgs {
            symbol: symbol.to_string(),
            db: db.to_string(),
            depth,
        }
    }

    #[test]
    fn run_context_returns_symbol_node() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, reason: 'direct call', startLine: 2, project: 'demo'});").unwrap();
        let args = make_args("a", 2, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "context should succeed: {:?}", result.err());
    }

    #[test]
    fn run_context_returns_incoming_and_outgoing() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_b', target: 'f_a', type: 'CALLS', confidence: 1.0, reason: '', startLine: 2, project: 'demo'});").unwrap();
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_c', target: 'f_b', type: 'CALLS', confidence: 1.0, reason: '', startLine: 2, project: 'demo'});").unwrap();
        let args = make_args("b", 2, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "context should succeed: {:?}", result.err());
    }

    #[test]
    fn run_context_with_process_node() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_main', project: 'demo', name: 'main', qualifiedName: 'demo.main', filePath: '/src/main.rs', startLine: 1, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        storage.execute("CREATE (:Process {id: 'p1', project: 'demo', name: 'bootstrap', qualifiedName: 'demo.bootstrap', docstring: ''});").unwrap();
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_main', target: 'p1', type: 'ENTRY_POINT_OF', confidence: 1.0, reason: '', startLine: null, project: 'demo'});").unwrap();
        let args = make_args("main", 2, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "context should succeed: {:?}", result.err());
    }

    #[test]
    fn run_context_missing_symbol_returns_trace_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("nonexistent", 2, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("missing symbol should error");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn run_context_invalid_depth_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("a", 0, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("zero depth should error");
        assert_eq!(err.exit_code(), 2, "TraceError::InvalidDepth → exit 2");
    }
}
