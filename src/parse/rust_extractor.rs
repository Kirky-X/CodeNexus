//! Rust language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-rust's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_item` → [`NodeLabel::Function`]
//! - `struct_item` → [`NodeLabel::Struct`]
//! - `enum_item` → [`NodeLabel::Enum`]
//! - `trait_item` → [`NodeLabel::Trait`]
//! - `impl_item` → [`NodeLabel::Impl`]
//! - `const_item` → [`NodeLabel::Const`]
//! - `static_item` → [`NodeLabel::Static`]
//! - `type_item` → [`NodeLabel::TypeAlias`]
//! - `macro_definition` → [`NodeLabel::Macro`]
//! - `mod_item` → [`NodeLabel::Module`] (P2-1: `mod foo;` / `mod foo {}`)
//!
//! # Extracted records
//!
//! - `use_declaration` → [`ImportInfo`]
//! - `call_expression` → [`CallInfo`]
//! - `let_declaration` → [`AssignInfo`]
//! - `extern_item` / `extern_block` → [`ExternInfo`]
//! - identifier in expression position → [`ReadInfo`] (BR-TRACE-005)
//! - `let_declaration` pattern / `assignment_expression` left → [`WriteInfo`]
//!   (BR-TRACE-006)

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{
    AssignInfo, CallInfo, ExternInfo, ExtractResult, Extractor, ImportInfo, ReadInfo, WriteInfo,
};
use super::parser_factory::ParserFactory;

/// Rust language tree-sitter extractor (Adapter pattern).
pub struct RustExtractor {
    _priv: (),
}

