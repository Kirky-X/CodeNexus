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
//! - [`scope`]: [`Scope`] and [`ScopeChain`] for nested scope resolution.
//! - [`symbol_table`]: [`SymbolEntry`], [`FileSymbolTable`],
//!   [`ProjectSymbolTable`] for symbol indexing.
//! - [`calls`]: [`CallResolver`] for resolving CALLS edges (ADR-011).
//! - [`dataflow`]: [`DataFlowResolver`] for resolving DataFlows edges
//!   (BR-TRACE-001~004).
//! - [`cross_lang`]: [`FfiResolver`] for resolving FfiCalls edges across
//!   languages (ADD §7.4, BR-TRACE-008).

pub mod capability;
pub mod module;
pub mod calls;
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
pub use mro::{mro_for, MroResolver, MroStrategy};
pub use scope::{Scope, ScopeChain, ScopeContext, ScopeResolver, ScopeResolverRegistry};
pub use symbol_table::{FileSymbolTable, ProjectSymbolTable, SymbolEntry};
pub use type_resolver::TypeResolver;
pub use module::{ResolverModule, ResolverModuleBuilder};

use crate::ir::ExtractResult;
use crate::model::{Edge, Graph};

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
            let fqn = FqnGenerator::generate(project, &result.file_path, entity_name, language, None);
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
///
/// # Returns
///
/// A vector of all resolved edges (also added to `graph`).
pub fn resolve_all(
    results: &[ExtractResult],
    symbol_table: &ProjectSymbolTable,
    project: &str,
    graph: &mut Graph,
) -> Vec<Edge> {
    let mut edges = Vec::new();
    let call_resolver = CallResolver::new(symbol_table, project);
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
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::ir::{CallInfo, ImportInfo};

    fn make_node(name: &str, label: NodeLabel, language: Language) -> Node {
        Node::builder(label, name, "placeholder")
            .language(language)
            .build()
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
        let fqn = FqnGenerator::generate_for_module("proj", "src/mod.f90", "mymod", "my_func", None);
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
                let qn = FqnGenerator::generate("proj", &r.file_path, &node.name, Language::Rust, None);
                let mut g = node.clone();
                g.id = qn.clone();
                g.qualified_name = qn;
                graph.add_node(g);
            }
        }

        let edges = resolve_all(&results, &table, "proj", &mut graph);

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
        let edges = resolve_all(&[], &table, "proj", &mut graph);
        assert!(edges.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_all_adds_edges_to_graph() {
        let foo_node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .file_path("a.rs")
            .is_exported(true)
            .build();
        let foo_qn = FqnGenerator::generate("proj", "a.rs", "foo", Language::Rust, None);

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
                let qn = FqnGenerator::generate("proj", &r.file_path, &node.name, Language::Rust, None);
                let mut g = node.clone();
                g.id = qn.clone();
                g.qualified_name = qn;
                graph.add_node(g);
            }
        }

        resolve_all(&results, &table, "proj", &mut graph);

        // The self-call edge should be in the graph.
        assert_eq!(graph.edge_count(), 1);
        let neighbors = graph.neighbors(&foo_qn, Some(crate::model::EdgeType::Calls));
        assert_eq!(neighbors.len(), 1);
    }
}
