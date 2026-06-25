//! Data flow resolution (resolve/dataflow.rs).
//!
//! Provides [`DataFlowResolver`] for resolving variable assignments, return
//! assignments, and parameter passing into DataFlows edges.
//!
//! # Business rules (PRD §4.2.4)
//!
//! - BR-TRACE-001: Parameter passing - Variable -> Parameter, DataFlows edge.
//!   `foo(var)` -> var flows to foo's parameter.
//! - BR-TRACE-002: Return assignment - Function -> Variable, DataFlows edge.
//!   `x = foo()` -> foo's return flows to x.
//! - BR-TRACE-003: Variable assignment - Variable -> Variable, DataFlows edge.
//!   `x = y` -> y flows to x.
//! - BR-TRACE-004: Function assignment - Function -> Variable, DataFlows edge.
//!   `x = foo()` (same as BR-TRACE-002).
//! - BR-TRACE-005: Variable read - Function -> Variable, Reads edge.
//! - BR-TRACE-006: Variable write - Function -> Variable, Writes edge.

use crate::model::{Edge, EdgeType, Graph, Node, NodeLabel};
use crate::parse::ExtractResult;
use crate::resolve::ProjectSymbolTable;

/// Confidence for a return-assignment data flow edge (BR-TRACE-002/004).
const CONFIDENCE_RETURN_ASSIGN: f32 = 0.90;
/// Confidence for a variable-assignment data flow edge (BR-TRACE-003).
const CONFIDENCE_VAR_ASSIGN: f32 = 0.85;
/// Confidence for a parameter-passing data flow edge (BR-TRACE-001).
const CONFIDENCE_ARG_PASS: f32 = 0.80;
/// Confidence for a variable-read edge (BR-TRACE-005, Function -> Variable).
const CONFIDENCE_READS: f32 = 0.75;
/// Confidence for a variable-write edge (BR-TRACE-006, Function -> Variable).
const CONFIDENCE_WRITES: f32 = 0.75;

/// Resolves data flow edges from extraction results.
///
/// Constructed with a reference to a [`ProjectSymbolTable`] and the project
/// name. Use [`resolve_dataflows`] for batch resolution or the individual
/// `resolve_*` methods for single-edge resolution.
///
/// [`resolve_dataflows`]: DataFlowResolver::resolve_dataflows
pub struct DataFlowResolver<'a> {
    symbol_table: &'a ProjectSymbolTable,
    project: &'a str,
}

impl<'a> DataFlowResolver<'a> {
    /// Creates a new `DataFlowResolver` with the given symbol table and project.
    #[must_use]
    pub fn new(symbol_table: &'a ProjectSymbolTable, project: &'a str) -> Self {
        Self {
            symbol_table,
            project,
        }
    }

