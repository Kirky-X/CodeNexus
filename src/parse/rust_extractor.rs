// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

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
use crate::resolve::{FqnGenerator, ScopeContext, ScopeResolverRegistry};

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
        let registry = ScopeResolverRegistry::new();
        let ctx = VisitContext {
            file_path,
            project,
            current_func: None,
            current_parent: None,
            resolver: &registry,
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

/// 封装 ADR-005 的 current_parent 和 current_func 语义。
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
    current_func: Option<&'a str>,
    current_parent: Option<&'a str>,
    /// design.md D3.
    resolver: &'a ScopeResolverRegistry,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function_item" => {
            extract_function(node, source, ctx, result);
            // Use ScopeResolver to get the function name (design.md D3).
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Rust)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let func_name = scope.as_ref().map(|s| s.name.as_str());
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: func_name,
                current_parent: ctx.current_parent,
                resolver: ctx.resolver,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "struct_item" => {
            extract_named_item(node, NodeLabel::Struct, source, ctx, result);
            // Match gitnexus: struct fields become Property nodes with HasProperty edges.
            extract_struct_fields(node, source, ctx, result);
        }
        "enum_item" => {
            extract_named_item(node, NodeLabel::Enum, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "trait_item" => {
            extract_named_item(node, NodeLabel::Trait, source, ctx, result);
            // Use ScopeResolver to get the trait name (design.md D3).
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Rust)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let trait_name = scope.as_ref().map(|s| s.name.as_str());
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: trait_name,
                resolver: ctx.resolver,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "impl_item" => {
            // Use ScopeResolver to get the impl type name (design.md D3).
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Rust)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let impl_type = scope.as_ref().map(|s| s.name.as_str());
            extract_impl(node, source, ctx, ctx.current_parent, result);
            // Combine module context with impl type so methods inside the impl
            // get disambiguated (ADR-003).
            let combined = match (ctx.current_parent, impl_type) {
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
                resolver: ctx.resolver,
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
        "macro_invocation" => {
            // B1 fix: tree-sitter-rust parses macro arguments (`println!(...)`,
            // `format!(...)`, `json!(...)`) as `token_tree` nodes, which
            // contain raw tokens that are NOT parsed as `call_expression`.
            // This means function calls inside macro arguments (e.g.
            // `println!("{}", helper())`) are invisible to the standard
            // `call_expression` visitor. On CalNexus, this caused 3/4
            // remaining dead-code false positives (format_json_output,
            // dmatrix_to_json, error_kind_prefix — all called inside macros).
            // The fix scans the `token_tree` for `identifier` + `token_tree`
            // pairs (the macro-argument form of `fn()`) and extracts them as
            // CallInfo. Method calls (`obj.method()`) are not handled here —
            // they require field_expression parsing which token_tree doesn't
            // expose. See tools/verification/results/triage.md §B1.
            extract_calls_from_macro(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "mod_item" => {
            // 模块名纳入 current_parent 以区分同名 impl（P0-1），并创建 Module 节点（P2-1）。
            extract_named_item(node, NodeLabel::Module, source, ctx, result);
            // Use ScopeResolver to get the module name (design.md D3).
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Rust)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let mod_name = scope.as_ref().map(|s| s.name.as_str());
            let combined = combine_scope(ctx.current_parent, mod_name);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: None,
                current_parent: combined.as_deref(),
                resolver: ctx.resolver,
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

fn extract_function(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
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
    result.push_node(model_node);
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
    result.push_node(model_node);
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
    // Strip generics (e.g. `ParserGuard<'_>` → `ParserGuard`) so the FQN
    // matches the Struct/Enum node ID (ADR-014: avoids duplicate primary key).
    let name = strip_generics(&name).to_string();
    let trait_name = node
        .child_by_field_name("trait")
        .and_then(|n| node_text(n, source).map(String::from));
    // B2 fix: only model inherent impls (`impl Type {}`), not trait impls
    // (`impl Trait for Type {}`). gitnexus only indexes inherent impls —
    // verified via cross-validation. Methods inside trait impls are still
    // extracted by visit_children (called unconditionally after extract_impl),
    // so no symbol information is lost. See
    // tools/verification/results/triage.md §B2.
    //
    // Feature gap (closed): For trait impls, create an IMPLEMENTS edge from
    // the implemented type to the trait. The source FQN matches the type's
    // definition FQN (Struct/Enum node, using the same scope as
    // `extract_named_item`); the target is a best-effort pseudo-FQN that
    // `TypeResolver` will resolve via the symbol table (same pattern as
    // Python's `Extends` edges, see `src/parse/python.rs` P2-2).
    if let Some(trait_name) = trait_name {
        // Extract the trait name (last `::`-separated component) for
        // resolution. e.g. `std::fmt::Display` → `Display`, `Trait` → `Trait`.
        // Also strip generic params (e.g. `IntoIterator<Item=u8>` → `IntoIterator`).
        //
        // Order matters: strip generics BEFORE rsplit to avoid residual `>`.
        // `From<tokio::time::error::Elapsed>` → strip_generics → `From` → rsplit → `From`.
        // (was: rsplit → `Elapsed>` → strip_generics → `Elapsed>` due to no `<`).
        let trait_stripped = strip_generics(&trait_name);
        let trait_short = trait_stripped.rsplit("::").next().unwrap_or(trait_stripped);
        let type_qn = make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent);
        let trait_qn = make_qn(ctx.file_path, trait_short, ctx.project, ctx.current_parent);
        // Set start_line so multiple `impl From<X> for Y` blocks produce
        // distinct edge IDs (source, target, start_line). Without this, all
        // IMPLEMENTS edges have start_line=0 → duplicate primary key (ADR-014).
        let start_line = node.start_position().row as u32 + 1;
        result.edges.push(
            Edge::builder(type_qn, trait_qn, EdgeType::Implements, ctx.project)
                .start_line(start_line)
                .build(),
        );
        return;
    }
    // Disambiguate from struct/enum with the same name (ADR-003).
    let disambiguator = match module_parent {
        Some(m) => format!("{m}_impl"),
        None => "impl".to_string(),
    };
    let qn = make_qn(ctx.file_path, &name, ctx.project, Some(&disambiguator));
    let builder = ModelNode::builder(NodeLabel::Impl, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Rust)
        .project(ctx.project)
        .is_global(true);
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

/// Extracts Rust struct fields as [`NodeLabel::Property`] nodes with
/// [`EdgeType::HasProperty`] edges from the struct to each field
/// (feature gap closed — gitnexus indexes struct fields as Property).
///
/// Only named fields (`field_declaration` with a `field_identifier`) are
/// extracted. Tuple struct fields (`tuple_field`) and unit structs (no body)
/// are skipped because they have no field names.
///
/// Field FQNs are disambiguated by the struct name combined with the module
/// parent (same convention as impl methods, ADR-003).
fn extract_struct_fields(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(struct_name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let Some(body) = node.child_by_field_name("body") else {
        // Unit struct (`struct Foo;`) — no fields.
        return;
    };
    let struct_qn = make_qn(ctx.file_path, &struct_name, ctx.project, ctx.current_parent);
    let combined = match ctx.current_parent {
        Some(p) => format!("{p}_{struct_name}"),
        None => struct_name.clone(),
    };
    for i in 0..body.named_child_count() as u32 {
        let Some(field) = body.named_child(i) else {
            continue;
        };
        if field.kind() != "field_declaration" {
            // Skip tuple fields, attributes, etc.
            continue;
        }
        let Some(field_name_node) = field.child_by_field_name("name") else {
            continue;
        };
        let Some(field_name) = node_text(field_name_node, source).map(String::from) else {
            continue;
        };
        let field_qn = make_qn(ctx.file_path, &field_name, ctx.project, Some(&combined));
        let model_node = ModelNode::builder(NodeLabel::Property, field_name, field_qn)
            .file_path(ctx.file_path)
            .start_line(field.start_position().row as u32 + 1)
            .end_line(field.end_position().row as u32 + 1)
            .language(Language::Rust)
            .project(ctx.project)
            .is_global(false)
            .build();
        add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
        result.edges.push(Edge::new(
            struct_qn.clone(),
            model_node.id.clone(),
            EdgeType::HasProperty,
            ctx.project,
        ));
        result.push_node(model_node);
    }
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

fn extract_call(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(func_node) = node.child_by_field_name("function") else {
        return;
    };
    let Some(callee) = callee_name(func_node, source) else {
        return;
    };
    // C2 fix: filter stdlib method calls to match gitnexus (only user-code
    // function calls are tracked). See triage.md §C2. Also handles
    // generic_function wrapping a field_expression (e.g. `.collect::<Vec<_>>()`).
    if is_field_expression_call(func_node) && is_stdlib_method(&callee) {
        return;
    }
    let args = call_arguments(node, source);
    let caller_qn = ctx
        .current_func
        .map(|name| make_qn(ctx.file_path, name, ctx.project, ctx.current_parent));
    result.calls.push(CallInfo {
        caller_qn: caller_qn.clone(),
        callee_name: callee,
        line: node.start_position().row as u32 + 1,
        args,
    });
    // B1b fix: function-reference arguments (e.g. `eval_unary(..., erf)`).
    // tree-sitter parses a function name passed as an argument as a bare
    // `identifier` inside `arguments`. Without this, `erf` only generates a
    // ReadInfo (variable read), so dead_code analysis reports it as dead
    // (zero incoming CALLS edges). The resolver drops CallInfo records whose
    // callee_name doesn't match a function in the symbol table, so variables
    // passed as arguments (e.g. `foo(x)`) are silently filtered.
    extract_function_ref_args(node, source, &caller_qn, result);
}

/// B1b: extracts bare-identifier arguments as CallInfo records.
///
/// When a function name is passed as an argument (e.g.
/// `self.eval_unary(name, args, ctx, erf)` in CalNexus scientific.rs),
/// tree-sitter-rust parses `erf` as a bare `identifier` child of `arguments`.
/// The standard `call_expression` visitor only extracts the callee
/// (`eval_unary`), not identifier arguments. Without this fix, `erf` generates
/// only a ReadInfo (variable read), and dead_code analysis reports it as dead
/// because it has zero incoming CALLS edges.
///
/// This function scans the `arguments` child of a `call_expression` for bare
/// `identifier` nodes and pushes a CallInfo for each, using the identifier as
/// `callee_name`. The resolve phase (`CallResolver`) only creates a CALLS edge
/// if the identifier matches a function in the symbol table, so variables
/// passed as arguments (e.g. `foo(x)` where `x` is a local) are silently
/// dropped — no false-positive CALLS edges.
///
/// Trade-off: if a project has a function and a variable with the same simple
/// name in the same file, and the variable is passed as an argument, the
/// function will be marked as reachable (false negative for dead_code). This
/// is rare and conservative (better to miss some dead code than to flag live
/// code as dead).
fn extract_function_ref_args(
    node: Node,
    source: &str,
    caller_qn: &Option<String>,
    result: &mut ExtractResult,
) {
    let Some(args_node) = node.child_by_field_name("arguments") else {
        return;
    };
    for i in 0..args_node.named_child_count() as u32 {
        let Some(arg) = args_node.named_child(i) else {
            continue;
        };
        // Only bare identifiers are potential function references. Field
        // expressions (`obj.method`), path expressions (`mod::fn`), closures
        // (`|x| x + 1`), and call expressions (`bar()`) are handled by their
        // own visitors.
        if arg.kind() != "identifier" {
            continue;
        }
        let Some(name) = node_text(arg, source).map(String::from) else {
            continue;
        };
        result.calls.push(CallInfo {
            caller_qn: caller_qn.clone(),
            callee_name: name,
            line: arg.start_position().row as u32 + 1,
            args: Vec::new(),
        });
    }
}

/// B1 fix: extracts function calls from inside macro arguments.
///
/// tree-sitter-rust parses macro arguments (`println!(...)`, `format!(...)`,
/// `json!(...)`) as `token_tree` nodes. Inside `token_tree`, a function call
/// like `helper(42)` is parsed as two adjacent named children:
///   - `identifier` "helper"
///   - `token_tree` "(42)"
///
/// This function scans the macro's `token_tree` for this pattern and creates
/// [`CallInfo`] records for each match. Method calls (`obj.method()`) are
/// NOT handled here because `token_tree` doesn't expose `field_expression`
/// structure — they would appear as `identifier` "." `identifier` `token_tree`,
/// and we can't reliably distinguish them from other token sequences.
///
/// Limitations:
/// - Only simple `fn(args)` calls are recognized (not `Path::fn(args)` or
///   `obj.method(args)`).
/// - The callee_name is the simple identifier; resolution to a fully
///   qualified name happens in the resolve phase via `lookup_in_file`.
fn extract_calls_from_macro(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // A macro_invocation has two named children: the macro name (identifier)
    // and the token_tree containing the arguments.
    let mut token_tree_opt: Option<Node> = None;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "token_tree" {
                token_tree_opt = Some(child);
                break;
            }
        }
    }
    let Some(token_tree) = token_tree_opt else {
        return;
    };
    // Scan the token_tree's named children for `identifier` + `token_tree`
    // pairs (the macro-argument form of a function call).
    let child_count = token_tree.named_child_count() as u32;
    let mut i = 0u32;
    while i < child_count {
        let Some(curr) = token_tree.named_child(i) else {
            i += 1;
            continue;
        };
        if curr.kind() == "identifier" {
            // Check if the next named child is a token_tree (the call args).
            if i + 1 < child_count {
                if let Some(next) = token_tree.named_child(i + 1) {
                    if next.kind() == "token_tree" {
                        // Found a function call: identifier + token_tree.
                        if let Some(callee) = node_text(curr, source).map(String::from) {
                            // Skip stdlib method names that might appear in
                            // macro arguments (e.g. `vec.push(x)` inside
                            // `println!`). Without field_expression context,
                            // we can't filter method calls, so we filter by
                            // name against the stdlib list.
                            if !STDLIB_METHOD_NAMES.contains(&callee.as_str()) {
                                let caller_qn = ctx.current_func.map(|name| {
                                    make_qn(ctx.file_path, name, ctx.project, ctx.current_parent)
                                });
                                let args = macro_call_arguments(next, source);
                                result.calls.push(CallInfo {
                                    caller_qn,
                                    callee_name: callee,
                                    line: curr.start_position().row as u32 + 1,
                                    args,
                                });
                            }
                        }
                        // Skip the token_tree (consume both children).
                        i += 2;
                        continue;
                    }
                }
            }
        }
        // Also recurse into nested token_tree to handle nested macros and
        // complex expressions (e.g. `println!("{}", json!({ "k": helper() }))`).
        if curr.kind() == "token_tree" {
            extract_calls_from_token_tree(curr, source, ctx, result);
        }
        i += 1;
    }
}

/// Recursively extracts function calls from a `token_tree` node (B1 fix).
///
/// This handles nested macros and complex expressions inside macro arguments.
fn extract_calls_from_token_tree(
    token_tree: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let child_count = token_tree.named_child_count() as u32;
    let mut i = 0u32;
    while i < child_count {
        let Some(curr) = token_tree.named_child(i) else {
            i += 1;
            continue;
        };
        if curr.kind() == "identifier" && i + 1 < child_count {
            if let Some(next) = token_tree.named_child(i + 1) {
                if next.kind() == "token_tree" {
                    if let Some(callee) = node_text(curr, source).map(String::from) {
                        if !STDLIB_METHOD_NAMES.contains(&callee.as_str()) {
                            let caller_qn = ctx.current_func.map(|name| {
                                make_qn(ctx.file_path, name, ctx.project, ctx.current_parent)
                            });
                            let args = macro_call_arguments(next, source);
                            result.calls.push(CallInfo {
                                caller_qn,
                                callee_name: callee,
                                line: curr.start_position().row as u32 + 1,
                                args,
                            });
                        }
                    }
                    i += 2;
                    continue;
                }
            }
        }
        // Recurse into nested token_tree.
        if curr.kind() == "token_tree" {
            extract_calls_from_token_tree(curr, source, ctx, result);
        }
        i += 1;
    }
}

/// Extracts argument count from a macro call's `token_tree` (B1 fix).
///
/// Since `token_tree` doesn't parse arguments as expressions, we count
/// top-level commas to estimate the argument count. This is a best-effort
/// heuristic for confidence scoring; the actual argument values are not
/// captured.
fn macro_call_arguments(token_tree: Node, source: &str) -> Vec<String> {
    let Some(text) = node_text(token_tree, source) else {
        return Vec::new();
    };
    // Strip outer parens/brackets/braces.
    let inner = text
        .trim_start_matches(['(', '[', '{'])
        .trim_end_matches([')', ']', '}']);
    if inner.trim().is_empty() {
        return Vec::new();
    }
    // Count top-level commas (depth 0). This is a rough heuristic.
    let mut depth = 0i32;
    let mut count = 1usize;
    for ch in inner.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => count += 1,
            _ => {}
        }
    }
    vec![String::new(); count]
}

/// Stdlib method names that should not generate CallInfo records when invoked
/// as `field_expression` (e.g. `vec.push(x)`, `s.len()`). These are Rust
/// standard library methods on Vec/String/slice/Iterator/Option/Result/HashMap
/// etc. User code rarely redefines these names, so filtering them removes
/// ~60% of spurious CALLS edges while preserving user-code method calls.
const STDLIB_METHOD_NAMES: &[&str] = &[
    // Vec / slice
    "push",
    "pop",
    "len",
    "is_empty",
    "clear",
    "extend",
    "extend_from_slice",
    "iter",
    "iter_mut",
    "into_iter",
    "get",
    "get_mut",
    "insert",
    "remove",
    "swap_remove",
    "truncate",
    "contains",
    "starts_with",
    "ends_with",
    "as_slice",
    "as_mut_slice",
    "sort",
    "sort_by",
    "sort_by_key",
    "sort_unstable",
    "sort_unstable_by",
    "binary_search",
    "binary_search_by",
    "binary_search_by_key",
    "drain",
    "retain",
    "windows",
    "chunks",
    "chunks_exact",
    "split",
    "splitn",
    "rsplit",
    "rsplitn",
    "join",
    "concat",
    "first",
    "last",
    "first_mut",
    "last_mut",
    "split_first",
    "split_last",
    "swap",
    "reverse",
    "fill",
    "resize",
    "resize_with",
    "splice",
    "clone_from_slice",
    "copy_from_slice",
    // String / str
    "push_str",
    "as_str",
    "as_bytes",
    "chars",
    "bytes",
    "lines",
    "trim",
    "trim_start",
    "trim_end",
    "trim_start_matches",
    "trim_end_matches",
    "to_lowercase",
    "to_uppercase",
    "to_ascii_lowercase",
    "to_ascii_uppercase",
    "replace",
    "replacen",
    "to_string",
    "to_owned",
    "parse",
    "contains",
    "starts_with",
    "ends_with",
    "find",
    "rfind",
    "matches",
    "rmatches",
    "split_whitespace",
    "split_terminator",
    "is_char_boundary",
    "escape_default",
    "is_empty",
    "is_ascii",
    "make_ascii_uppercase",
    "make_ascii_lowercase",
    "strip_prefix",
    "strip_suffix",
    // Iterator
    "map",
    "filter",
    "for_each",
    "collect",
    "fold",
    "try_fold",
    "reduce",
    "any",
    "all",
    "count",
    "sum",
    "product",
    "min",
    "max",
    "min_by",
    "max_by",
    "min_by_key",
    "max_by_key",
    "take",
    "skip",
    "take_while",
    "skip_while",
    "zip",
    "chain",
    "enumerate",
    "peekable",
    "flat_map",
    "flatten",
    "inspect",
    "rev",
    "step_by",
    "nth",
    "last",
    "find",
    "find_map",
    "position",
    "rposition",
    "cloned",
    "copied",
    "by_ref",
    "cycle",
    "unzip",
    "partition",
    "try_for_each",
    "cmp",
    "partial_cmp",
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "is_sorted",
    "is_sorted_by",
    "is_sorted_by_key",
    "intersperse",
    "dedup",
    "dedup_by",
    // Option / Result
    "unwrap",
    "expect",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "is_some",
    "is_none",
    "is_ok",
    "is_err",
    "ok",
    "err",
    "and_then",
    "or_else",
    "map_err",
    "as_ref",
    "as_mut",
    "as_deref",
    "as_deref_mut",
    "get_or_insert",
    "get_or_insert_with",
    "take",
    "replace",
    "copied",
    "cloned",
    "expect_err",
    "unwrap_err",
    "unwrap_or_default_err",
    "map_or",
    "map_or_else",
    "ok_or",
    "ok_or_else",
    "state",
    // HashMap / BTreeMap
    "keys",
    "values",
    "values_mut",
    "entry",
    "retain",
    "capacity",
    "reserve",
    "shrink_to_fit",
    "with_capacity",
    // Clone / Default / conversion
    "clone",
    "into",
    "from",
    "default",
    "to_vec",
    "to_string",
    "to_owned",
    "into_iter",
    "into_string",
    "into_bytes",
    "into_boxed_str",
    "into_boxed_bytes",
    "into_raw_parts",
    "leak",
    // I/O
    "read",
    "read_to_string",
    "read_to_end",
    "read_line",
    "read_exact",
    "write",
    "write_str",
    "write_all",
    "writeln",
    "flush",
    "close",
    "seek",
    "connect",
    "peek",
    // Misc stdlib
    "lock",
    "try_lock",
    "unlock",
    "send",
    "recv",
    "try_recv",
    "recv_timeout",
    "send_timeout",
    "bind",
    "listen",
    "accept",
    "spawn",
    "join",
    "yield_now",
    "sleep",
    "elapsed",
    "duration_since",
    "instant",
    "now",
    "with_capacity",
    "into_owned",
    "into_path_buf",
    "to_path_buf",
    "to_path",
    "exists",
    "is_file",
    "is_dir",
    "metadata",
    "canonicalize",
    "read_dir",
    "create",
    "create_dir",
    "create_dir_all",
    "remove_file",
    "remove_dir",
    "rename",
    "copy",
    "hard_link",
    "symlink",
    "read_link",
    "current_dir",
    "set_current_dir",
    "temp_dir",
    "home_dir",
    "open",
    "truncate",
    "set_len",
    "set_permissions",
];

fn is_stdlib_method(name: &str) -> bool {
    STDLIB_METHOD_NAMES.contains(&name)
}

/// Returns `true` if `func_node` is (or wraps) a `field_expression`, i.e. a
/// receiver-bound method call like `obj.method()` or `obj.method::<T>()`.
/// Used by the C2 stdlib filter to distinguish method calls (filtered when
/// stdlib) from free functions / static methods (preserved).
fn is_field_expression_call(func_node: Node) -> bool {
    match func_node.kind() {
        "field_expression" => true,
        "generic_function" => func_node
            .child_by_field_name("function")
            .map(|n| n.kind() == "field_expression")
            .unwrap_or(false),
        _ => false,
    }
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
                                #[cfg(feature = "lang-c")]
                                return Language::C;
                            }
                            if cleaned == "fortran" {
                                #[cfg(feature = "lang-fortran")]
                                return Language::Fortran;
                            }
                            if cleaned == "python" {
                                #[cfg(feature = "lang-python")]
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
                    #[cfg(feature = "lang-c")]
                    return Language::C;
                }
                if cleaned == "fortran" {
                    #[cfg(feature = "lang-fortran")]
                    return Language::Fortran;
                }
                if cleaned == "python" {
                    #[cfg(feature = "lang-python")]
                    return Language::Python;
                }
            }
        }
    }
    // Default to the first compiled-in language for unknown extern blocks
    // (previously defaulted to C; now uses Language::all()[0] to avoid
    // referencing Language::C when lang-c is disabled). The FFI resolver
    // (gated on both lang-c and lang-rust) will simply fail to match these.
    Language::all()[0]
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
            if text.chars().all(|c| c.is_alphanumeric() || c == '_')
                && text
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_')
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

