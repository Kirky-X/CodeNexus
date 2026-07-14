// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Ruby language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-ruby's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `method` → [`NodeLabel::Method`] (name field = `identifier`)
//! - `class` → [`NodeLabel::Class`] (name field)
//! - `module` → [`NodeLabel::Namespace`] (name field)
//!
//! # Extracted records
//!
//! - `call` → [`CallInfo`] (extracts receiver + method)
//!
//! # Known limitations
//!
//! - Ruby visibility modifiers (`private`/`protected`/`public`) are not
//!   tracked; all methods are treated as exported.
//! - Singleton methods (`def self.foo`) extract only the method name without
//!   the `self.` prefix.
//! - Calls without an explicit receiver (`foo(args)`) are parsed as
//!   `command`/`method_call` nodes by tree-sitter-ruby and are not extracted
//!   here (only `call` nodes with a receiver are handled).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor};
use super::parser_factory::ParserFactory;

/// Ruby language tree-sitter extractor (Adapter pattern).
pub struct RubyExtractor {
    _priv: (),
}

impl RubyExtractor {
    /// Creates a new `RubyExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for RubyExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for RubyExtractor {
    fn language(&self) -> Language {
        Language::Ruby
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Ruby);
        let mut parser = ParserFactory::create_parser(Language::Ruby)?;
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
    /// The enclosing class/module name, used as the FQN disambiguator
    /// for methods so same-name methods in different classes produce distinct
    /// FQNs (ADR-003).
    current_parent: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "method" => {
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
        "class" => {
            extract_class(node, source, ctx, result);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "module" => {
            extract_module(node, source, ctx, result);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "call" => {
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

fn extract_method(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = method_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source)
        .map(signature_first_line)
        .map(String::from);
    let is_global = ctx.current_parent.is_none();
    let mut builder = ModelNode::builder(NodeLabel::Method, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Ruby)
        .project(ctx.project)
        .is_global(is_global)
        .is_exported(true);
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent.to_string());
    }
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    if is_global {
        add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    }
    result.push_node(model_node);
}

fn extract_class(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = type_name(node, source) else {
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
    let mut builder = ModelNode::builder(NodeLabel::Class, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Ruby)
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

fn extract_module(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = type_name(node, source) else {
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
    let mut builder = ModelNode::builder(NodeLabel::Namespace, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Ruby)
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

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_call(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    // tree-sitter-ruby `call` node has:
    // - `receiver` field: the object expression (identifier, constant, ...)
    // - `method` field: the method name (identifier)
    // - optional `arguments` field: argument_list
    let Some(method_node) = node.child_by_field_name("method") else {
        return;
    };
    let Some(callee) = node_text(method_node, source).map(String::from) else {
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

fn method_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn type_name(node: Node, source: &str) -> Option<String> {
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
    FqnGenerator::generate(project, file_path, name, Language::Ruby, parent)
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
        let ext = RubyExtractor::new();
        ext.extract(source, "test.rb", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_ruby() {
        assert_eq!(RubyExtractor::new().language(), Language::Ruby);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = RubyExtractor::default();
        assert_eq!(ext.language(), Language::Ruby);
    }

    #[test]
    fn extracts_method() {
        let result = extract("def foo\n  1\nend\n");
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
        assert_eq!(methods[0].name, "foo");
        assert_eq!(methods[0].language, Some(Language::Ruby));
        assert_eq!(methods[0].project, "proj");
        assert_eq!(methods[0].file_path.as_deref(), Some("test.rb"));
        assert!(methods[0].is_global, "top-level method should be global");
    }

    #[test]
    fn extracts_method_with_params() {
        let result = extract("def greet(name)\n  puts name\nend\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract 1 method");
        assert_eq!(methods[0].name, "greet");
    }

    #[test]
    fn extracts_class() {
        let result = extract("class Foo\nend\n");
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
        assert!(classes[0].is_global);
    }

    #[test]
    fn extracts_module() {
        let result = extract("module MyModule\nend\n");
        let modules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Namespace)
            .collect();
        assert_eq!(
            modules.len(),
            1,
            "should extract 1 module: {:?}",
            result.nodes
        );
        assert_eq!(modules[0].name, "MyModule");
    }

    #[test]
    fn extracts_method_inside_class() {
        let result = extract("class Foo\n  def bar\n    1\n  end\nend\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract 1 method inside class");
        assert_eq!(methods[0].name, "bar");
        assert!(
            !methods[0].is_global,
            "method inside class should not be global"
        );
        assert_eq!(methods[0].parent_qn.as_deref(), Some("Foo"));
    }

    #[test]
    fn extracts_call() {
        let result = extract("def foo\n  obj.bar(1)\nend\n");
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
    fn call_has_line_and_args() {
        let result = extract("def foo\n  obj.bar(1, 2)\nend\n");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "bar")
            .expect("should find call to bar");
        assert_eq!(call.args.len(), 2, "bar(1, 2) should have 2 args");
        assert!(call.line > 0);
    }

    #[test]
    fn call_has_caller_qn() {
        let result = extract("def caller\n  obj.callee()\nend\n");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert!(
            call.caller_qn.is_some(),
            "call inside method should have caller_qn"
        );
        assert!(
            call.caller_qn.as_ref().unwrap().contains("caller"),
            "caller_qn should contain method name"
        );
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_ruby() {
        let result = extract("def foo\nend\n");
        assert_eq!(result.language, Language::Ruby);
        assert_eq!(result.file_path, "test.rb");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("class Foo\nend\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        assert!(
            defines_count >= 1,
            "should create at least 1 DEFINES edge for class"
        );
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract("class Foo\nend\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.rb.Foo");
    }

    #[test]
    fn method_has_signature() {
        let result = extract("def foo\n  1\nend\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert!(foo.signature.is_some(), "method should have a signature");
        assert!(
            foo.signature.as_deref().unwrap().contains("foo"),
            "signature should contain name"
        );
    }

    #[test]
    fn extracts_multiple_methods() {
        let result = extract("def foo\nend\ndef bar\nend\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 methods");
        let names: Vec<_> = methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn method_fqn_disambiguated_by_class() {
        let src = "class A\n  def read\n  end\nend\nclass B\n  def read\n  end\nend\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method && n.name == "read")
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 read methods");
        assert_ne!(
            methods[0].qualified_name, methods[1].qualified_name,
            "methods in different classes must have distinct FQNs"
        );
    }
}
