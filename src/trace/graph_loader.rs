// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Subgraph loader for the trace subsystem (PRD §4.2.3).
//!
//! Loads the subgraph reachable from a given symbol (within `depth` hops)
//! from the database into an in-memory [`Graph`]. The loader is shared
//! between the CLI `trace` / `impact` subcommands and the trait-kit
//! [`TraceCapability`](crate::trace::module::TraceCapability) so that both
//! code paths materialize the same in-memory graph shape.
//!
//! # History
//!
//! This module was extracted from `src/cli/trace_cmd.rs` during the
//! trait-kit unified-registry migration (T6 Phase 2, Task 2.10) so that
//! `TraceCapability` can load a subgraph without depending on the CLI
//! layer (which would create a circular dependency `trace → cli → trace`).

use std::collections::HashSet;
use std::path::Path;

use crate::model::{Edge, EdgeType, Graph, Node, NodeLabel};
use crate::storage::schema::{escape_cypher_string, escape_identifier};
use crate::storage::{Repository, StorageError};

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
///
/// # Errors
///
/// Returns [`StorageError`] for database open / query failures. Symbol
/// resolution failures (not-found / ambiguous) are NOT returned here — the
/// loader returns an empty graph instead, and [`TraceFacade::trace`] surfaces
/// the appropriate [`TraceError`](super::TraceError).
pub fn load_graph_for_symbol(
    db_path: &Path,
    symbol: &str,
    depth: usize,
) -> Result<Graph, StorageError> {
    let repo = Repository::open(db_path)?;
    // Phase 1: find start node ids matching the symbol.
    let start_ids = find_symbol_node_ids(&repo, symbol)?;
    if start_ids.is_empty() {
        // Return an empty graph; the trace facade will surface SymbolNotFound.
        return Ok(Graph::new());
    }

    // Phase 2: BFS-expand to collect reachable node ids within `depth` hops.
    //
    // `seen_edges` deduplicates edges by `(source, target, edge_type)` because
    // `fetch_edges_for_node(_, Either)` returns the same edge from both
    // endpoints — without dedup, BFS would push `a→b CALLS` twice (once when
    // visiting `a`, again when visiting `b`), inflating trace path counts.
    let mut visited: HashSet<String> = HashSet::new();
    for id in &start_ids {
        visited.insert(id.clone());
    }
    let mut frontier: Vec<String> = start_ids.clone();
    let mut edges: Vec<Edge> = Vec::new();
    let mut seen_edges: HashSet<(String, String, EdgeType)> = HashSet::new();
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
                let key = (edge.source.clone(), edge.target.clone(), edge.edge_type);
                if seen_edges.insert(key) {
                    edges.push(edge);
                }
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
fn find_symbol_node_ids(repo: &Repository, symbol: &str) -> Result<Vec<String>, StorageError> {
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
fn fetch_node_by_id(repo: &Repository, id: &str) -> Result<Option<Node>, StorageError> {
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
) -> Result<Vec<Edge>, StorageError> {
    let escaped = escape_cypher_string(node_id);
    let cypher = match direction {
        EdgeDirection::Outgoing => format!(
            "MATCH (r:CodeRelation) WHERE r.source = '{escaped}' RETURN r.source AS source, r.target AS target, r.type AS type, r.confidence AS confidence, r.confidenceTier AS confidenceTier, r.reason AS reason, r.startLine AS startLine, r.project AS project;"
        ),
        EdgeDirection::Incoming => format!(
            "MATCH (r:CodeRelation) WHERE r.target = '{escaped}' RETURN r.source AS source, r.target AS target, r.type AS type, r.confidence AS confidence, r.confidenceTier AS confidenceTier, r.reason AS reason, r.startLine AS startLine, r.project AS project;"
        ),
        EdgeDirection::Either => format!(
            "MATCH (r:CodeRelation) WHERE r.source = '{escaped}' OR r.target = '{escaped}' RETURN r.source AS source, r.target AS target, r.type AS type, r.confidence AS confidence, r.confidenceTier AS confidenceTier, r.reason AS reason, r.startLine AS startLine, r.project AS project;"
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
    // confidenceTier column may be absent in databases created before H4;
    // default to Global (fail-safe, not fail-loud — old data is unclassified).
    let confidence_tier = row
        .get(4)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(crate::model::ConfidenceTier::Global);
    let reason = row.get(5).and_then(|v| v.as_str()).map(String::from);
    let start_line = row
        .get(6)
        .and_then(|v| v.as_i64())
        .and_then(|i| u32::try_from(i).ok());
    let project = row
        .get(7)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let edge_type = parse_edge_type(type_str);
    Some(Edge {
        source,
        target,
        edge_type,
        confidence,
        confidence_tier,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StorageConnection;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("trace_loader_testdb");
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
            serde_json::json!("SAME_FILE"),
            serde_json::json!("direct call"),
            serde_json::json!(2),
            serde_json::json!("demo"),
        ];
        let edge = row_to_edge(&row).expect("edge");
        assert_eq!(edge.source, "f_a");
        assert_eq!(edge.target, "f_b");
        assert_eq!(edge.edge_type, EdgeType::Calls);
        assert!((edge.confidence - 0.95).abs() < f32::EPSILON);
        assert_eq!(edge.confidence_tier, crate::model::ConfidenceTier::SameFile);
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

    /// Regression test for BFS edge deduplication.
    ///
    /// `fetch_edges_for_node(node, Either)` returns the same `a→b` edge from
    /// both endpoints. Without dedup, BFS at depth ≥ 2 would push the edge
    /// twice — once when visiting `a`, again when visiting `b` — inflating
    /// trace path counts. This test pins the dedup behavior so the bug cannot
    /// silently return.
    #[test]
    fn load_graph_for_symbol_deduplicates_edges() {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let graph = load_graph_for_symbol(&db, "a", 3).expect("load");
        // Count CALLS edges between f_a and f_b — must be exactly 1.
        let dup_count = graph
            .edges
            .iter()
            .filter(|e| {
                e.edge_type == EdgeType::Calls
                    && e.source == "f_a"
                    && e.target == "f_b"
            })
            .count();
        assert_eq!(
            dup_count, 1,
            "expected exactly 1 f_a→f_b CALLS edge, got {dup_count} (BFS dedup regression)"
        );
    }

    #[test]
    fn fetch_edges_outgoing_direction_returns_only_outgoing() {
        // Covers EdgeDirection::Outgoing arm (line 175).
        let db = fresh_db_path();
        seed_call_graph(&db);
        let repo = Repository::open(&db).expect("open");
        let edges = fetch_edges_for_node(&repo, "f_a", EdgeDirection::Outgoing).expect("fetch");
        assert!(!edges.is_empty(), "f_a should have outgoing CALLS edges");
        assert!(edges.iter().all(|e| e.source == "f_a"), "all edges should have f_a as source");
    }

    #[test]
    fn fetch_edges_incoming_direction_returns_only_incoming() {
        // Covers EdgeDirection::Incoming arm (line 178).
        let db = fresh_db_path();
        seed_call_graph(&db);
        let repo = Repository::open(&db).expect("open");
        let edges = fetch_edges_for_node(&repo, "f_b", EdgeDirection::Incoming).expect("fetch");
        assert!(!edges.is_empty(), "f_b should have incoming CALLS edges");
        assert!(edges.iter().all(|e| e.target == "f_b"), "all edges should have f_b as target");
    }

    #[test]
    fn fetch_node_by_id_returns_none_for_missing_id() {
        // Covers the `Ok(None)` fall-through in fetch_node_by_id (line 164).
        let db = fresh_db_path();
        seed_call_graph(&db);
        let repo = Repository::open(&db).expect("open");
        let node = fetch_node_by_id(&repo, "nonexistent_id").expect("fetch");
        assert!(node.is_none(), "should return None for missing id");
    }
}
