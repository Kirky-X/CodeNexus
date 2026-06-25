//! In-memory graph structure (ADD §3.4).

use std::collections::HashMap;

use super::{Edge, EdgeType, Node, NodeId, NodeLabel};

/// An in-memory code knowledge graph (ADD §3.4).
///
/// Stores nodes in a hash map keyed by id and edges in a vector. Provides
/// query helpers for traversal (`neighbors`, `reverse_neighbors`) and
/// filtering (`nodes_by_label`, `nodes_by_project`).
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// Nodes keyed by id.
    pub nodes: HashMap<NodeId, Node>,
    /// Edges in insertion order.
    pub edges: Vec<Edge>,
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
        self.edges
            .iter()
            .filter(|e| &e.source == id && Self::type_matches(e.edge_type, edge_type))
            .filter_map(|e| self.nodes.get(&e.target))
            .collect()
    }

    /// Returns the source nodes of incoming edges to `id`, optionally
    /// filtered by edge type. Nodes are returned in edge insertion order.
    #[must_use]
    pub fn reverse_neighbors(&self, id: &NodeId, edge_type: Option<EdgeType>) -> Vec<&Node> {
        self.edges
            .iter()
            .filter(|e| &e.target == id && Self::type_matches(e.edge_type, edge_type))
            .filter_map(|e| self.nodes.get(&e.source))
            .collect()
    }

    /// Returns all nodes with the given label. Order is not guaranteed.
    #[must_use]
    pub fn nodes_by_label(&self, label: NodeLabel) -> Vec<&Node> {
        self.nodes.values().filter(|n| n.label == label).collect()
    }

    /// Returns all nodes belonging to the given project. Order is not
    /// guaranteed.
    #[must_use]
    pub fn nodes_by_project(&self, project: &str) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.project == project)
            .collect()
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

    /// Returns all outgoing edges from `id`, in insertion order.
    #[must_use]
    pub fn edges_from(&self, id: &NodeId) -> Vec<&Edge> {
        self.edges.iter().filter(|e| &e.source == id).collect()
    }

    /// Returns all incoming edges to `id`, in insertion order.
    #[must_use]
    pub fn edges_to(&self, id: &NodeId) -> Vec<&Edge> {
        self.edges.iter().filter(|e| &e.target == id).collect()
    }

    /// Returns `true` if `edge` matches the optional `filter` (or if the
    /// filter is `None`).
    fn type_matches(edge: EdgeType, filter: Option<EdgeType>) -> bool {
        match filter {
            Some(t) => edge == t,
            None => true,
        }
    }
}

#[cfg(test)]
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
