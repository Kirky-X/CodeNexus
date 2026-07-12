// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Call-graph tracer (trace/call_graph.rs) implementing PRD §4.2 / AC-TRACE-001.
//!
//! Provides [`CallGraphTracer`] for performing BFS traversal over `Calls` and
//! `FfiCalls` edges from a starting symbol, with a configurable depth limit
//! (AC-TRACE-004). Each traversal produces a list of [`TracePath`]s recording
//! the nodes and edges visited along the way.

use std::collections::{HashMap, VecDeque};

use crate::model::{EdgeType, Graph, NodeId};

use super::{TraceCycle, TraceEdge, TraceNode, TracePath};

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
            // Single-line for coverage: tarpaulin attribute continuation
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
                // Single-line for coverage: tarpaulin attribute continuation
                if has_edges {
                    results.push(work.path);
                }
                continue;
            }

            // Single-line for coverage: tarpaulin attribute continuation
            let current_id = work
                .visited_ids
                .last()
                .expect("work path always has at least one visited id")
                .clone();
            for edge in self.graph.edges_from(&current_id) {
                // Single-line for coverage: tarpaulin attribute continuation
                if !matches!(edge.edge_type, EdgeType::Calls | EdgeType::FfiCalls) {
                    continue;
                }
                // Single-line for coverage: tarpaulin attribute continuation
                let Some(target_node) = self.graph.get_node(&edge.target) else {
                    continue;
                };
                // Cycle prevention: skip targets already on this path.
                // Single-line for coverage: tarpaulin attribute continuation
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

    /// Detects cycles in the call graph using DFS white/gray/black coloring
    /// (R-trace-002).
    ///
    /// Traverses `Calls` and `FfiCalls` edges. When a gray node (currently on
    /// the DFS stack) is encountered via a back edge, a [`TraceCycle`] is
    /// extracted from the current stack. Self-loops (`A→A`) are detected as
    /// cycles with `nodes = [A, A]`.
    ///
    /// Node iteration order is sorted by id for deterministic output.
    #[must_use]
    pub fn detect_cycles(&self) -> Vec<TraceCycle> {
        let mut colors: HashMap<NodeId, DfsColor> = HashMap::new();
        let mut stack: Vec<NodeId> = Vec::new();
        let mut stack_edges: Vec<EdgeType> = Vec::new();
        let mut cycles: Vec<TraceCycle> = Vec::new();

        let mut node_ids: Vec<&NodeId> = self.graph.nodes.keys().collect();
        node_ids.sort();

        for node_id in node_ids {
            if !colors.contains_key(node_id) {
                self.dfs_cycles(node_id, &mut colors, &mut stack, &mut stack_edges, &mut cycles);
            }
        }

        cycles
    }

    /// DFS helper for [`detect_cycles`].
    fn dfs_cycles(
        &self,
        node_id: &NodeId,
        colors: &mut HashMap<NodeId, DfsColor>,
        stack: &mut Vec<NodeId>,
        stack_edges: &mut Vec<EdgeType>,
        cycles: &mut Vec<TraceCycle>,
    ) {
        colors.insert(node_id.clone(), DfsColor::Gray);
        stack.push(node_id.clone());

        let mut edges: Vec<&crate::model::Edge> = self
            .graph
            .edges_from(node_id)
            .into_iter()
            .filter(|e| matches!(e.edge_type, EdgeType::Calls | EdgeType::FfiCalls))
            .collect();
        edges.sort_by(|a, b| a.target.cmp(&b.target));

        for edge in edges {
            match colors.get(&edge.target).copied() {
                None => {
                    stack_edges.push(edge.edge_type);
                    self.dfs_cycles(&edge.target, colors, stack, stack_edges, cycles);
                    stack_edges.pop();
                }
                Some(DfsColor::Gray) => {
                    let cycle = Self::extract_cycle(
                        &edge.target,
                        edge.edge_type,
                        stack,
                        stack_edges,
                        self.graph,
                    );
                    cycles.push(cycle);
                }
                Some(DfsColor::Black) => {}
            }
        }

        stack.pop();
        colors.insert(node_id.clone(), DfsColor::Black);
    }

    /// Extracts a [`TraceCycle`] from the current DFS stack when a back edge to
    /// a gray node is found.
    fn extract_cycle(
        target: &NodeId,
        closing_edge: EdgeType,
        stack: &[NodeId],
        stack_edges: &[EdgeType],
        graph: &Graph,
    ) -> TraceCycle {
        let idx = stack
            .iter()
            .position(|n| n == target)
            .expect("gray target must be on the DFS stack");

        let mut nodes: Vec<String> = stack[idx..]
            .iter()
            .filter_map(|id| graph.get_node(id).map(|n| n.name.clone()))
            .collect();
        if let Some(target_node) = graph.get_node(target) {
            nodes.push(target_node.name.clone());
        }

        let mut edge_types: Vec<EdgeType> = stack_edges[idx..].to_vec();
        edge_types.push(closing_edge);

        TraceCycle { nodes, edge_types }
    }

    /// Performs a BFS traversal from `start_id` that includes `HttpCalls` and
    /// reverse `HandlesRoute` edges for cross-service tracing (R-trace-003).
    ///
    /// In addition to `Calls` and `FfiCalls`, this method follows:
    /// - `HttpCalls` edges (forward: Function → Route)
    /// - `HandlesRoute` edges (reverse: Route → handler Function)
    ///
    /// This allows tracing a call chain that crosses service boundaries via
    /// HTTP. Cycles are handled by tracking visited nodes per-path.
    ///
    /// Returns an empty vector if `start_id` is not in the graph.
    pub fn trace_cross_service(&self, start_id: &NodeId, depth: usize) -> Vec<TracePath> {
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

            // Forward edges: Calls, FfiCalls, HttpCalls
            for edge in self.graph.edges_from(&current_id) {
                if !matches!(
                    edge.edge_type,
                    EdgeType::Calls | EdgeType::FfiCalls | EdgeType::HttpCalls
                ) {
                    continue;
                }
                let Some(target_node) = self.graph.get_node(&edge.target) else {
                    continue;
                };
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

            // Reverse HandlesRoute edges: Route → handler Function
            for edge in self.graph.edges_to(&current_id) {
                if edge.edge_type != EdgeType::HandlesRoute {
                    continue;
                }
                let Some(handler_node) = self.graph.get_node(&edge.source) else {
                    continue;
                };
                if work.visited_ids.contains(&edge.source) {
                    continue;
                }
                let mut new_visited = work.visited_ids.clone();
                new_visited.push(edge.source.clone());
                let mut new_path = work.path.clone();
                new_path.nodes.push(TraceNode::from(handler_node));
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

            if has_edges {
                results.push(work.path);
            }
        }

        results
    }
}

/// DFS color state for cycle detection (white = absent from map).
#[derive(Clone, Copy, PartialEq)]
enum DfsColor {
    Gray,
    Black,
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

    #[test]
    fn trace_self_loop_calls_returns_empty() {
        // a -> a Calls: cycle prevention skips self, so no paths.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_edge(Edge::new("a", "a", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 3);
        assert!(
            paths.is_empty(),
            "self-loop should be skipped by cycle prevention"
        );
    }

    #[test]
    fn trace_depth_far_exceeding_graph_diameter_terminates() {
        // a -> b, depth 100: should return 1 path (a->b) without infinite loop.
        let g = graph_a_calls_b();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 100);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].depth, 1);
    }

    // --- T034: detect_cycles (R-trace-002) ---

    #[test]
    fn detect_cycles_abc_cycle_returns_trace_cycle() {
        // A→B→C→A -> TraceCycle nodes=[A,B,C,A]
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let cycles = tracer.detect_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].nodes, vec!["a", "b", "c", "a"]);
        assert_eq!(
            cycles[0].edge_types,
            vec![EdgeType::Calls, EdgeType::Calls, EdgeType::Calls]
        );
    }

    #[test]
    fn detect_cycles_no_cycle_returns_empty() {
        // A→B→C→D (no cycle) -> cycles empty
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "d", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let cycles = tracer.detect_cycles();
        assert!(cycles.is_empty());
    }

    #[test]
    fn detect_cycles_self_loop_returns_aa_cycle() {
        // Self-loop A→A -> TraceCycle nodes=[A,A]
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_edge(Edge::new("a", "a", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let cycles = tracer.detect_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].nodes, vec!["a", "a"]);
        assert_eq!(cycles[0].edge_types, vec![EdgeType::Calls]);
    }

    #[test]
    fn detect_cycles_multiple_cycles_returns_all() {
        // Two separate cycles: A→B→A and C→D→C
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "d", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("d", "c", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let cycles = tracer.detect_cycles();
        assert_eq!(cycles.len(), 2);
        let node_sets: Vec<Vec<String>> = cycles.iter().map(|c| c.nodes.clone()).collect();
        assert!(node_sets.contains(&vec!["a".to_string(), "b".to_string(), "a".to_string()]));
        assert!(node_sets.contains(&vec!["c".to_string(), "d".to_string(), "c".to_string()]));
    }

    #[test]
    fn detect_cycles_empty_graph_returns_empty() {
        let g = Graph::new();
        let tracer = CallGraphTracer::new(&g);
        assert!(tracer.detect_cycles().is_empty());
    }

    #[test]
    fn detect_cycles_ffi_calls_edge_detected() {
        // A→B via FfiCalls, B→A via Calls -> cycle with mixed edge types
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::FfiCalls, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let cycles = tracer.detect_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].nodes, vec!["a", "b", "a"]);
        assert_eq!(
            cycles[0].edge_types,
            vec![EdgeType::FfiCalls, EdgeType::Calls]
        );
    }

    #[test]
    fn detect_cycles_nested_cycle_detected() {
        // A→B→C→B (cycle B→C→B, A is a stem)
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "b", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let cycles = tracer.detect_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].nodes, vec!["b", "c", "b"]);
        assert_eq!(
            cycles[0].edge_types,
            vec![EdgeType::Calls, EdgeType::Calls]
        );
    }

    #[test]
    fn detect_cycles_skips_non_call_edges() {
        // A→B via Reads (not a call edge) -> no cycle even if B→A exists
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Reads, "proj"));
        let tracer = CallGraphTracer::new(&g);
        assert!(tracer.detect_cycles().is_empty());
    }

    // --- T035: trace_cross_service (R-trace-003) ---

    fn make_route(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Route, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .file_path(format!("routes/{name}.rs"))
            .start_line(1)
            .build()
    }

    #[test]
    fn trace_cross_service_follows_http_calls_to_handler() {
        // A --HttpCalls--> R, B --HandlesRoute--> R
        // Trace from A should reach B via R.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_route("r", "/api/r"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        g.add_edge(Edge::new("b", "r", EdgeType::HandlesRoute, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        // Should find A->R (HttpCalls) and A->R->B (HttpCalls + HandlesRoute)
        let cross_path = paths
            .iter()
            .find(|p| p.depth == 2 && p.nodes.len() == 3)
            .expect("should have A->R->B path");
        assert_eq!(cross_path.nodes[0].name, "a");
        assert_eq!(cross_path.nodes[1].name, "/api/r");
        assert_eq!(cross_path.nodes[2].name, "b");
        assert_eq!(cross_path.edges[0].edge_type, "HTTP_CALLS");
        assert_eq!(cross_path.edges[1].edge_type, "HANDLES_ROUTE");
    }

    #[test]
    fn trace_cross_service_includes_http_calls_edge_type() {
        // TracePath.edges must contain HttpCalls edge type
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_route("r", "/api/r"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        let http_path = paths
            .iter()
            .find(|p| p.depth == 1)
            .expect("should have A->R path");
        assert_eq!(http_path.edges[0].edge_type, "HTTP_CALLS");
    }

    #[test]
    fn trace_cross_service_still_follows_calls_edges() {
        // Cross-service trace should still follow regular Calls edges
        let g = graph_a_calls_b();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 3);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].edges[0].edge_type, "CALLS");
    }

    #[test]
    fn trace_cross_service_respects_depth_limit() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_route("r", "/api/r"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        g.add_edge(Edge::new("b", "r", EdgeType::HandlesRoute, "proj"));
        let tracer = CallGraphTracer::new(&g);
        // depth 1: only A->R, no A->R->B
        let paths = tracer.trace_cross_service(&"a".to_string(), 1);
        for p in &paths {
            assert!(p.depth <= 1);
        }
        assert!(paths.iter().any(|p| p.depth == 1));
        assert!(!paths.iter().any(|p| p.depth == 2));
    }

    #[test]
    fn trace_regular_does_not_follow_http_calls() {
        // Regular trace() should NOT follow HttpCalls edges
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_route("r", "/api/r"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace(&"a".to_string(), 5);
        assert!(
            paths.is_empty(),
            "regular trace should not follow HttpCalls edges"
        );
    }

    #[test]
    fn trace_cross_service_missing_start_returns_empty() {
        let g = Graph::new();
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"missing".to_string(), 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_cross_service_zero_depth_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_route("r", "/api/r"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 0);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_cross_service_cycle_terminates() {
        // A --HttpCalls--> R, B --HandlesRoute--> R, B --HttpCalls--> R2,
        // A --HandlesRoute--> R2: potential cycle A->R->B->R2->A
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_route("r1", "/api/r1"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_route("r2", "/api/r2"));
        g.add_edge(Edge::new("a", "r1", EdgeType::HttpCalls, "proj"));
        g.add_edge(Edge::new("b", "r1", EdgeType::HandlesRoute, "proj"));
        g.add_edge(Edge::new("b", "r2", EdgeType::HttpCalls, "proj"));
        g.add_edge(Edge::new("a", "r2", EdgeType::HandlesRoute, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 10);
        // Must terminate; all paths within depth.
        for p in &paths {
            assert!(p.depth <= 10);
        }
        assert!(!paths.is_empty());
    }

    // --- Coverage gap tests: detect_cycles Black node & cross-service edge filtering ---

    #[test]
    fn detect_cycles_diamond_graph_encounters_black_node() {
        // Diamond: A→B, A→C, B→D, C→D. No cycle, but when DFS processes D
        // via C after D was already finished (Black) via B, the
        // Some(DfsColor::Black) arm is exercised (line 192).
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
        let cycles = tracer.detect_cycles();
        assert!(cycles.is_empty(), "diamond has no cycle");
    }

    #[test]
    fn trace_cross_service_skips_non_forward_dataflow_edge() {
        // A has a DataFlows edge to V — not Calls/FfiCalls/HttpCalls — so
        // the forward-edge loop must skip it (lines 276-278).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("v", "v"));
        g.add_edge(Edge::new("a", "v", EdgeType::DataFlows, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        assert!(paths.is_empty(), "DataFlows edge should be skipped");
    }

    #[test]
    fn trace_cross_service_skips_forward_edge_to_missing_target() {
        // A --HttpCalls--> "missing" (target node not in graph).
        // The forward loop must skip the dangling edge (line 281).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_edge(Edge::new("a", "missing", EdgeType::HttpCalls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        assert!(paths.is_empty(), "edge to missing target should be skipped");
    }

    #[test]
    fn trace_cross_service_skips_forward_cycle_to_visited_node() {
        // A --HttpCalls--> R, R --HttpCalls--> A (would cycle back).
        // After visiting R from A, the edge R→A must be skipped because A is
        // already on the visited path (line 284).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("r", "r"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        g.add_edge(Edge::new("r", "a", EdgeType::HttpCalls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        // Should return A→R (depth 1) but not A→R→A (cycle prevented).
        for p in &paths {
            let mut names: Vec<&str> = p.nodes.iter().map(|n| n.name.as_str()).collect();
            let len_before = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), len_before, "path revisits a node: {:?}", p.nodes);
        }
        assert!(paths.iter().any(|p| p.depth == 1));
    }

    #[test]
    fn trace_cross_service_skips_non_handles_route_reverse_edge() {
        // B --Calls--> A (reverse edge from A's perspective). When tracing
        // reverse edges from A, this Calls edge is NOT HandlesRoute, so it
        // must be skipped (line 305).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        // Reverse traversal only follows HandlesRoute, not Calls. So B should
        // NOT be reached via the reverse loop (only forward Calls would find
        // it, but A has no outgoing Calls).
        assert!(
            paths.is_empty(),
            "non-HandlesRoute reverse edge should be skipped"
        );
    }

    #[test]
    fn trace_cross_service_skips_reverse_edge_to_missing_handler() {
        // A is the target of a HandlesRoute edge from "missing_handler"
        // (source node not in graph). The reverse loop must skip it (line 308).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_edge(Edge::new("missing_handler", "a", EdgeType::HandlesRoute, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        assert!(paths.is_empty(), "reverse edge to missing handler should be skipped");
    }

    #[test]
    fn trace_cross_service_skips_reverse_cycle_to_visited_node() {
        // A --HttpCalls--> R, A --HandlesRoute--> R.
        // Forward: A→R (HttpCalls). Reverse from R: A--HandlesRoute-->R means
        // reverse loop from R sees source A, but A is already visited (line 311).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_route("r", "/api/r"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        g.add_edge(Edge::new("a", "r", EdgeType::HandlesRoute, "proj"));
        let tracer = CallGraphTracer::new(&g);
        let paths = tracer.trace_cross_service(&"a".to_string(), 5);
        // A→R should exist; A should not be revisited via reverse HandlesRoute.
        for p in &paths {
            let mut names: Vec<&str> = p.nodes.iter().map(|n| n.name.as_str()).collect();
            let len_before = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), len_before, "path revisits a node: {:?}", p.nodes);
        }
        assert!(paths.iter().any(|p| p.depth == 1));
    }
}
