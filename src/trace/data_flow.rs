// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Data-flow tracer (trace/data_flow.rs) implementing PRD §4.2 / AC-TRACE-002.
//!
//! Provides [`DataFlowTracer`] for performing BFS traversal over `DataFlows`,
//! `Reads`, and `Writes` edges from a starting symbol, with a configurable
//! depth limit (AC-TRACE-004). Each traversal produces a list of [`TracePath`]s
//! recording the nodes and edges visited along the way.

use std::collections::VecDeque;

use crate::model::{EdgeType, Graph, NodeId};

use super::{TraceEdge, TraceNode, TracePath};

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

/// Internal BFS work item: tracks the chain of visited node ids alongside the
/// in-progress [`TracePath`] so cycles can be detected without storing an
/// `id` field on [`TraceNode`] itself.
struct WorkPath {
    visited_ids: Vec<NodeId>,
    path: TracePath,
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
        let Some(start_node) = self.graph.get_node(start_id) else {
            return Vec::new();
        };
        let mut queue: VecDeque<WorkPath> = VecDeque::new();
        queue.push_back(WorkPath {
            visited_ids: vec![start_id.clone()],
            path: TracePath {
                nodes: vec![TraceNode::from(start_node)],
                edges: Vec::new(),
                depth: 0,
            },
        });

        let mut results = Vec::new();

        while let Some(work) = queue.pop_front() {
            let has_edges = !work.path.edges.is_empty();
            let can_extend = work.path.depth < depth;

            if !can_extend {
                // Depth limit reached: record this path if it has edges.
                if has_edges {
                    results.push(work.path);
                }
                continue;
            }

            let current_id = work
                .visited_ids
                .last()
                .expect("work path always has at least one visited id")
                .clone();
            for edge in self.graph.edges_from(&current_id) {
                if !matches!(
                    edge.edge_type,
                    EdgeType::DataFlows | EdgeType::Reads | EdgeType::Writes
                ) {
                    continue;
                }
                let Some(target_node) = self.graph.get_node(&edge.target) else {
                    continue;
                };
                // Cycle prevention: skip targets already on this path.
                if work.visited_ids.contains(&edge.target) {
                    continue;
                }
                let mut new_visited = work.visited_ids.clone();
                new_visited.push(edge.target.clone());
                let mut new_path = work.path.clone();
                new_path.nodes.push(TraceNode::from(target_node));
                new_path.edges.push(TraceEdge {
                    edge_type: edge.edge_type.to_string(),
                    reason: edge.reason.clone(),
                    confidence: edge.confidence,
                });
                new_path.depth = work.path.depth + 1;
                queue.push_back(WorkPath {
                    visited_ids: new_visited,
                    path: new_path,
                });
            }

            // Record the current path itself (every prefix is a valid path).
            if has_edges {
                results.push(work.path);
            }
        }

        results
    }
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
}
