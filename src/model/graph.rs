// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! In-memory graph structure (ADD §3.4).

use std::collections::HashMap;

use super::{Edge, EdgeType, Node, NodeId, NodeLabel};

/// An in-memory code knowledge graph (ADD §3.4).
///
/// Stores nodes in a hash map keyed by id and edges in a vector, backed by
/// adjacency indices for O(deg(n)) traversal. Provides query helpers for
/// traversal (`neighbors`, `reverse_neighbors`) and filtering
/// (`nodes_by_label`, `nodes_by_project`).
///
/// # Index invariant
///
/// `adjacency_out` and `adjacency_in` are maintained automatically by
/// `add_edge` and `retain_edges`. If caller mutates `edges` directly (the
/// field is `pub` for backwards compatibility), call `rebuild_index`
/// before issuing traversal queries.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// Nodes keyed by id.
    pub nodes: HashMap<NodeId, Node>,
    /// Edges in insertion order.
    pub edges: Vec<Edge>,
    /// Outgoing edge indices keyed by source node id (MED-002).
    adjacency_out: HashMap<NodeId, Vec<usize>>,
    /// Incoming edge indices keyed by target node id (MED-002).
    adjacency_in: HashMap<NodeId, Vec<usize>>,
}

impl Graph {
    /// Creates an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a node into the graph, replacing any existing node with the
    /// same id. Returns `&mut Self` for chaining.
    pub fn add_node(&mut self, node: Node) -> &mut Self {
        self.nodes.insert(node.id.clone(), node);
        self
    }

    /// Appends an edge to the graph. Returns `&mut Self` for chaining.
    pub fn add_edge(&mut self, edge: Edge) -> &mut Self {
        let idx = self.edges.len();
        self.adjacency_out
            .entry(edge.source.clone())
            .or_default()
            .push(idx);
        self.adjacency_in
            .entry(edge.target.clone())
            .or_default()
            .push(idx);
        self.edges.push(edge);
        self
    }

    /// Returns a reference to the node with the given id, if present.
    #[must_use]
    pub fn get_node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Returns a mutable reference to the node with the given id, if present.
    pub fn get_node_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// Returns the target nodes of outgoing edges from `id`, optionally
    /// filtered by edge type. Nodes are returned in edge insertion order.
    #[must_use]
    pub fn neighbors(&self, id: &NodeId, edge_type: Option<EdgeType>) -> Vec<&Node> {
        self.adjacency_out
            .get(id)
            .into_iter()
            .flat_map(|indices| indices.iter().map(|&i| self.edge_at(i)))
            .filter(|e| Self::type_matches(e.edge_type, edge_type))
            .filter_map(|e| self.nodes.get(&e.target))
            .collect()
    }

    /// Returns the source nodes of incoming edges to `id`, optionally
    /// filtered by edge type. Nodes are returned in edge insertion order.
    #[must_use]
    pub fn reverse_neighbors(&self, id: &NodeId, edge_type: Option<EdgeType>) -> Vec<&Node> {
        self.adjacency_in
            .get(id)
            .into_iter()
            .flat_map(|indices| indices.iter().map(|&i| self.edge_at(i)))
            .filter(|e| Self::type_matches(e.edge_type, edge_type))
            .filter_map(|e| self.nodes.get(&e.source))
            .collect()
    }

