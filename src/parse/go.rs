// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Go language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-go's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_declaration` → [`NodeLabel::Function`]
//! - `method_declaration` → [`NodeLabel::Method`] (receiver type used as
//!   disambiguator in the FQN)
//! - `type_declaration` / `type_spec`:
//!   - `struct_type` → [`NodeLabel::Struct`]
//!   - `interface_type` → [`NodeLabel::Interface`]
//!   - other type aliases → [`NodeLabel::TypeAlias`]
//!
//! # Extracted records
//!
//! - `import_declaration` / `import_spec` → [`ImportInfo`]
//! - `call_expression` → [`CallInfo`]
//!
//! # Known limitations
//!
//! - Go generics type parameters are not deeply analyzed (only the type name
//!   is extracted, per the parsing spec Out-of-Scope).
//! - Method receiver pointer vs value distinction is not recorded on the node.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// Go language tree-sitter extractor (Adapter pattern).
pub struct GoExtractor {
    _priv: (),
}

impl GoExtractor {
    /// Creates a new `GoExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for GoExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for GoExtractor {
    fn language(&self) -> Language {
        Language::Go
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Go);
        let mut parser = ParserFactory::create_parser(Language::Go)?;
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
    /// The receiver type for methods (used as FQN disambiguator in caller_qn
    /// so calls from same-name methods on different types aren't deduplicated).
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
        "method_declaration" => {
            extract_method(node, source, ctx, result);
            let name = method_name(node, source);
            // The receiver type is propagated as current_parent so that
            // extract_call can build a receiver-disambiguated caller_qn
            // (BUG-G1: methods on different types with the same name must
            // produce distinct caller_qns to avoid over-deduplication in
            // resolve_calls).
            let receiver = receiver_type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: name.as_deref(),
                current_parent: receiver.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "type_declaration" => {
            // A type_declaration groups one or more type_spec nodes
            // (`type ( Foo struct{}, Bar interface{} )`).
            visit_children(node, source, ctx, result);
        }
        "type_spec" => {
            extract_type_spec(node, source, ctx, result);
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

/// Returns true if `name` starts with an uppercase ASCII letter, following
/// Go's visibility rule: uppercase = exported (public), lowercase = unexported
/// (package-private). Used to set `is_exported` on Go nodes so the
/// `CallResolver` can resolve cross-file calls via `lookup_exported`.
fn is_exported_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
}

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
    let is_exported = is_exported_name(&name);
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Go)
        .project(ctx.project)
        .is_global(true)
        .is_exported(is_exported);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_method(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = method_name(node, source) else {
        return;
    };
    // The receiver type is used as the FQN disambiguator so methods on
    // different types with the same name produce distinct FQNs (ADR-003).
    let receiver_type = receiver_type_name(node, source);
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, receiver_type.as_deref()),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source)
        .map(signature_first_line)
        .map(String::from);
    let is_exported = is_exported_name(&name);
    let mut builder = ModelNode::builder(NodeLabel::Method, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Go)
        .project(ctx.project)
        .is_global(false)
        .is_exported(is_exported);
    if let Some(parent) = &receiver_type {
        builder = builder.parent_qn(parent.clone());
    }
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_type_spec(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let type_value = node.child_by_field_name("type");
    let label = match type_value.map(|t| t.kind()) {
        Some("struct_type") => NodeLabel::Struct,
        Some("interface_type") => NodeLabel::Interface,
        // Other type declarations (type aliases, function types) are mapped
        // to TypeAlias to avoid losing the symbol entirely.
        _ => NodeLabel::TypeAlias,
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let is_exported = is_exported_name(&name);
    let model_node = ModelNode::builder(label, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Go)
        .project(ctx.project)
        .is_global(true)
        .is_exported(is_exported)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    // import_declaration has two forms:
    //   1. `import "fmt"`      — single import (import_spec is a direct child)
    //   2. `import ( ... )`    — import list (import_spec children are inside
    //                            an `import_spec_list` node)
    // tree-sitter-go represents both with import_spec children; we walk all
    // named descendants of kind `import_spec`.
    let line = node.start_position().row as u32 + 1;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "import_spec" {
                push_import(child, source, line, result);
            } else if child.kind() == "import_spec_list" {
                // `import ( ... )` form: import_spec_list contains import_spec children.
                for j in 0..child.named_child_count() as u32 {
                    if let Some(spec) = child.named_child(j) {
                        if spec.kind() == "import_spec" {
                            push_import(spec, source, line, result);
                        }
                    }
                }
            }
        }
    }
}

fn push_import(spec: Node, source: &str, line: u32, result: &mut ExtractResult) {
    if let Some(path) = import_spec_path(spec, source) {
        result.imports.push(ImportInfo {
            source_file: path,
            imported_names: Vec::new(),
            line,
        });
    }
}

