// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! OCaml language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-ocaml's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `module_definition` → [`NodeLabel::Module`]
//! - `type_definition` → [`NodeLabel::TypeAlias`]
//! - `value_definition` → [`NodeLabel::Function`] (function-like let bindings)
//!   or [`NodeLabel::Variable`] (simple value bindings)
//!
//! # Known limitations
//!
//! - Functor applications are not deeply analyzed.
//! - Signature files (.mli) use a different LANGUAGE constant and are not
//!   handled by this extractor (only `LANGUAGE_OCAML` is wired).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// OCaml language tree-sitter extractor (Adapter pattern).
pub struct OCamlExtractor {
    _priv: (),
}

impl OCamlExtractor {
    /// Creates a new `OCamlExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for OCamlExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for OCamlExtractor {
    fn language(&self) -> Language {
        Language::OCaml
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::OCaml);
        let mut parser = ParserFactory::create_parser(Language::OCaml)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        let ctx = VisitContext { file_path, project };
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

struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "module_definition" => {
            extract_module(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "type_definition" => {
            extract_type_definition(node, source, ctx, result);
        }
        "value_definition" => {
            extract_value_definition(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "include_module" => {
            extract_include(node, source, result);
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
    // module_definition contains a module_name field or a module_path.
    let name = find_descendant_of_kind(node, "module_name", source)
        .or_else(|| find_descendant_of_kind(node, "module_path", source))
        .unwrap_or_else(|| "anonymous".to_string());
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Module, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::OCaml)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_type_definition(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // type_definition contains a type_binding whose `name` field is a type_constructor.
    let name = find_descendant_of_kind(node, "type_constructor", source)
        .unwrap_or_else(|| "anonymous_type".to_string());
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::TypeAlias, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::OCaml)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_value_definition(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // A value_definition contains let_binding(s). Each let_binding has a
    // binding_pattern (value_name) and optionally a body.
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "let_binding" {
                extract_let_binding(child, source, ctx, result);
            }
        }
    }
}

fn extract_let_binding(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // let_binding has a `pattern` field containing the value_name.
    let name = node
        .child_by_field_name("pattern")
        .and_then(|p| node_text(p, source).map(String::from))
        .unwrap_or_else(|| "_".to_string());
    // Check if this is a function (has parameters) or a simple value.
    let has_params = node.child_by_field_name("parameter").is_some()
        || find_descendant_of_kind(node, "parameter", source).is_some();
    let label = if has_params {
        NodeLabel::Function
    } else {
        NodeLabel::Variable
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(label, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::OCaml)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true);
    let signature = node_text(node, source)
        .map(signature_first_line)
        .map(String::from);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_include(node: Node, source: &str, result: &mut ExtractResult) {
    // include_module has a module_path child.
    let path = find_descendant_of_kind(node, "module_path", source);
    if let Some(p) = path {
        result.imports.push(ImportInfo {
            source_file: p,
            imported_names: Vec::new(),
            line: node.start_position().row as u32 + 1,
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::OCaml, None)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    result.edges.push(Edge::new(
        file_path.to_string(),
        node.id.clone(),
        EdgeType::Defines,
        project,
    ));
}

/// Recursively searches for a descendant node of the given kind and returns
/// its text. Used to locate identifiers deep in the OCaml AST.
fn find_descendant_of_kind(node: Node, kind: &str, source: &str) -> Option<String> {
    find_descendant_dfs(node, kind, source)
}

fn find_descendant_dfs(node: Node, kind: &str, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == kind {
                return node_text(child, source).map(String::from);
            }
            if let Some(found) = find_descendant_dfs(child, kind, source) {
                return Some(found);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeLabel;

    fn extract(source: &str) -> ExtractResult {
        let ext = OCamlExtractor::new();
        ext.extract(source, "test.ml", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_ocaml() {
        assert_eq!(OCamlExtractor::new().language(), Language::OCaml);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = OCamlExtractor::default();
        assert_eq!(ext.language(), Language::OCaml);
    }

    #[test]
    fn extracts_simple_let_binding() {
        let result = extract("let x = 1\n");
        let vars: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Variable)
            .collect();
        assert_eq!(
            vars.len(),
            1,
            "should extract 1 variable: {:?}",
            result.nodes
        );
        assert_eq!(vars[0].name, "x");
    }

    #[test]
    fn extracts_function_let_binding() {
        let result = extract("let add x y = x + y\n");
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
    fn extracts_type_definition() {
        let result = extract("type color = Red | Green | Blue\n");
        let types: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias)
            .collect();
        assert_eq!(types.len(), 1, "should extract 1 type: {:?}", result.nodes);
        assert_eq!(types[0].name, "color");
    }

    #[test]
    fn extracts_module_definition() {
        let result = extract("module Foo = struct\n  let x = 1\nend\n");
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
        assert_eq!(modules[0].name, "Foo");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_ocaml() {
        let result = extract("let x = 1\n");
        assert_eq!(result.language, Language::OCaml);
        assert_eq!(result.file_path, "test.ml");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("let x = 1\n");
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
        let result = extract("let foo = 1\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.ml.foo");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("let add x y = x + y\n");
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn include_statement_extracts_import() {
        let result = extract("include List\n");
        assert!(
            !result.imports.is_empty(),
            "include should produce an import: {:?}",
            result.imports
        );
    }

    #[test]
    fn include_qualified_module_extracts_import() {
        let result = extract("include MyLib.SubModule\n");
        assert!(
            result
                .imports
                .iter()
                .any(|i| i.source_file.contains("SubModule")),
            "should extract qualified include: {:?}",
            result.imports
        );
    }

    #[test]
    fn nested_module_extracts_inner_let() {
        let src = "module Foo = struct\n  let bar = 42\nend\n";
        let result = extract(src);
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.label == NodeLabel::Module && n.name == "Foo"),
            "should extract outer module"
        );
        assert!(
            result.nodes.iter().any(|n| n.name == "bar"),
            "should extract inner let binding"
        );
    }

    #[test]
    fn multiple_type_definitions() {
        let src = "type color = Red | Green\ntype shape = Circle | Square\n";
        let result = extract(src);
        let types: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias)
            .collect();
        assert_eq!(types.len(), 2, "should extract 2 types");
        let names: Vec<_> = types.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"color"));
        assert!(names.contains(&"shape"));
    }

    #[test]
    fn comment_only_source_returns_empty() {
        let result = extract("(* just a comment *)\n");
        assert!(result.is_empty(), "comment-only should produce no nodes");
    }

    #[test]
    fn let_with_complex_pattern() {
        let result = extract("let (x, y) = (1, 2)\n");
        assert!(
            !result.nodes.is_empty(),
            "should extract something from tuple pattern"
        );
    }

    #[test]
    fn nested_let_inside_function() {
        let src = "let outer x =\n  let inner = x + 1 in\n  inner * 2\n";
        let result = extract(src);
        assert!(
            result.nodes.iter().any(|n| n.name == "outer"),
            "should extract outer function"
        );
    }

    #[test]
    fn type_with_parameters() {
        let result = extract("type 'a tree = Leaf | Node of 'a * 'a tree * 'a tree\n");
        let types: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias)
            .collect();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].name, "tree");
    }

    #[test]
    fn module_without_name_does_not_panic() {
        let result = extract("module struct\n  let x = 1\nend\n");
        let _ = result;
    }
}
