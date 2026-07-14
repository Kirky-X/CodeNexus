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
//! persistent linked list. Expanding a child is O(1) — just `Rc::clone` —
//! instead of cloning the entire `visited_ids` and `TracePath` (O(depth)).
//! The full [`TracePath`] is materialized via [`WorkItem::build_path`] only
//! when a result is recorded.
//!
//! [`DataFlowTracer::trace`]: super::data_flow::DataFlowTracer::trace
//! [`TaintPathTracer`]: super::taint::TaintPathTracer

use std::collections::VecDeque;
use std::rc::Rc;

use crate::model::{EdgeType, Graph, NodeId};

use super::{TraceEdge, TraceNode, TracePath};

/// Internal BFS work item: holds the current node/edge and a shared
/// reference to the parent, forming a persistent path chain (MED-003).
pub(crate) struct WorkItem {
    node_id: NodeId,
    node: TraceNode,
    edge: Option<TraceEdge>,
    depth: usize,
    parent: Option<Rc<WorkItem>>,
}

impl WorkItem {
    fn new_root(node_id: NodeId, node: TraceNode) -> Self {
        Self {
            node_id,
            node,
            edge: None,
            depth: 0,
            parent: None,
        }
    }

    fn child(parent: &Rc<WorkItem>, node_id: NodeId, node: TraceNode, edge: TraceEdge) -> Self {
        Self {
            node_id,
            node,
            edge: Some(edge),
            depth: parent.depth + 1,
            parent: Some(Rc::clone(parent)),
        }
    }

    fn has_edges(&self) -> bool {
        self.edge.is_some()
    }

    /// Returns true if `id` appears anywhere in this path's parent chain.
    fn path_contains(&self, id: &NodeId) -> bool {
        let mut current: &WorkItem = self;
        loop {
            if &current.node_id == id {
                return true;
            }
            match &current.parent {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    /// Reconstructs the full [`TracePath`] by walking the parent chain.
    fn build_path(&self) -> TracePath {
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
        let has_edges = work.has_edges();
        let current_id = work.node_id.clone();

        // Sink mode: record complete paths and stop expansion at sink.
        if let Some(sink_id) = sink {
            if &current_id == sink_id && has_edges {
                results.push(work.build_path());
                continue;
            }
        }

        if work.depth >= max_depth {
            // Non-sink mode: record prefix at depth limit.
            if sink.is_none() && has_edges {
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
                TraceEdge {
                    edge_type: edge.edge_type.to_string(),
                    reason: edge.reason.clone(),
                    confidence: edge.confidence,
                },
            );
            queue.push_back(child);
        }

        // Non-sink mode: record every path prefix with edges.
        if sink.is_none() && has_edges {
            results.push(work_rc.build_path());
        }
    }

    results
}
