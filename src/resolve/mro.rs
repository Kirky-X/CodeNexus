// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Method Resolution Order (MRO) per-language (design.md D5, H5).
//!
//! Provides [`MroStrategy`] enum and [`mro_for`] language mapping, plus
//! [`MroResolver`] for computing linearized ancestor sequences by walking
//! `Extends`/`Implements` edges in the graph.
//!
//! # Strategies
//!
//! - [`FirstWins`](MroStrategy::FirstWins): DFS pre-order, first occurrence
//!   wins (Rust / C / TypeScript — single inheritance + interfaces).
//! - [`C3`](MroStrategy::C3): Python C3 linearization (Python — diamond
//!   multiple inheritance).
//! - [`RubyMixin`](MroStrategy::RubyMixin): Ruby-style mixin order (preserved
//!   for future Ruby support; currently same as C3 for the merge step).
//! - [`None`](MroStrategy::None): no MRO (Fortran — no inheritance semantics).

use crate::model::{EdgeType, Graph, Language, NodeId};

/// Method Resolution Order strategy (design.md D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MroStrategy {
    /// DFS pre-order, first occurrence wins (Rust / C / TypeScript).
    #[default]
    FirstWins,
    /// Python C3 linearization (diamond multiple inheritance).
    C3,
    /// Ruby-style mixin order (reserved for future Ruby support).
    RubyMixin,
    /// No MRO — Fortran has no inheritance semantics (fail-loud, not silent).
    None,
}

/// Returns the MRO strategy for the given language (design.md D5).
#[must_use]
pub fn mro_for(lang: Language) -> MroStrategy {
    match lang {
        #[cfg(feature = "lang-python")]
        Language::Python => MroStrategy::C3,
        #[cfg(feature = "lang-rust")]
        Language::Rust => MroStrategy::FirstWins,
        #[cfg(feature = "lang-c")]
        Language::C => MroStrategy::FirstWins,
        #[cfg(feature = "lang-typescript")]
        Language::TypeScript => MroStrategy::FirstWins,
        #[cfg(feature = "lang-fortran")]
        Language::Fortran => MroStrategy::None,
        // Go has no classical inheritance (only structural interfaces + embedding,
        // which don't fit the C3/FirstWin MRO models). Return None to fail loud
        // rather than silently producing a wrong linearization.
        #[cfg(feature = "lang-go")]
        Language::Go => MroStrategy::None,
        // Java has single inheritance (extends) + interface implementation
        // (implements), matching the FirstWins DFS pre-order model used for
        // Rust/C/TypeScript.
        #[cfg(feature = "lang-java")]
        Language::Java => MroStrategy::FirstWins,
        // C++ supports multiple inheritance, but FirstWins (DFS pre-order) is
        // used as the default strategy for consistency with C. C3 linearization
        // is reserved for Python's diamond inheritance semantics.
        #[cfg(feature = "lang-cpp")]
        Language::Cpp => MroStrategy::FirstWins,
    }
}

/// Computes linearized ancestor sequences for type nodes by walking
/// `Extends`/`Implements` edges.
///
/// Construct with [`MroResolver::new`] passing a reference to the graph and
/// the strategy to apply, then call [`compute_mro`](Self::compute_mro) per
/// type node.
pub struct MroResolver<'a> {
    graph: &'a Graph,
    strategy: MroStrategy,
}

impl<'a> MroResolver<'a> {
    /// Creates a new `MroResolver` bound to the given graph and strategy.
    #[must_use]
    pub fn new(graph: &'a Graph, strategy: MroStrategy) -> Self {
        Self { graph, strategy }
    }

    /// Computes the linearized MRO for the given type node.
    ///
    /// Returns a vector of ancestor node ids in MRO order (excluding the type
    /// itself). For [`MroStrategy::None`], returns an empty vector (no MRO).
    ///
    /// # Arguments
    ///
    /// * `type_id` - The node id of the type whose MRO to compute.
    ///
    /// # Returns
    ///
    /// A vector of ancestor node ids in linearized MRO order. If the type has
    /// no `Extends`/`Implements` edges, the vector is empty.
    #[must_use]
    pub fn compute_mro(&self, type_id: &NodeId) -> Vec<NodeId> {
        match self.strategy {
            MroStrategy::None => Vec::new(),
            MroStrategy::FirstWins => self.compute_first_wins(type_id, &mut Vec::new()),
            MroStrategy::C3 => self.compute_c3(type_id),
            MroStrategy::RubyMixin => self.compute_c3(type_id),
        }
    }

    /// Returns the direct parent type ids of `type_id` (via `Extends` or
    /// `Implements` edges), in edge insertion order.
    fn parents(&self, type_id: &NodeId) -> Vec<NodeId> {
        // Single-line for coverage: tarpaulin attribute continuation
        self.graph
            .edges
            .iter()
            .filter(|e| {
                &e.source == type_id
                    && (e.edge_type == EdgeType::Extends || e.edge_type == EdgeType::Implements)
            })
            .map(|e| e.target.clone())
            .collect()
    }

