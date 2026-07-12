// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Cross-file type FQN resolution (design.md H6).
//!
//! Provides [`TypeResolver`] for resolving dangling type-reference edges
//! (`Extends`/`Implements`/`UsesType`) to their actual cross-file definition
//! FQNs. The parse phase creates these edges with best-effort FQNs constructed
//! from the current file's path/scope; for cross-file types (e.g.
//! `from foo import Bar; class Child(Bar)`), the target FQN won't match the
//! actual `Bar` node in `foo.py`, leaving the edge dangling. The
//! [`TypeResolver`] fixes these dangling edges by re-resolving the type name
//! against the symbol table using file-level, import, and project-level
//! lookup (same strategy as [`CallResolver`], ADR-011).
//!
//! # Per-language coverage
//!
//! - **C**: `#include "header.h"` brings typedef/macro types into scope; the
//!   resolver matches type names against all symbols from included files
//!   (project-level exported lookup covers this because C headers define
//!   exported symbols).
//! - **Rust**: `use crate::module::MyType;` creates an import binding; the
//!   resolver matches `MyType` against the imported names, resolving to
//!   `crate.module.MyType`. `impl Trait for Type` and `#[derive(Trait)]`
//!   edges are fixed the same way (the trait/type name is resolved via
//!   imports).
//! - **Python**: `from foo import Bar` — import-based lookup resolves `Bar`
//!   to `proj.foo.Bar`.
//! - **TypeScript**: `import { Foo } from './foo'` — same import-based
//!   resolution.
//! - **Fortran**: no inheritance semantics (MroStrategy::None); edges are
//!   still resolved if present but typically absent.
//!
//! # Lightweight design
//!
//! This resolver does NOT implement a full type system (no inference, no
//! generic substitution, no associated type resolution). It performs
//! name-based resolution only: extract the type name (last component of the
//! dangling FQN), look it up in the symbol table, and update the edge target.
//!
//! [`CallResolver`]: crate::resolve::CallResolver

use std::collections::HashMap;

use crate::ir::{ExtractResult, ImportInfo};
use crate::model::{ConfidenceTier, Edge, EdgeType, Graph};
use crate::resolve::ProjectSymbolTable;

/// Confidence for a file-level (same-file) type match.
const CONFIDENCE_EXACT: f32 = 0.95;
/// Confidence for an import-based type match.
const CONFIDENCE_IMPORT: f32 = 0.90;
/// Confidence for a project-level (exported) type match.
const CONFIDENCE_PROJECT: f32 = 0.80;

/// Edge types whose targets may need cross-file type FQN resolution.
const RESOLVABLE_EDGE_TYPES: [EdgeType; 3] =
    [EdgeType::Extends, EdgeType::Implements, EdgeType::UsesType];

/// Resolves dangling type-reference edges to their actual cross-file FQNs
/// (design.md H6).
///
/// Construct with [`TypeResolver::new`] passing a reference to the
/// [`ProjectSymbolTable`], then call
/// [`resolve_types`](Self::resolve_types) to fix dangling edges in the graph.
pub struct TypeResolver<'a> {
    symbol_table: &'a ProjectSymbolTable,
}

impl<'a> TypeResolver<'a> {
    /// Creates a new `TypeResolver` with the given symbol table.
    #[must_use]
    pub fn new(symbol_table: &'a ProjectSymbolTable) -> Self {
        Self { symbol_table }
    }

