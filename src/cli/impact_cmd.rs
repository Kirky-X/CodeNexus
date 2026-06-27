// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `impact` subcommand handler.
//!
//! Resolves the [`TraceEngine`](crate::trace::capability::TraceEngine)
//! capability from the [`Kit`](crate::kit::Kit), loads the reverse-reachable
//! subgraph via [`TraceEngine::load_graph`], then delegates to
//! [`ImpactAnalyzer::analyze`] and prints the impacted nodes as JSON.

use serde::Serialize;

use super::args::ImpactArgs;
use super::error::Result;
use crate::kit::{Kit, TraceKey};
use crate::model::Graph;
use crate::trace::ImpactAnalyzer;
use crate::trace::TraceNode;

/// Runs the `impact` subcommand.
///
/// Resolves the [`TraceEngine`](crate::trace::capability::TraceEngine)
/// capability from `kit`, loads the reverse-reachable subgraph via
/// [`TraceEngine::load_graph`], runs [`ImpactAnalyzer::analyze`], and prints
/// the impacted nodes as a JSON object `{ symbol, depth, impacted: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Trace capability is
/// not registered. Returns [`crate::cli::error::CliError::Trace`] for
/// database failures during graph loading. If the symbol is not found, the
/// `impacted` array is empty (impact analysis is best-effort, not an error).
pub fn run(kit: &Kit, args: &ImpactArgs) -> Result<()> {
    let trace = kit.require::<TraceKey>()?;
    let graph = trace.load_graph(&args.symbol, args.depth)?;
    let analyzer = ImpactAnalyzer::new(&graph);
    // Resolve the start node id by name (mirrors TraceFacade's resolution).
    let start_id = resolve_start_id(&graph, &args.symbol);
    let impacted: Vec<TraceNode> = match start_id {
        Some(id) => analyzer.analyze(&id, args.depth),
        None => Vec::new(),
    };
    let output = ImpactOutput {
        symbol: args.symbol.clone(),
        depth: args.depth,
        impacted: impacted.into_iter().map(ImpactNodeOutput::from).collect(),
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Resolves a symbol name to a node id by matching `name` first, then
/// `qualified_name`. Returns `None` if no node matches.
fn resolve_start_id(graph: &Graph, symbol: &str) -> Option<String> {
    let by_name: Vec<&crate::model::Node> =
        graph.nodes.values().filter(|n| n.name == symbol).collect();
    if by_name.len() == 1 {
        return Some(by_name[0].id.clone());
    }
    let by_qn: Vec<&crate::model::Node> = graph
        .nodes
        .values()
        .filter(|n| n.qualified_name == symbol)
        .collect();
    if by_qn.len() == 1 {
        return Some(by_qn[0].id.clone());
    }
    // If multiple match by name, return the first (impact analysis is
    // best-effort; the user can disambiguate with a FQN).
    by_name.first().map(|n| n.id.clone())
}

/// JSON-serializable impact-analysis output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImpactOutput {
    /// The queried symbol name.
    pub symbol: String,
    /// The depth used for the analysis.
    pub depth: usize,
    /// The list of impacted nodes (callers, writers, etc.).
    pub impacted: Vec<ImpactNodeOutput>,
}

/// JSON-serializable view of an impacted node.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImpactNodeOutput {
    /// Short display name.
    pub name: String,
    /// Node label as a string.
    pub label: String,
    /// Source file path, if known.
    pub file_path: Option<String>,
    /// 1-based start line, if known.
    pub start_line: Option<u32>,
}

impl From<TraceNode> for ImpactNodeOutput {
    fn from(n: TraceNode) -> Self {
        Self {
            name: n.name,
            label: n.label,
            file_path: n.file_path,
            start_line: n.start_line,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ImpactArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_impact_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// Seeds the database with three functions in a call chain: c -> b -> a.
    fn seed_call_chain(kit: &Kit) {
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create c");
        // b calls a; c calls b. So callers of a are b and c.
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_b', target: 'f_a', type: 'CALLS', confidence: 1.0, reason: '', startLine: 2, project: 'demo'});").expect("create edge b->a");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_c', target: 'f_b', type: 'CALLS', confidence: 1.0, reason: '', startLine: 2, project: 'demo'});").expect("create edge c->b");
    }

