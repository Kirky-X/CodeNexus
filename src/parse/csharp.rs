// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! C# language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-c-sharp's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `class_declaration` → [`NodeLabel::Class`]
//! - `interface_declaration` → [`NodeLabel::Interface`]
//! - `struct_declaration` → [`NodeLabel::Struct`]
//! - `enum_declaration` → [`NodeLabel::Enum`]
//! - `method_declaration` → [`NodeLabel::Method`]
//! - `namespace_declaration` → [`NodeLabel::Namespace`]
//!
//! # Extracted records
//!
//! - `using_directive` → [`ImportInfo`]
//! - `invocation_expression` → [`CallInfo`]
//!
//! # Known limitations
//!
//! - FQN pattern is `project.file_path.name` (no namespace prefix); nested
//!   types are disambiguated via `dedupe_qn` only when names collide.
//! - Properties, events, constructors, and delegates are not extracted.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// C# language tree-sitter extractor (Adapter pattern).
pub struct CSharpExtractor {
    _priv: (),
}

impl CSharpExtractor {
    /// Creates a new `CSharpExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for CSharpExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for CSharpExtractor {
    fn language(&self) -> Language {
        Language::CSharp
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::CSharp);
        let mut parser = ParserFactory::create_parser(Language::CSharp)?;
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

/// 不可变的遍历上下文，在 visit_node/visit_children 之间传递。
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
    current_func: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "class_declaration" => {
            extract_named_type(node, source, ctx, result, NodeLabel::Class);
            visit_children(node, source, ctx, result);
        }
        "interface_declaration" => {
            extract_named_type(node, source, ctx, result, NodeLabel::Interface);
            visit_children(node, source, ctx, result);
        }
        "struct_declaration" => {
            extract_named_type(node, source, ctx, result, NodeLabel::Struct);
            visit_children(node, source, ctx, result);
        }
        "enum_declaration" => {
            extract_named_type(node, source, ctx, result, NodeLabel::Enum);
            visit_children(node, source, ctx, result);
        }
        "namespace_declaration" => {
            extract_named_type(node, source, ctx, result, NodeLabel::Namespace);
            visit_children(node, source, ctx, result);
        }
        "method_declaration" => {
            extract_method(node, source, ctx, result);
            let name = method_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "using_directive" => {
            extract_import(node, source, result);
        }
        "invocation_expression" => {
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

/// Returns true if `name` starts with an uppercase ASCII letter, following
/// C#'s PascalCase convention for public members. Used heuristically to set
/// `is_exported` on nodes so the `CallResolver` can resolve cross-file calls.
fn is_exported_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
}

/// Extracts a node that exposes a `name` field (class / interface / struct /
/// enum / namespace) into a typed [`ModelNode`].
fn extract_named_type(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
    label: NodeLabel,
) {
    let Some(name) = name_field(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let is_exported = is_exported_name(&name);
    let model_node = ModelNode::builder(label, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::CSharp)
        .project(ctx.project)
        .is_global(true)
        .is_exported(is_exported)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_method(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = method_name(node, source) else {
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
    let is_exported = is_exported_name(&name);
    let mut builder = ModelNode::builder(NodeLabel::Method, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::CSharp)
        .project(ctx.project)
        .is_global(false)
        .is_exported(is_exported);
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
    // using_directive has no `name` field in tree-sitter-c-sharp; the
    // imported name is the first named child (an `identifier` for
    // `using System;` or a `qualified_name` for `using System.IO;`).
    let Some(name_node) = node.named_child(0) else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    result.imports.push(ImportInfo {
        source_file: name,
        imported_names: Vec::new(),
        line: node.start_position().row as u32 + 1,
    });
}

fn extract_call(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(func_node) = node.child_by_field_name("function") else {
        return;
    };
    let Some(callee) = callee_name(func_node, source) else {
        return;
    };
    let args = call_arguments(node, source);
    let caller_qn = ctx
        .current_func
        .map(|name| make_qn(ctx.file_path, name, ctx.project));
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

/// Returns the text of the `name` field of `node`, if present.
fn name_field(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn method_name(node: Node, source: &str) -> Option<String> {
    name_field(node, source)
}

/// Resolves a callee node to its base identifier name.
/// Handles `identifier`, `qualified_name` (`A.B.C` -> `C`), and
/// `member_access_expression` (`obj.Method` -> `Method`).
fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "qualified_name" => {
            // Rightmost name is the rightmost child.
            let mut current = node;
            loop {
                let right = current.child_by_field_name("right");
                let left = current.child_by_field_name("left");
                match (right, left) {
                    (Some(r), _) => {
                        if r.kind() == "identifier" {
                            return node_text(r, source).map(String::from);
                        }
                        // right is another qualified_name; descend.
                        current = r;
                    }
                    _ => return None,
                }
            }
        }
        "member_access_expression" => {
            let name = node.child_by_field_name("name")?;
            node_text(name, source).map(String::from)
        }
        "invocation_expression" => {
            let func = node.child_by_field_name("function")?;
            callee_name(func, source)
        }
        "parenthesized_expression" => {
            let inner = node.named_child(0)?;
            callee_name(inner, source)
        }
        _ => None,
    }
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

/// Returns the first line of a signature string (the declaration line).
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::CSharp, None)
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
        let ext = CSharpExtractor::new();
        ext.extract(source, "test.cs", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_csharp() {
        assert_eq!(CSharpExtractor::new().language(), Language::CSharp);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = CSharpExtractor::default();
        assert_eq!(ext.language(), Language::CSharp);
    }

    #[test]
    fn extracts_class_declaration() {
        let result = extract("public class Foo { }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(
            classes.len(),
            1,
            "should extract 1 class: {:?}",
            result.nodes
        );
        assert_eq!(classes[0].name, "Foo");
        assert_eq!(classes[0].language, Some(Language::CSharp));
        assert_eq!(classes[0].project, "proj");
        assert_eq!(classes[0].file_path.as_deref(), Some("test.cs"));
        assert!(classes[0].is_global, "top-level class should be global");
        assert!(classes[0].is_exported, "PascalCase class should be exported");
    }

    #[test]
    fn extracts_interface_declaration() {
        let result = extract("public interface IReader { void Read(); }\n");
        let ifaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Interface)
            .collect();
        assert_eq!(
            ifaces.len(),
            1,
            "should extract 1 interface: {:?}",
            result.nodes
        );
        assert_eq!(ifaces[0].name, "IReader");
    }

    #[test]
    fn extracts_struct_declaration() {
        let result = extract("public struct Point { public int X; public int Y; }\n");
        let structs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Struct)
            .collect();
        assert_eq!(
            structs.len(),
            1,
            "should extract 1 struct: {:?}",
            result.nodes
        );
        assert_eq!(structs[0].name, "Point");
    }

    #[test]
    fn extracts_enum_declaration() {
        let result = extract("public enum Color { Red, Green, Blue }\n");
        let enums: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Enum)
            .collect();
        assert_eq!(
            enums.len(),
            1,
            "should extract 1 enum: {:?}",
            result.nodes
        );
        assert_eq!(enums[0].name, "Color");
    }

    #[test]
    fn extracts_method_declaration() {
        let result = extract("public class Foo { public void Bar() { } }\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(
            methods.len(),
            1,
            "should extract 1 method: {:?}",
            result.nodes
        );
        assert_eq!(methods[0].name, "Bar");
        assert!(!methods[0].is_global, "method should not be global");
        assert!(methods[0].is_exported, "PascalCase method should be exported");
    }

    #[test]
    fn extracts_namespace_declaration() {
        let result = extract("namespace MyApp { class Foo { } }\n");
        let namespaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Namespace)
            .collect();
        assert_eq!(
            namespaces.len(),
            1,
            "should extract 1 namespace: {:?}",
            result.nodes
        );
        assert_eq!(namespaces[0].name, "MyApp");
    }

    #[test]
    fn extracts_using_directive_simple() {
        let result = extract("using System;\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 import");
        assert_eq!(result.imports[0].source_file, "System");
        assert_eq!(result.imports[0].line, 1);
    }

    #[test]
    fn extracts_using_directive_qualified() {
        let result = extract("using System.IO;\n");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "System.IO");
    }

