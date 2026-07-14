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
//! [`DataFlowTracer::trace`]: super::data_flow::DataFlowTracer::trace
//! [`TaintPathTracer`]: super::taint::TaintPathTracer

use std::collections::VecDeque;

use crate::model::{EdgeType, Graph, NodeId};

use super::{TraceEdge, TraceNode, TracePath};

/// Internal BFS work item: tracks the chain of visited node ids alongside the
/// in-progress [`TracePath`] so cycles can be detected.
pub(crate) struct WorkPath {
    pub(crate) visited_ids: Vec<NodeId>,
    pub(crate) path: TracePath,
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

    let mut queue: VecDeque<WorkPath> = VecDeque::new();
    queue.push_back(WorkPath {
        visited_ids: vec![start.clone()],
        path: TracePath {
            nodes: vec![TraceNode::from(start_node)],
            edges: Vec::new(),
            depth: 0,
        },
    });

    let mut results = Vec::new();

    while let Some(work) = queue.pop_front() {
        let has_edges = !work.path.edges.is_empty();
        let current_id = work
            .visited_ids
            .last()
            .expect("work path always has at least one visited id")
            .clone();

        // Sink mode: record complete paths and stop expansion at sink.
        if let Some(sink_id) = sink {
            if current_id == *sink_id && has_edges {
                results.push(work.path);
                continue;
            }
        }

        if work.path.depth >= max_depth {
            // Non-sink mode: record prefix at depth limit.
            if sink.is_none() && has_edges {
                results.push(work.path);
            }
            continue;
        }

        for edge in graph.edges_from(&current_id) {
            if !edge_filter(&edge.edge_type) {
                continue;
            }
            let Some(target_node) = graph.get_node(&edge.target) else {
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

        // Non-sink mode: record every path prefix with edges.
        if sink.is_none() && has_edges {
            results.push(work.path);
        }
    }

    results
}
