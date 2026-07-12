// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Trace facade (trace/facade.rs) — Facade pattern (PRD §4.2, ADD §3.4).
//!
//! Provides [`TraceFacade`] which hides the [`CallGraphTracer`] /
//! [`DataFlowTracer`] dispatch behind a single entry point. Callers look up a
//! symbol by name and request a [`TraceType`]; the facade resolves the symbol
//! to a node id and delegates to the appropriate tracer(s).
//!
//! Also provides advanced tracing types (T032): [`PathFilter`],
//! [`TraceCycle`], and [`TraceEngine`] for configurable tracing over a
//! [`Storage`] reference.

use crate::model::{EdgeType, Graph, NodeId};
use crate::storage::capability::Storage;

use super::call_graph::CallGraphTracer;
use super::data_flow::DataFlowTracer;
use super::error::{Result, TraceError};
use super::module::TraceConfig;
use super::TraceResult;

use serde::{Deserialize, Serialize};

// ===== Advanced tracing types (T032) =====

/// Filter applied to trace paths (R-trace-001).
///
/// All fields are optional; `None` means "no filtering on this dimension".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PathFilter {
    /// Glob patterns — only keep nodes whose `file_path` matches at least one.
    pub include_files: Option<Vec<String>>,
    /// Glob patterns — drop nodes whose `file_path` matches any.
    pub exclude_files: Option<Vec<String>>,
    /// Only keep nodes whose qualified name starts with one of these modules.
    pub include_modules: Option<Vec<String>>,
    /// Regex pattern — only keep nodes whose `name` matches.
    pub symbol_pattern: Option<String>,
}

/// A cycle detected in the call graph (R-trace-002).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceCycle {
    /// Node names along the cycle, starting and ending at the same node.
    pub nodes: Vec<String>,
    /// Edge types traversed along the cycle.
    pub edge_types: Vec<EdgeType>,
}

/// Advanced trace engine backed by a [`Storage`] reference (design.md §8).
///
/// Holds an immutable borrow of a `dyn Storage` and a [`TraceConfig`].
/// [`TraceEngine::new`] uses default config; [`TraceEngine::with_config`]
/// accepts a custom config.
pub struct TraceEngine<'a> {
    storage: &'a dyn Storage,
    config: TraceConfig,
}

impl<'a> TraceEngine<'a> {
    /// Creates a `TraceEngine` with default [`TraceConfig`].
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self {
            storage,
            config: TraceConfig::default(),
        }
    }

    /// Creates a `TraceEngine` with the supplied [`TraceConfig`].
    #[must_use]
    pub fn with_config(storage: &'a dyn Storage, config: TraceConfig) -> Self {
        Self { storage, config }
    }

    /// Returns a reference to the engine's config.
    #[must_use]
    pub fn config(&self) -> &TraceConfig {
        &self.config
    }

    /// Returns a reference to the underlying storage.
    #[must_use]
    pub fn storage(&self) -> &dyn Storage {
        self.storage
    }
}

/// The kind of trace to perform (PRD §4.2.3 `--type` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraceType {
    /// BFS over `Calls` / `FfiCalls` edges (call graph).
    Calls,
    /// BFS over `DataFlows` / `Reads` / `Writes` edges (data flow).
    DataFlow,
    /// Both [`TraceType::Calls`] and [`TraceType::DataFlow`] (results
    /// concatenated).
    All,
}

impl TraceType {
    /// Parses the `--type` string from the CLI (`calls`/`dataflow`/`all`).
    ///
    /// Returns `None` for unrecognized strings so the CLI can surface a
    /// helpful error.
    #[must_use]
    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "calls" => Some(Self::Calls),
            "dataflow" => Some(Self::DataFlow),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// Returns the canonical CLI string for this trace type.
    #[must_use]
    pub fn as_cli_str(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::DataFlow => "dataflow",
            Self::All => "all",
        }
    }
}

