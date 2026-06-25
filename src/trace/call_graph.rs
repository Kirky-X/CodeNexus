//! Call-graph tracer (trace/call_graph.rs) implementing PRD §4.2 / AC-TRACE-001.
//!
//! Provides [`CallGraphTracer`] for performing BFS traversal over `Calls` and
//! `FfiCalls` edges from a starting symbol, with a configurable depth limit
//! (AC-TRACE-004). Each traversal produces a list of [`TracePath`]s recording
//! the nodes and edges visited along the way.

use std::collections::VecDeque;

use crate::model::{EdgeType, Graph, NodeId};

use super::{TraceEdge, TraceNode, TracePath};

/// BFS tracer over `Calls` / `FfiCalls` edges (PRD §4.2, AC-TRACE-001/003/004).
///
/// Holds an immutable borrow of the [`Graph`] and exposes [`trace`] which
/// returns every path reachable from `start_id` within `depth` hops.
///
/// [`trace`]: CallGraphTracer::trace
pub struct CallGraphTracer<'a> {
    graph: &'a Graph,
}

/// Internal BFS work item: tracks the chain of visited node ids alongside the
/// in-progress [`TracePath`] so cycles can be detected without storing an
/// `id` field on [`TraceNode`] itself.
struct WorkPath {
    visited_ids: Vec<NodeId>,
    path: TracePath,
}

impl<'a> CallGraphTracer<'a> {
    /// Creates a new `CallGraphTracer` bound to the given graph.
    #[must_use]
    pub fn new(graph: &'a Graph) -> Self {
        Self { graph }
    }

    /// Performs a BFS traversal from `start_id` over `Calls` and `FfiCalls`
    /// edges, returning all paths whose length (in edges) does not exceed
    /// `depth`.
    ///
    /// A path of depth `n` contains `n + 1` nodes and `n` edges. The starting
    /// node is always included as the first node of every path. Cycles are
    /// handled by tracking visited nodes per-path so traversal terminates.
    ///
    /// Returns an empty vector if `start_id` is not in the graph or has no
    /// outgoing call edges.
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
                if !matches!(edge.edge_type, EdgeType::Calls | EdgeType::FfiCalls) {
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

    fn make_func_no_loc(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Function, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .build()
    }

    fn graph_a_calls_b() -> Graph {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g
    }

    fn graph_a_calls_b_calls_c() -> Graph {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g
    }

    #[test]
    fn trace_a_returns_path_a_to_b() {
        // AC-TRACE-001: A calls B -> trace A returns path A->B.
        let g = graph_a_calls_b();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert_eq!(path.nodes.len(), 2);
        assert_eq!(path.nodes[0].name, "a");
        assert_eq!(path.nodes[1].name, "b");
        assert_eq!(path.edges.len(), 1);
        assert_eq!(path.edges[0].edge_type, "CALLS");
        assert_eq!(path.depth, 1);
    }

    #[test]
    fn trace_a_depth_2_returns_two_paths() {
        // A calls B, B calls C -> trace A depth 2 returns paths A->B, A->B->C.
        let g = graph_a_calls_b_calls_c();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 2);
        assert_eq!(paths.len(), 2);
        let depths: Vec<usize> = paths.iter().map(|p| p.depth).collect();
        assert!(depths.contains(&1));
        assert!(depths.contains(&2));
        let deep = paths.iter().find(|p| p.depth == 2).unwrap();
        assert_eq!(deep.nodes.len(), 3);
        assert_eq!(deep.nodes[0].name, "a");
        assert_eq!(deep.nodes[1].name, "b");
        assert_eq!(deep.nodes[2].name, "c");
    }