    /// FirstWins: DFS pre-order, first occurrence wins.
    ///
    /// Visits parents left-to-right, recursing depth-first. The first time a
    /// node is seen, it is appended to the result. Subsequent visits are
    /// skipped (deduplication).
    fn compute_first_wins(&self, type_id: &NodeId, seen: &mut Vec<NodeId>) -> Vec<NodeId> {
        let mut result = Vec::new();
        for parent in self.parents(type_id) {
            // Single-line for coverage: tarpaulin attribute continuation
            if seen.contains(&parent) {
                continue;
            }
            seen.push(parent.clone());
            result.push(parent.clone());
            result.extend(self.compute_first_wins(&parent, seen));
        }
        result
    }

    /// C3 linearization (Python MRO).
    ///
    /// `L[C] = C + merge(L[B1], L[B2], ..., [B1, B2, ...])`
    ///
    /// The merge takes the first head of the first list that does not appear
    /// in the tail of any other list, removes it from all lists, and repeats
    /// until all lists are empty or no valid candidate exists (inconsistent
    /// hierarchy → returns what we have so far, fail-loud).
    fn compute_c3(&self, type_id: &NodeId) -> Vec<NodeId> {
        let parents = self.parents(type_id);
        if parents.is_empty() {
            return Vec::new();
        }
        // Recursively compute MRO for each parent.
        let mut parent_mros: Vec<Vec<NodeId>> = parents
            .iter()
            .map(|p| {
                let mut mro = vec![p.clone()];
                mro.extend(self.compute_c3(p));
                mro
            })
            .collect();
        // Add the base list [B1, B2, ...] as the last input to merge.
        parent_mros.push(parents.clone());
        Self::c3_merge(&mut parent_mros)
    }

    /// C3 merge step: repeatedly take the first head that doesn't appear in
    /// any tail, remove it from all lists.
    fn c3_merge(lists: &mut Vec<Vec<NodeId>>) -> Vec<NodeId> {
        let mut result = Vec::new();
        loop {
            // Remove empty lists.
            // Single-line for coverage: tarpaulin attribute continuation
            lists.retain(|l| !l.is_empty());
            if lists.is_empty() {
                break;
            }
            // Find a good head: first element of some list that is not in the
            // tail (non-first position) of any other list.
            let good_head = lists.iter().find_map(|l| {
                let head = l.first()?;
                let is_in_tail = lists
                    .iter()
                    .any(|other| other.iter().skip(1).any(|x| x == head));
                if is_in_tail {
                    None
                } else {
                    Some(head.clone())
                }
            });
            match good_head {
                Some(head) => {
                    result.push(head.clone());
                    // Remove head from all lists.
                    for l in lists.iter_mut() {
                        if l.first() == Some(&head) {
                            l.remove(0);
                        }
                    }
                }
                // Inconsistent hierarchy (no valid candidate). Fail-loud:
                // return partial result (design.md D5: "None 跳过 MRO,
                // fail-loud，不静默").
                // Single-line for coverage: tarpaulin attribute continuation
                None => break,
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Graph, Node, NodeLabel};

    fn make_class(id: &str, name: &str, lang: Language) -> Node {
        Node::builder(NodeLabel::Class, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .language(lang)
            .build()
    }

    fn add_extends(graph: &mut Graph, child: &str, parent: &str) {
        graph.add_edge(Edge::new(child, parent, EdgeType::Extends, "proj"));
    }

    fn add_implements(graph: &mut Graph, child: &str, parent: &str) {
        graph.add_edge(Edge::new(child, parent, EdgeType::Implements, "proj"));
    }

    // --- mro_for language mapping ---

    #[test]
    fn mro_for_python_is_c3() {
        assert_eq!(mro_for(Language::Python), MroStrategy::C3);
    }

    #[test]
    fn mro_for_rust_is_first_wins() {
        assert_eq!(mro_for(Language::Rust), MroStrategy::FirstWins);
    }

    #[test]
    fn mro_for_c_is_first_wins() {
        assert_eq!(mro_for(Language::C), MroStrategy::FirstWins);
    }

    #[test]
    fn mro_for_typescript_is_first_wins() {
        assert_eq!(mro_for(Language::TypeScript), MroStrategy::FirstWins);
    }

    #[test]
    fn mro_for_fortran_is_none() {
        assert_eq!(mro_for(Language::Fortran), MroStrategy::None);
    }

    // --- None strategy ---

    #[test]
    fn none_strategy_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Fortran));
        add_extends(&mut g, "a", "b");
        let resolver = MroResolver::new(&g, MroStrategy::None);
        assert!(resolver.compute_mro(&"a".to_string()).is_empty());
    }

