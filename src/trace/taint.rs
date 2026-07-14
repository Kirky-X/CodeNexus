// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Taint path tracer (trace/taint.rs) — cross-language multi-hop taint
//! tracking (v0.3.0).
//!
//! Provides [`TaintPathTracer`] for performing BFS traversal over
//! `DataFlows`, `Reads`, `Writes`, and `FfiCalls` edges from a source symbol
//! to a sink symbol, with a configurable depth limit and cycle detection.
//!
//! Unlike [`DataFlowTracer`], which traces all reachable data-flow paths from
//! a single start node, [`TaintPathTracer`] is designed for source-to-sink
//! taint analysis: it follows both intra-language data-flow edges and
//! cross-language FFI edges, returning only complete paths that reach the
//! sink.
//!
//! [`DataFlowTracer`]: super::data_flow::DataFlowTracer

use std::collections::VecDeque;

use crate::model::{EdgeType, Graph, NodeId};

use super::{TraceEdge, TraceNode, TracePath};

/// BFS tracer over `DataFlows` / `Reads` / `Writes` / `FfiCalls` edges for
/// source-to-sink taint analysis (v0.3.0).
///
/// Holds an immutable borrow of the [`Graph`] and exposes:
/// - [`trace_taint`]: returns all paths from `source` to `sink` within
///   `max_depth` hops.
/// - [`trace_from_source`]: returns all reachable paths from `source` within
///   `max_depth` hops (no sink filter).
///
/// [`trace_taint`]: TaintPathTracer::trace_taint
/// [`trace_from_source`]: TaintPathTracer::trace_from_source
pub struct TaintPathTracer<'a> {
    graph: &'a Graph,
}

/// Internal BFS work item: tracks the chain of visited node ids alongside the
/// in-progress [`TracePath`] so cycles can be detected.
struct WorkPath {
    visited_ids: Vec<NodeId>,
    path: TracePath,
}

impl<'a> TaintPathTracer<'a> {
    /// Creates a new `TaintPathTracer` bound to the given graph.
    #[must_use]
    pub fn new(graph: &'a Graph) -> Self {
        Self { graph }
    }

