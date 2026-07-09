// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! #include tracking graph for C++ scope-aware call resolution.
//!
//! [`IncludesGraph`] stores the directed `#include` relationships between
//! files (File A `#include`s File B → edge A → B) and supports transitive
//! closure queries ("which files are reachable from File A via #include
//! chains?").
//!
//! # Purpose
//!
//! BUG-C4 (reverted in v0.2.2): C++ free functions had `is_exported=false`
//! because [`ProjectSymbolTable::lookup_exported`] returned ALL same-name
//! functions across the entire project, causing massive over-resolution
//! (fmt CALLS 1,852 → 5,002, +54%). The fix requires scoping cross-file
//! resolution by the `#include` graph: a function in File B is only a
//! valid resolution target for a call in File A if A `#include`s B
//! (directly or transitively).
//!
//! # Design
//!
//! - Storage: `HashMap<String, HashSet<String>>` (file → directly included files)
//! - `reachable_from(start)`: BFS transitive closure, includes `start` itself
//!   (a file is always "reachable" from itself for scope purposes)
//! - `contains(from, to)`: direct edge check (no transitive closure)
//!
//! The graph is populated during [`ResolvePhase`] (see `phases.rs`) from
//! `EdgeType::Includes` edges and passed to [`CallResolver`] for
//! `lookup_exported_in_scope` filtering.
//!
//! [`ProjectSymbolTable::lookup_exported`]: crate::resolve::symbol_table::ProjectSymbolTable::lookup_exported
//! [`ResolvePhase`]: crate::index::phases::ResolvePhase
//! [`CallResolver`]: crate::resolve::calls::CallResolver

use std::collections::{HashMap, HashSet};

/// Directed graph of `#include` relationships between files.
///
/// Stores File A → File B edges where A `#include`s B. Supports transitive
/// closure queries via [`reachable_from`](Self::reachable_from).
///
/// # Examples
///
/// ```
/// use codenexus::resolve::includes_graph::IncludesGraph;
///
/// let mut graph = IncludesGraph::new();
/// graph.add_include("main.cpp", "foo.h");
/// graph.add_include("foo.h", "bar.h");
///
/// // Transitive closure: main.cpp reaches foo.h and bar.h (and itself).
/// let reachable = graph.reachable_from("main.cpp");
/// assert!(reachable.contains("main.cpp"));
/// assert!(reachable.contains("foo.h"));
/// assert!(reachable.contains("bar.h"));
///
/// // Direct edge check.
/// assert!(graph.contains("main.cpp", "foo.h"));
/// assert!(!graph.contains("foo.h", "main.cpp")); // directed, not symmetric
/// ```
#[derive(Debug, Clone, Default)]
pub struct IncludesGraph {
    /// Adjacency list: source file → set of directly included files.
    edges: HashMap<String, HashSet<String>>,
}

impl IncludesGraph {
    /// Creates an empty `IncludesGraph`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }

    /// Adds a directed `#include` edge: `from_file` includes `to_file`.
    ///
    /// Duplicate edges are silently collapsed (idempotent — `HashSet` dedup).
    /// Self-edges (`from == to`) are ignored (a file cannot `#include` itself
    /// in valid C++; if encountered, it's a parse artifact, not a real edge).
    pub fn add_include(&mut self, from_file: &str, to_file: &str) {
        if from_file == to_file {
            return;
        }
        self.edges
            .entry(from_file.to_string())
            .or_default()
            .insert(to_file.to_string());
    }

    /// Returns all files reachable from `start` via `#include` chains
    /// (transitive closure), **including `start` itself**.
    ///
    /// A file is always considered reachable from itself for scope purposes:
    /// a function defined in the same file as the caller is always a valid
    /// resolution target, regardless of `#include` relationships.
    ///
    /// # Algorithm
    ///
    /// BFS over the adjacency list. Avoids infinite loops on cycles
    /// (e.g. `a.h ↔ b.h` mutual includes) by tracking visited nodes.
    ///
    /// # Returns
    ///
    /// `HashSet<&str>` with lifetimes tied to `&self`. Empty set if `start`
    /// has no outgoing edges AND is not a key in `edges` (still returns
    /// `{start}` because a file always reaches itself).
    pub fn reachable_from<'a>(&'a self, start: &'a str) -> HashSet<&'a str> {
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(start);
        let mut queue: Vec<&str> = vec![start];
        while let Some(current) = queue.pop() {
            if let Some(neighbors) = self.edges.get(current) {
                for next in neighbors {
                    let next: &str = next;
                    if visited.insert(next) {
                        queue.push(next);
                    }
                }
            }
        }
        visited
    }

    /// Returns `true` if `from` directly includes `to` (no transitive closure).
    ///
    /// For transitive reachability, use [`reachable_from`](Self::reachable_from).
    pub fn contains(&self, from: &str, to: &str) -> bool {
        self.edges
            .get(from)
            .is_some_and(|neighbors| neighbors.contains(to))
    }

    /// Returns the number of direct `#include` edges in the graph.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(SetCount::len).sum()
    }

    /// Returns `true` if the graph has no edges.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edges.values().all(|s| s.is_empty())
    }
}

/// Trait alias to access `HashSet::len` without importing the type name.
/// Kept private to avoid leaking implementation detail.
trait SetCount {
    fn len(&self) -> usize;
}