    /// Resolves all data flows from [`ExtractResult`]s and adds edges to the
    /// graph.
    ///
    /// Processes:
    /// - Assignments: return assignments (BR-TRACE-002/004) and variable
    ///   assignments (BR-TRACE-003).
    /// - Call arguments: parameter passing (BR-TRACE-001).
    /// - Variable reads: Reads edges (BR-TRACE-005).
    /// - Variable writes: Writes edges (BR-TRACE-006).
    ///
    /// # Arguments
    ///
    /// * `results` - The extraction results containing assignment, call, read,
    ///   and write information.
    /// * `graph` - The graph to add resolved DataFlows/Reads/Writes edges to.
    ///
    /// # Returns
    ///
    /// A vector of all resolved data flow edges (also added to `graph`).
    pub fn resolve_dataflows(&self, results: &[ExtractResult], graph: &mut Graph) -> Vec<Edge> {
        let mut edges = Vec::new();
        for result in results {
            let file = &result.file_path;

            // Process assignments (BR-TRACE-002, BR-TRACE-003, BR-TRACE-004).
            for assign in &result.assignments {
                let edge = if assign.is_return_assign {
                    // x = foo() -> DataFlows edge foo -> x
                    self.resolve_return_assign(file, &assign.source_name, &assign.target_name)
                } else {
                    // x = y -> DataFlows edge y -> x
                    self.resolve_var_assign(file, &assign.target_name, &assign.source_name)
                };
                if let Some(mut edge) = edge {
                    edge.start_line = Some(assign.line);
                    graph.add_edge(edge.clone());
                    edges.push(edge);
                }
            }

            // Process call arguments (BR-TRACE-001).
            for call in &result.calls {
                for (arg_index, arg) in call.args.iter().enumerate() {
                    // Only create data flow edges for variable arguments,
                    // not literals like "42" or "\"hello\"".
                    if !is_identifier(arg) {
                        continue;
                    }
                    if let Some(mut edge) =
                        self.resolve_arg_pass(file, arg, &call.callee_name, arg_index)
                    {
                        edge.start_line = Some(call.line);
                        // Create the Parameter node so the edge target is not
                        // orphaned (DQ-004).
                        let param_qn = edge.target.clone();
                        let param_node = Node::builder(
                            NodeLabel::Parameter,
                            format!("param{arg_index}"),
                            param_qn.clone(),
                        )
                        .id(param_qn)
                        .project(self.project)
                        .file_path(file)
                        .build();
                        graph.add_node(param_node);
                        graph.add_edge(edge.clone());
                        edges.push(edge);
                    }
                }
            }
        }

        // Process variable reads (BR-TRACE-005) and writes (BR-TRACE-006).
        edges.extend(self.resolve_reads(results, graph));
        edges.extend(self.resolve_writes(results, graph));
        edges
    }

    /// Resolves a return assignment: `x = foo()` -> DataFlows edge from foo
    /// to x.
    ///
    /// Implements BR-TRACE-002 and BR-TRACE-004.
    ///
    /// # Arguments
    ///
    /// * `file` - The source file path.
    /// * `func_name` - The name of the function whose return value is
    ///   assigned.
    /// * `var_name` - The name of the variable receiving the return value.
    ///
    /// # Returns
    ///
    /// `Some(Edge)` with edge type DataFlows if the function is found in the
    /// symbol table, `None` otherwise.
    #[must_use]
    pub fn resolve_return_assign(
        &self,
        file: &str,
        func_name: &str,
        var_name: &str,
    ) -> Option<Edge> {
        let func_qn = self.lookup_symbol_qn(file, func_name)?;
        let var_qn = self.resolve_var_identifier(file, var_name);
        let edge = Edge::builder(func_qn, var_qn, EdgeType::DataFlows, self.project)
            .confidence(CONFIDENCE_RETURN_ASSIGN)
            .build();
        Some(edge)
    }

    /// Resolves a variable assignment: `x = y` -> DataFlows edge from y to x.
    ///
    /// Implements BR-TRACE-003.
    ///
    /// # Arguments
    ///
    /// * `file` - The source file path.
    /// * `target` - The name of the variable being assigned.
    /// * `source` - The name of the source variable.
    ///
    /// # Returns
    ///
    /// `Some(Edge)` with edge type DataFlows. Always returns `Some` since
    /// variable assignments are always valid.
    #[must_use]
    pub fn resolve_var_assign(&self, file: &str, target: &str, source: &str) -> Option<Edge> {
        let source_qn = self.resolve_var_identifier(file, source);
        let target_qn = self.resolve_var_identifier(file, target);
        let edge = Edge::builder(source_qn, target_qn, EdgeType::DataFlows, self.project)
            .confidence(CONFIDENCE_VAR_ASSIGN)
            .build();
        Some(edge)
    }

    /// Resolves parameter passing: `foo(var)` -> DataFlows edge from var to
    /// foo's parameter.
    ///
    /// Implements BR-TRACE-001.
    ///
    /// # Arguments
    ///
    /// * `file` - The source file path.
    /// * `var_name` - The name of the argument variable.
    /// * `callee` - The name of the called function.
    /// * `arg_index` - The zero-based index of the argument.
    ///
    /// # Returns
    ///
    /// `Some(Edge)` with edge type DataFlows if the callee is found in the
    /// symbol table, `None` otherwise. The target is
    /// `{callee_qn}.param{arg_index}`.
    #[must_use]
    pub fn resolve_arg_pass(
        &self,
        file: &str,
        var_name: &str,
        callee: &str,
        arg_index: usize,
    ) -> Option<Edge> {
        let callee_qn = self.lookup_symbol_qn(file, callee)?;
        let var_qn = self.resolve_var_identifier(file, var_name);
        let param_qn = format!("{callee_qn}.param{arg_index}");
        let edge = Edge::builder(var_qn, param_qn, EdgeType::DataFlows, self.project)
            .confidence(CONFIDENCE_ARG_PASS)
            .build();
        Some(edge)
    }

