// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Symbol resolution and data-flow analysis.
//!
//! Generates fully-qualified names (ADD §7.1), maintains scope chains and
//! symbol tables, and resolves call/data-flow/FFI edges (ADR-011).
//!
//! # Modules
//!
//! - [`error`]: [`ResolveError`] and [`Result`](error::Result) alias.
//! - [`fqn`]: [`FqnGenerator`] for ADD §7.1 FQN generation.
//! - [`includes_graph`]: [`IncludesGraph`] for C++ `#include` tracking and
//!   scope-aware cross-file call resolution (BUG-C4 fix).
//! - [`scope`]: [`Scope`] and [`ScopeChain`] for nested scope resolution.
//! - [`symbol_table`]: [`SymbolEntry`], [`FileSymbolTable`],
//!   [`ProjectSymbolTable`] for symbol indexing.
//! - [`calls`]: [`CallResolver`] for resolving CALLS edges (ADR-011).
//! - [`dataflow`]: [`DataFlowResolver`] for resolving DataFlows edges
//!   (BR-TRACE-001~004).
//! - [`cross_lang`]: [`FfiResolver`] for resolving FfiCalls edges across
//!   languages (ADD §7.4, BR-TRACE-008).

pub mod calls;
pub mod capability;
pub mod module;
// Cross-language FFI resolution is only meaningful when both C and Rust are
// compiled in (Rust extern "C" -> C definitions). Gate the entire module so
// leaner builds (e.g. `--features minimal`) don't reference unavailable
// `Language::C` / `Language::Rust` variants (unified-architecture Phase 1).
#[cfg(all(feature = "lang-c", feature = "lang-rust"))]
pub mod cross_lang;
pub mod dataflow;
pub mod error;
pub mod fqn;
pub mod imports;
pub mod includes_graph;
pub mod mro;
pub mod scope;
pub mod symbol_table;
pub mod type_resolver;

pub use calls::CallResolver;
#[cfg(all(feature = "lang-c", feature = "lang-rust"))]
pub use cross_lang::{FfiResolver, MatchStrategy};
pub use dataflow::DataFlowResolver;
pub use error::{ResolveError, Result};
pub use fqn::FqnGenerator;
pub use imports::ImportResolver;
pub use includes_graph::{resolve_include, IncludesGraph};
pub use module::{ResolverModule, ResolverModuleBuilder};
pub use mro::{mro_for, MroResolver, MroStrategy};
pub use scope::{Scope, ScopeChain, ScopeContext, ScopeResolver, ScopeResolverRegistry};
pub use symbol_table::{FileSymbolTable, ProjectSymbolTable, SymbolEntry};
pub use type_resolver::TypeResolver;

use crate::ir::ExtractResult;
use crate::model::{Edge, EdgeType, Graph};

/// Edge types whose dangling targets are pruned after TypeResolver runs.
///
/// These represent type-reference relationships (inheritance/implementation/
/// usage). If the target can't be resolved to a project node, the edge is
/// noise (e.g. `impl Display for Foo` where `Display` is a std trait not
/// indexed in the project). Pruning matches gitnexus behavior.
const PRUNABLE_EDGE_TYPES: [EdgeType; 3] =
    [EdgeType::Extends, EdgeType::Implements, EdgeType::UsesType];

/// Removes type-reference edges whose targets are still dangling (not in
/// `graph.nodes`) after [`TypeResolver`] has attempted resolution.
///
/// This prunes IMPLEMENTS/Extends/UsesType edges to std/external types
/// (e.g. `impl Display for Foo`) that can't be resolved to project-defined
/// symbols, matching gitnexus behavior (only project-defined type
/// relationships are retained).
///
/// Returns the count of pruned edges.
fn prune_dangling_type_edges(graph: &mut Graph) -> usize {
    let before = graph.edge_count();
    let node_ids: std::collections::HashSet<String> = graph.nodes.keys().cloned().collect();
    prune_dangling_type_edges_vec(&mut graph.edges, &node_ids);
    before - graph.edge_count()
}

