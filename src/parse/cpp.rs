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
        "enum_specifier" => {
            // BUG-C2: enum_specifier was not handled, so C++ enums were
            // silently dropped. Extract as Enum node (gitnexus extracts enums).
            extract_type(node, source, ctx, result, NodeLabel::Enum);
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
    let is_inside = is_inside_class_or_struct(node);
    // BUG-C1: out-of-class method definitions (`void Foo::bar() {}`) have a
    // qualified_identifier declarator. Detect the qualifier (class name) so
    // these are classified as Method, not Function.
    let qualifier = extract_qualifier(node, source);
    let is_method = is_inside || qualifier.is_some();
    let label = if is_method {
        NodeLabel::Method
    } else {
        NodeLabel::Function
    };
    // Use the qualifier (from Foo::bar) as parent if present, otherwise the
    // enclosing scope from ctx.current_parent (for in-class methods).
    let parent: Option<&str> = qualifier.as_deref().or(ctx.current_parent);
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source).map(signature_first_line).map(String::from);
    // BUG-C4 (resolved, v0.3.0): C++ free functions are now is_exported=true
    // to enable cross-file call resolution. Over-resolution is prevented by
    // scope-aware lookup_exported_in_scope (T004/T005) which filters by
    // #include reachability via IncludesGraph. Methods remain is_exported=false.
    let is_exported = !is_method;
    let mut builder = ModelNode::builder(label, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Cpp)
        .project(ctx.project)
        .is_global(!is_method)
        .is_exported(is_exported);
    if let Some(p) = parent {
        builder = builder.parent_qn(p);
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
    let mut builder = ModelNode::builder(label, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Cpp)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);

    extract_heritage(node, source, ctx, &qn, result);
}

/// Extracts EXTENDS edges from a class/struct's `base_class_clause`.
///
/// tree-sitter-cpp exposes `base_class_clause` as a named **child** of
/// `class_specifier`/`struct_specifier` (not a field). The clause's named
/// children are a mix of `access_specifier` (public/private/protected),
/// `type_identifier`, `qualified_identifier`, and `template_type`. Only the
/// type nodes produce EXTENDS edges; access specifiers are skipped.
///
/// Target FQNs are best-effort (same-file scope); cross-file resolution is
/// deferred to the type resolver. For `qualified_identifier` (e.g. `ns::Base`)
/// only the rightmost name is used so the TypeResolver can match it against
/// the symbol table (which indexes by simple name).
fn extract_heritage(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    class_qn: &str,
    result: &mut ExtractResult,
) {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "base_class_clause" {
                for_each_type_name(child, source, &mut |parent_name| {
                    let parent_qn = make_qn(ctx.file_path, &parent_name, ctx.project, None);
                    result.edges.push(Edge::new(
                        class_qn.to_string(),
                        parent_qn,
                        EdgeType::Extends,
                        ctx.project,
                    ));
                });
            }
        }
    }
}

/// Recursively walks `base_class_clause` and its type children, invoking `f`
/// for each concrete base class name found.
///
/// - `type_identifier` / `identifier` / `namespace_identifier`: emit the text.
/// - `qualified_identifier` (`ns::Base`): recurse into the `name` field so
///   only `Base` is emitted (matches symbol-table lookup by simple name).
/// - `template_type` (`std::vector<int>`): recurse into the `name` field.
/// - `base_class_clause`: iterate named children (skips `access_specifier`).
fn for_each_type_name<F: FnMut(String)>(node: Node, source: &str, f: &mut F) {
    match node.kind() {
        "type_identifier" | "identifier" | "namespace_identifier" => {
            if let Some(text) = node_text(node, source) {
                f(text.to_string());
            }
        }
        "qualified_identifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                for_each_type_name(name, source, f);
            }
        }
        "template_type" => {
            if let Some(name) = node.child_by_field_name("name") {
                for_each_type_name(name, source, f);
            }
        }
        "base_class_clause" => {
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    for_each_type_name(child, source, f);
                }
            }
        }
        _ => {}
    }
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

/// Extracts the qualifier (class/namespace name) from a function_definition
/// whose declarator is a `qualified_identifier` (BUG-C1).
///
/// For `void Foo::bar() {}`, returns `Some("Foo")`. For a plain function
/// `void bar() {}`, returns `None`. Unwraps intermediate declarator nodes
/// (function_declarator, pointer_declarator, etc.) to reach the
/// qualified_identifier.
fn extract_qualifier(node: Node, source: &str) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    qualifier_from_declarator(declarator, source)
}

