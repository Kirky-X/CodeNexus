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

    /// Filters trace paths by the given [`PathFilter`] (R-trace-001).
    ///
    /// Delegates to the standalone [`apply_path_filter`].
    pub fn apply_path_filter(
        &self,
        paths: Vec<super::TracePath>,
        filter: &PathFilter,
    ) -> Vec<super::TracePath> {
        apply_path_filter(paths, filter)
    }
}

// ===== Path filtering (T033) =====

/// Converts a glob pattern to a regex string.
///
/// Supports `*` (any sequence) and `?` (single char). All other regex
/// metacharacters are escaped.
fn glob_to_regex(glob: &str) -> String {
    // Note: `[` and `]` are NOT escaped — glob character classes (e.g. `[abc]`)
    // pass through as regex character classes. An unmatched `[` will cause
    // regex compilation to fail, handled gracefully by the caller.
    const META_CHARS: &[char] = &['.', '+', '^', '$', '(', ')', '{', '}', '|', '\\'];
    let mut regex = String::with_capacity(glob.len() * 2);
    for ch in glob.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            c if META_CHARS.contains(&c) => {
                regex.push('\\');
                regex.push(c);
            }
            c => regex.push(c),
        }
    }
    regex
}

/// Returns true if `value` matches any of the glob patterns.
fn matches_any_glob(value: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        let regex_str = glob_to_regex(p);
        regex::Regex::new(&regex_str)
            .map(|re| re.is_match(value))
            .unwrap_or(false)
    })
}

/// Returns true if `value` matches the regex pattern.
fn matches_regex(value: &str, pattern: &str) -> bool {
    regex::Regex::new(pattern)
        .map(|re| re.is_match(value))
        .unwrap_or(false)
}

/// Checks whether a single [`TraceNode`] passes all filter dimensions.
fn node_passes_filter(
    node: &super::TraceNode,
    filter: &PathFilter,
) -> bool {
    // include_files: if set, node's file_path must match at least one glob.
    if let Some(ref patterns) = filter.include_files {
        let Some(ref file_path) = node.file_path else {
            return false;
        };
        if !matches_any_glob(file_path, patterns) {
            return false;
        }
    }
    // exclude_files: if set, node's file_path must NOT match any glob.
    if let Some(ref patterns) = filter.exclude_files {
        if let Some(ref file_path) = node.file_path {
            if matches_any_glob(file_path, patterns) {
                return false;
            }
        }
    }
    // include_modules: if set, file_path must contain one of the module strings.
    if let Some(ref modules) = filter.include_modules {
        let Some(ref file_path) = node.file_path else {
            return false;
        };
        if !modules.iter().any(|m| file_path.contains(m.as_str())) {
            return false;
        }
    }
    // symbol_pattern: if set, node's name must match the regex.
    if let Some(ref pattern) = filter.symbol_pattern {
        if !matches_regex(&node.name, pattern) {
            return false;
        }
    }
    true
}