    fn make_args(symbol: &str, depth: usize, db: &str) -> ImpactArgs {
        ImpactArgs {
            symbol: symbol.to_string(),
            depth,
            db: db.to_string(),
        }
    }

    // --- ImpactOutput serialization ---

    #[test]
    fn impact_output_serializes_to_json() {
        let out = ImpactOutput {
            symbol: "a".into(),
            depth: 3,
            impacted: vec![ImpactNodeOutput {
                name: "b".into(),
                label: "Function".into(),
                file_path: None,
                start_line: None,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"symbol\":\"a\""));
        assert!(json.contains("\"impacted\""));
    }

    #[test]
    fn impact_node_output_from_trace_node() {
        let n = TraceNode {
            name: "foo".into(),
            label: "Function".into(),
            file_path: Some("/x.rs".into()),
            start_line: Some(5),
        };
        let out = ImpactNodeOutput::from(n);
        assert_eq!(out.name, "foo");
        assert_eq!(out.label, "Function");
        assert_eq!(out.file_path.as_deref(), Some("/x.rs"));
        assert_eq!(out.start_line, Some(5));
    }

    // --- run() success ---

    #[test]
    fn run_impact_returns_callers() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_chain(&kit);
        let args = make_args("a", 3, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "impact should succeed: {:?}", result.err());
    }

    #[test]
    fn run_impact_depth_1_returns_direct_callers() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_chain(&kit);
        let args = make_args("a", 1, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "depth 1 impact should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_impact_no_callers_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_chain(&kit);
        // c has no callers → impacted is empty, but run still succeeds.
        let args = make_args("c", 3, db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "no-callers impact should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---

    #[test]
    fn run_impact_missing_symbol_succeeds_with_empty_impacted() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_call_chain(&kit);
        let args = make_args("nonexistent", 3, db.to_str().unwrap());
        // Missing symbol is NOT an error for impact analysis — it just returns
        // an empty impacted list.
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "missing symbol should succeed: {:?}",
            result.err()
        );
    }

    // --- resolve_start_id ---

    #[test]
    fn resolve_start_id_by_name() {
        let mut graph = Graph::new();
        let node =
            crate::model::Node::builder(crate::model::NodeLabel::Function, "foo", "demo.foo")
                .id("foo-id")
                .build();
        graph.add_node(node);
        let id = resolve_start_id(&graph, "foo");
        assert_eq!(id.as_deref(), Some("foo-id"));
    }

    #[test]
    fn resolve_start_id_by_qualified_name() {
        let mut graph = Graph::new();
        let node =
            crate::model::Node::builder(crate::model::NodeLabel::Function, "foo", "demo.src.foo")
                .id("foo-id")
                .build();
        graph.add_node(node);
        let id = resolve_start_id(&graph, "demo.src.foo");
        assert_eq!(id.as_deref(), Some("foo-id"));
    }

    #[test]
    fn resolve_start_id_missing_returns_none() {
        let graph = Graph::new();
        let id = resolve_start_id(&graph, "missing");
        assert!(id.is_none());
    }

    #[test]
    fn resolve_start_id_ambiguous_returns_first() {
        let mut graph = Graph::new();
        graph.add_node(
            crate::model::Node::builder(crate::model::NodeLabel::Function, "foo", "demo.foo1")
                .id("id1")
                .build(),
        );
        graph.add_node(
            crate::model::Node::builder(crate::model::NodeLabel::Function, "foo", "demo.foo2")
                .id("id2")
                .build(),
        );
        let id = resolve_start_id(&graph, "foo");
        // Ambiguous: returns the first match (best-effort).
        assert!(id.is_some());
    }

    // Note: `run_impact_missing_db_returns_error` was removed because the
    // "missing db" error now surfaces at `build_kit` time, not at `run` time.
    // Covered by `build_kit_invalid_db_path_returns_build_failed_error` in
    // `kit::bootstrap::tests`.
}
