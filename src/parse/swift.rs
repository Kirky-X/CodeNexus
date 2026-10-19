// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Swift language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-swift's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_declaration` / `protocol_function_declaration` →
//!   [`NodeLabel::Function`] at top level, [`NodeLabel::Method`] inside a
//!   type, protocol, or extension.
//! - `init_declaration` → [`NodeLabel::Constructor`].
//! - `deinit_declaration` → [`NodeLabel::Method`].
//! - `class_declaration` (discriminated by its `declaration_kind` field):
//!   - `class` / `actor` → [`NodeLabel::Class`]
//!   - `struct` → [`NodeLabel::Struct`]
//!   - `enum` → [`NodeLabel::Enum`]
//!   - `extension` → [`NodeLabel::Impl`] (the extended type becomes the FQN
//!     disambiguator for its members)
//! - `protocol_declaration` → [`NodeLabel::Interface`].
//! - `typealias_declaration` / `associatedtype_declaration` →
//!   [`NodeLabel::TypeAlias`].
//! - `property_declaration` / `protocol_property_declaration` →
//!   [`NodeLabel::Property`] inside a type, [`NodeLabel::GlobalVar`] at top
//!   level. Local variables inside function bodies are intentionally skipped
//!   to keep the graph proportional to the public API surface.
//!
//! # Extracted records
//!
//! - `import_declaration` → [`ImportInfo`] (module name, first `.` segment).
//! - `call_expression` / `constructor_expression` → [`CallInfo`].
//!
//! # Known limitations
//!
//! - Inheritance/conformance edges (`extends` / `implements`) are not
//!   extracted yet; the MRO strategy is wired but has no Swift edges to walk.
//! - Assignments, reads/writes, enum cases, subscripts, operators, macros, and
//!   computed-property getter bodies are out of scope for this minimal
//!   extractor.
//! - Calls in top-level code and property initializers are not attributed to a
//!   caller (matching the other language extractors).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::helpers::node_text;

/// Swift language tree-sitter extractor (Adapter pattern).
pub struct SwiftExtractor {
    _priv: (),
}

