// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `trace` subcommand handler (PRD §4.2.3).
//!
//! Resolves the [`TraceEngine`](crate::trace::capability::TraceEngine)
//! capability from the [`Kit`](crate::kit::Kit) and delegates to
//! [`TraceEngine::trace`], printing the [`TraceResult`] as JSON.

use serde::Serialize;

use super::args::TraceArgs;
use super::error::{CliError, Result};
use crate::kit::{Kit, TraceKey};
use crate::trace::{TraceFacade, TraceResult, TraceType};

/// Runs the `trace` subcommand.
///
/// Resolves the [`TraceEngine`](crate::trace::capability::TraceEngine)
/// capability from `kit`, parses `--type` into a [`TraceType`], and runs
/// [`TraceEngine::trace`], printing the result as JSON.
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] for an unknown `--type` value.
/// Returns [`CliError::Trace`] for symbol-not-found / ambiguous-symbol /
/// invalid-depth errors. Returns [`CliError::Kit`] if the Trace capability
/// is not registered.
pub fn run(kit: &Kit, args: &TraceArgs) -> Result<()> {
    let trace_type = TraceType::from_cli_str(&args.trace_type).ok_or_else(|| {
        CliError::InvalidInput(format!(
            "unknown trace type '{}' (expected calls/dataflow/all)",
            args.trace_type
        ))
    })?;

    let trace = kit.require::<TraceKey>()?;
    let result = match args.min_confidence {
        Some(min_conf) => {
            // Filter path: load graph, drop low-confidence edges, trace via
            // facade (design.md D4: --min-confidence filters by edge score).
            let mut graph = trace.load_graph(&args.symbol, args.depth)?;
            let min_conf = min_conf as f32;
            graph.retain_edges(|e| e.confidence >= min_conf);
            let facade = TraceFacade::new(&graph);
            facade.trace(&args.symbol, trace_type, args.depth)?
        }
        None => trace.trace(&args.symbol, trace_type, args.depth)?,
    };
    let output = TraceOutput::from(result);
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable view of [`TraceResult`] (PRD §4.2.3 output table).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TraceOutput {
    /// The queried symbol name.
    pub symbol: String,
    /// The list of trace paths discovered.
    pub paths: Vec<TracePathOutput>,
}

/// JSON-serializable view of a single trace path.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TracePathOutput {
    /// Nodes on the path.
    pub nodes: Vec<TraceNodeOutput>,
    /// Edges on the path.
    pub edges: Vec<TraceEdgeOutput>,
    /// Path depth (number of edges).
    pub depth: usize,
}

/// JSON-serializable view of a trace node.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TraceNodeOutput {
    /// Short display name.
    pub name: String,
    /// Node label as a string.
    pub label: String,
    /// Source file path, if known.
    pub file_path: Option<String>,
    /// 1-based start line, if known.
    pub start_line: Option<u32>,
}

/// JSON-serializable view of a trace edge.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TraceEdgeOutput {
    /// Edge type as a string (e.g. `"CALLS"`).
    pub edge_type: String,
    /// Human-readable reason, if any.
    pub reason: Option<String>,
    /// Confidence score in `[0.0, 1.0]`.
    pub confidence: f32,
}

impl From<TraceResult> for TraceOutput {
    fn from(r: TraceResult) -> Self {
        Self {
            symbol: r.symbol,
            paths: r.paths.into_iter().map(TracePathOutput::from).collect(),
        }
    }
}

impl From<crate::trace::TracePath> for TracePathOutput {
    fn from(p: crate::trace::TracePath) -> Self {
        Self {
            nodes: p.nodes.into_iter().map(TraceNodeOutput::from).collect(),
            edges: p.edges.into_iter().map(TraceEdgeOutput::from).collect(),
            depth: p.depth,
        }
    }
}

impl From<crate::trace::TraceNode> for TraceNodeOutput {
    fn from(n: crate::trace::TraceNode) -> Self {
        Self {
            name: n.name,
            label: n.label,
            file_path: n.file_path,
            start_line: n.start_line,
        }
    }
}