impl<T, S> SetCount for HashSet<T, S> {
    fn len(&self) -> usize {
        HashSet::len(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- new() / empty graph ---

    #[test]
    fn new_creates_empty_graph() {
        let graph = IncludesGraph::new();
        assert!(graph.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn default_creates_empty_graph() {
        let graph = IncludesGraph::default();
        assert!(graph.is_empty());
    }

    // --- add_include + contains (direct edges) ---

    #[test]
    fn add_include_creates_direct_edge() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a.cpp", "b.h");
        assert!(graph.contains("a.cpp", "b.h"));
    }

    #[test]
    fn includes_graph_contains_direct() {
        // Spec T001 Red test: add_include("a","b") → contains("a","b")==true
        // and contains("b","a")==false (directed, not symmetric).
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        assert!(graph.contains("a", "b"));
        assert!(!graph.contains("b", "a"), "edge is directed: b→a should not exist");
    }

    #[test]
    fn contains_returns_false_for_missing_from() {
        let graph = IncludesGraph::new();
        assert!(!graph.contains("nonexistent", "b.h"));
    }

    #[test]
    fn contains_returns_false_for_missing_to() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        assert!(!graph.contains("a", "c"));
    }

    #[test]
    fn add_include_is_idempotent() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("a", "b");
        assert_eq!(graph.edge_count(), 1, "duplicate edge should collapse");
    }

    #[test]
    fn add_include_ignores_self_edge() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "a");
        assert!(!graph.contains("a", "a"), "self-edge should be ignored");
        assert!(graph.is_empty());
    }

    #[test]
    fn add_include_multiple_targets() {
        let mut graph = IncludesGraph::new();
        graph.add_include("main.cpp", "foo.h");
        graph.add_include("main.cpp", "bar.h");
        graph.add_include("main.cpp", "baz.h");
        assert_eq!(graph.edge_count(), 3);
        assert!(graph.contains("main.cpp", "foo.h"));
        assert!(graph.contains("main.cpp", "bar.h"));
        assert!(graph.contains("main.cpp", "baz.h"));
    }

    // --- reachable_from (transitive closure) ---

    #[test]
    fn includes_graph_reachable_transitive() {
        // Spec T001 Red test: a→b, b→c → reachable_from("a") contains a/b/c.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("b", "c");
        let reachable = graph.reachable_from("a");
        assert!(reachable.contains("a"), "start node should be reachable from itself");
        assert!(reachable.contains("b"), "direct neighbor should be reachable");
        assert!(reachable.contains("c"), "transitive neighbor should be reachable");
        assert_eq!(reachable.len(), 3);
    }

    #[test]
    fn reachable_from_includes_start_itself() {
        // A file is always reachable from itself (scope includes same-file).
        let graph = IncludesGraph::new();
        let reachable = graph.reachable_from("lonely.cpp");
        assert_eq!(reachable.len(), 1);
        assert!(reachable.contains("lonely.cpp"));
    }

    #[test]
    fn reachable_from_no_outgoing_edges() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        // "b" has no outgoing edges, but reachable_from("b") should still
        // include "b" itself.
        let reachable = graph.reachable_from("b");
        assert_eq!(reachable.len(), 1);
        assert!(reachable.contains("b"));
    }

    #[test]
    fn reachable_from_handles_cycle() {
        // Mutual includes: a↔b. BFS must terminate (visited set prevents loop).
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("b", "a");
        let reachable_from_a = graph.reachable_from("a");
        assert!(reachable_from_a.contains("a"));
        assert!(reachable_from_a.contains("b"));
        assert_eq!(reachable_from_a.len(), 2);

        let reachable_from_b = graph.reachable_from("b");
        assert!(reachable_from_b.contains("a"));
        assert!(reachable_from_b.contains("b"));
        assert_eq!(reachable_from_b.len(), 2);
    }

    #[test]
    fn reachable_from_diamond_shape() {
        // Diamond: a→b, a→c, b→d, c→d. d reachable from a via two paths.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("a", "c");
        graph.add_include("b", "d");
        graph.add_include("c", "d");
        let reachable = graph.reachable_from("a");
        assert!(reachable.contains("a"));
        assert!(reachable.contains("b"));
        assert!(reachable.contains("c"));
        assert!(reachable.contains("d"));
        assert_eq!(reachable.len(), 4, "d should appear once despite two paths");
    }

    #[test]
    fn reachable_from_deep_chain() {
        // Deep chain: a→b→c→d→e. All reachable from a.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("b", "c");
        graph.add_include("c", "d");
        graph.add_include("d", "e");
        let reachable = graph.reachable_from("a");
        assert_eq!(reachable.len(), 5);
        for file in &["a", "b", "c", "d", "e"] {
            assert!(reachable.contains(file), "{file} should be reachable");
        }
    }

    #[test]
    fn reachable_from_disconnected_components() {
        // Two disconnected subgraphs: a→b and c→d. reachable_from("a") should
        // NOT include c or d.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("c", "d");
        let reachable_from_a = graph.reachable_from("a");
        assert!(reachable_from_a.contains("a"));
        assert!(reachable_from_a.contains("b"));
        assert!(!reachable_from_a.contains("c"), "disconnected node should not be reachable");
        assert!(!reachable_from_a.contains("d"), "disconnected node should not be reachable");
    }

    // --- edge_count / is_empty ---

    #[test]
    fn edge_count_counts_all_direct_edges() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("a", "c");
        graph.add_include("b", "c");
        assert_eq!(graph.edge_count(), 3);
    }

    #[test]
    fn is_empty_true_for_new_graph() {
        let graph = IncludesGraph::new();
        assert!(graph.is_empty());
    }

    #[test]
    fn is_empty_false_after_add_edge() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        assert!(!graph.is_empty());
    }

    #[test]
    fn is_empty_true_when_only_self_edges_attempted() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "a"); // self-edge ignored
        assert!(graph.is_empty());
    }
}