impl SwiftExtractor {
    /// Creates a new `SwiftExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for SwiftExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for SwiftExtractor {
    fn language(&self) -> Language {
        Language::Swift
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Swift);
        // Use the thread-local parser pool (ADR-010) instead of creating a
        // fresh parser per file.
        let tree = super::with_pooled_parser(Language::Swift, |parser| parser.parse(source, None))?
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

/// Immutable traversal context passed between the visit/extract functions.
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
    /// The innermost function/method name, used for `CallInfo::caller_qn`.
    current_func: Option<&'a str>,
    /// The enclosing type/protocol/extension name, used as the FQN
    /// disambiguator and as the method/property `parent_qn`.
    current_parent: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function_declaration" | "protocol_function_declaration" => {
            extract_function(node, source, ctx, result);
        }
        "init_declaration" => {
            extract_init(node, source, ctx, result);
        }
        "deinit_declaration" => {
            extract_deinit(node, source, ctx, result);
        }
        "class_declaration" => {
            extract_class_like(node, source, ctx, result);
        }
        "protocol_declaration" => {
            extract_protocol(node, source, ctx, result);
        }
        "typealias_declaration" | "associatedtype_declaration" => {
            extract_typealias(node, source, ctx, result);
        }
        "property_declaration" | "protocol_property_declaration" => {
            extract_property(node, source, ctx, result);
            // Inside a function, recurse into the initializer so a local
            // `let x = Foo()` still produces a CallInfo for the enclosing
            // function. At type/top level, skip recursion: initializer
            // closures and computed-property getters would otherwise emit
            // local `let` bindings as Property/GlobalVar nodes.
            if ctx.current_func.is_some() {
                visit_children(node, source, ctx, result);
            }
        }
        "import_declaration" => {
            extract_import(node, source, result);
        }
        "call_expression" => {
            extract_call(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "constructor_expression" => {
            extract_constructor_call(node, source, ctx, result);
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
    let Some(name) = declaration_name(node, source) else {
        return;
    };
    let label = if ctx.current_parent.is_some() {
        NodeLabel::Method
    } else {
        NodeLabel::Function
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(label, name.clone(), qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Swift)
        .project(ctx.project)
        .is_global(ctx.current_parent.is_none())
        .is_exported(is_exported(node, source));
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    if let Some(sig) = signature(node, source) {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);

    let inner_ctx = VisitContext {
        file_path: ctx.file_path,
        project: ctx.project,
        current_func: Some(name.as_str()),
        current_parent: ctx.current_parent,
    };
    visit_children(node, source, &inner_ctx, result);
}

fn extract_init(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    // Name constructors after their type (like Java/C#), so `Foo()` call
    // resolution can match the Constructor node. Multiple initializers in the
    // same type are disambiguated by `dedupe_qn`'s `#L{line}` suffix.
    let name = ctx
        .current_parent
        .map(str::to_string)
        .or_else(|| declaration_name(node, source))
        .unwrap_or_else(|| "init".to_string());
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(NodeLabel::Constructor, name.clone(), qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Swift)
        .project(ctx.project)
        .is_global(ctx.current_parent.is_none())
        .is_exported(is_exported(node, source));
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    if let Some(sig) = signature(node, source) {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);

    let inner_ctx = VisitContext {
        file_path: ctx.file_path,
        project: ctx.project,
        current_func: Some(name.as_str()),
        current_parent: ctx.current_parent,
    };
    visit_children(node, source, &inner_ctx, result);
}

fn extract_deinit(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let name = "deinit".to_string();
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(NodeLabel::Method, name.clone(), qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Swift)
        .project(ctx.project)
        .is_global(false)
        .is_exported(is_exported(node, source));
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);

    let inner_ctx = VisitContext {
        file_path: ctx.file_path,
        project: ctx.project,
        current_func: Some(name.as_str()),
        current_parent: ctx.current_parent,
    };
    visit_children(node, source, &inner_ctx, result);
}

fn extract_class_like(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name) = type_decl_name(node, source) else {
        return;
    };
    let declaration_kind = node
        .child_by_field_name("declaration_kind")
        .map(|n| n.kind())
        .unwrap_or("class");
    let (label, disambiguator, is_extension) = match declaration_kind {
        "struct" => (NodeLabel::Struct, ctx.current_parent, false),
        "enum" => (NodeLabel::Enum, ctx.current_parent, false),
        "extension" => (NodeLabel::Impl, Some("extension"), true),
        // `class`, `actor`, and unknown declaration kinds are modelled as
        // Class. tree-sitter-swift currently emits `class`/`actor` here.
        _ => (NodeLabel::Class, ctx.current_parent, false),
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, disambiguator),
        node.start_position().row as u32 + 1,
        result,
    );
    // Extensions are not nominal types: suffix the display name so a
    // constructor call such as `Array(...)` cannot resolve to the extension
    // block itself (the FQN still keeps the plain type name + `#extension`).
    let display_name = if is_extension {
        format!("{name}.extension")
    } else {
        name.clone()
    };
    let mut builder = ModelNode::builder(label, display_name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Swift)
        .project(ctx.project)
        .is_global(true)
        .is_exported(is_exported(node, source));
    if let Some(sig) = signature(node, source) {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);

    // Members of an extension belong to the extended type, so the extension
    // name becomes the parent context for nested declarations.
    let inner_ctx = VisitContext {
        file_path: ctx.file_path,
        project: ctx.project,
        current_func: None,
        current_parent: Some(name.as_str()),
    };
    visit_children(node, source, &inner_ctx, result);
}

fn extract_protocol(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = type_decl_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(NodeLabel::Interface, name.clone(), qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Swift)
        .project(ctx.project)
        .is_global(true)
        .is_exported(is_exported(node, source));
    if let Some(sig) = signature(node, source) {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);

    let inner_ctx = VisitContext {
        file_path: ctx.file_path,
        project: ctx.project,
        current_func: None,
        current_parent: Some(name.as_str()),
    };
    visit_children(node, source, &inner_ctx, result);
}

