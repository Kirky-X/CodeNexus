//! Impact analyzer (trace/impact.rs) — P1 explosion-radius analysis.
//!
//! Provides [`ImpactAnalyzer`] for computing the set of nodes affected by a
//! change to a given symbol. Performs a reverse BFS over all edge types so
//! that any node that (transitively) depends on the symbol is reported.

use std::collections::{HashSet, VecDeque};

use crate::model::{Graph, NodeId};

use super::TraceNode;

/// Reverse-BFS impact analyzer (P1: change explosion radius).
///
/// Holds an immutable borrow of the [`Graph`] and exposes [`analyze`] which
/// returns every node that (transitively) reaches `symbol_id` via any edge
/// type, up to `depth` hops.
///
/// [`analyze`]: ImpactAnalyzer::analyze
pub struct ImpactAnalyzer<'a> {
    graph: &'a Graph,
}

impl<'a> ImpactAnalyzer<'a> {
    /// Creates a new `ImpactAnalyzer` bound to the given graph.
    #[must_use]
    pub fn new(graph: &'a Graph) -> Self {
        Self { graph }
    }

    /// Performs a reverse BFS from `symbol_id` over all edge types, returning
    /// the distinct set of nodes that (transitively) depend on `symbol_id`
    /// within `depth` hops.
    ///
    /// The returned list does not include `symbol_id` itself. Each dependent
    /// node appears exactly once (deduplicated). Order is BFS order from the
    /// start node.
    ///
    /// Returns an empty vector if `symbol_id` is not in the graph or no node
    /// reaches it within `depth` hops.
    pub fn analyze(&self, symbol_id: &NodeId, depth: usize) -> Vec<TraceNode> {
        if self.graph.get_node(symbol_id).is_none() {
            return Vec::new();
        }
        let mut visited: HashSet<NodeId> = HashSet::new();
        visited.insert(symbol_id.clone());
        // Queue holds (node_id, current_depth).
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
        queue.push_back((symbol_id.clone(), 0));

        let mut results: Vec<TraceNode> = Vec::new();

        while let Some((current_id, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }
            // Reverse traversal: find nodes whose outgoing edges point at
            // `current_id` (i.e. reverse_neighbors over all edge types).
            for predecessor in self.graph.reverse_neighbors(&current_id, None) {
                if visited.contains(&predecessor.id) {
                    continue;
                }
                visited.insert(predecessor.id.clone());
                results.push(TraceNode::from(predecessor));
                queue.push_back((predecessor.id.clone(), current_depth + 1));
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Node, NodeLabel};

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

    #[test]
    fn analyze_returns_callers() {
        // Reverse traversal: who calls A -> returns callers.
        // B -> A, C -> A : callers of A are B and C.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
        assert_eq!(impacted.len(), 2);
    }

    #[test]
    fn analyze_excludes_symbol_itself() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        assert!(!impacted.iter().any(|n| n.name == "a"));
    }

    #[test]
    fn analyze_depth_limit() {
        // A <- B <- C (C calls B, B calls A). Depth 1 from A returns only B.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "b", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 1);
        assert_eq!(impacted.len(), 1);
        assert_eq!(impacted[0].name, "b");
    }

    #[test]
    fn analyze_depth_2_returns_transitive_callers() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "b", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 2);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
        assert_eq!(impacted.len(), 2);
    }

    #[test]
    fn analyze_no_callers_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"b".to_string(), 3);
        assert!(impacted.is_empty());
    }

    #[test]
    fn analyze_missing_symbol_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"missing".to_string(), 3);
        assert!(impacted.is_empty());
    }

    #[test]
    fn analyze_zero_depth_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 0);
        assert!(impacted.is_empty());
    }

    #[test]
    fn analyze_deduplicates_nodes() {
        // Diamond: B -> A, C -> A, D -> B, D -> C. From A depth 3, D should
        // appear only once even though it reaches A via two paths.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("d", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("d", "c", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        let d_count = impacted.iter().filter(|n| n.name == "d").count();
        assert_eq!(d_count, 1, "D should appear only once");
        assert_eq!(impacted.len(), 3);
    }

    #[test]
    fn analyze_follows_all_edge_types() {
        // Impact analysis follows ALL edge types, not just Calls.
        // foo reads v, bar writes v -> both foo and bar depend on v.
        let mut g = Graph::new();
        g.add_node(make_var("v", "v"));
        g.add_node(make_func("foo", "foo"));
        g.add_node(make_func("bar", "bar"));
        g.add_edge(Edge::new("foo", "v", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("bar", "v", EdgeType::Writes, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"v".to_string(), 3);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn analyze_cyclic_graph_terminates() {
        // A <-> B (mutual calls). Should terminate and deduplicate.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 5);
        // B calls A, so B is impacted. A calls B but A is the symbol itself.
        assert!(impacted.iter().any(|n| n.name == "b"));
        assert!(!impacted.iter().any(|n| n.name == "a"));
    }

    #[test]
    fn analyze_returns_trace_nodes_with_location() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        assert_eq!(impacted.len(), 1);
        assert_eq!(impacted[0].name, "b");
        assert_eq!(impacted[0].label, "Function");
        assert_eq!(impacted[0].file_path.as_deref(), Some("src/b.rs"));
        assert_eq!(impacted[0].start_line, Some(10));
    }

    #[test]
    fn analyze_transitive_dataflow_impact() {
        // x dataflows to y, y dataflows to z. Changing z impacts y and x.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_node(make_var("z", "z"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("y", "z", EdgeType::DataFlows, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"z".to_string(), 3);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"y"));
        assert!(names.contains(&"x"));
        assert_eq!(impacted.len(), 2);
    }
}
