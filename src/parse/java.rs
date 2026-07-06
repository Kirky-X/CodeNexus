// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Java language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-java's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `class_declaration` → [`NodeLabel::Class`]
//! - `interface_declaration` → [`NodeLabel::Interface`]
//! - `enum_declaration` → [`NodeLabel::Enum`]
//! - `method_declaration` → [`NodeLabel::Method`] (enclosing class name used
//!   as disambiguator in the FQN)
//! - `constructor_declaration` → [`NodeLabel::Method`]
//!
//! # Extracted records
//!
//! - `import_declaration` → [`ImportInfo`]
//! - `method_invocation` → [`CallInfo`]
//!
//! # Known limitations
//!
//! - Java generics type parameters are not deeply analyzed (only the type
//!   name is extracted, per the parsing spec Out-of-Scope).
//! - Annotation arguments are not extracted.
//! - Nested classes are extracted but their FQN does not include the outer
//!   class name (only the file path + innermost name).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;
use super::dedupe_qn;

/// Java language tree-sitter extractor (Adapter pattern).
pub struct JavaExtractor {
    _priv: (),
}

impl JavaExtractor {
    /// Creates a new `JavaExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for JavaExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for JavaExtractor {
    fn language(&self) -> Language {
        Language::Java
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Java);
        let mut parser = ParserFactory::create_parser(Language::Java)?;
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
            current_parent: None,
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
    current_func: Option<&'a str>,
    /// The enclosing class/interface/enum name, used as the FQN disambiguator
    /// for methods so same-name methods in different classes produce distinct
    /// FQNs (ADR-003).
    current_parent: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "class_declaration" => {
            extract_class(node, source, ctx, result, NodeLabel::Class);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "interface_declaration" => {
            extract_class(node, source, ctx, result, NodeLabel::Interface);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "enum_declaration" => {
            extract_class(node, source, ctx, result, NodeLabel::Enum);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "method_declaration" | "constructor_declaration" => {
            extract_method(node, source, ctx, result);
            let name = method_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: name.as_deref(),
                current_parent: ctx.current_parent,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "import_declaration" => {
            extract_import(node, source, result);
        }
        "method_invocation" => {
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

/// Extracts a class/interface/enum declaration. `label` selects the node label.
fn extract_class(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
    label: NodeLabel,
) {
    let Some(name) = type_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source).map(signature_first_line).map(String::from);
    let mut builder = ModelNode::builder(label, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Java)
        .project(ctx.project)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_method(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name) = method_name(node, source) else {
        return;
    };
    // The enclosing class name is used as the FQN disambiguator so methods in
    // different classes with the same name produce distinct FQNs (ADR-003).
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source).map(signature_first_line).map(String::from);
    let mut builder = ModelNode::builder(NodeLabel::Method, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Java)
        .project(ctx.project)
        .is_global(false);
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    // import_declaration has a `name` field (scoped_identifier for
    // `java.util.List`, or identifier for single-segment imports) and an
    // optional `wildcard` child for `import java.util.*`.
    let line = node.start_position().row as u32 + 1;
    let path = if let Some(name_node) = node.child_by_field_name("name") {
        node_text(name_node, source).map(String::from)
    } else {
        // Fallback: walk named children for a scoped_identifier/identifier.
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                match child.kind() {
                    "scoped_identifier" | "identifier" => {
                        return push_import_text(child, source, line, result);
                    }
                    _ => {}
                }
            }
        }
        None
    };
    if let Some(p) = path {
        result.imports.push(ImportInfo {
            source_file: p,
            imported_names: Vec::new(),
            line,
        });
    }
}

fn push_import_text(node: Node, source: &str, line: u32, result: &mut ExtractResult) {
    if let Some(text) = node_text(node, source) {
        result.imports.push(ImportInfo {
            source_file: text.to_string(),
            imported_names: Vec::new(),
            line,
        });
    }
}

fn extract_call(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // method_invocation has a `name` field (the method name) and an optional
    // `object` field (the receiver expression).
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(callee) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let args = call_arguments(node, source);
    let caller_qn = ctx
        .current_func
        .map(|name| make_qn(ctx.file_path, name, ctx.project, ctx.current_parent));
    result.calls.push(CallInfo {
        caller_qn,
        callee_name: callee,
        line: node.start_position().row as u32 + 1,
        args,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the `name` field from a class/interface/enum declaration.
fn type_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn method_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn call_arguments(node: Node, source: &str) -> Vec<String> {
    let Some(args_node) = node.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut args = Vec::new();
    for i in 0..args_node.named_child_count() as u32 {
        if let Some(arg) = args_node.named_child(i) {
            if let Ok(text) = arg.utf8_text(source.as_bytes()) {
                args.push(text.to_string());
            }
        }
    }
    args
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

/// Returns the first line of a signature string.
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Java, parent)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    // DEFINES edge: file -> definition (matches the Python/C/Go pattern).
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
        let ext = JavaExtractor::new();
        ext.extract(source, "test.java", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_java() {
        assert_eq!(JavaExtractor::new().language(), Language::Java);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = JavaExtractor::default();
        assert_eq!(ext.language(), Language::Java);
    }

    #[test]
    fn extracts_class_declaration() {
        let result = extract("class Foo {}\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1, "should extract 1 class: {:?}", result.nodes);
        assert_eq!(classes[0].name, "Foo");
        assert_eq!(classes[0].language, Some(Language::Java));
        assert_eq!(classes[0].project, "proj");
        assert_eq!(classes[0].file_path.as_deref(), Some("test.java"));
        assert!(classes[0].is_global, "top-level class should be global");
    }

    #[test]
    fn extracts_interface_declaration() {
        let result = extract("interface Bar { void baz(); }\n");
        let ifaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Interface)
            .collect();
        assert_eq!(ifaces.len(), 1, "should extract 1 interface: {:?}", result.nodes);
        assert_eq!(ifaces[0].name, "Bar");
    }

    #[test]
    fn extracts_enum_declaration() {
        let result = extract("enum Color { RED, GREEN }\n");
        let enums: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Enum)
            .collect();
        assert_eq!(enums.len(), 1, "should extract 1 enum: {:?}", result.nodes);
        assert_eq!(enums[0].name, "Color");
    }

    #[test]
    fn extracts_method_declaration() {
        let result = extract("class Foo { void bar() {} }\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract 1 method: {:?}", result.nodes);
        assert_eq!(methods[0].name, "bar");
        assert!(!methods[0].is_global, "method should not be global");
        // The enclosing class name is used as the FQN disambiguator.
        assert!(
            methods[0].qualified_name.contains("Foo"),
            "FQN should contain class name: {}",
            methods[0].qualified_name
        );
        assert_eq!(methods[0].parent_qn.as_deref(), Some("Foo"));
    }

    #[test]
    fn method_fqn_is_disambiguated_by_class_name() {
        // Two methods named bar in different classes should produce distinct FQNs.
        let src = "class A { void bar() {} }\nclass B { void bar() {} }\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method && n.name == "bar")
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 bar methods");
        assert_ne!(
            methods[0].qualified_name, methods[1].qualified_name,
            "methods in different classes must have distinct FQNs"
        );
    }

    #[test]
    fn extracts_constructor_declaration() {
        let result = extract("class Foo { Foo() {} }\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "constructor should be a Method: {:?}", result.nodes);
        assert_eq!(methods[0].name, "Foo");
    }

    #[test]
    fn extracts_import() {
        let result = extract("import java.util.List;\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 import");
        assert_eq!(result.imports[0].source_file, "java.util.List");
    }

    #[test]
    fn extracts_multiple_imports() {
        let result = extract(
            "import java.util.List;\nimport java.util.Map;\n",
        );
        assert_eq!(result.imports.len(), 2, "should extract 2 imports: {:?}", result.imports);
        let paths: Vec<_> = result.imports.iter().map(|i| i.source_file.as_str()).collect();
        assert!(paths.contains(&"java.util.List"), "should import List: {:?}", paths);
        assert!(paths.contains(&"java.util.Map"), "should import Map: {:?}", paths);
    }

    #[test]
    fn extracts_static_import() {
        let result = extract("import static java.util.Math.PI;\n");
        assert_eq!(result.imports.len(), 1, "should extract static import");
        assert_eq!(result.imports[0].source_file, "java.util.Math.PI");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_java() {
        let result = extract("class Foo {}\n");
        assert_eq!(result.language, Language::Java);
        assert_eq!(result.file_path, "test.java");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("class Foo {}\n");
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract("class Foo {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.java.Foo");
    }

    #[test]
    fn class_has_signature() {
        let result = extract("public class Foo implements Runnable {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert!(foo.signature.is_some(), "class should have a signature");
        assert!(foo.signature.as_deref().unwrap().contains("Foo"));
    }

    #[test]
    fn method_has_signature() {
        let result = extract("class Foo { public int bar(int x) { return x; } }\n");
        let bar = result.nodes.iter().find(|n| n.name == "bar").unwrap();
        assert!(bar.signature.is_some(), "method should have a signature");
        assert!(bar.signature.as_deref().unwrap().contains("bar"));
    }

    #[test]
    fn extracts_method_invocation() {
        let result = extract(
            "class Foo { void run() { doSomething(); } }\n",
        );
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"doSomething"),
            "should extract call to doSomething: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(
            "class Foo { void run() { doSomething(1, 2); } }\n",
        );
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "doSomething")
            .expect("should find call to doSomething");
        assert_eq!(call.args.len(), 2, "doSomething(1, 2) should have 2 args");
    }

    #[test]
    fn nested_class_extracts_inner_class() {
        let result = extract("class Outer { class Inner {} }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 2, "should extract outer + inner class");
        let names: Vec<_> = classes.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Outer"));
        assert!(names.contains(&"Inner"));
    }

    #[test]
    fn class_with_method_with_body_extracts_both() {
        let result = extract("class Foo { void bar() { System.out.println(\"hi\"); } }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(classes.len(), 1, "should extract class Foo");
        assert_eq!(methods.len(), 1, "should extract method bar");
        assert_eq!(classes[0].name, "Foo");
        assert_eq!(methods[0].name, "bar");
    }
}