fn extract_typealias(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = declaration_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(NodeLabel::TypeAlias, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Swift)
        .project(ctx.project)
        .is_global(ctx.current_parent.is_none())
        .is_exported(is_exported(node, source));
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    if let Some(sig) = signature(node, source) {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_property(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    // Local variables inside function bodies are intentionally skipped: on
    // real-world projects they would dominate the graph without adding
    // call-graph information.
    if ctx.current_func.is_some() {
        return;
    }
    let Some(name) = property_name(node, source) else {
        return;
    };
    let (label, is_global) = if ctx.current_parent.is_some() {
        (NodeLabel::Property, false)
    } else {
        (NodeLabel::GlobalVar, true)
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(label, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Swift)
        .project(ctx.project)
        .is_global(is_global)
        .is_exported(is_exported(node, source));
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    if let Some(sig) = signature(node, source) {
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
    let line = node.start_position().row as u32 + 1;
    let mut module: Option<String> = None;
    for i in 0..node.named_child_count() as u32 {
        let Some(child) = node.named_child(i) else {
            continue;
        };
        // `@testable`, `@_exported`, etc. are modifiers, not the module name.
        if child.kind() == "modifiers" {
            continue;
        }
        if let Some(text) = node_text(child, source) {
            let first = text.split('.').next().unwrap_or(text).trim();
            if !first.is_empty() {
                module = Some(first.to_string());
                break;
            }
        }
    }
    if let Some(source_file) = module {
        result.imports.push(ImportInfo {
            source_file,
            imported_names: Vec::new(),
            line,
            is_reexport: false,
        });
    }
}

fn extract_call(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(caller) = ctx.current_func else {
        return;
    };
    let Some(callee_node) = node.named_child(0) else {
        return;
    };
    let Some(callee) = last_identifier_text(callee_node, source) else {
        return;
    };
    let args = call_arguments(node, source);
    result.calls.push(CallInfo {
        caller_qn: Some(make_qn(
            ctx.file_path,
            caller,
            ctx.project,
            ctx.current_parent,
        )),
        callee_name: callee,
        line: node.start_position().row as u32 + 1,
        args,
    });
}

fn extract_constructor_call(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(caller) = ctx.current_func else {
        return;
    };
    let Some(callee) = constructor_name(node, source) else {
        return;
    };
    let args = call_arguments(node, source);
    result.calls.push(CallInfo {
        caller_qn: Some(make_qn(
            ctx.file_path,
            caller,
            ctx.project,
            ctx.current_parent,
        )),
        callee_name: callee,
        line: node.start_position().row as u32 + 1,
        args,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the declaration's `name` field text.
fn declaration_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    first_identifier_text(name_node, source)
        .or_else(|| node_text(name_node, source).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
}

/// Returns the type name for a class/struct/enum/protocol/extension.
///
/// Extensions may target generic or nested types (`Array<Int>`, `Foo.Bar`);
/// the FQN only needs the leading nominal segment, so everything from the
/// first `<` is dropped.
fn type_decl_name(node: Node, source: &str) -> Option<String> {
    let raw = declaration_name(node, source)?;
    Some(raw.split('<').next().unwrap_or(&raw).trim().to_string())
}

/// Returns the first identifier-like descendant text.
fn first_identifier_text(node: Node, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "simple_identifier" | "type_identifier" | "identifier"
    ) {
        return node_text(node, source).map(str::to_string);
    }
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if let Some(text) = first_identifier_text(child, source) {
                return Some(text);
            }
        }
    }
    None
}

/// Returns the last identifier-like descendant text.
///
/// Used for call callees: `foo()` → `foo`, `foo.bar()` → `bar`.
fn last_identifier_text(node: Node, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "simple_identifier" | "type_identifier" | "identifier"
    ) {
        return node_text(node, source).map(str::to_string);
    }
    let mut last = None;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if let Some(text) = last_identifier_text(child, source) {
                last = Some(text);
            }
        }
    }
    last
}

/// Returns the constructed type name for `Foo()` expressions.
fn constructor_name(node: Node, source: &str) -> Option<String> {
    let type_node = node
        .child_by_field_name("constructed_type")
        .or_else(|| node.named_child(0))?;
    first_identifier_text(type_node, source).or_else(|| {
        node_text(type_node, source)
            .map(|text| text.split('<').next().unwrap_or(text).trim().to_string())
    })
}

/// Returns the declaration name from a `pattern` field (properties).
fn property_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    first_identifier_text(name_node, source)
        .or_else(|| node_text(name_node, source).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
}

/// Returns the first line of a declaration's source text.
fn signature(node: Node, source: &str) -> Option<String> {
    node_text(node, source).map(|text| text.lines().next().unwrap_or(text).to_string())
}

/// Returns true unless the declaration carries a `private`/`fileprivate`
/// modifier. Swift's default `internal` access is module-wide, so treating
/// unmodified declarations as exported keeps cross-file call resolution
/// useful.
fn is_exported(node: Node, source: &str) -> bool {
    for i in 0..node.named_child_count() as u32 {
        let Some(child) = node.named_child(i) else {
            continue;
        };
        if child.kind() == "modifiers" {
            return node_text(child, source).is_none_or(|text| !text.contains("private"));
        }
    }
    true
}

/// Finds a direct named child whose kind is in `kinds`.
fn find_child_by_kind<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if kinds.contains(&child.kind()) {
                return Some(child);
            }
        }
    }
    None
}

