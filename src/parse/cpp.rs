// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! C++ language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-cpp's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_definition` → [`NodeLabel::Function`] (or [`NodeLabel::Method`]
//!   when inside a class/struct)
//! - `class_specifier` → [`NodeLabel::Class`]
//! - `struct_specifier` → [`NodeLabel::Struct`]
//! - `namespace_definition` → [`NodeLabel::Namespace`]
//! - `template_declaration` → recurse into children (extract the inner
//!   function/class; the inner node's name is used)
//!
//! # FQN disambiguation
//!
//! Methods and namespace-scoped functions use the enclosing class or namespace
//! name as the FQN disambiguator (ADR-003).
//!
//! # Known limitations
//!
//! - C++ template type parameters are not deeply analyzed (only the type
//!   name is extracted, per the parsing spec Out-of-Scope).
//! - `#include` directives are not extracted as imports (tree-sitter-cpp
//!   represents them as `preproc_include`, which is handled separately).
//! - Operator overloading and conversion operators are extracted but the
//!   name may include operator symbols.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;
use super::dedupe_qn;

/// C++ language tree-sitter extractor (Adapter pattern).
pub struct CppExtractor {
    _priv: (),
}

impl CppExtractor {
    /// Creates a new `CppExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for CppExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for CppExtractor {
    fn language(&self) -> Language {
        Language::Cpp
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Cpp);
        let mut parser = ParserFactory::create_parser(Language::Cpp)?;
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
            in_template: false,
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
    /// The enclosing class/struct/namespace name, used as the FQN disambiguator
    /// for methods and namespace-scoped functions (ADR-003).
    current_parent: Option<&'a str>,
    /// Whether we are currently inside a template_declaration. Used to avoid
    /// double-extracting template-wrapped definitions.
    in_template: bool,
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
                current_parent: ctx.current_parent,
                in_template: ctx.in_template,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "class_specifier" => {
            extract_type(node, source, ctx, result, NodeLabel::Class);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
                in_template: ctx.in_template,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "struct_specifier" => {
            extract_type(node, source, ctx, result, NodeLabel::Struct);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
                in_template: ctx.in_template,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "namespace_definition" => {
            extract_namespace(node, source, ctx, result);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
                in_template: ctx.in_template,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "template_declaration" => {
            // Recurse into the template body to extract the inner function/class.
            // The inner definition is extracted normally; no separate Template
            // node is created (per spec: "Template OR Function").
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: ctx.current_parent,
                in_template: true,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "preproc_include" => {
            extract_include(node, source, result);
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

fn extract_function(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name) = function_name(node, source) else {
        return;
    };
    // If inside a class/struct, this is a Method; otherwise a Function.
    // A namespace-scoped function is still a Function (not a Method), so we
    // walk the ancestor chain to distinguish class/struct scope from
    // namespace scope (mirrors python.rs `is_inside_class`).
    let is_method = is_inside_class_or_struct(node);
    let label = if is_method {
        NodeLabel::Method
    } else {
        NodeLabel::Function
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source).map(signature_first_line).map(String::from);
    let mut builder = ModelNode::builder(label, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Cpp)
        .project(ctx.project)
        .is_global(!is_method);
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

fn extract_type(
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
        .language(Language::Cpp)
        .project(ctx.project)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_namespace(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name) = type_name(node, source) else {
        // Anonymous namespace — still recurse to extract its contents.
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Namespace, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Cpp)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

/// Extracts `#include` directives as import records.
fn extract_include(node: Node, source: &str, result: &mut ExtractResult) {
    // preproc_include has a `path` field (system_header or string_literal).
    let line = node.start_position().row as u32 + 1;
    if let Some(path_node) = node.child_by_field_name("path") {
        if let Some(text) = node_text(path_node, source) {
            // Strip surrounding < > or " " from the include path.
            let cleaned = text
                .trim_start_matches('<')
                .trim_end_matches('>')
                .trim_matches('"');
            result.imports.push(ImportInfo {
                source_file: cleaned.to_string(),
                imported_names: Vec::new(),
                line,
            });
        }
    }
}

fn extract_call(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // call_expression has a `function` field (the callee).
    let Some(func_node) = node.child_by_field_name("function") else {
        return;
    };
    let Some(callee) = callee_name(func_node, source) else {
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

/// Returns true if the node has a `class_specifier` or `struct_specifier`
/// ancestor (i.e. it is defined inside a class/struct body). Used to
/// distinguish methods from namespace-scoped free functions.
fn is_inside_class_or_struct(node: Node) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        match p.kind() {
            "class_specifier" | "struct_specifier" => return true,
            _ => cur = p.parent(),
        }
    }
    false
}

/// Extracts the `name` field from a class/struct/namespace declaration.
fn type_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

/// Extracts the function name from a `function_definition` node.
///
/// C++ function definitions have a `declarator` field that wraps the actual
/// name in nested `function_declarator`/`pointer_declarator`/etc. nodes. This
/// helper unwraps the declarator chain to find the base `identifier`.
fn function_name(node: Node, source: &str) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    declarator_name(declarator, source)
}

/// Recursively unwraps declarator nodes to find the base identifier name.
fn declarator_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "field_identifier" => {
            node_text(node, source).map(String::from)
        }
        "function_declarator" | "pointer_declarator" | "reference_declarator"
        | "array_declarator" | "parenthesized_declarator" => {
            // These declarator nodes have a `declarator` field pointing to
            // the inner declarator.
            if let Some(inner) = node.child_by_field_name("declarator") {
                declarator_name(inner, source)
            } else {
                // Fallback: search named children for a declarator-like node.
                for i in 0..node.named_child_count() as u32 {
                    if let Some(child) = node.named_child(i) {
                        if let Some(name) = declarator_name(child, source) {
                            return Some(name);
                        }
                    }
                }
                None
            }
        }
        "qualified_identifier" => {
            // `ns::func` — use the rightmost identifier (the function name).
            let name = node.child_by_field_name("name")?;
            node_text(name, source).map(String::from)
        }
        "operator_name" => {
            // Operator overloading — use the full operator name.
            node_text(node, source).map(String::from)
        }
        _ => {
            // Last resort: try the `declarator` field, then named children.
            if let Some(inner) = node.child_by_field_name("declarator") {
                return declarator_name(inner, source);
            }
            node_text(node, source).map(String::from)
        }
    }
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "qualified_identifier" => {
            // `std::cout` → use the rightmost identifier.
            let name = node.child_by_field_name("name")?;
            node_text(name, source).map(String::from)
        }
        "field_expression" => {
            // `obj.method()` → use the field name.
            let field = node.child_by_field_name("field")?;
            node_text(field, source).map(String::from)
        }
        "call_expression" => {
            let func = node.child_by_field_name("function")?;
            callee_name(func, source)
        }
        _ => node_text(node, source).map(String::from),
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

/// Returns the first line of a signature string.
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Cpp, parent)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    // DEFINES edge: file -> definition (matches the Python/C/Go/Java pattern).
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
        let ext = CppExtractor::new();
        ext.extract(source, "test.cpp", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_cpp() {
        assert_eq!(CppExtractor::new().language(), Language::Cpp);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = CppExtractor::default();
        assert_eq!(ext.language(), Language::Cpp);
    }

    #[test]
    fn extracts_function_definition() {
        let result = extract("int add(int a, int b) { return a + b; }\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 1, "should extract 1 function: {:?}", result.nodes);
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].language, Some(Language::Cpp));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.cpp"));
        assert!(funcs[0].is_global, "top-level function should be global");
    }

    #[test]
    fn extracts_class() {
        let result = extract("class Point { public: int x; };\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1, "should extract 1 class: {:?}", result.nodes);
        assert_eq!(classes[0].name, "Point");
    }

    #[test]
    fn extracts_struct() {
        let result = extract("struct Vec { int x; int y; };\n");
        let structs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Struct)
            .collect();
        assert_eq!(structs.len(), 1, "should extract 1 struct: {:?}", result.nodes);
        assert_eq!(structs[0].name, "Vec");
    }

    #[test]
    fn extracts_namespace() {
        let result = extract("namespace ns { void foo() {} }\n");
        let namespaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Namespace)
            .collect();
        assert_eq!(namespaces.len(), 1, "should extract 1 namespace: {:?}", result.nodes);
        assert_eq!(namespaces[0].name, "ns");
    }

    #[test]
    fn namespace_function_fqn_contains_namespace() {
        let result = extract("namespace ns { void foo() {} }\n");
        let foo = result
            .nodes
            .iter()
            .find(|n| n.name == "foo")
            .expect("should find function foo");
        assert!(
            foo.qualified_name.contains("ns"),
            "FQN should contain namespace name: {}",
            foo.qualified_name
        );
    }

    #[test]
    fn extracts_method_inside_class() {
        let result = extract("class C { void m() {} };\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract 1 method: {:?}", result.nodes);
        assert_eq!(methods[0].name, "m");
        assert!(!methods[0].is_global, "method should not be global");
        assert_eq!(methods[0].parent_qn.as_deref(), Some("C"));
    }

    #[test]
    fn extracts_template_function() {
        let result = extract(
            "template<typename T> T max(T a, T b) { return a > b ? a : b; }\n",
        );
        // Per spec: extract Template OR Function node named max.
        let max_node = result
            .nodes
            .iter()
            .find(|n| n.name == "max")
            .expect("should extract a node named max (Function or Template)");
        assert!(
            max_node.label == NodeLabel::Function || max_node.label == NodeLabel::Template,
            "max should be a Function or Template, got {:?}",
            max_node.label
        );
    }

    #[test]
    fn extracts_template_class() {
        let result = extract(
            "template<typename T> class Stack { T data[10]; };\n",
        );
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1, "should extract template class Stack");
        assert_eq!(classes[0].name, "Stack");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_cpp() {
        let result = extract("int main() { return 0; }\n");
        assert_eq!(result.language, Language::Cpp);
        assert_eq!(result.file_path, "test.cpp");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("int add(int a, int b) { return a + b; }\n");
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract("int add(int a, int b) { return a + b; }\n");
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert_eq!(add.qualified_name, "proj.test.cpp.add");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("int add(int a, int b) { return a + b; }\n");
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn extracts_call_expression() {
        let result = extract(
            "int main() { printf(\"hi\"); return 0; }\n",
        );
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"printf"),
            "should extract call to printf: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(
            "int main() { printf(\"hi\", 1); return 0; }\n",
        );
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "printf")
            .expect("should find call to printf");
        assert_eq!(call.args.len(), 2, "printf(\"hi\", 1) should have 2 args");
    }

    #[test]
    fn extracts_include() {
        let result = extract("#include <iostream>\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 include");
        assert_eq!(result.imports[0].source_file, "iostream");
    }

    #[test]
    fn extracts_local_include() {
        let result = extract("#include \"myheader.h\"\n");
        assert_eq!(result.imports.len(), 1, "should extract local include");
        assert_eq!(result.imports[0].source_file, "myheader.h");
    }

    #[test]
    fn nested_namespace_function_extracts_both() {
        let result = extract("namespace outer { void inner_func() {} }\n");
        let namespaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Namespace)
            .collect();
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(namespaces.len(), 1, "should extract namespace outer");
        assert_eq!(funcs.len(), 1, "should extract function inner_func");
        assert_eq!(namespaces[0].name, "outer");
        assert_eq!(funcs[0].name, "inner_func");
    }

    #[test]
    fn class_with_method_and_field_extracts_class_and_method() {
        let result = extract("class Point { public: int x; int getX() { return x; } };\n");
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
        assert_eq!(classes.len(), 1, "should extract class Point");
        assert_eq!(methods.len(), 1, "should extract method getX");
        assert_eq!(classes[0].name, "Point");
        assert_eq!(methods[0].name, "getX");
    }
}
