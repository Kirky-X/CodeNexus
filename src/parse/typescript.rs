// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! TypeScript language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-typescript's syntax tree into CodeNexus nodes, edges,
//! and intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_declaration` → [`NodeLabel::Function`]
//! - `class_declaration` → [`NodeLabel::Class`]
//! - `method_definition` → [`NodeLabel::Method`]
//! - `interface_declaration` → [`NodeLabel::Interface`] (P2-3: was Trait, now Interface for semantic alignment)
//! - `enum_declaration` → [`NodeLabel::Enum`]
//! - `type_alias_declaration` → [`NodeLabel::TypeAlias`]
//! - `lexical_declaration` (`const`, top-level) → [`NodeLabel::Const`] (P2-2: was AssignInfo-only)
//! - `lexical_declaration` with `arrow_function` / `function` value → [`NodeLabel::Function`] (P2-4: arrow/function expressions)
//!
//! # Extracted records
//!
//! - `import_statement` → [`ImportInfo`]
//! - `call_expression` → [`CallInfo`]
//! - `lexical_declaration` / `variable_declaration` → [`AssignInfo`]
//! - `assignment_expression` left → [`WriteInfo`] (BR-TRACE-006)
//! - `variable_declarator` name → [`WriteInfo`] (init, BR-TRACE-006)
//! - `update_expression` argument → [`WriteInfo`] (`++`/`--`, BR-TRACE-006)
//! - expression-position `identifier` → [`ReadInfo`] (BR-TRACE-005)

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::{FqnGenerator, ScopeContext, ScopeResolverRegistry};

use super::error::{ParseError, Result};
use super::extractor::{AssignInfo, CallInfo, ExtractResult, Extractor, ImportInfo, ReadInfo, WriteInfo};
use super::parser_factory::ParserFactory;
use super::dedupe_qn;

/// TypeScript language tree-sitter extractor (Adapter pattern).
pub struct TypeScriptExtractor {
    _priv: (),
}