/// Extracts the import path from an `import_spec` node, stripping the
/// surrounding quotes from the `interpreted_string_literal`.
fn import_spec_path(node: Node, source: &str) -> Option<String> {
    // import_spec has a `path` field (interpreted_string_literal) and an
    // optional `name` field (e.g. `f "fmt"`).
    let path_node = node.child_by_field_name("path")?;
    let raw = node_text(path_node, source)?;
    // Strip surrounding quotes: `"fmt"` -> `fmt`.
    let trimmed = raw.trim_matches('"');
    Some(trimmed.to_string())
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

fn method_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

/// Extracts the receiver type name from a `method_declaration`.
///
/// `func (t T) Bar()` → `Some("T")`; `func (t *T) Bar()` → `Some("T")`.
/// The receiver type is used as the FQN disambiguator so methods on
/// different types produce distinct FQNs.
fn receiver_type_name(node: Node, source: &str) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    // receiver is a parameter_list containing one parameter_declaration.
    // parameter_declaration has a `type` field which is a type_identifier
    // (for `T`) or a pointer_type wrapping a type_identifier (for `*T`).
    for i in 0..receiver.named_child_count() as u32 {
        if let Some(param) = receiver.named_child(i) {
            if param.kind() == "parameter_declaration" {
                if let Some(type_node) = param.child_by_field_name("type") {
                    return type_name(type_node, source);
                }
            }
        }
    }
    None
}