impl RustExtractor {
    /// Creates a new `RustExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for RustExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for RustExtractor {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Rust);
        let mut parser = ParserFactory::create_parser(Language::Rust)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        // source_file is the root for Rust.
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
/// 封装 ADR-005 的 current_parent 和 current_func 语义。
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
    current_func: Option<&'a str>,
    current_parent: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function_item" => {
            extract_function(node, source, ctx, result);
            // Pass the function's name as the enclosing function for body
            // traversal, so reads/writes can be attributed to it.
            let func_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: func_name.as_deref(),
                current_parent: ctx.current_parent,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "struct_item" => {
            extract_named_item(node, NodeLabel::Struct, source, ctx, result);
        }
        "enum_item" => {
            extract_named_item(node, NodeLabel::Enum, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "trait_item" => {
            extract_named_item(node, NodeLabel::Trait, source, ctx, result);
            let trait_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: trait_name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "impl_item" => {
            let impl_type = node
                .child_by_field_name("type")
                .and_then(|n| node_text(n, source).map(String::from));
            extract_impl(node, source, ctx, ctx.current_parent, result);
            // Combine module context with impl type so methods inside the impl
            // get disambiguated (ADR-003).
            let combined = match (ctx.current_parent, impl_type.as_deref()) {
                (Some(p), Some(t)) => Some(format!("{p}_{t}")),
                (None, Some(t)) => Some(t.to_string()),
                (Some(p), None) => Some(p.to_string()),
                (None, None) => None,
            };
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: combined.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "const_item" => {
            extract_named_item(node, NodeLabel::Const, source, ctx, result);
        }
        "static_item" => {
            extract_named_item(node, NodeLabel::Static, source, ctx, result);
        }
        "type_item" => {
            extract_named_item(node, NodeLabel::TypeAlias, source, ctx, result);
        }
        "macro_definition" => {
            extract_named_item(node, NodeLabel::Macro, source, ctx, result);
        }
        "use_declaration" => {
            extract_use(node, source, result);
        }
        "call_expression" => {
            extract_call(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "let_declaration" => {
            extract_let(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "assignment_expression" => {
            extract_assignment(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "identifier" => {
            // A bare identifier in an expression position is a variable read
            // (BR-TRACE-005). Name-defining positions (patterns, call
            // functions, field names) are excluded by `is_read_position`.
            if let Some(func) = ctx.current_func {
                if is_read_position(node) {
                    if let Some(name) = node_text(node, source).map(String::from) {
                        result.reads.push(ReadInfo {
                            reader_qn: Some(func.to_string()),
                            var_name: name,
                            line: node.start_position().row as u32 + 1,
                        });
                    }
                }
            }
        }
        "extern_item" | "extern_block" | "foreign_mod_item" => {
            extract_extern_block(node, source, result);
            visit_children(node, source, ctx, result);
        }
        "mod_item" => {
            // `mod name { ... }` 块：把模块名纳入 current_parent，使不同模块下
            // 的同名 impl 生成不同 FQN（修复 P0-1 rust-nested-tail-collision）。
            // P2-1: 同时创建 Module 节点（`mod foo;` 和 `mod foo {}` 都创建），
            // 之前只更新 current_parent 导致 Module 节点完全丢失（0 vs gitnexus 24）。
            extract_named_item(node, NodeLabel::Module, source, ctx, result);
            let mod_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            let combined = combine_scope(ctx.current_parent, mod_name.as_deref());
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: None,
                current_parent: combined.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
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
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let is_exported = is_pub(node);
    let signature = node_text(node, source).map(String::from);
    let qn = make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent);
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Rust)
        .project(ctx.project)
        .is_exported(is_exported)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_named_item(
    node: Node,
    label: NodeLabel,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let is_exported = is_pub(node);
    let qn = make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent);
    let model_node = ModelNode::builder(label, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Rust)
        .project(ctx.project)
        .is_exported(is_exported)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_impl(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    module_parent: Option<&str>,
    result: &mut ExtractResult,
) {
    // impl_item has a `type` field (the type being implemented) and an
    // optional `trait` field (the trait being implemented).
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Some(name) = node_text(type_node, source).map(String::from) else {
        return;
    };
    let trait_name = node
        .child_by_field_name("trait")
        .and_then(|n| node_text(n, source).map(String::from));
    // Impl blocks need disambiguation from struct/enum with the same name
    // (ADR-003). Use the trait name when present, otherwise the literal
    // "impl" marker, combined with the module parent context.
    let im = trait_name.as_deref().unwrap_or("impl");
    let disambiguator = match module_parent {
        Some(m) => format!("{m}_{im}"),
        None => im.to_string(),
    };
    let qn = make_qn(ctx.file_path, &name, ctx.project, Some(&disambiguator));
    let mut builder = ModelNode::builder(NodeLabel::Impl, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Rust)
        .project(ctx.project)
        .is_global(true);
    if let Some(trait_name) = trait_name {
        builder = builder.properties(serde_json::json!({"trait": trait_name}));
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_use(node: Node, source: &str, result: &mut ExtractResult) {
    // use_declaration has an `argument` field which is a use_clause.
    // The use_clause can be:
    //   - identifier (e.g. `use foo;`)
    //   - scoped_use_list (e.g. `use std::io;`)
    //   - use_as_clause (e.g. `use foo as bar;`)
    //   - use_wildcard (e.g. `use std::*;`)
    let Some(arg) = node.child_by_field_name("argument") else {
        return;
    };
    let path = use_path(arg, source).unwrap_or_default();
    let names = use_imported_names(arg, source);
    result.imports.push(ImportInfo {
        source_file: path,
        imported_names: names,
        line: node.start_position().row as u32 + 1,
    });
}

fn extract_call(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
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

fn extract_let(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    // let_declaration has a `pattern` field and an optional `value` field.
    let Some(pattern_node) = node.child_by_field_name("pattern") else {
        return;
    };
    let Some(target) = pattern_name(pattern_node, source) else {
        return;
    };
    let value_node = node.child_by_field_name("value");
    let (source_name, is_return_assign) = match value_node {
        Some(v) => {
            // If the value is a call_expression, this is a return assignment.
            let is_call = v.kind() == "call_expression";
            let name = if is_call {
                v.child_by_field_name("function")
                    .and_then(|f| callee_name(f, source))
                    .unwrap_or_default()
            } else {
                // Only capture simple identifier values for data flow
                // tracking. Complex expressions (match, if, block, etc.)
                // produce multi-line text that would corrupt CSV output.
                if v.kind() == "identifier" {
                    node_text(v, source).map(String::from).unwrap_or_default()
                } else {
                    String::new()
                }
            };
            (name, is_call)
        }
        None => (String::new(), false),
    };
    result.assignments.push(AssignInfo {
        target_name: target.clone(),
        source_name,
        line: node.start_position().row as u32 + 1,
        is_return_assign,
    });
    // A let binding also writes the bound variable (BR-TRACE-006). Only
    // attribute the write when inside a function body.
    if let Some(func) = ctx.current_func {
        result.writes.push(WriteInfo {
            writer_qn: Some(func.to_string()),
            var_name: target,
            line: node.start_position().row as u32 + 1,
        });
    }
}

/// Extracts a `WriteInfo` from the left-hand side of an `assignment_expression`
/// (e.g. `x = ...`), attributing the write to `current_func` (BR-TRACE-006).
/// Only simple identifier targets are captured; field/index writes are
/// ignored.
fn extract_assignment(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    let Some(name) = identifier_text(left, source) else {
        return;
    };
    if let Some(func) = ctx.current_func {
        result.writes.push(WriteInfo {
            writer_qn: Some(func.to_string()),
            var_name: name,
            line: node.start_position().row as u32 + 1,
        });
    }
}

/// Returns the text of `node` if it is a plain `identifier`, else `None`.
fn identifier_text(node: Node, source: &str) -> Option<String> {
    if node.kind() == "identifier" {
        node_text(node, source).map(String::from)
    } else {
        None
    }
}

/// Returns `true` if the identifier `node` is in a value-read position within
/// its parent expression (BR-TRACE-005).
///
/// Name-defining positions (let patterns, call functions, field names,
/// assignment left-hand sides) are excluded so only genuine variable reads
/// produce edges.
fn is_read_position(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        // Identifiers directly inside these containers/expressions are reads.
        "binary_expression"
        | "unary_expression"
        | "parenthesized_expression"
        | "return_expression"
        | "if_condition"
        | "while_condition"
        | "arguments"
        | "tuple_expression"
        | "array_expression"
        | "index_expression"
        | "reference_expression"
        | "deref_expression"
        | "closure_expression"
        | "format_args" => true,
        // `let x = y;` -> `y` (the value) is a read; `x` (the pattern) is not.
        "let_declaration" => !is_at_field(node, parent, "pattern"),
        // `foo(x)` -> the callee `foo` is not a read; arguments are handled
        // above via the `arguments` parent.
        "call_expression" => !is_at_field(node, parent, "function"),
        // `obj.field` -> `obj` (the value) is a read; the field name is not.
        "field_expression" => is_at_field(node, parent, "value"),
        // `x = y;` -> `y` (the right side) is a read; `x` (the left) is not.
        "assignment_expression" => !is_at_field(node, parent, "left"),
        _ => false,
    }
}

/// Returns `true` if `node` occupies the given named `field` of `parent`,
/// compared by byte range.
fn is_at_field(node: Node, parent: Node, field: &str) -> bool {
    parent
        .child_by_field_name(field)
        .is_some_and(|f| f.byte_range() == node.byte_range())
}

fn extract_extern_block(node: Node, source: &str, result: &mut ExtractResult) {
    // extern_block contains extern_item children which are function declarations.
    // Determine the foreign language from the string literal (e.g. "C").
    let lang = extern_language(node, source);
    let mut names = Vec::new();
    let mut signature = None;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            collect_extern_names(child, source, &mut names);
            if signature.is_none() {
                signature = node_text(child, source).map(String::from);
            }
        }
    }
    if names.is_empty() {
        return;
    }
    result.externs.push(ExternInfo {
        language: lang,
        names,
        line: node.start_position().row as u32 + 1,
        signature,
    });
}

fn collect_extern_names(node: Node, source: &str, names: &mut Vec<String>) {
    if node.kind() == "function_signature_item" {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Some(name) = node_text(name_node, source).map(String::from) {
                names.push(name);
            }
        }
    }
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            collect_extern_names(child, source, names);
        }
    }
}

fn extern_language(node: Node, source: &str) -> Language {
    // Look for a string_literal in the extern_modifier child
    // (e.g. extern "C" { ... }).
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "extern_modifier" {
                // The extern_modifier contains a string_literal.
                for j in 0..child.named_child_count() as u32 {
                    if let Some(grandchild) = child.named_child(j) {
                        if grandchild.kind() == "string_literal" {
                            let text = node_text(grandchild, source).unwrap_or("");
                            let cleaned = text.trim_matches('"').to_ascii_lowercase();
                            if cleaned == "c" {
                                return Language::C;
                            }
                            if cleaned == "fortran" {
                                return Language::Fortran;
                            }
                            if cleaned == "python" {
                                return Language::Python;
                            }
                        }
                    }
                }
            }
        }
    }
    // Also check direct string_literal children (older grammar versions).
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            if child.kind() == "string_literal" {
                let text = node_text(child, source).unwrap_or("");
                let cleaned = text.trim_matches('"').to_ascii_lowercase();
                if cleaned == "c" {
                    return Language::C;
                }
                if cleaned == "fortran" {
                    return Language::Fortran;
                }
                if cleaned == "python" {
                    return Language::Python;
                }
            }
        }
    }
    // Default to C for unknown extern blocks.
    Language::C
}

// ---------------------------------------------------------------------------
// Name / path helpers
// ---------------------------------------------------------------------------

fn is_pub(node: Node) -> bool {
    // Check if the node has a visibility modifier child that is `pub`.
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "visibility_modifier" {
                return true;
            }
        }
    }
    false
}

fn use_path(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "use_clause" => {
            // Recurse into the use_clause's child.
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(p) = use_path(child, source) {
                        return Some(p);
                    }
                }
            }
            None
        }
        "scoped_use_list" | "scoped_identifier" => {
            // Build the path from `path` and `name` fields.
            let path = node
                .child_by_field_name("path")
                .and_then(|n| use_path(n, source));
            let name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            match (path, name) {
                (Some(p), Some(n)) => Some(format!("{p}::{n}")),
                (None, Some(n)) => Some(n),
                (Some(p), None) => Some(p),
                (None, None) => None,
            }
        }
        "identifier" | "crate" | "self" | "super" => node_text(node, source).map(String::from),
        "use_as_clause" => {
            // `use foo as bar;` -> path is "foo"
            node.child_by_field_name("path")
                .and_then(|n| use_path(n, source))
        }
        "use_wildcard" => {
            // `use foo::*;` -> the path is the first named child
            // (e.g. scoped_identifier "std::collections"), and we append "::*".
            if let Some(path_node) = node.named_child(0) {
                if let Some(p) = use_path(path_node, source) {
                    return Some(format!("{p}::*"));
                }
            }
            Some("*".to_string())
        }
        "scoped_type_list" => {
            // Similar to scoped_use_list
            let path = node
                .child_by_field_name("path")
                .and_then(|n| use_path(n, source));
            let name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            match (path, name) {
                (Some(p), Some(n)) => Some(format!("{p}::{n}")),
                (None, Some(n)) => Some(n),
                (Some(p), None) => Some(p),
                (None, None) => None,
            }
        }
        _ => node_text(node, source).map(String::from),
    }
}

