// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! JavaScript language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-javascript's syntax tree into CodeNexus nodes, edges,
//! and intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_declaration` → [`NodeLabel::Function`]
//! - `class_declaration` → [`NodeLabel::Class`]
//! - `method_definition` → [`NodeLabel::Method`]
//! - `variable_declarator` (top-level) → [`NodeLabel::Variable`]
//!
//! # Extracted records
//!
//! - `import_statement` → [`ImportInfo`]
//! - `call_expression` → [`CallInfo`]
//!
//! # Known limitations
//!
//! - JavaScript has no simple visibility rule; top-level declarations default
//!   to `is_exported = true` (module-level visibility).
//! - Arrow functions and anonymous function expressions are not extracted as
//!   standalone nodes (only named declarations are captured).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// JavaScript language tree-sitter extractor (Adapter pattern).
pub struct JavaScriptExtractor {
    _priv: (),
}

impl JavaScriptExtractor {
    /// Creates a new `JavaScriptExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for JavaScriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for JavaScriptExtractor {
    fn language(&self) -> Language {
        Language::JavaScript
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::JavaScript);
        let mut parser = ParserFactory::create_parser(Language::JavaScript)?;
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
        "function_declaration" => {
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
        "method_definition" => {
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
        "lexical_declaration" | "variable_declaration" => {
            if is_top_level(node) {
                extract_variables(node, source, ctx, result);
            }
            visit_children(node, source, ctx, result);
        }
        "import_statement" => {
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
        .language(Language::JavaScript)
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
        .language(Language::JavaScript)
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
        .language(Language::JavaScript)
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

/// Extracts each `variable_declarator` child of a top-level
/// `lexical_declaration` / `variable_declaration` as a [`NodeLabel::Variable`].
fn extract_variables(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "variable_declarator" {
                if let Some(name) = variable_name(child, source) {
                    let qn = dedupe_qn(
                        make_qn(ctx.file_path, &name, ctx.project, None),
                        child.start_position().row as u32 + 1,
                        result,
                    );
                    let model_node = ModelNode::builder(NodeLabel::Variable, name, qn)
                        .file_path(ctx.file_path)
                        .start_line(child.start_position().row as u32 + 1)
                        .language(Language::JavaScript)
                        .project(ctx.project)
                        .is_global(true)
                        .is_exported(true)
                        .build();
                    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
                    result.push_node(model_node);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    // import_statement has a `source` field (string_literal) holding the
    // module path, e.g. `import foo from './bar'` → source = `'./bar'`.
    let Some(source_node) = node.child_by_field_name("source") else {
        return;
    };
    let raw = node_text(source_node, source).unwrap_or("");
    // Strip surrounding quotes (single or double).
    let cleaned = raw.trim_matches(|c| c == '"' || c == '\'').to_string();
    result.imports.push(ImportInfo {
        source_file: cleaned,
        imported_names: Vec::new(),
        line: node.start_position().row as u32 + 1,
        is_reexport: false,
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

fn variable_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "member_expression" => {
            // `obj.method()` -> extract the property (rightmost) name.
            let property = node.child_by_field_name("property")?;
            node_text(property, source).map(String::from)
        }
        "call_expression" => {
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

/// Returns true if `node`'s direct parent is the `program` root.
fn is_top_level(node: Node) -> bool {
    node.parent()
        .map(|p| p.kind() == "program")
        .unwrap_or(false)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::JavaScript, parent)
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
        let ext = JavaScriptExtractor::new();
        ext.extract(source, "test.js", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_javascript() {
        assert_eq!(JavaScriptExtractor::new().language(), Language::JavaScript);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = JavaScriptExtractor::default();
        assert_eq!(ext.language(), Language::JavaScript);
    }

    #[test]
    fn extracts_function_declaration() {
        let result = extract("function foo() { return 1; }\n");
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
        assert_eq!(funcs[0].language, Some(Language::JavaScript));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.js"));
        assert!(funcs[0].is_global, "top-level function should be global");
        assert!(
            funcs[0].is_exported,
            "top-level function should be exported"
        );
    }

    #[test]
    fn extracts_class_declaration() {
        let result = extract("class Point { constructor() {} }\n");
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
    fn extracts_method_definition() {
        let result = extract("class Foo { bar() { return 1; } }\n");
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
        let src = "class A { render() {} }\nclass B { render() {} }\n";
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
    fn extracts_top_level_variable() {
        let result = extract("var x = 42;\n");
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
        assert!(vars[0].is_global);
    }

    #[test]
    fn extracts_let_and_const_variables() {
        let result = extract("let a = 1;\nconst b = 2;\n");
        let vars: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Variable)
            .collect();
        assert_eq!(vars.len(), 2, "should extract 2 variables (let + const)");
        let names: Vec<_> = vars.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn local_variable_not_extracted() {
        // Variables inside a function body should not be extracted as Variable
        // nodes (only top-level declarations are captured).
        let result = extract("function foo() { var local = 1; }\n");
        let vars: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Variable)
            .collect();
        assert!(
            vars.is_empty(),
            "local variable should not be extracted: {:?}",
            vars
        );
    }

    #[test]
    fn extracts_import_statement() {
        let result = extract("import foo from './bar'\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 import");
        assert_eq!(result.imports[0].source_file, "./bar");
    }

    #[test]
    fn extracts_named_import() {
        let result = extract("import { a, b } from './mod'\n");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "./mod");
    }

    #[test]
    fn extracts_side_effect_import() {
        let result = extract("import './polyfill'\n");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "./polyfill");
    }

    #[test]
    fn extracts_call_expression() {
        let result = extract("function main() { foo(); }\n");
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
    fn extracts_member_call() {
        let result = extract("function main() { console.log(); }\n");
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"log"),
            "should extract member call to log: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract("function main() { foo(1, 2); }\n");
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
    fn result_language_is_javascript() {
        let result = extract("function foo() {}\n");
        assert_eq!(result.language, Language::JavaScript);
        assert_eq!(result.file_path, "test.js");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("function foo() {}\n");
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
        let result = extract("function foo() {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.js.foo");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("function add(a, b) { return a + b; }\n");
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        let src = "function caller() { callee(); }\n";
        let ext = JavaScriptExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.js", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.js.caller"),
            "caller_qn should be the dotted FQN of the enclosing function"
        );
    }

    #[test]
    fn top_level_call_has_none_caller_qn() {
        // A call at the top level (not inside a function) has no caller_qn.
        let result = extract("foo();\n");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "foo")
            .expect("should find call to foo");
        assert!(
            call.caller_qn.is_none(),
            "top-level call should have None caller_qn"
        );
    }

    #[test]
    fn comment_only_source_returns_empty() {
        let result = extract("// just a comment\n");
        assert!(result.is_empty(), "comment-only should produce no nodes");
    }

    #[test]
    fn parenthesized_call_extracts_callee() {
        let result = extract("function foo() {}\n(foo)();\n");
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"foo"),
            "should extract parenthesized call to foo: {:?}",
            callees
        );
    }

    #[test]
    fn chained_call_extracts_callee() {
        let result = extract("function getFunc() { return function() {} }\ngetFunc()();\n");
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"getFunc"),
            "should extract chained call to getFunc: {:?}",
            callees
        );
    }

    #[test]
    fn generator_function_does_not_break_extraction() {
        let result = extract("function* gen() { yield 1; }\n");
        assert!(result.nodes.iter().all(|n| !n.name.is_empty()));
    }

    #[test]
    fn class_extends_does_not_break_extraction() {
        let result = extract("class Dog extends Animal { bark() {} }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Dog");
    }

    #[test]
    fn export_function_extracts_function() {
        let result = extract("export function foo() { return 1; }\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "foo");
    }

    #[test]
    fn export_default_class_extracts_class() {
        let result = extract("export default class Foo {}\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Foo");
    }

    #[test]
    fn multiple_classes_extracted() {
        let result = extract("class A {}\nclass B {}\nclass C {}\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 3, "should extract 3 classes");
    }

    #[test]
    fn import_with_alias_extracts_import() {
        let result = extract("import { foo as bar } from './mod'\n");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "./mod");
    }

    #[test]
    fn const_arrow_function_not_extracted_as_function() {
        let result = extract("const fn = () => 1;\n");
        let vars: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Variable)
            .collect();
        assert_eq!(
            vars.len(),
            1,
            "arrow function assigned to const is a Variable"
        );
        assert_eq!(vars[0].name, "fn");
    }

    #[test]
    fn class_with_getter_method_does_not_break() {
        let result = extract("class Foo { get value() { return 42; } }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Foo");
    }
}
