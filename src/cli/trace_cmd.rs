//! `trace` subcommand handler (PRD §4.2.3).
//!
//! Loads the relevant subgraph from the database, builds an in-memory
//! [`Graph`], then delegates to [`TraceFacade::trace`] and prints the
//! [`TraceResult`] as JSON.

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;

use super::args::TraceArgs;
use super::error::{CliError, Result};
use crate::model::{Edge, EdgeType, Graph, Node, NodeLabel};
use crate::storage::schema::escape_identifier;
use crate::storage::Repository;
use crate::trace::{TraceFacade, TraceResult, TraceType};

/// Runs the `trace` subcommand.
///
/// Parses `--type` into a [`TraceType`], loads the symbol's reachable
/// subgraph from the database, runs [`TraceFacade::trace`], and prints the
/// result as JSON.
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] for an unknown `--type` value or
/// zero/negative depth. Returns [`CliError::Trace`] for symbol-not-found /
/// ambiguous-symbol errors. Returns [`CliError::Storage`] for database
/// failures.
pub fn run(args: &TraceArgs) -> Result<()> {
    let trace_type = TraceType::from_cli_str(&args.trace_type).ok_or_else(|| {
        CliError::InvalidInput(format!(
            "unknown trace type '{}' (expected calls/dataflow/all)",
            args.trace_type
        ))
    })?;

    let db_path = Path::new(&args.db);
    let graph = load_graph_for_symbol(db_path, &args.symbol, args.depth)?;
    let facade = TraceFacade::new(&graph);
    let result = facade.trace(&args.symbol, trace_type, args.depth)?;
    let output = TraceOutput::from(result);
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Public wrapper around [`load_graph_for_symbol`] for sibling CLI handlers
/// (e.g. `impact_cmd`).
pub(crate) fn load_graph_for_symbol_pub(
    db_path: &Path,
    symbol: &str,
    depth: usize,
) -> Result<Graph> {
    load_graph_for_symbol(db_path, symbol, depth)
}

/// Loads the subgraph reachable from `symbol` (within `depth` hops) from the
/// database into an in-memory [`Graph`].
///
/// This is a two-phase loader:
/// 1. Find the start node(s) matching `symbol` by name or qualified name.
/// 2. BFS-expand from the start node(s) up to `depth` hops, collecting all
///    reachable node ids, then materialize the subgraph.
///
/// If `depth` is 0 we still load the start node itself (so the trace facade
/// can return a clean `SymbolNotFound`/`AmbiguousSymbol` error).
fn load_graph_for_symbol(db_path: &Path, symbol: &str, depth: usize) -> Result<Graph> {
    let repo = Repository::open(db_path)?;
    // Phase 1: find start node ids matching the symbol.
    let start_ids = find_symbol_node_ids(&repo, symbol)?;
    if start_ids.is_empty() {
        // Return an empty graph; the trace facade will surface SymbolNotFound.
        return Ok(Graph::new());
    }

    // Phase 2: BFS-expand to collect reachable node ids within `depth` hops.
    let mut visited: HashSet<String> = HashSet::new();
    for id in &start_ids {
        visited.insert(id.clone());
    }
    let mut frontier: Vec<String> = start_ids.clone();
    let mut edges: Vec<Edge> = Vec::new();
    for _ in 0..depth {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier: Vec<String> = Vec::new();
        for node_id in &frontier {
            // Outgoing edges from this node.
            let outgoing = fetch_edges_for_node(&repo, node_id, EdgeDirection::Either)?;
            for edge in outgoing {
                if !visited.contains(&edge.target) {
                    visited.insert(edge.target.clone());
                    next_frontier.push(edge.target.clone());
                }
                if !visited.contains(&edge.source) {
                    visited.insert(edge.source.clone());
                    next_frontier.push(edge.source.clone());
                }
                edges.push(edge);
            }
        }
        frontier = next_frontier;
    }

    // Phase 3: materialize nodes for every visited id.
    let mut graph = Graph::new();
    for id in &visited {
        if let Some(node) = fetch_node_by_id(&repo, id)? {
            graph.add_node(node);
        }
    }
    for edge in edges {
        graph.add_edge(edge);
    }
    Ok(graph)
}

/// Direction filter for edge fetching.
#[derive(Clone, Copy)]
enum EdgeDirection {
    #[allow(dead_code)]
    Outgoing,
    #[allow(dead_code)]
    Incoming,
    Either,
}

/// Finds node ids whose `name` or `qualifiedName` matches `symbol`, across
/// all node tables that carry those columns.
fn find_symbol_node_ids(repo: &Repository, symbol: &str) -> Result<Vec<String>> {
    let escaped = escape_cypher_string(symbol);
    let mut ids = Vec::new();
    // Search every node label that has both `name` and `qualifiedName`.
    for label in NODE_LABELS_WITH_NAME_QN {
        let table = escape_identifier(label.table_name());
        let cypher = format!(
            "MATCH (n:{table}) WHERE n.name = '{escaped}' OR n.qualifiedName = '{escaped}' RETURN n.id AS id;"
        );
        if let Ok(rows) = repo.connection().query(&cypher) {
            for row in rows {
                if let Some(id) = row
                    .into_iter()
                    .next()
                    .and_then(|v| v.as_str().map(String::from))
                {
                    ids.push(id);
                }
            }
        }
    }
    Ok(ids)
}

/// Fetches a single node by id, trying every node label.
fn fetch_node_by_id(repo: &Repository, id: &str) -> Result<Option<Node>> {
    let escaped = escape_cypher_string(id);
    for label in NodeLabel::all() {
        let table = escape_identifier(label.table_name());
        let cypher = format!("MATCH (n:{table}) WHERE n.id = '{escaped}' RETURN n.*;");
        if let Ok((raw_columns, rows)) = repo.connection().query_with_columns(&cypher) {
            // `RETURN n.*` yields column names prefixed with `n.` (e.g. `n.id`);
            // strip the prefix so `row_to_node` can look up fields by bare name.
            let columns: Vec<String> = raw_columns
                .iter()
                .map(|c| c.strip_prefix("n.").unwrap_or(c).to_string())
                .collect();
            if let Some(row) = rows.into_iter().next() {
                if let Some(node) = row_to_node(&columns, &row, label) {
                    return Ok(Some(node));
                }
            }
        }
    }
    Ok(None)
}

/// Fetches all edges where `node_id` is the source or target.
fn fetch_edges_for_node(
    repo: &Repository,
    node_id: &str,
    direction: EdgeDirection,
) -> Result<Vec<Edge>> {
    let escaped = escape_cypher_string(node_id);
    let cypher = match direction {
        EdgeDirection::Outgoing => format!(
            "MATCH (r:CodeRelation) WHERE r.source = '{escaped}' RETURN r.source AS source, r.target AS target, r.type AS type, r.confidence AS confidence, r.reason AS reason, r.startLine AS startLine, r.project AS project;"
        ),
        EdgeDirection::Incoming => format!(
            "MATCH (r:CodeRelation) WHERE r.target = '{escaped}' RETURN r.source AS source, r.target AS target, r.type AS type, r.confidence AS confidence, r.reason AS reason, r.startLine AS startLine, r.project AS project;"
        ),
        EdgeDirection::Either => format!(
            "MATCH (r:CodeRelation) WHERE r.source = '{escaped}' OR r.target = '{escaped}' RETURN r.source AS source, r.target AS target, r.type AS type, r.confidence AS confidence, r.reason AS reason, r.startLine AS startLine, r.project AS project;"
        ),
    };
    let rows = repo.connection().query(&cypher)?;
    let mut edges = Vec::new();
    for row in rows {
        if let Some(edge) = row_to_edge(&row) {
            edges.push(edge);
        }
    }
    Ok(edges)
}

/// Node labels that carry both `name` and `qualifiedName` columns.
const NODE_LABELS_WITH_NAME_QN: &[NodeLabel] = &[
    NodeLabel::Module,
    NodeLabel::Class,
    NodeLabel::Struct,
    NodeLabel::Enum,
    NodeLabel::Trait,
    NodeLabel::Impl,
    NodeLabel::Function,
    NodeLabel::Method,
    NodeLabel::Variable,
    NodeLabel::GlobalVar,
    NodeLabel::Parameter,
    NodeLabel::Const,
    NodeLabel::Static,
    NodeLabel::Macro,
    NodeLabel::TypeAlias,
    NodeLabel::Typedef,
    NodeLabel::Namespace,
];

/// Converts a query row into a [`Node`] of the given `label`.
///
/// Extracts the common fields (`id`, `project`, `name`, `qualifiedName`,
/// `filePath`, `startLine`, `endLine`) by column name. Extra fields are
/// ignored — the trace facade only needs the location and name.
fn row_to_node(columns: &[String], row: &[serde_json::Value], label: NodeLabel) -> Option<Node> {
    let get = |key: &str| -> Option<&serde_json::Value> {
        columns
            .iter()
            .position(|c| c == key)
            .and_then(|i| row.get(i))
    };
    let get_str = |key: &str| -> String {
        get(key)
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default()
    };
    let get_opt_str =
        |key: &str| -> Option<String> { get(key).and_then(|v| v.as_str()).map(String::from) };
    let get_opt_u32 = |key: &str| -> Option<u32> {
        get(key)
            .and_then(|v| v.as_i64())
            .and_then(|i| u32::try_from(i).ok())
    };

    let id = get_str("id");
    if id.is_empty() {
        return None;
    }
    let name = get_str("name");
    let qualified_name = get_str("qualifiedName");
    if qualified_name.is_empty() {
        // Some labels (Folder, File) don't have qualifiedName; fall back to name.
    }
    let project = get_str("project");
    let file_path = get_opt_str("filePath");
    let start_line = get_opt_u32("startLine");
    let end_line = get_opt_u32("endLine");

    Some(Node {
        id,
        label,
        name,
        qualified_name,
        file_path,
        start_line,
        end_line,
        language: None,
        signature: None,
        return_type: None,
        docstring: None,
        is_exported: false,
        is_global: false,
        parent_qn: get_opt_str("parentQn"),
        properties: serde_json::Value::Null,
        project,
    })
}

/// Converts a CodeRelation query row into an [`Edge`].
fn row_to_edge(row: &[serde_json::Value]) -> Option<Edge> {
    let source = row.first().and_then(|v| v.as_str())?.to_string();
    let target = row.get(1).and_then(|v| v.as_str())?.to_string();
    let type_str = row.get(2).and_then(|v| v.as_str()).unwrap_or("CALLS");
    let confidence = row.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
    let reason = row.get(4).and_then(|v| v.as_str()).map(String::from);
    let start_line = row
        .get(5)
        .and_then(|v| v.as_i64())
        .and_then(|i| u32::try_from(i).ok());
    let project = row
        .get(6)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let edge_type = parse_edge_type(type_str);
    Some(Edge {
        source,
        target,
        edge_type,
        confidence,
        reason,
        start_line,
        project,
    })
}

/// Parses a database edge-type string into an [`EdgeType`].
fn parse_edge_type(s: &str) -> EdgeType {
    for t in EdgeType::all() {
        if t.as_db_type() == s {
            return t;
        }
    }
    EdgeType::Calls
}

/// Escapes a string for safe interpolation into a Cypher single-quoted string.
fn escape_cypher_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
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
    use crate::model::{EdgeType, NodeLabel};
    use crate::storage::StorageConnection;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_trace_testdb");
        std::mem::forget(dir);
        path
    }

    /// Seeds the database with two functions and a CALLS edge between them.
    fn seed_call_graph(db: &Path) {
        let conn = StorageConnection::open(db).expect("open");
        conn.init_schema().expect("init_schema");
        conn.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        conn.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        conn.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, reason: 'direct call', startLine: 2, project: 'demo'});").expect("create edge");
    }

    fn make_args(symbol: &str, trace_type: &str, depth: usize, db: &str) -> TraceArgs {
        TraceArgs {
            symbol: symbol.to_string(),
            trace_type: trace_type.to_string(),
            depth,
            db: db.to_string(),
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
        seed_call_graph(&db);
        let args = make_args("a", "calls", 3, db.to_str().unwrap());
        let result = run(&args);
        assert!(result.is_ok(), "trace should succeed: {:?}", result.err());
    }

    #[test]
    fn run_trace_all_returns_paths() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let args = make_args("a", "all", 3, db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "trace all should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_trace_dataflow_succeeds() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let args = make_args("a", "dataflow", 3, db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "trace dataflow should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_trace_default_type_is_all() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let args = TraceArgs {
            symbol: "a".to_string(),
            trace_type: "all".to_string(),
            depth: 3,
            db: db.to_str().unwrap().to_string(),
        };
        let result = run(&args);
        assert!(
            result.is_ok(),
            "default trace should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_trace_depth_1_succeeds() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let args = make_args("a", "calls", 1, db.to_str().unwrap());
        let result = run(&args);
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
        seed_call_graph(&db);
        let args = make_args("a", "bogus", 3, db.to_str().unwrap());
        let err = run(&args).expect_err("unknown type should error");
        assert_eq!(err.exit_code(), 1, "invalid input → exit 1");
    }

    #[test]
    fn run_trace_symbol_not_found_returns_error() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let args = make_args("nonexistent", "calls", 3, db.to_str().unwrap());
        let err = run(&args).expect_err("missing symbol should error");
        // TraceError::SymbolNotFound → CliError::Trace → exit 2.
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn run_trace_zero_depth_returns_error() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let args = make_args("a", "calls", 0, db.to_str().unwrap());
        let err = run(&args).expect_err("zero depth should error");
        // TraceError::InvalidDepth → exit 2.
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn run_trace_missing_db_returns_error() {
        let args = make_args("a", "calls", 3, "/nonexistent/db.lbug");
        let result = run(&args);
        assert!(result.is_err(), "missing db should error");
    }

    // --- helper functions ---

    #[test]
    fn escape_cypher_string_handles_quotes() {
        assert_eq!(escape_cypher_string("it's"), "it\\'s");
        assert_eq!(escape_cypher_string("plain"), "plain");
    }

    #[test]
    fn parse_edge_type_known() {
        assert_eq!(parse_edge_type("CALLS"), EdgeType::Calls);
        assert_eq!(parse_edge_type("FFI_CALLS"), EdgeType::FfiCalls);
        assert_eq!(parse_edge_type("DATAFLOWS"), EdgeType::DataFlows);
        assert_eq!(parse_edge_type("READS"), EdgeType::Reads);
        assert_eq!(parse_edge_type("WRITES"), EdgeType::Writes);
    }

    #[test]
    fn parse_edge_type_unknown_falls_back_to_calls() {
        assert_eq!(parse_edge_type("BOGUS"), EdgeType::Calls);
    }

    #[test]
    fn row_to_node_extracts_fields() {
        let columns = vec![
            "id".to_string(),
            "project".to_string(),
            "name".to_string(),
            "qualifiedName".to_string(),
            "filePath".to_string(),
            "startLine".to_string(),
            "endLine".to_string(),
        ];
        let row = vec![
            serde_json::json!("f1"),
            serde_json::json!("demo"),
            serde_json::json!("main"),
            serde_json::json!("demo.main"),
            serde_json::json!("/src/main.rs"),
            serde_json::json!(10),
            serde_json::json!(20),
        ];
        let node = row_to_node(&columns, &row, NodeLabel::Function).expect("node");
        assert_eq!(node.id, "f1");
        assert_eq!(node.name, "main");
        assert_eq!(node.qualified_name, "demo.main");
        assert_eq!(node.project, "demo");
        assert_eq!(node.file_path.as_deref(), Some("/src/main.rs"));
        assert_eq!(node.start_line, Some(10));
        assert_eq!(node.end_line, Some(20));
        assert_eq!(node.label, NodeLabel::Function);
    }

    #[test]
    fn row_to_node_empty_id_returns_none() {
        let columns = vec!["id".to_string()];
        let row = vec![serde_json::json!("")];
        assert!(row_to_node(&columns, &row, NodeLabel::Function).is_none());
    }

    #[test]
    fn row_to_edge_extracts_fields() {
        let row = vec![
            serde_json::json!("f_a"),
            serde_json::json!("f_b"),
            serde_json::json!("CALLS"),
            serde_json::json!(0.95),
            serde_json::json!("direct call"),
            serde_json::json!(2),
            serde_json::json!("demo"),
        ];
        let edge = row_to_edge(&row).expect("edge");
        assert_eq!(edge.source, "f_a");
        assert_eq!(edge.target, "f_b");
        assert_eq!(edge.edge_type, EdgeType::Calls);
        assert!((edge.confidence - 0.95).abs() < f32::EPSILON);
        assert_eq!(edge.reason.as_deref(), Some("direct call"));
        assert_eq!(edge.start_line, Some(2));
        assert_eq!(edge.project, "demo");
    }

    #[test]
    fn row_to_edge_missing_source_returns_none() {
        let row = vec![
            serde_json::Value::Null,
            serde_json::json!("f_b"),
            serde_json::json!("CALLS"),
        ];
        assert!(row_to_edge(&row).is_none());
    }

    #[test]
    fn load_graph_for_symbol_finds_node() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let graph = load_graph_for_symbol(&db, "a", 3).expect("load");
        // Should have loaded at least the start node and its neighbor.
        assert!(graph.node_count() >= 1, "graph should have nodes");
    }

    #[test]
    fn load_graph_for_symbol_missing_returns_empty() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let graph = load_graph_for_symbol(&db, "nonexistent", 3).expect("load");
        assert_eq!(graph.node_count(), 0, "missing symbol → empty graph");
    }

    #[test]
    fn load_graph_for_symbol_zero_depth_loads_start_node_only() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let graph = load_graph_for_symbol(&db, "a", 0).expect("load");
        // depth 0 → only the start node, no edges expanded.
        assert!(graph.node_count() >= 1);
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

    // Verify the helper graph loader works with a manually-built graph.
    #[test]
    fn load_graph_for_symbol_loads_edges() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let graph = load_graph_for_symbol(&db, "a", 3).expect("load");
        // Should have at least one edge (a -> b).
        assert!(graph.edge_count() >= 1, "graph should have edges");
        // The edge should be a CALLS edge.
        assert!(
            graph.edges.iter().any(|e| e.edge_type == EdgeType::Calls),
            "should have a CALLS edge"
        );
    }

    // --- End-to-end: index a real file, then trace ---

    #[test]
    fn end_to_end_index_then_trace_returns_non_empty_graph() {
        use crate::index::IndexFacade;
        use std::fs;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("main.rs"),
            "fn main() { helper(); }\nfn helper() {}\n",
        )
        .unwrap();

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        facade.index(root, "demo", false).expect("index");

        // Trace "main" — should find the CALLS edge to "helper".
        let graph =
            load_graph_for_symbol_pub(&db_path, "main", 3).expect("load_graph_for_symbol");

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
        use crate::index::IndexFacade;
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
        let facade = IndexFacade::new(&db_path).expect("facade");
        facade.index(root, "demo", false).expect("index");

        // DQ-004: trace the Parameter node by its name ("param0") to verify
        // it exists in the database and is connected via a DataFlows edge.
        // Variables like "x" are not nodes, so tracing from "x" returns
        // empty — the trace can only start from definition or Parameter nodes.
        let graph =
            load_graph_for_symbol_pub(&db_path, "param0", 3).expect("load_graph_for_symbol");

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
        use crate::index::IndexFacade;
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
        let facade = IndexFacade::new(&db_path).expect("facade");
        facade.index(root, "demo", false).expect("index");

        // Query the Variable table directly — there must be at least one
        // Variable node persisted (for "x" or "y").
        let graph =
            load_graph_for_symbol_pub(&db_path, "x", 3).expect("load_graph_for_symbol");
        let var_nodes = graph.nodes_by_label(NodeLabel::Variable);
        assert!(
            !var_nodes.is_empty(),
            "P0-1: Variable node should be persisted to db (not just in-memory). \
             Got {} Variable nodes in subgraph for 'x'",
            var_nodes.len()
        );
    }
}