    /// Resolves a type name to its FQN, considering the file and its imports.
    ///
    /// Resolution order (same as [`CallResolver`](crate::resolve::CallResolver)):
    ///
    /// 1. **File-level lookup** (confidence 0.95, `SameFile`): the type is
    ///    defined in the same file as the reference.
    /// 2. **Import-based lookup** (confidence 0.90, `ImportScoped`): the type
    ///    name appears in the file's import list, and a matching symbol exists
    ///    in the project table.
    /// 3. **Project-level exported lookup** (confidence 0.80, `Global`): the
    ///    type is an exported symbol somewhere in the project.
    ///
    /// Returns `None` if the type cannot be resolved.
    #[must_use]
    pub fn resolve_type(
        &self,
        user_file: &str,
        type_name: &str,
        imports: &[ImportInfo],
    ) -> Option<(String, f32, ConfidenceTier)> {
        // 1. File-level lookup (same file).
        // Single-line for coverage: tarpaulin attribute continuation
        if let Some(entry) = self
            .symbol_table
            .lookup_in_file(user_file, type_name)
            .first()
        {
            return Some((entry.qn.clone(), CONFIDENCE_EXACT, ConfidenceTier::SameFile));
        }
        // 2. Import-based lookup.
        // Single-line for coverage: tarpaulin attribute continuation
        let is_imported = imports
            .iter()
            .any(|imp| imp.imported_names.iter().any(|n| n == type_name));
        if is_imported {
            if let Some(entry) = self.symbol_table.lookup(type_name).first() {
                // Single-line for coverage: tarpaulin attribute continuation
                return Some((
                    entry.qn.clone(),
                    CONFIDENCE_IMPORT,
                    ConfidenceTier::ImportScoped,
                ));
            }
        }
        // 3. Project-level exported lookup.
        if let Some(entry) = self.symbol_table.lookup_exported(type_name).first() {
            return Some((entry.qn.clone(), CONFIDENCE_PROJECT, ConfidenceTier::Global));
        }
        None
    }

    /// Resolves dangling `Extends`/`Implements`/`UsesType` edges in the graph.
    ///
    /// For each edge of a resolvable type whose target FQN does NOT correspond
    /// to an existing graph node (i.e. the edge is dangling), this method:
    ///
    /// 1. Extracts the type name (last `.`-separated component of the target
    ///    FQN).
    /// 2. Looks up the source node's `file_path` from the graph.
    /// 3. Retrieves the imports for that file from `results`.
    /// 4. Calls [`resolve_type`](Self::resolve_type) to find the real FQN.
    /// 5. If resolved, updates the edge's `target`, `confidence`, and
    ///    `confidence_tier` in-place.
    ///
    /// Edges whose target already exists in the graph are left unchanged.
    ///
    /// # Arguments
    ///
    /// * `results` - Extraction results (used to build a file → imports map).
    /// * `graph` - The graph whose dangling edges to fix (mutated in-place).
    ///
    /// # Returns
    ///
    /// A vector of the resolved (fixed) edges. These are the same edges that
    /// were mutated in `graph` — returned for logging/reporting purposes.
    pub fn resolve_types(&self, results: &[ExtractResult], graph: &mut Graph) -> Vec<Edge> {
        // In production, `ExtractResult.file_path` is absolute (set by
        // `extract_file` from `file.path`), while graph nodes' `file_path` is
        // relative (normalized by `ScopeResolutionPhase`, phases.rs:344-346).
        // This path-format mismatch causes `imports_map.get(source_file)` and
        // `symbol_table.lookup_in_file(source_file, ...)` to miss, leaving all
        // Extends/Implements/UsesType edges unresolved (confidence stuck at
        // 1.0). Build a bidirectional path mapping by matching result nodes'
        // `qualified_name` to graph nodes' `qualified_name` (both are FQNs
        // generated from the absolute path during the parse phase, so they
        // match even though `file_path` differs).
        let mut result_to_graph_fp: HashMap<&str, &str> = HashMap::new();
        for result in results {
            // Skip if we already mapped this file.
            if result_to_graph_fp.contains_key(result.file_path.as_str()) {
                continue;
            }
            for node in &result.nodes {
                if let Some(graph_node) = graph.nodes.get(&node.qualified_name) {
                    if let Some(graph_fp) = graph_node.file_path.as_deref() {
                        // Single-line for coverage: tarpaulin attribute continuation
                        result_to_graph_fp.insert(result.file_path.as_str(), graph_fp);
                        break;
                    }
                }
            }
        }
        // Reverse mapping: graph file_path (relative) → result file_path
        // (absolute), used to translate `source_file` back to the format
        // expected by `symbol_table.lookup_in_file`.
        // Single-line for coverage: tarpaulin attribute continuation
        let graph_to_result_fp: HashMap<&str, &str> =
            result_to_graph_fp.iter().map(|(k, v)| (*v, *k)).collect();

        // Build file_path → imports map, keyed by GRAPH file_path (relative)
        // so it matches `fqn_to_file` values.
        // Single-line for coverage: tarpaulin attribute continuation
        let imports_map: HashMap<&str, &[ImportInfo]> = results
            .iter()
            .map(|r| {
                let key = result_to_graph_fp
                    .get(r.file_path.as_str())
                    .copied()
                    .unwrap_or(r.file_path.as_str());
                (key, r.imports.as_slice())
            })
            .collect();

        // Build fqn → file_path map from graph nodes (for source lookup).
        // Single-line for coverage: tarpaulin attribute continuation
        let fqn_to_file: HashMap<&str, &str> = graph
            .nodes
            .values()
            .filter_map(|n| {
                n.file_path
                    .as_deref()
                    .map(|fp| (n.qualified_name.as_str(), fp))
            })
            .collect();

        let mut resolved_edges = Vec::new();
        for edge in &mut graph.edges {
            // Only fix resolvable edge types.
            if !RESOLVABLE_EDGE_TYPES.contains(&edge.edge_type) {
                continue;
            }
            // Skip if target FQN already matches a real node (not dangling).
            if graph.nodes.contains_key(&edge.target) {
                continue;
            }
            // Get the source node's file path (relative, from graph).
            // Single-line for coverage: tarpaulin attribute continuation
            let Some(&source_file) = fqn_to_file.get(edge.source.as_str()) else {
                continue;
            };
            // Extract the type name from the dangling FQN (last component).
            let type_name = edge.target.rsplit('.').next().unwrap_or(&edge.target);
            // Retrieve imports for this file (imports_map keyed by relative
            // path, matching source_file).
            let imports = imports_map.get(source_file).copied().unwrap_or(&[]);
            // Translate relative source_file back to absolute for symbol table
            // lookup (file table keyed by result.file_path = absolute).
            // Single-line for coverage: tarpaulin attribute continuation
            let lookup_file = graph_to_result_fp
                .get(source_file)
                .copied()
                .unwrap_or(source_file);
            // Attempt resolution.
            // Single-line for coverage: tarpaulin attribute continuation
            let Some((resolved_qn, confidence, tier)) =
                self.resolve_type(lookup_file, type_name, imports)
            else {
                continue;
            };
            // Skip if the resolved FQN is the same as the current target
            // (no change needed — shouldn't happen since target is dangling,
            // but guard against edge cases where the FQN generator produced
            // the correct string but the node just isn't in the graph).
            if resolved_qn == edge.target {
                continue;
            }
            // Update the edge in-place.
            edge.target = resolved_qn;
            edge.confidence = confidence;
            edge.confidence_tier = tier;
            resolved_edges.push(edge.clone());
        }
        resolved_edges
    }
}