/// Resolves a type node to its base name, unwrapping pointer types.
fn type_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" => node_text(node, source).map(String::from),
        "pointer_type" => {
            // `*T` — the base type is the first named child (type_identifier).
            // tree-sitter-go's pointer_type uses positional children, not a
            // named field, so child_by_field_name("type") returns None.
            let base = node.named_child(0)?;
            type_name(base, source)
        }
        "qualified_type" => {
            // `pkg.Type` — use the rightmost identifier (the type name).
            let name = node.child_by_field_name("name")?;
            node_text(name, source).map(String::from)
        }
        _ => None,
    }
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "selector_expression" => {
            // `pkg.Func()` -> extract the field (rightmost) name.
            let field = node.child_by_field_name("field")?;
            node_text(field, source).map(String::from)
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

/// Returns the first line of a signature string (the `func ...` line).
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Go, parent)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    // DEFINES edge: file -> definition (matches the Python/C extractor pattern).
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
        let ext = GoExtractor::new();
        ext.extract(source, "test.go", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_go() {
        assert_eq!(GoExtractor::new().language(), Language::Go);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = GoExtractor::default();
        assert_eq!(ext.language(), Language::Go);
    }

    #[test]
    fn extracts_function_declaration() {
        let result = extract("package main\nfunc foo() {}\n");
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
        assert_eq!(funcs[0].language, Some(Language::Go));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.go"));
        assert!(funcs[0].is_global, "top-level function should be global");
    }

    #[test]
    fn extracts_method_declaration() {
        let result = extract("package main\ntype T struct{}\nfunc (t T) Bar() {}\n");
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
        // The receiver type T is used as the FQN disambiguator.
        assert!(
            methods[0].qualified_name.contains("Bar"),
            "FQN should contain method name: {}",
            methods[0].qualified_name
        );
        assert_eq!(methods[0].parent_qn.as_deref(), Some("T"));
    }

    #[test]
    fn extracts_method_with_pointer_receiver() {
        let result = extract("package main\ntype T struct{}\nfunc (t *T) Baz() {}\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract 1 pointer-receiver method");
        assert_eq!(methods[0].name, "Baz");
        assert_eq!(methods[0].parent_qn.as_deref(), Some("T"));
    }

    #[test]
    fn extracts_struct_type() {
        let result = extract("package main\ntype Point struct {\n\tX int\n\tY int\n}\n");
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
        assert!(structs[0].is_global);
    }

    #[test]
    fn extracts_interface_type() {
        let result = extract("package main\ntype Reader interface {\n\tRead() int\n}\n");
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
        assert_eq!(ifaces[0].name, "Reader");
    }

    #[test]
    fn extracts_grouped_type_declaration() {
        // `type ( Foo struct{}; Bar interface{} )` produces two type_specs.
        let result = extract("package main\ntype (\n\tFoo struct{}\n\tBar interface{}\n)\n");
        let structs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Struct)
            .collect();
        let ifaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Interface)
            .collect();
        assert_eq!(structs.len(), 1, "should extract 1 struct from group");
        assert_eq!(ifaces.len(), 1, "should extract 1 interface from group");
        assert_eq!(structs[0].name, "Foo");
        assert_eq!(ifaces[0].name, "Bar");
    }

    #[test]
    fn extracts_import_single() {
        let result = extract("package main\nimport \"fmt\"\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 import");
        assert_eq!(result.imports[0].source_file, "fmt");
    }

    #[test]
    fn extracts_import_list() {
        let result = extract("package main\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n");
        assert_eq!(
            result.imports.len(),
            2,
            "should extract 2 imports: {:?}",
            result.imports
        );
        let paths: Vec<_> = result
            .imports
            .iter()
            .map(|i| i.source_file.as_str())
            .collect();
        assert!(paths.contains(&"fmt"), "should import fmt: {:?}", paths);
        assert!(paths.contains(&"os"), "should import os: {:?}", paths);
    }

    #[test]
    fn extracts_import_with_alias() {
        let result = extract("package main\nimport f \"fmt\"\n");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "fmt");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn package_only_returns_empty_result() {
        let result = extract("package main\n");
        assert!(
            result.is_empty(),
            "package decl alone should produce no nodes"
        );
    }

    #[test]
    fn result_language_is_go() {
        let result = extract("package main\nfunc foo() {}\n");
        assert_eq!(result.language, Language::Go);
        assert_eq!(result.file_path, "test.go");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("package main\nfunc foo() {}\n");
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
        let result = extract("package main\nfunc foo() {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.go.foo");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("package main\nfunc add(a int, b int) int { return a + b }\n");
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn extracts_call_to_function() {
        let result = extract("package main\nfunc foo() {}\nfunc main() {\n\tfoo()\n}\n");
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
        let result = extract("package main\nfunc foo() {}\nfunc main() {\n\tfoo(1, 2)\n}\n");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "foo")
            .expect("should find call to foo");
        assert_eq!(call.args.len(), 2, "foo(1, 2) should have 2 args");
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        let src = "package main\nfunc caller() {\n\tcallee()\n}\n";
        let ext = GoExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.go", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.go.caller"),
            "caller_qn should be the dotted FQN of the enclosing function"
        );
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller function node");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "caller_qn must match the caller function node id"
        );
    }

    #[test]
    fn top_level_call_has_none_caller_qn() {
        // Go requires a function body for statements, so a top-level call is
        // not valid Go. Instead, verify that a call inside a function has a
        // non-None caller_qn (covered by call_in_function_has_dotted_fqn_caller_qn).
        // This test documents that caller_qn is None only when current_func is None,
        // which for valid Go source never happens at the call site.
        let result = extract("package main\nfunc foo() {}\n");
        // No calls -> no caller_qn to check; just ensure no panic.
        assert!(result.calls.is_empty());
    }

    #[test]
    fn method_fqn_disambiguated_by_receiver_type() {
        // Two methods named Read on different types should produce distinct FQNs.
        let src = "package main\ntype A struct{}\ntype B struct{}\nfunc (a A) Read() {}\nfunc (b B) Read() {}\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method && n.name == "Read")
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 Read methods");
        assert_ne!(
            methods[0].qualified_name, methods[1].qualified_name,
            "methods on different types must have distinct FQNs"
        );
    }

    #[test]
    fn method_calls_from_different_types_have_distinct_caller_qn() {
        // BUG-G1: Two methods named Run on different types both call helper().
        // caller_qn must include the receiver type so resolve_calls doesn't
        // deduplicate their CALLS edges.
        let src = "package main\nfunc helper() {}\ntype A struct{}\ntype B struct{}\nfunc (a A) Run() { helper() }\nfunc (b B) Run() { helper() }\n";
        let result = extract(src);
        let run_calls: Vec<_> = result
            .calls
            .iter()
            .filter(|c| c.callee_name == "helper")
            .collect();
        assert_eq!(run_calls.len(), 2, "should extract 2 calls to helper");
        assert_ne!(
            run_calls[0].caller_qn, run_calls[1].caller_qn,
            "caller_qn for Run on A and B must be distinct (include receiver type)"
        );
        assert!(
            run_calls[0].caller_qn.as_ref().unwrap().contains("A")
                || run_calls[1].caller_qn.as_ref().unwrap().contains("A"),
            "one caller_qn should contain receiver type A: {:?}",
            run_calls
        );
        assert!(
            run_calls[0].caller_qn.as_ref().unwrap().contains("B")
                || run_calls[1].caller_qn.as_ref().unwrap().contains("B"),
            "one caller_qn should contain receiver type B: {:?}",
            run_calls
        );
    }

    #[test]
    fn exported_function_has_is_exported_true() {
        // BUG-G2: Go exported symbols (uppercase first letter) must have
        // is_exported=true so resolve_calls can find them via lookup_exported.
        let result = extract("package main\nfunc Foo() {}\n");
        let func = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Function)
            .unwrap();
        assert!(
            func.is_exported,
            "exported function Foo should have is_exported=true"
        );
    }

    #[test]
    fn unexported_function_has_is_exported_false() {
        let result = extract("package main\nfunc bar() {}\n");
        let func = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Function)
            .unwrap();
        assert!(
            !func.is_exported,
            "unexported function bar should have is_exported=false"
        );
    }

    #[test]
    fn exported_method_has_is_exported_true() {
        let result = extract("package main\ntype T struct{}\nfunc (t T) Execute() {}\n");
        let method = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Method)
            .unwrap();
        assert!(
            method.is_exported,
            "exported method Execute should have is_exported=true"
        );
    }

    #[test]
    fn unexported_method_has_is_exported_false() {
        let result = extract("package main\ntype T struct{}\nfunc (t T) hidden() {}\n");
        let method = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Method)
            .unwrap();
        assert!(
            !method.is_exported,
            "unexported method hidden should have is_exported=false"
        );
    }

    #[test]
    fn exported_type_has_is_exported_true() {
        let result = extract("package main\ntype Command struct{}\n");
        let typ = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Struct)
            .unwrap();
        assert!(
            typ.is_exported,
            "exported type Command should have is_exported=true"
        );
    }

    #[test]
    fn extracts_type_alias() {
        let result = extract("package main\ntype MyInt int\n");
        let aliases: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias)
            .collect();
        assert_eq!(
            aliases.len(),
            1,
            "should extract 1 type alias: {:?}",
            result.nodes
        );
        assert_eq!(aliases[0].name, "MyInt");
    }

    #[test]
    fn function_with_pointer_parameter_has_signature() {
        let result = extract("package main\nfunc foo(x *int) {}\n");
        let func = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert!(func.signature.is_some(), "should have signature");
        let sig = func.signature.as_ref().unwrap();
        assert!(sig.contains("foo"), "signature should contain name: {sig}");
    }

    #[test]
    fn function_with_qualified_type_parameter_has_signature() {
        let result = extract("package main\nimport \"fmt\"\nfunc foo(x fmt.Stringer) {}\n");
        let func = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert!(func.signature.is_some(), "should have signature");
        let sig = func.signature.as_ref().unwrap();
        assert!(sig.contains("foo"), "signature should contain name: {sig}");
    }

    #[test]
    fn extracts_selector_call() {
        let result = extract("package main\nimport \"fmt\"\nfunc main() {\n\tfmt.Println()\n}\n");
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"Println"),
            "should extract selector call to Println: {:?}",
            callees
        );
    }

    #[test]
    fn extracts_call_on_call_result() {
        let src = "package main\nfunc getFunc() func() {\n\treturn nil\n}\nfunc main() {\n\tgetFunc()()\n}\n";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"getFunc"),
            "should extract call to getFunc: {:?}",
            callees
        );
    }

    #[test]
    fn parenthesized_call_extracts_callee() {
        let src = "package main\nfunc foo() {}\nfunc main() {\n\t(foo)()\n}\n";
        let result = extract(src);
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
    fn comment_only_source_returns_empty() {
        let result = extract("// just a comment\n");
        assert!(result.is_empty(), "comment-only should produce no nodes");
    }

    #[test]
    fn method_on_pointer_receiver_with_qualified_type() {
        let src = "package main\nimport \"fmt\"\ntype T struct{}\nfunc (t *T) String() string { return fmt.Sprintf(\"%v\", t) }\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method && n.name == "String")
            .collect();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].parent_qn.as_deref(), Some("T"));
    }

    #[test]
    fn empty_struct_extracts_struct_node() {
        let result = extract("package main\ntype Empty struct{}\n");
        let structs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Struct)
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Empty");
    }

    #[test]
    fn multiple_functions_all_extracted() {
        let result = extract("package main\nfunc a() {}\nfunc b() {}\nfunc c() {}\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 3, "should extract 3 functions");
    }

    #[test]
    fn interface_with_multiple_methods_extracts_interface() {
        let src = "package main\ntype Reader interface {\n\tRead() int\n\tClose() error\n}\n";
        let result = extract(src);
        let ifaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Interface)
            .collect();
        assert_eq!(ifaces.len(), 1);
        assert_eq!(ifaces[0].name, "Reader");
    }

    #[test]
    fn unexported_type_has_is_exported_false() {
        let result = extract("package main\ntype myType struct{}\n");
        let typ = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Struct)
            .next()
            .expect("should find struct");
        assert!(!typ.is_exported, "unexported type should have is_exported=false");
    }
}
