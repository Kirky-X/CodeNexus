// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Resolver subsystem (T6/unified-architecture
//! Phase 2, Task 2.8; v0.3.3 AsyncKit migration).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`ResolverModule`],
//! wiring the existing free functions [`build_symbol_table`](super::build_symbol_table)
//! and [`resolve_all`](super::resolve_all) into the unified Kit registry as
//! `Arc<dyn Resolver>` under [`ResolverModule`](crate::kit::ResolverModule).

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

use super::capability::Resolver;
use super::error::ResolveError;
use super::includes_graph::IncludesGraph;
use super::symbol_table::ProjectSymbolTable;
use crate::ir::ExtractResult;
use crate::model::{Edge, Graph};

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Resolver subsystem (Task 2.8).
///
/// Zero-sized marker — construction logic lives in
/// [`ResolverModule::build_cap`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, ResolverModule};
///
/// let mut kit = AsyncKit::new();
/// kit.register::<ResolverModule>()?;
/// let kit = kit.build().await?;
/// let resolver = kit.require::<ResolverModule>()?;
/// ```
pub struct ResolverModule;

impl ModuleMeta for ResolverModule {
    const NAME: &'static str = "resolver";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for ResolverModule {
    type Capability = Arc<dyn Resolver>;
    type Error = ResolveError;

    fn build<'a>(
        _kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move { Self::build_cap() })
    }
}

impl ResolverModule {
    /// Constructs a ResolverCapability.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests.
    pub(crate) fn build_cap() -> Result<Arc<dyn Resolver>, ResolveError> {
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
    use crate::kit::{AsyncKit, ResolverModule};
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
        let _ = bar_qn;
        (results, table, graph)
    }

    #[test]
    fn build_returns_capability() {
        let cap = ResolverModule::build_cap().expect("build_cap");
        let table = cap.build_symbol_table(&[], "proj");
        assert_eq!(table.symbol_count(), 0, "empty results → empty table");
        assert_eq!(table.file_count(), 0);
    }

    #[test]
    fn capability_build_symbol_table_returns_table() {
        let (results, _table, _graph) = fixture_call_foo_to_bar();
        let cap = ResolverModule::build_cap().expect("build_cap");
        let table = cap.build_symbol_table(&results, "proj");
        assert_eq!(table.symbol_count(), 2, "foo + bar");
        assert_eq!(table.file_count(), 1);
        assert!(table.lookup_exact("foo").is_some());
        assert!(table.lookup_exact("bar").is_some());
    }

    #[test]
    fn capability_resolve_all_produces_calls_edge() {
        let (results, table, mut graph) = fixture_call_foo_to_bar();
        let cap = ResolverModule::build_cap().expect("build_cap");
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

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.register::<ResolverModule>()
            .expect("register::<ResolverModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<ResolverModule>());

        let required = kit
            .require::<ResolverModule>()
            .expect("require::<ResolverModule>");
        let table = required.build_symbol_table(&[], "proj");
        assert_eq!(table.symbol_count(), 0);
    }
}