    #[test]
    fn trace_a_depth_1_returns_only_a_to_b() {
        // AC-TRACE-004: A calls B, B calls C -> trace A depth 1 returns only A->B.
        let g = graph_a_calls_b_calls_c();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 1);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].depth, 1);
        assert_eq!(paths[0].nodes.len(), 2);
        assert_eq!(paths[0].nodes[1].name, "b");
    }

    #[test]
    fn trace_respects_depth_limit() {
        // AC-TRACE-004: depth limit respected — no path exceeds depth.
        let g = graph_a_calls_b_calls_c();
        let tracer = CallGraphTracer::new(&g);
        let depth = 1;
        let paths = tracer.trace(&"a".to_string(), depth);
        for p in &paths {
            assert!(p.depth <= depth);
        }
    }

    #[test]
    fn trace_ffi_calls_returns_path_with_ffi_edge() {
        // AC-TRACE-003 (call-graph portion): A ffi_calls B -> path with FfiCalls.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(
            Edge::builder("a", "b", EdgeType::FfiCalls, "proj")
                .confidence(0.85)
                .reason("extern \"C\" declaration match")
                .build(),
        );
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert_eq!(path.edges.len(), 1);
        assert_eq!(path.edges[0].edge_type, "FFI_CALLS");
        assert!((path.edges[0].confidence - 0.85).abs() < f32::EPSILON);
        assert_eq!(
            path.edges[0].reason.as_deref(),
            Some("extern \"C\" declaration match")
        );
    }

    #[test]
    fn trace_no_outgoing_calls_returns_empty() {
        // No outgoing calls -> empty paths.
        let g = graph_a_calls_b();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"b".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_missing_start_node_returns_empty() {
        let g = graph_a_calls_b();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"missing".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_cyclic_calls_does_not_infinite_loop() {
        // A -> B -> A cycle: must terminate and return paths within depth.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 5);
        assert!(!paths.is_empty());
        for p in &paths {
            let mut names: Vec<&str> = p.nodes.iter().map(|n| n.name.as_str()).collect();
            let len_before = names.len();
            names.sort();
            names.dedup();
            assert_eq!(
                names.len(),
                len_before,
                "path revisits a node: {:?}",
                p.nodes
            );
            assert!(p.depth <= 5);
        }
    }

    #[test]
    fn trace_cyclic_returns_paths_within_depth() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        for p in &paths {
            assert!(p.depth <= 3);
        }
        assert!(paths.iter().any(|p| p.depth == 1));
        assert!(paths.iter().any(|p| p.depth == 2));
    }

    #[test]
    fn trace_node_includes_location_info() {
        let g = graph_a_calls_b();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        let path = &paths[0];
        assert_eq!(path.nodes[0].file_path.as_deref(), Some("src/a.rs"));
        assert_eq!(path.nodes[0].start_line, Some(10));
        assert_eq!(path.nodes[0].label, "Function");
    }

    #[test]
    fn trace_node_without_location_has_none() {
        let mut g = Graph::new();
        g.add_node(make_func_no_loc("a", "a"));
        g.add_node(make_func_no_loc("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].nodes[0].file_path.is_none());
        assert!(paths[0].nodes[0].start_line.is_none());
    }

    #[test]
    fn trace_zero_depth_returns_empty() {
        // depth 0 means no hops allowed, so no paths with edges.
        let g = graph_a_calls_b();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 0);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_skips_non_call_edges() {
        // READS edges should not be followed by the call-graph tracer.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Reads, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_branching_graph_returns_all_paths() {
        // A -> B, A -> C, B -> D, C -> D
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "d", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "d", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        // Expect: A->B, A->C, A->B->D, A->C->D = 4 paths.
        assert_eq!(paths.len(), 4);
        let depth2_count = paths.iter().filter(|p| p.depth == 2).count();
        assert_eq!(depth2_count, 2);
        let depth1_count = paths.iter().filter(|p| p.depth == 1).count();
        assert_eq!(depth1_count, 2);
    }

    #[test]
    fn trace_skips_edges_to_missing_nodes() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        // Edge to "b" which is not in the graph.
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_mixed_calls_and_ffi_calls() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::FfiCalls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        // Expect A->B and A->B->C (with FFI_CALLS edge).
        assert_eq!(paths.len(), 2);
        let deep = paths.iter().find(|p| p.depth == 2).unwrap();
        assert_eq!(deep.edges[1].edge_type, "FFI_CALLS");
    }
}
