// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Shared BFS traversal helper for trace engines.
//!
//! [`bfs_trace`] unifies the BFS loop used by [`DataFlowTracer::trace`] and
//! [`TaintPathTracer`] so the traversal logic (queue management, cycle
//! detection, depth limiting, path recording) lives in exactly one place.
//! Callers customize behavior via the `edge_filter` closure and the optional
//! `sink` parameter.
//!
//! # Path sharing (MED-003)
//!
//! Each [`WorkItem`] holds an `Rc` reference to its parent, forming a
//! persistent linked list. Expanding a child shares the parent chain via
//! `Rc::clone` (O(1)) and clones only the per-path `HashSet<NodeId>` cycle
//! set (O(path_len)), avoiding the O(depth) deep-clone of the full
//! `TracePath` (with its String-heavy `TraceNode` fields) that the previous
//! `WorkPath` design required. The full [`TracePath`] is materialized via
//! [`WorkItem::build_path`] only when a result is recorded.
//!
//! # Cycle detection (MED-002)
//!
//! Each [`WorkItem`] also carries an `Rc<HashSet<NodeId>>` of every node on
//! its path, so [`WorkItem::path_contains`] is O(1) instead of an O(depth)
//! parent-chain walk — important because it runs in the inner BFS edge loop.
//!
//! [`DataFlowTracer::trace`]: super::data_flow::DataFlowTracer::trace
//! [`TaintPathTracer`]: super::taint::TaintPathTracer

use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use crate::model::{EdgeType, Graph, NodeId};

use super::{TraceEdge, TraceNode, TracePath};

/// Internal BFS work item: holds the current node/edge and a shared
/// reference to the parent, forming a persistent path chain (MED-003).
///
/// Reused by [`call_graph::CallGraphTracer`] (C2) so both trace engines share
/// the same O(1) cycle-detection and O(1) child-expansion data structures.
///
/// [`call_graph::CallGraphTracer`]: super::call_graph::CallGraphTracer
pub(crate) struct WorkItem {
    pub(crate) node_id: NodeId,
    node: TraceNode,
    edge: Option<TraceEdge>,
    pub(crate) depth: usize,
    parent: Option<Rc<WorkItem>>,
    /// O(1) membership set for cycle detection (MED-002). Each child clones
    /// the parent set and inserts its own id so [`path_contains`](Self::path_contains)
    /// is a HashSet lookup instead of an O(depth) parent-chain walk.
    path_set: Rc<HashSet<NodeId>>,
}

impl WorkItem {
    pub(crate) fn new_root(node_id: NodeId, node: TraceNode) -> Self {
        Self {
            path_set: Rc::new(HashSet::from([node_id.clone()])),
            node_id,
            node,
            edge: None,
            depth: 0,
            parent: None,
        }
    }

    pub(crate) fn child(
        parent: &Rc<WorkItem>,
        node_id: NodeId,
        node: TraceNode,
        edge: TraceEdge,
    ) -> Self {
        let mut path_set = (*parent.path_set).clone();
        path_set.insert(node_id.clone());
        Self {
            path_set: Rc::new(path_set),
            node_id,
            node,
            edge: Some(edge),
            depth: parent.depth + 1,
            parent: Some(Rc::clone(parent)),
        }
    }

    pub(crate) fn has_parent_edge(&self) -> bool {
        self.edge.is_some()
    }

    /// Returns true if `id` is on this path (O(1) HashSet lookup, MED-002).
    pub(crate) fn path_contains(&self, id: &NodeId) -> bool {
        self.path_set.contains(id)
    }

    /// Reconstructs the full [`TracePath`] by walking the parent chain.
    pub(crate) fn build_path(&self) -> TracePath {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut current: &WorkItem = self;
        loop {
            nodes.push(current.node.clone());
            if let Some(edge) = &current.edge {
                edges.push(edge.clone());
            }
            match &current.parent {
                Some(parent) => current = parent,
                None => break,
            }
        }
        nodes.reverse();
        edges.reverse();
        TracePath {
            nodes,
            edges,
            depth: self.depth,
        }
    }
}