    /// Returns all nodes with the given label, sorted by node id.
    ///
    /// Sorting by `id` (FQN or `file_<uuid>`) makes the order deterministic
    /// across runs, since `self.nodes` is a `HashMap` whose iteration order
    /// is randomized per process (SipHash seed). Callers that pick `.first()`
    /// or iterate the result to build edges/parameters now observe stable
    /// output across indexes. See B12 fix in `tools/verification/results/triage.md`.
    #[must_use]
    pub fn nodes_by_label(&self, label: NodeLabel) -> Vec<&Node> {
        let mut v: Vec<&Node> = self.nodes.values().filter(|n| n.label == label).collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Returns all nodes belonging to the given project, sorted by node id.
    ///
    /// Same determinism guarantee as [`nodes_by_label`](Self::nodes_by_label):
    /// the underlying `HashMap` iterates in random order per process, so we
    /// sort by `id` to give callers a stable result. See B12 fix in
    /// `tools/verification/results/triage.md`.
    #[must_use]
    pub fn nodes_by_project(&self, project: &str) -> Vec<&Node> {
        let mut v: Vec<&Node> = self
            .nodes
            .values()
            .filter(|n| n.project == project)
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Returns the number of nodes in the graph.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the number of edges in the graph.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Iterates over all nodes in the graph (L3 of the memory-overflow fix).
    ///
    /// Replaces the `all_nodes: Vec<Node>` duplicate previously held in
    /// [`ScopeOutput`](crate::index::ScopeOutput) /
    /// [`ResolveOutput`](crate::index::ResolveOutput). Callers that
    /// previously consumed `&all_nodes` now consume `graph.nodes_view()`,
    /// eliminating one of the 4-5 copies of the same node set.
    ///
    /// Iteration order is the HashMap's unspecified order; callers that need
    /// determinism should use [`nodes_by_label`](Self::nodes_by_label) or
    /// [`nodes_by_project`](Self::nodes_by_project) which sort by id.
    pub fn nodes_view(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Iterates over all edges in the graph in insertion order (L3 of the
    /// memory-overflow fix).
    ///
    /// Replaces the `all_edges: Vec<Edge>` duplicate previously held in
    /// [`ScopeOutput`](crate::index::ScopeOutput) /
    /// [`ResolveOutput`](crate::index::ResolveOutput).
    pub fn edges_view(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter()
    }

    /// Mutates every node with the given `label` via `f` (L3 of the
    /// memory-overflow fix).
    ///
    /// Used by [`ResolvePhase`](crate::index::ResolvePhase) to rewrite
    /// `file_path` on `Parameter` / `Variable` nodes in place, instead of
    /// cloning them into a separate `all_nodes: Vec<Node>` and rewriting
    /// the clones.
    pub fn for_each_node_with_label_mut<F>(&mut self, label: NodeLabel, mut f: F)
    where
        F: FnMut(&mut Node),
    {
        for node in self.nodes.values_mut() {
            if node.label == label {
                f(node);
            }
        }
    }

    /// Returns all outgoing edges from `id`, in insertion order.
    #[must_use]
    pub fn edges_from(&self, id: &NodeId) -> Vec<&Edge> {
        self.adjacency_out
            .get(id)
            .map(|indices| indices.iter().map(|&i| self.edge_at(i)).collect())
            .unwrap_or_default()
    }

    /// Returns all incoming edges to `id`, in insertion order.
    #[must_use]
    pub fn edges_to(&self, id: &NodeId) -> Vec<&Edge> {
        self.adjacency_in
            .get(id)
            .map(|indices| indices.iter().map(|&i| self.edge_at(i)).collect())
            .unwrap_or_default()
    }

    /// Retains only edges for which `f` returns `true`, dropping the rest.
    ///
    /// Nodes are NOT removed — only edges. This is used by the CLI
    /// `--min-confidence` filter to drop low-confidence edges before trace /
    /// impact analysis (design.md D4). The adjacency index is rebuilt
    /// afterwards to stay consistent.
    pub fn retain_edges<F>(&mut self, f: F)
    where
        F: FnMut(&Edge) -> bool,
    {
        self.edges.retain(f);
        self.rebuild_index();
    }

    /// Rebuilds the adjacency index from `edges` (MED-002).
    ///
    /// Call this after mutating the `edges` field directly (it is `pub`
    /// for backwards compatibility). `add_edge` and `retain_edges` already
    /// keep the index in sync, so the common path never needs this.
    pub fn rebuild_index(&mut self) {
        self.adjacency_out.clear();
        self.adjacency_in.clear();
        for (idx, edge) in self.edges.iter().enumerate() {
            self.adjacency_out
                .entry(edge.source.clone())
                .or_default()
                .push(idx);
            self.adjacency_in
                .entry(edge.target.clone())
                .or_default()
                .push(idx);
        }
    }

    /// Returns `true` if `edge` matches the optional `filter` (or if the
    /// filter is `None`).
    fn type_matches(edge: EdgeType, filter: Option<EdgeType>) -> bool {
        match filter {
            Some(t) => edge == t,
            None => true,
        }
    }

    /// Looks up an edge by adjacency index, panicking with a diagnostic
    /// message if the index is stale (caller mutated `edges` without
    /// `rebuild_index`).
    fn edge_at(&self, idx: usize) -> &Edge {
        self.edges
            .get(idx)
            .expect("adjacency index stale: call Graph::rebuild_index after mutating Graph::edges")
    }
}

#[cfg(all(test, feature = "lang-c", feature = "lang-rust"))]
mod tests {
    use super::super::{Language, NodeLabel};
    use super::*;

    fn make_node(id: &str, label: NodeLabel, name: &str, project: &str) -> Node {
        Node::builder(label, name, format!("{project}.{name}"))
            .id(id)
            .project(project)
            .build()
    }

    fn make_node_with_lang(
        id: &str,
        label: NodeLabel,
        name: &str,
        project: &str,
        lang: Language,
    ) -> Node {
        Node::builder(label, name, format!("{project}.{name}"))
            .id(id)
            .project(project)
            .language(lang)
            .build()
    }

    #[test]
    fn new_creates_empty_graph() {
        let g = Graph::new();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
    }

    #[test]
    fn default_creates_empty_graph() {
        let g = Graph::default();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn add_node_inserts_into_map() {
        let mut g = Graph::new();
        let node = make_node("n1", NodeLabel::Function, "foo", "proj");
        g.add_node(node);
        assert_eq!(g.node_count(), 1);
        assert!(g.get_node(&"n1".to_string()).is_some());
    }

    #[test]
    fn add_node_returns_self_for_chaining() {
        let mut g = Graph::new();
        g.add_node(make_node("n1", NodeLabel::Function, "foo", "proj"))
            .add_node(make_node("n2", NodeLabel::Function, "bar", "proj"));
        assert_eq!(g.node_count(), 2);
    }

    #[test]
    fn add_node_replaces_existing_id() {
        let mut g = Graph::new();
        g.add_node(make_node("n1", NodeLabel::Function, "foo", "proj"));
        g.add_node(make_node("n1", NodeLabel::Struct, "bar", "proj"));
        assert_eq!(g.node_count(), 1);
        let node = g.get_node(&"n1".to_string()).unwrap();
        assert_eq!(node.name, "bar");
        assert_eq!(node.label, NodeLabel::Struct);
    }

    #[test]
    fn add_edge_appends_to_vec() {
        let mut g = Graph::new();
        let e = Edge::new("s", "t", EdgeType::Calls, "proj");
        g.add_edge(e);
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn add_edge_returns_self_for_chaining() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("s", "t", EdgeType::Calls, "proj"))
            .add_edge(Edge::new("t", "u", EdgeType::Calls, "proj"));
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn get_node_returns_some_for_existing() {
        let mut g = Graph::new();
        g.add_node(make_node("n1", NodeLabel::Function, "foo", "proj"));
        let node = g.get_node(&"n1".to_string());
        assert!(node.is_some());
        assert_eq!(node.unwrap().name, "foo");
    }

    #[test]
    fn get_node_returns_none_for_missing() {
        let g = Graph::new();
        assert!(g.get_node(&"missing".to_string()).is_none());
    }

    #[test]
    fn get_node_mut_allows_mutation() {
        let mut g = Graph::new();
        g.add_node(make_node("n1", NodeLabel::Function, "foo", "proj"));
        {
            let node = g.get_node_mut(&"n1".to_string()).unwrap();
            node.name = "renamed".to_string();
        }
        assert_eq!(g.get_node(&"n1".to_string()).unwrap().name, "renamed");
    }

    #[test]
    fn get_node_mut_returns_none_for_missing() {
        let mut g = Graph::new();
        assert!(g.get_node_mut(&"missing".to_string()).is_none());
    }

    #[test]
    fn neighbors_returns_targets_of_outgoing_edges() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_node(make_node("c", NodeLabel::Function, "c", "proj"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Calls, "proj"));

        let neighbors = g.neighbors(&"a".to_string(), None);
        assert_eq!(neighbors.len(), 2);
        let names: Vec<&str> = neighbors.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn neighbors_preserves_edge_insertion_order() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_node(make_node("c", NodeLabel::Function, "c", "proj"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Calls, "proj"));

        let neighbors = g.neighbors(&"a".to_string(), None);
        assert_eq!(neighbors[0].name, "b");
        assert_eq!(neighbors[1].name, "c");
    }

    #[test]
    fn neighbors_filtered_by_edge_type() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_node(make_node("c", NodeLabel::Function, "c", "proj"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Reads, "proj"));

        let calls = g.neighbors(&"a".to_string(), Some(EdgeType::Calls));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "b");

        let reads = g.neighbors(&"a".to_string(), Some(EdgeType::Reads));
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].name, "c");

        let writes = g.neighbors(&"a".to_string(), Some(EdgeType::Writes));
        assert!(writes.is_empty());
    }