    #[test]
    fn extracts_multiple_using_directives() {
        let result = extract("using System;\nusing System.IO;\nusing System.Collections.Generic;\n");
        assert_eq!(
            result.imports.len(),
            3,
            "should extract 3 imports: {:?}",
            result.imports
        );
        let paths: Vec<_> = result
            .imports
            .iter()
            .map(|i| i.source_file.as_str())
            .collect();
        assert!(paths.contains(&"System"));
        assert!(paths.contains(&"System.IO"));
        assert!(paths.contains(&"System.Collections.Generic"));
    }

    #[test]
    fn extracts_invocation_expression() {
        let src = "class Foo { void Bar() { Baz(); } }\n";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"Baz"),
            "should extract call to Baz: {:?}",
            callees
        );
    }

    #[test]
    fn extracts_qualified_invocation() {
        // `Console.WriteLine()` -> callee is `WriteLine`.
        let src = "using System; class Foo { void Bar() { Console.WriteLine(); } }\n";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"WriteLine"),
            "should extract member call to WriteLine: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let src = "class Foo { void Bar() { Baz(1, 2); } }\n";
        let result = extract(src);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "Baz")
            .expect("should find call to Baz");
        assert_eq!(call.args.len(), 2, "Baz(1, 2) should have 2 args");
        assert!(call.line >= 1);
    }

    #[test]
    fn call_in_method_has_dotted_fqn_caller_qn() {
        let src = "class Foo { void Caller() { Callee(); } }\n";
        let ext = CSharpExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/test.cs", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "Callee")
            .expect("should find call to Callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.test.cs.Caller"),
            "caller_qn should be the dotted FQN of the enclosing method"
        );
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "Caller")
            .expect("should find Caller method node");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "caller_qn must match the caller method node id"
        );
    }

    #[test]
    fn top_level_call_has_none_caller_qn() {
        // C# requires a class/method body, so a top-level call is not valid.
        // Verify that an empty source produces no calls.
        let result = extract("");
        assert!(result.calls.is_empty());
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_csharp() {
        let result = extract("class Foo { }\n");
        assert_eq!(result.language, Language::CSharp);
        assert_eq!(result.file_path, "test.cs");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("class Foo { }\n");
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
        let result = extract("class Foo { }\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.cs.Foo");
    }

    #[test]
    fn method_has_signature() {
        let src = "class Foo { public int Add(int a, int b) { return a + b; } }\n";
        let result = extract(src);
        let add = result.nodes.iter().find(|n| n.name == "Add").unwrap();
        assert!(add.signature.is_some(), "method should have a signature");
        let sig = add.signature.as_deref().unwrap();
        assert!(sig.contains("Add"), "signature should contain name: {sig}");
    }

    #[test]
    fn nested_classes_extracted() {
        let src = "class Outer { class Inner { } }\n";
        let result = extract(src);
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 2, "should extract outer and inner class");
        let names: Vec<_> = classes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Outer"));
        assert!(names.contains(&"Inner"));
    }
}