impl From<crate::trace::TraceEdge> for TraceEdgeOutput {
    fn from(e: crate::trace::TraceEdge) -> Self {
        Self {
            edge_type: e.edge_type,
            reason: e.reason,
            confidence: e.confidence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::TraceArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, IndexerKey, StorageKey, TraceKey};
    use crate::model::{EdgeType, NodeLabel};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_trace_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// Seeds the database with two functions and a CALLS edge between them.
    fn seed_call_graph(kit: &Kit) {
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, reason: 'direct call', startLine: 2, project: 'demo'});").expect("create edge");
    }

    fn make_args(symbol: &str, trace_type: &str, depth: usize, db: &str) -> TraceArgs {
        TraceArgs {
            symbol: symbol.to_string(),
            trace_type: trace_type.to_string(),
            depth,
            db: db.to_string(),
            min_confidence: None,
        }
    }

    // --- TraceOutput serialization ---

    #[test]
    fn trace_output_serializes_to_json() {
        let out = TraceOutput {
            symbol: "main".into(),
            paths: vec![TracePathOutput {
                nodes: vec![TraceNodeOutput {
                    name: "main".into(),
                    label: "Function".into(),
                    file_path: Some("/x.rs".into()),
                    start_line: Some(1),
                }],
                edges: vec![],
                depth: 0,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"symbol\":\"main\""));
        assert!(json.contains("\"paths\""));
    }

    #[test]
    fn trace_output_from_trace_result() {
        let result = TraceResult {
            symbol: "foo".to_string(),
            paths: vec![],
        };
        let out = TraceOutput::from(result);
        assert_eq!(out.symbol, "foo");
        assert!(out.paths.is_empty());
    }

    // --- run() success ---

    #[test]
    fn run_trace_calls_returns_path() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = make_args("a", "calls", 3, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "trace should succeed: {:?}", result.err());
    }

    #[test]
    fn run_trace_all_returns_paths() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = make_args("a", "all", 3, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "trace all should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_trace_dataflow_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = make_args("a", "dataflow", 3, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "trace dataflow should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_trace_default_type_is_all() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = TraceArgs {
            symbol: "a".to_string(),
            trace_type: "all".to_string(),
            depth: 3,
            db: db.to_str().unwrap().to_string(),
            min_confidence: None,
        };
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "default trace should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_trace_depth_1_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = make_args("a", "calls", 1, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "depth 1 trace should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---

    #[test]
    fn run_trace_unknown_type_returns_exit_code_1() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = make_args("a", "bogus", 3, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("unknown type should error");
        assert_eq!(err.exit_code(), 1, "invalid input → exit 1");
    }

    #[test]
    fn run_trace_symbol_not_found_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = make_args("nonexistent", "calls", 3, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("missing symbol should error");
        // TraceError::SymbolNotFound → CliError::Trace → exit 2.
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn run_trace_zero_depth_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_graph(&kit);
        let args = make_args("a", "calls", 0, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("zero depth should error");
        // TraceError::InvalidDepth → exit 2.
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn trace_path_output_from_trace_path() {
        let path = crate::trace::TracePath {
            nodes: vec![crate::trace::TraceNode {
                name: "x".into(),
                label: "Function".into(),
                file_path: None,
                start_line: None,
            }],
            edges: vec![crate::trace::TraceEdge {
                edge_type: "CALLS".into(),
                reason: None,
                confidence: 1.0,
            }],
            depth: 1,
        };
        let out = TracePathOutput::from(path);
        assert_eq!(out.depth, 1);
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.edges.len(), 1);
    }

    #[test]
    fn trace_node_output_from_trace_node() {
        let n = crate::trace::TraceNode {
            name: "foo".into(),
            label: "Function".into(),
            file_path: Some("/x.rs".into()),
            start_line: Some(5),
        };
        let out = TraceNodeOutput::from(n);
        assert_eq!(out.name, "foo");
        assert_eq!(out.label, "Function");
        assert_eq!(out.file_path.as_deref(), Some("/x.rs"));
        assert_eq!(out.start_line, Some(5));
    }

    #[test]
    fn trace_edge_output_from_trace_edge() {
        let e = crate::trace::TraceEdge {
            edge_type: "READS".into(),
            reason: Some("r".into()),
            confidence: 0.5,
        };
        let out = TraceEdgeOutput::from(e);
        assert_eq!(out.edge_type, "READS");
        assert_eq!(out.reason.as_deref(), Some("r"));
        assert!((out.confidence - 0.5).abs() < f32::EPSILON);
    }

    // --- End-to-end: index a real file, then trace ---

    #[test]
    fn end_to_end_index_then_trace_returns_non_empty_graph() {
        use std::fs;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("main.rs"),
            "fn main() { helper(); }\nfn helper() {}\n",
        )
        .unwrap();

        let db_path = fresh_db_path();
        let kit = build_kit_for_db(db_path.to_str().unwrap());
        let indexer = kit.require::<IndexerKey>().expect("require_indexer");
        indexer.index(root, "demo", false).expect("index");

        // Trace "main" — should find the CALLS edge to "helper".
        let trace = kit.require::<TraceKey>().expect("require_trace");
        let graph = trace.load_graph("main", 3).expect("load_graph");

        assert!(
            graph.node_count() >= 1,
            "graph should have at least one node (main)"
        );
        assert!(
            graph.edge_count() >= 1,
            "graph should have at least one edge (main -> helper CALLS)"
        );
        assert!(
            graph.edges.iter().any(|e| e.edge_type == EdgeType::Calls),
            "should have a CALLS edge from main to helper"
        );
    }

    #[test]
    fn end_to_end_parameter_node_exists_after_index() {
        use std::fs;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // main writes x, then passes x to foo -> DataFlows edge x -> foo.param0.
        fs::write(
            root.join("main.rs"),
            "fn main() { let x = 1; foo(x); }\nfn foo(_v: i32) {}\n",
        )
        .unwrap();

        let db_path = fresh_db_path();
        let kit = build_kit_for_db(db_path.to_str().unwrap());
        let indexer = kit.require::<IndexerKey>().expect("require_indexer");
        indexer.index(root, "demo", false).expect("index");

        // DQ-004: trace the Parameter node by its name ("param0") to verify
        // it exists in the database and is connected via a DataFlows edge.
        let trace = kit.require::<TraceKey>().expect("require_trace");
        let graph = trace.load_graph("param0", 3).expect("load_graph");

        let param_nodes = graph.nodes_by_label(NodeLabel::Parameter);
        assert!(
            !param_nodes.is_empty(),
            "DQ-004: Parameter node should exist in graph (not orphaned)"
        );
        assert!(
            graph.edges.iter().any(|e| e.edge_type == EdgeType::DataFlows),
            "should have a DataFlows edge for parameter passing"
        );
    }

    #[test]
    fn end_to_end_variable_node_persisted_after_index() {
        // P0-1 regression: Variable nodes created by resolve_var_identifier
        // fallback must be persisted to the database. Before the pipeline fix,
        // Variable nodes were added to the in-memory graph but never collected
        // into all_nodes for persistence — leaving DataFlows edges as orphans
        // pointing at non-existent Variable nodes (Variable count = 0 in db).
        use std::fs;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // `let y = x;` triggers resolve_var_assign -> resolve_var_identifier
        // fallback for both "x" and "y" (neither is a definition node), which
        // creates Variable nodes in the graph.
        fs::write(
            root.join("main.rs"),
            "fn main() { let x = 1; let y = x; }\n",
        )
        .unwrap();

        let db_path = fresh_db_path();
        let kit = build_kit_for_db(db_path.to_str().unwrap());
        let indexer = kit.require::<IndexerKey>().expect("require_indexer");
        indexer.index(root, "demo", false).expect("index");

        // Query the Variable table directly — there must be at least one
        // Variable node persisted (for "x" or "y").
        let trace = kit.require::<TraceKey>().expect("require_trace");
        let graph = trace.load_graph("x", 3).expect("load_graph");
        let var_nodes = graph.nodes_by_label(NodeLabel::Variable);
        assert!(
            !var_nodes.is_empty(),
            "P0-1: Variable node should be persisted to db (not just in-memory). \
             Got {} Variable nodes in subgraph for 'x'",
            var_nodes.len()
        );
    }

    // Note: `run_trace_missing_db_returns_error` was removed because the
    // "missing db" error now surfaces at `build_kit` time, not at `run` time.
    // Covered by `build_kit_invalid_db_path_returns_build_failed_error` in
    // `kit::bootstrap::tests`.
}