/// Facade over the call-graph and data-flow tracers (Facade pattern).
///
/// Holds an immutable borrow of the [`Graph`] and exposes [`trace`] which
/// resolves `symbol` to a node id and dispatches to the appropriate tracer(s)
/// based on the requested [`TraceType`].
///
/// [`trace`]: TraceFacade::trace
pub struct TraceFacade<'a> {
    graph: &'a Graph,
}

impl<'a> TraceFacade<'a> {
    /// Creates a new `TraceFacade` bound to the given graph.
    #[must_use]
    pub fn new(graph: &'a Graph) -> Self {
        Self { graph }
    }

    /// Resolves `symbol` to a node id, then dispatches to the appropriate
    /// tracer(s) based on `trace_type`.
    ///
    /// Symbol resolution matches by node `name` first, falling back to
    /// `qualified_name`. If multiple nodes match, an
    /// [`AmbiguousSymbol`][TraceError::AmbiguousSymbol] error is returned. If
    /// no node matches, a [`SymbolNotFound`][TraceError::SymbolNotFound] error
    /// is returned.
    ///
    /// `depth` must be at least 1; otherwise an
    /// [`InvalidDepth`][TraceError::InvalidDepth] error is returned.
    pub fn trace(&self, symbol: &str, trace_type: TraceType, depth: usize) -> Result<TraceResult> {
        if depth == 0 {
            return Err(TraceError::InvalidDepth(depth));
        }
        let start_id = self.resolve_symbol(symbol)?;
        self.trace_by_id(&start_id, symbol, trace_type, depth)
    }

    /// Traces from a specific node id, bypassing symbol resolution.
    ///
    /// Used by the H14 disambiguation gate in `trace_cmd` / `impact_cmd`:
    /// when `--uid`/`--file`/`--kind` narrows to a single candidate, the
    /// caller already knows the node id and passes it here directly.
    /// `symbol_label` is used only for the `TraceResult.symbol` field.
    ///
    /// `depth` must be at least 1; otherwise an
    /// [`InvalidDepth`][TraceError::InvalidDepth] error is returned.
    pub fn trace_by_id(
        &self,
        start_id: &NodeId,
        symbol_label: &str,
        trace_type: TraceType,
        depth: usize,
    ) -> Result<TraceResult> {
        if depth == 0 {
            return Err(TraceError::InvalidDepth(depth));
        }
        let paths = match trace_type {
            TraceType::Calls => CallGraphTracer::new(self.graph).trace(start_id, depth),
            TraceType::DataFlow => DataFlowTracer::new(self.graph).trace(start_id, depth),
            TraceType::All => {
                let mut combined = CallGraphTracer::new(self.graph).trace(start_id, depth);
                combined.extend(DataFlowTracer::new(self.graph).trace(start_id, depth));
                combined
            }
        };
        Ok(TraceResult {
            symbol: symbol_label.to_string(),
            paths,
            cycles: Vec::new(),
        })
    }

    /// Resolves a symbol name (or qualified name) to a single node id.
    ///
    /// Matches `name` first, then `qualified_name`. Returns an error if zero
    /// or more than one node matches. When ambiguous, the returned
    /// [`TraceError::AmbiguousSymbol`] carries the fully-qualified names of
    /// every candidate so the CLI can surface them for disambiguation
    /// (P1-1, GitNexus UX).
    fn resolve_symbol(&self, symbol: &str) -> Result<NodeId> {
        // Single-line for coverage: tarpaulin attribute continuation
        let by_name: Vec<&crate::model::Node> = self
            .graph
            .nodes
            .values()
            .filter(|n| n.name == symbol)
            .collect();
        if by_name.len() == 1 {
            return Ok(by_name[0].id.clone());
        }
        if by_name.len() > 1 {
            // Single-line for coverage: tarpaulin attribute continuation
            let candidates: Vec<String> =
                by_name.iter().map(|n| n.qualified_name.clone()).collect();
            return Err(TraceError::AmbiguousSymbol {
                symbol: symbol.to_string(),
                candidates,
            });
        }
        // Fall back to qualified_name match.
        // Single-line for coverage: tarpaulin attribute continuation
        let by_qn: Vec<&crate::model::Node> = self
            .graph
            .nodes
            .values()
            .filter(|n| n.qualified_name == symbol)
            .collect();
        if by_qn.len() == 1 {
            return Ok(by_qn[0].id.clone());
        }
        if by_qn.len() > 1 {
            // Single-line for coverage: tarpaulin attribute continuation
            let candidates: Vec<String> = by_qn.iter().map(|n| n.qualified_name.clone()).collect();
            return Err(TraceError::AmbiguousSymbol {
                symbol: symbol.to_string(),
                candidates,
            });
        }
        Err(TraceError::SymbolNotFound(symbol.to_string()))
    }
}