fn use_imported_names(node: Node, source: &str) -> Vec<String> {
    match node.kind() {
        "use_clause" => {
            let mut names = Vec::new();
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    names.extend(use_imported_names(child, source));
                }
            }
            names
        }
        "use_as_clause" => {
            // `use foo as bar;` -> imported name is "bar"
            node.child_by_field_name("alias")
                .and_then(|n| node_text(n, source).map(String::from))
                .into_iter()
                .collect()
        }
        "identifier" | "type_identifier" => node_text(node, source)
            .map(String::from)
            .into_iter()
            .collect(),
        "use_wildcard" => Vec::new(),
        "scoped_use_list" | "scoped_identifier" | "scoped_type_list" => {
            // For `std::io`, the imported name is the last component.
            node.child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from))
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" => node_text(node, source).map(String::from),
        "field_expression" => {
            // e.g. `obj.method()` or `Module::func()` -> extract the field name.
            let field = node.child_by_field_name("field")?;
            node_text(field, source).map(String::from)
        }
        "scoped_identifier" => {
            // e.g. `std::mem::swap` -> extract the last component.
            node.child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from))
        }
        "call_expression" => {
            let func = node.child_by_field_name("function")?;
            callee_name(func, source)
        }
        "parenthesized_expression" => {
            let inner = node.named_child(0)?;
            callee_name(inner, source)
        }
        "generic_function" => {
            // generic_function has a `function` field.
            let func = node.child_by_field_name("function")?;
            callee_name(func, source)
        }
        _ => None,
    }
}

