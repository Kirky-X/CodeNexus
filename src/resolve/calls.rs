//! Call resolution (resolve/calls.rs) implementing ADR-011.
//!
//! Provides [`CallResolver`] for resolving function/method calls to CALLS
//! edges. Uses a two-pass strategy:
//!
//! 1. **receiver-bound-calls**: If a call has a receiver (e.g. `obj.method()`),
//!    resolve the receiver's type and find the method on that type.
//! 2. **free-call-fallback**: If no receiver or receiver-bound fails, look up
//!    the function name in the symbol table (file-level first, then
//!    project-level exported symbols).
//!
//! # Business rules
//!
//! - BR-TRACE-007: Same-language function call -> CALLS edge (confidence
//!   0.80-0.95).
//! - Confidence: exact match 0.95, import match 0.90, project-level match 0.80.

use std::collections::HashMap;

use crate::model::{Edge, EdgeType, Graph};
use crate::ir::{ExtractResult, ImportInfo};
use crate::resolve::ProjectSymbolTable;

/// Confidence for an exact (file-level) call match.
const CONFIDENCE_EXACT: f32 = 0.95;
/// Confidence for an import-based call match.
const CONFIDENCE_IMPORT: f32 = 0.90;
/// Confidence for a project-level (exported) call match.
const CONFIDENCE_PROJECT: f32 = 0.80;

/// Resolves function/method calls to CALLS edges (ADR-011).
///
/// The resolver is constructed with a reference to a [`ProjectSymbolTable`]
/// and the project name. Call [`with_imports`] to register import information
/// for import-based resolution, then use [`resolve_call`] for single-call
/// resolution or [`resolve_calls`] for batch resolution from [`ExtractResult`]s.
///
/// [`with_imports`]: CallResolver::with_imports
/// [`resolve_call`]: CallResolver::resolve_call
/// [`resolve_calls`]: CallResolver::resolve_calls
pub struct CallResolver<'a> {
    symbol_table: &'a ProjectSymbolTable,
    project: &'a str,
    /// Imports indexed by caller file path, used by [`resolve_call`].
    imports: HashMap<String, Vec<ImportInfo>>,
}

impl<'a> CallResolver<'a> {
    /// Creates a new `CallResolver` with the given symbol table and project.
    ///
    /// The resolver starts with no import information. Use [`with_imports`]
    /// to populate it for import-based resolution.
    ///
    /// [`with_imports`]: CallResolver::with_imports
    #[must_use]
    pub fn new(symbol_table: &'a ProjectSymbolTable, project: &'a str) -> Self {
        Self {
            symbol_table,
            project,
            imports: HashMap::new(),
        }
    }

    /// Registers import information from extraction results (builder pattern).
    ///
    /// Collects imports from each [`ExtractResult`] indexed by file path, so
    /// that [`resolve_call`] can perform import-based resolution.
    ///
    /// [`resolve_call`]: CallResolver::resolve_call
    #[must_use]
    pub fn with_imports(mut self, results: &[ExtractResult]) -> Self {
        for result in results {
            self.imports
                .insert(result.file_path.clone(), result.imports.clone());
        }
        self
    }

    /// Resolves all calls from [`ExtractResult`]s and adds CALLS edges to the
    /// graph.
    ///
    /// For each call in each result, resolves the callee using the file's
    /// imports and the symbol table. If the callee is found and the call has
    /// a known caller qualified name, a CALLS edge is created and added to
    /// both the graph and the returned vector.
    ///
    /// # Arguments
    ///
    /// * `results` - The extraction results containing call information.
    /// * `graph` - The graph to add resolved CALLS edges to.
    ///
    /// # Returns
    ///
    /// A vector of all resolved CALLS edges (also added to `graph`).
    pub fn resolve_calls(&self, results: &[ExtractResult], graph: &mut Graph) -> Vec<Edge> {
        let mut edges = Vec::new();
        for result in results {
            let caller_file = &result.file_path;
            let imports = &result.imports;
            for call in &result.calls {
                let Some(caller_qn) = &call.caller_qn else {
                    continue;
                };
                let Some((callee_qn, confidence)) =
                    self.resolve_call_internal(caller_file, &call.callee_name, imports)
                else {
                    continue;
                };
                let edge =
                    Edge::builder(caller_qn.clone(), callee_qn, EdgeType::Calls, self.project)
                        .confidence(confidence)
                        .start_line(call.line)
                        .build();
                graph.add_edge(edge.clone());
                edges.push(edge);
            }
        }
        edges
    }

