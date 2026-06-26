//! Trace facade (trace/facade.rs) — Facade pattern (PRD §4.2, ADD §3.4).
//!
//! Provides [`TraceFacade`] which hides the [`CallGraphTracer`] /
//! [`DataFlowTracer`] dispatch behind a single entry point. Callers look up a
//! symbol by name and request a [`TraceType`]; the facade resolves the symbol
//! to a node id and delegates to the appropriate tracer(s).

use crate::model::{Graph, NodeId};

use super::call_graph::CallGraphTracer;
use super::data_flow::DataFlowTracer;
use super::error::{Result, TraceError};
use super::TraceResult;

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
        let paths = match trace_type {
            TraceType::Calls => CallGraphTracer::new(self.graph).trace(&start_id, depth),
            TraceType::DataFlow => DataFlowTracer::new(self.graph).trace(&start_id, depth),
            TraceType::All => {
                let mut combined = CallGraphTracer::new(self.graph).trace(&start_id, depth);
                combined.extend(DataFlowTracer::new(self.graph).trace(&start_id, depth));
                combined
            }
        };
        Ok(TraceResult {
            symbol: symbol.to_string(),
            paths,
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
            let candidates: Vec<String> =
                by_name.iter().map(|n| n.qualified_name.clone()).collect();
            return Err(TraceError::AmbiguousSymbol {
                symbol: symbol.to_string(),
                candidates,
            });
        }
        // Fall back to qualified_name match.
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
            let candidates: Vec<String> =
                by_qn.iter().map(|n| n.qualified_name.clone()).collect();
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
}
