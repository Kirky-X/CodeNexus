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
        if let Some(entry) = self
            .symbol_table
            .lookup_in_file(user_file, type_name)
            .first()
        {
            return Some((entry.qn.clone(), CONFIDENCE_EXACT, ConfidenceTier::SameFile));
        }
        // 2. Import-based lookup.
        let is_imported = imports
            .iter()
            .any(|imp| imp.imported_names.iter().any(|n| n == type_name));
        if is_imported {
            if let Some(entry) = self.symbol_table.lookup(type_name).first() {
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
    pub fn resolve_types(
        &self,
        results: &[ExtractResult],
        graph: &mut Graph,
    ) -> Vec<Edge> {
        // Build file_path → imports map from extraction results.
        let imports_map: HashMap<&str, &[ImportInfo]> = results
            .iter()
            .map(|r| (r.file_path.as_str(), r.imports.as_slice()))
            .collect();

        // Build fqn → file_path map from graph nodes (for source lookup).
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
            // Get the source node's file path.
            let Some(&source_file) = fqn_to_file.get(edge.source.as_str()) else {
                continue;
            };
            // Extract the type name from the dangling FQN (last component).
            let type_name = edge.target.rsplit('.').next().unwrap_or(&edge.target);
            // Retrieve imports for this file.
            let imports = imports_map.get(source_file).copied().unwrap_or(&[]);
            // Attempt resolution.
            let Some((resolved_qn, confidence, tier)) =
                self.resolve_type(source_file, type_name, imports)
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

#[cfg(test)]
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
        graph.add_edge(Edge::new(
            source,
            target,
            EdgeType::Implements,
            "proj",
        ));
    }

    fn build_results_and_table(
        nodes: Vec<(Node, &str)>,
    ) -> (Vec<ExtractResult>, ProjectSymbolTable) {
        let mut results: std::collections::HashMap<&str, ExtractResult> =
            std::collections::HashMap::new();
        for (node, file) in nodes {
            let lang = node.language.unwrap_or(Language::Python);
            let result = results.entry(file).or_insert_with(|| {
                ExtractResult::new(file, lang)
            });
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
        let (results, table) =
            build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);
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
        let (results, table) =
            build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);

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
        let (results, table) =
            build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);

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
        let (results, table) = build_results_and_table(vec![
            (trait_def, "a.rs"),
            (impl_class, "b.rs"),
        ]);

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
        let (results, table) =
            build_results_and_table(vec![(class_a, "a.py"), (class_b, "b.py")]);

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
        assert_eq!(
            graph.edges[0].confidence_tier,
            ConfidenceTier::Global
        );
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
}