fn pattern_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "tuple_pattern" | "tuple_struct_pattern" => {
            // Extract the first identifier in the tuple pattern.
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(name) = pattern_name(child, source) {
                        return Some(name);
                    }
                }
            }
            None
        }
        "struct_pattern" => node
            .child_by_field_name("type")
            .and_then(|n| node_text(n, source).map(String::from)),
        "reference_pattern" | "mut_pattern" => {
            let inner = node.named_child(0)?;
            pattern_name(inner, source)
        }
        _ => {
            // Fallback: only accept simple identifier text. Complex
            // patterns (array patterns, slices, etc.) would produce FQNs
            // with invalid characters (brackets, commas) that corrupt CSV
            // imports.
            let text = node_text(node, source)?;
            if text
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_')
                && text.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            {
                Some(text.to_string())
            } else {
                None
            }
        }
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

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Rust, parent)
}

/// Combines a parent scope context with a child scope name (ADR-005).
/// Returns `Some("{parent}_{child}")` when both are present, the non-`None`
/// value when only one is, or `None` when neither is.
fn combine_scope(parent: Option<&str>, child: Option<&str>) -> Option<String> {
    match (parent, child) {
        (Some(p), Some(c)) => Some(format!("{p}_{c}")),
        (None, Some(c)) => Some(c.to_string()),
        (Some(p), None) => Some(p.to_string()),
        (None, None) => None,
    }
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
        EdgeType::Contains,
        project,
    ));
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

    const RUST_SOURCE: &str = r#"use std::io;
