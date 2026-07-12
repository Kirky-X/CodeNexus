// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Context graph traversal helpers for the `context` command.

use super::types::RelatedNodeOutput;
use crate::model::{EdgeType, Graph, NodeId};
use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
use crate::storage::schema::escape_cypher_string;
use serde::{Deserialize, Serialize};

/// Maximum byte length for the `source` field (10 KB per spec constraint).
const SOURCE_MAX_BYTES: usize = 10 * 1024;
/// Suffix appended when `source` is truncated.
const SOURCE_TRUNCATED_MARKER: &str = "[truncated]";

/// Truncates `source` to [`SOURCE_MAX_BYTES`] on a UTF-8 char boundary,
/// appending [`SOURCE_TRUNCATED_MARKER`] if truncation occurred.
fn truncate_source(source: &str) -> String {
    if source.len() <= SOURCE_MAX_BYTES {
        return source.to_string();
    }
    let mut end = SOURCE_MAX_BYTES;
    while end > 0 && !source.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = source[..end].to_string();
    result.push_str(SOURCE_TRUNCATED_MARKER);
    result
}

/// Parses parameter list from a function signature string.
///
/// Handles signatures like `"(a: i32, b: String)"` or
/// `"fn foo(a: i32, b: String) -> bool"`, extracting each parameter's
/// name and type.
fn parse_parameters(signature: &str) -> Vec<ParamInfo> {
    let start = signature.find('(');
    let end = signature.rfind(')');
    let (Some(start), Some(end)) = (start, end) else {
        return Vec::new();
    };
    if start >= end {
        return Vec::new();
    }
    let params_str = &signature[start + 1..end];
    if params_str.trim().is_empty() {
        return Vec::new();
    }
    params_str
        .split(',')
        .filter_map(|p| {
            let p = p.trim();
            if p.is_empty() {
                return None;
            }
            if let Some(colon_pos) = p.find(':') {
                Some(ParamInfo {
                    name: p[..colon_pos].trim().to_string(),
                    type_name: p[colon_pos + 1..].trim().to_string(),
                })
            } else {
                Some(ParamInfo {
                    name: p.to_string(),
                    type_name: String::new(),
                })
            }
        })
        .collect()
}

/// Derives a dotted module path from a file path.
///
/// `/src/auth/login.rs` → `src.auth.login`
fn derive_module_path(file_path: &str) -> String {
    let path = std::path::Path::new(file_path);
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let full = if parent.is_empty() {
        stem
    } else {
        format!("{parent}/{stem}")
    };
    full.strip_prefix('/').unwrap_or(&full).replace('/', ".")
}

pub fn resolve_start_id(graph: &Graph, symbol: &str) -> Option<NodeId> {
    let by_name: Vec<&crate::model::Node> =
        graph.nodes.values().filter(|n| n.name == symbol).collect();
    if by_name.len() == 1 {
        return Some(by_name[0].id.clone());
    }
    let by_qn: Vec<&crate::model::Node> = graph
        .nodes
        .values()
        .filter(|n| n.qualified_name == symbol)
        .collect();
    if by_qn.len() == 1 {
        return Some(by_qn[0].id.clone());
    }
    by_name.first().map(|n| n.id.clone())
}

