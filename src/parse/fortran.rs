// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Fortran language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-fortran's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `module` → [`NodeLabel::Module`]
//! - `subroutine` → [`NodeLabel::Function`]
//! - `function` → [`NodeLabel::Function`]
//! - `program` → [`NodeLabel::Function`] (treated as a function)
//!
//! # Extracted records
//!
//! - `use_statement` → [`ImportInfo`]
//! - `subroutine_call` / `call_statement` → [`CallInfo`]
//! - `use iso_c_binding` → [`ExternInfo`] (FFI detection)
//! - `assignment_statement` left → [`WriteInfo`] (BR-TRACE-006)
//! - `do_loop` loop variable → [`WriteInfo`] (BR-TRACE-006)
//! - expression-position `identifier` → [`ReadInfo`] (BR-TRACE-005)

use std::collections::HashSet;

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::{FqnGenerator, ScopeContext, ScopeResolverRegistry};

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{
    CallInfo, ExternInfo, ExtractResult, Extractor, ImportInfo, ReadInfo, WriteInfo,
};
use super::parser_factory::ParserFactory;

/// Fortran language tree-sitter extractor (Adapter pattern).
pub struct FortranExtractor {
    _priv: (),
}

impl FortranExtractor {
    /// Creates a new `FortranExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for FortranExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for FortranExtractor {
    fn language(&self) -> Language {
        Language::Fortran
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Fortran);
        let mut parser = ParserFactory::create_parser(Language::Fortran)?;
        // B11 fix: tree-sitter-fortran only supports free-form Fortran
        // comments (`!`). Fixed-form files (`.f` extension) use `*`, `C`, or
        // `c` in column 1 as comment characters, which tree-sitter-fortran
        // mis-parses as code. This causes the parser to fail catastrophically
        // on small files (e.g. LAPACK's xerbla.f) — the entire AST becomes
        // ERROR nodes and subroutine/function definitions are never
        // recognized. We preprocess fixed-form files by replacing the
        // column-1 comment character with `!`, preserving byte offsets so
        // tree-sitter positions stay valid.
        let effective_source = preprocess_fixed_form_comments(source, file_path);
        let tree = parser
            .parse(effective_source.as_str(), None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        // B10 fix: collect declared array names to distinguish function calls
        // from array access. tree-sitter-fortran parses both `ABS(Y)` and
        // `D(I)` as `call_expression`; we filter out array access by checking
        // if the callee matches a declared array name.
        let declared_arrays = collect_declared_arrays(root, &effective_source);
        let registry = ScopeResolverRegistry::new();
        let ctx = VisitContext {
            file_path,
            project,
            current_func: None,
            current_parent: None,
            resolver: &registry,
            declared_arrays: &declared_arrays,
        };
        for i in 0..root.named_child_count() as u32 {
            if let Some(child) = root.named_child(i) {
                visit_node(child, &effective_source, &ctx, &mut result);
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
    /// Declared array names in the current file (B10 fix: distinguish
    /// function calls from array access in Fortran, since tree-sitter-fortran
    /// parses both `ABS(Y)` and `D(I)` as `call_expression`).
    declared_arrays: &'a HashSet<String>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "module" => {
            extract_module(node, source, ctx, result);
            // 把模块名纳入 current_parent，使模块内子程序/函数生成不同 FQN
            // （与 c.rs / rust_extractor.rs / python.rs 的 parent 传递一致）。
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Fortran)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let mod_name = scope.as_ref().map(|s| s.name.as_str());
            let combined = combine_scope(ctx.current_parent, mod_name);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: None,
                current_parent: combined.as_deref(),
                resolver: ctx.resolver,
                declared_arrays: ctx.declared_arrays,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "subroutine" => {
            extract_subroutine_or_function(node, source, ctx, result, "subroutine_statement");
            extract_bind_c(node, source, ctx, result, "subroutine_statement");
            // Pass the subroutine's name as the enclosing function for body
            // traversal, so calls inside it can be attributed to it.
            // NOTE: 不把子程序名纳入 current_parent —— 否则 caller_qn 会与
            // 子程序自身 FQN 不匹配。嵌套同名子程序由 dedupe_qn 消歧。
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Fortran)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let func_name = scope.as_ref().map(|s| s.name.as_str());
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: func_name,
                current_parent: ctx.current_parent,
                resolver: ctx.resolver,
                declared_arrays: ctx.declared_arrays,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "function" => {
            extract_subroutine_or_function(node, source, ctx, result, "function_statement");
            extract_bind_c(node, source, ctx, result, "function_statement");
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Fortran)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let func_name = scope.as_ref().map(|s| s.name.as_str());
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: func_name,
                current_parent: ctx.current_parent,
                resolver: ctx.resolver,
                declared_arrays: ctx.declared_arrays,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "program" => {
            extract_program(node, source, ctx, result);
            let scope_ctx = ScopeContext {
                source,
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            let scope = ctx
                .resolver
                .get(Language::Fortran)
                .and_then(|r| r.resolve(node, &scope_ctx));
            let func_name = scope.as_ref().map(|s| s.name.as_str());
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: func_name,
                current_parent: ctx.current_parent,
                resolver: ctx.resolver,
                declared_arrays: ctx.declared_arrays,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "use_statement" => {
            extract_use(node, source, result);
        }
        "subroutine_call" | "call_statement" => {
            extract_call(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "call_expression" => {
            // B10 fix: tree-sitter-fortran parses both function calls (`ABS(Y)`)
            // and array access (`D(I)`) as `call_expression`. We distinguish
            // them by checking if the callee matches a declared array name.
            // If it's an array access, skip CallInfo creation but still
            // visit_children to capture nested function calls in array indices
            // (e.g., `D(FUNC(X))` — D is array, FUNC(X) is a function call).
            let callee = first_identifier_child(node, source);
            let is_array_access = callee
                .map(|name| ctx.declared_arrays.contains(name))
                .unwrap_or(false);
            if !is_array_access {
                extract_call(node, source, ctx, result);
            }
            visit_children(node, source, ctx, result);
        }
        "assignment_statement" => {
            // `x = expr` writes the left-hand identifier (BR-TRACE-006). Only
            // simple identifier targets are captured; array/struct writes are
            // ignored. Only attribute a write when inside a function body
            // (current_func is Some). The right-hand expression's identifiers
            // are captured as reads by the `identifier` branch during
            // `visit_children`.
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
        "do_loop" => {
            // `do i = 1, 10 ... end do` writes the loop variable (BR-TRACE-006).
            // The loop variable is the first `identifier` inside the
            // `loop_control_expression` child of the `do_statement`. `do while`
            // loops have no loop variable and are skipped here. The loop body's
            // identifiers are captured as reads by the `identifier` branch
            // during `visit_children`.
            if let Some(func) = ctx.current_func {
                if let Some(loop_var) = do_loop_variable(node, source) {
                    result.writes.push(WriteInfo {
                        writer_qn: Some(make_qn(
                            ctx.file_path,
                            func,
                            ctx.project,
                            ctx.current_parent,
                        )),
                        var_name: loop_var,
                        line: node.start_position().row as u32 + 1,
                    });
                }
            }
            visit_children(node, source, ctx, result);
        }
        "identifier" => {
            // A bare identifier in an expression position is a variable read
            // (BR-TRACE-005). Name-defining positions (assignment left, loop
            // control variable, declaration declarator, callee) are excluded by
            // `is_fortran_read_position`.
            if let Some(func) = ctx.current_func {
                if is_fortran_read_position(node) {
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

/// Returns `true` if the file path has a fixed-form Fortran extension (`.f`).
///
/// Fixed-form Fortran (F77) uses `*`, `C`, or `c` in column 1 as comment
/// characters. Free-form Fortran (F90+, `.f90`/`.f95`) uses `!` only.
fn is_fixed_form_fortran(file_path: &str) -> bool {
    let ext = file_path.rsplit('.').next().unwrap_or("");
    ext.eq_ignore_ascii_case("f")
}

/// Preprocesses fixed-form Fortran source so tree-sitter-fortran can parse it.
///
/// tree-sitter-fortran only recognizes free-form comments (`!`). Fixed-form
/// files (`.f` extension) use `*`, `C`, or `c` in column 1 as comment
/// characters, which tree-sitter-fortran mis-parses as code, causing
/// catastrophic parse failures on small files (e.g. LAPACK's `xerbla.f`).
///
/// This function replaces the column-1 comment character with `!` for `.f`
/// files. The replacement is a single-byte swap (no length change), so
/// tree-sitter byte offsets remain valid for `node_text` lookups.
///
/// For free-form files (`.f90`, `.f95`), the source is returned unchanged.
fn preprocess_fixed_form_comments(source: &str, file_path: &str) -> String {
    if !is_fixed_form_fortran(file_path) {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        if let Some(first) = line.bytes().next() {
            if first == b'*' || first == b'C' || first == b'c' {
                out.push('!');
                out.push_str(&line[1..]);
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

/// Collects all declared array names in the file by walking the tree and
/// finding `sized_declarator` nodes (e.g., `D(10)` in `REAL :: D(10)`).
/// These names are used to filter array access from function calls (B10 fix).
fn collect_declared_arrays(root: Node, source: &str) -> HashSet<String> {
    let mut arrays = HashSet::new();
    collect_arrays_recursive(root, source, &mut arrays);
    arrays
}

fn collect_arrays_recursive(node: Node, source: &str, arrays: &mut HashSet<String>) {
    if node.kind() == "sized_declarator" {
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                if child.kind() == "identifier" {
                    if let Some(name) = node_text(child, source) {
                        arrays.insert(name.to_string());
                    }
                }
            }
        }
    }
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            collect_arrays_recursive(child, source, arrays);
        }
    }
}

/// Returns the text of the first `identifier` named child of `node`, if any.
fn first_identifier_child<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "identifier" {
                return node_text(child, source);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Definition extractors
// ---------------------------------------------------------------------------

fn extract_module(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = statement_name(node, "module_statement", source) else {
        return;
    };
    let line = node.start_position().row as u32 + 1;
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        line,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Module, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Fortran)
        .project(ctx.project)
        // Fortran modules are public by default — mark as exported so
        // cross-file `use module_name` import resolution can find them.
        .is_exported(true)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_subroutine_or_function(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
    statement_kind: &str,
) {
    let Some(name) = statement_name(node, statement_kind, source) else {
        return;
    };
    let line = node.start_position().row as u32 + 1;
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        line,
        result,
    );
    let signature = node_text(node, source).map(String::from);
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Fortran)
        .project(ctx.project)
        // Fortran top-level subroutines/functions are public by default —
        // mark as exported so cross-file CALLS resolution via
        // `lookup_exported` (resolve/symbol_table.rs) can find them.
        // Without this, B10 fix produced LAPACK CALLS=80 (was 2747) because
        // all cross-file calls were unresolvable and silently skipped.
        .is_exported(true)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_program(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = statement_name(node, "program_statement", source) else {
        return;
    };
    let line = node.start_position().row as u32 + 1;
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        line,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Fortran)
        .project(ctx.project)
        // Fortran programs are top-level public entities — mark as exported
        // for consistency with subroutines/functions (Fortran default
        // visibility is public).
        .is_exported(true)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

/// Extracts FFI binding names from `bind(C)` declarations on subroutines and
/// functions (BR-TRACE-008).
///
/// Scans the `language_binding` child of a subroutine/function statement for
/// the `bind(C)` attribute. If present, collects the Fortran symbol name and
/// the optional C alias specified via `name="..."`.
///
/// # Arguments
///
/// * `node` - The `subroutine` or `function` node.
/// * `source` - The source text.
/// * `ctx` - The visit context (for file path / project).
/// * `result` - The extraction result to push `ExternInfo` into.
/// * `statement_kind` - `"subroutine_statement"` or `"function_statement"`.
fn extract_bind_c(
    node: Node,
    source: &str,
    _ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
    statement_kind: &str,
) {
    // Find the statement child (subroutine_statement / function_statement).
    let Some(stmt) = find_child(node, source, statement_kind) else {
        return;
    };
    // Find the language_binding child of the statement.
    let Some(binding) = find_child(stmt, source, "language_binding") else {
        return;
    };
    // Verify it binds to C (identifier child == "C").
    let binds_c = (0..binding.named_child_count() as u32)
        .filter_map(|i| binding.named_child(i))
        .any(|c| c.kind() == "identifier" && node_text(c, source) == Some("C"));
    if !binds_c {
        return;
    }
    // Get the subroutine/function name.
    let Some(symbol_name) = statement_name(node, statement_kind, source) else {
        return;
    };
    // Look for an optional `name="c_alias"` keyword argument.
    let mut c_name = symbol_name.clone();
    for i in 0..binding.named_child_count() as u32 {
        if let Some(kw) = binding.named_child(i) {
            if kw.kind() == "keyword_argument" {
                // keyword_argument has identifier + string_literal children.
                let mut is_name_kw = false;
                let mut alias = None;
                for j in 0..kw.named_child_count() as u32 {
                    if let Some(child) = kw.named_child(j) {
                        match child.kind() {
                            "identifier" if node_text(child, source) == Some("name") => {
                                is_name_kw = true;
                            }
                            "string_literal" => {
                                alias = node_text(child, source).map(String::from);
                            }
                            _ => {}
                        }
                    }
                }
                if is_name_kw {
                    if let Some(a) = alias {
                        // Strip surrounding quotes from the string literal.
                        c_name = a.trim_matches('"').to_string();
                    }
                    break;
                }
            }
        }
    }
    let line = node.start_position().row as u32 + 1;
    result.externs.push(ExternInfo {
        language: Language::C,
        names: vec![c_name],
        line,
        signature: Some(symbol_name),
    });
}

/// Returns the first named child of `node` matching `kind`.
fn find_child<'a>(node: Node<'a>, _source: &'a str, kind: &str) -> Option<Node<'a>> {
    (0..node.named_child_count() as u32)
        .filter_map(|i| node.named_child(i))
        .find(|c| c.kind() == kind)
}

fn extract_use(node: Node, source: &str, result: &mut ExtractResult) {
    // use_statement has a module_name child.
    let mut module_name = None;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "module_name" {
                module_name = node_text(child, source).map(String::from);
                break;
            }
        }
    }
    let Some(name) = module_name else {
        return;
    };
    let line = node.start_position().row as u32 + 1;
    // Note: `use iso_c_binding` is an implicit module import that does not
    // list specific FFI symbol names. Actual FFI bindings are declared via
    // `bind(C)` attributes on subroutines/functions, handled by
    // `extract_bind_c`. We no longer push an empty-names ExternInfo here —
    // it produced 0 FfiCalls edges and obscured the real FFI mechanism.
    result.imports.push(ImportInfo {
        source_file: name,
        imported_names: Vec::new(),
        line,
    });
}

fn extract_call(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    // subroutine_call has an identifier child (the callee) and an argument_list.
    let mut callee = None;
    let mut args = Vec::new();
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            match child.kind() {
                "identifier" => {
                    if callee.is_none() {
                        callee = node_text(child, source).map(String::from);
                    }
                }
                "argument_list" => {
                    for j in 0..child.named_child_count() as u32 {
                        if let Some(arg) = child.named_child(j) {
                            if let Ok(text) = arg.utf8_text(source.as_bytes()) {
                                args.push(text.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let Some(callee) = callee else {
        return;
    };
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

/// Extracts the name from a statement child node
/// (e.g. `module_statement`, `subroutine_statement`).
/// Tries the `name` field first, then falls back to a child with kind `name`.
fn statement_name(node: Node, statement_kind: &str, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == statement_kind {
                // Try the `name` field first.
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Some(text) = node_text(name_node, source) {
                        return Some(text.to_string());
                    }
                }
                // Fall back to a named child with kind `name` or `identifier`.
                for j in 0..child.named_child_count() as u32 {
                    if let Some(grandchild) = child.named_child(j) {
                        if grandchild.kind() == "name" || grandchild.kind() == "identifier" {
                            if let Some(text) = node_text(grandchild, source) {
                                return Some(text.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
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
/// position rather than a name-defining position (assignment left, loop control
/// variable, declaration declarator, callee). Mirrors the c.rs convention
/// (design.md Decision 4, Open Question 2): only the direct parent kind is
/// inspected, plus a field check for the assignment left / call function cases.
fn is_fortran_read_position(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        // Identifiers directly inside these expression containers are reads.
        "math_expression"
        | "relational_expression"
        | "parenthesized_expression"
        | "return_statement"
        | "argument_list"
        | "subscript_expression"
        | "conditional_expression" => true,
        // `call callee(arg)` -> the callee `identifier` (direct child of
        // subroutine_call / call_statement) is not a read; arguments are
        // handled above via the `argument_list` parent.
        "subroutine_call" | "call_statement" => false,
        // `x = y` -> `y` (the right side) is a read; `x` (the left) is not.
        "assignment_statement" => !is_at_field(node, parent, "left"),
        // `obj%field` -> the field identifier is not a read (it is a name, not
        // a value). The object side is a read.
        "field_expression" => false,
        // Loop control variable, declaration declarator, module/use name etc.
        // are name-defining, not reads.
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

/// Returns the loop variable name of a `do_loop` node, if any. The loop
/// variable is the first `identifier` child of the `loop_control_expression`
/// (which is a child of the `do_statement`). `do while` loops return `None`.
fn do_loop_variable(node: Node, source: &str) -> Option<String> {
    // Find the do_statement child, then its loop_control_expression.
    let do_statement = find_child(node, source, "do_statement")?;
    let loop_control = find_child(do_statement, source, "loop_control_expression")?;
    for i in 0..loop_control.named_child_count() as u32 {
        if let Some(child) = loop_control.named_child(i) {
            if child.kind() == "identifier" {
                return node_text(child, source).map(String::from);
            }
        }
    }
    None
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Fortran, parent)
}

/// Combine parent scope with child name using `{parent}_{child}` pattern.
/// Mirrors the helper in c.rs / rust_extractor.rs / python.rs.
fn combine_scope(parent: Option<&str>, child: Option<&str>) -> Option<String> {
    match (parent, child) {
        (Some(p), Some(c)) => Some(format!("{p}_{c}")),
        (None, Some(c)) => Some(c.to_string()),
        (Some(p), None) => Some(p.to_string()),
        (None, None) => None,
    }
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

    const FORTRAN_SOURCE: &str = r#"module mymod
    use iso_c_binding
contains
    subroutine my_sub(a, b)
        integer, intent(in) :: a
        integer, intent(out) :: b
        b = a + 1
    end subroutine
    function my_func(x) result(y)
        integer, intent(in) :: x
        integer :: y
        y = x * 2
    end function
end module

program main
    use mymod
    integer :: a, b
    call my_sub(1, b)
end program
"#;

    fn extract(source: &str) -> ExtractResult {
        let ext = FortranExtractor::new();
        ext.extract(source, "test.f90", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_fortran() {
        assert_eq!(FortranExtractor::new().language(), Language::Fortran);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = FortranExtractor::default();
        assert_eq!(ext.language(), Language::Fortran);
    }

    #[test]
    fn extracts_module() {
        let result = extract(FORTRAN_SOURCE);
        let modules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Module)
            .collect();
        assert_eq!(modules.len(), 1, "should extract 1 module");
        assert_eq!(modules[0].name, "mymod");
        assert_eq!(modules[0].language, Some(Language::Fortran));
        assert_eq!(modules[0].project, "proj");
        assert_eq!(modules[0].file_path.as_deref(), Some("test.f90"));
    }

    #[test]
    fn extracts_subroutine() {
        let result = extract(FORTRAN_SOURCE);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"my_sub"),
            "should extract my_sub subroutine: {:?}",
            names
        );
    }

    #[test]
    fn extracts_function() {
        let result = extract(FORTRAN_SOURCE);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"my_func"),
            "should extract my_func function: {:?}",
            names
        );
    }

    #[test]
    fn extracts_program() {
        let result = extract(FORTRAN_SOURCE);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"main"),
            "should extract main program: {:?}",
            names
        );
    }

    #[test]
    fn extracts_use_statements() {
        let result = extract(FORTRAN_SOURCE);
        // Two use statements: iso_c_binding and mymod.
        assert_eq!(result.imports.len(), 2, "should extract 2 use statements");
        let sources: Vec<_> = result
            .imports
            .iter()
            .map(|i| i.source_file.as_str())
            .collect();
        assert!(sources.contains(&"iso_c_binding"));
        assert!(sources.contains(&"mymod"));
    }

    #[test]
    fn extracts_call_to_my_sub() {
        let result = extract(FORTRAN_SOURCE);
        let callees: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"my_sub"),
            "should extract call to my_sub: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(FORTRAN_SOURCE);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "my_sub")
            .expect("call to my_sub should exist");
        assert_eq!(call.line, 19);
        assert_eq!(call.args.len(), 2, "my_sub(1, b) should have 2 args");
    }

    #[test]
    fn iso_c_binding_use_does_not_create_empty_extern() {
        // After the fix, `use iso_c_binding` no longer pushes an empty-names
        // ExternInfo — only `bind(C)` declarations produce ExternInfo with
        // actual symbol names.
        let result = extract(FORTRAN_SOURCE);
        assert!(
            result.externs.is_empty(),
            "use iso_c_binding without bind(C) should not create extern"
        );
    }

    #[test]
    fn extract_bind_c_collects_symbol_name_with_alias() {
        // subroutine my_func(a) bind(C, name="my_func_c") -> names=["my_func_c"]
        let src = r#"subroutine my_func(a) bind(C, name="my_func_c")
    use iso_c_binding
    integer(c_int), value :: a
end subroutine"#;
        let result = extract(src);
        assert_eq!(result.externs.len(), 1, "should detect 1 bind(C) FFI");
        let ext = &result.externs[0];
        assert_eq!(ext.language, Language::C);
        assert_eq!(ext.names, vec!["my_func_c"], "should use the C alias");
        assert_eq!(ext.signature.as_deref(), Some("my_func"));
    }

    #[test]
    fn extract_bind_c_without_name_uses_fortran_name() {
        // subroutine my_func(a) bind(C) -> names=["my_func"]
        let src = r#"subroutine my_func(a) bind(C)
    use iso_c_binding
    integer(c_int), value :: a
end subroutine"#;
        let result = extract(src);
        assert_eq!(result.externs.len(), 1, "should detect 1 bind(C) FFI");
        let ext = &result.externs[0];
        assert_eq!(ext.names, vec!["my_func"], "should use the Fortran name");
    }

    #[test]
    fn extract_bind_c_skips_non_bind_c_subroutines() {
        // Plain subroutine without bind(C) -> no externs
        let src = "subroutine my_func(a)\n    integer :: a\nend subroutine\n";
        let result = extract(src);
        assert!(
            result.externs.is_empty(),
            "non-bind(C) should not create extern"
        );
    }

    #[test]
    fn extract_bind_c_works_for_functions() {
        // function with bind(C) should also be detected
        let src = r#"function my_func(x) bind(C, name="my_func_c") result(y)
    use iso_c_binding
    integer(c_int), value :: x
    integer(c_int) :: y
end function"#;
        let result = extract(src);
        assert_eq!(
            result.externs.len(),
            1,
            "should detect 1 bind(C) FFI on function"
        );
        let ext = &result.externs[0];
        assert_eq!(ext.names, vec!["my_func_c"]);
    }

    #[test]
    fn creates_defines_edges() {
        // B1 fix: CONTAINS emission removed; only DEFINES remains.
        let result = extract(FORTRAN_SOURCE);
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
        let result = extract(FORTRAN_SOURCE);
        let mymod = result.nodes.iter().find(|n| n.name == "mymod").unwrap();
        assert_eq!(mymod.qualified_name, "proj.test.f90.mymod");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn subroutine_has_signature() {
        let result = extract(FORTRAN_SOURCE);
        let my_sub = result.nodes.iter().find(|n| n.name == "my_sub").unwrap();
        assert!(
            my_sub.signature.is_some(),
            "subroutine should have a signature"
        );
        assert!(my_sub.signature.as_deref().unwrap().contains("my_sub"));
    }

    #[test]
    fn result_language_is_fortran() {
        let result = extract(FORTRAN_SOURCE);
        assert_eq!(result.language, Language::Fortran);
        assert_eq!(result.file_path, "test.f90");
    }

    #[test]
    fn handles_standalone_subroutine() {
        let src = "subroutine foo(a)\n    integer :: a\nend subroutine\n";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "foo");
    }

    #[test]
    fn handles_standalone_function() {
        let src = "function bar(x) result(y)\n    integer :: x, y\n    y = x\nend function\n";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "bar");
    }

    #[test]
    fn use_without_iso_c_binding_no_extern() {
        let src = "program p\n    use other_mod\nend program\n";
        let result = extract(src);
        assert!(
            result.externs.is_empty(),
            "non-iso_c_binding use should not create extern"
        );
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        // Spec: Fortran 函数内调用生成非 None caller_qn (点分 FQN 格式)。
        let src = "subroutine caller()\n    call callee()\nend subroutine\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.f90", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.f90.caller"),
            "caller_qn should be the dotted FQN of the enclosing subroutine"
        );
        // The caller FQN must match the enclosing subroutine's node id.
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller subroutine node");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "caller_qn must match the caller subroutine node id"
        );
    }

    #[test]
    fn call_in_program_has_program_caller_qn() {
        // Spec intent: 顶层调用 caller_qn 为 None。Fortran 语义冲突 surfaced
        // (Rule 7/12): Fortran 要求每条可执行语句必须位于 program/subroutine/
        // function 内，且 program 被当作 NodeLabel::Function（见模块文档第 11
        // 行）。因此 Fortran 不存在 Python/TypeScript 意义上的"模块顶层调用"。
        // 这里验证等价语义：program 内的调用 caller_qn 应为 program 自身的
        // 点分 FQN，且与 program 节点的 qualified_name 一致。
        let src = "program main\n    call callee()\nend program\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "main.f90", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee inside program");
        let program_node = result
            .nodes
            .iter()
            .find(|n| n.name == "main")
            .expect("should find program main node");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some(program_node.qualified_name.as_str()),
            "call inside program should have caller_qn matching the program's FQN"
        );
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.main.f90.main"),
            "caller_qn should be the dotted FQN of the enclosing program"
        );
    }

    #[test]
    fn nested_subroutine_duplicate_name_disambiguated() {
        // 模拟 WRF share/dfi.F 场景：顶层子程序实现 + INTERFACE 块内同名声明。
        // tree-sitter-fortran 把 INTERFACE 内的 SUBROUTINE 也解析为 subroutine
        // 节点，导致与顶层实现生成相同 FQN。dedupe_qn 用 #L{line} 消歧。
        let src = r#"   SUBROUTINE dfi_array_reset(grid)
      INTEGER :: grid
   END SUBROUTINE dfi_array_reset

   RECURSIVE SUBROUTINE dfi_array_reset_recurse(grid)
      INTERFACE
         SUBROUTINE dfi_array_reset(grid)
            INTEGER :: grid
         END SUBROUTINE dfi_array_reset
      END INTERFACE
   END SUBROUTINE dfi_array_reset_recurse
"#;
        let result = extract(src);
        let resets: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.name == "dfi_array_reset")
            .collect();
        assert_eq!(
            resets.len(),
            2,
            "should extract 2 dfi_array_reset subroutines, got: {:?}",
            result.nodes.iter().map(|n| &n.name).collect::<Vec<_>>()
        );
        // FQNs must be unique (no primary key collision).
        assert_ne!(
            resets[0].qualified_name, resets[1].qualified_name,
            "FQNs must be unique: {} vs {}",
            resets[0].qualified_name, resets[1].qualified_name
        );
        // First occurrence: no disambiguator.
        assert!(
            !resets[0].qualified_name.contains('#'),
            "first occurrence should have no disambiguator: {}",
            resets[0].qualified_name
        );
        // Second occurrence: disambiguated with #L{line}.
        assert!(
            resets[1].qualified_name.contains("#L"),
            "duplicate should be disambiguated with #L: {}",
            resets[1].qualified_name
        );
    }

    #[test]
    fn read_in_subroutine_has_dotted_fqn_reader_qn() {
        // Spec: Fortran 子程序内 identifier 读取提取 (BR-TRACE-005)。
        let src = "subroutine caller(x)\n    integer, intent(in) :: x\n    integer :: y\n    y = x + 1\nend subroutine\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.f90", "proj")
            .expect("extraction should succeed");
        let read = result
            .reads
            .iter()
            .find(|r| r.var_name == "x")
            .expect("should find a read of x");
        assert_eq!(
            read.reader_qn.as_deref(),
            Some("proj.tmp.demo.main.f90.caller"),
            "reader_qn should be the dotted FQN of the enclosing subroutine"
        );
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller subroutine node");
        assert_eq!(
            read.reader_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "reader_qn must match the caller subroutine node id"
        );
    }

    #[test]
    fn write_in_subroutine_assignment_has_dotted_fqn_writer_qn() {
        // Spec: Fortran 子程序内 assignment_statement 写入提取 (BR-TRACE-006)。
        let src = "subroutine caller(x)\n    integer, intent(in) :: x\n    integer :: y\n    y = x + 1\nend subroutine\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.f90", "proj")
            .expect("extraction should succeed");
        let write = result
            .writes
            .iter()
            .find(|w| w.var_name == "y")
            .expect("should find a write of y");
        assert_eq!(
            write.writer_qn.as_deref(),
            Some("proj.tmp.demo.main.f90.caller"),
            "writer_qn should be the dotted FQN of the enclosing subroutine"
        );
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller subroutine node");
        assert_eq!(
            write.writer_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "writer_qn must match the caller subroutine node id"
        );
    }

    #[test]
    fn do_loop_variable_is_captured_as_write() {
        // Spec: Fortran do_loop 循环变量写入提取 (BR-TRACE-006)。
        let src = "subroutine looper()\n    integer :: i, s\n    do i = 1, 10\n        s = s + i\n    end do\nend subroutine\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.f90", "proj")
            .expect("extraction should succeed");
        let i_write = result
            .writes
            .iter()
            .find(|w| w.var_name == "i")
            .expect("should find a write of loop variable i");
        assert_eq!(
            i_write.writer_qn.as_deref(),
            Some("proj.tmp.demo.main.f90.looper"),
            "loop variable writer_qn should be the dotted FQN of the enclosing subroutine"
        );
        // The loop body `s = s + i` also writes s and reads s, i.
        assert!(
            result.writes.iter().any(|w| w.var_name == "s"),
            "loop body assignment should write s"
        );
        assert!(
            result.reads.iter().any(|r| r.var_name == "i"),
            "loop body should read i"
        );
    }

    #[test]
    fn module_level_declaration_no_reads_or_writes() {
        // Spec: Fortran 模块级声明不生成读写记录 (current_func 为 None)。
        let src = "module m\n    integer :: g\nend module\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.f90", "proj")
            .expect("extraction should succeed");
        assert!(
            result.reads.is_empty(),
            "module-level declaration must not produce ReadInfo: {:?}",
            result.reads
        );
        assert!(
            result.writes.is_empty(),
            "module-level declaration must not produce WriteInfo: {:?}",
            result.writes
        );
    }

    #[test]
    fn function_call_in_expression_is_extracted() {
        // B10 fix: function calls in expressions (e.g., `EPS = SLAMCH('Epsilon')`)
        // must be extracted as CallInfo. Previously, only `CALL` statements
        // (subroutine_call) were captured, missing all function calls.
        let src = r#"      SUBROUTINE TEST()
      REAL :: EPS
      EPS = SLAMCH('Epsilon')
      END SUBROUTINE
"#;
        let result = extract(src);
        let calls: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            calls.contains(&"SLAMCH"),
            "B10 fix: function call SLAMCH should be extracted: {:?}",
            calls
        );
    }

    #[test]
    fn array_access_not_extracted_as_call() {
        // B10 fix: array access `D(I)` must NOT be extracted as a function call.
        // tree-sitter-fortran parses both `ABS(Y)` and `D(I)` as call_expression;
        // we filter by checking if the callee is a declared array.
        let src = r#"      SUBROUTINE TEST()
      REAL :: D(10)
      INTEGER :: I, X
      X = D(I)
      END SUBROUTINE
"#;
        let result = extract(src);
        let calls: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            !calls.contains(&"D"),
            "B10 fix: array access D(I) must NOT be extracted as call: {:?}",
            calls
        );
    }

    #[test]
    fn nested_function_call_in_array_index_is_captured() {
        // B10 fix: when an array index contains a function call, the function
        // call should still be captured. e.g., `D(FUNC(I))` — D is array
        // (skip), but FUNC(I) is a function call (capture).
        let src = r#"      SUBROUTINE TEST()
      REAL :: D(10)
      INTEGER :: I, X
      X = D(MYFUNC(I))
      END SUBROUTINE
"#;
        let result = extract(src);
        let calls: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            !calls.contains(&"D"),
            "array access D(...) should not be a call: {:?}",
            calls
        );
        assert!(
            calls.contains(&"MYFUNC"),
            "nested function call MYFUNC should be captured: {:?}",
            calls
        );
    }

    #[test]
    fn multiple_function_calls_in_expression() {
        // B10 fix: multiple function calls in one expression should all be
        // captured. e.g., `Z = MAX(X, MIN(Y, EPS))` has two function calls.
        let src = r#"      SUBROUTINE TEST()
      REAL :: X, Y, Z, EPS
      Z = MAX(X, MIN(Y, EPS))
      END SUBROUTINE
"#;
        let result = extract(src);
        let calls: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            calls.contains(&"MAX"),
            "MAX should be captured: {:?}",
            calls
        );
        assert!(
            calls.contains(&"MIN"),
            "MIN should be captured: {:?}",
            calls
        );
    }

    #[test]
    fn function_call_with_array_arg_not_double_counted() {
        // B10 fix: `X = ABS(D(I))` — ABS is a function call (capture), D(I)
        // is array access (skip). Only one CallInfo should be created.
        let src = r#"      SUBROUTINE TEST()
      REAL :: D(10)
      INTEGER :: I
      REAL :: X
      X = ABS(D(I))
      END SUBROUTINE
"#;
        let result = extract(src);
        let calls: Vec<_> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            calls.contains(&"ABS"),
            "ABS function call should be captured: {:?}",
            calls
        );
        assert!(
            !calls.contains(&"D"),
            "array access D(I) should NOT be captured: {:?}",
            calls
        );
        // Only one call (ABS), not two.
        assert_eq!(
            result.calls.len(),
            1,
            "should have exactly 1 call (ABS only): {:?}",
            calls
        );
    }