extern "C" {
    fn c_function(x: i32) -> i32;
}
pub struct Point { x: i32, y: i32 }
enum Color { Red, Green, Blue }
trait Drawable { fn draw(&self); }
impl Drawable for Point { fn draw(&self) {} }
fn add(a: i32, b: i32) -> i32 { a + b }
fn main() {
    let result = add(1, 2);
    let p = Point { x: 1, y: 2 };
}
"#;

    fn extract(source: &str) -> ExtractResult {
        let ext = RustExtractor::new();
        ext.extract(source, "test.rs", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_rust() {
        assert_eq!(RustExtractor::new().language(), Language::Rust);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = RustExtractor::default();
        assert_eq!(ext.language(), Language::Rust);
    }

    #[test]
    fn extracts_use_declaration() {
        let result = extract(RUST_SOURCE);
        assert_eq!(result.imports.len(), 1, "should extract 1 use declaration");
        assert!(
            result.imports[0].source_file.contains("std"),
            "use path should contain std: {}",
            result.imports[0].source_file
        );
        assert!(
            result.imports[0].source_file.contains("io"),
            "use path should contain io: {}",
            result.imports[0].source_file
        );
    }

    #[test]
    fn extracts_extern_block_with_c_function() {
        let result = extract(RUST_SOURCE);
        assert_eq!(result.externs.len(), 1, "should extract 1 extern block");
        let ext = &result.externs[0];
        assert_eq!(ext.language, Language::C, "extern language should be C");
        assert!(
            ext.names.contains(&"c_function".to_string()),
            "extern names should contain c_function: {:?}",
            ext.names
        );
    }

    #[test]
    fn extracts_struct() {
        let result = extract(RUST_SOURCE);
        let structs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Struct)
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Point");
        assert!(structs[0].is_exported, "Point should be exported (pub)");
    }

    #[test]
    fn extracts_enum() {
        let result = extract(RUST_SOURCE);
        let enums: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Enum)
            .collect();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Color");
        assert!(!enums[0].is_exported, "Color should not be exported");
    }

    #[test]
    fn extracts_trait() {
        let result = extract(RUST_SOURCE);
        let traits: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Trait)
            .collect();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].name, "Drawable");
    }

    #[test]
    fn extracts_impl() {
        let result = extract(RUST_SOURCE);
        let impls: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Impl)
            .collect();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "Point");
    }

    #[test]
    fn extracts_functions() {
        let result = extract(RUST_SOURCE);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        // add, main, and draw (inside impl) are functions.
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"add"), "should extract add function");
        assert!(names.contains(&"main"), "should extract main function");
    }

    #[test]
    fn function_is_exported_when_pub() {
        let result = extract("pub fn public_fn() {} fn private_fn() {}");
        let public = result.nodes.iter().find(|n| n.name == "public_fn").unwrap();
        let private = result
            .nodes
            .iter()
            .find(|n| n.name == "private_fn")
            .unwrap();
        assert!(public.is_exported, "pub fn should be exported");
        assert!(!private.is_exported, "private fn should not be exported");
    }

    #[test]
    fn function_has_signature() {
        let result = extract(RUST_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn extracts_calls() {
        let result = extract(RUST_SOURCE);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"add"),
            "should extract call to add: {:?}",
            callees
        );
    }

    #[test]
    fn extracts_assignments() {
        let result = extract(RUST_SOURCE);
        assert!(!result.assignments.is_empty(), "should extract assignments");
        let result_assign = result
            .assignments
            .iter()
            .find(|a| a.target_name == "result")
            .expect("should find `let result = add(1, 2)` assignment");
        assert_eq!(result_assign.source_name, "add");
        assert!(
            result_assign.is_return_assign,
            "assignment from function call should be return assign"
        );
    }

    #[test]
    fn non_call_assignment_is_not_return_assign() {
        let result = extract("fn main() { let x = 5; }");
        let assign = result
            .assignments
            .iter()
            .find(|a| a.target_name == "x")
            .expect("should find `let x = 5` assignment");
        assert!(!assign.is_return_assign);
    }

    #[test]
    fn creates_contains_and_defines_edges() {
        let result = extract(RUST_SOURCE);
        let contains_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Contains)
            .count();
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert_eq!(contains_count, node_count);
        assert_eq!(defines_count, node_count);
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract(RUST_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert_eq!(add.qualified_name, "proj.test.rs.add");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn extracts_const_and_static() {
        let src = "const MAX: i32 = 100; static GLOBAL: i32 = 0;";
        let result = extract(src);
        let consts: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Const)
            .collect();
        let statics: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Static)
            .collect();
        assert_eq!(consts.len(), 1);
        assert_eq!(consts[0].name, "MAX");
        assert_eq!(statics.len(), 1);
        assert_eq!(statics[0].name, "GLOBAL");
    }

    #[test]
    fn extracts_type_alias() {
        let src = "type Score = i32;";
        let result = extract(src);
        let aliases: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias)
            .collect();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].name, "Score");
    }

    #[test]
    fn extracts_macro_definition() {
        let src = "macro_rules! say_hello { () => { println!(\"hello\"); } }";
        let result = extract(src);
        let macros: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Macro)
            .collect();
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "say_hello");
    }

    #[test]
    fn handles_method_calls() {
        let src = "fn main() { let s = String::new(); s.push_str(\"hi\"); }";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callees.contains(&"new"), "should extract String::new call");
        assert!(
            callees.contains(&"push_str"),
            "should extract s.push_str call"
        );
    }

    #[test]
    fn handles_generic_function_calls() {
        let src = "fn main() { let v = Vec::<i32>::new(); }";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"new"),
            "should extract generic function call"
        );
    }

    #[test]
    fn use_wildcard_extracts_path() {
        let src = "use std::collections::*;";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert!(result.imports[0].source_file.contains("*"));
        assert!(result.imports[0].imported_names.is_empty());
    }

    #[test]
    fn use_as_clause_extracts_alias() {
        let src = "use std::io as ioo;";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert!(result.imports[0]
            .imported_names
            .contains(&"ioo".to_string()));
    }

    #[test]
    fn result_language_is_rust() {
        let result = extract(RUST_SOURCE);
        assert_eq!(result.language, Language::Rust);
        assert_eq!(result.file_path, "test.rs");
    }

    #[test]
    fn impl_stores_trait_in_properties() {
        let result = extract(RUST_SOURCE);
        let impls: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Impl)
            .collect();
        assert_eq!(impls.len(), 1);
        let props = &impls[0].properties;
        assert!(
            props.get("trait").is_some(),
            "impl should store trait name in properties: {props}"
        );
        assert_eq!(props.get("trait").unwrap(), "Drawable");
    }

    // --- reads/writes extraction (BR-TRACE-005 / BR-TRACE-006) ---

    #[test]
    fn extracts_reads_from_binary_expression() {
        // `a + b` reads both operands.
        let src = "fn add(a: i32, b: i32) -> i32 { a + b }";
        let result = extract(src);
        let read_vars: Vec<_> = result.reads.iter().map(|r| r.var_name.as_str()).collect();
        assert!(
            read_vars.contains(&"a"),
            "should read operand a: {read_vars:?}"
        );
        assert!(
            read_vars.contains(&"b"),
            "should read operand b: {read_vars:?}"
        );
        for read in &result.reads {
            assert_eq!(
                read.reader_qn.as_deref(),
                Some("add"),
                "reader should be the enclosing function"
            );
        }
    }

    #[test]
    fn extracts_writes_from_let_declarations() {
        let src = "fn main() { let x = 1; let y = 2; }";
        let result = extract(src);
        let write_vars: Vec<_> = result.writes.iter().map(|w| w.var_name.as_str()).collect();
        assert!(write_vars.contains(&"x"), "should write x: {write_vars:?}");
        assert!(write_vars.contains(&"y"), "should write y: {write_vars:?}");
        for write in &result.writes {
            assert_eq!(
                write.writer_qn.as_deref(),
                Some("main"),
                "writer should be the enclosing function"
            );
        }
    }

    #[test]
    fn extracts_writes_from_assignment_expression() {
        // `x = 5;` reassigns x -> WriteInfo(x). `y` is read on the right side.
        let src = "fn main() { let mut x = 0; let y = 1; x = y; }";
        let result = extract(src);
        let x_writes: Vec<_> = result.writes.iter().filter(|w| w.var_name == "x").collect();
        // One write from `let mut x = 0` and one from `x = y`.
        assert_eq!(
            x_writes.len(),
            2,
            "x should be written twice: {:?}",
            x_writes
        );

        let read_vars: Vec<_> = result.reads.iter().map(|r| r.var_name.as_str()).collect();
        assert!(
            read_vars.contains(&"y"),
            "right-hand side of assignment should be a read: {read_vars:?}"
        );
    }

    #[test]
    fn reads_exclude_callee_and_pattern_positions() {
        // `let result = add(1, 2);` -> `result` is a write (pattern), `add` is
        // the callee (function field), `1`/`2` are literals. No reads expected.
        let src = "fn main() { let result = add(1, 2); } fn add(a: i32, b: i32) -> i32 { a + b }";
        let result = extract(src);
        let main_reads: Vec<_> = result
            .reads
            .iter()
            .filter(|r| r.reader_qn.as_deref() == Some("main"))
            .collect();
        assert!(
            main_reads.is_empty(),
            "main should produce no reads (only a write + a call): {main_reads:?}"
        );
        // `result` is written, not read.
        let main_writes: Vec<_> = result
            .writes
            .iter()
            .filter(|w| w.writer_qn.as_deref() == Some("main"))
            .collect();
        assert_eq!(main_writes.len(), 1);
        assert_eq!(main_writes[0].var_name, "result");
    }

    #[test]
    fn reads_from_field_expression_object() {
        // `obj.field` -> `obj` (the value) is read; `field` is a property name.
        let src = "fn main() { let obj = make(); let v = obj.field; }";
        let result = extract(src);
        let read_vars: Vec<_> = result.reads.iter().map(|r| r.var_name.as_str()).collect();
        assert!(
            read_vars.contains(&"obj"),
            "object of field access should be a read: {read_vars:?}"
        );
        assert!(
            !read_vars.contains(&"field"),
            "field name should not be a variable read: {read_vars:?}"
        );
    }

    #[test]
    fn no_reads_or_writes_outside_function() {
        // Top-level const has no enclosing function -> no reads/writes.
        let src = "const MAX: i32 = 100;";
        let result = extract(src);
        assert!(
            result.reads.is_empty(),
            "top-level const should produce no reads"
        );
        assert!(
            result.writes.is_empty(),
            "top-level const should produce no writes"
        );
    }

    #[test]
    fn extern_block_with_fortran_language() {
        let src = r#"extern "Fortran" { fn f subroutine(x: i32); }"#;
        let result = extract(src);
        assert_eq!(result.externs.len(), 1);
        assert_eq!(result.externs[0].language, Language::Fortran);
    }

    #[test]
    fn extern_block_with_python_language() {
        let src = r#"extern "Python" { fn py_func(x: i32); }"#;
        let result = extract(src);
        assert_eq!(result.externs.len(), 1);
        assert_eq!(result.externs[0].language, Language::Python);
    }

    #[test]
    fn extern_block_default_language_is_c() {
        // An extern block with an unrecognized ABI string defaults to C.
        let src = r#"extern "Rust" { fn rust_func(x: i32); }"#;
        let result = extract(src);
        assert_eq!(result.externs.len(), 1);
        assert_eq!(result.externs[0].language, Language::C);
    }

    #[test]
    fn tuple_destructuring_pattern() {
        let src = "fn main() { let (a, b) = (1, 2); }";
        let result = extract(src);
        let writes: Vec<_> = result.writes.iter().map(|w| w.var_name.as_str()).collect();
        // pattern_name extracts only the first binding of a tuple pattern.
        assert!(writes.contains(&"a"), "should write first binding a: {writes:?}");
        assert!(!writes.contains(&"b"), "should not write b (only first binding extracted): {writes:?}");
    }

    #[test]
    fn reference_pattern() {
        let src = "fn main() { let x = 1; let &y = &x; }";
        let result = extract(src);
        let writes: Vec<_> = result.writes.iter().map(|w| w.var_name.as_str()).collect();
        assert!(writes.contains(&"y"), "should write y from reference pattern: {writes:?}");
    }

    #[test]
    fn struct_pattern() {
        let src = "fn main() { struct P { x: i32 } let p = P { x: 1 }; let P { x } = p; }";
        let result = extract(src);
        let writes: Vec<_> = result.writes.iter().map(|w| w.var_name.as_str()).collect();
        // pattern_name returns the type name for a struct pattern (`P`), not
        // the field name (`x`).
        assert!(writes.contains(&"P"), "should write type name P from struct pattern: {writes:?}");
    }

    #[test]
    fn parenthesized_call_expression() {
        let src = "fn foo() -> fn() { bar } fn bar() {} fn main() { (foo())(); }";
        let result = extract(src);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"foo"),
            "should extract call to foo: {callees:?}"
        );
    }

    #[test]
    fn chained_call_expression() {
        // `foo()()` — the outer call's function is itself a call_expression.
        let src = "fn foo() -> fn() { bar } fn bar() {} fn main() { foo()(); }";
        let result = extract(src);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"foo"),
            "should extract outer call to foo: {callees:?}"
        );
    }

    #[test]
    fn let_binding_with_non_identifier_value() {
        // `let x = if ... { ... } else { ... };` — the value is not a simple
        // identifier, so source_name is empty and is_return_assign is false.
        let src = "fn main() { let x = if true { 1 } else { 2 }; }";
        let result = extract(src);
        let assign = result
            .assignments
            .iter()
            .find(|a| a.target_name == "x")
            .expect("should find assignment to x");
        assert_eq!(assign.source_name, "");
        assert!(!assign.is_return_assign);
    }

    #[test]
    fn use_declaration_with_scoped_identifier() {
        // `use std::collections::HashMap;` — covers use_path scoped_identifier
        // with both path and name fields present.
        let src = "use std::collections::HashMap;";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert!(
            result.imports[0]
                .imported_names
                .contains(&"HashMap".to_string()),
            "should import HashMap: {:?}",
            result.imports[0].imported_names
        );
    }

    #[test]
    fn mod_block_includes_module_in_parent() {
        // 两个不同 mod 块各有同名 struct + impl，模块名应纳入 parent，
        // 使两个 impl 的 FQN 不同（修复 P0-1 rust-nested-tail-collision）。
        let src = r#"pub mod outer {
    pub struct Inner;
    impl Inner { pub fn from_outer(&self) {} }
}
pub mod other {
    pub struct Inner;
    impl Inner { pub fn from_other(&self) {} }
}
"#;
        let result = extract(src);
        let impl_qns: Vec<&str> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function && n.name.contains("from_"))
            .map(|n| n.qualified_name.as_str())
            .collect();
        // 两个 impl 方法的 FQN 应分别含 outer 和 other
        assert!(
            impl_qns.iter().any(|q| q.contains("outer")),
            "outer impl FQN should contain 'outer': {impl_qns:?}"
        );
        assert!(
            impl_qns.iter().any(|q| q.contains("other")),
            "other impl FQN should contain 'other': {impl_qns:?}"
        );
        // 无 FQN 碰撞
        let mut sorted = impl_qns.clone();
        sorted.sort();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            before,
            "FQN collision detected: {impl_qns:?}"
        );
    }

    #[test]
    fn mod_block_nested() {
        // 嵌套 mod：parent 链应含 a_b
        let src = "pub mod a { pub mod b { pub struct X; } }";
        let result = extract(src);
        let x_qn = result
            .nodes
            .iter()
            .find(|n| n.name == "X")
            .map(|n| n.qualified_name.as_str())
            .expect("X struct should be extracted");
        assert!(
            x_qn.contains("a_b"),
            "nested mod FQN should contain 'a_b': {x_qn}"
        );
    }

    // --- P2-1 regression: mod_item MUST create Module nodes ---

    #[test]
    fn extracts_mod_item_as_module_node() {
        // P2-1 regression: `mod foo;` and `mod foo {}` previously only updated
        // current_parent without creating a Module node, causing 100% loss of
        // Rust module declarations (0 vs gitnexus 24 in tokei).
        let src = "pub mod network;\nmod parser {}";
        let result = extract(src);
        let modules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Module)
            .collect();
        assert_eq!(modules.len(), 2, "should extract 2 Module nodes");
        let names: Vec<_> = modules.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"network"), "mod network; should be a Module");
        assert!(names.contains(&"parser"), "mod parser {{}} should be a Module");
        for m in &modules {
            assert_eq!(m.language, Some(Language::Rust));
            assert!(m.is_global, "top-level mod should be global");
        }
    }

    #[test]
    fn mod_item_has_contains_and_defines_edges() {
        // P2-1: Module node must have CONTAINS/DEFINES edges or it's invisible.
        let src = "pub mod foo;";
        let result = extract(src);
        let module_node = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Module)
            .expect("Module node should exist");
        let contains_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Contains && e.target == module_node.id)
            .count();
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines && e.target == module_node.id)
            .count();
        assert_eq!(contains_count, 1, "Module should have 1 CONTAINS edge");
        assert_eq!(defines_count, 1, "Module should have 1 DEFINES edge");
    }
}
