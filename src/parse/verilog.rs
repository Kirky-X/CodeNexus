// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Verilog language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-verilog's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `module_declaration` → [`NodeLabel::Module`] (name read from the
//!   nested `module_header` child)
//! - `function_declaration` → [`NodeLabel::Function`]
//! - `task_declaration` → [`NodeLabel::Function`] (Verilog tasks are
//!   void-returning functions)
//! - `always_construct` → [`NodeLabel::Method`] (uses the literal name
//!   `"always"` so multiple always blocks are de-duplicated via `dedupe_qn`
//!   into `always#L{line}` FQNs)
//!
//! # Known limitations
//!
//! - FQN pattern is `project.file_path.name` (no module hierarchy prefix).
//! - `initial_construct`, `generate_region`, and continuous assignments are
//!   not extracted.
//! - Calls (`function_call`, `system_function_call`) and imports
//!   (`include_statement`) are not extracted in this revision.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor};
use super::parser_factory::ParserFactory;

/// Verilog language tree-sitter extractor (Adapter pattern).
pub struct VerilogExtractor {
    _priv: (),
}

impl VerilogExtractor {
    /// Creates a new `VerilogExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for VerilogExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for VerilogExtractor {
    fn language(&self) -> Language {
        Language::Verilog
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Verilog);
        let mut parser = ParserFactory::create_parser(Language::Verilog)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        let ctx = VisitContext {
            file_path,
            project,
        };
        for i in 0..root.named_child_count() as u32 {
            if let Some(child) = root.named_child(i) {
                visit_node(child, source, &ctx, &mut result);
            }
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tree-walking helpers
// ---------------------------------------------------------------------------

/// 不可变的遍历上下文，在 visit_node/visit_children 之间传递。
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "module_declaration" => {
            extract_module(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "function_declaration" => {
            extract_function(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "task_declaration" => {
            // Tasks are void functions in Verilog — extract as Function.
            extract_function(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "always_construct" => {
            extract_always(node, ctx, result);
            visit_children(node, source, ctx, result);
        }
        _ => {
            visit_children(node, source, ctx, result);
        }
    }
}

fn visit_children(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            visit_node(child, source, ctx, result);
        }
    }
}

// ---------------------------------------------------------------------------
// Definition extractors
// ---------------------------------------------------------------------------

fn extract_module(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = module_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Module, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Verilog)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_function(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = function_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source)
        .map(signature_first_line)
        .map(String::from);
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Verilog)
        .project(ctx.project)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_always(node: Node, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    // Per task spec: use the literal name "always" for all always_construct
    // nodes. Multiple always blocks in the same file are disambiguated via
    // dedupe_qn into `always#L{line}` FQNs.
    let name = "always".to_string();
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Method, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Verilog)
        .project(ctx.project)
        .is_global(false)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the module name from a `module_declaration` node by walking its
/// `module_header` child. The `module_header` exposes the name as a
/// `simple_identifier` (either via the `name` field or as the first
/// `simple_identifier` child).
fn module_name(node: Node, source: &str) -> Option<String> {
    // First, try to find a `module_header` child.
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "module_header" {
                // Try the `name` field on the header.
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Some(text) = node_text(name_node, source).map(String::from) {
                        return Some(text);
                    }
                }
                // Fall back to the first `simple_identifier` child.
                for j in 0..child.named_child_count() as u32 {
                    if let Some(id) = child.named_child(j) {
                        if id.kind() == "simple_identifier" {
                            return node_text(id, source).map(String::from);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Extracts the name of a `function_declaration` or `task_declaration` node.
///
/// tree-sitter-verilog nests the name inside wrapper nodes:
/// - `function_declaration` → `function_body_declaration` → `function_identifier` → `simple_identifier`
/// - `task_declaration` → `task_body_declaration` → `task_identifier` → `simple_identifier`
///
/// This helper locates the first `function_identifier`/`task_identifier`
/// descendant, then reads the `simple_identifier` inside it.
fn function_name(node: Node, source: &str) -> Option<String> {
    let id_node = find_first_descendant_of_kind(node, &["function_identifier", "task_identifier"])?;
    let simple_id = find_first_descendant_of_kind(id_node, &["simple_identifier"])?;
    node_text(simple_id, source).map(String::from)
}

/// Depth-first search for the first descendant of `node` whose kind is in
/// `kinds`. Returns `None` if no match is found.
fn find_first_descendant_of_kind<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    if kinds.contains(&node.kind()) {
        return Some(node);
    }
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if let Some(found) = find_first_descendant_of_kind(child, kinds) {
                return Some(found);
            }
        }
    }
    None
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

/// Returns the first line of a signature string (the declaration line).
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Verilog, None)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    // DEFINES edge: file -> definition (matches the Go/C extractor pattern).
    result.edges.push(Edge::new(
        file_path.to_string(),
        node.id.clone(),
        EdgeType::Defines,
        project,
    ));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeLabel;

    fn extract(source: &str) -> ExtractResult {
        let ext = VerilogExtractor::new();
        ext.extract(source, "test.v", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_verilog() {
        assert_eq!(VerilogExtractor::new().language(), Language::Verilog);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = VerilogExtractor::default();
        assert_eq!(ext.language(), Language::Verilog);
    }

    #[test]
    fn extracts_module_declaration() {
        let result = extract("module foo(input clk); endmodule\n");
        let modules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Module)
            .collect();
        assert_eq!(
            modules.len(),
            1,
            "should extract 1 module: {:?}",
            result.nodes
        );
        assert_eq!(modules[0].name, "foo");
        assert_eq!(modules[0].language, Some(Language::Verilog));
        assert_eq!(modules[0].project, "proj");
        assert_eq!(modules[0].file_path.as_deref(), Some("test.v"));
        assert!(modules[0].is_global, "top-level module should be global");
    }

    #[test]
    fn extracts_module_without_ports() {
        let result = extract("module bar; endmodule\n");
        let modules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Module)
            .collect();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "bar");
    }

    #[test]
    fn extracts_function_declaration() {
        let result = extract("module m; function integer add; endfunction endmodule\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(
            funcs.len(),
            1,
            "should extract 1 function: {:?}",
            result.nodes
        );
        assert_eq!(funcs[0].name, "add");
    }

    #[test]
    fn extracts_task_declaration_as_function() {
        // Tasks are void functions — extract as NodeLabel::Function.
        let result = extract("module m; task reset; endtask endmodule\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(
            funcs.len(),
            1,
            "should extract 1 task as function: {:?}",
            result.nodes
        );
        assert_eq!(funcs[0].name, "reset");
    }

    #[test]
    fn extracts_always_construct() {
        let result = extract("module m; always begin end endmodule\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(
            methods.len(),
            1,
            "should extract 1 always method: {:?}",
            result.nodes
        );
        assert_eq!(methods[0].name, "always");
        assert!(!methods[0].is_global, "always should not be global");
    }

    #[test]
    fn multiple_always_blocks_disambiguated() {
        let src = "module m;\nalways begin end\nalways begin end\nendmodule\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 always blocks");
        assert_ne!(
            methods[0].qualified_name, methods[1].qualified_name,
            "always blocks must have distinct FQNs via dedupe_qn"
        );
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_verilog() {
        let result = extract("module m; endmodule\n");
        assert_eq!(result.language, Language::Verilog);
        assert_eq!(result.file_path, "test.v");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("module m; endmodule\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract("module foo; endmodule\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.v.foo");
    }

    #[test]
    fn nested_function_inside_module_extracted() {
        let src = "module m; function integer f1; endfunction function integer f2; endfunction endmodule\n";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 2, "should extract 2 functions inside module");
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"f1"));
        assert!(names.contains(&"f2"));
    }
}