    #[test]
    fn neighbors_empty_for_node_with_no_outgoing_edges() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));

        let neighbors = g.neighbors(&"b".to_string(), None);
        assert!(neighbors.is_empty());
    }

    #[test]
    fn neighbors_empty_for_missing_node() {
        let g = Graph::new();
        let neighbors = g.neighbors(&"missing".to_string(), None);
        assert!(neighbors.is_empty());
    }

    #[test]
    fn neighbors_skips_edges_to_missing_nodes() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        // Target "b" is not in the graph.
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));

        let neighbors = g.neighbors(&"a".to_string(), None);
        assert!(neighbors.is_empty());
    }

    #[test]
    fn reverse_neighbors_returns_sources_of_incoming_edges() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_node(make_node("c", NodeLabel::Function, "c", "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));

        let rev = g.reverse_neighbors(&"c".to_string(), None);
        assert_eq!(rev.len(), 2);
        let names: Vec<&str> = rev.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn reverse_neighbors_filtered_by_edge_type() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_node(make_node("c", NodeLabel::Function, "c", "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Reads, "proj"));

        let calls = g.reverse_neighbors(&"c".to_string(), Some(EdgeType::Calls));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "a");

        let reads = g.reverse_neighbors(&"c".to_string(), Some(EdgeType::Reads));
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].name, "b");
    }

    #[test]
    fn reverse_neighbors_empty_for_node_with_no_incoming_edges() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));

        let rev = g.reverse_neighbors(&"a".to_string(), None);
        assert!(rev.is_empty());
    }

    #[test]
    fn nodes_by_label_returns_matching_nodes() {
        let mut g = Graph::new();
        g.add_node(make_node("f1", NodeLabel::Function, "f1", "proj"));
        g.add_node(make_node("s1", NodeLabel::Struct, "s1", "proj"));
        g.add_node(make_node("f2", NodeLabel::Function, "f2", "proj"));

        let funcs = g.nodes_by_label(NodeLabel::Function);
        assert_eq!(funcs.len(), 2);
        let structs = g.nodes_by_label(NodeLabel::Struct);
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "s1");
    }

    #[test]
    fn nodes_by_label_empty_when_none_match() {
        let mut g = Graph::new();
        g.add_node(make_node("s1", NodeLabel::Struct, "s1", "proj"));
        let funcs = g.nodes_by_label(NodeLabel::Function);
        assert!(funcs.is_empty());
    }

    #[test]
    fn nodes_by_label_empty_for_empty_graph() {
        let g = Graph::new();
        assert!(g.nodes_by_label(NodeLabel::Function).is_empty());
    }

    // --- nodes_view (L3 memory-overflow fix) ---

    #[test]
    fn nodes_view_returns_all_nodes() {
        let mut g = Graph::new();
        g.add_node(make_node("f1", NodeLabel::Function, "f1", "proj"));
        g.add_node(make_node("s1", NodeLabel::Struct, "s1", "proj"));
        g.add_node(make_node("f2", NodeLabel::Function, "f2", "proj"));
        let count = g.nodes_view().count();
        assert_eq!(count, 3, "nodes_view must return every node");
    }

    #[test]
    fn nodes_view_empty_for_empty_graph() {
        let g = Graph::new();
        assert_eq!(g.nodes_view().count(), 0);
    }

    // --- edges_view (L3 memory-overflow fix) ---

    #[test]
    fn edges_view_returns_all_edges_in_insertion_order() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        g.add_node(make_node("c", NodeLabel::Function, "c", "proj"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::UsesType, "proj"));
        let collected: Vec<&Edge> = g.edges_view().collect();
        assert_eq!(collected.len(), 3);
        // Insertion order: a→b, b→c, a→c.
        assert_eq!(collected[0].source, "a");
        assert_eq!(collected[0].target, "b");
        assert_eq!(collected[1].source, "b");
        assert_eq!(collected[1].target, "c");
        assert_eq!(collected[2].source, "a");
        assert_eq!(collected[2].target, "c");
    }

    #[test]
    fn edges_view_empty_for_empty_graph() {
        let g = Graph::new();
        assert_eq!(g.edges_view().count(), 0);
    }

    // --- for_each_node_with_label_mut (L3 memory-overflow fix) ---

    #[test]
    fn for_each_node_with_label_mut_modifies_only_matching_label() {
        let mut g = Graph::new();
        g.add_node(make_node("f1", NodeLabel::Function, "f1", "proj"));
        g.add_node(make_node("p1", NodeLabel::Parameter, "p1", "proj"));
        g.add_node(make_node("p2", NodeLabel::Parameter, "p2", "proj"));
        g.add_node(make_node("s1", NodeLabel::Struct, "s1", "proj"));

        // Append "_mut" to every Parameter node's name, in place.
        g.for_each_node_with_label_mut(NodeLabel::Parameter, |n| {
            n.name.push_str("_mut");
        });

        assert_eq!(g.get_node(&"f1".to_string()).unwrap().name, "f1");
        assert_eq!(g.get_node(&"p1".to_string()).unwrap().name, "p1_mut");
        assert_eq!(g.get_node(&"p2".to_string()).unwrap().name, "p2_mut");
        assert_eq!(g.get_node(&"s1".to_string()).unwrap().name, "s1");
    }

    #[test]
    fn for_each_node_with_label_mut_noop_when_no_match() {
        let mut g = Graph::new();
        g.add_node(make_node("f1", NodeLabel::Function, "f1", "proj"));
        g.for_each_node_with_label_mut(NodeLabel::Parameter, |n| {
            n.name.push_str("_mut");
        });
        assert_eq!(g.get_node(&"f1".to_string()).unwrap().name, "f1");
    }

    #[test]
    fn for_each_node_with_label_mut_empty_graph_is_noop() {
        let mut g = Graph::new();
        g.for_each_node_with_label_mut(NodeLabel::Parameter, |_| {
            panic!("callback should not fire on empty graph");
        });
    }

    #[test]
    fn nodes_by_project_returns_matching_nodes() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj1"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj2"));
        g.add_node(make_node("c", NodeLabel::Function, "c", "proj1"));

        let p1 = g.nodes_by_project("proj1");
        assert_eq!(p1.len(), 2);
        let p2 = g.nodes_by_project("proj2");
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].name, "b");
    }

    #[test]
    fn nodes_by_project_isolates_projects() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj1"));
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj2"));

        let p1 = g.nodes_by_project("proj1");
        let p2 = g.nodes_by_project("proj2");
        assert_eq!(p1.len(), 1);
        assert_eq!(p2.len(), 1);
        assert_ne!(p1[0].id, p2[0].id);
    }

    #[test]
    fn nodes_by_project_empty_for_unknown_project() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj1"));
        assert!(g.nodes_by_project("unknown").is_empty());
    }

    #[test]
    fn node_count_tracks_insertions() {
        let mut g = Graph::new();
        assert_eq!(g.node_count(), 0);
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        assert_eq!(g.node_count(), 1);
        g.add_node(make_node("b", NodeLabel::Function, "b", "proj"));
        assert_eq!(g.node_count(), 2);
        // Replacing an existing id does not increase the count.
        g.add_node(make_node("a", NodeLabel::Struct, "a2", "proj"));
        assert_eq!(g.node_count(), 2);
    }

    #[test]
    fn edge_count_tracks_insertions() {
        let mut g = Graph::new();
        assert_eq!(g.edge_count(), 0);
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        assert_eq!(g.edge_count(), 1);
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn edges_from_returns_outgoing_edges() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));

        let from_a = g.edges_from(&"a".to_string());
        assert_eq!(from_a.len(), 2);
        assert_eq!(from_a[0].target, "b");
        assert_eq!(from_a[1].target, "c");

        let from_b = g.edges_from(&"b".to_string());
        assert_eq!(from_b.len(), 1);
    }

    #[test]
    fn edges_from_empty_for_node_with_no_outgoing() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        assert!(g.edges_from(&"b".to_string()).is_empty());
    }

    #[test]
    fn edges_from_empty_for_missing_node() {
        let g = Graph::new();
        assert!(g.edges_from(&"missing".to_string()).is_empty());
    }

    #[test]
    fn edges_to_returns_incoming_edges() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("c", "d", EdgeType::Calls, "proj"));

        let to_c = g.edges_to(&"c".to_string());
        assert_eq!(to_c.len(), 2);
        assert_eq!(to_c[0].source, "a");
        assert_eq!(to_c[1].source, "b");

        let to_d = g.edges_to(&"d".to_string());
        assert_eq!(to_d.len(), 1);
    }

    #[test]
    fn edges_to_empty_for_node_with_no_incoming() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        assert!(g.edges_to(&"a".to_string()).is_empty());
    }

    #[test]
    fn edges_to_empty_for_missing_node() {
        let g = Graph::new();
        assert!(g.edges_to(&"missing".to_string()).is_empty());
    }

    #[test]
    fn retain_edges_drops_low_confidence_edges() {
        let mut g = Graph::new();
        // confidence defaults to 1.0 via Edge::new
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        // Build a low-confidence edge manually
        let mut low = Edge::new("c", "d", EdgeType::Calls, "proj");
        low.confidence = 0.5;
        g.add_edge(low);

        assert_eq!(g.edge_count(), 2);
        g.retain_edges(|e| e.confidence >= 0.8);
        assert_eq!(g.edge_count(), 1);
        assert_eq!(g.edges[0].source, "a");
    }

    #[test]
    fn retain_edges_keeps_all_when_predicate_true() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "d", EdgeType::Reads, "proj"));
        g.retain_edges(|_| true);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn full_graph_scenario() {
        let mut g = Graph::new();
        // proj1: a -> b -> c (Calls)
        g.add_node(make_node_with_lang(
            "a",
            NodeLabel::Function,
            "a",
            "proj1",
            Language::Rust,
        ));
        g.add_node(make_node_with_lang(
            "b",
            NodeLabel::Function,
            "b",
            "proj1",
            Language::Rust,
        ));
        g.add_node(make_node_with_lang(
            "c",
            NodeLabel::Function,
            "c",
            "proj1",
            Language::Rust,
        ));
        // proj2: x -> y (Calls) - isolated
        g.add_node(make_node_with_lang(
            "x",
            NodeLabel::Function,
            "x",
            "proj2",
            Language::C,
        ));
        g.add_node(make_node_with_lang(
            "y",
            NodeLabel::Function,
            "y",
            "proj2",
            Language::C,
        ));

        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj1"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj1"));
        g.add_edge(Edge::new("x", "y", EdgeType::Calls, "proj2"));

        assert_eq!(g.node_count(), 5);
        assert_eq!(g.edge_count(), 3);

        // proj1 traversal: a -> b -> c
        let a_neighbors = g.neighbors(&"a".to_string(), Some(EdgeType::Calls));
        assert_eq!(a_neighbors.len(), 1);
        assert_eq!(a_neighbors[0].id, "b");

        let b_neighbors = g.neighbors(&"b".to_string(), Some(EdgeType::Calls));
        assert_eq!(b_neighbors.len(), 1);
        assert_eq!(b_neighbors[0].id, "c");

        // c has no outgoing Calls
        assert!(g
            .neighbors(&"c".to_string(), Some(EdgeType::Calls))
            .is_empty());

        // reverse: c <- b <- a
        let c_rev = g.reverse_neighbors(&"c".to_string(), Some(EdgeType::Calls));
        assert_eq!(c_rev.len(), 1);
        assert_eq!(c_rev[0].id, "b");

        // proj isolation
        assert_eq!(g.nodes_by_project("proj1").len(), 3);
        assert_eq!(g.nodes_by_project("proj2").len(), 2);

        // a's outgoing edges
        assert_eq!(g.edges_from(&"a".to_string()).len(), 1);
        // c's incoming edges
        assert_eq!(g.edges_to(&"c".to_string()).len(), 1);
    }

    #[test]
    fn clone_is_independent() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        let mut cloned = g.clone();
        cloned.add_node(make_node("b", NodeLabel::Function, "b", "proj"));

        assert_eq!(g.node_count(), 1);
        assert_eq!(cloned.node_count(), 2);
    }

    #[test]
    fn debug_is_non_empty() {
        let mut g = Graph::new();
        g.add_node(make_node("a", NodeLabel::Function, "a", "proj"));
        let debug = format!("{g:?}");
        assert!(debug.contains("Graph"));
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;

    #[test]
    fn add_edge_maintains_adjacency_index() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));

        let from_a = g.edges_from(&"a".to_string());
        assert_eq!(from_a.len(), 2);
        assert_eq!(from_a[0].target, "b");
        assert_eq!(from_a[1].target, "c");

        let to_c = g.edges_to(&"c".to_string());
        assert_eq!(to_c.len(), 2);
        assert_eq!(to_c[0].source, "a");
        assert_eq!(to_c[1].source, "b");
    }

    #[test]
    fn edges_from_preserves_insertion_order() {
        let mut g = Graph::new();
        for i in 0..10 {
            g.add_edge(Edge::new("src", format!("t{i}"), EdgeType::Calls, "proj"));
        }
        let edges = g.edges_from(&"src".to_string());
        let targets: Vec<&str> = edges.iter().map(|e| e.target.as_str()).collect();
        assert_eq!(
            targets,
            ["t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8", "t9"]
        );
    }

    #[test]
    fn retain_edges_rebuilds_index() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));

        g.retain_edges(|e| e.edge_type == EdgeType::Calls);

        assert_eq!(g.edge_count(), 2);
        let from_a = g.edges_from(&"a".to_string());
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_a[0].target, "b");

        let to_c = g.edges_to(&"c".to_string());
        assert_eq!(to_c.len(), 1);
        assert_eq!(to_c[0].source, "b");
    }

    #[test]
    fn rebuild_index_after_direct_mutation() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        // Directly push to edges (bypassing add_edge, breaking the index)
        g.edges.push(Edge::new("x", "y", EdgeType::Calls, "proj"));

        // Index is now stale — edges_from("x") returns empty
        assert!(g.edges_from(&"x".to_string()).is_empty());

        g.rebuild_index();
        let from_x = g.edges_from(&"x".to_string());
        assert_eq!(from_x.len(), 1);
        assert_eq!(from_x[0].target, "y");
    }

    #[test]
    fn clone_preserves_index() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Reads, "proj"));

        let cloned = g.clone();
        let from_a = cloned.edges_from(&"a".to_string());
        assert_eq!(from_a.len(), 2);
    }

    #[test]
    fn empty_graph_queries_return_empty() {
        let g = Graph::new();
        assert!(g.edges_from(&"any".to_string()).is_empty());
        assert!(g.edges_to(&"any".to_string()).is_empty());
        assert!(g.neighbors(&"any".to_string(), None).is_empty());
        assert!(g.reverse_neighbors(&"any".to_string(), None).is_empty());
    }

    #[test]
    fn large_graph_index_matches_linear_scan() {
        let mut g = Graph::new();
        for i in 0..500 {
            g.add_edge(Edge::new(
                format!("n{i}"),
                format!("n{}", (i + 1) % 500),
                EdgeType::Calls,
                "proj",
            ));
        }

        for i in 0..500 {
            let id = format!("n{i}").to_string();
            let indexed: Vec<&str> = g
                .edges_from(&id)
                .iter()
                .map(|e| e.target.as_str())
                .collect();
            let linear: Vec<&str> = g
                .edges
                .iter()
                .filter(|e| e.source == id)
                .map(|e| e.target.as_str())
                .collect();
            assert_eq!(indexed, linear, "mismatch at node {i}");
        }
    }

    #[test]
    fn neighbors_via_index_matches_linear() {
        let mut g = Graph::new();
        g.add_node(
            Node::builder(NodeLabel::Function, "hub", "proj.hub")
                .id("hub")
                .project("proj")
                .build(),
        );
        for i in 0..100 {
            let spoke = format!("spoke{i}");
            g.add_node(
                Node::builder(NodeLabel::Function, spoke.clone(), format!("proj.{spoke}"))
                    .id(spoke.clone())
                    .project("proj")
                    .build(),
            );
            g.add_edge(Edge::new(
                "hub",
                spoke,
                if i % 2 == 0 {
                    EdgeType::Calls
                } else {
                    EdgeType::Reads
                },
                "proj",
            ));
        }

        let indexed = g.neighbors(&"hub".to_string(), Some(EdgeType::Calls));
        let linear_count = g
            .edges
            .iter()
            .filter(|e| e.source == "hub" && e.edge_type == EdgeType::Calls)
            .count();
        assert_eq!(indexed.len(), linear_count);
    }

    #[test]
    #[should_panic(expected = "adjacency index stale")]
    fn retain_without_rebuild_panics() {
        let mut g = Graph::new();
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("a", "c", EdgeType::Reads, "proj"));
        // Directly retain on edges without rebuild_index — indices become stale.
        g.edges.retain(|e| e.edge_type == EdgeType::Calls);
        // This should panic because adjacency_out still references index 1 (now removed).
        let _ = g.edges_from(&"a".to_string());
    }
}