/// Prunes dangling type-reference edges from a `Vec<Edge>`.
///
/// This is the public entry point for callers that persist a separate edge
/// collection (e.g. `ResolvePhase` in `phases.rs` persists `all_edges`, not
/// `graph.edges`). The prune inside [`resolve_all`] only affects
/// `graph.edges`, so the persisted Vec must also be pruned to actually remove
/// dangling IMPLEMENTS/Extends/UsesType edges from the database.
///
/// Returns the count of pruned edges.
pub fn prune_dangling_type_edges_vec(
    edges: &mut Vec<Edge>,
    node_ids: &std::collections::HashSet<String>,
) -> usize {
    let before = edges.len();
    edges.retain(|edge| {
        !PRUNABLE_EDGE_TYPES.contains(&edge.edge_type) || node_ids.contains(&edge.target)
    });
    before - edges.len()
}

/// Builds a project-level symbol table from extraction results.
///
/// For each [`ExtractResult`], generates FQNs for all definition nodes and
/// registers them in both the file-level and project-level tables.
///
/// # Arguments
///
/// * `results` - The extraction results from the parse phase.
/// * `project` - The project name used as the FQN prefix.
///
/// # Returns
///
/// A [`ProjectSymbolTable`] containing all symbols indexed by name (global)
/// and by file path (file-scoped).
#[must_use]
pub fn build_symbol_table(results: &[ExtractResult], project: &str) -> ProjectSymbolTable {
    let mut table = ProjectSymbolTable::new();
    for result in results {
        let mut file_table = FileSymbolTable::new();
        for node in &result.nodes {
            let entity_name = &node.name;
            let language = node.language.unwrap_or(result.language);
            // Use the parser-generated qualified_name (includes disambiguator
            // like #tests) so symbol table FQNs match node IDs in the graph.
            // Falling back to FqnGenerator with None would drop the
            // disambiguator, causing edge source/target mismatch and
            // breaking delete_file_nodes_batch edge cleanup (ADR-014).
            let fqn = if node.qualified_name.is_empty() {
                FqnGenerator::generate(project, &result.file_path, entity_name, language, None)
            } else {
                node.qualified_name.clone()
            };
            let entry = SymbolEntry::new(
                entity_name.clone(),
                fqn,
                node.label,
                result.file_path.clone(),
                project,
            )
            .with_language(language)
            .with_exported(node.is_exported)
            .with_signature_opt(node.signature.clone());
            file_table.add(entry);
        }
        if !file_table.is_empty() {
            table.add_file_table(&result.file_path, file_table);
        }
    }
    table
}

