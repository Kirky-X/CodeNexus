// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! PHP language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-php's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_definition` → [`NodeLabel::Function`]
//! - `class_declaration` → [`NodeLabel::Class`]
//! - `method_declaration` → [`NodeLabel::Method`]
//! - `namespace_definition` → [`NodeLabel::Namespace`]
//!
//! # Extracted records
//!
//! - `namespace_use_declaration` → [`ImportInfo`]
//! - `function_call_expression` → [`CallInfo`]
//!
//! # Known limitations
//!
//! - PHP has no simple visibility rule; top-level declarations default to
//!   `is_exported = true` (module-level visibility).
//! - Interface, trait, and enum declarations are not yet extracted (only
//!   `class_declaration` is handled per the parsing spec).
//! - Member call expressions (`$obj->method()`) are not captured as CallInfo;
//!   only free function calls (`function_call_expression`) are extracted.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// PHP language tree-sitter extractor (Adapter pattern).
pub struct PhpExtractor {
    _priv: (),
}

impl PhpExtractor {
    /// Creates a new `PhpExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for PhpExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for PhpExtractor {
    fn language(&self) -> Language {
        Language::Php
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Php);
        let mut parser = ParserFactory::create_parser(Language::Php)?;
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
    /// The enclosing class name for methods (used as FQN disambiguator so
    /// same-name methods on different classes produce distinct FQNs).
    current_parent: Option<&'a str>,
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
                current_parent: None,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "class_declaration" => {
            extract_class(node, source, ctx, result);
            let name = class_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: None,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "method_declaration" => {
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
        "namespace_definition" => {
            extract_namespace(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "namespace_use_declaration" => {
            extract_use(node, source, result);
        }
        "function_call_expression" => {
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
        make_qn(ctx.file_path, &name, ctx.project, None),
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
        .language(Language::Php)
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
    let Some(name) = class_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Class, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Php)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_method(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = method_name(node, source) else {
        return;
    };
    // The enclosing class name is used as the FQN disambiguator so methods on
    // different classes with the same name produce distinct FQNs (ADR-003).
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source)
        .map(signature_first_line)
        .map(String::from);
    let mut builder = ModelNode::builder(NodeLabel::Method, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Php)
        .project(ctx.project)
        .is_global(false)
        .is_exported(false);
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent.to_string());
    }
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_namespace(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = namespace_name(node, source) else {
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
        .language(Language::Php)
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

/// Extracts each `namespace_use_clause` child of a
/// `namespace_use_declaration` as an [`ImportInfo`]. The `qualified_name`
/// child of the clause is used as the `source_file`.
fn extract_use(node: Node, source: &str, result: &mut ExtractResult) {
    let line = node.start_position().row as u32 + 1;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "namespace_use_clause" {
                if let Some(path) = use_clause_path(child, source) {
                    result.imports.push(ImportInfo {
                        source_file: path,
                        imported_names: Vec::new(),
                        line,
                        is_reexport: false,
                    });
                }
            }
        }
    }
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

fn function_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn class_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn method_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

/// Extracts the namespace name from a `namespace_definition`. The `name`
/// field is a `namespace_name` node whose text is the full dotted path
/// (e.g. `App\Controllers`).
fn namespace_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

/// Extracts the fully-qualified path from a `namespace_use_clause` by
/// locating its `qualified_name` child (which has no field label in
/// tree-sitter-php 0.24).
fn use_clause_path(node: Node, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "qualified_name" {
                return node_text(child, source).map(String::from);
            }
        }
    }
    None
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "name" => node_text(node, source).map(String::from),
        "qualified_name" => {
            // `\NS\foo` or `NS\foo` — take the last `name` segment.
            let mut last: Option<String> = None;
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if child.kind() == "name" {
                        last = node_text(child, source).map(String::from);
                    }
                }
            }
            last.or_else(|| node_text(node, source).map(String::from))
        }
        "function_call_expression" => {
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

/// Returns the first line of a signature string (the `function ...` line).
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Php, parent)
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
        let ext = PhpExtractor::new();
        ext.extract(source, "test.php", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_php() {
        assert_eq!(PhpExtractor::new().language(), Language::Php);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = PhpExtractor::default();
        assert_eq!(ext.language(), Language::Php);
    }

    #[test]
    fn extracts_function_definition() {
        let result = extract("<?php\nfunction foo() { return 1; }\n");
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
        assert_eq!(funcs[0].name, "foo");
        assert_eq!(funcs[0].language, Some(Language::Php));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.php"));
        assert!(funcs[0].is_global, "top-level function should be global");
        assert!(
            funcs[0].is_exported,
            "top-level function should be exported"
        );
    }

    #[test]
    fn extracts_class_declaration() {
        let result = extract("<?php\nclass Point { public function __construct() {} }\n");
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
        assert_eq!(classes[0].name, "Point");
        assert!(classes[0].is_global);
        assert!(classes[0].is_exported);
    }

    #[test]
    fn extracts_method_declaration() {
        let result = extract("<?php\nclass Foo { public function bar() { return 1; } }\n");
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
        assert_eq!(methods[0].name, "bar");
        assert!(!methods[0].is_global, "method should not be global");
    }

    #[test]
    fn method_fqn_disambiguated_by_class_name() {
        // Two methods named `render` on different classes should produce
        // distinct FQNs.
        let src = "<?php\nclass A { public function render() {} }\nclass B { public function render() {}\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method && n.name == "render")
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 render methods");
        assert_ne!(
            methods[0].qualified_name, methods[1].qualified_name,
            "methods on different classes must have distinct FQNs"
        );
    }

    #[test]
    fn extracts_namespace_definition() {
        let result = extract("<?php\nnamespace App\\Controllers;\n");
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
        assert_eq!(namespaces[0].name, "App\\Controllers");
        assert!(namespaces[0].is_global);
    }

    #[test]
    fn extracts_use_declaration() {
        let result = extract("<?php\nuse App\\Models\\User;\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 use declaration");
        assert_eq!(result.imports[0].source_file, "App\\Models\\User");
    }

    #[test]
    fn extracts_multiple_use_declarations() {
        let result = extract("<?php\nuse App\\Models\\User;\nuse App\\Services\\Mailer;\n");
        assert_eq!(result.imports.len(), 2, "should extract 2 use declarations");
        let paths: Vec<_> = result
            .imports
            .iter()
            .map(|i| i.source_file.as_str())
            .collect();
        assert!(
            paths.contains(&"App\\Models\\User"),
            "should import User: {:?}",
            paths
        );
        assert!(
            paths.contains(&"App\\Services\\Mailer"),
            "should import Mailer: {:?}",
            paths
        );
    }

    #[test]
    fn extracts_function_call_expression() {
        let result = extract("<?php\nfunction main() { foo(); }\n");
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"foo"),
            "should extract call to foo: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract("<?php\nfunction main() { foo(1, 2); }\n");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "foo")
            .expect("should find call to foo");
        assert_eq!(call.args.len(), 2, "foo(1, 2) should have 2 args");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_php() {
        let result = extract("<?php\nfunction foo() {}\n");
        assert_eq!(result.language, Language::Php);
        assert_eq!(result.file_path, "test.php");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("<?php\nfunction foo() {}\n");
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
        let result = extract("<?php\nfunction foo() {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.php.foo");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("<?php\nfunction add($a, $b) { return $a + $b; }\n");
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        let src = "<?php\nfunction caller() { callee(); }\n";
        let ext = PhpExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.php", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.php.caller"),
            "caller_qn should be the dotted FQN of the enclosing function"
        );
    }

    #[test]
    fn comment_only_source_returns_empty_result() {
        let result = extract("<?php\n// just a comment\n");
        assert!(
            result.is_empty(),
            "comment-only file should produce no nodes"
        );
    }

    #[test]
    fn multiple_classes_extracted() {
        let src = "<?php\nclass A {}\nclass B {}\nclass C {}\n";
        let result = extract(src);
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 3, "should extract 3 classes");
    }

    #[test]
    fn class_with_extends_does_not_break_extraction() {
        let src = "<?php\nclass Base {}\nclass Child extends Base {}\n";
        let result = extract(src);
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 2, "should extract both Base and Child");
        let names: Vec<_> = classes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Base"));
        assert!(names.contains(&"Child"));
    }

    #[test]
    fn class_with_implements_does_not_break_extraction() {
        let src = "<?php\ninterface IFoo {}\nclass Foo implements IFoo {}\n";
        let result = extract(src);
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1, "should extract Foo class");
        assert_eq!(classes[0].name, "Foo");
    }

    #[test]
    fn trait_declaration_does_not_break_extraction() {
        let src = "<?php\ntrait Loggable { public function log() {} }\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract log method from trait");
        assert_eq!(methods[0].name, "log");
    }

    #[test]
    fn method_with_parameters_has_signature() {
        let src = "<?php\nclass Foo { public function bar($a, $b) { return $a + $b; } }\n";
        let result = extract(src);
        let method = result
            .nodes
            .iter()
            .find(|n| n.name == "bar")
            .expect("should find bar method");
        assert!(method.signature.is_some());
        assert!(method.signature.as_deref().unwrap().contains("bar"));
    }

    #[test]
    fn qualified_function_call_extracts_last_component() {
        let src = "<?php\nfunction main() { \\NS\\foo(); }\n";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"foo"),
            "should extract last component of qualified call: {callees:?}"
        );
    }

    #[test]
    fn function_with_return_type() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\n";
        let result = extract(src);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "add")
            .expect("should find add function");
        assert_eq!(func.label, NodeLabel::Function);
        assert!(func.signature.is_some());
    }

    #[test]
    fn empty_class_extracted() {
        let src = "<?php\nclass EmptyClass {}\n";
        let result = extract(src);
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "EmptyClass");
    }

    #[test]
    fn nested_function_calls_extracted() {
        let src = "<?php\nfunction main() { outer(inner()); }\n";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"outer"),
            "should extract outer call: {callees:?}"
        );
        assert!(
            callees.contains(&"inner"),
            "should extract inner call: {callees:?}"
        );
    }

    #[test]
    fn method_call_in_class_context() {
        let src = "<?php\nclass Foo { public function bar() { baz(); } }\n";
        let result = extract(src);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "baz")
            .expect("should find call to baz");
        assert!(
            call.caller_qn.is_some(),
            "method call should have caller_qn"
        );
        assert!(call.caller_qn.as_deref().unwrap().contains("bar"));
    }

    #[test]
    fn parenthesized_call_expression_extracts_callee() {
        // Covers the `parenthesized_expression` branch in callee_name
        // (line 374-377): `(foo)()` should extract `foo` as the callee.
        let src = "<?php\nfunction main() { (foo)(); }\n";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"foo"),
            "should extract callee from parenthesized expression: {callees:?}"
        );
    }

    #[test]
    fn nested_function_call_expression_as_callee() {
        // Covers the `function_call_expression` branch in callee_name
        // (line 370-373): `foo()()` should extract `foo` as the callee of
        // the outer call.
        let src = "<?php\nfunction main() { getCallable()(); }\n";
        let result = extract(src);
        // The outer call's callee should be extracted from the inner
        // function_call_expression.
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"getCallable"),
            "should extract callee from nested function_call_expression: {callees:?}"
        );
    }

    #[test]
    fn call_without_arguments_extracts_empty_args() {
        // Covers the early return when `arguments` field is missing (line 383).
        let src = "<?php\nfunction main() { foo(); }\n";
        let result = extract(src);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "foo")
            .expect("should find call to foo");
        assert!(call.args.is_empty(), "foo() should have 0 args");
    }

    #[test]
    fn interface_declaration_does_not_break_extraction() {
        // Interface declarations should not crash the extractor (they are
        // not yet promoted to Class nodes, but the visitor should not break).
        let src =
            "<?php\ninterface IFoo { public function bar(); }\nclass Foo implements IFoo {}\n";
        let result = extract(src);
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1, "should still extract Foo class");
        assert_eq!(classes[0].name, "Foo");
    }

    #[test]
    fn abstract_class_with_method() {
        // Abstract class with abstract method should extract the method.
        let src = "<?php\nabstract class Base { abstract public function render(); }\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract render method");
        assert_eq!(methods[0].name, "render");
    }

    #[test]
    fn function_with_default_parameter_values() {
        // Function with default parameter values should still extract the
        // function and its signature.
        let src = "<?php\nfunction greet($name = 'World') { echo $name; }\n";
        let result = extract(src);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "greet")
            .expect("should find greet function");
        assert_eq!(func.label, NodeLabel::Function);
        assert!(func.signature.is_some());
        assert!(func.signature.as_deref().unwrap().contains("greet"));
    }

    #[test]
    fn class_with_static_method_and_property() {
        // Class with static method and static property should not crash.
        let src = "<?php\nclass Counter { private static $count = 0; public static function increment() { self::$count++; } }\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract increment method");
        assert_eq!(methods[0].name, "increment");
    }

    #[test]
    fn multiple_namespaces_in_one_file() {
        // Multiple namespace declarations should each produce a Namespace node.
        let src = "<?php\nnamespace App\\Models;\nnamespace App\\Services;\n";
        let result = extract(src);
        let namespaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Namespace)
            .collect();
        assert_eq!(namespaces.len(), 2, "should extract 2 namespaces");
    }

    #[test]
    fn function_with_variadic_parameter() {
        // Function with variadic parameter should extract correctly.
        let src = "<?php\nfunction sum(...$nums) { return array_sum($nums); }\n";
        let result = extract(src);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "sum")
            .expect("should find sum function");
        assert_eq!(func.label, NodeLabel::Function);
        assert!(func.signature.is_some());
    }

    #[test]
    fn class_with_constructor_and_destructor() {
        // Class with __construct and __destruct methods.
        let src = "<?php\nclass Lifecycle { public function __construct() {} public function __destruct() {} }\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 methods");
        let names: Vec<_> = methods.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"__construct"));
        assert!(names.contains(&"__destruct"));
    }
}
