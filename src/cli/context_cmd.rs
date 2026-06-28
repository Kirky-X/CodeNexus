// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `context` subcommand handler (H8).
//!
//! Produces a 360° view of a symbol: the resolved node, its incoming edges
//! (callers/importers/readers/writers), outgoing edges (callees/imports/uses),
//! and the processes/routes/endpoints the symbol participates in.
//!
//! The subcommand resolves the [`TraceEngine`] capability from the [`Kit`],
//! loads the BFS subgraph around the symbol via [`TraceEngine::load_graph`],
//! then partitions the loaded edges into the four sections described above.

use serde::Serialize;

use super::args::ContextArgs;
use super::error::Result;
use crate::kit::{Kit, TraceKey};
use crate::model::{EdgeType, Graph, Node, NodeId};
use crate::trace::TraceError;

/// Runs the `context` subcommand.
///
/// # Errors
///
/// Returns [`CliError::Trace`] if the symbol is not found or the graph cannot
/// be loaded. Returns [`CliError::Kit`] if the Trace capability is not
/// registered.
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

/// Resolves a symbol name to a node id by matching `name` first, then
/// `qualified_name`. Returns `None` if no node matches.
fn resolve_start_id(graph: &Graph, symbol: &str) -> Option<NodeId> {
    let by_name: Vec<&Node> = graph.nodes.values().filter(|n| n.name == symbol).collect();
    if by_name.len() == 1 {
        return Some(by_name[0].id.clone());
    }
    let by_qn: Vec<&Node> = graph
        .nodes
        .values()
        .filter(|n| n.qualified_name == symbol)
        .collect();
    if by_qn.len() == 1 {
        return Some(by_qn[0].id.clone());
    }
    by_name.first().map(|n| n.id.clone())
}