    // --- B11 fix: fixed-form Fortran comment preprocessing ---

    #[test]
    fn is_fixed_form_detects_f_extension() {
        assert!(is_fixed_form_fortran("foo.f"));
        assert!(is_fixed_form_fortran("foo.F"));
        assert!(is_fixed_form_fortran("src/bar.f"));
        assert!(!is_fixed_form_fortran("foo.f90"));
        assert!(!is_fixed_form_fortran("foo.f95"));
        assert!(!is_fixed_form_fortran("foo.rs"));
    }

    #[test]
    fn preprocess_converts_star_comments_to_bang() {
        let src = "* This is a comment\n      SUBROUTINE FOO()\n      END\n";
        let out = preprocess_fixed_form_comments(src, "test.f");
        assert!(out.starts_with("! This is a comment"), "got: {out:?}");
        assert!(out.contains("SUBROUTINE FOO"));
    }

    #[test]
    fn preprocess_converts_uppercase_c_comments() {
        let src = "C This is a comment\n      SUBROUTINE FOO()\n      END\n";
        let out = preprocess_fixed_form_comments(src, "test.f");
        assert!(out.starts_with("! This is a comment"), "got: {out:?}");
    }

    #[test]
    fn preprocess_converts_lowercase_c_comments() {
        let src = "c This is a comment\n      SUBROUTINE FOO()\n      END\n";
        let out = preprocess_fixed_form_comments(src, "test.f");
        assert!(out.starts_with("! This is a comment"), "got: {out:?}");
    }

