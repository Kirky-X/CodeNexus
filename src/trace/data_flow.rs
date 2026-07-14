// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Data-flow tracer (trace/data_flow.rs) implementing PRD §4.2 / AC-TRACE-002.
//!
//! Provides [`DataFlowTracer`] for performing BFS traversal over `DataFlows`,
//! `Reads`, and `Writes` edges from a starting symbol, with a configurable
//! depth limit (AC-TRACE-004). Each traversal produces a list of [`TracePath`]s
//! recording the nodes and edges visited along the way.

use crate::model::{EdgeType, Graph, NodeId};

use super::bfs::bfs_trace;
use super::TracePath;

/// BFS tracer over `DataFlows` / `Reads` / `Writes` edges (PRD §4.2,
/// AC-TRACE-002/004, BR-TRACE-001~006).
///
/// Holds an immutable borrow of the [`Graph`] and exposes [`trace`] which
/// returns every path reachable from `start_id` within `depth` hops.
///
/// [`trace`]: DataFlowTracer::trace
pub struct DataFlowTracer<'a> {
    graph: &'a Graph,
}

impl<'a> DataFlowTracer<'a> {
    /// Creates a new `DataFlowTracer` bound to the given graph.
    #[must_use]
    pub fn new(graph: &'a Graph) -> Self {
        Self { graph }
    }

    /// Performs a BFS traversal from `start_id` over `DataFlows`, `Reads`, and
    /// `Writes` edges, returning all paths whose length (in edges) does not
    /// exceed `depth`.
    ///
    /// A path of depth `n` contains `n + 1` nodes and `n` edges. The starting
    /// node is always included as the first node of every path. Cycles are
    /// handled by tracking visited nodes per-path so traversal terminates.
    ///
    /// Returns an empty vector if `start_id` is not in the graph or has no
    /// outgoing data-flow edges.
    pub fn trace(&self, start_id: &NodeId, depth: usize) -> Vec<TracePath> {
        bfs_trace(self.graph, start_id, depth, is_dataflow_edge, None)
    }
}