#[cfg(all(test, feature = "lang-rust", feature = "lang-python"))]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Graph, Language, Node, NodeLabel};
    use crate::resolve::build_symbol_table;

    // FQN format: proj.{file_path_with_dots}.{name} (ADR-001: extension retained)
    // e.g. a.py + A → proj.a.py.A

    // --- helpers ---

    fn make_class(name: &str, file: &str, lang: Language) -> Node {
        let qn = format!("proj.{file}.{name}");
        Node::builder(NodeLabel::Class, name, qn)
            .file_path(file)
            .language(lang)
            .project("proj")
            .is_exported(true)
            .build()
    }

    fn make_function(name: &str, file: &str, lang: Language) -> Node {
        let qn = format!("proj.{file}.{name}");
        Node::builder(NodeLabel::Function, name, qn)
            .file_path(file)
            .language(lang)
            .project("proj")
            .build()
    }

    fn add_extends(graph: &mut Graph, source: &str, target: &str) {
        graph.add_edge(Edge::new(source, target, EdgeType::Extends, "proj"));
    }

    fn add_implements(graph: &mut Graph, source: &str, target: &str) {
        graph.add_edge(Edge::new(source, target, EdgeType::Implements, "proj"));
    }

    fn build_results_and_table(
        nodes: Vec<(Node, &str)>,
    ) -> (Vec<ExtractResult>, ProjectSymbolTable) {
        let mut results: std::collections::HashMap<&str, ExtractResult> =
            std::collections::HashMap::new();
        for (node, file) in nodes {
            let lang = node.language.unwrap_or(Language::Python);
            let result = results
                .entry(file)
                .or_insert_with(|| ExtractResult::new(file, lang));
            result.push_node(node);
        }
        let results_vec: Vec<ExtractResult> = results.into_values().collect();
        let table = build_symbol_table(&results_vec, "proj");
        (results_vec, table)
    }

    // --- resolve_type ---

    #[test]
    fn resolve_type_same_file() {
        let class = make_class("A", "a.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class, "a.py")]);
        let resolver = TypeResolver::new(&table);
        let (qn, conf, tier) = resolver
            .resolve_type("a.py", "A", &[])
            .expect("should resolve");
        assert_eq!(qn, "proj.a.py.A");
        assert!((conf - 0.95).abs() < f32::EPSILON);
        assert_eq!(tier, ConfidenceTier::SameFile);
        let _ = &results;
    }

    #[test]
    fn resolve_type_import_scoped() {
        let class_a = make_class("A", "a.py", Language::Python);
        let class_b = make_class("B", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);
        let imports = vec![ImportInfo {
            source_file: "a".to_string(),
            imported_names: vec!["A".to_string()],
            line: 1,
        }];
        let resolver = TypeResolver::new(&table);
        let (qn, conf, tier) = resolver
            .resolve_type("b.py", "A", &imports)
            .expect("should resolve via import");
        assert_eq!(qn, "proj.a.py.A");
        assert!((conf - 0.90).abs() < f32::EPSILON);
        assert_eq!(tier, ConfidenceTier::ImportScoped);
        let _ = &results;
    }

    #[test]
    fn resolve_type_global_exported() {
        let class_a = make_class("A", "a.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_a, "a.py")]);
        let resolver = TypeResolver::new(&table);
        // No imports — should fall through to project-level exported lookup.
        let (qn, conf, tier) = resolver
            .resolve_type("b.py", "A", &[])
            .expect("should resolve via project export");
        assert_eq!(qn, "proj.a.py.A");
        assert!((conf - 0.80).abs() < f32::EPSILON);
        assert_eq!(tier, ConfidenceTier::Global);
        let _ = &results;
    }

    #[test]
    fn resolve_type_not_found() {
        let (results, table) = build_results_and_table(vec![]);
        let resolver = TypeResolver::new(&table);
        assert!(resolver.resolve_type("a.py", "Nonexistent", &[]).is_none());
        let _ = &results;
    }

    // --- resolve_types: dangling Extends edge fix ---

    #[test]
    fn resolve_types_fixes_dangling_extends_edge() {
        // File a.py defines class A (FQN: proj.a.py.A).
        // File b.py defines class B (FQN: proj.b.py.B) that extends A.
        // The parse phase creates B -> proj.b.py.A (dangling, wrong file).
        // The TypeResolver should fix it to B -> proj.a.py.A.
        let class_a = make_class("A", "a.py", Language::Python);
        let class_b = make_class("B", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("A", "a.py", Language::Python));
        graph.add_node(make_class("B", "b.py", Language::Python));
        // Dangling edge: B extends proj.b.py.A (wrong — A is in a.py).
        add_extends(&mut graph, "proj.b.py.B", "proj.b.py.A");

        // Add import to b.py: from a import A.
        let mut results_with_imports = results;
        for r in &mut results_with_imports {
            if r.file_path == "b.py" {
                r.imports.push(ImportInfo {
                    source_file: "a".to_string(),
                    imported_names: vec!["A".to_string()],
                    line: 1,
                });
            }
        }

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results_with_imports, &mut graph);

        assert_eq!(fixed.len(), 1);
        let edge = &graph.edges[0];
        assert_eq!(edge.target, "proj.a.py.A");
        assert!((edge.confidence - 0.90).abs() < f32::EPSILON);
        assert_eq!(edge.confidence_tier, ConfidenceTier::ImportScoped);
    }

    #[test]
    fn resolve_types_skips_non_dangling_edges() {
        // Edge target exists in the graph — should not be touched.
        let class_a = make_class("A", "a.py", Language::Python);
        let class_b = make_class("B", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("A", "a.py", Language::Python));
        graph.add_node(make_class("B", "b.py", Language::Python));
        // Non-dangling: target proj.a.py.A exists.
        add_extends(&mut graph, "proj.b.py.B", "proj.a.py.A");

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        assert!(fixed.is_empty());
        // Edge unchanged.
        assert_eq!(graph.edges[0].target, "proj.a.py.A");
    }

    #[test]
    fn resolve_types_skips_non_resolvable_edge_types() {
        let func = make_function("foo", "a.py", Language::Python);
        let class = make_class("A", "a.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(func, "a.py"), (class, "a.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_function("foo", "a.py", Language::Python));
        graph.add_node(make_class("A", "a.py", Language::Python));
        // Calls edge — not resolvable by TypeResolver.
        graph.add_edge(Edge::new(
            "proj.a.py.foo",
            "proj.a.py.Nonexistent",
            EdgeType::Calls,
            "proj",
        ));

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        assert!(fixed.is_empty());
    }

    #[test]
    fn resolve_types_fixes_dangling_implements_edge() {
        let trait_def = make_class("MyTrait", "a.rs", Language::Rust);
        let impl_class = make_class("MyImpl", "b.rs", Language::Rust);
        let (results, table) =
            build_results_and_table(vec![(trait_def, "a.rs"), (impl_class, "b.rs")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("MyTrait", "a.rs", Language::Rust));
        graph.add_node(make_class("MyImpl", "b.rs", Language::Rust));
        // Dangling: MyImpl implements proj.b.rs.MyTrait (wrong file).
        add_implements(&mut graph, "proj.b.rs.MyImpl", "proj.b.rs.MyTrait");

        let mut results_with_imports = results;
        for r in &mut results_with_imports {
            if r.file_path == "b.rs" {
                r.imports.push(ImportInfo {
                    source_file: "a".to_string(),
                    imported_names: vec!["MyTrait".to_string()],
                    line: 1,
                });
            }
        }

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results_with_imports, &mut graph);
        assert_eq!(fixed.len(), 1);
        assert_eq!(graph.edges[0].target, "proj.a.rs.MyTrait");
        assert_eq!(graph.edges[0].edge_type, EdgeType::Implements);
    }

    #[test]
    fn resolve_types_unresolvable_edge_left_unchanged() {
        // Dangling edge whose type name doesn't exist anywhere — left as-is.
        let class_b = make_class("B", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_b, "b.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("B", "b.py", Language::Python));
        // Dangling: target Nonexistent doesn't exist anywhere.
        add_extends(&mut graph, "proj.b.py.B", "proj.b.py.Nonexistent");

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        assert!(fixed.is_empty());
        // Edge unchanged (still dangling).
        assert_eq!(graph.edges[0].target, "proj.b.py.Nonexistent");
    }

    #[test]
    fn resolve_types_global_fallback_when_no_imports() {
        // Dangling edge with no imports — should use project-level exported
        // lookup.
        let class_a = make_class("A", "a.py", Language::Python);
        let class_b = make_class("B", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("A", "a.py", Language::Python));
        graph.add_node(make_class("B", "b.py", Language::Python));
        // Dangling: B extends proj.b.py.A (wrong — A is in a.py, no import).
        add_extends(&mut graph, "proj.b.py.B", "proj.b.py.A");

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        assert_eq!(fixed.len(), 1);
        assert_eq!(graph.edges[0].target, "proj.a.py.A");
        assert!((graph.edges[0].confidence - 0.80).abs() < f32::EPSILON);
        assert_eq!(graph.edges[0].confidence_tier, ConfidenceTier::Global);
    }

    #[test]
    fn resolve_types_skips_edge_when_source_node_missing() {
        // Source FQN not in graph — can't determine file path — skip.
        let class_a = make_class("A", "a.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_a, "a.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("A", "a.py", Language::Python));
        // Source proj.unknown.X doesn't exist in graph.
        add_extends(&mut graph, "proj.unknown.X", "proj.unknown.Y");

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        assert!(fixed.is_empty());
    }

    #[test]
    fn resolve_types_skips_when_resolved_equals_current_target() {
        // Dangling target whose type name doesn't exist in the symbol table
        // — resolve_type returns None, edge unchanged.
        let class_b = make_class("B", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_b, "b.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("B", "b.py", Language::Python));
        add_extends(&mut graph, "proj.b.py.B", "proj.b.py.Nonexistent");

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        assert!(fixed.is_empty());
    }

    #[test]
    fn resolve_types_handles_path_format_mismatch() {
        // Regression test for the TypeResolver path mismatch bug.
        //
        // In production, `ExtractResult.file_path` is absolute (e.g.
        // "/home/user/proj/src/b.py") while graph nodes' `file_path` is
        // relative (e.g. "src/b.py") — normalized by ScopeResolutionPhase.
        // TypeResolver builds `imports_map` from `ExtractResult.file_path`
        // (absolute) but queries it with `fqn_to_file` values (relative),
        // causing `imports_map.get(source_file)` to return None.
        //
        // The fix: TypeResolver builds a bidirectional path mapping by
        // matching result nodes' `qualified_name` to graph nodes'
        // `qualified_name` (both are FQNs from the absolute path during
        // parse, so they match). This allows `imports_map` to be keyed by
        // the graph's relative paths and `lookup_in_file` to be queried
        // with the result's absolute path.
        //
        // FQNs are generated from the absolute path (as in production), so
        // they include all path segments: "/abs/path/a.py" → "proj.abs.path.a.py.A".
        // Graph nodes use the SAME FQN but have file_path normalized to relative.

        // Simulate production: extraction with ABSOLUTE file_path.
        let abs_a = "/abs/path/a.py";
        let abs_b = "/abs/path/b.py";
        // FQN segments from absolute path: ["abs", "path", "a.py"]
        let qn_a = "proj.abs.path.a.py.A";
        let qn_b = "proj.abs.path.b.py.B";

        let class_a = Node::builder(NodeLabel::Class, "A", qn_a)
            .file_path(abs_a)
            .language(Language::Python)
            .project("proj")
            .is_exported(true)
            .build();
        let class_b = Node::builder(NodeLabel::Class, "B", qn_b)
            .file_path(abs_b)
            .language(Language::Python)
            .project("proj")
            .build();

        let results = vec![
            {
                let mut r = ExtractResult::new(abs_a, Language::Python);
                r.push_node(class_a);
                r
            },
            {
                let mut r = ExtractResult::new(abs_b, Language::Python);
                r.push_node(class_b);
                r.imports.push(ImportInfo {
                    source_file: "a".to_string(),
                    imported_names: vec!["A".to_string()],
                    line: 1,
                });
                r
            },
        ];
        let table = build_symbol_table(&results, "proj");

        // Graph nodes: id set to FQN (simulating ScopeResolutionPhase
        // phases.rs:338-342 which sets g.id = node.qualified_name), and
        // file_path normalized to RELATIVE (phases.rs:344-346).
        let mut graph = Graph::new();
        let graph_a = Node::builder(NodeLabel::Class, "A", qn_a)
            .id(qn_a)
            .file_path("a.py")
            .language(Language::Python)
            .project("proj")
            .is_exported(true)
            .build();
        let graph_b = Node::builder(NodeLabel::Class, "B", qn_b)
            .id(qn_b)
            .file_path("b.py")
            .language(Language::Python)
            .project("proj")
            .build();
        graph.add_node(graph_a);
        graph.add_node(graph_b);
        // Dangling edge: B extends A (wrong file segment in FQN).
        graph.add_edge(Edge::new(
            qn_b,
            "proj.abs.path.b.py.A",
            EdgeType::Extends,
            "proj",
        ));

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);

        // With the path-mapping fix, import-scoped resolution (0.90) should
        // succeed despite the path-format mismatch.
        assert_eq!(fixed.len(), 1, "should fix the dangling edge");
        assert_eq!(graph.edges[0].target, qn_a, "should resolve to A's FQN");
        assert!(
            (graph.edges[0].confidence - 0.90).abs() < f32::EPSILON,
            "import-scoped (0.90) should succeed with path mapping; got {}",
            graph.edges[0].confidence
        );
        assert_eq!(graph.edges[0].confidence_tier, ConfidenceTier::ImportScoped);
    }

    #[test]
    fn resolve_types_skips_duplicate_file_path_in_results() {
        // Two ExtractResults with the same file_path — the second result
        // should be skipped by the `result_to_graph_fp.contains_key` guard.
        let class_a = make_class("A", "a.py", Language::Python);

        let mut result1 = ExtractResult::new("a.py", Language::Python);
        result1.push_node(class_a.clone());
        let mut result2 = ExtractResult::new("a.py", Language::Python);
        result2.push_node(class_a);

        let results = vec![result1, result2];
        let table = build_symbol_table(&results, "proj");

        let mut graph = Graph::new();
        graph.add_node(make_class("A", "a.py", Language::Python));
        // Dangling edge to force resolve_types to iterate results.
        add_extends(&mut graph, "proj.a.py.A", "proj.a.py.Nonexistent");

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        // Edge is unresolvable → no fix, but duplicate path is exercised.
        assert!(fixed.is_empty(), "unresolvable edge → no fix");
    }

    #[test]
    fn resolve_types_fixes_dangling_uses_type_edge() {
        // UsesType is in RESOLVABLE_EDGE_TYPES but not previously tested.
        let type_a = make_class("TypeA", "a.py", Language::Python);
        let user_b = make_class("UserB", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(type_a, "a.py"), (user_b, "b.py")]);

        let mut graph = Graph::new();
        graph.add_node(make_class("TypeA", "a.py", Language::Python));
        graph.add_node(make_class("UserB", "b.py", Language::Python));
        // Dangling: UserB uses proj.b.py.TypeA (wrong file).
        graph.add_edge(Edge::new(
            "proj.b.py.UserB",
            "proj.b.py.TypeA",
            EdgeType::UsesType,
            "proj",
        ));

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        assert_eq!(fixed.len(), 1, "should fix the dangling UsesType edge");
        assert_eq!(graph.edges[0].target, "proj.a.py.TypeA");
        assert_eq!(graph.edges[0].edge_type, EdgeType::UsesType);
    }

    #[test]
    fn resolve_types_skips_when_resolved_qn_equals_current_target() {
        // Edge case: the type name resolves to a FQN that equals the current
        // (dangling) edge target. This happens when the symbol table has an
        // entry for the type but the graph doesn't have a node with that FQN.
        // Covers L248: `if resolved_qn == edge.target { continue; }`.
        //
        // Setup: class A is in the symbol table (via ExtractResult) but NOT
        // in the graph. Class B is in both. Both are in file b.py. Edge
        // B -> proj.b.py.A is dangling (A not in graph). resolve_type finds
        // A via file-level lookup (same file as B) → returns "proj.b.py.A"
        // which equals edge.target → L248 continue → no fix.
        let class_a = make_class("A", "b.py", Language::Python);
        let class_b = make_class("B", "b.py", Language::Python);
        let (results, table) = build_results_and_table(vec![(class_a, "b.py"), (class_b, "b.py")]);

        let mut graph = Graph::new();
        // Only add B to the graph — A is in the symbol table but not the graph.
        graph.add_node(make_class("B", "b.py", Language::Python));
        // Dangling edge: B extends proj.b.py.A (A is in symbol table but not graph).
        add_extends(&mut graph, "proj.b.py.B", "proj.b.py.A");

        let resolver = TypeResolver::new(&table);
        let fixed = resolver.resolve_types(&results, &mut graph);
        // resolved_qn ("proj.b.py.A") == edge.target ("proj.b.py.A") → skip.
        assert!(
            fixed.is_empty(),
            "resolved_qn == current target → no fix needed"
        );
        // Edge target unchanged.
        assert_eq!(graph.edges[0].target, "proj.b.py.A");
        // Confidence/tier unchanged (still default 1.0 / Global from Edge::new).
        assert!((graph.edges[0].confidence - 1.0).abs() < f32::EPSILON);
        assert_eq!(graph.edges[0].confidence_tier, ConfidenceTier::Global);
    }
}