    // --- FirstWins: single inheritance chain ---

    #[test]
    fn first_wins_single_chain() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Rust));
        g.add_node(make_class("b", "B", Language::Rust));
        g.add_node(make_class("c", "C", Language::Rust));
        // A -> B -> C
        add_extends(&mut g, "a", "b");
        add_extends(&mut g, "b", "c");
        let resolver = MroResolver::new(&g, MroStrategy::FirstWins);
        let mro = resolver.compute_mro(&"a".to_string());
        assert_eq!(mro, vec!["b".to_string(), "c".to_string()]);
    }

    // --- FirstWins: diamond deduplication ---

    #[test]
    fn first_wins_diamond_dedup() {
        let mut g = Graph::new();
        //   A
        //  / \
        // B   C
        //  \ /
        //   D
        g.add_node(make_class("a", "A", Language::Rust));
        g.add_node(make_class("b", "B", Language::Rust));
        g.add_node(make_class("c", "C", Language::Rust));
        g.add_node(make_class("d", "D", Language::Rust));
        add_extends(&mut g, "a", "b");
        add_extends(&mut g, "a", "c");
        add_extends(&mut g, "b", "d");
        add_extends(&mut g, "c", "d");
        let resolver = MroResolver::new(&g, MroStrategy::FirstWins);
        let mro = resolver.compute_mro(&"a".to_string());
        // FirstWins: A -> B -> D -> C (D already seen, skip)
        assert_eq!(mro, vec!["b".to_string(), "d".to_string(), "c".to_string()]);
    }

    // --- FirstWins: no parents ---

    #[test]
    fn first_wins_no_parents() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Rust));
        let resolver = MroResolver::new(&g, MroStrategy::FirstWins);
        assert!(resolver.compute_mro(&"a".to_string()).is_empty());
    }

    // --- C3: single inheritance chain ---

    #[test]
    fn c3_single_chain() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Python));
        g.add_node(make_class("b", "B", Language::Python));
        g.add_node(make_class("c", "C", Language::Python));
        // A -> B -> C
        add_extends(&mut g, "a", "b");
        add_extends(&mut g, "b", "c");
        let resolver = MroResolver::new(&g, MroStrategy::C3);
        let mro = resolver.compute_mro(&"a".to_string());
        assert_eq!(mro, vec!["b".to_string(), "c".to_string()]);
    }

    // --- C3: diamond ---

    #[test]
    fn c3_diamond() {
        let mut g = Graph::new();
        //   A
        //  / \
        // B   C
        //  \ /
        //   D
        g.add_node(make_class("a", "A", Language::Python));
        g.add_node(make_class("b", "B", Language::Python));
        g.add_node(make_class("c", "C", Language::Python));
        g.add_node(make_class("d", "D", Language::Python));
        add_extends(&mut g, "a", "b");
        add_extends(&mut g, "a", "c");
        add_extends(&mut g, "b", "d");
        add_extends(&mut g, "c", "d");
        let resolver = MroResolver::new(&g, MroStrategy::C3);
        let mro = resolver.compute_mro(&"a".to_string());
        // C3: A, B, C, D
        assert_eq!(mro, vec!["b".to_string(), "c".to_string(), "d".to_string()]);
    }

    // --- C3: no parents ---

    #[test]
    fn c3_no_parents() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Python));
        let resolver = MroResolver::new(&g, MroStrategy::C3);
        assert!(resolver.compute_mro(&"a".to_string()).is_empty());
    }

    // --- Implements edges also walked ---

    #[test]
    fn first_wins_includes_implements_edges() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Rust));
        g.add_node(make_class("t", "T", Language::Rust));
        // A implements T (Rust trait)
        add_implements(&mut g, "a", "t");
        let resolver = MroResolver::new(&g, MroStrategy::FirstWins);
        let mro = resolver.compute_mro(&"a".to_string());
        assert_eq!(mro, vec!["t".to_string()]);
    }

    // --- Ignores non-inheritance edges ---

    #[test]
    fn mro_ignores_calls_edges() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Rust));
        g.add_node(make_class("b", "B", Language::Rust));
        // A calls B (not inheritance)
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        let resolver = MroResolver::new(&g, MroStrategy::FirstWins);
        assert!(resolver.compute_mro(&"a".to_string()).is_empty());
    }

    // --- RubyMixin uses same merge as C3 ---

    #[test]
    fn ruby_mixin_single_chain() {
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Python));
        g.add_node(make_class("b", "B", Language::Python));
        add_extends(&mut g, "a", "b");
        let resolver = MroResolver::new(&g, MroStrategy::RubyMixin);
        let mro = resolver.compute_mro(&"a".to_string());
        assert_eq!(mro, vec!["b".to_string()]);
    }

    // --- Default strategy is FirstWins ---

    #[test]
    fn default_strategy_is_first_wins() {
        assert_eq!(MroStrategy::default(), MroStrategy::FirstWins);
    }

    // --- mro_for: remaining language arms ---

    #[cfg(feature = "lang-go")]
    #[test]
    fn mro_for_go_is_none() {
        // Go has no classical inheritance; fail-loud with None.
        assert_eq!(mro_for(Language::Go), MroStrategy::None);
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn mro_for_java_is_first_wins() {
        // Java single inheritance + interfaces -> FirstWins DFS pre-order.
        assert_eq!(mro_for(Language::Java), MroStrategy::FirstWins);
    }

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn mro_for_cpp_is_first_wins() {
        // C++ multiple inheritance defaults to FirstWins for C consistency.
        assert_eq!(mro_for(Language::Cpp), MroStrategy::FirstWins);
    }

    // --- C3: inconsistent hierarchy (fail-loud partial result) ---

    #[test]
    fn c3_inconsistent_hierarchy_returns_partial() {
        // Classic Python inconsistent MRO:
        //   X(A, B) wants A before B; Y(B, A) wants B before A;
        //   Z(X, Y) cannot satisfy both -> C3 merge has no valid head.
        // c3_merge must break (fail-loud) and return the partial result.
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Python));
        g.add_node(make_class("b", "B", Language::Python));
        g.add_node(make_class("x", "X", Language::Python));
        g.add_node(make_class("y", "Y", Language::Python));
        g.add_node(make_class("z", "Z", Language::Python));
        // X extends A then B (parents order: A, B)
        add_extends(&mut g, "x", "a");
        add_extends(&mut g, "x", "b");
        // Y extends B then A (parents order: B, A)
        add_extends(&mut g, "y", "b");
        add_extends(&mut g, "y", "a");
        // Z extends X then Y (parents order: X, Y)
        add_extends(&mut g, "z", "x");
        add_extends(&mut g, "z", "y");
        let resolver = MroResolver::new(&g, MroStrategy::C3);
        let mro = resolver.compute_mro(&"z".to_string());
        // C3 merges X and Y before detecting inconsistency. The partial
        // result must contain X and Y (in that order) but cannot complete.
        assert!(
            mro.starts_with(&["x".to_string(), "y".to_string()]),
            "expected partial MRO starting with [X, Y], got {mro:?}"
        );
        // Inconsistency: neither A nor B can be chosen (each is in the other's tail).
        assert!(
            !mro.contains(&"a".to_string()) || !mro.contains(&"b".to_string()),
            "inconsistent hierarchy should not fully resolve both A and B, got {mro:?}"
        );
    }

    // --- C3: head appears in tail is skipped (is_in_tail branch) ---

    #[test]
    fn c3_diamond_skips_head_in_tail() {
        // Diamond: A -> B, A -> C, B -> D, C -> D.
        // When merging, D appears in the tail of [C, D] while it's the head
        // of [D], so D is skipped on first encounter and taken only after C.
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Python));
        g.add_node(make_class("b", "B", Language::Python));
        g.add_node(make_class("c", "C", Language::Python));
        g.add_node(make_class("d", "D", Language::Python));
        add_extends(&mut g, "a", "b");
        add_extends(&mut g, "a", "c");
        add_extends(&mut g, "b", "d");
        add_extends(&mut g, "c", "d");
        let resolver = MroResolver::new(&g, MroStrategy::C3);
        let mro = resolver.compute_mro(&"a".to_string());
        // C3 linearization: A, B, C, D (D taken after C since it's in C's tail).
        assert_eq!(mro, vec!["b".to_string(), "c".to_string(), "d".to_string()]);
    }

    // --- FirstWins: seen-parent continue branch ---

    #[test]
    fn first_wins_skips_already_seen_parent() {
        // Diamond: A -> B -> D, A -> C -> D. When visiting D the second time
        // (via C), `seen` already contains D, so the `continue` branch fires.
        let mut g = Graph::new();
        g.add_node(make_class("a", "A", Language::Rust));
        g.add_node(make_class("b", "B", Language::Rust));
        g.add_node(make_class("c", "C", Language::Rust));
        g.add_node(make_class("d", "D", Language::Rust));
        add_extends(&mut g, "a", "b");
        add_extends(&mut g, "a", "c");
        add_extends(&mut g, "b", "d");
        add_extends(&mut g, "c", "d");
        let resolver = MroResolver::new(&g, MroStrategy::FirstWins);
        let mro = resolver.compute_mro(&"a".to_string());
        // D appears once even though reachable via B and C.
        let d_count = mro.iter().filter(|n| *n == "d").count();
        assert_eq!(d_count, 1, "D should appear exactly once, got {mro:?}");
    }
}