    /// Resolves a single call: finds the callee by name.
    ///
    /// Resolution strategy (ADR-011 free-call-fallback):
    /// 1. Look up in the file-level symbol table (confidence 0.95).
    /// 2. Look up in imported symbols of the caller file (confidence 0.90).
    /// 3. Look up in project-level exported symbols (confidence 0.80).
    /// 4. Return `None` if not found.
    ///
    /// # Arguments
    ///
    /// * `caller_file` - The file path of the calling function.
    /// * `callee_name` - The simple name of the called function.
    ///
    /// # Returns
    ///
    /// `Some((callee_qn, confidence))` if the callee is found, `None`
    /// otherwise.
    #[must_use]
    pub fn resolve_call(&self, caller_file: &str, callee_name: &str) -> Option<(String, f32)> {
        let imports = self
            .imports
            .get(caller_file)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        self.resolve_call_internal(caller_file, callee_name, imports)
    }

    /// Internal resolution logic shared by [`resolve_call`] and
    /// [`resolve_calls`].
    ///
    /// [`resolve_call`]: CallResolver::resolve_call
    /// [`resolve_calls`]: CallResolver::resolve_calls
    fn resolve_call_internal(
        &self,
        caller_file: &str,
        callee_name: &str,
        imports: &[ImportInfo],
    ) -> Option<(String, f32)> {
        // 1. File-level lookup (confidence 0.95)
        if let Some(entry) = self
            .symbol_table
            .lookup_in_file(caller_file, callee_name)
            .first()
        {
            return Some((entry.qn.clone(), CONFIDENCE_EXACT));
        }

        // 2. Import lookup (confidence 0.90)
        let is_imported = imports
            .iter()
            .any(|imp| imp.imported_names.iter().any(|n| n == callee_name));
        if is_imported {
            if let Some(entry) = self.symbol_table.lookup(callee_name).first() {
                return Some((entry.qn.clone(), CONFIDENCE_IMPORT));
            }
        }

        // 3. Project-level exported lookup (confidence 0.80)
        if let Some(entry) = self.symbol_table.lookup_exported(callee_name).first() {
            return Some((entry.qn.clone(), CONFIDENCE_PROJECT));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::ir::{AssignInfo, CallInfo};
    use crate::resolve::{build_symbol_table, FqnGenerator};

    /// Generates the FQN for a top-level entity, matching `build_symbol_table`.
    fn fqn(project: &str, file: &str, name: &str, language: Language) -> String {
        FqnGenerator::generate(project, file, name, language, None)
    }

    /// Creates a definition node with the FQN as both `id` and `qualified_name`.
    fn make_node(name: &str, file: &str, project: &str, label: NodeLabel) -> Node {
        let qn = fqn(project, file, name, Language::Rust);
        Node::builder(label, name, qn)
            .file_path(file)
            .project(project)
            .language(Language::Rust)
            .build()
    }

    /// Creates an exported definition node.
    fn make_exported_node(name: &str, file: &str, project: &str, label: NodeLabel) -> Node {
        let qn = fqn(project, file, name, Language::Rust);
        Node::builder(label, name, qn)
            .file_path(file)
            .project(project)
            .language(Language::Rust)
            .is_exported(true)
            .build()
    }

    /// Creates an `ExtractResult` with the given nodes.
    fn make_result(file: &str, nodes: Vec<Node>) -> ExtractResult {
        let mut result = ExtractResult::new(file, Language::Rust);
        result.nodes = nodes;
        result
    }

    /// Adds nodes from results to the graph, using each node's FQN as its id.
    fn add_nodes_to_graph(graph: &mut Graph, results: &[ExtractResult], project: &str) {
        for result in results {
            for node in &result.nodes {
                let qn = fqn(project, &result.file_path, &node.name, Language::Rust);
                let mut graph_node = node.clone();
                graph_node.id = qn.clone();
                graph_node.qualified_name = qn;
                graph.add_node(graph_node);
            }
        }
    }

    // --- resolve_call: file-level lookup ---

    #[test]
    fn resolve_call_finds_function_in_same_file() {
        let node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let result = make_result("a.rs", vec![node]);
        let results = vec![result];
        let table = build_symbol_table(&results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let resolved = resolver.resolve_call("a.rs", "foo");

        assert!(resolved.is_some());
        let (qn, confidence) = resolved.unwrap();
        assert_eq!(qn, "proj.a.rs.foo");
        assert!((confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn resolve_call_file_level_returns_correct_qn_for_nested_path() {
        let node = make_node("bar", "src/deep/file.rs", "proj", NodeLabel::Function);
        let result = make_result("src/deep/file.rs", vec![node]);
        let results = vec![result];
        let table = build_symbol_table(&results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let resolved = resolver.resolve_call("src/deep/file.rs", "bar");

        assert!(resolved.is_some());
        let (qn, _) = resolved.unwrap();
        assert_eq!(qn, "proj.src.deep.file.rs.bar");
    }

    // --- resolve_call: import lookup ---

    #[test]
    fn resolve_call_finds_function_via_import() {
        let bar_node = make_exported_node("bar", "b.rs", "proj", NodeLabel::Function);
        let bar_result = make_result("b.rs", vec![bar_node]);
        let mut a_result = make_result("a.rs", vec![]);
        a_result.imports.push(ImportInfo {
            source_file: "b.rs".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 1,
        });

        let results = vec![bar_result, a_result];
        let table = build_symbol_table(&results, "proj");
        let resolver = CallResolver::new(&table, "proj").with_imports(&results);

        let resolved = resolver.resolve_call("a.rs", "bar");

        assert!(resolved.is_some());
        let (qn, confidence) = resolved.unwrap();
        assert_eq!(qn, "proj.b.rs.bar");
        assert!((confidence - 0.90).abs() < 1e-6);
    }

    #[test]
    fn resolve_call_import_takes_precedence_over_project_export() {
        // When a symbol is imported, it should be resolved via import (0.90)
        // rather than project-level export (0.80).
        let bar_node = make_exported_node("bar", "b.rs", "proj", NodeLabel::Function);
        let bar_result = make_result("b.rs", vec![bar_node]);
        let mut a_result = make_result("a.rs", vec![]);
        a_result.imports.push(ImportInfo {
            source_file: "b.rs".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 1,
        });

        let results = vec![bar_result, a_result];
        let table = build_symbol_table(&results, "proj");
        let resolver = CallResolver::new(&table, "proj").with_imports(&results);

        let resolved = resolver.resolve_call("a.rs", "bar").unwrap();
        assert!((resolved.1 - 0.90).abs() < 1e-6);
    }

    #[test]
    fn resolve_call_without_imports_registered_uses_project_lookup() {
        // If with_imports was not called, import lookup is skipped and
        // project-level exported lookup is used instead.
        let bar_node = make_exported_node("bar", "b.rs", "proj", NodeLabel::Function);
        let bar_result = make_result("b.rs", vec![bar_node]);
        let a_result = make_result("a.rs", vec![]);

        let results = vec![bar_result, a_result];
        let table = build_symbol_table(&results, "proj");
        let resolver = CallResolver::new(&table, "proj");

        let resolved = resolver.resolve_call("a.rs", "bar");

        assert!(resolved.is_some());
        let (qn, confidence) = resolved.unwrap();
        assert_eq!(qn, "proj.b.rs.bar");
        assert!((confidence - 0.80).abs() < 1e-6);
    }

    // --- resolve_call: project-level exported lookup ---

    #[test]
    fn resolve_call_finds_exported_function_in_project() {
        let bar_node = make_exported_node("bar", "b.rs", "proj", NodeLabel::Function);
        let bar_result = make_result("b.rs", vec![bar_node]);
        let a_result = make_result("a.rs", vec![]);

        let results = vec![bar_result, a_result];
        let table = build_symbol_table(&results, "proj");
        let resolver = CallResolver::new(&table, "proj");

        let resolved = resolver.resolve_call("a.rs", "bar");

        assert!(resolved.is_some());
        let (qn, confidence) = resolved.unwrap();
        assert_eq!(qn, "proj.b.rs.bar");
        assert!((confidence - 0.80).abs() < 1e-6);
    }

    #[test]
    fn resolve_call_project_level_skips_non_exported() {
        // Non-exported symbols should not be found via project-level lookup.
        let bar_node = make_node("bar", "b.rs", "proj", NodeLabel::Function);
        let bar_result = make_result("b.rs", vec![bar_node]);
        let a_result = make_result("a.rs", vec![]);

        let results = vec![bar_result, a_result];
        let table = build_symbol_table(&results, "proj");
        let resolver = CallResolver::new(&table, "proj");

        let resolved = resolver.resolve_call("a.rs", "bar");
        assert!(resolved.is_none());
    }

    // --- resolve_call: not found ---

    #[test]
    fn resolve_call_returns_none_for_unknown_function() {
        let a_result = make_result("a.rs", vec![]);
        let results = vec![a_result];
        let table = build_symbol_table(&results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let resolved = resolver.resolve_call("a.rs", "nonexistent");
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_call_returns_none_for_empty_callee_name() {
        let node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let result = make_result("a.rs", vec![node]);
        let results = vec![result];
        let table = build_symbol_table(&results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let resolved = resolver.resolve_call("a.rs", "");
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_call_returns_none_for_unknown_file() {
        let node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let result = make_result("a.rs", vec![node]);
        let results = vec![result];
        let table = build_symbol_table(&results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let resolved = resolver.resolve_call("nonexistent.rs", "foo");
        assert!(resolved.is_none());
    }

    // --- resolve_calls: batch resolution ---

    #[test]
    fn resolve_calls_creates_calls_edges_for_all_resolvable_calls() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let bar_node = make_node("bar", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node, bar_node]);
        let foo_qn = fqn("proj", "a.rs", "foo", Language::Rust);
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn.clone()),
            callee_name: "bar".to_string(),
            line: 5,
            args: vec![],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let edges = resolver.resolve_calls(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        let edge = &edges[0];
        assert_eq!(edge.source, foo_qn);
        assert_eq!(edge.target, "proj.a.rs.bar");
        assert_eq!(edge.edge_type, EdgeType::Calls);
        assert!((edge.confidence - 0.95).abs() < 1e-6);
        assert_eq!(edge.start_line, Some(5));
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn resolve_calls_skips_calls_without_caller_qn() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let bar_node = make_node("bar", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node, bar_node]);
        result.calls.push(CallInfo {
            caller_qn: None,
            callee_name: "bar".to_string(),
            line: 5,
            args: vec![],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = CallResolver::new(&table, "proj");
        let edges = resolver.resolve_calls(&results, &mut graph);

        assert!(edges.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_calls_skips_unresolvable_callees() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let foo_qn = fqn("proj", "a.rs", "foo", Language::Rust);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn),
            callee_name: "nonexistent".to_string(),
            line: 5,
            args: vec![],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = CallResolver::new(&table, "proj");
        let edges = resolver.resolve_calls(&results, &mut graph);

        assert!(edges.is_empty());
    }

    #[test]
    fn resolve_calls_handles_multiple_results() {
        let a_node = make_node("func_a", "a.rs", "proj", NodeLabel::Function);
        let b_node = make_exported_node("func_b", "b.rs", "proj", NodeLabel::Function);
        let a_qn = fqn("proj", "a.rs", "func_a", Language::Rust);

        let mut a_result = make_result("a.rs", vec![a_node]);
        a_result.calls.push(CallInfo {
            caller_qn: Some(a_qn.clone()),
            callee_name: "func_b".to_string(),
            line: 3,
            args: vec![],
        });
        let b_result = make_result("b.rs", vec![b_node]);

        let results = vec![a_result, b_result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let edges = resolver.resolve_calls(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, a_qn);
        assert_eq!(edges[0].target, "proj.b.rs.func_b");
        assert!((edges[0].confidence - 0.80).abs() < 1e-6);
    }

    #[test]
    fn resolve_calls_creates_multiple_edges_for_multiple_calls() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let bar_node = make_node("bar", "a.rs", "proj", NodeLabel::Function);
        let baz_node = make_node("baz", "a.rs", "proj", NodeLabel::Function);
        let foo_qn = fqn("proj", "a.rs", "foo", Language::Rust);

        let mut result = make_result("a.rs", vec![foo_node, bar_node, baz_node]);
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn.clone()),
            callee_name: "bar".to_string(),
            line: 3,
            args: vec![],
        });
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn.clone()),
            callee_name: "baz".to_string(),
            line: 4,
            args: vec![],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        let edges = resolver.resolve_calls(&results, &mut graph);

        assert_eq!(edges.len(), 2);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn resolve_calls_empty_results_returns_empty() {
        let table = ProjectSymbolTable::new();
        let mut graph = Graph::new();
        let resolver = CallResolver::new(&table, "proj");
        let edges = resolver.resolve_calls(&[], &mut graph);
        assert!(edges.is_empty());
    }

    #[test]
    fn resolve_calls_adds_edges_to_graph() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let bar_node = make_node("bar", "a.rs", "proj", NodeLabel::Function);
        let foo_qn = fqn("proj", "a.rs", "foo", Language::Rust);
        let bar_qn = fqn("proj", "a.rs", "bar", Language::Rust);

        let mut result = make_result("a.rs", vec![foo_node, bar_node]);
        result.calls.push(CallInfo {
            caller_qn: Some(foo_qn.clone()),
            callee_name: "bar".to_string(),
            line: 5,
            args: vec![],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        resolver.resolve_calls(&results, &mut graph);

        // Verify the edge is in the graph and neighbors work.
        let neighbors = graph.neighbors(&foo_qn, Some(EdgeType::Calls));
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].id, bar_qn);
    }

    // --- AC-TRACE-001: A calls B -> CALLS edge A->B in graph ---

    #[test]
    fn ac_trace_001_call_path_a_to_b() {
        // Given: function A in a.rs calls function B in b.rs
        let a_node = make_node("A", "a.rs", "proj", NodeLabel::Function);
        let a_qn = fqn("proj", "a.rs", "A", Language::Rust);
        let b_qn = fqn("proj", "b.rs", "B", Language::Rust);

        let mut a_result = make_result("a.rs", vec![a_node]);
        a_result.calls.push(CallInfo {
            caller_qn: Some(a_qn.clone()),
            callee_name: "B".to_string(),
            line: 5,
            args: vec![],
        });
        // Make B exported so it can be resolved from a.rs
        let b_result = make_result(
            "b.rs",
            vec![make_exported_node("B", "b.rs", "proj", NodeLabel::Function)],
        );

        let results = vec![a_result, b_result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        let resolver = CallResolver::new(&table, "proj");
        resolver.resolve_calls(&results, &mut graph);

        // When: trace A --type calls
        let neighbors = graph.neighbors(&a_qn, Some(EdgeType::Calls));

        // Then: return A->B call path
        assert_eq!(
            neighbors.len(),
            1,
            "A should have exactly one CALLS neighbor"
        );
        assert_eq!(neighbors[0].id, b_qn, "A's CALLS neighbor should be B");
        assert_eq!(neighbors[0].name, "B");
    }

    // --- with_imports builder ---

    #[test]
    fn with_imports_is_chainable() {
        let table = ProjectSymbolTable::new();
        let result = ExtractResult::new("a.rs", Language::Rust);
        let resolver = CallResolver::new(&table, "proj").with_imports(&[result]);
        // Should not panic; resolver should have imports registered.
        assert!(resolver.imports.contains_key("a.rs"));
    }

    #[test]
    fn with_imports_empty_results_is_noop() {
        let table = ProjectSymbolTable::new();
        let resolver = CallResolver::new(&table, "proj").with_imports(&[]);
        assert!(resolver.imports.is_empty());
    }

    // --- Edge case: assignment data is ignored by call resolver ---

    #[test]
    fn resolve_calls_ignores_assignments() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.assignments.push(AssignInfo {
            target_name: "x".to_string(),
            source_name: "foo".to_string(),
            line: 5,
            is_return_assign: true,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = CallResolver::new(&table, "proj");
        let edges = resolver.resolve_calls(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "CallResolver should not process assignments"
        );
    }
}