/// Extracts call arguments from `call_suffix`/`constructor_suffix` →
/// `value_arguments` → `value_argument`.
fn call_arguments(node: Node, source: &str) -> Vec<String> {
    let Some(suffix) = find_child_by_kind(node, &["call_suffix", "constructor_suffix"]) else {
        return Vec::new();
    };
    let Some(args) = find_child_by_kind(suffix, &["value_arguments"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..args.named_child_count() as u32 {
        if let Some(arg) = args.named_child(i) {
            if arg.kind() == "value_argument" {
                if let Some(text) = node_text(arg, source) {
                    out.push(text.to_string());
                }
            }
        }
    }
    out
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Swift, parent)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    // DEFINES edge: file -> definition (matches the other extractors).
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

    fn extract(source: &str) -> ExtractResult {
        SwiftExtractor::new()
            .extract(source, "test.swift", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_swift() {
        assert_eq!(SwiftExtractor::new().language(), Language::Swift);
    }

    #[test]
    fn default_creates_extractor() {
        assert_eq!(SwiftExtractor::default().language(), Language::Swift);
    }

    #[test]
    fn extracts_top_level_function() {
        let result = extract("func greet() {}\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(funcs[0].name, "greet");
        assert_eq!(funcs[0].language, Some(Language::Swift));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.swift"));
        assert!(funcs[0].is_global);
        assert!(funcs[0]
            .signature
            .as_deref()
            .unwrap_or("")
            .contains("func greet"));
    }

    #[test]
    fn extracts_class_struct_enum_actor() {
        let result = extract("class A {}\nstruct B {}\nenum C {}\nactor D {}\n");
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.label == NodeLabel::Class && n.name == "A"),
            "nodes: {:?}",
            result.nodes
        );
        assert!(result
            .nodes
            .iter()
            .any(|n| n.label == NodeLabel::Struct && n.name == "B"));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.label == NodeLabel::Enum && n.name == "C"));
        assert!(result
            .nodes
            .iter()
            .any(|n| n.label == NodeLabel::Class && n.name == "D"));
    }

    #[test]
    fn extracts_protocol_as_interface() {
        let result = extract("protocol Greeter {\n    func greet() -> String\n}\n");
        let interfaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Interface)
            .collect();
        assert_eq!(interfaces.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(interfaces[0].name, "Greeter");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(methods[0].name, "greet");
        assert_eq!(methods[0].parent_qn.as_deref(), Some("Greeter"));
    }

    #[test]
    fn extracts_method_with_parent() {
        let result = extract("class A {\n    func m() {}\n}\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(methods[0].name, "m");
        assert_eq!(methods[0].parent_qn.as_deref(), Some("A"));
        assert!(!methods[0].is_global);
        assert!(methods[0].qualified_name.contains("#A"));
    }

    #[test]
    fn extracts_extension_method_under_type() {
        let result = extract("struct A {}\nextension A {\n    func m() {}\n}\n");
        let impls: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Impl)
            .collect();
        assert_eq!(impls.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(impls[0].name, "A.extension");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(methods[0].parent_qn.as_deref(), Some("A"));
    }

    #[test]
    fn extracts_import() {
        let result = extract("import Foundation\n@testable import MyModule\n");
        assert_eq!(result.imports.len(), 2, "imports: {:?}", result.imports);
        let modules: Vec<_> = result
            .imports
            .iter()
            .map(|i| i.source_file.as_str())
            .collect();
        assert!(modules.contains(&"Foundation"));
        assert!(modules.contains(&"MyModule"));
    }

    #[test]
    fn extracts_call() {
        let result = extract("func a() {\n    b()\n}\n");
        assert_eq!(result.calls.len(), 1, "calls: {:?}", result.calls);
        assert_eq!(result.calls[0].callee_name, "b");
        assert!(result.calls[0]
            .caller_qn
            .as_deref()
            .unwrap_or("")
            .contains("a"));
    }

    #[test]
    fn extracts_constructor_and_call() {
        let result = extract("class A {\n    init() {}\n}\nfunc f() {\n    let a = A()\n}\n");
        let ctors: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Constructor)
            .collect();
        assert_eq!(ctors.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(ctors[0].name, "A");
        assert!(
            result.calls.iter().any(|c| c.callee_name == "A"),
            "calls: {:?}",
            result.calls
        );
    }

    #[test]
    fn extracts_property_and_skips_locals() {
        let result =
            extract("struct A {\n    let x: Int\n}\nlet y = 1\nfunc f() {\n    let z = 1\n}\n");
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.label == NodeLabel::Property && n.name == "x"),
            "nodes: {:?}",
            result.nodes
        );
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.label == NodeLabel::GlobalVar && n.name == "y"),
            "nodes: {:?}",
            result.nodes
        );
        assert!(
            !result.nodes.iter().any(|n| n.name == "z"),
            "local variable z should be skipped: {:?}",
            result.nodes
        );
    }

    #[test]
    fn extracts_typealias() {
        let result = extract("typealias Handler = () -> Void\n");
        let aliases: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias)
            .collect();
        assert_eq!(aliases.len(), 1, "nodes: {:?}", result.nodes);
        assert_eq!(aliases[0].name, "Handler");
    }

    #[test]
    fn private_modifier_marks_not_exported() {
        let result = extract("private func hidden() {}\nfunc visible() {}\n");
        let hidden = result
            .nodes
            .iter()
            .find(|n| n.name == "hidden")
            .expect("hidden function");
        let visible = result
            .nodes
            .iter()
            .find(|n| n.name == "visible")
            .expect("visible function");
        assert!(!hidden.is_exported);
        assert!(visible.is_exported);
    }

    #[test]
    fn malformed_source_does_not_panic() {
        let result = extract("func {\nclass {\n");
        // The tree is malformed, but extraction must still return Ok.
        assert!(result.nodes.len() < 10);
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.nodes.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.calls.is_empty());
    }

    #[test]
    fn call_arguments_and_callee_variants() {
        let result = extract("func caller() {\n    foo.bar(x: 1, y: 2)\n}\n");
        assert_eq!(result.calls.len(), 1, "calls: {:?}", result.calls);
        assert_eq!(result.calls[0].callee_name, "bar");
        assert_eq!(
            result.calls[0].args.len(),
            2,
            "args: {:?}",
            result.calls[0].args
        );
        assert!(result.calls[0].args[0].contains('x'));
    }
}