/// Performs a BFS traversal from `start` over edges accepted by `edge_filter`,
/// returning paths within `max_depth` hops.
///
/// When `sink` is `Some(s)`, only complete paths that reach `s` are returned
/// (path prefixes are discarded, expansion stops at `s`). When `sink` is
/// `None`, every path prefix with at least one edge is returned.
///
/// Returns an empty vector if `start` (or `sink`, when provided) is not in the
/// graph.
pub(crate) fn bfs_trace(
    graph: &Graph,
    start: &NodeId,
    max_depth: usize,
    edge_filter: impl Fn(&EdgeType) -> bool,
    sink: Option<&NodeId>,
) -> Vec<TracePath> {
    let Some(start_node) = graph.get_node(start) else {
        return Vec::new();
    };
    if let Some(sink_id) = sink {
        if graph.get_node(sink_id).is_none() {
            return Vec::new();
        }
    }

    let mut queue: VecDeque<WorkItem> = VecDeque::new();
    queue.push_back(WorkItem::new_root(
        start.clone(),
        TraceNode::from(start_node),
    ));

    let mut results = Vec::new();

    while let Some(work) = queue.pop_front() {
        let has_parent_edge = work.has_parent_edge();
        let current_id = work.node_id.clone();

        // Sink mode: record complete paths and stop expansion at sink.
        if let Some(sink_id) = sink {
            if &current_id == sink_id && has_parent_edge {
                results.push(work.build_path());
                continue;
            }
        }

        if work.depth >= max_depth {
            // Non-sink mode: record prefix at depth limit.
            if sink.is_none() && has_parent_edge {
                results.push(work.build_path());
            }
            continue;
        }

        let work_rc = Rc::new(work);

        for edge in graph.edges_from(&current_id) {
            if !edge_filter(&edge.edge_type) {
                continue;
            }
            let Some(target_node) = graph.get_node(&edge.target) else {
                continue;
            };
            if work_rc.path_contains(&edge.target) {
                continue;
            }
            let child = WorkItem::child(
                &work_rc,
                edge.target.clone(),
                TraceNode::from(target_node),
                TraceEdge::from(edge),
            );
            queue.push_back(child);
        }

        // Non-sink mode: record every path prefix with edges.
        if sink.is_none() && has_parent_edge {
            results.push(work_rc.build_path());
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, Node, NodeLabel};

    /// Builds a 3-node graph A -Calls-> B -Calls-> C.
    fn abc_graph() -> Graph {
        let mut g = Graph::new();
        g.add_node(
            Node::builder(NodeLabel::Function, "a", "proj.a")
                .id("a")
                .project("proj")
                .build(),
        );
        g.add_node(
            Node::builder(NodeLabel::Function, "b", "proj.b")
                .id("b")
                .project("proj")
                .build(),
        );
        g.add_node(
            Node::builder(NodeLabel::Function, "c", "proj.c")
                .id("c")
                .project("proj")
                .build(),
        );
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g
    }

    fn trace_edge() -> TraceEdge {
        TraceEdge {
            edge_type: "CALLS".to_string(),
            reason: None,
            confidence: 1.0,
        }
    }

    #[test]
    fn has_parent_edge_false_for_root_true_for_child() {
        let g = abc_graph();
        let a = g.get_node(&"a".to_string()).unwrap();
        let b = g.get_node(&"b".to_string()).unwrap();
        let root = WorkItem::new_root("a".to_string(), TraceNode::from(a));
        assert!(!root.has_parent_edge());
        let rc_root = Rc::new(root);
        let child = WorkItem::child(&rc_root, "b".to_string(), TraceNode::from(b), trace_edge());
        assert!(child.has_parent_edge());
    }

    #[test]
    fn build_path_root_returns_single_node_no_edges() {
        let g = abc_graph();
        let a = g.get_node(&"a".to_string()).unwrap();
        let root = WorkItem::new_root("a".to_string(), TraceNode::from(a));
        let path = root.build_path();
        assert_eq!(path.depth, 0);
        assert_eq!(path.nodes.len(), 1);
        assert!(path.edges.is_empty());
    }

    #[test]
    fn build_path_reconstructs_root_to_leaf_order() {
        let g = abc_graph();
        let a = g.get_node(&"a".to_string()).unwrap();
        let b = g.get_node(&"b".to_string()).unwrap();
        let c = g.get_node(&"c".to_string()).unwrap();

        let root = WorkItem::new_root("a".to_string(), TraceNode::from(a));
        let rc_root = Rc::new(root);
        let mid = WorkItem::child(&rc_root, "b".to_string(), TraceNode::from(b), trace_edge());
        let rc_mid = Rc::new(mid);
        let leaf = WorkItem::child(&rc_mid, "c".to_string(), TraceNode::from(c), trace_edge());

        let path = leaf.build_path();
        assert_eq!(path.depth, 2);
        assert_eq!(path.nodes.len(), 3);
        assert_eq!(path.nodes[0].name, "a");
        assert_eq!(path.nodes[1].name, "b");
        assert_eq!(path.nodes[2].name, "c");
        assert_eq!(path.edges.len(), 2);
        assert_eq!(path.edges[0].edge_type, "CALLS");
        assert_eq!(path.edges[1].edge_type, "CALLS");
    }

    #[test]
    fn path_contains_returns_true_for_self() {
        let g = abc_graph();
        let a = g.get_node(&"a".to_string()).unwrap();
        let root = WorkItem::new_root("a".to_string(), TraceNode::from(a));
        assert!(root.path_contains(&"a".to_string()));
    }

    #[test]
    fn path_contains_returns_true_for_ancestor() {
        let g = abc_graph();
        let a = g.get_node(&"a".to_string()).unwrap();
        let b = g.get_node(&"b".to_string()).unwrap();
        let root = WorkItem::new_root("a".to_string(), TraceNode::from(a));
        let rc_root = Rc::new(root);
        let mid = WorkItem::child(&rc_root, "b".to_string(), TraceNode::from(b), trace_edge());
        assert!(mid.path_contains(&"a".to_string()));
        assert!(mid.path_contains(&"b".to_string()));
    }

    #[test]
    fn path_contains_returns_false_for_outside_node() {
        let g = abc_graph();
        let a = g.get_node(&"a".to_string()).unwrap();
        let root = WorkItem::new_root("a".to_string(), TraceNode::from(a));
        assert!(!root.path_contains(&"zzz".to_string()));
    }

    #[test]
    fn bfs_trace_start_not_in_graph_returns_empty() {
        let g = abc_graph();
        let paths = bfs_trace(&g, &"zzz".to_string(), 5, |_| true, None);
        assert!(paths.is_empty());
    }

    #[test]
    fn bfs_trace_sink_not_in_graph_returns_empty() {
        let g = abc_graph();
        let paths = bfs_trace(&g, &"a".to_string(), 5, |_| true, Some(&"zzz".to_string()));
        assert!(paths.is_empty());
    }

    #[test]
    fn bfs_trace_sink_mode_skips_trivial_start_eq_sink() {
        // start == sink: the root has no edges, so no path is recorded.
        // Guards against accidentally emitting a 0-edge trivial path.
        let g = abc_graph();
        let paths = bfs_trace(&g, &"a".to_string(), 5, |_| true, Some(&"a".to_string()));
        assert!(paths.is_empty());
    }

    #[test]
    fn bfs_trace_sink_mode_records_complete_path() {
        let g = abc_graph();
        let paths = bfs_trace(&g, &"a".to_string(), 5, |_| true, Some(&"c".to_string()));
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert_eq!(path.depth, 2);
        assert_eq!(path.nodes.len(), 3);
        assert_eq!(path.nodes[0].name, "a");
        assert_eq!(path.nodes[2].name, "c");
    }

    #[test]
    fn bfs_trace_sink_mode_records_path_at_max_depth_boundary() {
        // max_depth = 2: a→b→c is exactly 2 hops, must still reach sink.
        let g = abc_graph();
        let paths = bfs_trace(&g, &"a".to_string(), 2, |_| true, Some(&"c".to_string()));
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].nodes.last().unwrap().name, "c");
    }

    #[test]
    fn bfs_trace_sink_mode_max_depth_below_sink_returns_empty() {
        // max_depth = 1: a→b→c needs 2 hops; sink unreachable.
        let g = abc_graph();
        let paths = bfs_trace(&g, &"a".to_string(), 1, |_| true, Some(&"c".to_string()));
        assert!(paths.is_empty());
    }

    #[test]
    fn bfs_trace_non_sink_returns_all_prefixes() {
        let g = abc_graph();
        let paths = bfs_trace(&g, &"a".to_string(), 5, |_| true, None);
        // Prefixes with at least one edge: a→b, a→b→c.
        assert_eq!(paths.len(), 2);
        let depths: Vec<usize> = paths.iter().map(|p| p.depth).collect();
        assert!(depths.contains(&1));
        assert!(depths.contains(&2));
    }

    #[test]
    fn bfs_trace_edge_filter_skips_unmatched() {
        // Filter rejects Calls — no edges accepted, no paths emitted.
        let g = abc_graph();
        let paths = bfs_trace(&g, &"a".to_string(), 5, |et| *et != EdgeType::Calls, None);
        assert!(paths.is_empty());
    }

    #[test]
    fn bfs_trace_cycle_does_not_revisit_node() {
        // Add c→a to create a cycle. Sink mode to c must not loop.
        let mut g = abc_graph();
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        let paths = bfs_trace(&g, &"a".to_string(), 5, |_| true, Some(&"c".to_string()));
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].nodes.len(), 3);
    }
}