    /// Performs a BFS traversal from `source` to `sink` over `DataFlows`,
    /// `Reads`, `Writes`, and `FfiCalls` edges, returning all complete paths
    /// that reach `sink` within `max_depth` hops.
    ///
    /// A path of depth `n` contains `n + 1` nodes and `n` edges. Only paths
    /// whose last node is `sink` are returned (incomplete prefixes are
    /// discarded). Cycles are handled by tracking visited nodes per-path.
    ///
    /// Returns an empty vector if `source` or `sink` is not in the graph, or
    /// no taint path exists within the depth limit.
    pub fn trace_taint(
        &self,
        source: &NodeId,
        sink: &NodeId,
        max_depth: usize,
    ) -> Vec<TracePath> {
        let Some(start_node) = self.graph.get_node(source) else {
            return Vec::new();
        };
        if self.graph.get_node(sink).is_none() {
            return Vec::new();
        }

        let mut queue: VecDeque<WorkPath> = VecDeque::new();
        queue.push_back(WorkPath {
            visited_ids: vec![source.clone()],
            path: TracePath {
                nodes: vec![TraceNode::from(start_node)],
                edges: Vec::new(),
                depth: 0,
            },
        });

        let mut results = Vec::new();

        while let Some(work) = queue.pop_front() {
            let current_id = work
                .visited_ids
                .last()
                .expect("work path always has at least one visited id")
                .clone();

            // Reached sink: record this path (only if it has edges, i.e., depth > 0).
            if current_id == *sink && !work.path.edges.is_empty() {
                results.push(work.path);
                continue;
            }

            if work.path.depth >= max_depth {
                continue;
            }

            for edge in self.graph.edges_from(&current_id) {
                if !is_taint_edge(&edge.edge_type) {
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
        }

        results
    }

    /// Performs a BFS traversal from `source` over `DataFlows`, `Reads`,
    /// `Writes`, and `FfiCalls` edges, returning all reachable paths within
    /// `max_depth` hops (no sink filter).
    ///
    /// Similar to [`DataFlowTracer::trace`] but additionally follows
    /// `FfiCalls` edges for cross-language taint tracking. Every path prefix
    /// (with at least one edge) is returned as a valid path.
    ///
    /// Returns an empty vector if `source` is not in the graph or has no
    /// outgoing taint edges.
    ///
    /// [`DataFlowTracer::trace`]: super::data_flow::DataFlowTracer::trace
    pub fn trace_from_source(&self, source: &NodeId, max_depth: usize) -> Vec<TracePath> {
        let Some(start_node) = self.graph.get_node(source) else {
            return Vec::new();
        };

        let mut queue: VecDeque<WorkPath> = VecDeque::new();
        queue.push_back(WorkPath {
            visited_ids: vec![source.clone()],
            path: TracePath {
                nodes: vec![TraceNode::from(start_node)],
                edges: Vec::new(),
                depth: 0,
            },
        });

        let mut results = Vec::new();

        while let Some(work) = queue.pop_front() {
            let has_edges = !work.path.edges.is_empty();
            let can_extend = work.path.depth < max_depth;

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
            for edge in self.graph.edges_from(&current_id) {
                if !is_taint_edge(&edge.edge_type) {
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

            if has_edges {
                results.push(work.path);
            }
        }

        results
    }
}

/// Returns `true` if the edge type is a taint-relevant edge (data flow or FFI).
#[inline]
fn is_taint_edge(edge_type: &EdgeType) -> bool {
    matches!(
        edge_type,
        EdgeType::DataFlows | EdgeType::Reads | EdgeType::Writes | EdgeType::FfiCalls
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

    // === trace_taint: basic data-flow paths ===

    #[test]
    fn trace_taint_simple_dataflow_path() {
        // source -> mid -> sink
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("mid", "mid"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "mid", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("mid", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 5);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].depth, 2);
        assert_eq!(paths[0].nodes.len(), 3);
        assert_eq!(paths[0].nodes[0].name, "source");
        assert_eq!(paths[0].nodes[2].name, "sink");
        assert_eq!(paths[0].edges.len(), 2);
        assert_eq!(paths[0].edges[0].edge_type, "DATAFLOWS");
        assert_eq!(paths[0].edges[1].edge_type, "DATAFLOWS");
    }

    #[test]
    fn trace_taint_direct_edge() {
        // source -> sink (direct)
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 5);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].depth, 1);
        assert_eq!(paths[0].nodes.len(), 2);
    }

    #[test]
    fn trace_taint_no_path_returns_empty() {
        // source and sink disconnected
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("sink", "sink"));
        g.add_node(make_var("other", "other"));
        g.add_edge(Edge::new("source", "other", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_taint_source_equals_sink_returns_empty() {
        // source == sink: no path with edges, so empty.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_edge(Edge::new("x", "x", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"x".to_string(), &"x".to_string(), 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_taint_missing_source_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_var("sink", "sink"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"missing".to_string(), &"sink".to_string(), 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_taint_missing_sink_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"missing".to_string(), 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_taint_depth_limit_excludes_long_paths() {
        // source -> mid -> sink, but max_depth=1 excludes the 2-hop path.
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("mid", "mid"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "mid", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("mid", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 1);
        assert!(paths.is_empty(), "depth 1 cannot reach sink at depth 2");
    }

    #[test]
    fn trace_taint_zero_depth_returns_empty() {
        // max_depth=0: initial path has no edges, so sink check's
        // !edges.is_empty() guard skips it; depth 0 >= 0 prevents expansion.
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 0);
        assert!(paths.is_empty(), "zero depth yields no path with edges");
    }

    #[test]
    fn trace_taint_sink_reached_at_exact_depth_limit() {
        // source -> mid -> sink, max_depth=2: path reaches sink at depth==max_depth.
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("mid", "mid"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "mid", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("mid", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 2);
        assert_eq!(paths.len(), 1, "sink reached at exact depth limit");
        assert_eq!(paths[0].depth, 2);
        assert_eq!(paths[0].nodes.len(), 3);
    }

    #[test]
    fn trace_taint_multiple_paths() {
        // source -> a -> sink, source -> b -> sink (two paths)
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("a", "a"));
        g.add_node(make_var("b", "b"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "a", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("source", "b", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("a", "sink", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("b", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 5);
        assert_eq!(paths.len(), 2);
        for p in &paths {
            assert_eq!(p.depth, 2);
            assert_eq!(p.nodes[0].name, "source");
            assert_eq!(p.nodes[2].name, "sink");
        }
    }

    #[test]
    fn trace_taint_cycle_terminates() {
        // source -> mid -> source (cycle), mid -> sink
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("mid", "mid"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "mid", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("mid", "source", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("mid", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 10);
        assert_eq!(paths.len(), 1, "one path source->mid->sink");
        assert_eq!(paths[0].nodes[0].name, "source");
        assert_eq!(paths[0].nodes[1].name, "mid");
        assert_eq!(paths[0].nodes[2].name, "sink");
    }

    #[test]
    fn trace_taint_reads_writes_edges_followed() {
        // source writes var, var dataflows to sink
        let mut g = Graph::new();
        g.add_node(make_func("source", "source"));
        g.add_node(make_var("v", "v"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "v", EdgeType::Writes, "proj"));
        g.add_edge(Edge::new("v", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 5);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].edges[0].edge_type, "WRITES");
        assert_eq!(paths[0].edges[1].edge_type, "DATAFLOWS");
    }

    // === trace_taint: cross-language FFI paths ===

    #[test]
    fn trace_taint_follows_ffi_calls_edge() {
        // rust_func -> [FfiCalls] -> c_func -> [DataFlows] -> sink
        let mut g = Graph::new();
        g.add_node(make_func("rust_func", "rust_func"));
        g.add_node(make_func("c_func", "c_func"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("rust_func", "c_func", EdgeType::FfiCalls, "proj"));
        g.add_edge(Edge::new("c_func", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"rust_func".to_string(), &"sink".to_string(), 5);
        assert_eq!(paths.len(), 1, "should follow FfiCalls edge to reach sink");
        assert_eq!(paths[0].depth, 2);
        assert_eq!(paths[0].edges[0].edge_type, "FFI_CALLS");
        assert_eq!(paths[0].edges[1].edge_type, "DATAFLOWS");
        assert_eq!(paths[0].nodes[0].name, "rust_func");
        assert_eq!(paths[0].nodes[1].name, "c_func");
        assert_eq!(paths[0].nodes[2].name, "sink");
    }

    #[test]
    fn trace_taint_cross_language_multi_hop() {
        // Rust source -> [DataFlows] -> Rust ffi_wrapper -> [FfiCalls] -> C func
        // -> [DataFlows] -> C sink
        let mut g = Graph::new();
        g.add_node(make_var("rust_source", "rust_source"));
        g.add_node(make_func("ffi_wrapper", "ffi_wrapper"));
        g.add_node(make_func("c_handler", "c_handler"));
        g.add_node(make_var("c_sink", "c_sink"));
        g.add_edge(Edge::new("rust_source", "ffi_wrapper", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("ffi_wrapper", "c_handler", EdgeType::FfiCalls, "proj"));
        g.add_edge(Edge::new("c_handler", "c_sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"rust_source".to_string(), &"c_sink".to_string(), 10);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].depth, 3);
        assert_eq!(paths[0].edges[0].edge_type, "DATAFLOWS");
        assert_eq!(paths[0].edges[1].edge_type, "FFI_CALLS");
        assert_eq!(paths[0].edges[2].edge_type, "DATAFLOWS");
    }

    // === trace_taint: edge filtering ===

    #[test]
    fn trace_taint_skips_calls_edges() {
        // Calls edges should NOT be followed by the taint tracer.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"a".to_string(), &"sink".to_string(), 5);
        assert!(paths.is_empty(), "Calls edges should not be followed");
    }

    #[test]
    fn trace_taint_skips_imports_edges() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("a", "b", EdgeType::Imports, "proj"));
        g.add_edge(Edge::new("b", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"a".to_string(), &"sink".to_string(), 5);
        assert!(paths.is_empty(), "Imports edges should not be followed");
    }

    // === trace_taint: confidence and reason ===

    #[test]
    fn trace_taint_carries_reason_and_confidence() {
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(
            Edge::builder("source", "sink", EdgeType::DataFlows, "proj")
                .confidence(0.85)
                .reason("taint flow".to_string())
                .build(),
        );
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_taint(&"source".to_string(), &"sink".to_string(), 5);
        assert_eq!(paths.len(), 1);
        let edge = &paths[0].edges[0];
        assert!((edge.confidence - 0.85).abs() < f32::EPSILON);
        assert_eq!(edge.reason.as_deref(), Some("taint flow"));
    }

    // === trace_from_source ===

    #[test]
    fn trace_from_source_returns_all_reachable_paths() {
        // source -> mid -> sink
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("mid", "mid"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "mid", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("mid", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_from_source(&"source".to_string(), 5);
        // source->mid (depth 1) and source->mid->sink (depth 2)
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|p| p.depth == 1));
        assert!(paths.iter().any(|p| p.depth == 2));
    }

    #[test]
    fn trace_from_source_follows_ffi_calls() {
        // source -> [FfiCalls] -> target
        let mut g = Graph::new();
        g.add_node(make_func("source", "source"));
        g.add_node(make_func("target", "target"));
        g.add_edge(Edge::new("source", "target", EdgeType::FfiCalls, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_from_source(&"source".to_string(), 5);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].edges[0].edge_type, "FFI_CALLS");
    }

    #[test]
    fn trace_from_source_missing_node_returns_empty() {
        let g = Graph::new();
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_from_source(&"missing".to_string(), 5);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_from_source_zero_depth_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("target", "target"));
        g.add_edge(Edge::new("source", "target", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_from_source(&"source".to_string(), 0);
        assert!(paths.is_empty());
    }

    #[test]
    fn trace_from_source_cycle_terminates() {
        let mut g = Graph::new();
        g.add_node(make_var("a", "a"));
        g.add_node(make_var("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_from_source(&"a".to_string(), 10);
        assert!(!paths.is_empty());
        for p in &paths {
            let mut names: Vec<&str> = p.nodes.iter().map(|n| n.name.as_str()).collect();
            let len_before = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), len_before, "no revisited nodes");
        }
    }

    #[test]
    fn trace_from_source_diamond_graph() {
        // source -> a, source -> b, a -> sink, b -> sink
        let mut g = Graph::new();
        g.add_node(make_var("source", "source"));
        g.add_node(make_var("a", "a"));
        g.add_node(make_var("b", "b"));
        g.add_node(make_var("sink", "sink"));
        g.add_edge(Edge::new("source", "a", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("source", "b", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("a", "sink", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("b", "sink", EdgeType::DataFlows, "proj"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_from_source(&"source".to_string(), 10);
        // 4 paths: source->a, source->b, source->a->sink, source->b->sink
        assert_eq!(paths.len(), 4);
    }

    #[test]
    fn trace_from_source_no_outgoing_edges_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_var("solo", "solo"));
        let tracer = TaintPathTracer::new(&g);
        let paths = tracer.trace_from_source(&"solo".to_string(), 5);
        assert!(paths.is_empty());
    }

    // === is_taint_edge ===

    #[test]
    fn is_taint_edge_dataflows() {
        assert!(is_taint_edge(&EdgeType::DataFlows));
    }

    #[test]
    fn is_taint_edge_reads() {
        assert!(is_taint_edge(&EdgeType::Reads));
    }

    #[test]
    fn is_taint_edge_writes() {
        assert!(is_taint_edge(&EdgeType::Writes));
    }

    #[test]
    fn is_taint_edge_ffi_calls() {
        assert!(is_taint_edge(&EdgeType::FfiCalls));
    }

    #[test]
    fn is_taint_edge_rejects_calls() {
        assert!(!is_taint_edge(&EdgeType::Calls));
    }

    #[test]
    fn is_taint_edge_rejects_imports() {
        assert!(!is_taint_edge(&EdgeType::Imports));
    }
}