pub fn collect_incoming(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges_to(start_id) {
        if let Some(src) = graph.get_node(&edge.source) {
            out.push(RelatedNodeOutput {
                name: src.name.clone(),
                label: src.label.to_string(),
                qualified_name: src.qualified_name.clone(),
                file_path: src.file_path.clone(),
                start_line: src.start_line,
                edge_type: edge.edge_type.to_string(),
                edge_confidence: edge.confidence,
                edge_reason: edge.reason.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.edge_type
            .cmp(&b.edge_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

pub fn collect_outgoing(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges_from(start_id) {
        if let Some(dst) = graph.get_node(&edge.target) {
            out.push(RelatedNodeOutput {
                name: dst.name.clone(),
                label: dst.label.to_string(),
                qualified_name: dst.qualified_name.clone(),
                file_path: dst.file_path.clone(),
                start_line: dst.start_line,
                edge_type: edge.edge_type.to_string(),
                edge_confidence: edge.confidence,
                edge_reason: edge.reason.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.edge_type
            .cmp(&b.edge_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

pub fn collect_processes(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    const PROCESS_EDGE_TYPES: [EdgeType; 4] = [
        EdgeType::StepInProcess,
        EdgeType::EntryPointOf,
        EdgeType::HandlesRoute,
        EdgeType::HandlesTool,
    ];
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges.iter() {
        if !PROCESS_EDGE_TYPES.contains(&edge.edge_type) {
            continue;
        }
        let other_id = if edge.source == *start_id {
            Some(&edge.target)
        } else if edge.target == *start_id {
            Some(&edge.source)
        } else {
            None
        };
        let Some(other_id) = other_id else { continue };
        let Some(other) = graph.get_node(other_id) else {
            continue;
        };
        out.push(RelatedNodeOutput {
            name: other.name.clone(),
            label: other.label.to_string(),
            qualified_name: other.qualified_name.clone(),
            file_path: other.file_path.clone(),
            start_line: other.start_line,
            edge_type: edge.edge_type.to_string(),
            edge_confidence: edge.confidence,
            edge_reason: edge.reason.clone(),
        });
    }
    out.sort_by(|a, b| {
        a.edge_type
            .cmp(&b.edge_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

// ===== Multi-dimensional context types (T014-T018) =====

/// A symbol's definition (name, signature, source, location).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolDefinition {
    pub name: String,
    pub qualified_name: String,
    pub signature: String,
    pub docstring: String,
    pub source: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// A single function parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamInfo {
    pub name: String,
    pub type_name: String,
}

/// Type-level context for a symbol (parameters, return type, generics, traits).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeContext {
    pub parameters: Vec<ParamInfo>,
    pub return_type: String,
    pub generics: Vec<String>,
    pub implements: Vec<String>,
}

/// Module-level context for the file containing a symbol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModuleContext {
    pub file_path: String,
    pub module_path: String,
    pub package: String,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
}

/// Info about a test function that tests a target symbol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestInfo {
    pub test_name: String,
    pub file_path: String,
    pub line: u32,
}

/// Data-flow summary (Out of Scope — empty placeholder).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DataFlowSummary {}

/// Info about a caller (incoming edge to the symbol).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallerInfo {
    pub name: String,
    pub qualified_name: String,
    pub edge_type: String,
}

/// Info about a callee (outgoing edge from the symbol).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalleeInfo {
    pub name: String,
    pub qualified_name: String,
    pub edge_type: String,
}

/// Full 360-degree context for a symbol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolContext {
    pub symbol: SymbolDefinition,
    pub callers: Vec<CallerInfo>,
    pub callees: Vec<CalleeInfo>,
    pub type_context: TypeContext,
    pub module_context: ModuleContext,
    pub test_context: Vec<TestInfo>,
    pub data_flow: DataFlowSummary,
}

/// Collects multi-dimensional context for a symbol from the graph store.
///
/// Backed by a `&'a dyn Storage` capability, matching the convention used by
/// [`crate::analysis::dead_code::DeadCodeDetector`] and
/// [`crate::analysis::cross_service::CrossServiceLinker`].
pub struct ContextCollector<'a> {
    storage: &'a dyn Storage,
}

impl<'a> ContextCollector<'a> {
    /// Creates a new collector backed by the given storage capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
    }

    /// Collects the full [`SymbolContext`] for `qualified_name` in `project`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if any underlying Cypher query fails or the
    /// symbol is not found.
    pub fn collect(
        &self,
        project: &str,
        qualified_name: &str,
    ) -> StorageResult<SymbolContext> {
        let symbol = self.collect_symbol_definition(qualified_name)?;
        let callers = self.collect_callers(project, &symbol)?;
        let callees = self.collect_callees(project, &symbol)?;
        let type_context = self.collect_type_context(&symbol)?;
        let module_context = self.collect_module_context(&symbol.file_path)?;
        let test_context = self.collect_test_context(qualified_name)?;
        let data_flow = self.collect_data_flow(&symbol)?;
        Ok(SymbolContext {
            symbol,
            callers,
            callees,
            type_context,
            module_context,
            test_context,
            data_flow,
        })
    }

    fn collect_symbol_definition(
        &self,
        qualified_name: &str,
    ) -> StorageResult<SymbolDefinition> {
        let escaped = escape_cypher_string(qualified_name);
        // LadybugDB doesn't support `WHERE (n:Function OR n:Method)` label
        // expressions, so we issue two separate queries.
        let cols = "n.name AS name, n.qualifiedName AS qualified_name, \
                    n.signature AS signature, n.docstring AS docstring, \
                    n.content AS content, n.filePath AS file_path, \
                    n.startLine AS start_line, n.endLine AS end_line";
        let function_cypher = format!(
            "MATCH (n:Function) WHERE n.qualifiedName = '{escaped}' RETURN {cols};"
        );
        let method_cypher = format!(
            "MATCH (n:Method) WHERE n.qualifiedName = '{escaped}' RETURN {cols};"
        );
        for cypher in [function_cypher, method_cypher] {
            let rows = self.storage.query(&cypher)?;
            if let Some(row) = rows.into_iter().next() {
                return Ok(SymbolDefinition {
                    name: row.first().and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    qualified_name: row.get(1).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    signature: row.get(2).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    docstring: row.get(3).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    source: truncate_source(row.get(4).and_then(|v| v.as_str()).unwrap_or_default()),
                    file_path: row.get(5).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    start_line: row.get(6).and_then(|v| v.as_u64()).map(|v| v as u32).or_else(|| row.get(6).and_then(|v| v.as_i64()).map(|v| v as u32)).unwrap_or(0),
                    end_line: row.get(7).and_then(|v| v.as_u64()).map(|v| v as u32).or_else(|| row.get(7).and_then(|v| v.as_i64()).map(|v| v as u32)).unwrap_or(0),
                });
            }
        }
        Err(crate::storage::error::StorageError::NotFound(format!(
            "symbol not found: {qualified_name}"
        )))
    }

    fn collect_type_context(
        &self,
        symbol: &SymbolDefinition,
    ) -> StorageResult<TypeContext> {
        let escaped_qn = escape_cypher_string(&symbol.qualified_name);
        // Query Function/Method node for id and returnType.
        let function_cypher = format!(
            "MATCH (n:Function) WHERE n.qualifiedName = '{escaped_qn}' \
             RETURN n.id AS id, n.returnType AS return_type;"
        );
        let method_cypher = format!(
            "MATCH (n:Method) WHERE n.qualifiedName = '{escaped_qn}' \
             RETURN n.id AS id, n.returnType AS return_type;"
        );
        let mut node_id = String::new();
        let mut return_type = String::new();
        for cypher in [function_cypher, method_cypher] {
            let rows = self.storage.query(&cypher)?;
            if let Some(row) = rows.into_iter().next() {
                node_id = row
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                return_type = row
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                break;
            }
        }
        // Parse parameters from signature.
        let parameters = parse_parameters(&symbol.signature);
        // Query IMPLEMENTS edges to find implemented traits.
        let implements = self.query_implements(&node_id)?;
        Ok(TypeContext {
            parameters,
            return_type,
            generics: Vec::new(),
            implements,
        })
    }

    fn collect_module_context(&self, file_path: &str) -> StorageResult<ModuleContext> {
        let escaped_path = escape_cypher_string(file_path);
        let cypher = format!(
            "MATCH (n:File) WHERE n.filePath = '{escaped_path}' \
             RETURN n.id AS id, n.project AS project;"
        );
        let rows = self.storage.query(&cypher)?;
        let (file_id, package) = match rows.into_iter().next() {
            Some(row) => (
                row.first()
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                row.get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ),
            None => {
                return Err(crate::storage::error::StorageError::NotFound(format!(
                    "file not found: {file_path}"
                )));
            }
        };
        let module_path = derive_module_path(file_path);
        let imports = self.query_edge_target_names(&file_id, "IMPORTS")?;
        let exports = self.query_edge_target_names(&file_id, "DEFINES")?;
        Ok(ModuleContext {
            file_path: file_path.to_string(),
            module_path,
            package,
            imports,
            exports,
        })
    }

    fn collect_test_context(
        &self,
        qualified_name: &str,
    ) -> StorageResult<Vec<TestInfo>> {
        let escaped_qn = escape_cypher_string(qualified_name);
        // Find the target symbol's id.
        let function_cypher = format!(
            "MATCH (n:Function) WHERE n.qualifiedName = '{escaped_qn}' RETURN n.id AS id;"
        );
        let method_cypher = format!(
            "MATCH (n:Method) WHERE n.qualifiedName = '{escaped_qn}' RETURN n.id AS id;"
        );
        let mut symbol_id = String::new();
        for cypher in [function_cypher, method_cypher] {
            let rows = self.storage.query(&cypher)?;
            if let Some(row) = rows.into_iter().next() {
                symbol_id = row
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                break;
            }
        }
        if symbol_id.is_empty() {
            return Ok(Vec::new());
        }
        // Query TESTS edges where target = symbol_id.
        let escaped_id = escape_cypher_string(&symbol_id);
        let edge_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.target = '{escaped_id}' AND e.type = 'TESTS' \
             RETURN e.source AS source;"
        );
        let rows = self.storage.query(&edge_cypher)?;
        let mut tests = Vec::new();
        for row in rows {
            if let Some(source_id) = row.first().and_then(|v| v.as_str()) {
                if let Some(info) = self.query_test_info(source_id)? {
                    tests.push(info);
                }
            }
        }
        Ok(tests)
    }

    fn collect_data_flow(
        &self,
        _symbol: &SymbolDefinition,
    ) -> StorageResult<DataFlowSummary> {
        Ok(DataFlowSummary {})
    }

    fn collect_callers(
        &self,
        _project: &str,
        _symbol: &SymbolDefinition,
    ) -> StorageResult<Vec<CallerInfo>> {
        Ok(Vec::new())
    }

    fn collect_callees(
        &self,
        _project: &str,
        _symbol: &SymbolDefinition,
    ) -> StorageResult<Vec<CalleeInfo>> {
        Ok(Vec::new())
    }

    /// Queries IMPLEMENTS edges from `node_id` and resolves target Trait /
    /// Interface names.
    fn query_implements(&self, node_id: &str) -> StorageResult<Vec<String>> {
        if node_id.is_empty() {
            return Ok(Vec::new());
        }
        let escaped_id = escape_cypher_string(node_id);
        let edge_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.source = '{escaped_id}' AND e.type = 'IMPLEMENTS' \
             RETURN e.target AS target;"
        );
        let rows = self.storage.query(&edge_cypher)?;
        let mut implements = Vec::new();
        for row in rows {
            if let Some(target_id) = row.first().and_then(|v| v.as_str()) {
                let target_escaped = escape_cypher_string(target_id);
                for label in ["Trait", "Interface"] {
                    let name_cypher = format!(
                        "MATCH (n:{label}) WHERE n.id = '{target_escaped}' \
                         RETURN n.name AS name;"
                    );
                    if let Some(name_row) = self.storage.query(&name_cypher)?.into_iter().next() {
                        if let Some(name) = name_row.first().and_then(|v| v.as_str()) {
                            implements.push(name.to_string());
                        }
                        break;
                    }
                }
            }
        }
        Ok(implements)
    }

    /// Queries edges of `edge_type` from `source_id` and resolves each
    /// target node's name across common label tables.
    fn query_edge_target_names(
        &self,
        source_id: &str,
        edge_type: &str,
    ) -> StorageResult<Vec<String>> {
        if source_id.is_empty() {
            return Ok(Vec::new());
        }
        let escaped_source = escape_cypher_string(source_id);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.source = '{escaped_source}' AND e.type = '{edge_type}' \
             RETURN e.target AS target;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut names = Vec::new();
        for row in rows {
            if let Some(target_id) = row.first().and_then(|v| v.as_str()) {
                names.push(self.resolve_node_name(target_id)?);
            }
        }
        Ok(names)
    }

    /// Resolves a node's name by searching across common label tables.
    /// Falls back to the id itself if no matching node is found.
    fn resolve_node_name(&self, node_id: &str) -> StorageResult<String> {
        if node_id.is_empty() {
            return Ok(String::new());
        }
        let escaped = escape_cypher_string(node_id);
        for label in [
            "Function", "Method", "Class", "Struct", "Enum", "Trait", "File", "Module",
            "Interface", "Const", "Static", "Variable",
        ] {
            let cypher = format!(
                "MATCH (n:{label}) WHERE n.id = '{escaped}' RETURN n.name AS name;"
            );
            if let Some(row) = self.storage.query(&cypher)?.into_iter().next() {
                if let Some(name) = row.first().and_then(|v| v.as_str()) {
                    return Ok(name.to_string());
                }
            }
        }
        Ok(node_id.to_string())
    }

    /// Resolves a test function's name, file path, and start line by id.
    fn query_test_info(&self, test_id: &str) -> StorageResult<Option<TestInfo>> {
        let escaped = escape_cypher_string(test_id);
        for label in ["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{label}) WHERE n.id = '{escaped}' \
                 RETURN n.name AS name, n.filePath AS file_path, n.startLine AS line;"
            );
            if let Some(row) = self.storage.query(&cypher)?.into_iter().next() {
                return Ok(Some(TestInfo {
                    test_name: row
                        .first()
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    file_path: row
                        .get(1)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    line: row
                        .get(2)
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32)
                        .or_else(|| row.get(2).and_then(|v| v.as_i64()).map(|v| v as u32))
                        .unwrap_or(0),
                }));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use crate::model::{Edge, EdgeType, Language, Node, NodeLabel};
    use tempfile::TempDir;

    fn make_node(id: &str, name: &str, qn: &str, label: NodeLabel, file: &str, line: u32) -> Node {
        Node::builder(label, name, qn)
            .id(id)
            .file_path(file)
            .start_line(line)
            .end_line(line + 5)
            .language(Language::Rust)
            .signature(format!("fn {name}()"))
            .is_exported(true)
            .build()
    }

    #[test]
    fn resolve_start_id_by_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "id1",
            "foo",
            "demo.foo",
            NodeLabel::Function,
            "/x.rs",
            1,
        ));
        assert_eq!(resolve_start_id(&graph, "foo").as_deref(), Some("id1"));
    }

    #[test]
    fn resolve_start_id_by_qualified_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "id1",
            "foo",
            "demo.foo",
            NodeLabel::Function,
            "/x.rs",
            1,
        ));
        assert_eq!(resolve_start_id(&graph, "demo.foo").as_deref(), Some("id1"));
    }

    #[test]
    fn resolve_start_id_missing_returns_none() {
        let graph = Graph::new();
        assert!(resolve_start_id(&graph, "missing").is_none());
    }

    #[test]
    fn collect_incoming_returns_callers() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(make_node(
            "b",
            "b",
            "demo.b",
            NodeLabel::Function,
            "/b.rs",
            1,
        ));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let incoming = collect_incoming(&graph, &"b".to_string());
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].name, "a");
        assert_eq!(incoming[0].edge_type, "CALLS");
    }

    #[test]
    fn collect_outgoing_returns_callees() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(make_node(
            "b",
            "b",
            "demo.b",
            NodeLabel::Function,
            "/b.rs",
            1,
        ));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let outgoing = collect_outgoing(&graph, &"a".to_string());
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].name, "b");
        assert_eq!(outgoing[0].edge_type, "CALLS");
    }

    #[test]
    fn collect_processes_finds_step_in_process() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(
            Node::builder(NodeLabel::Process, "checkout", "demo.checkout")
                .id("p1")
                .build(),
        );
        graph.add_edge(Edge::new("a", "p1", EdgeType::StepInProcess, "demo"));
        let processes = collect_processes(&graph, &"a".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "checkout");
        assert_eq!(processes[0].edge_type, "STEP_IN_PROCESS");
    }

    #[test]
    fn collect_processes_finds_entry_point_of() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "main",
            "main",
            "demo.main",
            NodeLabel::Function,
            "/m.rs",
            1,
        ));
        graph.add_node(
            Node::builder(NodeLabel::Process, "bootstrap", "demo.bootstrap")
                .id("p1")
                .build(),
        );
        graph.add_edge(Edge::new("main", "p1", EdgeType::EntryPointOf, "demo"));
        let processes = collect_processes(&graph, &"main".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "bootstrap");
        assert_eq!(processes[0].edge_type, "ENTRY_POINT_OF");
    }

    #[test]
    fn collect_processes_ignores_call_edges() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(make_node(
            "b",
            "b",
            "demo.b",
            NodeLabel::Function,
            "/b.rs",
            1,
        ));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let processes = collect_processes(&graph, &"a".to_string());
        assert!(processes.is_empty());
    }

    #[test]
    fn collect_incoming_sorts_by_edge_type_then_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node("target", "target", "demo.target", NodeLabel::Function, "/t.rs", 1));
        graph.add_node(make_node("c1", "z_caller", "demo.z_caller", NodeLabel::Function, "/z.rs", 1));
        graph.add_node(make_node("c2", "a_caller", "demo.a_caller", NodeLabel::Function, "/a.rs", 1));
        graph.add_node(make_node("c3", "m_caller", "demo.m_caller", NodeLabel::Function, "/m.rs", 1));
        graph.add_edge(Edge::new("c1", "target", EdgeType::DataFlows, "demo"));
        graph.add_edge(Edge::new("c2", "target", EdgeType::Calls, "demo"));
        graph.add_edge(Edge::new("c3", "target", EdgeType::Calls, "demo"));
        let incoming = collect_incoming(&graph, &"target".to_string());
        assert_eq!(incoming.len(), 3);
        // Sort: edge_type asc, then name asc → CALLS before DATAFLOWS,
        // and within CALLS: a_caller before m_caller.
        assert_eq!(incoming[0].edge_type, "CALLS");
        assert_eq!(incoming[0].name, "a_caller");
        assert_eq!(incoming[1].edge_type, "CALLS");
        assert_eq!(incoming[1].name, "m_caller");
        assert_eq!(incoming[2].edge_type, "DATAFLOWS");
        assert_eq!(incoming[2].name, "z_caller");
    }

    #[test]
    fn collect_outgoing_sorts_by_edge_type_then_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node("src", "src", "demo.src", NodeLabel::Function, "/s.rs", 1));
        graph.add_node(make_node("d1", "z_callee", "demo.z_callee", NodeLabel::Function, "/z.rs", 1));
        graph.add_node(make_node("d2", "a_callee", "demo.a_callee", NodeLabel::Function, "/a.rs", 1));
        graph.add_edge(Edge::new("src", "d1", EdgeType::DataFlows, "demo"));
        graph.add_edge(Edge::new("src", "d2", EdgeType::Calls, "demo"));
        let outgoing = collect_outgoing(&graph, &"src".to_string());
        assert_eq!(outgoing.len(), 2);
        // CALLS before DATAFLOWS.
        assert_eq!(outgoing[0].edge_type, "CALLS");
        assert_eq!(outgoing[0].name, "a_callee");
        assert_eq!(outgoing[1].edge_type, "DATAFLOWS");
        assert_eq!(outgoing[1].name, "z_callee");
    }

    #[test]
    fn collect_processes_finds_start_as_target() {
        let mut graph = Graph::new();
        graph.add_node(make_node("handler", "handler", "demo.handler", NodeLabel::Function, "/h.rs", 1));
        graph.add_node(
            Node::builder(NodeLabel::Process, "checkout", "demo.checkout")
                .id("p1")
                .build(),
        );
        // Edge from process TO start node (start is the target).
        graph.add_edge(Edge::new("p1", "handler", EdgeType::HandlesRoute, "demo"));
        let processes = collect_processes(&graph, &"handler".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "checkout");
        assert_eq!(processes[0].edge_type, "HANDLES_ROUTE");
    }

    #[test]
    fn collect_processes_sorts_multiple() {
        let mut graph = Graph::new();
        graph.add_node(make_node("handler", "handler", "demo.handler", NodeLabel::Function, "/h.rs", 1));
        graph.add_node(
            Node::builder(NodeLabel::Process, "z_process", "demo.z_process")
                .id("p1")
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Process, "a_process", "demo.a_process")
                .id("p2")
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Process, "m_process", "demo.m_process")
                .id("p3")
                .build(),
        );
        // Mix of StepInProcess and HandlesTool edges.
        graph.add_edge(Edge::new("handler", "p1", EdgeType::StepInProcess, "demo"));
        graph.add_edge(Edge::new("handler", "p2", EdgeType::HandlesTool, "demo"));
        graph.add_edge(Edge::new("p3", "handler", EdgeType::EntryPointOf, "demo"));
        let processes = collect_processes(&graph, &"handler".to_string());
        assert_eq!(processes.len(), 3);
        // Sort: edge_type asc → ENTRY_POINT_OF, HANDLES_TOOL, STEP_IN_PROCESS.
        assert_eq!(processes[0].edge_type, "ENTRY_POINT_OF");
        assert_eq!(processes[0].name, "m_process");
        assert_eq!(processes[1].edge_type, "HANDLES_TOOL");
        assert_eq!(processes[1].name, "a_process");
        assert_eq!(processes[2].edge_type, "STEP_IN_PROCESS");
        assert_eq!(processes[2].name, "z_process");
    }

    // ===== T014: Serialization tests for multi-dimensional context types =====

    #[test]
    fn symbol_definition_roundtrip() {
        let sym = SymbolDefinition {
            name: "foo".to_string(),
            qualified_name: "demo.foo".to_string(),
            signature: "fn foo() -> bool".to_string(),
            docstring: "Does foo.".to_string(),
            source: "fn foo() -> bool { true }".to_string(),
            file_path: "/src/foo.rs".to_string(),
            start_line: 1,
            end_line: 3,
        };
        let json = serde_json::to_string(&sym).expect("serialize");
        let back: SymbolDefinition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sym, back);
        assert!(json.contains("qualified_name"));
        assert!(json.contains("fn foo() -> bool"));
    }

    #[test]
    fn param_info_roundtrip() {
        let p = ParamInfo {
            name: "a".to_string(),
            type_name: "i32".to_string(),
        };
        let json = serde_json::to_string(&p).expect("serialize");
        let back: ParamInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn type_context_roundtrip() {
        let tc = TypeContext {
            parameters: vec![
                ParamInfo {
                    name: "a".to_string(),
                    type_name: "i32".to_string(),
                },
                ParamInfo {
                    name: "b".to_string(),
                    type_name: "String".to_string(),
                },
            ],
            return_type: "bool".to_string(),
            generics: vec!["T".to_string()],
            implements: vec!["Display".to_string()],
        };
        let json = serde_json::to_string(&tc).expect("serialize");
        let back: TypeContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tc, back);
        assert_eq!(back.parameters.len(), 2);
    }

    #[test]
    fn module_context_roundtrip() {
        let mc = ModuleContext {
            file_path: "/src/auth/login.rs".to_string(),
            module_path: "src.auth.login".to_string(),
            package: "demo".to_string(),
            imports: vec!["std::io".to_string()],
            exports: vec!["login".to_string()],
        };
        let json = serde_json::to_string(&mc).expect("serialize");
        let back: ModuleContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(mc, back);
    }

    #[test]
    fn test_info_roundtrip() {
        let ti = TestInfo {
            test_name: "test_foo".to_string(),
            file_path: "/tests/foo_test.rs".to_string(),
            line: 5,
        };
        let json = serde_json::to_string(&ti).expect("serialize");
        let back: TestInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ti, back);
    }

    #[test]
    fn data_flow_summary_roundtrip() {
        let df = DataFlowSummary {};
        let json = serde_json::to_string(&df).expect("serialize");
        let back: DataFlowSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(df, back);
    }

    #[test]
    fn caller_info_roundtrip() {
        let c = CallerInfo {
            name: "caller".to_string(),
            qualified_name: "demo.caller".to_string(),
            edge_type: "CALLS".to_string(),
        };
        let json = serde_json::to_string(&c).expect("serialize");
        let back: CallerInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
    }

    #[test]
    fn callee_info_roundtrip() {
        let c = CalleeInfo {
            name: "callee".to_string(),
            qualified_name: "demo.callee".to_string(),
            edge_type: "CALLS".to_string(),
        };
        let json = serde_json::to_string(&c).expect("serialize");
        let back: CalleeInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
    }

    #[test]
    fn symbol_context_roundtrip() {
        let sc = SymbolContext {
            symbol: SymbolDefinition {
                name: "foo".to_string(),
                qualified_name: "demo.foo".to_string(),
                signature: "fn foo()".to_string(),
                docstring: "".to_string(),
                source: "fn foo() {}".to_string(),
                file_path: "/src/foo.rs".to_string(),
                start_line: 1,
                end_line: 2,
            },
            callers: vec![CallerInfo {
                name: "bar".to_string(),
                qualified_name: "demo.bar".to_string(),
                edge_type: "CALLS".to_string(),
            }],
            callees: vec![CalleeInfo {
                name: "baz".to_string(),
                qualified_name: "demo.baz".to_string(),
                edge_type: "CALLS".to_string(),
            }],
            type_context: TypeContext {
                parameters: vec![],
                return_type: "".to_string(),
                generics: vec![],
                implements: vec![],
            },
            module_context: ModuleContext {
                file_path: "/src/foo.rs".to_string(),
                module_path: "src.foo".to_string(),
                package: "demo".to_string(),
                imports: vec![],
                exports: vec![],
            },
            test_context: vec![TestInfo {
                test_name: "test_foo".to_string(),
                file_path: "/tests/foo_test.rs".to_string(),
                line: 1,
            }],
            data_flow: DataFlowSummary {},
        };
        let json = serde_json::to_string(&sc).expect("serialize");
        let back: SymbolContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sc, back);
        assert!(json.contains("symbol"));
        assert!(json.contains("callers"));
        assert!(json.contains("callees"));
        assert!(json.contains("type_context"));
        assert!(json.contains("module_context"));
        assert!(json.contains("test_context"));
        assert!(json.contains("data_flow"));
    }

    // ===== Storage-based test helpers =====

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("context_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    fn storage(kit: &AsyncKit<AsyncReady>) -> std::sync::Arc<dyn Storage> {
        kit.require::<StorageModule>().expect("require_storage")
    }

    // ===== T015: collect_symbol_definition tests =====

    #[test]
    fn collect_symbol_definition_returns_function() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/foo.rs', startLine: 10, endLine: 20, signature: 'fn foo()', returnType: 'void', isExported: true, docstring: 'Foo function', content: 'fn foo() {}', parentQn: ''});").expect("create");

        let collector = ContextCollector::new(&*s);
        let sym = collector
            .collect_symbol_definition("demo.foo")
            .expect("collect");
        assert_eq!(sym.name, "foo");
        assert_eq!(sym.qualified_name, "demo.foo");
        assert_eq!(sym.signature, "fn foo()");
        assert_eq!(sym.docstring, "Foo function");
        assert_eq!(sym.source, "fn foo() {}");
        assert_eq!(sym.file_path, "/src/foo.rs");
        assert_eq!(sym.start_line, 10);
        assert_eq!(sym.end_line, 20);
    }

    #[test]
    fn collect_symbol_definition_returns_method() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Method {id: 'm1', project: 'demo', name: 'bar', qualifiedName: 'demo.bar', filePath: '/src/bar.rs', startLine: 5, endLine: 15, signature: 'fn bar(&self)', returnType: 'bool', isExported: false, docstring: '', content: 'fn bar(&self) -> bool { true }', parentQn: 'demo'});").expect("create");

        let collector = ContextCollector::new(&*s);
        let sym = collector
            .collect_symbol_definition("demo.bar")
            .expect("collect");
        assert_eq!(sym.name, "bar");
        assert_eq!(sym.source, "fn bar(&self) -> bool { true }");
        assert_eq!(sym.start_line, 5);
    }

    #[test]
    fn collect_symbol_definition_not_found_returns_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let collector = ContextCollector::new(&*s);
        let err = collector
            .collect_symbol_definition("missing.symbol")
            .expect_err("should error");
        assert!(err.to_string().contains("symbol not found"));
        assert!(err.to_string().contains("missing.symbol"));
    }

    #[test]
    fn collect_symbol_definition_empty_fields_ok() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'empty', qualifiedName: 'demo.empty', filePath: '/src/empty.rs', startLine: 1, endLine: 2, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create");

        let collector = ContextCollector::new(&*s);
        let sym = collector
            .collect_symbol_definition("demo.empty")
            .expect("collect");
        assert_eq!(sym.signature, "");
        assert_eq!(sym.docstring, "");
        assert_eq!(sym.source, "");
    }

    #[test]
    fn truncate_source_under_limit_returns_unchanged() {
        let src = "fn foo() {}";
        assert_eq!(truncate_source(src), src);
    }

    #[test]
    fn truncate_source_over_limit_marks_truncated() {
        let src = "x".repeat(SOURCE_MAX_BYTES + 100);
        let result = truncate_source(&src);
        assert!(result.len() < src.len());
        assert!(result.ends_with(SOURCE_TRUNCATED_MARKER));
        // The prefix should be from the original source.
        assert!(result.starts_with('x'));
    }

    // ===== T016: collect_type_context tests =====

    #[test]
    fn parse_parameters_extracts_two_params() {
        let params = parse_parameters("(a: i32, b: String)");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "a");
        assert_eq!(params[0].type_name, "i32");
        assert_eq!(params[1].name, "b");
        assert_eq!(params[1].type_name, "String");
    }

    #[test]
    fn parse_parameters_handles_full_signature() {
        let params = parse_parameters("fn foo(a: i32, b: String) -> bool");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "a");
        assert_eq!(params[1].name, "b");
    }

    #[test]
    fn parse_parameters_empty_returns_empty() {
        assert!(parse_parameters("()").is_empty());
        assert!(parse_parameters("").is_empty());
        assert!(parse_parameters("no parens").is_empty());
    }

    #[test]
    fn collect_type_context_with_two_params() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/foo.rs', startLine: 1, endLine: 5, signature: '(a: i32, b: String)', returnType: 'bool', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create");

        let collector = ContextCollector::new(&*s);
        let symbol = collector
            .collect_symbol_definition("demo.foo")
            .expect("symbol");
        let tc = collector.collect_type_context(&symbol).expect("type context");
        assert_eq!(tc.parameters.len(), 2);
        assert_eq!(tc.parameters[0].name, "a");
        assert_eq!(tc.parameters[0].type_name, "i32");
        assert_eq!(tc.parameters[1].name, "b");
        assert_eq!(tc.parameters[1].type_name, "String");
        assert_eq!(tc.return_type, "bool");
        assert!(tc.implements.is_empty());
        assert!(tc.generics.is_empty());
    }

    #[test]
    fn collect_type_context_no_params_no_return() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'noop', qualifiedName: 'demo.noop', filePath: '/src/noop.rs', startLine: 1, endLine: 2, signature: '()', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create");

        let collector = ContextCollector::new(&*s);
        let symbol = collector
            .collect_symbol_definition("demo.noop")
            .expect("symbol");
        let tc = collector.collect_type_context(&symbol).expect("type context");
        assert!(tc.parameters.is_empty());
        assert_eq!(tc.return_type, "");
    }

    #[test]
    fn collect_type_context_with_implements_edge() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Function {id: 'f3', project: 'demo', name: 'bar', qualifiedName: 'demo.bar', filePath: '/src/bar.rs', startLine: 1, endLine: 5, signature: '(x: u32)', returnType: 'u32', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        s.execute("CREATE (:Trait {id: 't1', project: 'demo', name: 'Display', qualifiedName: 'demo.Display', filePath: '/src/display.rs', startLine: 1, endLine: 10, isExported: true, docstring: '', content: '', parentQn: ''});").expect("create trait");
        s.execute("CREATE (:CodeRelation {id: 'e1', source: 'f3', target: 't1', type: 'IMPLEMENTS', confidence: 1.0, confidenceTier: 'High', reason: 'impl Display', startLine: 1, project: 'demo'});").expect("create edge");

        let collector = ContextCollector::new(&*s);
        let symbol = collector
            .collect_symbol_definition("demo.bar")
            .expect("symbol");
        let tc = collector.collect_type_context(&symbol).expect("type context");
        assert_eq!(tc.parameters.len(), 1);
        assert_eq!(tc.parameters[0].name, "x");
        assert_eq!(tc.parameters[0].type_name, "u32");
        assert_eq!(tc.return_type, "u32");
        assert_eq!(tc.implements.len(), 1);
        assert_eq!(tc.implements[0], "Display");
    }

    // ===== T017: collect_module_context tests =====

    #[test]
    fn derive_module_path_strips_extension_and_dots() {
        assert_eq!(derive_module_path("/src/auth/login.rs"), "src.auth.login");
        assert_eq!(derive_module_path("/src/auth/login"), "src.auth.login");
        assert_eq!(derive_module_path("login.rs"), "login");
        assert_eq!(derive_module_path("/lib/util.rs"), "lib.util");
    }

    #[test]
    fn collect_module_context_returns_file_info() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:File {id: 'file1', project: 'demo', name: 'login.rs', filePath: '/src/auth/login.rs', language: 'Rust', hash: '', lineCount: 100});").expect("create file");

        let collector = ContextCollector::new(&*s);
        let mc = collector
            .collect_module_context("/src/auth/login.rs")
            .expect("module context");
        assert_eq!(mc.file_path, "/src/auth/login.rs");
        assert_eq!(mc.module_path, "src.auth.login");
        assert_eq!(mc.package, "demo");
        assert!(mc.imports.is_empty());
        assert!(mc.exports.is_empty());
    }

    #[test]
    fn collect_module_context_with_imports() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:File {id: 'file1', project: 'demo', name: 'login.rs', filePath: '/src/auth/login.rs', language: 'Rust', hash: '', lineCount: 100});").expect("create file");
        s.execute("CREATE (:File {id: 'file2', project: 'demo', name: 'db.rs', filePath: '/src/db/db.rs', language: 'Rust', hash: '', lineCount: 50});").expect("create imported file");
        s.execute("CREATE (:CodeRelation {id: 'e_imp', source: 'file1', target: 'file2', type: 'IMPORTS', confidence: 1.0, confidenceTier: 'High', reason: 'use', startLine: 1, project: 'demo'});").expect("create import edge");

        let collector = ContextCollector::new(&*s);
        let mc = collector
            .collect_module_context("/src/auth/login.rs")
            .expect("module context");
        assert_eq!(mc.imports.len(), 1);
        assert_eq!(mc.imports[0], "db.rs");
    }

    #[test]
    fn collect_module_context_with_defines() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:File {id: 'file1', project: 'demo', name: 'login.rs', filePath: '/src/auth/login.rs', language: 'Rust', hash: '', lineCount: 100});").expect("create file");
        s.execute("CREATE (:Function {id: 'fn1', project: 'demo', name: 'login', qualifiedName: 'demo.login', filePath: '/src/auth/login.rs', startLine: 5, endLine: 20, signature: 'fn login()', returnType: 'void', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create function");
        s.execute("CREATE (:CodeRelation {id: 'e_def', source: 'file1', target: 'fn1', type: 'DEFINES', confidence: 1.0, confidenceTier: 'High', reason: 'defined in file', startLine: 5, project: 'demo'});").expect("create defines edge");

        let collector = ContextCollector::new(&*s);
        let mc = collector
            .collect_module_context("/src/auth/login.rs")
            .expect("module context");
        assert_eq!(mc.exports.len(), 1);
        assert_eq!(mc.exports[0], "login");
    }

    #[test]
    fn collect_module_context_file_not_found() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let collector = ContextCollector::new(&*s);
        let err = collector
            .collect_module_context("/missing/file.rs")
            .expect_err("should error");
        assert!(err.to_string().contains("file not found"));
    }

    // ===== T018: collect_test_context tests =====

    #[test]
    fn collect_test_context_with_tests_edge() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        // Target symbol
        s.execute("CREATE (:Function {id: 'target1', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/foo.rs', startLine: 10, endLine: 20, signature: 'fn foo()', returnType: 'void', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create target");
        // Test function
        s.execute("CREATE (:Function {id: 'test1', project: 'demo', name: 'test_foo', qualifiedName: 'demo.test_foo', filePath: '/tests/foo_test.rs', startLine: 5, endLine: 15, signature: 'fn test_foo()', returnType: 'void', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create test");
        // TESTS edge: test1 → target1
        s.execute("CREATE (:CodeRelation {id: 'e_test', source: 'test1', target: 'target1', type: 'TESTS', confidence: 1.0, confidenceTier: 'High', reason: 'tests foo', startLine: 5, project: 'demo'});").expect("create tests edge");

        let collector = ContextCollector::new(&*s);
        let tests = collector
            .collect_test_context("demo.foo")
            .expect("test context");
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].test_name, "test_foo");
        assert_eq!(tests[0].file_path, "/tests/foo_test.rs");
        assert_eq!(tests[0].line, 5);
    }

    #[test]
    fn collect_test_context_no_tests_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Function {id: 'target2', project: 'demo', name: 'bar', qualifiedName: 'demo.bar', filePath: '/src/bar.rs', startLine: 1, endLine: 5, signature: 'fn bar()', returnType: 'void', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create target");

        let collector = ContextCollector::new(&*s);
        let tests = collector
            .collect_test_context("demo.bar")
            .expect("test context");
        assert!(tests.is_empty());
    }

    #[test]
    fn collect_test_context_multiple_tests() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        // Target symbol
        s.execute("CREATE (:Function {id: 'target3', project: 'demo', name: 'baz', qualifiedName: 'demo.baz', filePath: '/src/baz.rs', startLine: 1, endLine: 10, signature: 'fn baz()', returnType: 'void', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create target");
        // Test function 1
        s.execute("CREATE (:Function {id: 'test_a', project: 'demo', name: 'test_baz_basic', qualifiedName: 'demo.test_baz_basic', filePath: '/tests/baz_test.rs', startLine: 1, endLine: 10, signature: 'fn test_baz_basic()', returnType: 'void', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create test a");
        // Test function 2
        s.execute("CREATE (:Function {id: 'test_b', project: 'demo', name: 'test_baz_edge', qualifiedName: 'demo.test_baz_edge', filePath: '/tests/baz_test.rs', startLine: 20, endLine: 30, signature: 'fn test_baz_edge()', returnType: 'void', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create test b");
        // TESTS edges
        s.execute("CREATE (:CodeRelation {id: 'e_ta', source: 'test_a', target: 'target3', type: 'TESTS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge a");
        s.execute("CREATE (:CodeRelation {id: 'e_tb', source: 'test_b', target: 'target3', type: 'TESTS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 20, project: 'demo'});").expect("create edge b");

        let collector = ContextCollector::new(&*s);
        let tests = collector
            .collect_test_context("demo.baz")
            .expect("test context");
        assert_eq!(tests.len(), 2);
        let names: Vec<&str> = tests.iter().map(|t| t.test_name.as_str()).collect();
        assert!(names.contains(&"test_baz_basic"));
        assert!(names.contains(&"test_baz_edge"));
    }

    #[test]
    fn collect_test_context_unknown_symbol_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let collector = ContextCollector::new(&*s);
        let tests = collector
            .collect_test_context("missing.symbol")
            .expect("should not error");
        assert!(tests.is_empty());
    }

    // ===== Edge-case coverage for pure helper functions =====

    #[test]
    fn truncate_source_multibyte_at_boundary_adjusts_to_char_boundary() {
        // Build a string where SOURCE_MAX_BYTES falls in the middle of a
        // multi-byte UTF-8 character, forcing the char-boundary search loop
        // to decrement `end` (line 26).
        let prefix = "x".repeat(SOURCE_MAX_BYTES - 1);
        let src = format!("{prefix}α"); // α is 2 bytes in UTF-8
        let result = truncate_source(&src);
        assert!(result.ends_with(SOURCE_TRUNCATED_MARKER));
        // The result must be valid UTF-8 (char boundary respected).
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn parse_parameters_without_types_returns_empty_type_names() {
        // `(a, b)` — no colons, so each param has an empty type_name.
        // Covers the `else` branch (lines 64-66).
        let params = parse_parameters("(a, b)");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "a");
        assert_eq!(params[0].type_name, "");
        assert_eq!(params[1].name, "b");
        assert_eq!(params[1].type_name, "");
    }

    #[test]
    fn parse_parameters_skips_empty_param_between_commas() {
        // `(a, , b)` — the middle entry is empty after trimming, so it's
        // skipped via `return None` (line 56).
        let params = parse_parameters("(a, , b)");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "a");
        assert_eq!(params[1].name, "b");
    }

    #[test]
    fn parse_parameters_reversed_parens_returns_empty() {
        // `)foo(` — the `)` comes before `(`, so `start >= end` (line 45).
        let params = parse_parameters(")foo(");
        assert!(params.is_empty());
    }

    #[test]
    fn collect_processes_ignores_edge_not_involving_start() {
        // An edge between two nodes neither of which is `start_id` should
        // hit the `None` arm (line 176) and be skipped.
        let mut graph = Graph::new();
        graph.add_node(make_node("a", "a", "demo.a", NodeLabel::Function, "/a.rs", 1));
        graph.add_node(make_node("b", "b", "demo.b", NodeLabel::Function, "/b.rs", 1));
        graph.add_node(
            Node::builder(NodeLabel::Process, "checkout", "demo.checkout")
                .id("p1")
                .build(),
        );
        // Edge between "b" and "p1" — does NOT involve "a".
        graph.add_edge(Edge::new("b", "p1", EdgeType::StepInProcess, "demo"));
        let processes = collect_processes(&graph, &"a".to_string());
        assert!(processes.is_empty(), "edge not involving start should be skipped");
    }

    // ===== ContextCollector::collect full integration =====

    #[test]
    fn collect_returns_full_context_for_function() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        s.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/foo.rs', startLine: 1, endLine: 10, signature: '(x: i32)', returnType: 'i32', isExported: true, docstring: '', content: 'fn foo() {}', parentQn: ''});").expect("create function");
        s.execute("CREATE (:File {id: 'file1', project: 'demo', name: 'foo.rs', filePath: '/src/foo.rs', language: 'Rust', hash: '', lineCount: 10});").expect("create file");

        let collector = ContextCollector::new(&*s);
        let ctx = collector
            .collect("demo", "demo.foo")
            .expect("collect should succeed");
        assert_eq!(ctx.symbol.name, "foo");
        assert_eq!(ctx.symbol.qualified_name, "demo.foo");
        assert_eq!(ctx.symbol.file_path, "/src/foo.rs");
        // Stub methods return empty collections.
        assert!(ctx.callers.is_empty());
        assert!(ctx.callees.is_empty());
        assert_eq!(ctx.data_flow, DataFlowSummary {});
        // Type context is populated from the Function node.
        assert_eq!(ctx.type_context.return_type, "i32");
        // Module context is populated from the File node.
        assert_eq!(ctx.module_context.file_path, "/src/foo.rs");
    }

    #[test]
    fn collect_returns_error_when_symbol_not_found() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let collector = ContextCollector::new(&*s);
        assert!(
            collector.collect("demo", "missing.symbol").is_err(),
            "should error when symbol is not found"
        );
    }
}