/// Helper used by [`TraceFacade::trace`] tests to assert on path shapes.
#[cfg(test)]
fn path_node_names(path: &super::TracePath) -> Vec<&str> {
    path.nodes.iter().map(|n| n.name.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Node, NodeLabel};

    fn make_func(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Function, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .file_path(format!("src/{name}.rs"))
            .start_line(10)
            .build()
    }

    fn make_var(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Variable, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .build()
    }

    fn graph_a_calls_b_and_dataflow() -> Graph {
        // a calls b; a reads v
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_var("v", "v"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "v", EdgeType::Reads, "proj"));
        g
    }

    // --- TraceType ---

    #[test]
    fn trace_type_from_cli_str_parses_known() {
        assert_eq!(TraceType::from_cli_str("calls"), Some(TraceType::Calls));
        assert_eq!(
            TraceType::from_cli_str("dataflow"),
            Some(TraceType::DataFlow)
        );
        assert_eq!(TraceType::from_cli_str("all"), Some(TraceType::All));
    }

    #[test]
    fn trace_type_from_cli_str_case_insensitive() {
        assert_eq!(TraceType::from_cli_str("CALLS"), Some(TraceType::Calls));
        assert_eq!(
            TraceType::from_cli_str("DataFlow"),
            Some(TraceType::DataFlow)
        );
        assert_eq!(TraceType::from_cli_str("ALL"), Some(TraceType::All));
    }

    #[test]
    fn trace_type_from_cli_str_rejects_unknown() {
        assert!(TraceType::from_cli_str("unknown").is_none());
        assert!(TraceType::from_cli_str("").is_none());
        assert!(TraceType::from_cli_str("calls ").is_none());
    }

    #[test]
    fn trace_type_as_cli_str_roundtrips() {
        for t in [TraceType::Calls, TraceType::DataFlow, TraceType::All] {
            let s = t.as_cli_str();
            assert_eq!(TraceType::from_cli_str(s), Some(t));
        }
    }

    // --- TraceFacade: Calls ---

    #[test]
    fn facade_trace_calls_returns_call_path() {
        // AC-TRACE-001: A calls B -> trace A --type calls returns A->B path.
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::Calls, 3).unwrap();
        assert_eq!(result.symbol, "a");
        assert_eq!(result.paths.len(), 1);
        assert_eq!(path_node_names(&result.paths[0]), vec!["a", "b"]);
        assert_eq!(result.paths[0].edges[0].edge_type, "CALLS");
    }

    #[test]
    fn facade_trace_calls_does_not_include_dataflow() {
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::Calls, 3).unwrap();
        // Only the call path A->B; no Reads path A->v.
        for p in &result.paths {
            for e in &p.edges {
                assert_eq!(e.edge_type, "CALLS");
            }
        }
    }

    // --- TraceFacade: DataFlow ---

    #[test]
    fn facade_trace_dataflow_returns_dataflow_path() {
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::DataFlow, 3).unwrap();
        assert_eq!(result.paths.len(), 1);
        assert_eq!(path_node_names(&result.paths[0]), vec!["a", "v"]);
        assert_eq!(result.paths[0].edges[0].edge_type, "READS");
    }

    #[test]
    fn facade_trace_dataflow_does_not_include_calls() {
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::DataFlow, 3).unwrap();
        for p in &result.paths {
            for e in &p.edges {
                assert_ne!(e.edge_type, "CALLS");
            }
        }
    }

    // --- TraceFacade: All ---

    #[test]
    fn facade_trace_all_returns_both_call_and_dataflow_paths() {
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::All, 3).unwrap();
        // 1 call path (A->B) + 1 dataflow path (A->v) = 2 paths.
        assert_eq!(result.paths.len(), 2);
        let edge_types: Vec<String> = result
            .paths
            .iter()
            .filter(|p| p.depth == 1)
            .map(|p| p.edges[0].edge_type.clone())
            .collect();
        assert!(edge_types.contains(&"CALLS".to_string()));
        assert!(edge_types.contains(&"READS".to_string()));
    }

    // --- TraceFacade: error cases ---

    #[test]
    fn facade_trace_symbol_not_found_returns_error() {
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("missing", TraceType::Calls, 3);
        assert!(matches!(result, Err(TraceError::SymbolNotFound(_))));
        if let Err(TraceError::SymbolNotFound(s)) = result {
            assert_eq!(s, "missing");
        }
    }

    #[test]
    fn facade_trace_ambiguous_symbol_returns_error_with_candidates() {
        // Two nodes named "a" -> ambiguous. P1-1: error must carry both
        // candidate FQNs so the CLI can list them.
        let mut g = Graph::new();
        g.add_node(make_func("a1", "a"));
        g.add_node(make_func("a2", "a"));
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::Calls, 3);
        match result {
            Err(TraceError::AmbiguousSymbol { symbol, candidates }) => {
                assert_eq!(symbol, "a");
                assert_eq!(
                    candidates.len(),
                    2,
                    "expected 2 candidate FQNs, got {candidates:?}"
                );
                // make_func sets qualified_name = format!("proj.{name}") = "proj.a"
                // for both. Verify both candidate FQNs are present (order is not
                // guaranteed by HashMap iteration).
                assert!(
                    candidates.iter().any(|q| q == "proj.a"),
                    "candidates should contain proj.a: {candidates:?}"
                );
            }
            other => panic!("expected AmbiguousSymbol with candidates, got {other:?}"),
        }
    }

    #[test]
    fn facade_trace_ambiguous_symbol_by_qualified_name_returns_candidates() {
        // Two nodes with the same qualified_name "proj.dup" matched via the
        // qualified_name fallback branch. P1-1: candidates must still be
        // populated.
        let mut g = Graph::new();
        let n1 = Node::builder(NodeLabel::Function, "first", "proj.dup".to_string())
            .id("dup1")
            .project("proj")
            .build();
        let n2 = Node::builder(NodeLabel::Function, "second", "proj.dup".to_string())
            .id("dup2")
            .project("proj")
            .build();
        g.add_node(n1);
        g.add_node(n2);
        let facade = TraceFacade::new(&g);
        let result = facade.trace("proj.dup", TraceType::Calls, 3);
        match result {
            Err(TraceError::AmbiguousSymbol { symbol, candidates }) => {
                assert_eq!(symbol, "proj.dup");
                assert_eq!(candidates.len(), 2, "got {candidates:?}");
                assert!(candidates.iter().all(|q| q == "proj.dup"));
            }
            other => panic!("expected AmbiguousSymbol, got {other:?}"),
        }
    }

    #[test]
    fn facade_trace_invalid_depth_returns_error() {
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::Calls, 0);
        assert!(matches!(result, Err(TraceError::InvalidDepth(0))));
    }

    // --- TraceFacade: AC-TRACE-004 depth limit ---

    #[test]
    fn facade_trace_respects_depth_limit() {
        // AC-TRACE-004: --depth 2 -> paths depth <= 2.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "d", EdgeType::Calls, "proj"));
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::Calls, 2).unwrap();
        for p in &result.paths {
            assert!(p.depth <= 2);
        }
    }

    #[test]
    fn facade_trace_depth_1_returns_only_direct_edges() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::Calls, 1).unwrap();
        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0].depth, 1);
    }

    // --- TraceFacade: qualified_name resolution ---

    #[test]
    fn facade_trace_resolves_by_qualified_name() {
        let mut g = Graph::new();
        let node = Node::builder(NodeLabel::Function, "foo", "proj.src.foo")
            .id("foo-id")
            .project("proj")
            .build();
        g.add_node(node);
        g.add_node(make_func("bar", "bar"));
        g.add_edge(Edge::new("foo-id", "bar", EdgeType::Calls, "proj"));
        let facade = TraceFacade::new(&g);
        // Resolve by qualified_name since "foo" name also matches.
        let result = facade.trace("proj.src.foo", TraceType::Calls, 3).unwrap();
        assert_eq!(result.paths.len(), 1);
        assert_eq!(path_node_names(&result.paths[0]), vec!["foo", "bar"]);
    }

    // --- TraceFacade: result symbol field ---

    #[test]
    fn facade_trace_result_symbol_matches_input() {
        let g = graph_a_calls_b_and_dataflow();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::All, 3).unwrap();
        assert_eq!(result.symbol, "a");
    }

    #[test]
    fn facade_trace_empty_graph_symbol_not_found() {
        let g = Graph::new();
        let facade = TraceFacade::new(&g);
        let result = facade.trace("anything", TraceType::Calls, 3);
        assert!(matches!(result, Err(TraceError::SymbolNotFound(_))));
    }

    #[test]
    fn facade_trace_no_outgoing_edges_returns_empty_paths() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        let facade = TraceFacade::new(&g);
        let result = facade.trace("a", TraceType::Calls, 3).unwrap();
        assert!(result.paths.is_empty());
    }

    #[test]
    fn facade_trace_all_with_only_dataflow_edges() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let facade = TraceFacade::new(&g);
        let result = facade.trace("x", TraceType::All, 3).unwrap();
        // Only dataflow path; no call paths.
        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0].edges[0].edge_type, "DATAFLOWS");
    }

    // --- T032: Advanced tracing types ---

    fn build_storage() -> std::sync::Arc<dyn Storage> {
        use crate::kit::StorageModule;
        use crate::storage::StorageConfig;
        StorageModule::build_cap(&StorageConfig::in_memory()).expect("StorageModule::build_cap")
    }

    #[test]
    fn path_filter_default_is_all_none() {
        let pf = PathFilter::default();
        assert!(pf.include_files.is_none());
        assert!(pf.exclude_files.is_none());
        assert!(pf.include_modules.is_none());
        assert!(pf.symbol_pattern.is_none());
    }

    #[test]
    fn path_filter_serializes_to_json() {
        let pf = PathFilter {
            include_files: Some(vec!["/src/a.rs".to_string()]),
            exclude_files: None,
            include_modules: Some(vec!["mod_a".to_string()]),
            symbol_pattern: Some("handler.*".to_string()),
        };
        let json = serde_json::to_string(&pf).expect("serialize PathFilter");
        assert!(json.contains("\"include_files\""));
        assert!(json.contains("/src/a.rs"));
        assert!(json.contains("\"include_modules\""));
        assert!(json.contains("handler.*"));
        assert!(json.contains("\"exclude_files\":null"));
    }

    #[test]
    fn path_filter_round_trips_through_json() {
        let pf = PathFilter {
            include_files: Some(vec!["/src/*.rs".to_string()]),
            exclude_files: Some(vec!["/target/*".to_string()]),
            include_modules: None,
            symbol_pattern: Some("test_.*".to_string()),
        };
        let json = serde_json::to_string(&pf).expect("serialize");
        let deserialized: PathFilter = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pf, deserialized);
    }

    #[test]
    fn trace_cycle_serializes_with_edge_types() {
        let cycle = TraceCycle {
            nodes: vec!["a".to_string(), "b".to_string(), "c".to_string(), "a".to_string()],
            edge_types: vec![EdgeType::Calls, EdgeType::Calls, EdgeType::Calls],
        };
        let json = serde_json::to_string(&cycle).expect("serialize TraceCycle");
        assert!(json.contains("\"nodes\""));
        assert!(json.contains("\"edge_types\""));
        assert!(json.contains("\"Calls\""));
        let de: TraceCycle = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cycle, de);
    }

    #[test]
    fn trace_config_default_has_expected_values() {
        let config = super::TraceConfig::default();
        assert_eq!(config.max_depth, 5);
        assert_eq!(config.edge_types, vec![EdgeType::Calls]);
        assert!(config.path_filter.is_none());
        assert!(!config.detect_cycles);
        assert!(!config.cross_service);
    }

    #[test]
    fn trace_config_clamped_depth_respects_limit() {
        let mut config = super::TraceConfig::default();
        config.max_depth = 50;
        assert_eq!(config.clamped_depth(), 10);
        config.max_depth = 3;
        assert_eq!(config.clamped_depth(), 3);
    }

    #[test]
    fn trace_config_serializes_to_json() {
        let config = super::TraceConfig {
            db_path: std::path::PathBuf::from(":memory:"),
            max_depth: 7,
            edge_types: vec![EdgeType::Calls, EdgeType::HttpCalls],
            path_filter: Some(PathFilter {
                include_files: Some(vec!["/src/*.rs".to_string()]),
                ..Default::default()
            }),
            detect_cycles: true,
            cross_service: true,
        };
        let json = serde_json::to_string(&config).expect("serialize TraceConfig");
        assert!(json.contains("\"max_depth\":7"));
        assert!(json.contains("\"detect_cycles\":true"));
        assert!(json.contains("\"cross_service\":true"));
        assert!(json.contains("\"HttpCalls\""));
    }

    #[test]
    fn trace_engine_new_uses_default_config() {
        let storage = build_storage();
        let engine = TraceEngine::new(storage.as_ref());
        assert_eq!(engine.config().max_depth, 5);
        assert!(!engine.config().detect_cycles);
        assert!(!engine.config().cross_service);
    }

    #[test]
    fn trace_engine_with_config_applies_custom_config() {
        let storage = build_storage();
        let config = super::TraceConfig {
            db_path: std::path::PathBuf::from(":memory:"),
            max_depth: 10,
            edge_types: vec![EdgeType::Calls, EdgeType::HttpCalls],
            path_filter: None,
            detect_cycles: true,
            cross_service: true,
        };
        let engine = TraceEngine::with_config(storage.as_ref(), config);
        assert_eq!(engine.config().max_depth, 10);
        assert!(engine.config().detect_cycles);
        assert!(engine.config().cross_service);
        assert_eq!(engine.config().edge_types.len(), 2);
    }

    #[test]
    fn trace_result_serializes_with_cycles_field() {
        let result = super::TraceResult {
            symbol: "a".to_string(),
            paths: Vec::new(),
            cycles: vec![TraceCycle {
                nodes: vec!["a".to_string(), "b".to_string(), "a".to_string()],
                edge_types: vec![EdgeType::Calls, EdgeType::Calls],
            }],
        };
        let json = serde_json::to_string(&result).expect("serialize TraceResult");
        assert!(json.contains("\"symbol\":\"a\""));
        assert!(json.contains("\"paths\":[]"));
        assert!(json.contains("\"cycles\""));
        assert!(json.contains("\"Calls\""));
    }
}