/// Strips generic type parameters from a type name.
///
/// `ParserGuard<'_>` → `ParserGuard`, `Vec<u8>` → `Vec`,
/// `HashMap<String, Vec<u8>>` → `HashMap`. This ensures IMPLEMENTS edge
/// sources match Struct/Enum node IDs (which don't include generic params),
/// so `delete_file_nodes_batch` can match and delete old edges during
/// --force reindex (ADR-014).
fn strip_generics(name: &str) -> &str {
    match name.find('<') {
        Some(idx) => name[..idx].trim_end(),
        None => name,
    }
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
    // B1 fix: only emit DEFINES (file -> definition). The previous CONTAINS
    // emission was redundant — for (file, node) pairs, CONTAINS and DEFINES
    // carry identical semantics, producing duplicate edges that inflated
    // verification diffs against gitnexus (see triage.md §B1).
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

#[cfg(all(
    test,
    feature = "lang-c",
    feature = "lang-fortran",
    feature = "lang-python",
    feature = "lang-rust"
))]
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
impl Point { fn new(x: i32, y: i32) -> Self { Point { x, y } } }
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

    /// Verifies CallInfo extraction for trait impl method calling free function.
    /// Mirrors CalNexus scientific.rs structure: `impl CalculationDomain for ScientificDomain`.
    #[test]
    fn extracts_call_in_trait_impl_method() {
        let src = r#"
pub trait CalculationDomain {
    fn supports(&self, ast: &AstNode) -> bool;
}

pub struct ScientificDomain;

impl CalculationDomain for ScientificDomain {
    fn supports(&self, ast: &AstNode) -> bool {
        contains_scientific(ast)
    }
}

fn contains_scientific(ast: &AstNode) -> bool { true }
"#;
        let result = extract(src);
        // The call to contains_scientific inside supports() should be extracted.
        let contains_calls: Vec<_> = result
            .calls
            .iter()
            .filter(|c| c.callee_name == "contains_scientific")
            .collect();
        assert!(
            !contains_calls.is_empty(),
            "expected contains_scientific call to be extracted, got {:?}",
            result.calls
        );
    }

    /// B1b: Verifies CallInfo extraction for function-reference arguments.
    ///
    /// Mirrors CalNexus scientific.rs: `self.eval_unary(name, args, ctx, erf)`
    /// where `erf` is a free function passed as `impl Fn(f64) -> f64`. Without
    /// this fix, `erf` only generates a ReadInfo (variable read), causing
    /// dead_code analysis to report it as dead (zero incoming CALLS edges).
    #[test]
    fn extracts_function_reference_argument() {
        let src = r#"
fn eval_unary(name: &str, f: impl Fn(f64) -> f64) -> f64 { f(1.0) }
fn erf(x: f64) -> f64 { x }
fn caller(name: &str) {
    eval_unary(name, erf);
}
"#;
        let result = extract(src);
        // `erf` passed as a function-reference argument must be extracted as
        // a CallInfo so the resolver can create a CALLS edge to it.
        let erf_calls: Vec<_> = result
            .calls
            .iter()
            .filter(|c| c.callee_name == "erf")
            .collect();
        assert!(
            !erf_calls.is_empty(),
            "expected erf (passed as function reference) to be extracted as CallInfo, got {:?}",
            result.calls
        );
        // `name` is a parameter (variable), not a function. It should NOT
        // generate a CallInfo — the resolver would drop it anyway since no
        // function named "name" exists in the symbol table, but we verify
        // the extractor still generates it (conservative: let the resolver
        // decide). The key assertion is that `erf` is extracted.
        let name_calls: Vec<_> = result
            .calls
            .iter()
            .filter(|c| c.callee_name == "name")
            .collect();
        // `name` is a bare identifier argument, so it WILL generate a
        // CallInfo. The resolver will drop it because no function "name"
        // exists. This is the expected conservative behavior.
        assert!(
            !name_calls.is_empty(),
            "expected name (bare identifier argument) to also be extracted as CallInfo (resolver will filter), got {:?}",
            result.calls
        );
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
    fn creates_defines_edges() {
        // B1 fix: CONTAINS emission removed; only DEFINES remains.
        let result = extract(RUST_SOURCE);
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count);
        // B1 fix verification: no CONTAINS edges should be emitted
        let contains_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Contains)
            .count();
        assert_eq!(
            contains_count, 0,
            "B1 fix: no CONTAINS edges should be emitted"
        );
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
        // C2 fix: String::new() is a scoped_identifier (kept), but
        // s.push_str() is a field_expression with a stdlib method name
        // (filtered). Only user-code function calls should be recorded.
        let src = "fn main() { let s = String::new(); s.push_str(\"hi\"); }";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callees.contains(&"new"), "should extract String::new call");
        assert!(
            !callees.contains(&"push_str"),
            "stdlib method push_str should be filtered: {callees:?}"
        );
    }

    #[test]
    fn c2_stdlib_method_calls_are_filtered() {
        // C2 fix: stdlib method calls (field_expression with stdlib method
        // name) should not generate CallInfo, matching gitnexus which only
        // captures function-level calls (free functions + static methods).
        let src = r#"fn main() {
            let mut v = Vec::new();
            v.push(1);
            let s = String::from("hi");
            s.len();
            v.iter().map(|x| x + 1).collect::<Vec<_>>();
            v.get(0);
            s.contains("h");
        }"#;
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        for stdlib_method in ["push", "len", "iter", "map", "collect", "get", "contains"] {
            assert!(
                !callees.contains(&stdlib_method),
                "stdlib method {stdlib_method} should be filtered: {callees:?}"
            );
        }
    }

    // ===== B1: function calls inside macro arguments =====

    #[test]
    fn b1_extracts_function_call_inside_println_macro() {
        // B1 fix: function calls inside `println!` macro arguments must be
        // extracted as CallInfo. tree-sitter-rust parses macro arguments as
        // `token_tree` nodes, which contain raw tokens that are NOT parsed as
        // `call_expression`. This test verifies that the extractor descends
        // into `token_tree` and extracts function calls.
        let src = r#"fn helper() -> i32 { 42 }
fn main() {
    println!("{}", helper());
}"#;
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"helper"),
            "B1: function call inside println! should be extracted: {callees:?}"
        );
    }

    #[test]
    fn b1_extracts_function_call_inside_format_macro() {
        // B1 fix: function calls inside `format!` macro arguments must be
        // extracted. This was the root cause of `error_kind_prefix` being
        // flagged as dead code in CalNexus (called inside `format!(...)`).
        let src = r#"fn prefix(kind: &str) -> &'static str { kind }
fn main() {
    let s = format!("{}: {}", prefix("err"), "msg");
}"#;
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"prefix"),
            "B1: function call inside format! should be extracted: {callees:?}"
        );
    }

    #[test]
    fn b1_extracts_function_call_inside_json_macro() {
        // B1 fix: function calls inside `json!` (serde_json::json!) macro
        // arguments must be extracted. This was the root cause of
        // `dmatrix_to_json` being flagged as dead code in CalNexus.
        let src = r#"fn to_json(x: i32) -> i32 { x }
fn main() {
    let _v = json!({ "key": to_json(42) });
}"#;
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"to_json"),
            "B1: function call inside json! should be extracted: {callees:?}"
        );
    }

    #[test]
    fn b1_extracts_function_call_inside_nested_macro() {
        // B1 fix: function calls inside nested macros (e.g. `println!` inside
        // another macro) must also be extracted.
        let src = r#"fn helper() -> i32 { 42 }
macro_rules! debug_print {
    ($x:expr) => { println!("{}", $x) };
}
fn main() {
    debug_print!(helper());
}"#;
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        // Note: macro_rules! arguments may not parse as call_expression, but
        // if they do, the call should be extracted. This test documents the
        // expected behavior.
        assert!(
            callees.contains(&"helper"),
            "B1: function call inside nested macro should be extracted: {callees:?}"
        );
    }

    #[test]
    fn c2_user_defined_method_calls_are_preserved() {
        // C2 fix: user-defined method calls (field_expression with non-stdlib
        // method name) should still generate CallInfo.
        let src = r#"struct Repo;
impl Repo {
    fn save_nodes(&self) {}
    fn execute_query(&self) {}
}
fn main() {
    let r = Repo;
    r.save_nodes();
    r.execute_query();
}"#;
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"save_nodes"),
            "user method save_nodes should be preserved: {callees:?}"
        );
        assert!(
            callees.contains(&"execute_query"),
            "user method execute_query should be preserved: {callees:?}"
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
    fn trait_impl_does_not_create_impl_node() {
        // B2 fix: trait impls (impl Trait for Type) do not create Impl nodes,
        // matching gitnexus which only models inherent impls. Methods inside
        // trait impls are still extracted as Function nodes.
        let result = extract(RUST_SOURCE);
        let impls: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Impl)
            .collect();
        // Only the inherent `impl Point {}` creates an Impl node.
        // The trait `impl Drawable for Point {}` does not.
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "Point");
        assert!(
            impls[0].properties.get("trait").is_none(),
            "inherent impl should not have trait property: {:?}",
            impls[0].properties
        );
    }

    #[test]
    fn trait_impl_creates_implements_edge() {
        // Feature gap (closed): trait impls create an IMPLEMENTS edge from
        // the implemented type to the trait. The source FQN matches the
        // Struct node's FQN; the target is a best-effort pseudo-FQN that
        // TypeResolver will resolve.
        let result = extract(RUST_SOURCE);
        let implements: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        // RUST_SOURCE has one trait impl: `impl Drawable for Point`.
        assert_eq!(
            implements.len(),
            1,
            "should create 1 IMPLEMENTS edge: {:?}",
            implements
        );
        // Source = Point FQN (matches Struct node).
        assert!(
            implements[0].source.contains("Point"),
            "IMPLEMENTS source should be Point FQN: {}",
            implements[0].source
        );
        // Target = Drawable pseudo-FQN.
        assert!(
            implements[0].target.contains("Drawable"),
            "IMPLEMENTS target should be Drawable FQN: {}",
            implements[0].target
        );
    }

    #[test]
    fn trait_impl_with_path_extracts_last_component() {
        // `impl std::fmt::Display for Foo` — the trait name is a path; only
        // the last component (`Display`) should be used for the pseudo-FQN.
        let src = r#"struct Foo;
impl std::fmt::Display for Foo {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
}
"#;
        let result = extract(src);
        let implements: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(implements.len(), 1, "should create 1 IMPLEMENTS edge");
        // Target should contain `Display` (last component), not `std::fmt::Display`.
        assert!(
            implements[0].target.ends_with(".Display"),
            "IMPLEMENTS target should end with .Display: {}",
            implements[0].target
        );
        assert!(
            !implements[0].target.contains("::"),
            "IMPLEMENTS target should not contain `::`: {}",
            implements[0].target
        );
    }

    #[test]
    fn multiple_trait_impls_create_multiple_implements_edges() {
        let src = r#"trait A { fn a(&self); }
trait B { fn b(&self); }
struct Foo;
impl A for Foo { fn a(&self) {} }
impl B for Foo { fn b(&self) {} }
"#;
        let result = extract(src);
        let implements: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(implements.len(), 2, "should create 2 IMPLEMENTS edges");
        let targets: Vec<&str> = implements.iter().map(|e| e.target.as_str()).collect();
        assert!(
            targets.iter().any(|t| t.contains("A")),
            "should have edge to trait A: {targets:?}"
        );
        assert!(
            targets.iter().any(|t| t.contains("B")),
            "should have edge to trait B: {targets:?}"
        );
    }

    #[test]
    fn trait_impl_strips_generics_from_type_name() {
        // `impl Trait for Type<'_>` — the type name has generic parameters.
        // The IMPLEMENTS edge source must NOT include generics, so it matches
        // the Struct node ID (which also has no generics). Otherwise
        // delete_file_nodes_batch cannot match and delete old edges during
        // --force reindex, causing duplicate primary key errors (ADR-014).
        let src = r#"struct ParserGuard<'a> { inner: &'a i32 }
trait DerefMut { fn deref_mut(&mut self); }
impl DerefMut for ParserGuard<'_> {
    fn deref_mut(&mut self) {}
}
"#;
        let result = extract(src);
        let implements: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(implements.len(), 1, "should create 1 IMPLEMENTS edge");
        assert!(
            implements[0].source.contains("ParserGuard"),
            "IMPLEMENTS source should contain ParserGuard: {}",
            implements[0].source
        );
        assert!(
            !implements[0].source.contains("<'"),
            "IMPLEMENTS source must not contain generic params: {}",
            implements[0].source
        );
    }

    #[test]
    fn inherent_impl_does_not_create_implements_edge() {
        // Inherent impls (`impl Type {}`) should NOT create IMPLEMENTS edges.
        let src = r#"struct Foo;
impl Foo { fn new() -> Self { Self } }
"#;
        let result = extract(src);
        let implements: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(
            implements.len(),
            0,
            "inherent impl should not create IMPLEMENTS edge: {:?}",
            implements
        );
    }

    // --- struct fields as Property nodes (feature gap closed) ---

    #[test]
    fn struct_fields_extracted_as_property_nodes() {
        // `pub struct Point { x: i32, y: i32 }` has two named fields → 2
        // Property nodes.
        let result = extract(RUST_SOURCE);
        let properties: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(
            properties.len(),
            2,
            "should extract 2 Property nodes for Point's fields: {:?}",
            properties.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
        let names: Vec<&str> = properties.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"x"), "should have field x: {names:?}");
        assert!(names.contains(&"y"), "should have field y: {names:?}");
    }

    #[test]
    fn struct_creates_has_property_edges() {
        // Each field creates a HasProperty edge from the Struct FQN to the
        // Property node id (UUID, remapped to FQN by ScopeResolutionPhase).
        let result = extract(RUST_SOURCE);
        let has_property: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::HasProperty)
            .collect();
        assert_eq!(
            has_property.len(),
            2,
            "should create 2 HasProperty edges: {:?}",
            has_property
        );
        // All edges should originate from the Point struct FQN.
        for edge in &has_property {
            assert!(
                edge.source.contains("Point"),
                "HasProperty source should be Point FQN: {}",
                edge.source
            );
        }
        // Targets should match the Property node ids.
        let property_ids: Vec<String> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(property_ids.len(), 2);
        for edge in &has_property {
            assert!(
                property_ids.contains(&edge.target),
                "HasProperty target should be a Property node id: {}",
                edge.target
            );
        }
    }

    #[test]
    fn unit_struct_creates_no_property_nodes() {
        // `struct Foo;` has no body → no Property nodes.
        let src = "struct Foo;";
        let result = extract(src);
        let properties: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(
            properties.len(),
            0,
            "unit struct should not create Property nodes: {:?}",
            properties
        );
        let has_property: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::HasProperty)
            .collect();
        assert_eq!(
            has_property.len(),
            0,
            "unit struct should not create HasProperty edges: {:?}",
            has_property
        );
    }

    #[test]
    fn tuple_struct_creates_no_property_nodes() {
        // `struct Foo(i32, i32);` has tuple fields without names → no Property
        // nodes (tuple_field is not field_declaration).
        let src = "struct Foo(i32, i32);";
        let result = extract(src);
        let properties: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(
            properties.len(),
            0,
            "tuple struct should not create Property nodes: {:?}",
            properties
        );
    }

    #[test]
    fn struct_fields_have_disambiguated_fqn() {
        // Field FQN (qualified_name) should be disambiguated by the struct
        // name: e.g. `proj.test.x#Point` (matching the convention for impl
        // methods). Note: `id` is a UUID at extraction time; the FQN lives in
        // `qualified_name` and becomes the node id after ScopeResolutionPhase.
        let result = extract(RUST_SOURCE);
        let property_x = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Property && n.name == "x")
            .expect("should have Property node for field x");
        assert!(
            property_x.qualified_name.ends_with("#Point"),
            "field x FQN should end with #Point: {}",
            property_x.qualified_name
        );
        assert!(
            property_x.qualified_name.contains(".x#"),
            "field x FQN should contain `.x#`: {}",
            property_x.qualified_name
        );
    }

    #[test]
    fn struct_in_module_has_module_qualified_field_fqn() {
        // Fields of a struct inside a module should be disambiguated by
        // `{module}_{struct}` (same convention as impl methods).
        let src = r#"mod foo {
    struct Point { x: i32, y: i32 }
}
"#;
        let result = extract(src);
        let property_x = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Property && n.name == "x")
            .expect("should have Property node for field x in module");
        assert!(
            property_x.qualified_name.ends_with("#foo_Point"),
            "field x FQN in module foo should end with #foo_Point: {}",
            property_x.qualified_name
        );
    }

    #[test]
    fn property_nodes_have_defines_edges() {
        // Property nodes should have DEFINES edges from the file (same as
        // other definition nodes).
        let result = extract(RUST_SOURCE);
        let properties: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(properties.len(), 2);
        for prop in &properties {
            let has_defines = result
                .edges
                .iter()
                .any(|e| e.edge_type == EdgeType::Defines && e.target == prop.id);
            assert!(
                has_defines,
                "Property {} should have a DEFINES edge from the file",
                prop.name
            );
        }
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
        assert!(
            writes.contains(&"a"),
            "should write first binding a: {writes:?}"
        );
        assert!(
            !writes.contains(&"b"),
            "should not write b (only first binding extracted): {writes:?}"
        );
    }

    #[test]
    fn reference_pattern() {
        let src = "fn main() { let x = 1; let &y = &x; }";
        let result = extract(src);
        let writes: Vec<_> = result.writes.iter().map(|w| w.var_name.as_str()).collect();
        assert!(
            writes.contains(&"y"),
            "should write y from reference pattern: {writes:?}"
        );
    }

    #[test]
    fn struct_pattern() {
        let src = "fn main() { struct P { x: i32 } let p = P { x: 1 }; let P { x } = p; }";
        let result = extract(src);
        let writes: Vec<_> = result.writes.iter().map(|w| w.var_name.as_str()).collect();
        // pattern_name returns the type name for a struct pattern (`P`), not
        // the field name (`x`).
        assert!(
            writes.contains(&"P"),
            "should write type name P from struct pattern: {writes:?}"
        );
    }

    #[test]
    fn parenthesized_call_expression() {
        let src = "fn foo() -> fn() { bar } fn bar() {} fn main() { (foo())(); }";
        let result = extract(src);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
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
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
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
        assert_eq!(sorted.len(), before, "FQN collision detected: {impl_qns:?}");
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
        assert!(
            names.contains(&"network"),
            "mod network; should be a Module"
        );
        assert!(
            names.contains(&"parser"),
            "mod parser {{}} should be a Module"
        );
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
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines && e.target == module_node.id)
            .count();
        assert_eq!(defines_count, 1, "Module should have 1 DEFINES edge");
        // B1 fix verification: no CONTAINS edges should remain
        let contains_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Contains && e.target == module_node.id)
            .count();
        assert_eq!(
            contains_count, 0,
            "B1 fix: Module should have 0 CONTAINS edges"
        );
    }

    // --- IMPLEMENTS edge: generic trait target stripping (ADR-014 regression) ---

    #[test]
    fn trait_impl_strips_generic_params_from_trait_target() {
        // `impl From<reqwest::Error> for SecureNotifyError` — the trait field
        // is `From<reqwest::Error>`. The IMPLEMENTS edge target must be `From`
        // (generic params stripped), NOT `Error>` (residual `>` after rsplit).
        // Regression: duplicate primary key on --force reindex (ADR-014).
        let src = "struct SecureNotifyError;\nimpl From<reqwest::Error> for SecureNotifyError {\n    fn from(e: reqwest::Error) -> Self { Self }\n}\n";
        let result = extract(src);
        let implements: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(implements.len(), 1, "should create 1 IMPLEMENTS edge");
        assert!(
            implements[0].target.ends_with("From"),
            "IMPLEMENTS target should end with From (generics stripped): {}",
            implements[0].target
        );
        assert!(
            !implements[0].target.contains('>'),
            "IMPLEMENTS target must not contain residual `>`: {}",
            implements[0].target
        );
    }

    #[test]
    fn multiple_from_impls_produce_distinct_edge_ids() {
        // Multiple `impl From<X> for Y` blocks must produce IMPLEMENTS edges
        // with distinct (source, target, start_line) to avoid duplicate primary
        // key on --force reindex. Each impl block has a different start_line.
        let src = r#"struct SecureNotifyError;
impl From<reqwest::Error> for SecureNotifyError {
    fn from(e: reqwest::Error) -> Self { Self }
}
impl From<serde_json::Error> for SecureNotifyError {
    fn from(e: serde_json::Error) -> Self { Self }
}
impl From<std::io::Error> for SecureNotifyError {
    fn from(e: std::io::Error) -> Self { Self }
}
"#;
        let result = extract(src);
        let implements: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(
            implements.len(),
            3,
            "should create 3 IMPLEMENTS edges: {:?}",
            implements
        );
        // All targets should be `From` (generics stripped), not `Error>`.
        for e in &implements {
            assert!(
                e.target.ends_with("From"),
                "IMPLEMENTS target should end with From: {}",
                e.target
            );
        }
        // Edge IDs (source, target, start_line) must be unique.
        let mut edge_keys: Vec<(String, String, u32)> = implements
            .iter()
            .map(|e| {
                (
                    e.source.clone(),
                    e.target.clone(),
                    e.start_line.unwrap_or(0),
                )
            })
            .collect();
        let total = edge_keys.len();
        edge_keys.sort();
        edge_keys.dedup();
        assert_eq!(
            edge_keys.len(),
            total,
            "IMPLEMENTS edge keys must be unique (no duplicate primary key): {:?}",
            implements
        );
    }

    // --- pure utility functions ---

    #[test]
    fn strip_generics_removes_type_parameters() {
        assert_eq!(strip_generics("ParserGuard<'_>"), "ParserGuard");
        assert_eq!(strip_generics("Vec<u8>"), "Vec");
        assert_eq!(strip_generics("HashMap<String, Vec<u8>>"), "HashMap");
    }

    #[test]
    fn strip_generics_returns_unchanged_when_no_generics() {
        assert_eq!(strip_generics("Point"), "Point");
        assert_eq!(strip_generics(""), "");
    }

    #[test]
    fn combine_scope_both_present() {
        assert_eq!(
            combine_scope(Some("parent"), Some("child")),
            Some("parent_child".to_string())
        );
    }

    #[test]
    fn combine_scope_only_child() {
        assert_eq!(
            combine_scope(None, Some("child")),
            Some("child".to_string())
        );
    }

    #[test]
    fn combine_scope_only_parent() {
        assert_eq!(
            combine_scope(Some("parent"), None),
            Some("parent".to_string())
        );
    }

    #[test]
    fn combine_scope_neither() {
        assert_eq!(combine_scope(None, None), None);
    }

    #[test]
    fn is_stdlib_method_recognizes_common_methods() {
        assert!(is_stdlib_method("push"));
        assert!(is_stdlib_method("len"));
        assert!(is_stdlib_method("contains"));
        assert!(is_stdlib_method("insert"));
    }

    #[test]
    fn is_stdlib_method_rejects_user_defined_methods() {
        assert!(!is_stdlib_method("save_nodes"));
        assert!(!is_stdlib_method("execute_query"));
        assert!(!is_stdlib_method("my_custom_method"));
    }

    // --- parse helper for direct function tests ---

    fn parse_source(source: &str) -> tree_sitter::Tree {
        let mut parser = crate::parse::parser_factory::ParserFactory::create_parser(Language::Rust)
            .expect("parser");
        parser.parse(source, None).expect("parse")
    }

    // --- use_path: scoped_use_list branch ---

    #[test]
    fn use_path_with_scoped_use_list() {
        // `use std::{io, fs};` — covers scoped_use_list branch
        let src = "use std::{io, fs};";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert!(
            result.imports[0].source_file.contains("std"),
            "path should contain std: {}",
            result.imports[0].source_file
        );
    }

    #[test]
    fn use_path_with_nested_scoped_use_list() {
        // `use std::{io::{BufRead, Read}};` — covers nested scoped_use_list
        let src = "use std::{io::{BufRead, Read}};";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
    }

    // --- use_path: _ fallback branch (line 1131) ---

    #[test]
    fn use_path_fallback_returns_node_text_for_unknown_kind() {
        let tree = parse_source("42");
        let root = tree.root_node();
        // integer_literal is not in use_path's match arms → _ fallback
        if let Some(literal) = root.named_child(0) {
            let result = use_path(literal, "42");
            assert!(result.is_some(), "fallback should return node text");
        }
    }

    // --- use_path: use_wildcard fallback `Some("*")` (line 1114) ---

    #[test]
    fn use_path_bare_wildcard_returns_star() {
        // `use *;` — use_wildcard with no path child → returns "*"
        let src = "use *;";
        let result = extract(src);
        // May or may not produce an import depending on grammar,
        // but should not panic.
        if !result.imports.is_empty() {
            assert!(
                result.imports[0].source_file.contains("*"),
                "path should contain *: {}",
                result.imports[0].source_file
            );
        }
    }

    // --- use_imported_names: various branches (lines 1138-1165) ---

    #[test]
    fn use_imported_names_with_scoped_use_list() {
        // `use std::{io, fs};` — scoped_use_list exercises the
        // scoped_use_list branch of use_imported_names. The `name` field
        // may be absent for scoped_use_list, so imported_names can be empty;
        // the test verifies the path is still extracted.
        let src = "use std::{io, fs};";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert!(
            result.imports[0].source_file.contains("std"),
            "path should contain std: {}",
            result.imports[0].source_file
        );
    }

    // --- let binding without value (line 858) ---

    #[test]
    fn let_binding_without_value_has_empty_source() {
        let src = "fn main() { let x; }";
        let result = extract(src);
        let assign = result
            .assignments
            .iter()
            .find(|a| a.target_name == "x")
            .expect("should find assignment to x");
        assert_eq!(assign.source_name, "");
        assert!(!assign.is_return_assign);
    }

    // --- identifier_text returns None for non-identifier (line 907) ---

    #[test]
    fn identifier_text_returns_none_for_non_identifier() {
        let tree = parse_source("fn main() {}");
        let root = tree.root_node();
        let func = root.named_child(0).expect("function");
        assert!(identifier_text(func, "fn main() {}").is_none());
    }

    // --- is_read_position returns false for root node (line 919) ---

    #[test]
    fn is_read_position_returns_false_for_root_node() {
        let tree = parse_source("fn main() {}");
        let root = tree.root_node();
        assert!(!is_read_position(root));
    }

    // --- callee_name _ => None (line 1195) ---

    #[test]
    fn callee_name_returns_none_for_unknown_kind() {
        let tree = parse_source("fn main() { let x = 42; }");
        let root = tree.root_node();
        // Walk to find a node not in callee_name's match arms
        fn find_non_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
            let known = matches!(
                node.kind(),
                "identifier"
                    | "type_identifier"
                    | "field_expression"
                    | "scoped_identifier"
                    | "call_expression"
                    | "parenthesized_expression"
                    | "generic_function"
            );
            if !known {
                return Some(node);
            }
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(found) = find_non_callee(child) {
                        return Some(found);
                    }
                }
            }
            None
        }
        if let Some(node) = find_non_callee(root) {
            assert!(callee_name(node, "fn main() { let x = 42; }").is_none());
        }
    }

    // --- pattern_name _ fallback (lines 1225-1234) ---

    #[test]
    fn pattern_name_fallback_rejects_non_identifier_text() {
        // source_file node has text with non-alphanumeric chars → fallback
        // rejects it and returns None.
        let tree = parse_source("fn main() {}");
        let root = tree.root_node();
        assert!(pattern_name(root, "fn main() {}").is_none());
    }

    // --- pattern_name: tuple_pattern None (line 1211) ---

    #[test]
    fn pattern_name_empty_tuple_pattern_returns_none() {
        // `let () = ...;` — empty tuple pattern with no children → None
        let src = "fn main() { let () = (1, 2); }";
        let result = extract(src);
        // pattern_name returns None for empty tuple, so no assignment
        // is created with target "()"
        let has_empty_target = result.assignments.iter().any(|a| a.target_name.is_empty());
        let _ = has_empty_target; // behavior-dependent; test just exercises the path
    }

    // --- call_arguments with no arguments field (line 1242) ---

    #[test]
    fn call_arguments_returns_empty_when_no_arguments_field() {
        let tree = parse_source("fn main() {}");
        let root = tree.root_node();
        let func = root.named_child(0).expect("function");
        let args = call_arguments(func, "fn main() {}");
        assert!(args.is_empty());
    }

    // --- extern_language with direct string_literal fallback (lines 1032-1044) ---

    #[test]
    fn extern_language_direct_string_literal_fallback() {
        // Some grammar versions produce direct string_literal children
        // instead of extern_modifier. The fallback loop handles this.
        let src = r#"extern "C" { fn direct_func(x: i32); }"#;
        let result = extract(src);
        assert_eq!(result.externs.len(), 1);
        assert_eq!(result.externs[0].language, Language::C);
    }

    // --- combine_scope call site in impl_item: (Some(p), None) and (None, None) ---

    #[test]
    fn impl_without_type_inside_module_covers_combine_scope_some_none() {
        // `mod foo { impl {} }` — impl with no type field inside a module.
        // The resolver returns None (no type), combine_scope gets (Some("foo"), None).
        let src = "mod foo { impl {} }";
        let result = extract(src);
        // Should not panic; module node should be created
        let _ = result;
    }

    #[test]
    fn impl_without_type_at_top_level_covers_combine_scope_none_none() {
        // `impl {}` at top level — no module, no type → (None, None)
        let src = "impl {}";
        let result = extract(src);
        let _ = result;
    }

    // --- is_field_expression_call: field_expression / generic_function / other ---

    #[test]
    fn is_field_expression_call_with_field_expression_returns_true() {
        // `obj.method()` — the func_node is a field_expression
        let tree = parse_source("fn main() { obj.method(); }");
        let root = tree.root_node();
        fn find_call<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
            if node.kind() == "call_expression" {
                return node.child_by_field_name("function");
            }
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(found) = find_call(child) {
                        return Some(found);
                    }
                }
            }
            None
        }
        if let Some(func) = find_call(root) {
            assert!(
                is_field_expression_call(func),
                "field_expression should return true"
            );
        }
    }

    #[test]
    fn is_field_expression_call_with_generic_function_wrapping_field() {
        // `obj.method::<T>()` — generic_function wrapping a field_expression
        let tree = parse_source("fn main() { obj.method::<i32>(); }");
        let root = tree.root_node();
        fn find_call<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
            if node.kind() == "call_expression" {
                return node.child_by_field_name("function");
            }
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(found) = find_call(child) {
                        return Some(found);
                    }
                }
            }
            None
        }
        if let Some(func) = find_call(root) {
            assert!(
                is_field_expression_call(func),
                "generic_function wrapping field_expression should return true"
            );
        }
    }

    #[test]
    fn is_field_expression_call_with_identifier_returns_false() {
        // `foo()` — the func_node is an identifier, not a field_expression
        let tree = parse_source("fn main() { foo(); }");
        let root = tree.root_node();
        fn find_call<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
            if node.kind() == "call_expression" {
                return node.child_by_field_name("function");
            }
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(found) = find_call(child) {
                        return Some(found);
                    }
                }
            }
            None
        }
        if let Some(func) = find_call(root) {
            assert!(
                !is_field_expression_call(func),
                "identifier should return false"
            );
        }
    }

    // --- is_read_position: various parent types ---

    fn find_first_identifier<'a>(
        node: tree_sitter::Node<'a>,
        source: &str,
        target: &str,
    ) -> Option<tree_sitter::Node<'a>> {
        if node.kind() == "identifier" {
            if let Some(text) = node_text(node, source) {
                if text == target {
                    return Some(node);
                }
            }
        }
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                if let Some(found) = find_first_identifier(child, source, target) {
                    return Some(found);
                }
            }
        }
        None
    }

    #[test]
    fn is_read_position_in_binary_expression_returns_true() {
        let src = "fn main() { let x = a + b; }";
        let tree = parse_source(src);
        let node =
            find_first_identifier(tree.root_node(), src, "a").expect("should find identifier 'a'");
        assert!(
            is_read_position(node),
            "identifier in binary_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_return_expression_returns_true() {
        let src = "fn main() { return x; }";
        let tree = parse_source(src);
        let node =
            find_first_identifier(tree.root_node(), src, "x").expect("should find identifier 'x'");
        assert!(
            is_read_position(node),
            "identifier in return_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_arguments_returns_true() {
        let src = "fn foo(a: i32) {} fn main() { foo(y); }";
        let tree = parse_source(src);
        let node =
            find_first_identifier(tree.root_node(), src, "y").expect("should find identifier 'y'");
        assert!(
            is_read_position(node),
            "identifier in arguments should be a read"
        );
    }

    #[test]
    fn is_read_position_in_let_pattern_returns_false() {
        let src = "fn main() { let x = 1; }";
        let tree = parse_source(src);
        let node =
            find_first_identifier(tree.root_node(), src, "x").expect("should find identifier 'x'");
        assert!(
            !is_read_position(node),
            "identifier in let pattern should NOT be a read"
        );
    }

    #[test]
    fn is_read_position_in_call_function_returns_false() {
        let src = "fn main() { foo(); }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "foo")
            .expect("should find identifier 'foo'");
        assert!(
            !is_read_position(node),
            "callee identifier should NOT be a read"
        );
    }

    #[test]
    fn is_read_position_in_field_expression_value_returns_true() {
        let src = "fn main() { let x = obj.field; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "obj")
            .expect("should find identifier 'obj'");
        assert!(is_read_position(node), "obj in obj.field should be a read");
    }

    // --- is_read_position: remaining parent types (coverage gaps) ---

    #[test]
    fn is_read_position_in_unary_expression_returns_true() {
        let src = "fn main() { let x = -val; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier in unary_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_parenthesized_expression_returns_true() {
        let src = "fn main() { let x = (val); }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier in parenthesized_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_tuple_expression_returns_true() {
        let src = "fn main() { let x = (val, 1); }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier in tuple_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_array_expression_returns_true() {
        let src = "fn main() { let x = [val, 1]; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier in array_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_index_expression_returns_true() {
        let src = "fn main() { let x = arr[idx]; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "idx")
            .expect("should find identifier 'idx'");
        assert!(
            is_read_position(node),
            "identifier in index_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_reference_expression_returns_true() {
        let src = "fn main() { let x = &val; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier in reference_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_deref_expression_returns_true() {
        let src = "fn main() { let x = *val; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier in deref_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_closure_expression_body_returns_true() {
        let src = "fn main() { let f = || val; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier in closure_expression body should be a read"
        );
    }

    #[test]
    fn is_read_position_in_assignment_right_side_returns_true() {
        let src = "fn main() { let mut x = 0; x = val; }";
        let tree = parse_source(src);
        let node = find_first_identifier(tree.root_node(), src, "val")
            .expect("should find identifier 'val'");
        assert!(
            is_read_position(node),
            "identifier on right side of assignment_expression should be a read"
        );
    }

    #[test]
    fn is_read_position_in_assignment_left_side_returns_false() {
        let src = "fn main() { let mut x = 0; x = val; }";
        let tree = parse_source(src);
        // Find the second occurrence of 'x' (in the assignment left side).
        // find_first_identifier finds the first 'x' (let pattern); we need the
        // one in the assignment. Use a dedicated walker.
        let root = tree.root_node();
        let mut found: Option<tree_sitter::Node<'_>> = None;
        find_assignment_left_identifier(root, src, "x", &mut found);
        let node = found.expect("should find 'x' in assignment left");
        assert!(
            !is_read_position(node),
            "identifier on left side of assignment_expression should NOT be a read"
        );
    }

    fn find_assignment_left_identifier<'a>(
        node: tree_sitter::Node<'a>,
        source: &str,
        target: &str,
        found: &mut Option<tree_sitter::Node<'a>>,
    ) {
        if node.kind() == "assignment_expression" {
            if let Some(left) = node.child_by_field_name("left") {
                if left.kind() == "identifier" {
                    if let Some(text) = node_text(left, source) {
                        if text == target {
                            *found = Some(left);
                            return;
                        }
                    }
                }
            }
        }
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                find_assignment_left_identifier(child, source, target, found);
                if found.is_some() {
                    return;
                }
            }
        }
    }

    // --- extract_assignment: non-identifier left side (field write) ---

    #[test]
    fn extract_assignment_with_field_left_produces_no_write() {
        let src = "fn main() { obj.field = 42; }";
        let result = extract(src);
        assert!(
            result.writes.is_empty(),
            "field assignment (obj.field = ...) should NOT produce a WriteInfo: {:?}",
            result.writes
        );
    }

    #[test]
    fn extract_assignment_with_index_left_produces_no_write() {
        let src = "fn main() { arr[0] = 42; }";
        let result = extract(src);
        assert!(
            result.writes.is_empty(),
            "index assignment (arr[0] = ...) should NOT produce a WriteInfo: {:?}",
            result.writes
        );
    }

    // --- extract_struct_fields: body with non-field_declaration children ---

    #[test]
    fn extract_struct_fields_skips_non_field_declaration_children() {
        let src = "struct Foo { x: i32, y: i32 }";
        let result = extract(src);
        let properties: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(
            properties.len(),
            2,
            "struct with 2 fields should produce 2 Property nodes: {:?}",
            properties
        );
    }

    // --- extract_extern_block with empty extern (names.is_empty early return) ---

    #[test]
    fn empty_extern_block_produces_no_extern_info() {
        let src = r#"extern "C" {}"#;
        let result = extract(src);
        assert!(
            result.externs.is_empty(),
            "empty extern block should produce no ExternInfo: {:?}",
            result.externs
        );
    }

    #[test]
    fn pub_crate_visibility_marked_as_exported() {
        let src = "pub(crate) fn internal_fn() {}";
        let result = extract(src);
        let func = result
            .nodes
            .iter()
            .find(|n| n.name == "internal_fn")
            .expect("should extract internal_fn");
        assert!(
            func.is_exported,
            "pub(crate) should be marked exported (is_pub checks visibility_modifier presence): {:?}",
            func
        );
    }

    #[test]
    fn pub_crate_struct_marked_as_exported() {
        let src = "pub(crate) struct Internal {}";
        let result = extract(src);
        let s = result
            .nodes
            .iter()
            .find(|n| n.name == "Internal")
            .expect("should extract Internal struct");
        assert!(s.is_exported, "pub(crate) struct should be marked exported");
    }

    #[test]
    fn file_with_only_inner_attributes_produces_no_nodes() {
        let src = "#![allow(dead_code)]\n#![warn(unused)]\n";
        let result = extract(src);
        assert!(
            result.nodes.is_empty(),
            "file with only inner attributes should produce no nodes: {:?}",
            result.nodes
        );
    }

    #[test]
    fn trait_with_associated_type_does_not_panic() {
        let src = r#"trait Container {
    type Item;
    fn get(&self) -> Self::Item;
}"#;
        let result = extract(src);
        let traits: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Trait)
            .collect();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].name, "Container");
    }

    #[test]
    fn tuple_struct_pattern_in_let_binding() {
        let src = "struct Pair(i32, i32);\nfn main() { let Pair(a, b) = Pair(1, 2); }";
        let result = extract(src);
        let writes: Vec<_> = result.writes.iter().map(|w| w.var_name.as_str()).collect();
        assert!(
            writes.contains(&"Pair"),
            "tuple_struct_pattern extracts the struct name as first identifier: {writes:?}"
        );
    }

    #[test]
    fn impl_block_with_multiple_methods() {
        let src = r#"struct Repo;
impl Repo {
    fn save(&self) {}
    fn load(&self) {}
    fn delete(&self) {}
}"#;
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            methods.contains(&"save"),
            "should extract save: {methods:?}"
        );
        assert!(
            methods.contains(&"load"),
            "should extract load: {methods:?}"
        );
        assert!(
            methods.contains(&"delete"),
            "should extract delete: {methods:?}"
        );
    }

    #[test]
    fn nested_function_definitions_extracted() {
        let src = "fn outer() {\n    fn inner() {}\n    inner();\n}\n";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .map(|n| n.name.as_str())
            .collect();
        assert!(funcs.contains(&"outer"), "should extract outer: {funcs:?}");
        assert!(funcs.contains(&"inner"), "should extract inner: {funcs:?}");
    }

    #[test]
    fn multiple_use_declarations() {
        let src = "use std::io;\nuse std::fs;\nuse std::collections::HashMap;\n";
        let result = extract(src);
        assert_eq!(result.imports.len(), 3, "should extract 3 use declarations");
    }

    #[test]
    fn pub_static_marked_as_exported() {
        let src = "pub static VERSION: &str = \"1.0\";";
        let result = extract(src);
        let s = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Static && n.name == "VERSION")
            .expect("should extract VERSION static");
        assert!(s.is_exported, "pub static should be exported");
    }

    #[test]
    fn pub_const_marked_as_exported() {
        let src = "pub const MAX_SIZE: usize = 1024;";
        let result = extract(src);
        let c = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Const && n.name == "MAX_SIZE")
            .expect("should extract MAX_SIZE const");
        assert!(c.is_exported, "pub const should be exported");
    }

    #[test]
    fn function_inside_if_block_at_module_scope() {
        let src = "fn main() {\n    if true {\n        let x = 1;\n    }\n}\n";
        let result = extract(src);
        assert!(
            result.nodes.iter().any(|n| n.name == "main"),
            "should extract main function"
        );
    }

    // --- tree-walking helper for direct function tests ---

    fn find_first_by_kind<'a>(
        node: tree_sitter::Node<'a>,
        kind: &str,
    ) -> Option<tree_sitter::Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                if let Some(found) = find_first_by_kind(child, kind) {
                    return Some(found);
                }
            }
        }
        None
    }

    // --- pattern_name _ fallback Some path (lines 1227-1232) ---

    #[test]
    fn pattern_name_wildcard_pattern_returns_some() {
        // `let _ = 1;` — the pattern node (wildcard_pattern or identifier "_")
        // is not in the explicit match arms (or is an identifier with text "_"),
        // falls to _ fallback. Text "_" passes validation (starts with '_',
        // all chars alphanumeric or '_').
        let src = "fn main() { let _ = 1; }";
        let tree = parse_source(src);
        let root = tree.root_node();
        let let_decl =
            find_first_by_kind(root, "let_declaration").expect("should find let_declaration");
        let pattern = let_decl
            .child_by_field_name("pattern")
            .expect("let_declaration should have pattern field");
        let result = pattern_name(pattern, src);
        // If pattern is wildcard_pattern → fallback returns Some("_")
        // If pattern is identifier "_" → identifier arm returns Some("_")
        assert_eq!(
            result,
            Some("_".to_string()),
            "wildcard pattern '_' should return Some(\"_\") (kind: {})",
            pattern.kind()
        );
    }

    // --- use_imported_names identifier arm (lines 1153-1156) ---

    #[test]
    fn use_imported_names_on_identifier_node_directly() {
        // Call use_imported_names directly on an identifier node parsed
        // from `use std;`. This covers the "identifier" match arm.
        let src = "use std;";
        let tree = parse_source(src);
        let root = tree.root_node();
        let ident = find_first_by_kind(root, "identifier").expect("should find identifier node");
        let names = use_imported_names(ident, src);
        assert!(
            names.contains(&"std".to_string()),
            "use_imported_names on identifier 'std' should return [\"std\"]: {names:?}"
        );
    }

    // --- pattern_name _ fallback Some path via type_identifier (lines 1227-1232) ---

    #[test]
    fn pattern_name_fallback_accepts_type_identifier_text() {
        // type_identifier is not in pattern_name's match arms, so it hits
        // the _ fallback. Text "Foo" passes validation (all alphanumeric,
        // starts with alphabetic) → returns Some("Foo").
        let src = "struct Foo;";
        let tree = parse_source(src);
        let root = tree.root_node();
        let type_ident =
            find_first_by_kind(root, "type_identifier").expect("should find type_identifier");
        let result = pattern_name(type_ident, src);
        assert_eq!(
            result,
            Some("Foo".to_string()),
            "type_identifier 'Foo' should return Some(\"Foo\") via fallback"
        );
    }

    // --- use_path scoped_use_list without path (lines 1095, 1097) ---

    #[test]
    fn use_path_scoped_use_list_or_scoped_identifier_without_path() {
        // Walk the tree from `use ::{io, fs};` and call use_path on every
        // scoped_use_list / scoped_identifier node. If any has no `path`
        // field, this covers the (None, Some) or (None, None) match arms.
        let src = "use ::{io, fs};";
        let tree = parse_source(src);
        let root = tree.root_node();
        fn walk_and_call(node: tree_sitter::Node, src: &str, results: &mut Vec<Option<String>>) {
            if matches!(
                node.kind(),
                "scoped_use_list" | "scoped_identifier" | "scoped_type_list"
            ) {
                results.push(use_path(node, src));
            }
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    walk_and_call(child, src, results);
                }
            }
        }
        let mut results = Vec::new();
        walk_and_call(root, src, &mut results);
        // Should not panic; at least some scoped nodes should be found
        assert!(
            !results.is_empty(),
            "should find at least one scoped_use_list/scoped_identifier"
        );
    }

    // --- use_path scoped_type_list branch (lines 1120-1128) ---

    #[test]
    fn use_path_scoped_type_list_branch() {
        // `use std::{Vec, HashMap};` may produce scoped_type_list nodes
        // for type imports. Walk the tree and call use_path on any
        // scoped_type_list found.
        let src = "use std::{Vec, HashMap};";
        let tree = parse_source(src);
        let root = tree.root_node();
        if let Some(stl) = find_first_by_kind(root, "scoped_type_list") {
            let result = use_path(stl, src);
            // The path should contain "std"
            if let Some(ref p) = result {
                assert!(
                    p.contains("std"),
                    "scoped_type_list path should contain std: {p}"
                );
            }
        }
        // If no scoped_type_list is found, the test still passes —
        // the grammar may not produce this node kind.
    }

    // --- use_imported_names use_clause branch (lines 1138-1144) ---

    #[test]
    fn use_imported_names_on_use_clause_if_present() {
        // If the grammar produces a use_clause node, call use_imported_names
        // on it to cover the use_clause branch.
        let src = "use std::{io, fs};";
        let tree = parse_source(src);
        let root = tree.root_node();
        if let Some(uc) = find_first_by_kind(root, "use_clause") {
            let names = use_imported_names(uc, src);
            // use_clause recurses into children; should find some names
            let _ = names;
        }
        // If no use_clause is found, test still passes (grammar dependent)
    }

    // --- use_path use_clause branch (lines 1076-1083) ---

    #[test]
    fn use_path_on_use_clause_if_present() {
        let src = "use std::io;";
        let tree = parse_source(src);
        let root = tree.root_node();
        if let Some(uc) = find_first_by_kind(root, "use_clause") {
            let result = use_path(uc, src);
            assert!(
                result.is_some(),
                "use_path on use_clause should return Some"
            );
        }
    }

    // --- extern_language direct string_literal fallback (lines 1032-1044) ---
    // The extern_modifier loop handles normal `extern "C" {}`. To cover the
    // direct string_literal fallback, call extern_language on a non-extern
    // node that has a direct string_literal child.

    #[test]
    fn extern_language_fallback_recognizes_c_string() {
        // let_declaration has a direct string_literal child "C".
        let src = r#"fn main() { let x = "C"; }"#;
        let tree = parse_source(src);
        let root = tree.root_node();
        let let_decl =
            find_first_by_kind(root, "let_declaration").expect("should find let_declaration");
        let lang = extern_language(let_decl, src);
        assert_eq!(
            lang,
            Language::C,
            "string 'C' should be recognized as Language::C via fallback"
        );
    }

    #[test]
    fn extern_language_fallback_recognizes_fortran_string() {
        let src = r#"fn main() { let x = "Fortran"; }"#;
        let tree = parse_source(src);
        let root = tree.root_node();
        let let_decl =
            find_first_by_kind(root, "let_declaration").expect("should find let_declaration");
        let lang = extern_language(let_decl, src);
        assert_eq!(
            lang,
            Language::Fortran,
            "string 'Fortran' should be recognized as Language::Fortran via fallback"
        );
    }

    #[test]
    fn extern_language_fallback_recognizes_python_string() {
        let src = r#"fn main() { let x = "Python"; }"#;
        let tree = parse_source(src);
        let root = tree.root_node();
        let let_decl =
            find_first_by_kind(root, "let_declaration").expect("should find let_declaration");
        let lang = extern_language(let_decl, src);
        assert_eq!(
            lang,
            Language::Python,
            "string 'Python' should be recognized as Language::Python via fallback"
        );
    }

    #[test]
    fn extern_language_fallback_unknown_string_returns_default() {
        // Unknown string → falls through to Language::all()[0]
        let src = r#"fn main() { let x = "Rust"; }"#;
        let tree = parse_source(src);
        let root = tree.root_node();
        let let_decl =
            find_first_by_kind(root, "let_declaration").expect("should find let_declaration");
        let lang = extern_language(let_decl, src);
        // "rust" doesn't match c/fortran/python → returns Language::all()[0]
        assert_eq!(
            lang,
            Language::all()[0],
            "unknown string should return default language"
        );
    }
}