    /// Resolves variable reads: function reads variable -> Reads edge
    /// (Function -> Variable).
    ///
    /// Implements BR-TRACE-005. For each [`ReadInfo`], the enclosing function
    /// (identified by `reader_qn`, which holds the function name) is looked up
    /// in the symbol table to obtain its FQN; the variable is resolved via
    /// [`resolve_var_identifier`](Self::resolve_var_identifier). If the reader
    /// cannot be resolved, no edge is produced.
    ///
    /// # Arguments
    ///
    /// * `results` - The extraction results containing read records.
    /// * `graph` - The graph to add resolved Reads edges to.
    ///
    /// # Returns
    ///
    /// A vector of all resolved Reads edges (also added to `graph`).
    pub fn resolve_reads(&self, results: &[ExtractResult], graph: &mut Graph) -> Vec<Edge> {
        let mut edges = Vec::new();
        for result in results {
            let file = &result.file_path;
            for read in &result.reads {
                let Some(reader_name) = read.reader_qn.as_deref() else {
                    continue;
                };
                let Some(func_qn) = self.lookup_symbol_qn(file, reader_name) else {
                    continue;
                };
                let var_qn = self.resolve_var_identifier(file, &read.var_name);
                let mut edge = Edge::builder(func_qn, var_qn, EdgeType::Reads, self.project)
                    .confidence(CONFIDENCE_READS)
                    .build();
                edge.start_line = Some(read.line);
                graph.add_edge(edge.clone());
                edges.push(edge);
            }
        }
        edges
    }

    /// Resolves variable writes: function writes variable -> Writes edge
    /// (Function -> Variable).
    ///
    /// Implements BR-TRACE-006. For each [`WriteInfo`], the enclosing function
    /// (identified by `writer_qn`, which holds the function name) is looked up
    /// in the symbol table to obtain its FQN; the variable is resolved via
    /// [`resolve_var_identifier`](Self::resolve_var_identifier). If the writer
    /// cannot be resolved, no edge is produced.
    ///
    /// # Arguments
    ///
    /// * `results` - The extraction results containing write records.
    /// * `graph` - The graph to add resolved Writes edges to.
    ///
    /// # Returns
    ///
    /// A vector of all resolved Writes edges (also added to `graph`).
    pub fn resolve_writes(&self, results: &[ExtractResult], graph: &mut Graph) -> Vec<Edge> {
        let mut edges = Vec::new();
        for result in results {
            let file = &result.file_path;
            for write in &result.writes {
                let Some(writer_name) = write.writer_qn.as_deref() else {
                    continue;
                };
                let Some(func_qn) = self.lookup_symbol_qn(file, writer_name) else {
                    continue;
                };
                let var_qn = self.resolve_var_identifier(file, &write.var_name);
                let mut edge = Edge::builder(func_qn, var_qn, EdgeType::Writes, self.project)
                    .confidence(CONFIDENCE_WRITES)
                    .build();
                edge.start_line = Some(write.line);
                graph.add_edge(edge.clone());
                edges.push(edge);
            }
        }
        edges
    }

    /// Looks up a symbol's qualified name in the symbol table.
    ///
    /// Tries file-level lookup first, then project-level lookup.
    fn lookup_symbol_qn(&self, file: &str, name: &str) -> Option<String> {
        if let Some(entry) = self.symbol_table.lookup_in_file(file, name).first() {
            return Some(entry.qn.clone());
        }
        if let Some(entry) = self.symbol_table.lookup(name).first() {
            return Some(entry.qn.clone());
        }
        None
    }

