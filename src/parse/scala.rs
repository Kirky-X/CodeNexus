// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Scala language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-scala's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_definition` → [`NodeLabel::Function`]
//! - `class_definition` → [`NodeLabel::Class`]
//! - `object_definition` → [`NodeLabel::Class`] (objects are singleton classes)
//! - `trait_definition` → [`NodeLabel::Trait`]
//! - `import_declaration` → [`ImportInfo`]
//! - `call_expression` → [`CallInfo`]
//!
//! # Known limitations
//!
//! - Pattern matching `match` expressions are not deeply analyzed.
//! - Given/using implicit parameters are not recorded.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// Scala language tree-sitter extractor (Adapter pattern).
pub struct ScalaExtractor {
    _priv: (),
}

impl ScalaExtractor {
    /// Creates a new `ScalaExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for ScalaExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for ScalaExtractor {
    fn language(&self) -> Language {
        Language::Scala
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Scala);
        let mut parser = ParserFactory::create_parser(Language::Scala)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        let ctx = VisitContext {
            file_path,
            project,
            current_func: None,
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

struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
    current_func: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function_definition" => {
            extract_function(node, source, ctx, result);
            let name = function_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "class_definition" => {
            extract_class(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "object_definition" => {
            extract_object(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "trait_definition" => {
            extract_trait(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "import_declaration" => {
            extract_import(node, source, result);
        }
        "call_expression" => {
            extract_call(node, source, ctx, result);
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
        .language(Language::Scala)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_class(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = definition_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Class, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Scala)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_object(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = definition_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Class, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Scala)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_trait(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = definition_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Trait, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Scala)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    // import_declaration has children that are identifier nodes forming the
    // package path. Concatenate them into a dot-separated path.
    let path = collect_identifier_path(node, source);
    if !path.is_empty() {
        result.imports.push(ImportInfo {
            source_file: path,
            imported_names: Vec::new(),
            line: node.start_position().row as u32 + 1,
        });
    }
}

fn extract_call(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let callee = callee_name(node, source);
    if let Some(callee) = callee {
        let caller_qn = ctx
            .current_func
            .map(|name| make_qn(ctx.file_path, name, ctx.project));
        result.calls.push(CallInfo {
            caller_qn,
            callee_name: callee,
            line: node.start_position().row as u32 + 1,
            args: Vec::new(),
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn function_name(node: Node, source: &str) -> Option<String> {
    // function_definition has an `identifier` child for the name.
    find_child_of_kind(node, "identifier", source)
}

fn definition_name(node: Node, source: &str) -> Option<String> {
    // class_definition / object_definition / trait_definition have an
    // `identifier` child for the name.
    find_child_of_kind(node, "identifier", source)
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    // call_expression has a `function` field or an identifier child.
    if let Some(func) = node.child_by_field_name("function") {
        return node_text(func, source).map(String::from);
    }
    find_child_of_kind(node, "identifier", source)
}

fn find_child_of_kind(node: Node, kind: &str, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == kind {
                return node_text(child, source).map(String::from);
            }
        }
    }
    None
}

fn collect_identifier_path(node: Node, source: &str) -> String {
    let mut parts = Vec::new();
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            collect_identifiers(child, source, &mut parts);
        }
    }
    parts.join(".")
}

fn collect_identifiers(node: Node, source: &str, parts: &mut Vec<String>) {
    if node.kind() == "identifier" {
        if let Some(text) = node_text(node, source) {
            parts.push(text.to_string());
        }
    }
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            collect_identifiers(child, source, parts);
        }
    }
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Scala, None)
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeLabel;

    fn extract(source: &str) -> ExtractResult {
        let ext = ScalaExtractor::new();
        ext.extract(source, "test.scala", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_scala() {
        assert_eq!(ScalaExtractor::new().language(), Language::Scala);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = ScalaExtractor::default();
        assert_eq!(ext.language(), Language::Scala);
    }

    #[test]
    fn extracts_function_definition() {
        let result = extract("object Foo { def bar(): Int = 1 }\n");
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
        assert_eq!(funcs[0].name, "bar");
    }

    #[test]
    fn extracts_class_definition() {
        let result = extract("class Foo { def bar(): Int = 1 }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1, "should extract 1 class: {:?}", result.nodes);
        assert_eq!(classes[0].name, "Foo");
    }

    #[test]
    fn extracts_object_definition() {
        let result = extract("object Bar { def foo(): Int = 1 }\n");
        let objects: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class && n.name == "Bar")
            .collect();
        assert_eq!(objects.len(), 1, "should extract 1 object: {:?}", result.nodes);
    }

    #[test]
    fn extracts_trait_definition() {
        let result = extract("trait Serializable { def serialize(): String }\n");
        let traits: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Trait)
            .collect();
        assert_eq!(traits.len(), 1, "should extract 1 trait: {:?}", result.nodes);
        assert_eq!(traits[0].name, "Serializable");
    }

    #[test]
    fn extracts_import() {
        let result = extract("import scala.collection.mutable\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 import");
        assert!(
            result.imports[0].source_file.contains("scala"),
            "import path should contain scala: {}",
            result.imports[0].source_file
        );
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_scala() {
        let result = extract("object Foo {}\n");
        assert_eq!(result.language, Language::Scala);
        assert_eq!(result.file_path, "test.scala");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("class Foo {}\n");
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
        let result = extract("class Foo {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.scala.Foo");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("object Foo { def bar(): Int = 1 }\n");
        let bar = result.nodes.iter().find(|n| n.name == "bar").unwrap();
        assert!(bar.signature.is_some(), "function should have a signature");
        assert!(bar.signature.as_deref().unwrap().contains("bar"));
    }

    #[test]
    fn extracts_call_to_function() {
        let result = extract("object Foo { def bar(): Int = 1\ndef main(): Int = bar() }\n");
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"bar"),
            "should extract call to bar: {:?}",
            callees
        );
    }

    #[test]
    fn comment_only_source_returns_empty() {
        let result = extract("// just a comment\n");
        assert!(result.is_empty(), "comment-only should produce no nodes");
    }

    #[test]
    fn multiple_imports_extracted() {
        let src = "import scala.collection.mutable\nimport akka.actor.Actor\n";
        let result = extract(src);
        assert_eq!(result.imports.len(), 2, "should extract 2 imports");
    }

    #[test]
    fn class_with_methods_extracts_all() {
        let src = "class Calculator {\n  def add(a: Int, b: Int): Int = a + b\n  def sub(a: Int, b: Int): Int = a - b\n}\n";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 2, "should extract 2 methods");
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"add"));
        assert!(names.contains(&"sub"));
    }

    #[test]
    fn nested_object_definition() {
        let src = "object Outer { object Inner { def foo(): Int = 1 } }\n";
        let result = extract(src);
        assert!(
            result.nodes.iter().any(|n| n.name == "Outer"),
            "should extract outer object"
        );
        assert!(
            result.nodes.iter().any(|n| n.name == "Inner"),
            "should extract inner object"
        );
    }

    #[test]
    fn trait_with_method_signature() {
        let src = "trait Drawable { def draw(): Unit }\n";
        let result = extract(src);
        let traits: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Trait)
            .collect();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].name, "Drawable");
    }

    #[test]
    fn call_inside_function_has_caller_qn() {
        let src = "object Foo { def main(): Int = { helper() } def helper(): Int = 1 }\n";
        let result = extract(src);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "helper")
            .expect("should find call to helper");
        assert!(
            call.caller_qn.is_some(),
            "call inside function should have caller_qn"
        );
    }

    #[test]
    fn empty_class_extracts_class_node() {
        let result = extract("class Empty\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Empty");
    }

    #[test]
    fn case_class_extracts_class_node() {
        let result = extract("case class Point(x: Int, y: Int)\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert!(!classes.is_empty(), "case class should be extracted");
    }
}