/// Recursively unwraps declarator nodes to find a `qualified_identifier`
/// and extract its `scope` field (the class/namespace name).
fn qualifier_from_declarator(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "qualified_identifier" => {
            let scope = node.child_by_field_name("scope")?;
            node_text(scope, source).map(String::from)
        }
        "function_declarator" | "pointer_declarator" | "reference_declarator"
        | "array_declarator" | "parenthesized_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            qualifier_from_declarator(inner, source)
        }
        _ => None,
    }
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

    // --- declarator_name branch coverage ---

    #[test]
    fn function_with_pointer_declarator_is_extracted() {
        // `int *get_ptr(void)` — declarator is pointer_declarator wrapping
        // function_declarator. Covers declarator_name's pointer_declarator arm.
        let result = extract("int *get_ptr(void) { return nullptr; }\n");
        let f = result
            .nodes
            .iter()
            .find(|n| n.name == "get_ptr")
            .expect("should extract get_ptr");
        assert_eq!(f.label, NodeLabel::Function);
    }

    #[test]
    fn function_with_reference_declarator_is_extracted() {
        // `int &ref_get(void)` — reference_declarator branch.
        let result = extract("int& ref_get(void) { static int x = 0; return x; }\n");
        let f = result
            .nodes
            .iter()
            .find(|n| n.name == "ref_get")
            .expect("should extract ref_get");
        assert_eq!(f.label, NodeLabel::Function);
    }

    #[test]
    fn function_with_qualified_identifier_name_is_extracted() {
        // `void ns::func()` — qualified_identifier declarator. Covers the
        // qualified_identifier arm of declarator_name.
        let result = extract("namespace ns { void ns_func() {} }\n");
        let f = result
            .nodes
            .iter()
            .find(|n| n.name == "ns_func")
            .expect("should extract ns_func");
        assert_eq!(f.label, NodeLabel::Function);
    }

    #[test]
    fn operator_overload_is_extracted() {
        // `bool operator==(const Foo&)` — operator_name declarator. Covers
        // the operator_name arm of declarator_name.
        let result = extract("class Foo { public: bool operator==(const Foo& other) const { return true; } };\n");
        let m = result
            .nodes
            .iter()
            .find(|n| n.name.contains("operator=="))
            .expect("should extract operator== as a method");
        assert_eq!(m.label, NodeLabel::Method);
    }

    // --- callee_name branch coverage ---

    #[test]
    fn call_to_qualified_name_is_extracted() {
        // `std::cout`-style qualified call. Covers callee_name's
        // qualified_identifier arm.
        let result = extract("int main() { std::swap(1, 2); return 0; }\n");
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"swap"),
            "should extract call to swap via qualified name: {:?}",
            callees
        );
    }

    #[test]
    fn call_to_method_is_extracted() {
        // `obj.method()` — field_expression callee. Covers callee_name's
        // field_expression arm.
        let result = extract(
            "class C { public: void m() {} };\nint main() { C c; c.m(); return 0; }\n",
        );
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"m"),
            "should extract call to method m via field_expression: {:?}",
            callees
        );
    }

    #[test]
    fn call_to_chained_invocation_is_extracted() {
        // `get_fn()()` — call_expression whose function is itself a
        // call_expression. Covers callee_name's call_expression arm.
        let result = extract("typedef void(*Fn)(); Fn get_fn(); int main() { get_fn()(); return 0; }\n");
        // The inner call `get_fn()` is extracted; the outer `()` call may
        // resolve to the same callee or to the result — either way, at least
        // one call record must exist.
        assert!(
            !result.calls.is_empty(),
            "chained call should produce at least one call record: {:?}",
            result.calls
        );
    }

    // --- signature_first_line coverage ---

    #[test]
    fn multi_line_function_signature_uses_first_line() {
        // A function whose declaration spans multiple lines must still get a
        // single-line signature (signature_first_line trims to the first line).
        let result = extract("int add(int a,\n        int b) {\n    return a + b;\n}\n");
        let add = result.nodes.iter().find(|n| n.name == "add").expect("add");
        let sig = add.signature.as_deref().expect("signature should be set");
        assert!(
            !sig.contains('\n'),
            "signature must be a single line, got: {sig:?}"
        );
        assert!(sig.contains("add"), "signature should contain the function name");
    }

    // --- enum extraction (extract_type with NodeLabel::Enum) ---

    #[test]
    fn enum_is_extracted_as_top_level_node() {
        // BUG-C2: enum_specifier should be extracted as an Enum node.
        // Previously enum was not extracted at all (the old test pinned the
        // missing behavior). gitnexus extracts enums; without this, C++
        // enum counts are zero in CodeNexus.
        let result = extract("enum Color { RED, GREEN, BLUE };\n");
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
    fn out_of_class_method_definition_is_classified_as_method() {
        // BUG-C1: `void Foo::bar() {}` is an out-of-class method definition.
        // The declarator is a qualified_identifier (Foo::bar). The previous
        // is_inside_class_or_struct only checked ancestors, not the declarator,
        // so this was misclassified as Function. It should be Method with
        // parent_qn = "Foo".
        let src = "class Foo {};\nvoid Foo::bar() {}\n";
        let result = extract(src);
        let bar = result
            .nodes
            .iter()
            .find(|n| n.name == "bar")
            .expect("should extract bar");
        assert_eq!(
            bar.label,
            NodeLabel::Method,
            "out-of-class Foo::bar should be Method, not Function: {:?}",
            bar.label
        );
        assert_eq!(
            bar.parent_qn.as_deref(),
            Some("Foo"),
            "parent_qn should be Foo: {:?}",
            bar.parent_qn
        );
    }

    // --- EXTENDS heritage extraction (base_class_clause) ---

    #[test]
    fn class_extends_base_creates_extends_edge() {
        let result = extract("class Base {};\nclass Derived : public Base {};\n");
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(extends.len(), 1, "should have 1 EXTENDS edge: {:?}", extends);
        assert!(
            extends[0].source.contains("Derived"),
            "source should be Derived: {}",
            extends[0].source
        );
        assert!(
            extends[0].target.contains("Base"),
            "target should be Base: {}",
            extends[0].target
        );
    }

    #[test]
    fn struct_extends_base_creates_extends_edge() {
        let result = extract("struct Base {};\nstruct Derived : Base {};\n");
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(extends.len(), 1, "struct should have 1 EXTENDS edge: {:?}", extends);
        assert!(
            extends[0].source.contains("Derived"),
            "source should be Derived: {}",
            extends[0].source
        );
    }

    #[test]
    fn multiple_inheritance_creates_multiple_extends_edges() {
        let result =
            extract("class Base1 {};\nclass Base2 {};\nclass Derived : public Base1, public Base2 {};\n");
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(
            extends.len(),
            2,
            "multiple inheritance should have 2 EXTENDS edges: {:?}",
            extends
        );
    }

    #[test]
    fn class_is_marked_exported() {
        let result = extract("class Point { public: int x; };\n");
        let cls = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Class)
            .expect("should find class Point");
        assert!(
            cls.is_exported,
            "class should be marked is_exported for TypeResolver strategy 3"
        );
    }

    #[test]
    fn struct_is_marked_exported() {
        let result = extract("struct Vec { int x; };\n");
        let st = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Struct)
            .expect("should find struct Vec");
        assert!(
            st.is_exported,
            "struct should be marked is_exported for TypeResolver strategy 3"
        );
    }

    #[test]
    fn cpp_free_function_is_exported() {
        // BUG-C4 (resolved, v0.3.0): C++ free functions are now is_exported=true
        // to enable cross-file call resolution. Over-resolution is prevented by
        // scope-aware lookup_exported_in_scope (T004/T005) via IncludesGraph.
        let result = extract("int add(int a, int b) { return a + b; }\n");
        let func = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(func.is_exported, "free function should have is_exported=true (BUG-C4 resolved)");
    }

    #[test]
    fn class_method_is_not_exported() {
        let result = extract("class Calc { public: int add(int a, int b) { return a + b; } };\n");
        let method = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(!method.is_exported, "class method should have is_exported=false");
    }

    #[test]
    fn struct_method_is_not_exported() {
        let result = extract("struct Vec { int length() { return 0; } };\n");
        let method = result.nodes.iter().find(|n| n.name == "length").unwrap();
        assert!(!method.is_exported, "struct method should have is_exported=false");
    }

    #[test]
    fn out_of_class_method_is_not_exported() {
        let result = extract("class Foo { public: void bar(); };\nvoid Foo::bar() {}\n");
        let methods: Vec<_> = result.nodes.iter().filter(|n| n.name == "bar").collect();
        assert!(!methods.is_empty(), "should find bar method");
        for m in &methods {
            assert!(!m.is_exported, "out-of-class method should have is_exported=false");
        }
    }

    #[test]
    fn cpp_namespace_function_is_exported() {
        let result = extract("namespace ns { int helper() { return 42; } }\n");
        let func = result.nodes.iter().find(|n| n.name == "helper").unwrap();
        assert!(func.is_exported, "namespace function should have is_exported=true (BUG-C4 resolved)");
    }

    #[test]
    fn private_inheritance_creates_extends_edge() {
        let result = extract("class Base {};\nclass Derived : private Base {};\n");
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(
            extends.len(),
            1,
            "private inheritance should still produce 1 EXTENDS edge: {:?}",
            extends
        );
    }

    #[test]
    fn qualified_base_class_creates_extends_edge() {
        // `ns::Base` — qualified_identifier in base_class_clause.
        let result = extract("namespace ns { class Base {}; }\nclass Derived : public ns::Base {};\n");
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(
            extends.len(),
            1,
            "qualified base class should produce 1 EXTENDS edge: {:?}",
            extends
        );
        assert!(
            extends[0].target.contains("Base"),
            "target should contain Base: {}",
            extends[0].target
        );
    }

    #[test]
    fn class_without_base_has_no_extends_edge() {
        let result = extract("class Standalone {};\n");
        let extends_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .count();
        assert_eq!(extends_count, 0, "class without base should have 0 EXTENDS edges");
    }

    #[test]
    fn cpp_multifile_call_graph_resolved() {
        // T007: End-to-end test for multi-file C++ call graph with #include
        // scoping (BUG-C4 fix verification).
        //
        // Scenario (header-only definition — common C++ pattern like STL):
        // - main.cpp: #includes "foo.h", calls foo()
        // - foo.h:    defines void foo() {} (is_exported=true after T006)
        // - bar.cpp:  defines void foo() {} (unrelated, NOT included by main.cpp)
        //
        // Assertions:
        // 1. main.cpp → foo.h has an INCLUDES edge in the IncludesGraph
        // 2. main.cpp's foo() call resolves to foo.h's foo (reachable via #include)
        // 3. Does NOT resolve to bar.cpp's foo (not reachable via #include)
        //
        // Note: The classic header/implementation split (foo.h declaration +
        // foo.cpp definition) is a known limitation — IncludesGraph only tracks
        // #include edges, not declaration→definition linking. For the call to
        // resolve, the definition must be in a file reachable via #include.
        use crate::resolve::{build_symbol_table, resolve_include, CallResolver, IncludesGraph};

        let ext = CppExtractor::new();

        let main_src = "#include \"foo.h\"\nint main() { foo(); return 0; }\n";
        let foo_h_src = "void foo() {}\n";
        let bar_src = "void foo() {}\n";

        let main_result = ext.extract(main_src, "/abs/main.cpp", "proj").unwrap();
        let foo_h_result = ext.extract(foo_h_src, "/abs/foo.h", "proj").unwrap();
        let bar_result = ext.extract(bar_src, "/abs/bar.cpp", "proj").unwrap();

        let results = vec![main_result, foo_h_result, bar_result];

        // Build IncludesGraph from #include directives using resolve_include.
        let all_files: Vec<String> = results.iter().map(|r| r.file_path.clone()).collect();
        let mut graph = IncludesGraph::new();
        for result in &results {
            for imp in &result.imports {
                if let Some(resolved) = resolve_include(&imp.source_file, &result.file_path, &all_files) {
                    graph.add_include(&result.file_path, &resolved);
                }
            }
        }

        // Assertion 1: main.cpp → foo.h INCLUDES edge exists.
        assert!(
            graph.contains("/abs/main.cpp", "/abs/foo.h"),
            "main.cpp should #include foo.h"
        );
        assert!(
            !graph.contains("/abs/main.cpp", "/abs/bar.cpp"),
            "main.cpp should NOT #include bar.cpp"
        );

        // Build symbol table and resolver.
        let table = build_symbol_table(&results, "proj");
        let resolver = CallResolver::new(&table, "proj").with_includes_graph(graph);

        // Assertion 2 & 3: call resolution with scope-aware filtering.
        let resolved = resolver.resolve_call("/abs/main.cpp", "foo");
        assert!(resolved.is_some(), "foo() call should resolve to foo.h's foo");
        let (qn, _confidence, _tier) = resolved.unwrap();
        assert!(
            qn.contains("foo.h"),
            "should resolve to foo.h's foo, got: {qn}"
        );
        assert!(
            !qn.contains("bar.cpp"),
            "should NOT resolve to bar.cpp's foo, got: {qn}"
        );
    }
}