/// Filters trace paths by the given [`PathFilter`] (R-trace-001).
///
/// For each path, nodes that fail the filter are removed. Edges between
/// consecutive remaining nodes are preserved; non-consecutive gaps drop the
/// corresponding edges. Paths with zero remaining nodes are removed entirely.
pub fn apply_path_filter(
    paths: Vec<super::TracePath>,
    filter: &PathFilter,
) -> Vec<super::TracePath> {
    paths
        .into_iter()
        .filter_map(|path| {
            let keep_indices: Vec<usize> = path
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| node_passes_filter(n, filter))
                .map(|(i, _)| i)
                .collect();
            if keep_indices.is_empty() {
                return None;
            }
            let nodes: Vec<super::TraceNode> = keep_indices
                .iter()
                .map(|&i| path.nodes[i].clone())
                .collect();
            let mut edges = Vec::new();
            for window in keep_indices.windows(2) {
                if window[1] == window[0] + 1 {
                    edges.push(path.edges[window[0]].clone());
                }
            }
            let depth = edges.len();
            Some(super::TracePath {
                nodes,
                edges,
                depth,
            })
        })
        .collect()
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

    // --- T033: Path filtering ---

    use crate::trace::{TraceEdge, TraceNode, TracePath};

    fn make_node_with_path(name: &str, file_path: &str) -> TraceNode {
        TraceNode {
            name: name.to_string(),
            label: "Function".to_string(),
            file_path: Some(file_path.to_string()),
            start_line: Some(1),
        }
    }

    fn make_3node_path() -> TracePath {
        // a -> b -> c, each in a different file.
        TracePath {
            nodes: vec![
                make_node_with_path("a", "/src/a.rs"),
                make_node_with_path("b", "/src/b.rs"),
                make_node_with_path("c", "/src/c.rs"),
            ],
            edges: vec![
                TraceEdge {
                    edge_type: "CALLS".to_string(),
                    reason: None,
                    confidence: 1.0,
                },
                TraceEdge {
                    edge_type: "CALLS".to_string(),
                    reason: None,
                    confidence: 1.0,
                },
            ],
            depth: 2,
        }
    }

    #[test]
    fn apply_path_filter_include_files_keeps_only_matching() {
        let paths = vec![make_3node_path()];
        let filter = PathFilter {
            include_files: Some(vec!["/src/a.rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].nodes.len(), 1);
        assert_eq!(result[0].nodes[0].name, "a");
    }

    #[test]
    fn apply_path_filter_include_files_glob_matches_multiple() {
        let paths = vec![make_3node_path()];
        let filter = PathFilter {
            include_files: Some(vec!["/src/[ab].rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].nodes.len(), 2);
        assert_eq!(result[0].nodes[0].name, "a");
        assert_eq!(result[0].nodes[1].name, "b");
        // Consecutive nodes: edge preserved.
        assert_eq!(result[0].edges.len(), 1);
    }

    #[test]
    fn apply_path_filter_exclude_files_drops_matching() {
        let paths = vec![make_3node_path()];
        let filter = PathFilter {
            exclude_files: Some(vec!["/src/b.rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        // a and c kept; b dropped. a and c are not consecutive → no edges.
        assert_eq!(result[0].nodes.len(), 2);
        assert_eq!(result[0].nodes[0].name, "a");
        assert_eq!(result[0].nodes[1].name, "c");
        assert_eq!(result[0].edges.len(), 0);
    }

    #[test]
    fn apply_path_filter_include_modules_keeps_matching() {
        let paths = vec![TracePath {
            nodes: vec![
                make_node_with_path("a", "/src/module_a/foo.rs"),
                make_node_with_path("b", "/src/module_b/bar.rs"),
            ],
            edges: vec![TraceEdge {
                edge_type: "CALLS".to_string(),
                reason: None,
                confidence: 1.0,
            }],
            depth: 1,
        }];
        let filter = PathFilter {
            include_modules: Some(vec!["module_a".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].nodes.len(), 1);
        assert_eq!(result[0].nodes[0].name, "a");
    }

    #[test]
    fn apply_path_filter_symbol_pattern_keeps_matching() {
        let paths = vec![TracePath {
            nodes: vec![
                make_node_with_path("handler_create", "/src/a.rs"),
                make_node_with_path("do_work", "/src/b.rs"),
                make_node_with_path("handler_delete", "/src/c.rs"),
            ],
            edges: vec![
                TraceEdge {
                    edge_type: "CALLS".to_string(),
                    reason: None,
                    confidence: 1.0,
                },
                TraceEdge {
                    edge_type: "CALLS".to_string(),
                    reason: None,
                    confidence: 1.0,
                },
            ],
            depth: 2,
        }];
        let filter = PathFilter {
            symbol_pattern: Some("handler.*".to_string()),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].nodes.len(), 2);
        assert_eq!(result[0].nodes[0].name, "handler_create");
        assert_eq!(result[0].nodes[1].name, "handler_delete");
    }

    #[test]
    fn apply_path_filter_no_match_removes_path() {
        let paths = vec![make_3node_path()];
        let filter = PathFilter {
            include_files: Some(vec!["/nonexistent.rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert!(result.is_empty());
    }

    #[test]
    fn apply_path_filter_no_filter_returns_all_paths() {
        let paths = vec![make_3node_path()];
        let filter = PathFilter::default();
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].nodes.len(), 3);
    }

    #[test]
    fn apply_path_filter_multiple_paths() {
        let paths = vec![
            make_3node_path(),
            TracePath {
                nodes: vec![
                    make_node_with_path("x", "/src/a.rs"),
                    make_node_with_path("y", "/src/d.rs"),
                ],
                edges: vec![TraceEdge {
                    edge_type: "CALLS".to_string(),
                    reason: None,
                    confidence: 1.0,
                }],
                depth: 1,
            },
        ];
        let filter = PathFilter {
            include_files: Some(vec!["/src/a.rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 2);
        // First path: only "a" kept.
        assert_eq!(result[0].nodes.len(), 1);
        assert_eq!(result[0].nodes[0].name, "a");
        // Second path: only "x" kept.
        assert_eq!(result[1].nodes.len(), 1);
        assert_eq!(result[1].nodes[0].name, "x");
    }

    #[test]
    fn apply_path_filter_wildcard_glob_matches_any_file() {
        let paths = vec![make_3node_path()];
        let filter = PathFilter {
            include_files: Some(vec!["/src/*.rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].nodes.len(), 3);
    }

    #[test]
    fn apply_path_filter_node_without_file_path_excluded_by_include() {
        let paths = vec![TracePath {
            nodes: vec![
                TraceNode {
                    name: "no_file".to_string(),
                    label: "Function".to_string(),
                    file_path: None,
                    start_line: None,
                },
                make_node_with_path("a", "/src/a.rs"),
            ],
            edges: vec![TraceEdge {
                edge_type: "CALLS".to_string(),
                reason: None,
                confidence: 1.0,
            }],
            depth: 1,
        }];
        let filter = PathFilter {
            include_files: Some(vec!["/src/a.rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        // "no_file" dropped (no file_path), "a" kept.
        assert_eq!(result[0].nodes.len(), 1);
        assert_eq!(result[0].nodes[0].name, "a");
    }

    #[test]
    fn apply_path_filter_exclude_files_keeps_node_without_file_path() {
        let paths = vec![TracePath {
            nodes: vec![
                TraceNode {
                    name: "no_file".to_string(),
                    label: "Function".to_string(),
                    file_path: None,
                    start_line: None,
                },
                make_node_with_path("a", "/src/a.rs"),
            ],
            edges: vec![TraceEdge {
                edge_type: "CALLS".to_string(),
                reason: None,
                confidence: 1.0,
            }],
            depth: 1,
        }];
        let filter = PathFilter {
            exclude_files: Some(vec!["/src/a.rs".to_string()]),
            ..Default::default()
        };
        let result = apply_path_filter(paths, &filter);
        assert_eq!(result.len(), 1);
        // "no_file" kept (no file_path to exclude), "a" dropped.
        assert_eq!(result[0].nodes.len(), 1);
        assert_eq!(result[0].nodes[0].name, "no_file");
    }
}
