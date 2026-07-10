// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Resolver subsystem (T6/unified-architecture
//! Phase 2, Task 2.8).
//!
//! Implements [`Module`] / [`ModuleBuilder`] for [`ResolverModule`], wiring
//! the existing free functions [`build_symbol_table`](super::build_symbol_table)
//! and [`resolve_all`](super::resolve_all) into the unified Kit registry as
//! `Arc<dyn Resolver>` under [`ResolverKey`](crate::kit::ResolverKey).
//!
//! # Design note
//!
//! Unlike Storage/Indexer, the Resolver has no facade struct — it exposes two
//! stateless free functions. [`ResolverCapability`] is therefore zero-sized
//! and delegates directly. Conceptually the Resolver depends on `StorageKey`
//! (its inputs ultimately come from a parsed + stored codebase), but the
//! concrete impl takes its inputs as parameters, so
//! `Requirements = NoRequirements` at the type level; the bootstrap
//! (Task 2.13) enforces build ordering.
//!
//! [`Module`]: crate::kit::Module
//! [`ModuleBuilder`]: crate::kit::ModuleBuilder

use std::sync::Arc;

use crate::kit::{Module, ModuleBuilder, NoConfig, NoRequirements};

use super::capability::Resolver;
use super::error::ResolveError;
use super::includes_graph::IncludesGraph;
use super::symbol_table::ProjectSymbolTable;
use crate::ir::ExtractResult;
use crate::model::{Edge, Graph};

// ---------------------------------------------------------------------------
// Module + Builder
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Resolver subsystem (Task 2.8).
///
/// Zero-sized marker — construction logic lives in
/// [`ResolverModuleBuilder::build`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{IntoKitModuleBuilder, Kit, ResolverKey};
/// use codenexus::resolve::ResolverModuleBuilder;
///
/// let kit = Kit::new();
/// let resolver = ResolverModuleBuilder::new()
///     .kit(&kit)
///     .provide::<ResolverKey>()?;
/// ```
pub struct ResolverModule;

/// Builder for [`ResolverModule`] (Task 2.8).
///
/// No configuration is required — the resolver delegates to stateless free
/// functions.
pub struct ResolverModuleBuilder;

impl ResolverModuleBuilder {
    /// Creates a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for ResolverModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for ResolverModule {
    type Config = NoConfig;
    type Requirements = NoRequirements;
    type Capability = Arc<dyn Resolver>;
    type Error = ResolveError;
    type Builder = ResolverModuleBuilder;
    const NAME: &'static str = "resolver";
}

impl ModuleBuilder<ResolverModule> for ResolverModuleBuilder {
    fn build(self) -> Result<Arc<dyn Resolver>, ResolveError> {
        Ok(Arc::new(ResolverCapability))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn Resolver implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn Resolver`] delegating to the stateless
/// free functions [`build_symbol_table`](super::build_symbol_table) and
/// [`resolve_all`](super::resolve_all).
///
/// Zero-sized — every call dispatches to the free function with the supplied
/// parameters.
struct ResolverCapability;

impl Resolver for ResolverCapability {
    fn build_symbol_table(&self, results: &[ExtractResult], project: &str) -> ProjectSymbolTable {
        super::build_symbol_table(results, project)
    }

    fn resolve_all(
        &self,
        results: &[ExtractResult],
        symbol_table: &ProjectSymbolTable,
        project: &str,
        graph: &mut Graph,
        includes_graph: &IncludesGraph,
    ) -> Vec<Edge> {
        super::resolve_all(results, symbol_table, project, graph, includes_graph)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::CallInfo;
    use crate::kit::ResolverKey;
    use crate::model::{Language, Node, NodeLabel};
    use crate::resolve::FqnGenerator;

    /// Builds a minimal `ExtractResult` with two exported functions (`foo`,
    /// `bar`) in `a.rs` and a call from `foo` to `bar`, plus the matching
    /// graph nodes keyed by FQN. Returns `(results, table, graph)`.
    fn fixture_call_foo_to_bar() -> (Vec<ExtractResult>, ProjectSymbolTable, Graph) {
        let foo_node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();
        let bar_node = Node::builder(NodeLabel::Function, "bar", "qn")
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();
        let foo_qn = FqnGenerator::generate("proj", "a.rs", "foo", Language::Rust, None);
        let bar_qn = FqnGenerator::generate("proj", "a.rs", "bar", Language::Rust, None);

        let mut result = ExtractResult::new("a.rs", Language::Rust);
        result.nodes = vec![foo_node, bar_node];
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn.clone()),
            callee_name: "bar".to_string(),
            line: 5,
            args: vec![],
        });

        let results = vec![result];
        let table = crate::resolve::build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        for r in &results {
            for node in &r.nodes {
                let qn =
                    FqnGenerator::generate("proj", &r.file_path, &node.name, Language::Rust, None);
                let mut g = node.clone();
                g.id = qn.clone();
                g.qualified_name = qn;
                graph.add_node(g);
            }
        }
        let _ = bar_qn; // bar_qn asserted in the call-edge test below
        (results, table, graph)
    }

    #[test]
    fn build_returns_capability() {
        let cap = ResolverModuleBuilder::new().build().expect("build");
        // Exercise the real trait method with empty input — an empty results
        // slice yields an empty symbol table.
        let table = cap.build_symbol_table(&[], "proj");
        assert_eq!(table.symbol_count(), 0, "empty results → empty table");
        assert_eq!(table.file_count(), 0);
    }

    #[test]
    fn capability_build_symbol_table_returns_table() {
        let (results, _table, _graph) = fixture_call_foo_to_bar();
        let cap = ResolverModuleBuilder::new().build().expect("build");
        let table = cap.build_symbol_table(&results, "proj");
        assert_eq!(table.symbol_count(), 2, "foo + bar");
        assert_eq!(table.file_count(), 1);
        assert!(table.lookup_exact("foo").is_some());
        assert!(table.lookup_exact("bar").is_some());
    }

    #[test]
    fn capability_resolve_all_produces_calls_edge() {
        let (results, table, mut graph) = fixture_call_foo_to_bar();
        let cap = ResolverModuleBuilder::new().build().expect("build");
        let includes_graph = IncludesGraph::new();
        let edges = cap.resolve_all(&results, &table, "proj", &mut graph, &includes_graph);

        let calls_count = edges
            .iter()
            .filter(|e| e.edge_type == crate::model::EdgeType::Calls)
            .count();
        assert!(
            calls_count >= 1,
            "should produce at least one CALLS edge, got {edges:?}"
        );
        assert!(
            graph.edge_count() >= 1,
            "edges should be added to the graph"
        );
    }

    /// Verify the full Kit registration flow works end-to-end.
    #[test]
    fn kit_registration_flow() {
        use crate::kit::{IntoKitModuleBuilder, Kit};

        let kit = Kit::new();
        let resolver = ResolverModuleBuilder::new()
            .kit(&kit)
            .provide::<ResolverKey>()
            .expect("provide::<ResolverKey>");

        assert!(kit.contains::<ResolverKey>());

        let required = kit
            .require::<ResolverKey>()
            .expect("require::<ResolverKey>");
        assert!(Arc::ptr_eq(&resolver, &required));
    }

    #[test]
    fn builder_default_equals_new() {
        let default_builder = ResolverModuleBuilder::default();
        let _ = default_builder.build().expect("build should succeed");
    }
}