/// Returns `true` if the edge type is a data-flow edge.
#[inline]
fn is_dataflow_edge(edge_type: &EdgeType) -> bool {
    matches!(
        edge_type,
        EdgeType::DataFlows | EdgeType::Reads | EdgeType::Writes
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, Node, NodeLabel};

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

    fn make_param(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Parameter, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .build()
    }

    #[test]
    fn trace_dataflows_returns_path() {
        // AC-TRACE-002 (dataflow portion): x dataflows to y -> path x->y.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 3);
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert_eq!(path.nodes.len(), 2);
        assert_eq!(path.nodes[0].name, "x");
        assert_eq!(path.nodes[1].name, "y");
        assert_eq!(path.edges.len(), 1);
        assert_eq!(path.edges[0].edge_type, "DATAFLOWS");
        assert_eq!(path.depth, 1);
    }

    #[test]
    fn trace_reads_edge_included() {
        // Function reads variable -> trace includes READS edge.
        let mut g = Graph::new();
        g.add_node(make_func("foo", "foo"));
        g.add_node(make_var("v", "v"));
        g.add_edge(Edge::new("foo", "v", EdgeType::Reads, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"foo".to_string(), 3);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].edges[0].edge_type, "READS");
        assert_eq!(paths[0].nodes[0].name, "foo");
        assert_eq!(paths[0].nodes[1].name, "v");
    }

    #[test]
    fn trace_writes_edge_included() {
        // Function writes variable -> trace includes WRITES edge.
        let mut g = Graph::new();
        g.add_node(make_func("foo", "foo"));
        g.add_node(make_var("v", "v"));
        g.add_edge(Edge::new("foo", "v", EdgeType::Writes, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"foo".to_string(), 3);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].edges[0].edge_type, "WRITES");
    }

    #[test]
    fn trace_depth_limit_respected() {
        // AC-TRACE-004: depth limit respected.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_node(make_var("z", "z"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("y", "z", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 1);
        // Only x->y should be returned (depth 1).
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].depth, 1);
        assert_eq!(paths[0].nodes.last().unwrap().name, "y");
    }

    #[test]
    fn trace_depth_2_returns_two_paths() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_node(make_var("z", "z"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("y", "z", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 2);
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|p| p.depth == 1));
        assert!(paths.iter().any(|p| p.depth == 2));
    }

    #[test]
    fn trace_no_outgoing_dataflow_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"y".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_missing_start_node_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"missing".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_skips_non_dataflow_edges() {
        // CALLS edges should not be followed by the data-flow tracer.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_zero_depth_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 0);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_cyclic_dataflow_terminates() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("y", "x", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 5);
        assert!(!paths.is_empty());
        for p in &paths {
            assert!(p.depth <= 5);
            // No node revisited.
            let mut names: Vec<&str> = p.nodes.iter().map(|n| n.name.as_str()).collect();
            let len_before = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), len_before);
        }
    }

    #[test]
    fn trace_mixed_dataflow_reads_writes() {
        // foo reads v1, foo writes v2, v2 dataflows to v3
        let mut g = Graph::new();
        g.add_node(make_func("foo", "foo"));
        g.add_node(make_var("v1", "v1"));
        g.add_node(make_var("v2", "v2"));
        g.add_node(make_var("v3", "v3"));
        g.add_edge(Edge::new("foo", "v1", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("foo", "v2", EdgeType::Writes, "proj"));
        g.add_edge(Edge::new("v2", "v3", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"foo".to_string(), 3);
        // foo->v1 (READS), foo->v2 (WRITES), foo->v2->v3 (WRITES + DATAFLOWS)
        assert_eq!(paths.len(), 3);
        let edge_types: Vec<&str> = paths
            .iter()
            .filter(|p| p.depth == 1)
            .map(|p| p.edges[0].edge_type.as_str())
            .collect();
        assert!(edge_types.contains(&"READS"));
        assert!(edge_types.contains(&"WRITES"));
    }

    #[test]
    fn trace_param_dataflow() {
        // BR-TRACE-001: var -> param dataflow
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_param("p", "p"));
        g.add_edge(Edge::new("x", "p", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 3);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].nodes[0].label, "Variable");
        assert_eq!(paths[0].nodes[1].label, "Parameter");
    }

    #[test]
    fn trace_skips_edges_to_missing_nodes() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_edge(Edge::new("x", "missing", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_carries_reason_and_confidence() {
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(
            Edge::builder("x", "y", EdgeType::DataFlows, "proj")
                .confidence(0.9)
                .reason("assignment: y = x")
                .build(),
        );
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 3);
        assert_eq!(paths.len(), 1);
        let edge = &paths[0].edges[0];
        assert!((edge.confidence - 0.9).abs() < f32::EPSILON);
        assert_eq!(edge.reason.as_deref(), Some("assignment: y = x"));
    }

    #[test]
    fn trace_self_loop_dataflow_returns_empty() {
        // x -> x DataFlows: cycle prevention skips self, so no paths.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_edge(Edge::new("x", "x", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 3);
        assert!(
            paths.is_empty(),
            "self-loop should be skipped by cycle prevention"
        );
    }

    #[test]
    fn trace_diamond_graph_returns_all_four_paths() {
        // x -> y, x -> z, y -> w, z -> w (diamond).
        // Expect 4 paths: x->y, x->z, x->y->w, x->z->w.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_node(make_var("z", "z"));
        g.add_node(make_var("w", "w"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("x", "z", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("y", "w", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("z", "w", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 5);
        assert_eq!(paths.len(), 4, "diamond should yield 4 paths");
        assert_eq!(paths.iter().filter(|p| p.depth == 1).count(), 2);
        assert_eq!(paths.iter().filter(|p| p.depth == 2).count(), 2);
        // Both depth-2 paths should end at w.
        for p in paths.iter().filter(|p| p.depth == 2) {
            assert_eq!(p.nodes.last().unwrap().name, "w");
        }
    }

    #[test]
    fn trace_depth_far_exceeding_graph_diameter_terminates() {
        // x -> y, depth 100: should return 1 path (x->y) without infinite loop.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 100);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].depth, 1);
    }

    #[test]
    fn trace_mixed_calls_and_dataflows_follows_only_dataflow() {
        // Node a has both a Calls edge (skipped, line 95) and a DataFlows edge
        // (followed). Verifies the edge-type filter keeps dataflow edges while
        // dropping non-dataflow edges in the same iteration.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_var("v", "v"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "v", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        // Only a->v (DataFlows) should be returned; a->b (Calls) is skipped.
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].edges[0].edge_type, "DATAFLOWS");
        assert_eq!(paths[0].nodes[0].name, "a");
        assert_eq!(paths[0].nodes[1].name, "v");
    }

    #[test]
    fn trace_depth_limit_records_path_with_edges_explicitly() {
        // Explicitly covers line 80: depth limit reached on a path that HAS
        // edges, so the path is pushed to results before `continue`.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_node(make_var("z", "z"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("y", "z", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 2);
        // depth=2: x->y (depth 1), x->y->z (depth 2, hits limit with edges).
        assert_eq!(paths.len(), 2);
        let max_depth_path = paths.iter().max_by_key(|p| p.depth).unwrap();
        assert_eq!(max_depth_path.depth, 2);
        assert_eq!(max_depth_path.nodes.last().unwrap().name, "z");
        assert!(!max_depth_path.edges.is_empty());
    }

    #[test]
    fn trace_cycle_to_intermediate_node_skipped() {
        // a -> b -> c -> a (cycle back to start). Cycle prevention (line 104)
        // skips the edge back to a, but valid paths a->b, a->b->c are returned.
        let mut g = Graph::new();
        g.add_node(make_var("a", "a"));
        g.add_node(make_var("b", "b"));
        g.add_node(make_var("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 5);
        // a->b (depth 1), a->b->c (depth 2). c->a is skipped (cycle).
        assert_eq!(paths.len(), 2);
        for p in &paths {
            let names: Vec<&str> = p.nodes.iter().map(|n| n.name.as_str()).collect();
            let mut sorted = names.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(names.len(), sorted.len(), "no revisited nodes in path");
        }
    }

    #[test]
    fn trace_zero_depth_with_edge_skips_extension() {
        // depth=0: initial path has no edges (has_edges=false), can_extend is
        // false (0 < 0), so we `continue` without pushing to results (line 80
        // with has_edges=false branch).
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 0);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_initial_path_depth_is_zero() {
        // Verifies line 64: the initial WorkPath has depth: 0. By tracing a
        // node with no outgoing dataflow edges, the initial path (depth 0, no
        // edges) is popped but not recorded (has_edges=false), confirming the
        // initial depth field is set to 0.
        let mut g = Graph::new();
        g.add_node(make_var("solo", "solo"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"solo".to_string(), 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_node_includes_location_info() {
        // Verifies TraceNode::from(&Node) carries file_path and start_line
        // from nodes that have location info (make_func sets both).
        let mut g = Graph::new();
        g.add_node(make_func("foo", "foo"));
        g.add_node(make_func("bar", "bar"));
        g.add_edge(Edge::new("foo", "bar", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"foo".to_string(), 3);
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert_eq!(path.nodes[0].file_path.as_deref(), Some("src/foo.rs"));
        assert_eq!(path.nodes[0].start_line, Some(10));
        assert_eq!(path.nodes[1].file_path.as_deref(), Some("src/bar.rs"));
        assert_eq!(path.nodes[1].start_line, Some(10));
    }

    #[test]
    fn trace_node_without_location_has_none() {
        // Verifies TraceNode::from(&Node) yields None for file_path and
        // start_line when the source node has no location info (make_var
        // sets neither).
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 3);
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert!(path.nodes[0].file_path.is_none());
        assert!(path.nodes[0].start_line.is_none());
        assert!(path.nodes[1].file_path.is_none());
        assert!(path.nodes[1].start_line.is_none());
    }

    #[test]
    fn trace_empty_graph_returns_empty() {
        // No nodes at all → get_node returns None → empty result.
        let g = Graph::new();
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"any".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_dataflow_with_reason_and_confidence_default_values() {
        // Edge created with Edge::new (no builder) has confidence=1.0 and
        // reason=None. Verifies the default edge fields are carried through
        // to TraceEdge without modification.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"x".to_string(), 3);
        assert_eq!(paths.len(), 1);
        let edge = &paths[0].edges[0];
        assert!((edge.confidence - 1.0).abs() < f32::EPSILON);
        assert!(edge.reason.is_none());
    }

    #[test]
    fn trace_ffi_calls_edge_not_followed() {
        // FfiCalls edges should NOT be followed by the data-flow tracer.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::FfiCalls, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert!(paths.is_empty(), "FfiCalls should not be followed by data-flow tracer");
    }

    #[test]
    fn trace_multiple_writes_from_same_node() {
        // Node with multiple outgoing Writes edges: each produces a path.
        let mut g = Graph::new();
        g.add_node(make_func("foo", "foo"));
        g.add_node(make_var("v1", "v1"));
        g.add_node(make_var("v2", "v2"));
        g.add_node(make_var("v3", "v3"));
        g.add_edge(Edge::new("foo", "v1", EdgeType::Writes, "proj"));
        g.add_edge(Edge::new("foo", "v2", EdgeType::Writes, "proj"));
        g.add_edge(Edge::new("foo", "v3", EdgeType::Writes, "proj"));
        let tracer = DataFlowTracer::new(&g);
        let paths = tracer.trace(&"foo".to_string(), 3);
        assert_eq!(paths.len(), 3);
        for p in &paths {
            assert_eq!(p.depth, 1);
            assert_eq!(p.edges[0].edge_type, "WRITES");
        }
    }
}