    /// Resolves a variable identifier to a qualified name.
    ///
    /// If the variable is in the symbol table, returns its qn. Otherwise,
    /// returns a file-qualified fallback `{file_stem}.{name}` where
    /// `file_stem` is the file path with the extension removed and slashes
    /// converted to dots (matching FQN conventions).
    fn resolve_var_identifier(&self, file: &str, name: &str) -> String {
        if let Some(entry) = self.symbol_table.lookup_in_file(file, name).first() {
            return entry.qn.clone();
        }
        if let Some(entry) = self.symbol_table.lookup(name).first() {
            return entry.qn.clone();
        }
        // Fallback: file-qualified name with extension stripped, matching FQN
        // path-segment conventions. A leading "./" is stripped first so that
        // relative paths like "./src/foo.rs" produce "src.foo.x" rather than
        // "..src.foo.x".
        let normalized = file.replace('\\', "/");
        let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
        let file_stem = normalized
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(normalized);
        let file_dotted = file_stem.replace('/', ".");
        format!("{file_dotted}.{name}")
    }
}

/// Returns `true` if `s` looks like a valid identifier (variable name).
///
/// An identifier starts with an alphabetic character or underscore, and
/// contains only alphanumeric characters or underscores. This is used to
/// filter out literals (e.g. `"42"`, `"\"hello\""`) from data flow analysis.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => chars.all(|c| c.is_alphanumeric() || c == '_'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::parse::{AssignInfo, CallInfo, ReadInfo, WriteInfo};
    use crate::resolve::{build_symbol_table, FqnGenerator};

    /// Generates the FQN for a top-level entity, matching `build_symbol_table`.
    fn fqn(project: &str, file: &str, name: &str, language: Language) -> String {
        FqnGenerator::generate(project, file, name, language)
    }

    /// Creates a definition node.
    fn make_node(name: &str, file: &str, project: &str, label: NodeLabel) -> Node {
        let qn = fqn(project, file, name, Language::Rust);
        Node::builder(label, name, qn)
            .file_path(file)
            .project(project)
            .language(Language::Rust)
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

    // --- is_identifier helper ---

    #[test]
    fn is_identifier_valid_names() {
        assert!(is_identifier("x"));
        assert!(is_identifier("foo"));
        assert!(is_identifier("my_var"));
        assert!(is_identifier("_private"));
        assert!(is_identifier("CamelCase"));
        assert!(is_identifier("var123"));
    }

    #[test]
    fn is_identifier_rejects_literals() {
        assert!(!is_identifier("42"));
        assert!(!is_identifier("\"hello\""));
        assert!(!is_identifier("x + 1"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("3.14"));
        assert!(!is_identifier("foo()"));
    }

    // --- resolve_return_assign (BR-TRACE-002, BR-TRACE-004) ---

    #[test]
    fn resolve_return_assign_creates_dataflows_edge() {
        // x = foo() -> DataFlows edge foo -> x
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let results = vec![make_result("a.rs", vec![foo_node])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_return_assign("a.rs", "foo", "x");

        assert!(edge.is_some());
        let edge = edge.unwrap();
        assert_eq!(edge.edge_type, EdgeType::DataFlows);
        assert_eq!(edge.source, "proj.a.foo");
        assert_eq!(edge.target, "a.x");
        assert!((edge.confidence - 0.90).abs() < 1e-6);
    }

    #[test]
    fn resolve_return_assign_returns_none_if_function_not_found() {
        let results = vec![make_result("a.rs", vec![])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_return_assign("a.rs", "nonexistent", "x");
        assert!(edge.is_none());
    }

    #[test]
    fn resolve_return_assign_uses_variable_qn_if_in_symbol_table() {
        // If the variable is in the symbol table, use its qn as the target.
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let x_node = make_node("x", "a.rs", "proj", NodeLabel::Variable);
        let results = vec![make_result("a.rs", vec![foo_node, x_node])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_return_assign("a.rs", "foo", "x").unwrap();

        assert_eq!(edge.source, "proj.a.foo");
        assert_eq!(edge.target, "proj.a.x");
    }

    #[test]
    fn resolve_return_assign_finds_function_via_project_lookup() {
        // Function defined in another file should be found via project lookup.
        let foo_node = make_node("foo", "b.rs", "proj", NodeLabel::Function);
        let results = vec![
            make_result("a.rs", vec![]),
            make_result("b.rs", vec![foo_node]),
        ];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_return_assign("a.rs", "foo", "x");

        assert!(edge.is_some());
        let edge = edge.unwrap();
        assert_eq!(edge.source, "proj.b.foo");
    }

    // --- resolve_var_assign (BR-TRACE-003) ---

    #[test]
    fn resolve_var_assign_creates_dataflows_edge() {
        // x = y -> DataFlows edge y -> x
        let results = vec![make_result("a.rs", vec![])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_var_assign("a.rs", "x", "y");

        assert!(edge.is_some());
        let edge = edge.unwrap();
        assert_eq!(edge.edge_type, EdgeType::DataFlows);
        assert_eq!(edge.source, "a.y");
        assert_eq!(edge.target, "a.x");
        assert!((edge.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn resolve_var_assign_uses_symbol_table_qns() {
        let y_node = make_node("y", "a.rs", "proj", NodeLabel::Variable);
        let x_node = make_node("x", "a.rs", "proj", NodeLabel::Variable);
        let results = vec![make_result("a.rs", vec![y_node, x_node])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_var_assign("a.rs", "x", "y").unwrap();

        assert_eq!(edge.source, "proj.a.y");
        assert_eq!(edge.target, "proj.a.x");
    }

    #[test]
    fn resolve_var_assign_always_returns_edge() {
        // Variable assignment should always produce an edge, even if neither
        // variable is in the symbol table.
        let table = ProjectSymbolTable::new();
        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_var_assign("a.rs", "x", "y");
        assert!(edge.is_some());
    }

    // --- resolve_var_identifier fallback FQN format ---

    #[test]
    fn resolve_var_identifier_fallback_strips_leading_dot_slash() {
        // Relative path "./src/foo.rs" must not produce a leading ".." in the
        // FQN. Expected: src.foo.{name}, not ..src.foo.{name}.
        let table = ProjectSymbolTable::new();
        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver
            .resolve_var_assign("./src/foo.rs", "x", "y")
            .unwrap();
        assert_eq!(edge.source, "src.foo.y");
        assert_eq!(edge.target, "src.foo.x");
    }

    #[test]
    fn resolve_var_identifier_fallback_handles_absolute_path() {
        // Path without a leading "./" must keep working unchanged.
        let table = ProjectSymbolTable::new();
        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_var_assign("src/foo.rs", "x", "y").unwrap();
        assert_eq!(edge.source, "src.foo.y");
        assert_eq!(edge.target, "src.foo.x");
    }

    #[test]
    fn resolve_var_identifier_fallback_handles_windows_path() {
        // Backslash separators must be normalised to dots, no leading dot.
        let table = ProjectSymbolTable::new();
        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver
            .resolve_var_assign("src\\foo.rs", "x", "y")
            .unwrap();
        assert_eq!(edge.source, "src.foo.y");
        assert_eq!(edge.target, "src.foo.x");
    }

    // --- resolve_arg_pass (BR-TRACE-001) ---

    #[test]
    fn resolve_arg_pass_creates_dataflows_edge() {
        // foo(var) -> DataFlows edge var -> foo.param0
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let results = vec![make_result("a.rs", vec![foo_node])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_arg_pass("a.rs", "var", "foo", 0);

        assert!(edge.is_some());
        let edge = edge.unwrap();
        assert_eq!(edge.edge_type, EdgeType::DataFlows);
        assert_eq!(edge.source, "a.var");
        assert_eq!(edge.target, "proj.a.foo.param0");
        assert!((edge.confidence - 0.80).abs() < 1e-6);
    }

    #[test]
    fn resolve_arg_pass_returns_none_if_callee_not_found() {
        let results = vec![make_result("a.rs", vec![])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_arg_pass("a.rs", "var", "nonexistent", 0);
        assert!(edge.is_none());
    }

    #[test]
    fn resolve_arg_pass_uses_correct_arg_index() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let results = vec![make_result("a.rs", vec![foo_node])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_arg_pass("a.rs", "var", "foo", 2).unwrap();

        assert_eq!(edge.target, "proj.a.foo.param2");
    }

    #[test]
    fn resolve_arg_pass_uses_variable_qn_if_in_symbol_table() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let var_node = make_node("var", "a.rs", "proj", NodeLabel::Variable);
        let results = vec![make_result("a.rs", vec![foo_node, var_node])];
        let table = build_symbol_table(&results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        let edge = resolver.resolve_arg_pass("a.rs", "var", "foo", 0).unwrap();

        assert_eq!(edge.source, "proj.a.var");
        assert_eq!(edge.target, "proj.a.foo.param0");
    }

    // --- resolve_dataflows: batch resolution ---

    #[test]
    fn resolve_dataflows_creates_all_data_flow_edges() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let bar_node = make_node("bar", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node, bar_node]);

        // x = foo() -> return assignment (BR-TRACE-002)
        result.assignments.push(AssignInfo {
            target_name: "x".to_string(),
            source_name: "foo".to_string(),
            line: 5,
            is_return_assign: true,
        });
        // y = z -> variable assignment (BR-TRACE-003)
        result.assignments.push(AssignInfo {
            target_name: "y".to_string(),
            source_name: "z".to_string(),
            line: 6,
            is_return_assign: false,
        });
        // bar(var) -> parameter passing (BR-TRACE-001)
        result.calls.push(CallInfo {
            caller_qn: Some("proj.a.foo".to_string()),
            callee_name: "bar".to_string(),
            line: 7,
            args: vec!["var".to_string()],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&results, &mut graph);

        assert_eq!(edges.len(), 3, "should create 3 data flow edges");
        assert_eq!(graph.edge_count(), 3);

        // Verify edge types
        assert!(edges.iter().all(|e| e.edge_type == EdgeType::DataFlows));

        // Verify return assignment edge: foo -> x
        let return_edge = edges.iter().find(|e| e.source == "proj.a.foo").unwrap();
        assert_eq!(return_edge.target, "a.x");

        // Verify variable assignment edge: z -> y
        let var_edge = edges.iter().find(|e| e.source == "a.z").unwrap();
        assert_eq!(var_edge.target, "a.y");

        // Verify arg pass edge: var -> bar.param0
        let arg_edge = edges
            .iter()
            .find(|e| e.target == "proj.a.bar.param0")
            .unwrap();
        assert_eq!(arg_edge.source, "a.var");
    }

    #[test]
    fn resolve_dataflows_creates_parameter_node_for_arg_pass() {
        // DQ-004: resolve_dataflows must create a Parameter node for each
        // arg-pass edge so the edge target is not orphaned.
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.calls.push(CallInfo {
            caller_qn: Some("proj.a.foo".to_string()),
            callee_name: "foo".to_string(),
            line: 5,
            args: vec!["x".to_string()],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&results, &mut graph);

        assert_eq!(edges.len(), 1, "should create 1 arg-pass edge");
        let param_qn = "proj.a.foo.param0";
        assert_eq!(edges[0].target, param_qn);

        let param_nodes = graph.nodes_by_label(NodeLabel::Parameter);
        assert_eq!(
            param_nodes.len(),
            1,
            "DQ-004: Parameter node must be created, not orphaned"
        );
        assert_eq!(param_nodes[0].id, param_qn);
        assert_eq!(param_nodes[0].qualified_name, param_qn);
        assert_eq!(param_nodes[0].project, "proj");
    }

    #[test]
    fn resolve_dataflows_skips_literal_args() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.calls.push(CallInfo {
            caller_qn: Some("proj.a.foo".to_string()),
            callee_name: "foo".to_string(),
            line: 5,
            args: vec!["42".to_string(), "\"hello\"".to_string(), "x".to_string()],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&results, &mut graph);

        // Only "x" is a valid identifier; "42" and "\"hello\"" are literals.
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "a.x");
        assert_eq!(edges[0].target, "proj.a.foo.param2");
    }

    #[test]
    fn resolve_dataflows_skips_calls_with_unresolvable_callee() {
        let mut result = make_result("a.rs", vec![]);
        result.calls.push(CallInfo {
            caller_qn: Some("proj.a.foo".to_string()),
            callee_name: "nonexistent".to_string(),
            line: 5,
            args: vec!["x".to_string()],
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "unresolvable callee should produce no edge"
        );
    }

    #[test]
    fn resolve_dataflows_empty_results_returns_empty() {
        let table = ProjectSymbolTable::new();
        let mut graph = Graph::new();
        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&[], &mut graph);
        assert!(edges.is_empty());
    }

    #[test]
    fn resolve_dataflows_handles_multiple_results() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let bar_node = make_node("bar", "b.rs", "proj", NodeLabel::Function);

        let mut a_result = make_result("a.rs", vec![foo_node]);
        a_result.assignments.push(AssignInfo {
            target_name: "x".to_string(),
            source_name: "foo".to_string(),
            line: 5,
            is_return_assign: true,
        });

        let mut b_result = make_result("b.rs", vec![bar_node]);
        b_result.assignments.push(AssignInfo {
            target_name: "y".to_string(),
            source_name: "bar".to_string(),
            line: 3,
            is_return_assign: true,
        });

        let results = vec![a_result, b_result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&results, &mut graph);

        assert_eq!(edges.len(), 2);
    }

    // --- AC-TRACE-002: x passed to foo param -> DataFlows edge ---

    #[test]
    fn ac_trace_002_dataflow_path_x_to_foo_param() {
        // Given: variable x is passed to function foo's parameter
        let foo_node = make_node("foo", "b.rs", "proj", NodeLabel::Function);
        let x_node = make_node("x", "a.rs", "proj", NodeLabel::Variable);
        let foo_qn = fqn("proj", "b.rs", "foo", Language::Rust);
        let x_qn = fqn("proj", "a.rs", "x", Language::Rust);
        let param_qn = format!("{foo_qn}.param0");

        let mut a_result = make_result("a.rs", vec![x_node]);
        a_result.calls.push(CallInfo {
            caller_qn: Some("proj.a.bar".to_string()),
            callee_name: "foo".to_string(),
            line: 5,
            args: vec!["x".to_string()],
        });
        let b_result = make_result("b.rs", vec![foo_node]);

        let results = vec![a_result, b_result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        let resolver = DataFlowResolver::new(&table, "proj");
        resolver.resolve_dataflows(&results, &mut graph);

        // When: trace x --type dataflow
        let neighbors = graph.neighbors(&x_qn, Some(EdgeType::DataFlows));

        // Then: return x -> foo.param data flow path
        assert_eq!(neighbors.len(), 1, "x should have one DataFlows neighbor");
        assert_eq!(
            neighbors[0].id, param_qn,
            "x's DataFlows neighbor should be foo.param0"
        );
    }

    // --- resolve_reads (BR-TRACE-005) ---

    #[test]
    fn resolve_reads_creates_reads_edge() {
        // foo reads x -> Reads edge foo -> x
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.reads.push(ReadInfo {
            reader_qn: Some("foo".to_string()),
            var_name: "x".to_string(),
            line: 5,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_reads(&results, &mut graph);

        assert_eq!(edges.len(), 1, "should create 1 Reads edge");
        let edge = &edges[0];
        assert_eq!(edge.edge_type, EdgeType::Reads);
        assert_eq!(edge.source, "proj.a.foo");
        assert_eq!(edge.target, "a.x");
        assert!(
            (edge.confidence - 0.75).abs() < 1e-6,
            "Reads confidence should be 0.75, got {}",
            edge.confidence
        );
        assert_eq!(graph.edge_count(), 1, "edge should be added to graph");
    }

    #[test]
    fn resolve_reads_skips_when_reader_not_resolvable() {
        // No function in symbol table -> reader_qn cannot be resolved -> no edge.
        let mut result = make_result("a.rs", vec![]);
        result.reads.push(ReadInfo {
            reader_qn: Some("nonexistent".to_string()),
            var_name: "x".to_string(),
            line: 5,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_reads(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "unresolvable reader should produce no edge"
        );
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_reads_skips_when_reader_qn_is_none() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.reads.push(ReadInfo {
            reader_qn: None,
            var_name: "x".to_string(),
            line: 5,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_reads(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "read with no reader_qn should produce no edge"
        );
    }

    #[test]
    fn resolve_reads_uses_variable_qn_if_in_symbol_table() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let x_node = make_node("x", "a.rs", "proj", NodeLabel::Variable);
        let mut result = make_result("a.rs", vec![foo_node, x_node]);
        result.reads.push(ReadInfo {
            reader_qn: Some("foo".to_string()),
            var_name: "x".to_string(),
            line: 5,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_reads(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target, "proj.a.x");
    }

    // --- resolve_writes (BR-TRACE-006) ---

    #[test]
    fn resolve_writes_creates_writes_edge() {
        // foo writes y -> Writes edge foo -> y
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.writes.push(WriteInfo {
            writer_qn: Some("foo".to_string()),
            var_name: "y".to_string(),
            line: 7,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_writes(&results, &mut graph);

        assert_eq!(edges.len(), 1, "should create 1 Writes edge");
        let edge = &edges[0];
        assert_eq!(edge.edge_type, EdgeType::Writes);
        assert_eq!(edge.source, "proj.a.foo");
        assert_eq!(edge.target, "a.y");
        assert!(
            (edge.confidence - 0.75).abs() < 1e-6,
            "Writes confidence should be 0.75, got {}",
            edge.confidence
        );
        assert_eq!(graph.edge_count(), 1, "edge should be added to graph");
    }

    #[test]
    fn resolve_writes_skips_when_writer_not_resolvable() {
        let mut result = make_result("a.rs", vec![]);
        result.writes.push(WriteInfo {
            writer_qn: Some("nonexistent".to_string()),
            var_name: "y".to_string(),
            line: 7,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_writes(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "unresolvable writer should produce no edge"
        );
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_writes_skips_when_writer_qn_is_none() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        result.writes.push(WriteInfo {
            writer_qn: None,
            var_name: "y".to_string(),
            line: 7,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_writes(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "write with no writer_qn should produce no edge"
        );
    }

    // --- resolve_dataflows integration (BR-TRACE-005/006) ---

    #[test]
    fn resolve_dataflows_includes_reads_and_writes() {
        let foo_node = make_node("foo", "a.rs", "proj", NodeLabel::Function);
        let mut result = make_result("a.rs", vec![foo_node]);
        // foo reads x (BR-TRACE-005)
        result.reads.push(ReadInfo {
            reader_qn: Some("foo".to_string()),
            var_name: "x".to_string(),
            line: 3,
        });
        // foo writes y (BR-TRACE-006)
        result.writes.push(WriteInfo {
            writer_qn: Some("foo".to_string()),
            var_name: "y".to_string(),
            line: 4,
        });

        let results = vec![result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&results, &mut graph);

        // Should contain at least one Reads and one Writes edge.
        let reads_edges: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Reads)
            .collect();
        let writes_edges: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Writes)
            .collect();

        assert_eq!(reads_edges.len(), 1, "expected 1 Reads edge");
        assert_eq!(writes_edges.len(), 1, "expected 1 Writes edge");

        let reads_edge = reads_edges[0];
        assert_eq!(reads_edge.source, "proj.a.foo");
        assert_eq!(reads_edge.target, "a.x");
        assert!((reads_edge.confidence - 0.75).abs() < 1e-6);

        let writes_edge = writes_edges[0];
        assert_eq!(writes_edge.source, "proj.a.foo");
        assert_eq!(writes_edge.target, "a.y");
        assert!((writes_edge.confidence - 0.75).abs() < 1e-6);

        assert_eq!(
            graph.edge_count(),
            2,
            "both edges should be added to the graph"
        );
    }

    #[test]
    fn resolve_dataflows_without_reads_or_writes_unchanged() {
        // Existing DataFlows behavior must not regress when reads/writes empty.
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

        let resolver = DataFlowResolver::new(&table, "proj");
        let edges = resolver.resolve_dataflows(&results, &mut graph);

        assert_eq!(edges.len(), 1, "only the DataFlows edge should be present");
        assert!(edges.iter().all(|e| e.edge_type == EdgeType::DataFlows));
    }
}