/// Resolves all symbols: calls + dataflows + FFI + imports, returning resolved edges.
///
/// This is the top-level orchestration function for the resolve phase
/// (ADR-011). It runs [`CallResolver`] to produce CALLS edges,
/// [`DataFlowResolver`] to produce DataFlows edges, [`FfiResolver`] to
/// produce FfiCalls edges (ADD §7.4), [`ImportResolver`] to produce IMPORTS
/// edges (DDD §7.2), and [`TypeResolver`] to fix dangling type edges,
/// adding all resolved edges to the graph.
///
/// # Arguments
///
/// * `results` - The extraction results from the parse phase.
/// * `symbol_table` - The project-level symbol table built from `results`.
/// * `project` - The project name.
/// * `graph` - The graph to add resolved edges to.
/// * `includes_graph` - C++ `#include` graph for scope-aware call resolution
///   (BUG-C4 fix, v0.3.0). Built by `build_includes_edges` before this call.
///
/// # Returns
///
/// A vector of all resolved edges (also added to `graph`).
pub fn resolve_all(
    results: &[ExtractResult],
    symbol_table: &ProjectSymbolTable,
    project: &str,
    graph: &mut Graph,
    includes_graph: &IncludesGraph,
) -> Vec<Edge> {
    let mut edges = Vec::new();
    let call_resolver =
        CallResolver::new(symbol_table, project).with_includes_graph(includes_graph.clone());
    edges.extend(call_resolver.resolve_calls(results, graph));
    let df_resolver = DataFlowResolver::new(symbol_table, project);
    edges.extend(df_resolver.resolve_dataflows(results, graph));
    // FFI resolution requires both C and Rust to be compiled in (gated with
    // the `cross_lang` module). Skipped in leaner builds.
    #[cfg(all(feature = "lang-c", feature = "lang-rust"))]
    {
        let ffi_resolver = FfiResolver::new(symbol_table, project);
        edges.extend(ffi_resolver.resolve_ffi(results, graph));
    }
    // Import resolution creates File → File IMPORTS edges from ImportInfo
    // records extracted by the parse phase (DDD §7.2). Runs after the other
    // resolvers; needs File nodes already in the graph (created by the scope
    // phase).
    let import_resolver = ImportResolver::new(project);
    edges.extend(import_resolver.resolve_imports(results, graph));
    // Type resolution fixes dangling Extends/Implements/UsesType edges
    // (design.md H6). Runs after other resolvers so it can fix edges created
    // by the parse phase. Returns the list of fixed edges (already mutated
    // in `graph`).
    let type_resolver = TypeResolver::new(symbol_table);
    edges.extend(type_resolver.resolve_types(results, graph));
    // Prune type-reference edges that TypeResolver could not resolve (e.g.
    // std trait impls like `impl Display for Foo`). These dangling edges
    // are noise — the target doesn't exist in the project graph.
    prune_dangling_type_edges(graph);
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CallInfo, ImportInfo};
    use crate::model::{EdgeType, Language, Node, NodeLabel};

    fn make_node(name: &str, label: NodeLabel, language: Language) -> Node {
        // qualified_name left empty so build_symbol_table falls back to
        // FqnGenerator::generate (matching pre-fix behaviour for these
        // legacy tests). Tests that need a disambiguator use make_node_with_fqn.
        let mut node = Node::builder(label, name, String::new())
            .language(language)
            .build();
        node.qualified_name.clear();
        node
    }

    /// Creates a node with a proper FQN as qualified_name (matching what
    /// the parser would set), so build_symbol_table uses it directly.
    fn make_node_with_fqn(
        name: &str,
        label: NodeLabel,
        language: Language,
        file_path: &str,
        project: &str,
        disambiguator: Option<&str>,
    ) -> Node {
        let qn = FqnGenerator::generate(project, file_path, name, language, disambiguator);
        Node::builder(label, name, qn).language(language).build()
    }

    fn make_result(file_path: &str, language: Language, nodes: Vec<Node>) -> ExtractResult {
        let mut result = ExtractResult::new(file_path, language);
        result.nodes = nodes;
        result
    }

    // --- build_symbol_table ---

    #[test]
    fn build_from_empty_results() {
        let table = build_symbol_table(&[], "proj");
        assert_eq!(table.file_count(), 0);
        assert_eq!(table.symbol_count(), 0);
    }

    #[test]
    fn build_from_single_result_single_node() {
        let node = make_node("parse", NodeLabel::Function, Language::Rust);
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "myproject");
        assert_eq!(table.file_count(), 1);
        assert_eq!(table.symbol_count(), 1);
        let entries = table.lookup("parse");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].qn, "myproject.src.main.rs.parse");
        assert_eq!(entries[0].file_path, "src/main.rs");
        assert_eq!(entries[0].language, Some(Language::Rust));
    }

    #[test]
    fn build_from_multiple_results() {
        let r1 = make_result(
            "src/main.rs",
            Language::Rust,
            vec![make_node("main", NodeLabel::Function, Language::Rust)],
        );
        let r2 = make_result(
            "src/utils.rs",
            Language::Rust,
            vec![make_node("helper", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        assert_eq!(table.file_count(), 2);
        assert_eq!(table.symbol_count(), 2);
        assert_eq!(table.lookup("main")[0].qn, "proj.src.main.rs.main");
        assert_eq!(table.lookup("helper")[0].qn, "proj.src.utils.rs.helper");
    }

    #[test]
    fn build_generates_correct_fqns() {
        let r1 = make_result(
            "src/deep/file.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.qn, "proj.src.deep.file.rs.foo");
    }

    #[test]
    fn build_python_init_py_fqn() {
        let r = make_result(
            "src/pkg/__init__.py",
            Language::Python,
            vec![make_node("MyClass", NodeLabel::Class, Language::Python)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("MyClass").unwrap();
        assert_eq!(entry.qn, "proj.src.pkg.MyClass");
    }

    #[test]
    fn build_c_header_fqn() {
        let r = make_result(
            "include/header.h",
            Language::C,
            vec![make_node("MY_DEFINE", NodeLabel::Const, Language::C)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("MY_DEFINE").unwrap();
        assert_eq!(entry.qn, "proj.include.header.h.MY_DEFINE");
    }

    #[cfg(feature = "lang-fortran")]
    #[test]
    fn build_fortran_module_fqn_via_generate_for_module() {
        // Direct test of generate_for_module since build_symbol_table uses
        // generate() (top-level entities). Module-nested entities would be
        // handled by a higher-level resolver.
        let fqn =
            FqnGenerator::generate_for_module("proj", "src/mod.f90", "mymod", "my_func", None);
        assert_eq!(fqn, "proj.src.mod.f90.mymod.my_func");
    }

    #[test]
    fn build_symbols_added_to_correct_file_tables() {
        let r1 = make_result(
            "a.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let r2 = make_result(
            "b.rs",
            Language::Rust,
            vec![make_node("bar", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        assert_eq!(table.lookup_in_file("a.rs", "foo").len(), 1);
        assert!(table.lookup_in_file("a.rs", "bar").is_empty());
        assert_eq!(table.lookup_in_file("b.rs", "bar").len(), 1);
        assert!(table.lookup_in_file("b.rs", "foo").is_empty());
    }

    #[test]
    fn build_preserves_exported_flag() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .is_exported(true)
            .build();
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert!(entry.is_exported);
    }

    #[test]
    fn build_preserves_signature() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .signature("fn foo(x: i32) -> i32")
            .build();
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.signature.as_deref(), Some("fn foo(x: i32) -> i32"));
    }

    #[test]
    fn build_preserves_label() {
        let node = make_node("MyClass", NodeLabel::Class, Language::Rust);
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("MyClass").unwrap();
        assert_eq!(entry.label, NodeLabel::Class);
    }

    #[test]
    fn build_skips_empty_results() {
        let r1 = make_result("a.rs", Language::Rust, vec![]);
        let r2 = make_result(
            "b.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        assert_eq!(table.file_count(), 1);
        assert_eq!(table.symbol_count(), 1);
    }

    #[test]
    fn build_multiple_nodes_same_file() {
        let r = make_result(
            "src/main.rs",
            Language::Rust,
            vec![
                make_node("foo", NodeLabel::Function, Language::Rust),
                make_node("bar", NodeLabel::Function, Language::Rust),
                make_node("MyClass", NodeLabel::Class, Language::Rust),
            ],
        );
        let table = build_symbol_table(&[r], "proj");
        assert_eq!(table.symbol_count(), 3);
        assert_eq!(table.file_count(), 1);
        assert_eq!(table.lookup("foo").len(), 1);
        assert_eq!(table.lookup("bar").len(), 1);
        assert_eq!(table.lookup("MyClass").len(), 1);
    }

    #[test]
    fn build_cross_file_lookup() {
        let r1 = make_result(
            "a.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let r2 = make_result(
            "b.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        let results = table.lookup("foo");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn build_uses_result_language_when_node_has_none() {
        let mut node = make_node("foo", NodeLabel::Function, Language::Rust);
        node.language = None;
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.language, Some(Language::Rust));
    }

    #[test]
    fn build_ignores_imports_and_calls() {
        let mut result = ExtractResult::new("src/main.rs", Language::Rust);
        result
            .nodes
            .push(make_node("foo", NodeLabel::Function, Language::Rust));
        result.imports.push(ImportInfo {
            source_file: "std::io".to_string(),
            imported_names: vec!["println".to_string()],
            line: 1,
        });
        result.calls.push(CallInfo {
            caller_qn: Some("foo".to_string()),
            callee_name: "println".to_string(),
            line: 3,
            args: vec![],
        });
        let table = build_symbol_table(&[result], "proj");
        // Only the node should be in the symbol table.
        assert_eq!(table.symbol_count(), 1);
        assert_eq!(table.lookup("foo").len(), 1);
        assert!(table.lookup("println").is_empty());
    }

    #[test]
    fn build_typescript_fqn() {
        let r = make_result(
            "src/components/Button.tsx",
            Language::TypeScript,
            vec![make_node("Button", NodeLabel::Class, Language::TypeScript)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("Button").unwrap();
        assert_eq!(entry.qn, "proj.src.components.Button.tsx.Button");
    }

    #[test]
    fn build_normalizes_dot_slash_path() {
        let r = make_result(
            "./src/main.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.qn, "proj.src.main.rs.foo");
    }

    #[test]
    fn build_exported_lookup_works() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .is_exported(true)
            .build();
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let exported = table.lookup_exported("foo");
        assert_eq!(exported.len(), 1);
    }

    #[test]
    fn build_all_symbols_returns_all() {
        let r = make_result(
            "src/main.rs",
            Language::Rust,
            vec![
                make_node("foo", NodeLabel::Function, Language::Rust),
                make_node("bar", NodeLabel::Function, Language::Rust),
            ],
        );
        let table = build_symbol_table(&[r], "proj");
        let all = table.all_symbols();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn build_preserves_project_field() {
        let node = make_node("foo", NodeLabel::Function, Language::Rust);
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "myproject");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.project, "myproject");
    }

    // --- resolve_all orchestration ---

    #[test]
    fn resolve_all_combines_calls_and_dataflows() {
        let foo_qn = FqnGenerator::generate("proj", "a.rs", "foo", Language::Rust, None);
        let bar_qn = FqnGenerator::generate("proj", "a.rs", "bar", Language::Rust, None);
        let foo_node = Node::builder(NodeLabel::Function, "foo", foo_qn.clone())
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();
        let bar_node = Node::builder(NodeLabel::Function, "bar", bar_qn.clone())
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();

        let mut result = ExtractResult::new("a.rs", Language::Rust);
        result.nodes = vec![foo_node, bar_node];
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn.clone()),
            callee_name: "bar".to_string(),
            line: 5,
            args: vec![],
        });
        result.assignments.push(crate::parse::AssignInfo {
            target_name: "x".to_string(),
            source_name: "bar".to_string(),
            line: 6,
            is_return_assign: true,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        // Add nodes to graph with qn as id.
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

        let edges = resolve_all(&results, &table, "proj", &mut graph, &IncludesGraph::new());

        // Should have 1 CALLS edge + 1 DataFlows edge = 2 total.
        assert_eq!(edges.len(), 2);
        assert_eq!(graph.edge_count(), 2);

        let calls_count = edges
            .iter()
            .filter(|e| e.edge_type == crate::model::EdgeType::Calls)
            .count();
        let dataflows_count = edges
            .iter()
            .filter(|e| e.edge_type == crate::model::EdgeType::DataFlows)
            .count();
        assert_eq!(calls_count, 1);
        assert_eq!(dataflows_count, 1);

        // Verify CALLS edge: foo -> bar
        let call_edge = edges
            .iter()
            .find(|e| e.edge_type == crate::model::EdgeType::Calls)
            .unwrap();
        assert_eq!(call_edge.source, foo_qn);
        assert_eq!(call_edge.target, bar_qn);
    }

    #[test]
    fn resolve_all_empty_results_returns_empty() {
        let table = ProjectSymbolTable::new();
        let mut graph = Graph::new();
        let edges = resolve_all(&[], &table, "proj", &mut graph, &IncludesGraph::new());
        assert!(edges.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_all_adds_edges_to_graph() {
        let foo_qn = FqnGenerator::generate("proj", "a.rs", "foo", Language::Rust, None);
        let foo_node = Node::builder(NodeLabel::Function, "foo", foo_qn.clone())
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();

        let mut result = ExtractResult::new("a.rs", Language::Rust);
        result.nodes = vec![foo_node];
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn.clone()),
            callee_name: "foo".to_string(),
            line: 5,
            args: vec![],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
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

        resolve_all(&results, &table, "proj", &mut graph, &IncludesGraph::new());

        // The self-call edge should be in the graph.
        assert_eq!(graph.edge_count(), 1);
        let neighbors = graph.neighbors(&foo_qn, Some(crate::model::EdgeType::Calls));
        assert_eq!(neighbors.len(), 1);
    }

    // --- Bug 1: build_symbol_table must use node.qualified_name (with
    // disambiguator) so symbol table FQNs match node IDs in the graph. ---

    #[test]
    fn build_symbol_table_uses_qualified_name_with_disambiguator() {
        // A function inside `mod tests` gets a #tests disambiguator in its
        // qualified_name (set by the parser). build_symbol_table must use
        // that qualified_name, not regenerate without the disambiguator.
        let node = make_node_with_fqn(
            "my_test",
            NodeLabel::Function,
            Language::Rust,
            "src/lib.rs",
            "proj",
            Some("tests"),
        );
        // The qualified_name should include #tests.
        assert_eq!(
            node.qualified_name, "proj.src.lib.rs.my_test#tests",
            "test setup: qualified_name must include disambiguator"
        );

        let result = make_result("src/lib.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("my_test").unwrap();
        // The symbol table FQN must match the node's qualified_name.
        assert_eq!(
            entry.qn, "proj.src.lib.rs.my_test#tests",
            "symbol table FQN must include disambiguator (matches node ID)"
        );
    }

    #[test]
    fn build_symbol_table_qualified_name_matches_node_id_for_deletion() {
        // Regression: when a node has a disambiguator, the symbol table FQN
        // must match the node ID so delete_file_nodes_batch can find edges.
        let node = make_node_with_fqn(
            "helper",
            NodeLabel::Function,
            Language::Rust,
            "src/util.rs",
            "proj",
            Some("impl"),
        );
        let expected_id = node.qualified_name.clone();
        let result = make_result("src/util.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("helper").unwrap();
        assert_eq!(
            entry.qn, expected_id,
            "FQN must match node ID for edge cleanup to work"
        );
    }

    // --- C1: prune unresolvable dangling type edges ---

    #[test]
    fn resolve_all_prunes_unresolvable_dangling_type_edges() {
        // C1 fix: std trait impls (Display, Debug, etc.) create dangling
        // IMPLEMENTS edges that TypeResolver can't resolve. These should be
        // pruned to match gitnexus behavior (only project-defined type
        // relationships). Resolvable edges (project-defined traits) must be
        // kept and resolved.
        let trait_qn = FqnGenerator::generate("proj", "a.rs", "MyTrait", Language::Rust, None);
        let trait_node = Node::builder(NodeLabel::Trait, "MyTrait", trait_qn.clone())
            .id(trait_qn.clone())
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();

        let foo_qn = FqnGenerator::generate("proj", "a.rs", "Foo", Language::Rust, None);
        let foo_node = Node::builder(NodeLabel::Struct, "Foo", foo_qn.clone())
            .id(foo_qn.clone())
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();

        let bar_qn = FqnGenerator::generate("proj", "b.rs", "Bar", Language::Rust, None);
        let bar_node = Node::builder(NodeLabel::Struct, "Bar", bar_qn.clone())
            .id(bar_qn.clone())
            .language(Language::Rust)
            .file_path("b.rs")
            .is_exported(true)
            .build();

        let result_a = make_result(
            "a.rs",
            Language::Rust,
            vec![trait_node.clone(), foo_node.clone()],
        );
        let result_b = make_result("b.rs", Language::Rust, vec![bar_node.clone()]);
        let results = vec![result_a, result_b];
        let table = build_symbol_table(&results, "proj");

        let mut graph = Graph::new();
        graph.add_node(trait_node);
        graph.add_node(foo_node);
        graph.add_node(bar_node);

        // Dangling: Foo implements Display (std trait, not in project)
        graph.add_edge(Edge::new(
            &foo_qn,
            "proj.a.rs.Display",
            EdgeType::Implements,
            "proj",
        ));
        // Resolvable: Bar implements MyTrait (in a.rs, TypeResolver should fix)
        graph.add_edge(Edge::new(
            &bar_qn,
            "proj.b.rs.MyTrait",
            EdgeType::Implements,
            "proj",
        ));

        assert_eq!(graph.edge_count(), 2, "precondition: 2 IMPLEMENTS edges");

        resolve_all(&results, &table, "proj", &mut graph, &IncludesGraph::new());

        let implements_edges: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(
            implements_edges.len(),
            1,
            "dangling IMPLEMENTS edge should be pruned"
        );
        assert_eq!(
            implements_edges[0].target, trait_qn,
            "resolved edge target should be MyTrait"
        );
    }

    #[test]
    fn resolve_all_prunes_unresolvable_extends_and_uses_type() {
        // Same pruning applies to Extends and UsesType edges.
        let struct_qn = FqnGenerator::generate("proj", "a.rs", "Foo", Language::Rust, None);
        let struct_node = Node::builder(NodeLabel::Struct, "Foo", struct_qn.clone())
            .id(struct_qn.clone())
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();

        let result = make_result("a.rs", Language::Rust, vec![struct_node.clone()]);
        let results = vec![result];
        let table = build_symbol_table(&results, "proj");

        let mut graph = Graph::new();
        graph.add_node(struct_node);

        graph.add_edge(Edge::new(
            &struct_qn,
            "proj.a.rs.BaseClass",
            EdgeType::Extends,
            "proj",
        ));
        graph.add_edge(Edge::new(
            &struct_qn,
            "proj.a.rs.ExternalType",
            EdgeType::UsesType,
            "proj",
        ));

        assert_eq!(graph.edge_count(), 2, "precondition: 2 dangling type edges");

        resolve_all(&results, &table, "proj", &mut graph, &IncludesGraph::new());

        assert_eq!(
            graph.edge_count(),
            0,
            "both dangling type edges should be pruned"
        );
    }

    #[test]
    fn resolve_all_keeps_non_type_edges_with_dangling_targets() {
        // CALLS edges with dangling targets should NOT be pruned — only
        // type-reference edges (Implements/Extends/UsesType) are pruned.
        let foo_qn = FqnGenerator::generate("proj", "a.rs", "foo", Language::Rust, None);
        let foo_node = Node::builder(NodeLabel::Function, "foo", foo_qn.clone())
            .id(foo_qn.clone())
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();

        let result = make_result("a.rs", Language::Rust, vec![foo_node.clone()]);
        let results = vec![result];
        let table = build_symbol_table(&results, "proj");

        let mut graph = Graph::new();
        graph.add_node(foo_node);

        graph.add_edge(Edge::new(
            &foo_qn,
            "proj.a.rs.external_fn",
            EdgeType::Calls,
            "proj",
        ));

        resolve_all(&results, &table, "proj", &mut graph, &IncludesGraph::new());

        let calls_count = graph
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Calls)
            .count();
        assert_eq!(
            calls_count, 1,
            "CALLS edge with dangling target should NOT be pruned"
        );
    }

    // --- C1 fix: prune persisted edge Vec (not just graph.edges) ---
    // The caller (phases.rs ResolvePhase) persists `all_edges` (a Vec<Edge>),
    // not `graph.edges`. The prune inside resolve_all only affects
    // graph.edges, so the persisted Vec must also be pruned.

    #[test]
    fn prune_dangling_type_edges_vec_removes_dangling_type_edges() {
        let mut edges = vec![
            // dangling: target "Display" not in node_ids
            Edge::new(
                "proj.a.rs.Foo",
                "proj.a.rs.Display",
                EdgeType::Implements,
                "proj",
            ),
            // resolvable: target "MyTrait" in node_ids
            Edge::new(
                "proj.a.rs.Bar",
                "proj.a.rs.MyTrait",
                EdgeType::Implements,
                "proj",
            ),
            // non-type edge with dangling target — must NOT be pruned
            Edge::new("proj.a.rs.foo", "proj.a.rs.bar", EdgeType::Calls, "proj"),
        ];
        let mut node_ids = std::collections::HashSet::new();
        node_ids.insert("proj.a.rs.MyTrait".to_string());

        let pruned = prune_dangling_type_edges_vec(&mut edges, &node_ids);
        assert_eq!(pruned, 1, "1 dangling IMPLEMENTS edge should be pruned");
        assert_eq!(
            edges.len(),
            2,
            "2 edges remain (resolvable Implements + Calls)"
        );
        let implements: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(implements.len(), 1);
        assert_eq!(implements[0].target, "proj.a.rs.MyTrait");
    }

    #[test]
    fn prune_dangling_type_edges_vec_keeps_extends_and_uses_type_when_resolved() {
        let mut edges = vec![
            Edge::new("a", "proj.a.rs.Base", EdgeType::Extends, "proj"), // resolvable
            Edge::new("b", "proj.a.rs.Unknown", EdgeType::UsesType, "proj"), // dangling
        ];
        let mut node_ids = std::collections::HashSet::new();
        node_ids.insert("proj.a.rs.Base".to_string());

        let pruned = prune_dangling_type_edges_vec(&mut edges, &node_ids);
        assert_eq!(
            pruned, 1,
            "only the dangling UsesType edge should be pruned"
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::Extends);
    }
}