/// Collects incoming edges (other nodes pointing at `start_id`).
fn collect_incoming(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges_to(start_id) {
        if let Some(src) = graph.get_node(&edge.source) {
            out.push(RelatedNodeOutput {
                name: src.name.clone(),
                label: src.label.to_string(),
                qualified_name: src.qualified_name.clone(),
                file_path: src.file_path.clone(),
                start_line: src.start_line,
                edge_type: edge.edge_type.to_string(),
                edge_confidence: edge.confidence,
                edge_reason: edge.reason.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.edge_type.cmp(&b.edge_type).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Collects outgoing edges (`start_id` pointing at other nodes).
fn collect_outgoing(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges_from(start_id) {
        if let Some(dst) = graph.get_node(&edge.target) {
            out.push(RelatedNodeOutput {
                name: dst.name.clone(),
                label: dst.label.to_string(),
                qualified_name: dst.qualified_name.clone(),
                file_path: dst.file_path.clone(),
                start_line: dst.start_line,
                edge_type: edge.edge_type.to_string(),
                edge_confidence: edge.confidence,
                edge_reason: edge.reason.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.edge_type.cmp(&b.edge_type).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Collects process/route/endpoint/tool nodes the symbol participates in.
///
/// Walks both directions of the structural edge types `StepInProcess`,
/// `EntryPointOf`, `HandlesRoute`, and `HandlesTool` — the symbol may be either
/// the participant (source) or the process itself (target).
fn collect_processes(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    const PROCESS_EDGE_TYPES: [EdgeType; 4] = [
        EdgeType::StepInProcess,
        EdgeType::EntryPointOf,
        EdgeType::HandlesRoute,
        EdgeType::HandlesTool,
    ];
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges.iter() {
        if !PROCESS_EDGE_TYPES.contains(&edge.edge_type) {
            continue;
        }
        let other_id = if edge.source == *start_id {
            Some(&edge.target)
        } else if edge.target == *start_id {
            Some(&edge.source)
        } else {
            None
        };
        let Some(other_id) = other_id else { continue };
        let Some(other) = graph.get_node(other_id) else { continue };
        out.push(RelatedNodeOutput {
            name: other.name.clone(),
            label: other.label.to_string(),
            qualified_name: other.qualified_name.clone(),
            file_path: other.file_path.clone(),
            start_line: other.start_line,
            edge_type: edge.edge_type.to_string(),
            edge_confidence: edge.confidence,
            edge_reason: edge.reason.clone(),
        });
    }
    out.sort_by(|a, b| a.edge_type.cmp(&b.edge_type).then_with(|| a.name.cmp(&b.name)));
    out
}

/// JSON-serializable 360° context output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContextOutput {
    /// The queried symbol name.
    pub symbol: String,
    /// The resolved symbol node.
    pub node: SymbolNodeOutput,
    /// Nodes with edges pointing at the symbol (callers, importers, etc.).
    pub incoming: Vec<RelatedNodeOutput>,
    /// Nodes the symbol points at (callees, imports, etc.).
    pub outgoing: Vec<RelatedNodeOutput>,
    /// Processes/routes/endpoints the symbol participates in.
    pub processes: Vec<RelatedNodeOutput>,
}

/// JSON-serializable view of the resolved symbol node.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SymbolNodeOutput {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub language: Option<String>,
    pub signature: Option<String>,
    pub is_exported: bool,
}

impl From<&Node> for SymbolNodeOutput {
    fn from(n: &Node) -> Self {
        Self {
            name: n.name.clone(),
            label: n.label.to_string(),
            qualified_name: n.qualified_name.clone(),
            file_path: n.file_path.clone(),
            start_line: n.start_line,
            end_line: n.end_line,
            language: n.language.map(|l| l.to_string()),
            signature: n.signature.clone(),
            is_exported: n.is_exported,
        }
    }
}

/// JSON-serializable view of a node related to the symbol by an edge.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RelatedNodeOutput {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub edge_type: String,
    pub edge_confidence: f32,
    pub edge_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ContextArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use crate::model::{Edge, EdgeType, Language, Node, NodeLabel};
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

    fn make_node(id: &str, name: &str, qn: &str, label: NodeLabel, file: &str, line: u32) -> Node {
        Node::builder(label, name, qn)
            .id(id)
            .file_path(file)
            .start_line(line)
            .end_line(line + 5)
            .language(Language::Rust)
            .signature(format!("fn {name}()"))
            .is_exported(true)
            .build()
    }

    fn make_args(symbol: &str, depth: usize, db: &str) -> ContextArgs {
        ContextArgs {
            symbol: symbol.to_string(),
            db: db.to_string(),
            depth,
        }
    }

    // --- SymbolNodeOutput ---

    #[test]
    fn symbol_node_output_from_node() {
        let node = make_node("f1", "foo", "demo.foo", NodeLabel::Function, "/x.rs", 10);
        let out = SymbolNodeOutput::from(&node);
        assert_eq!(out.name, "foo");
        assert_eq!(out.label, "Function");
        assert_eq!(out.qualified_name, "demo.foo");
        assert_eq!(out.file_path.as_deref(), Some("/x.rs"));
        assert_eq!(out.start_line, Some(10));
        assert_eq!(out.end_line, Some(15));
        assert_eq!(out.language.as_deref(), Some("rust"));
        assert_eq!(out.signature.as_deref(), Some("fn foo()"));
        assert!(out.is_exported);
    }

    // --- RelatedNodeOutput sorting ---

    #[test]
    fn related_nodes_sort_by_edge_type_then_name() {
        let mut v = [
            RelatedNodeOutput {
                name: "z".into(),
                label: "Function".into(),
                qualified_name: "demo.z".into(),
                file_path: None,
                start_line: None,
                edge_type: "CALLS".into(),
                edge_confidence: 0.9,
                edge_reason: None,
            },
            RelatedNodeOutput {
                name: "a".into(),
                label: "Function".into(),
                qualified_name: "demo.a".into(),
                file_path: None,
                start_line: None,
                edge_type: "CALLS".into(),
                edge_confidence: 0.5,
                edge_reason: None,
            },
            RelatedNodeOutput {
                name: "b".into(),
                label: "Module".into(),
                qualified_name: "demo.b".into(),
                file_path: None,
                start_line: None,
                edge_type: "IMPORTS".into(),
                edge_confidence: 1.0,
                edge_reason: None,
            },
        ];
        v.sort_by(|a, b| {
            a.edge_type.cmp(&b.edge_type).then_with(|| a.name.cmp(&b.name))
        });
        // CALLS sorts before IMPORTS (alphabetical); within CALLS, sort by name.
        assert_eq!(v[0].edge_type, "CALLS");
        assert_eq!(v[0].name, "a");
        assert_eq!(v[1].edge_type, "CALLS");
        assert_eq!(v[1].name, "z");
        assert_eq!(v[2].edge_type, "IMPORTS");
        assert_eq!(v[2].name, "b");
    }

    // --- resolve_start_id ---

    #[test]
    fn resolve_start_id_by_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node("id1", "foo", "demo.foo", NodeLabel::Function, "/x.rs", 1));
        assert_eq!(resolve_start_id(&graph, "foo").as_deref(), Some("id1"));
    }

    #[test]
    fn resolve_start_id_by_qualified_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node("id1", "foo", "demo.foo", NodeLabel::Function, "/x.rs", 1));
        assert_eq!(
            resolve_start_id(&graph, "demo.foo").as_deref(),
            Some("id1")
        );
    }

    #[test]
    fn resolve_start_id_missing_returns_none() {
        let graph = Graph::new();
        assert!(resolve_start_id(&graph, "missing").is_none());
    }

    // --- collect_incoming / collect_outgoing ---

    #[test]
    fn collect_incoming_returns_callers() {
        let mut graph = Graph::new();
        graph.add_node(make_node("a", "a", "demo.a", NodeLabel::Function, "/a.rs", 1));
        graph.add_node(make_node("b", "b", "demo.b", NodeLabel::Function, "/b.rs", 1));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let incoming = collect_incoming(&graph, &"b".to_string());
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].name, "a");
        assert_eq!(incoming[0].edge_type, "CALLS");
    }

    #[test]
    fn collect_outgoing_returns_callees() {
        let mut graph = Graph::new();
        graph.add_node(make_node("a", "a", "demo.a", NodeLabel::Function, "/a.rs", 1));
        graph.add_node(make_node("b", "b", "demo.b", NodeLabel::Function, "/b.rs", 1));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let outgoing = collect_outgoing(&graph, &"a".to_string());
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].name, "b");
        assert_eq!(outgoing[0].edge_type, "CALLS");
    }

    // --- collect_processes ---

    #[test]
    fn collect_processes_finds_step_in_process() {
        let mut graph = Graph::new();
        graph.add_node(make_node("a", "a", "demo.a", NodeLabel::Function, "/a.rs", 1));
        graph.add_node(
            Node::builder(NodeLabel::Process, "checkout", "demo.checkout")
                .id("p1")
                .build(),
        );
        graph.add_edge(Edge::new("a", "p1", EdgeType::StepInProcess, "demo"));
        let processes = collect_processes(&graph, &"a".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "checkout");
        assert_eq!(processes[0].edge_type, "STEP_IN_PROCESS");
    }

    #[test]
    fn collect_processes_finds_entry_point_of() {
        let mut graph = Graph::new();
        graph.add_node(make_node("main", "main", "demo.main", NodeLabel::Function, "/m.rs", 1));
        graph.add_node(
            Node::builder(NodeLabel::Process, "bootstrap", "demo.bootstrap")
                .id("p1")
                .build(),
        );
        // main is the entry point of bootstrap → edge main -> bootstrap.
        graph.add_edge(Edge::new("main", "p1", EdgeType::EntryPointOf, "demo"));
        let processes = collect_processes(&graph, &"main".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "bootstrap");
        assert_eq!(processes[0].edge_type, "ENTRY_POINT_OF");
    }

    #[test]
    fn collect_processes_ignores_call_edges() {
        let mut graph = Graph::new();
        graph.add_node(make_node("a", "a", "demo.a", NodeLabel::Function, "/a.rs", 1));
        graph.add_node(make_node("b", "b", "demo.b", NodeLabel::Function, "/b.rs", 1));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let processes = collect_processes(&graph, &"a".to_string());
        assert!(processes.is_empty());
    }

    // --- run() success ---

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
        // c -> b -> a: b has incoming from c and outgoing to a.
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

    // --- run() error cases ---

    #[test]
    fn run_context_missing_symbol_returns_trace_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("nonexistent", 2, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("missing symbol should error");
        // TraceError::SymbolNotFound → CliError::Trace → exit 2.
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

    // --- ContextOutput serialization ---

    #[test]
    fn context_output_serializes_to_json() {
        let out = ContextOutput {
            symbol: "main".into(),
            node: SymbolNodeOutput {
                name: "main".into(),
                label: "Function".into(),
                qualified_name: "demo.main".into(),
                file_path: Some("/x.rs".into()),
                start_line: Some(1),
                end_line: Some(10),
                language: Some("rust".into()),
                signature: Some("fn main()".into()),
                is_exported: true,
            },
            incoming: vec![],
            outgoing: vec![],
            processes: vec![],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"symbol\":\"main\""));
        assert!(json.contains("\"node\""));
        assert!(json.contains("\"incoming\""));
        assert!(json.contains("\"outgoing\""));
        assert!(json.contains("\"processes\""));
    }
}