impl TypeScriptExtractor {
    /// Creates a new `TypeScriptExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for TypeScriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for TypeScriptExtractor {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::TypeScript);
        let mut parser = ParserFactory::create_parser(Language::TypeScript)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
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

/// 不可变的遍历上下文，在 visit_node/visit_children 之间传递。
/// 封装 ADR-005 的 current_parent 和 current_func 语义。
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
    current_func: Option<&'a str>,
    current_parent: Option<&'a str>,
    resolver: &'a ScopeResolverRegistry,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            // P2-4: also handle `function* foo() {}` (generator_function_declaration)
            // which was previously missed.
            extract_function(node, source, ctx, result);
            // Pass the function's name as the enclosing function for body
            // traversal, so calls inside it can be attributed to it.
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::TypeScript)
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
        "class_declaration" => {
            extract_class(node, source, ctx.file_path, ctx.project, result);
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::TypeScript)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let class_name = scope.as_ref().map(|s| s.name.as_str());
            // TypeScript uses `.or()` (replace, not combine) — different from
            // Python/Rust/Fortran which use `combine_scope`.
            let parent = class_name.or(ctx.current_parent);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: parent,
                resolver: ctx.resolver,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "method_definition" => {
            extract_method(node, source, ctx, result);
            // Pass the method's name as the enclosing function for body
            // traversal, so calls inside it can be attributed to it.
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::TypeScript)
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
        "interface_declaration" => {
            // P2-3: TS interface → Interface (was Trait). Semantic alignment
            // with gitnexus which uses Interface for TS `interface Foo {}`.
            extract_named_item(node, NodeLabel::Interface, source, ctx.file_path, ctx.project, result);
            visit_children(node, source, ctx, result);
        }
        "enum_declaration" => {
            extract_named_item(node, NodeLabel::Enum, source, ctx.file_path, ctx.project, result);
            visit_children(node, source, ctx, result);
        }
        "type_alias_declaration" => {
            extract_named_item(node, NodeLabel::TypeAlias, source, ctx.file_path, ctx.project, result);
        }
        "import_statement" => {
            extract_import(node, source, result);
        }
        "export_statement" => {
            // P2-4: `export default function() {}` and `export default () => {}`
            // store the anonymous function as the `value` field (an expression),
            // not as a `declaration` field. visit_children won't promote it to
            // a Function node, so handle it explicitly here.
            if let Some(value) = node.child_by_field_name("value") {
                if matches!(value.kind(), "arrow_function" | "function_expression" | "function") {
                    let start_line = value.start_position().row as u32 + 1;
                    let end_line = value.end_position().row as u32 + 1;
                    let qn = dedupe_qn(
                        make_qn(ctx.file_path, "default", ctx.project, ctx.current_parent),
                        start_line,
                        result,
                    );
                    let signature = node_text(value, source).map(String::from);
                    build_and_push_function(
                        "default".to_string(),
                        qn,
                        start_line,
                        end_line,
                        signature,
                        true,
                        ctx,
                        result,
                    );
                }
            }
            // Recurse into the export to find the declaration inside.
            visit_children(node, source, ctx, result);
        }
        "call_expression" => {
            extract_call(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "lexical_declaration" | "variable_declaration" => {
            // P2-2/P2-4: extract Const nodes (top-level const) and Function
            // nodes (arrow_function / function expression values) in addition
            // to the existing AssignInfo records.
            extract_lexical_declaration(node, source, ctx, result);
            // BR-TRACE-006: each `variable_declarator`'s simple-identifier name
            // is a write (initialization). Only attribute a write when inside a
            // function body (current_func is Some); top-level const/let/var are
            // handled by extract_lexical_declaration as Const/Function nodes.
            if ctx.current_func.is_some() {
                for i in 0..node.named_child_count() as u32 {
                    if let Some(child) = node.named_child(i) {
                        if child.kind() == "variable_declarator" {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                if let Some(name) = identifier_text(name_node, source) {
                                    if let Some(func) = ctx.current_func {
                                        result.writes.push(WriteInfo {
                                            writer_qn: Some(make_qn(
                                                ctx.file_path,
                                                func,
                                                ctx.project,
                                                ctx.current_parent,
                                            )),
                                            var_name: name,
                                            line: child.start_position().row as u32 + 1,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            visit_children(node, source, ctx, result);
        }
        "variable_declarator" => {
            let var_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            let parent = var_name.as_deref().or(ctx.current_parent);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: parent,
                resolver: ctx.resolver,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "assignment_expression" => {
            // extract_assignment preserves the existing AssignInfo extraction
            // (P2-2). BR-TRACE-006: the left-hand simple identifier is a write,
            // captured only inside a function body. The right-hand expression's
            // identifiers are captured as reads by the `identifier` branch
            // during `visit_children`.
            extract_assignment(node, source, result);
            if let Some(func) = ctx.current_func {
                if let Some(left) = node.child_by_field_name("left") {
                    if let Some(name) = identifier_text(left, source) {
                        result.writes.push(WriteInfo {
                            writer_qn: Some(make_qn(
                                ctx.file_path,
                                func,
                                ctx.project,
                                ctx.current_parent,
                            )),
                            var_name: name,
                            line: node.start_position().row as u32 + 1,
                        });
                    }
                }
            }
            visit_children(node, source, ctx, result);
        }
        "update_expression" => {
            // `x++` / `++x` / `x--` / `--x` writes the operand identifier
            // (BR-TRACE-006). Only simple identifiers are captured; member
            // updates (`obj.x++`) are ignored. Only attribute a write when
            // inside a function body.
            if let Some(func) = ctx.current_func {
                if let Some(arg) = node.child_by_field_name("argument") {
                    if let Some(name) = identifier_text(arg, source) {
                        result.writes.push(WriteInfo {
                            writer_qn: Some(make_qn(
                                ctx.file_path,
                                func,
                                ctx.project,
                                ctx.current_parent,
                            )),
                            var_name: name,
                            line: node.start_position().row as u32 + 1,
                        });
                    }
                }
            }
            visit_children(node, source, ctx, result);
        }
        "identifier" => {
            // A bare identifier in an expression position is a variable read
            // (BR-TRACE-005). Name-defining positions (declarator name,
            // assignment left, update operand, callee, member property) are
            // excluded by `is_ts_read_position`.
            if let Some(func) = ctx.current_func {
                if is_ts_read_position(node) {
                    if let Some(name) = node_text(node, source).map(String::from) {
                        result.reads.push(ReadInfo {
                            reader_qn: Some(make_qn(
                                ctx.file_path,
                                func,
                                ctx.project,
                                ctx.current_parent,
                            )),
                            var_name: name,
                            line: node.start_position().row as u32 + 1,
                        });
                    }
                }
            }
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

/// Builds a TypeScript `Function` node, emits its `Defines`/`Contains` edges,
/// and pushes it into `result` (MED-003: shared by `extract_function` and the
/// `export_statement` anonymous-default-export handler).
#[allow(clippy::too_many_arguments)]
fn build_and_push_function(
    name: String,
    qn: String,
    start_line: u32,
    end_line: u32,
    signature: Option<String>,
    is_exported: bool,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(ctx.file_path)
        .start_line(start_line)
        .end_line(end_line)
        .language(Language::TypeScript)
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

// ---------------------------------------------------------------------------
// Definition extractors
// ---------------------------------------------------------------------------

fn extract_function(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // P2-4: anonymous `export default function() {}` has no name field.
    // Use "default" as the name (matches gitnexus behavior for default exports).
    //
    // NOTE: This None branch is currently defensive — extract_function is only
    // called from visit_node for function_declaration/generator_function_declaration,
    // both of which always have a name field per the tree-sitter grammar. The
    // anonymous export default case is handled inline in the export_statement
    // branch. This branch guards against future grammar evolution and is kept
    // intentionally.
    let name = match node.child_by_field_name("name") {
        Some(n) => match node_text(n, source).map(String::from) {
            Some(s) => s,
            None => return,
        },
        None => {
            // Only synthesize "default" when this is an export_statement child;
            // other anonymous functions (IIFEs etc.) are skipped.
            let is_default_export = node
                .parent()
                .map(|p| p.kind() == "export_statement")
                .unwrap_or(false);
            if !is_default_export {
                return;
            }
            "default".to_string()
        }
    };
    let is_exported = is_exported(node);
    let signature = node_text(node, source).map(String::from);
    let is_top_level = matches!(
        node.parent().map(|p| p.kind()),
        Some("program") | Some("export_statement") | None
    );
    let disambiguator = if is_top_level {
        None
    } else {
        let line = node.start_position().row as u32 + 1;
        Some(format!("L{line}"))
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, disambiguator.as_deref()),
        node.start_position().row as u32 + 1,
        result,
    );
    build_and_push_function(
        name,
        qn,
        node.start_position().row as u32 + 1,
        node.end_position().row as u32 + 1,
        signature,
        is_exported,
        ctx,
        result,
    );
}

fn extract_class(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let is_exported = is_exported(node);
    let qn = make_qn(file_path, &name, project, None);
    let model_node = ModelNode::builder(NodeLabel::Class, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::TypeScript)
        .project(project)
        .is_exported(is_exported)
        .is_global(true)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
    result.push_node(model_node);
}

fn extract_method(
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
    // When no semantic parent context (e.g. methods in anonymous object literals
    // like Object.defineProperty args), use line number as positional disambiguator
    let parent = ctx.current_parent;
    let disambiguator = match parent {
        Some(p) => Some(p.to_string()),
        None => {
            let line = node.start_position().row as u32 + 1;
            Some(format!("L{line}"))
        }
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, disambiguator.as_deref()),
        node.start_position().row as u32 + 1,
        result,
    );
    let mut builder = ModelNode::builder(NodeLabel::Method, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::TypeScript)
        .project(ctx.project)
        .is_global(false);
    // B8 fix: set parentQn for Method nodes so class_methods.cql can find them
    // (CodeNexus doesn't emit HAS_METHOD edges; parentQn is the linkage).
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_named_item(
    node: Node,
    label: NodeLabel,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let is_exported = is_exported(node);
    let qn = make_qn(file_path, &name, project, None);
    let model_node = ModelNode::builder(label, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::TypeScript)
        .project(project)
        .is_exported(is_exported)
        .is_global(true)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    // import_statement has an optional import_clause and a source (string).
    // The source field is the module path string.
    let source_file = node
        .child_by_field_name("source")
        .and_then(|n| node_text(n, source).map(String::from))
        .map(|s| s.trim_matches('\'').trim_matches('"').to_string())
        .unwrap_or_default();
    let mut imported_names = Vec::new();
    // Find the import_clause by kind (it may not be a named field).
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "import_clause" {
                collect_imported_names(child, source, &mut imported_names);
            }
        }
    }
    result.imports.push(ImportInfo {
        source_file,
        imported_names,
        line: node.start_position().row as u32 + 1,
    });
}

fn collect_imported_names(node: Node, source: &str, names: &mut Vec<String>) {
    match node.kind() {
        "import_specifier" => {
            // Try the `name` field first, then fall back to identifier child.
            if let Some(name_node) = node.child_by_field_name("name") {
                if let Some(name) = node_text(name_node, source).map(String::from) {
                    names.push(name);
                    return;
                }
            }
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if child.kind() == "identifier" {
                        if let Some(name) = node_text(child, source).map(String::from) {
                            names.push(name);
                            return;
                        }
                    }
                }
            }
        }
        "namespace_import" => {
            // import * as foo from 'mod'
            // Try the `alias` field first, then fall back to identifier child.
            if let Some(alias) = node.child_by_field_name("alias") {
                if let Some(name) = node_text(alias, source).map(String::from) {
                    names.push(name);
                    return;
                }
            }
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if child.kind() == "identifier" {
                        if let Some(name) = node_text(child, source).map(String::from) {
                            names.push(name);
                            return;
                        }
                    }
                }
            }
        }
        "identifier" => {
            // default import: import foo from 'mod'
            if let Some(name) = node_text(node, source).map(String::from) {
                names.push(name);
            }
        }
        _ => {
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    collect_imported_names(child, source, names);
                }
            }
        }
    }
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

fn extract_variable_declaration(node: Node, source: &str, result: &mut ExtractResult) {
    // lexical_declaration / variable_declaration contains variable_declarator children.
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "variable_declarator" {
                extract_variable_declarator(child, source, result);
            }
        }
    }
}

/// Returns true if the lexical_declaration / variable_declaration uses the
/// `const` keyword (first unnamed child is the `const` token).
fn is_const_declaration(node: Node) -> bool {
    // tree-sitter-typescript: lexical_declaration := choice('const','let','var') + declarators
    // The keyword is the first child (unnamed token with kind "const"/"let"/"var").
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            if child.is_named() {
                // First unnamed token already passed; no keyword found.
                return false;
            }
            return child.kind() == "const";
        }
    }
    false
}

/// Returns true if the declaration is at the top level (program or export_statement).
fn is_top_level_declaration(node: Node) -> bool {
    matches!(
        node.parent().map(|p| p.kind()),
        Some("program") | Some("export_statement") | None
    )
}

/// P2-2/P2-4: extract Const nodes (top-level `const`) and Function nodes
/// (arrow_function / function expression values) in addition to AssignInfo.
fn extract_lexical_declaration(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // Preserve original AssignInfo extraction.
    extract_variable_declaration(node, source, result);

    let is_const = is_const_declaration(node);
    let is_top_level = is_top_level_declaration(node);
    if !is_const {
        return;
    }

    for i in 0..node.named_child_count() as u32 {
        let Some(child) = node.named_child(i) else { continue };
        if child.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else { continue };
        // Only simple identifier names produce Const/Function nodes. Object/array
        // destructuring (`const { a, b } = ...`) produces pattern nodes whose
        // text contains braces/commas that corrupt CSV imports — gitnexus also
        // skips destructuring for Const nodes.
        if name_node.kind() != "identifier" {
            continue;
        }
        let Some(name) = node_text(name_node, source).map(String::from) else {
            continue;
        };
        let start_line = child.start_position().row as u32 + 1;
        let end_line = child.end_position().row as u32 + 1;
        let value_node = child.child_by_field_name("value");
        let value_kind = value_node.map(|v| v.kind());

        // P2-4: arrow_function / function_expression → Function node.
        // `const f = () => {}` and `const g = function() {}` are function
        // definitions gitnexus captures; codenexus previously missed them.
        if matches!(value_kind, Some("arrow_function") | Some("function_expression")) {
            let is_exported = is_exported(node);
            let qn = dedupe_qn(
                make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
                start_line,
                result,
            );
            let signature = node_text(child, source).map(String::from);
            let mut builder = ModelNode::builder(NodeLabel::Function, name.clone(), qn)
                .file_path(ctx.file_path)
                .start_line(start_line)
                .end_line(end_line)
                .language(Language::TypeScript)
                .project(ctx.project)
                .is_exported(is_exported)
                .is_global(true);
            if let Some(sig) = signature {
                builder = builder.signature(sig);
            }
            let model_node = builder.build();
            add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
            result.push_node(model_node);
            // Don't also create a Const node for the same declarator — a
            // function-typed const is modeled as a Function, not a Const.
            continue;
        }

        // P2-4 sub-case: only create a Const node when at the top level.
        // Nested consts (inside functions/blocks) are local variables, not
        // module-level constants — gitnexus applies the same rule.
        if !is_top_level {
            continue;
        }

        let is_exported = is_exported(node);
        let qn = dedupe_qn(
            make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
            start_line,
            result,
        );
        let signature = value_node.and_then(|v| node_text(v, source).map(String::from));
        let mut builder = ModelNode::builder(NodeLabel::Const, name, qn)
            .file_path(ctx.file_path)
            .start_line(start_line)
            .end_line(end_line)
            .language(Language::TypeScript)
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
}

fn extract_variable_declarator(node: Node, source: &str, result: &mut ExtractResult) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(target) = assignment_target_name(name_node, source) else {
        return;
    };
    let value_node = node.child_by_field_name("value");
    let (source_name, is_return_assign) = match value_node {
        Some(v) => {
            let is_call = v.kind() == "call_expression";
            let name = if is_call {
                v.child_by_field_name("function")
                    .and_then(|f| callee_name(f, source))
                    .unwrap_or_default()
            } else {
                // Only capture simple identifiers/attributes as source names.
                // Complex expressions (await, binary ops, arrays, etc.) would
                // produce FQNs with invalid characters (brackets, quotes,
                // newlines) that corrupt CSV imports.
                callee_name(v, source).unwrap_or_default()
            };
            (name, is_call)
        }
        None => (String::new(), false),
    };
    result.assignments.push(AssignInfo {
        target_name: target,
        source_name,
        line: node.start_position().row as u32 + 1,
        is_return_assign,
    });
}

fn extract_assignment(node: Node, source: &str, result: &mut ExtractResult) {
    // assignment_expression has `left` and `right` fields.
    let Some(left_node) = node.child_by_field_name("left") else {
        return;
    };
    let Some(target) = assignment_target_name(left_node, source) else {
        return;
    };
    let right_node = node.child_by_field_name("right");
    let (source_name, is_return_assign) = match right_node {
        Some(v) => {
            let is_call = v.kind() == "call_expression";
            let name = if is_call {
                v.child_by_field_name("function")
                    .and_then(|f| callee_name(f, source))
                    .unwrap_or_default()
            } else {
                // Only capture simple identifiers/attributes as source names.
                // Complex expressions (await, binary ops, arrays, etc.) would
                // produce FQNs with invalid characters (brackets, quotes,
                // newlines) that corrupt CSV imports.
                callee_name(v, source).unwrap_or_default()
            };
            (name, is_call)
        }
        None => (String::new(), false),
    };
    result.assignments.push(AssignInfo {
        target_name: target,
        source_name,
        line: node.start_position().row as u32 + 1,
        is_return_assign,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns true if the node is inside an `export_statement`.
fn is_exported(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == "export_statement"
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" | "property_identifier" => {
            node_text(node, source).map(String::from)
        }
        "member_expression" => {
            // e.g. `obj.method()` -> extract the property name.
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

fn assignment_target_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "property_identifier" => node_text(node, source).map(String::from),
        "member_expression" => {
            // e.g. `this.x = ...` -> extract "x"
            let property = node.child_by_field_name("property")?;
            node_text(property, source).map(String::from)
        }
        "array_pattern" | "object_pattern" => {
            // Extract the first identifier in the pattern.
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(name) = assignment_target_name(child, source) {
                        return Some(name);
                    }
                }
            }
            None
        }
        _ => {
            // Fallback: only accept simple identifier text. Complex
            // expressions (await, JSX, ternary, multi-statement blocks)
            // would produce FQNs with invalid characters (brackets, quotes,
            // newlines, semicolons) that corrupt CSV imports.
            let text = node_text(node, source)?;
            if text
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                && text
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
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

/// Returns the text of `node` if it is a plain `identifier`, else `None`.
fn identifier_text(node: Node, source: &str) -> Option<String> {
    if node.kind() == "identifier" {
        node_text(node, source).map(String::from)
    } else {
        None
    }
}

/// Returns `true` if a bare `identifier` node sits in a read (expression)
/// position rather than a name-defining position (declarator name, assignment
/// left, update operand, callee, member property). Mirrors the c.rs convention
/// (design.md Decision 4, Open Question 2): only the direct parent kind is
/// inspected, plus field checks for the assignment left / call function /
/// member object cases.
fn is_ts_read_position(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        // Identifiers directly inside these expression containers are reads.
        "binary_expression"
        | "unary_expression"
        | "parenthesized_expression"
        | "return_statement"
        | "argument_list"
        | "subscript_expression"
        | "conditional_expression"
        | "template_substitution"
        | "await_expression" => true,
        // `foo(x)` -> the callee `foo` (function field) is not a read;
        // arguments are handled above via the `argument_list` parent.
        "call_expression" => !is_at_field(node, parent, "function"),
        // `new Foo(x)` -> the constructor `Foo` (function field) is not a read.
        "new_expression" => !is_at_field(node, parent, "function"),
        // `x = y` -> `y` (the right side) is a read; `x` (the left) is not.
        "assignment_expression" => !is_at_field(node, parent, "left"),
        // `obj.prop` -> `obj` (the object) is a read; the property is a
        // `property_identifier`, not a plain `identifier`, so it is not
        // reached here, but guard explicitly for safety.
        "member_expression" => is_at_field(node, parent, "object"),
        // Declarator name, update operand, definition name, import name —
        // name-defining positions, not reads.
        "variable_declarator" | "update_expression" | "lexical_declaration"
        | "variable_declaration" | "function_declaration" | "method_definition"
        | "class_declaration" | "import_specifier" | "namespace_import"
        | "import_clause" | "export_statement" => false,
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

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::TypeScript, parent)
}

// `dedupe_qn` is shared across all extractors — see `parse::dedupe_qn` (MED-002).

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeLabel;

    const TS_SOURCE: &str = r#"import { foo } from './foo';
export function add(a: number, b: number): number {
    return a + b;
}
class Point {
    x: number;
    y: number;
    constructor(x: number, y: number) {
        this.x = x;
        this.y = y;
    }
    distance(): number {
        return this.x + this.y;
    }
}
const result = add(1, 2);
"#;

    fn extract(source: &str) -> ExtractResult {
        let ext = TypeScriptExtractor::new();
        ext.extract(source, "test.ts", "proj").expect("extraction should succeed")
    }

    #[test]
    fn language_returns_typescript() {
        assert_eq!(TypeScriptExtractor::new().language(), Language::TypeScript);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = TypeScriptExtractor::default();
        assert_eq!(ext.language(), Language::TypeScript);
    }

    #[test]
    fn extracts_import() {
        let result = extract(TS_SOURCE);
        assert_eq!(result.imports.len(), 1, "should extract 1 import");
        assert_eq!(result.imports[0].source_file, "./foo");
        assert!(
            result.imports[0].imported_names.contains(&"foo".to_string()),
            "imported names should contain foo: {:?}",
            result.imports[0].imported_names
        );
    }

    #[test]
    fn extracts_exported_function() {
        let result = extract(TS_SOURCE);
        let funcs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Function).collect();
        assert_eq!(funcs.len(), 1, "should extract 1 function (add)");
        assert_eq!(funcs[0].name, "add");
        assert!(funcs[0].is_exported, "add should be exported");
        assert_eq!(funcs[0].language, Some(Language::TypeScript));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.ts"));
    }

    #[test]
    fn extracts_class() {
        let result = extract(TS_SOURCE);
        let classes: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Class).collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Point");
    }

    #[test]
    fn extracts_methods() {
        let result = extract(TS_SOURCE);
        let methods: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Method).collect();
        let names: Vec<_> = methods.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"constructor"),
            "should extract constructor: {:?}",
            names
        );
        assert!(
            names.contains(&"distance"),
            "should extract distance method: {:?}",
            names
        );
    }

    #[test]
    fn extracts_call_to_add() {
        let result = extract(TS_SOURCE);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"add"),
            "should extract call to add: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(TS_SOURCE);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "add")
            .expect("call to add should exist");
        assert_eq!(call.args.len(), 2, "add(1, 2) should have 2 args");
    }

    #[test]
    fn extracts_assignment() {
        let result = extract(TS_SOURCE);
        let assign = result
            .assignments
            .iter()
            .find(|a| a.target_name == "result")
            .expect("should find `const result = add(1, 2)` assignment");
        assert_eq!(assign.source_name, "add");
        assert!(
            assign.is_return_assign,
            "assignment from function call should be return assign"
        );
    }

    #[test]
    fn creates_defines_edges() {
        // B1 fix: CONTAINS emission removed; only DEFINES remains.
        let result = extract(TS_SOURCE);
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count);
        // B1 fix verification: no CONTAINS edges should be emitted
        let contains_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Contains).count();
        assert_eq!(contains_count, 0, "B1 fix: no CONTAINS edges should be emitted");
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract(TS_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert_eq!(add.qualified_name, "proj.test.ts.add");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn function_has_signature() {
        let result = extract(TS_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn non_exported_function_not_marked_exported() {
        let src = "function private_fn() {}";
        let result = extract(src);
        let func = result.nodes.iter().find(|n| n.name == "private_fn").unwrap();
        assert!(!func.is_exported, "non-exported function should not be exported");
    }

    #[test]
    fn extracts_interface_as_interface_node() {
        // P2-3: TS interface → Interface (was Trait, now Interface for semantic
        // alignment with gitnexus).
        let src = "interface Drawable { draw(): void; }";
        let result = extract(src);
        let interfaces: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Interface).collect();
        assert_eq!(interfaces.len(), 1, "interface should map to Interface");
        assert_eq!(interfaces[0].name, "Drawable");
        // No Trait node should be created for an interface anymore.
        let traits: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Trait).collect();
        assert!(traits.is_empty(), "interface must not map to Trait");
    }

    #[test]
    fn extracts_enum() {
        let src = "enum Color { Red, Green, Blue }";
        let result = extract(src);
        let enums: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Enum).collect();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Color");
    }

    #[test]
    fn extracts_type_alias() {
        let src = "type Score = number;";
        let result = extract(src);
        let aliases: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::TypeAlias).collect();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].name, "Score");
    }

    #[test]
    fn handles_default_import() {
        let src = "import foo from './mod';";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "./mod");
        assert!(result.imports[0].imported_names.contains(&"foo".to_string()));
    }

    #[test]
    fn handles_namespace_import() {
        let src = "import * as utils from './utils';";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "./utils");
        assert!(result.imports[0].imported_names.contains(&"utils".to_string()));
    }

    #[test]
    fn handles_method_call() {
        let src = "class A { foo() { this.bar(); } }";
        let result = extract(src);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(callees.contains(&"bar"), "should extract this.bar() call");
    }

    #[test]
    fn handles_member_assignment() {
        let src = "class A { foo(x: number) { this.x = x; } }";
        let result = extract(src);
        assert!(
            result
                .assignments
                .iter()
                .any(|a| a.target_name == "x"),
            "should find this.x = x assignment"
        );
    }

    #[test]
    fn result_language_is_typescript() {
        let result = extract(TS_SOURCE);
        assert_eq!(result.language, Language::TypeScript);
        assert_eq!(result.file_path, "test.ts");
    }

    #[test]
    fn exported_interface_is_marked_exported() {
        // P2-3: interface → Interface (not Trait).
        let src = "export interface Drawable { draw(): void; }";
        let result = extract(src);
        let interfaces: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Interface).collect();
        assert_eq!(interfaces.len(), 1);
        assert!(interfaces[0].is_exported, "exported interface should be marked exported");
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        // Spec: TypeScript 函数内调用生成非 None caller_qn (点分 FQN 格式)。
        let src = "function caller(): void {\n    callee();\n}\n";
        let ext = TypeScriptExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.ts", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.ts.caller"),
            "caller_qn should be the dotted FQN of the enclosing function"
        );
        // The caller FQN must match the enclosing function's node id.
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
        // Spec: 顶层调用（无函数上下文）caller_qn 为 None。
        let src = "callee();\n";
        let ext = TypeScriptExtractor::new();
        let result = ext.extract(src, "main.ts", "proj").expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find top-level call to callee");
        assert!(call.caller_qn.is_none(), "top-level call should have None caller_qn");
    }

    #[test]
    fn method_without_parent_uses_line_disambiguator() {
        let src = "\
Object.defineProperty(el, 'boom', {
  enumerable: true,
  get() { throw new Error('a'); },
});
Object.defineProperty(el2, 'boom', {
  enumerable: true,
  get() { throw new Error('b'); },
});
";
        let result = extract(src);
        let gets: Vec<_> = result.nodes.iter().filter(|n| n.name == "get").collect();
        assert_eq!(gets.len(), 2, "should extract two `get` methods");
        assert_ne!(gets[0].qualified_name, gets[1].qualified_name);
        for g in &gets {
            assert!(g.qualified_name.contains("#L"), "expected line disambiguator: {}", g.qualified_name);
        }
    }

    #[test]
    fn nested_function_declaration_uses_line_disambiguator() {
        let src = "\
export function topLevel(): void {}
describe('suite', () => {
  function helper(): void {}
  it('test', () => {
    function helper(): void {}
  });
});
";
        let result = extract(src);
        let toplevel = result.nodes.iter().find(|n| n.name == "topLevel").expect("topLevel should exist");
        assert_eq!(toplevel.qualified_name, "proj.test.ts.topLevel");
        let helpers: Vec<_> = result.nodes.iter().filter(|n| n.name == "helper").collect();
        assert_eq!(helpers.len(), 2);
        assert_ne!(helpers[0].qualified_name, helpers[1].qualified_name);
        for h in &helpers {
            assert!(h.qualified_name.contains("#L"), "expected line disambiguator: {}", h.qualified_name);
        }
    }

    #[test]
    fn same_name_method_in_same_parent_scope_disambiguated() {
        // 模拟 GitNexus pipeline-runner.test.ts 场景：多个回调内各自定义
        // 同名变量 `phases`，其对象字面量内含同名方法 `execute`。
        // 两处 current_parent 均为 "phases"，产生相同 FQN → dedupe_qn 消歧。
        let src = "\
function setupFirst() {
  const phases = {
    execute() { return 1; }
  };
}
function setupSecond() {
  const phases = {
    execute() { return 2; }
  };
}
";
        let result = extract(src);
        let executes: Vec<_> = result.nodes.iter().filter(|n| n.name == "execute").collect();
        assert_eq!(executes.len(), 2, "should extract two `execute` methods");
        assert_ne!(executes[0].qualified_name, executes[1].qualified_name);
        // 第一个保留原 FQN（含 #phases parent 消歧符），第二个追加 #L{line}
        let (first, second) = (&executes[0], &executes[1]);
        assert!(first.qualified_name.contains("#phases"), "first qn: {}", first.qualified_name);
        assert!(second.qualified_name.contains("#phases"), "second qn: {}", second.qualified_name);
        assert!(second.qualified_name.contains("#L"), "second should have line dedupe: {}", second.qualified_name);
    }

    // --- P2-2 regression: top-level `const` → Const node ---

    #[test]
    fn extracts_top_level_const_as_const_node() {
        // P2-2 regression: `export const z = ...` previously only produced an
        // AssignInfo, no Const node (0 vs gitnexus 1384 in zod).
        let src = "export const MAX_RETRIES = 3;";
        let result = extract(src);
        let consts: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Const).collect();
        assert_eq!(consts.len(), 1, "should extract 1 Const node");
        assert_eq!(consts[0].name, "MAX_RETRIES");
        assert_eq!(consts[0].language, Some(Language::TypeScript));
        assert!(consts[0].is_exported, "exported const should be marked exported");
        assert!(consts[0].is_global, "top-level const should be global");
    }

    #[test]
    fn does_not_extract_nested_const_as_const_node() {
        // P2-2: nested const (inside a function) is a local variable, not a
        // module-level constant — gitnexus applies the same rule.
        let src = "function f() { const local = 1; return local; }";
        let result = extract(src);
        let consts: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Const).collect();
        assert!(consts.is_empty(), "nested const must NOT be a Const node");
    }

    #[test]
    fn does_not_extract_let_as_const_node() {
        // P2-2: only `const` declarations become Const nodes, not `let`/`var`.
        let src = "export let mutable = 1;";
        let result = extract(src);
        let consts: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Const).collect();
        assert!(consts.is_empty(), "`let` must NOT produce a Const node");
    }

    // --- P2-4 regression: arrow function / function expression → Function node ---

    #[test]
    fn extracts_arrow_function_const_as_function_node() {
        // P2-4 regression: `const f = () => {}` was missed entirely.
        let src = "export const handler = () => { return 42; };";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function && n.name == "handler")
            .collect();
        assert_eq!(funcs.len(), 1, "arrow function const should be a Function node");
        assert_eq!(funcs[0].language, Some(Language::TypeScript));
        assert!(funcs[0].is_exported, "exported arrow function should be marked exported");
        // Must NOT also be a Const node (function-typed const → Function, not Const).
        let consts: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Const && n.name == "handler").collect();
        assert!(consts.is_empty(), "arrow function const must not double-count as Const");
    }

    #[test]
    fn extracts_function_expression_const_as_function_node() {
        // P2-4: `const g = function() {}` (named function expression).
        let src = "const callback = function() { return 0; };";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function && n.name == "callback")
            .collect();
        assert_eq!(funcs.len(), 1, "function expression const should be a Function node");
    }

    #[test]
    fn extracts_anonymous_export_default_function() {
        // P2-4: `export default function() {}` (anonymous) was missed because
        // the function is stored as export_statement's `value` field (an
        // expression), not as a `declaration`. tree-sitter represents it as
        // function_expression, not function_declaration.
        let src = "export default function() { return 42; }";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function && n.name == "default")
            .collect();
        assert_eq!(
            funcs.len(),
            1,
            "anonymous export default function should be a Function named 'default'"
        );
        assert!(
            funcs[0].is_exported,
            "export default function should be marked exported"
        );
    }

    #[test]
    fn extracts_anonymous_export_default_arrow_function() {
        // P2-4 edge case: `export default () => {}` — anonymous arrow function
        // as default export. tree-sitter stores it as export_statement's value
        // field with kind arrow_function.
        let src = "export default () => { return 42; }";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function && n.name == "default")
            .collect();
        assert_eq!(
            funcs.len(),
            1,
            "anonymous export default arrow function should be a Function named 'default'"
        );
        assert!(
            funcs[0].is_exported,
            "export default arrow function should be marked exported"
        );
    }

    #[test]
    fn named_export_default_function_not_double_extracted() {
        // P2-4 edge case: `export default function foo() {}` (NAMED default
        // export) goes through the declaration field, not the value field.
        // It should produce exactly ONE Function node named "foo", not a
        // duplicate "default" node from the export_statement value handler.
        let src = "export default function foo() { return 42; }";
        let result = extract(src);
        let foo_funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function && n.name == "foo")
            .collect();
        let default_funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function && n.name == "default")
            .collect();
        assert_eq!(
            foo_funcs.len(),
            1,
            "named default export should produce one Function named 'foo'"
        );
        assert!(
            default_funcs.is_empty(),
            "named default export must NOT produce a synthetic 'default' Function"
        );
        assert!(
            foo_funcs[0].is_exported,
            "named default export should be marked exported"
        );
    }

    #[test]
    fn read_in_function_has_dotted_fqn_reader_qn() {
        // Spec: TypeScript 函数内 identifier 读取提取 (BR-TRACE-005)。
        // Uses `return x;` (not `let y = x + 1;`) so the read sits in a
        // return_statement rather than a variable_declarator value — the
        // latter would scope reader_qn to `caller#y` because the TS
        // `variable_declarator` branch threads the var name as
        // current_parent (a known divergence from c.rs, tracked separately).
        let src = "function caller(x: number): number {\n    return x;\n}\n";
        let ext = TypeScriptExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.ts", "proj")
            .expect("extraction should succeed");
        let read = result
            .reads
            .iter()
            .find(|r| r.var_name == "x")
            .expect("should find a read of x");
        assert_eq!(
            read.reader_qn.as_deref(),
            Some("proj.tmp.demo.main.ts.caller"),
            "reader_qn should be the dotted FQN of the enclosing function"
        );
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller function node");
        assert_eq!(
            read.reader_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "reader_qn must match the caller function node id"
        );
    }

    #[test]
    fn write_in_function_let_declaration_has_dotted_fqn_writer_qn() {
        // Spec: TypeScript 函数内 lexical_declaration 写入提取 (BR-TRACE-006)。
        let src = "function caller(x: number): number {\n    let y = x + 1;\n    return y;\n}\n";
        let ext = TypeScriptExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.ts", "proj")
            .expect("extraction should succeed");
        let write = result
            .writes
            .iter()
            .find(|w| w.var_name == "y")
            .expect("should find a write of y");
        assert_eq!(
            write.writer_qn.as_deref(),
            Some("proj.tmp.demo.main.ts.caller"),
            "writer_qn should be the dotted FQN of the enclosing function"
        );
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller function node");
        assert_eq!(
            write.writer_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "writer_qn must match the caller function node id"
        );
    }

    #[test]
    fn write_in_function_assignment_has_dotted_fqn_writer_qn() {
        // Spec: TypeScript 函数内 assignment_expression 写入提取 (BR-TRACE-006)。
        let src = "function caller(): number {\n    let y = 1;\n    y = y * 2;\n    return y;\n}\n";
        let ext = TypeScriptExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.ts", "proj")
            .expect("extraction should succeed");
        let y_writes: Vec<_> = result
            .writes
            .iter()
            .filter(|w| w.var_name == "y")
            .collect();
        assert!(
            y_writes.len() >= 2,
            "y should be written at least twice (let + assignment): {:?}",
            y_writes
        );
        for w in y_writes {
            assert_eq!(
                w.writer_qn.as_deref(),
                Some("proj.tmp.demo.main.ts.caller"),
                "writer_qn should be the dotted FQN of the enclosing function"
            );
        }
    }

    #[test]
    fn update_expression_is_write() {
        // Spec: TypeScript update_expression 写入提取 (BR-TRACE-006)。
        let src = "function caller(x: number): number {\n    let y = x;\n    y++;\n    return y;\n}\n";
        let ext = TypeScriptExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.ts", "proj")
            .expect("extraction should succeed");
        let y_writes: Vec<_> = result
            .writes
            .iter()
            .filter(|w| w.var_name == "y")
            .collect();
        assert!(
            y_writes.len() >= 2,
            "y should be written at least twice (let + update): {:?}",
            y_writes
        );
        for w in y_writes {
            assert_eq!(
                w.writer_qn.as_deref(),
                Some("proj.tmp.demo.main.ts.caller"),
                "writer_qn should be the dotted FQN of the enclosing function"
            );
        }
    }
}