    #[test]
    fn preprocess_preserves_byte_offsets() {
        // Single-char replacement must not change byte offsets, otherwise
        // tree-sitter node positions would be invalid for node_text lookups.
        let src = "* comment\n      X = 1\n";
        let out = preprocess_fixed_form_comments(src, "test.f");
        assert_eq!(src.len(), out.len());
        assert_eq!(out.as_bytes()[0], b'!');
        // Position of "X = 1" is unchanged.
        assert_eq!(
            out.find("X = 1"),
            src.find("X = 1"),
            "byte offset of code must be preserved"
        );
    }

    #[test]
    fn preprocess_leaves_free_form_unchanged() {
        let src = "! comment\nsubroutine foo()\nend subroutine\n";
        let out = preprocess_fixed_form_comments(src, "test.f90");
        assert_eq!(src, out.as_str());
    }

    #[test]
    fn extract_finds_subroutine_in_fixed_form_file() {
        // B11 regression test: LAPACK's xerbla.f uses fixed-form Fortran
        // with `*` comment lines. Before the fix, tree-sitter-fortran
        // mis-parsed the entire file as ERROR, and the XERBLA subroutine
        // node was never extracted.
        let src = "*      SUBROUTINE XERBLA( SRNAME, INFO )\n*\n      SUBROUTINE XERBLA( SRNAME, INFO )\n      CHARACTER*(*) SRNAME\n      INTEGER INFO\n      WRITE(*,FMT=9999) SRNAME, INFO\n      STOP\n 9999 FORMAT(' error')\n      END\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "xerbla.f", "proj")
            .expect("extraction should succeed");
        let has_xerbla = result
            .nodes
            .iter()
            .any(|n| n.name == "XERBLA" && n.label == NodeLabel::Function);
        assert!(
            has_xerbla,
            "XERBLA subroutine node should be extracted from fixed-form file; got nodes: {:?}",
            result.nodes.iter().map(|n| &n.name).collect::<Vec<_>>()
        );
    }
}
