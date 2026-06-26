//! C language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-c's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_definition` → [`NodeLabel::Function`]
//! - `declaration` (top-level) → [`NodeLabel::GlobalVar`]
//! - `type_definition` → [`NodeLabel::Typedef`]
//! - `struct_specifier` (with body) → [`NodeLabel::Struct`]
//! - `enum_specifier` (with body) → [`NodeLabel::Enum`]
//!
//! # Extracted records
//!
//! - `preproc_include` → [`ImportInfo`]
//! - `call_expression` → [`CallInfo`]

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor, ImportInfo, ReadInfo, WriteInfo};
use super::parser_factory::ParserFactory;

/// C language tree-sitter extractor (Adapter pattern).
pub struct CExtractor {
    _priv: (),
}

impl CExtractor {
    /// Creates a new `CExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for CExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for CExtractor {
    fn language(&self) -> Language {
        Language::C
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::C);
        let mut parser = ParserFactory::create_parser(Language::C)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        // Walk all named children of the translation_unit.
        let ctx = VisitContext {
            file_path,
            project,
            current_func: None,
            current_parent: None,
        };
        for i in 0..root.named_child_count() as u32 {
            let child = match root.named_child(i) {
                Some(c) => c,
                None => continue,
            };
            visit_node(child, source, &ctx, &mut result);
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
        "function_definition" => {
            // C++ `namespace X { }` / `class X { }` / `struct X { }` are
            // misparsed by tree-sitter-c as function_definition whose `type`
            // field is a `type_identifier` whose text is the keyword.
            let type_text = node
                .child_by_field_name("type")
                .filter(|n| n.kind() == "type_identifier")
                .and_then(|n| node_text(n, source).map(String::from));
            match type_text.as_deref() {
                Some("namespace") | Some("class") | Some("struct") => {
                    // C++ scope block misparsed as function_definition.
                    let scope_name = node
                        .child_by_field_name("declarator")
                        .and_then(|n| node_text(n, source).map(String::from));
                    let combined = combine_scope(ctx.current_parent, scope_name.as_deref());
                    for i in 0..node.named_child_count() as u32 {
                        if let Some(child) = node.named_child(i) {
                            if child.kind() == "compound_statement" {
                                let child_ctx = VisitContext {
                                    file_path: ctx.file_path,
                                    project: ctx.project,
                                    current_func: None,
                                    current_parent: combined.as_deref(),
                                };
                                visit_children(child, source, &child_ctx, result);
                            }
                        }
                    }
                }
                _ => {
                    // Normal C function — with overload disambiguation for C++ methods.
                    // When inside a struct/class/namespace (current_parent is Some),
                    // append line number to parent so overloaded methods get distinct FQNs.
                    let start_line = node.start_position().row as u32 + 1;
                    let method_parent =
                        ctx.current_parent.map(|p| format!("{p}_L{start_line}"));
                    extract_function(node, source, ctx, method_parent.as_deref(), result);
                    let func_name = function_name(node, source);
                    for i in 0..node.named_child_count() as u32 {
                        if let Some(child) = node.named_child(i) {
                            if child.kind() == "compound_statement" {
                                let child_ctx = VisitContext {
                                    file_path: ctx.file_path,
                                    project: ctx.project,
                                    current_func: func_name.as_deref(),
                                    current_parent: method_parent.as_deref(),
                                };
                                visit_children(child, source, &child_ctx, result);
                            }
                        }
                    }
                }
            }
        }
        "declaration" => {
            extract_global_var(node, source, ctx, result);
            // Always recurse into declarations to find calls/reads inside
            // (e.g. `int x = foo();`).
            visit_children(node, source, ctx, result);
        }
        "preproc_include" => {
            extract_include(node, source, result);
        }
        "type_definition" => {
            extract_typedef(node, source, ctx, result);
        }
        "struct_specifier" => {
            extract_struct(node, source, ctx, result);
            // Pass the struct name as current_parent to children when a body exists.
            if node.child_by_field_name("body").is_some() {
                let struct_name = node
                    .child_by_field_name("name")
                    .and_then(|n| node_text(n, source).map(String::from));
                let combined = combine_scope(ctx.current_parent, struct_name.as_deref());
                let child_ctx = VisitContext {
                    file_path: ctx.file_path,
                    project: ctx.project,
                    current_func: ctx.current_func,
                    current_parent: combined.as_deref(),
                };
                visit_children(node, source, &child_ctx, result);
            }
        }
        "enum_specifier" => {
            extract_enum(node, source, ctx, result);
        }
        "call_expression" => {
            extract_call(node, source, ctx, result);
            // Recurse to handle nested calls in arguments.
            visit_children(node, source, ctx, result);
        }
        "init_declarator" => {
            // A local `int x = 1;` writes the declarator's identifier
            // (BR-TRACE-006). Only attribute the write when inside a function
            // body (current_func is Some).
            if let Some(func) = ctx.current_func {
                if let Some(name) = declarator_name(node, source) {
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
            visit_children(node, source, ctx, result);
        }
        "assignment_expression" => {
            // `x = ...;` writes the left-hand identifier (BR-TRACE-006). Only
            // simple identifier targets are captured; field/index writes are
            // ignored.
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
        "identifier" => {
            // A bare identifier in an expression position is a variable read
            // (BR-TRACE-005). Name-defining positions (declarators, call
            // functions, assignment left) are excluded by `is_read_position`.
            if let Some(func) = ctx.current_func {
                if is_read_position(node) {
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
        "linkage_specification" => {
            // extern "C" { ... } blocks: recurse to find function definitions.
            visit_children(node, source, ctx, result);
        }
        _ => {
            // Recurse into other nodes to find nested definitions/calls.
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
    parent: Option<&str>,
    result: &mut ExtractResult,
) {
    let Some(name) = function_name(node, source) else {
        return;
    };
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    let signature = declarator_signature(node, source);
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, parent),
        start_line,
        result,
    );
    let mut builder = ModelNode::builder(NodeLabel::Function, name.clone(), qn.clone())
        .file_path(ctx.file_path)
        .start_line(start_line)
        .end_line(end_line)
        .language(Language::C)
        .project(ctx.project)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_global_var(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // Only treat as global var if at the top level (parent is translation_unit).
    let is_top_level = node
        .parent()
        .map(|p| p.kind() == "translation_unit")
        .unwrap_or(false);
    if !is_top_level {
        return;
    }
    // P1-2: Detect function declarations in headers (e.g. `int foo(int x);`).
    // Walk named children for `function_declarator` (direct or wrapped in
    // `pointer_declarator`). If found, create Function nodes and skip global
    // var extraction to avoid double-counting the same declarator.
    let mut has_function_decl = false;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if let Some(fd) = find_function_declarator(child) {
                extract_function_declaration(fd, node, source, ctx, result);
                has_function_decl = true;
            }
        }
    }
    if has_function_decl {
        return;
    }
    // A declaration may declare multiple variables; extract each declarator.
    let mut i: u32 = 0;
    while i < node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "init_declarator" {
                if let Some(name) = declarator_name(child, source) {
                    push_global_var(
                        &name,
                        node.start_position().row as u32 + 1,
                        ctx.file_path,
                        ctx.project,
                        result,
                    );
                }
            }
        }
        i += 1;
    }
    // If no init_declarator, check for plain declarator children.
    let has_init = (0..node.named_child_count() as u32)
        .any(|i| node.named_child(i).map(|c| c.kind() == "init_declarator").unwrap_or(false));
    if !has_init {
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                if child.kind() == "identifier" {
                    if let Some(name) = node_text(child, source).map(String::from) {
                        push_global_var(
                            &name,
                            node.start_position().row as u32 + 1,
                            ctx.file_path,
                            ctx.project,
                            result,
                        );
                    }
                }
            }
        }
    }
}

fn push_global_var(name: &str, line: u32, file_path: &str, project: &str, result: &mut ExtractResult) {
    let qn = dedupe_qn(make_qn(file_path, name, project, None), line, result);
    let model_node = ModelNode::builder(NodeLabel::GlobalVar, name.to_string(), qn.clone())
        .file_path(file_path)
        .start_line(line)
        .language(Language::C)
        .project(project)
        .is_global(true)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
}

/// P1-2: Creates a [`NodeLabel::Function`] node for a function declaration
/// found in a header (e.g. `int foo(int x);`). The `fd_node` is the
/// `function_declarator` child of the parent `declaration` node; `decl_node`
/// is the parent `declaration` (used for line range and signature text).
fn extract_function_declaration(
    fd_node: Node,
    decl_node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name) = declarator_name(fd_node, source) else {
        return;
    };
    let start_line = decl_node.start_position().row as u32 + 1;
    let end_line = decl_node.end_position().row as u32 + 1;
    let signature = node_text(fd_node, source).map(String::from);
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        start_line,
        result,
    );
    let mut builder = ModelNode::builder(NodeLabel::Function, name.clone(), qn)
        .file_path(ctx.file_path)
        .start_line(start_line)
        .end_line(end_line)
        .language(Language::C)
        .project(ctx.project)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

/// P1-2: Recursively unwraps declarator wrappers to find a `function_declarator`
/// child, if any. Handles `int *foo(int x);` (pointer_declarator wrapping
/// function_declarator) and `int (foo)(int x);` (parenthesized_declarator).
fn find_function_declarator(node: Node) -> Option<Node> {
    match node.kind() {
        "function_declarator" => Some(node),
        "pointer_declarator" | "parenthesized_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            find_function_declarator(inner)
        }
        _ => None,
    }
}

fn extract_typedef(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // type_definition has a `type` field and a `declarator` field (type_identifier).
    // Walk all children for type_identifier nodes.
    // Also detect anonymous struct/enum unions inside typedef (P1-1):
    //   typedef struct { int x; } Name;  → create Struct node "Name"
    //   typedef enum { A, B } Name;       → create Enum node "Name"
    let mut typedef_name: Option<String> = None;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "type_identifier" {
                if let Some(name) = node_text(child, source).map(String::from) {
                    let line = node.start_position().row as u32 + 1;
                    let qn = dedupe_qn(
                        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
                        line,
                        result,
                    );
                    let model_node = ModelNode::builder(NodeLabel::Typedef, name.clone(), qn)
                        .file_path(ctx.file_path)
                        .start_line(line)
                        .language(Language::C)
                        .project(ctx.project)
                        .is_global(true)
                        .build();
                    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
                    result.nodes.push(model_node);
                    typedef_name = Some(name);
                }
            }
        }
    }
    // If there is an anonymous struct/enum inside, create a Struct/Enum node
    // using the typedef name (P1-1).
    if let Some(name) = typedef_name {
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                match child.kind() {
                    "struct_specifier" if child.child_by_field_name("name").is_none()
                        && child.child_by_field_name("body").is_some() =>
                    {
                        let qn = dedupe_qn(
                            make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
                            child.start_position().row as u32 + 1,
                            result,
                        );
                        let model_node = ModelNode::builder(NodeLabel::Struct, name.clone(), qn)
                            .file_path(ctx.file_path)
                            .start_line(child.start_position().row as u32 + 1)
                            .end_line(child.end_position().row as u32 + 1)
                            .language(Language::C)
                            .project(ctx.project)
                            .is_global(true)
                            .build();
                        add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
                        result.nodes.push(model_node);
                    }
                    "enum_specifier" if child.child_by_field_name("name").is_none()
                        && child.child_by_field_name("body").is_some() =>
                    {
                        let qn = dedupe_qn(
                            make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
                            child.start_position().row as u32 + 1,
                            result,
                        );
                        let model_node = ModelNode::builder(NodeLabel::Enum, name.clone(), qn)
                            .file_path(ctx.file_path)
                            .start_line(child.start_position().row as u32 + 1)
                            .end_line(child.end_position().row as u32 + 1)
                            .language(Language::C)
                            .project(ctx.project)
                            .is_global(true)
                            .build();
                        add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
                        result.nodes.push(model_node);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn extract_struct(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // Only extract if the struct has a name and a body.
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    if node.child_by_field_name("body").is_none() {
        return;
    }
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Struct, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::C)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_enum(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    if node.child_by_field_name("body").is_none() {
        return;
    }
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Enum, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::C)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_include(node: Node, source: &str, result: &mut ExtractResult) {
    // preproc_include has a `path` field that is either system_lib_string
    // (<stdio.h>) or string_literal ("myheader.h").
    let Some(path_node) = node.child_by_field_name("path") else {
        return;
    };
    let raw = node_text(path_node, source).unwrap_or("");
    // Strip surrounding quotes/angle brackets.
    let cleaned = raw
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_start_matches('"')
        .trim_end_matches('"')
        .to_string();
    result.imports.push(ImportInfo {
        source_file: cleaned,
        imported_names: Vec::new(),
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
    result.calls.push(super::extractor::CallInfo {
        caller_qn,
        callee_name: callee,
        line: node.start_position().row as u32 + 1,
        args,
    });
}

// ---------------------------------------------------------------------------
// Name / signature helpers
// ---------------------------------------------------------------------------

fn function_name(node: Node, source: &str) -> Option<String> {
    // function_definition has a `declarator` field (function_declarator).
    let declarator = node.child_by_field_name("declarator")?;
    declarator_name(declarator, source)
}

/// Recursively unwraps declarator nodes (function_declarator, pointer_declarator,
/// etc.) to find the inner identifier.
fn declarator_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "function_declarator"
        | "pointer_declarator"
        | "array_declarator"
        | "parenthesized_declarator"
        | "init_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            declarator_name(inner, source)
        }
        _ => None,
    }
}

fn declarator_signature(node: Node, source: &str) -> Option<String> {
    // Use the declarator text as the signature.
    let declarator = node.child_by_field_name("declarator")?;
    node_text(declarator, source).map(String::from)
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" => node_text(node, source).map(String::from),
        "field_expression" => {
            // e.g. obj.method() -> "method"
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

/// Returns the text of `node` if it is a plain `identifier`, else `None`.
fn identifier_text(node: Node, source: &str) -> Option<String> {
    if node.kind() == "identifier" {
        node_text(node, source).map(String::from)
    } else {
        None
    }
}

/// Returns `true` if a bare `identifier` node sits in a read (expression)
/// position rather than a name-defining position (declarator, callee, write
/// target). Minimal version: only checks the direct parent kind, matching the
/// rust_extractor convention (design.md Decision 4, Open Question 2).
fn is_read_position(node: Node) -> bool {
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
        | "conditional_expression" => true,
        // `foo(x)` -> the callee `foo` is not a read; arguments are handled
        // above via the `argument_list` parent.
        "call_expression" => !is_at_field(node, parent, "function"),
        // `x = y;` -> `y` (the right side) is a read; `x` (the left) is not.
        "assignment_expression" => !is_at_field(node, parent, "left"),
        // `obj.field` -> `obj` (the object) is a read; the field name is a
        // `field_identifier`, not a plain `identifier`, so it is not reached
        // here, but guard explicitly for safety.
        "field_expression" => is_at_field(node, parent, "argument"),
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
    FqnGenerator::generate(project, file_path, name, Language::C, parent)
}

/// 同文件同名定义消歧：若 result.nodes 已有相同 qn，追加行号消歧符 `#L{line}`。
/// 用于条件编译（#ifdef/#else）导致的同名 typedef/global_var/struct/enum。
fn dedupe_qn(qn: String, line: u32, result: &ExtractResult) -> String {
    if result.nodes.iter().any(|n| n.qualified_name == qn) {
        format!("{qn}#L{line}")
    } else {
        qn
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
    // CONTAINS edge: file -> definition
    result.edges.push(Edge::new(
        file_path.to_string(),
        node.id.clone(),
        EdgeType::Contains,
        project,
    ));
    // DEFINES edge: file -> definition
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

    const C_SOURCE: &str = r#"#include <stdio.h>
#include "myheader.h"
typedef int my_int;
int global_var = 42;
int add(int a, int b) {
    return a + b;
}
int main() {
    int result = add(1, 2);
    printf("hello");
    return result;
}
"#;

    fn extract(source: &str) -> ExtractResult {
        let ext = CExtractor::new();
        ext.extract(source, "test.c", "proj").expect("extraction should succeed")
    }

    #[test]
    fn language_returns_c() {
        assert_eq!(CExtractor::new().language(), Language::C);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = CExtractor::default();
        assert_eq!(ext.language(), Language::C);
    }

    #[test]
    fn extracts_two_includes() {
        let result = extract(C_SOURCE);
        assert_eq!(result.imports.len(), 2, "should extract 2 #include directives");
        assert_eq!(result.imports[0].source_file, "stdio.h");
        assert_eq!(result.imports[1].source_file, "myheader.h");
        assert_eq!(result.imports[0].line, 1);
        assert_eq!(result.imports[1].line, 2);
    }

    #[test]
    fn extracts_typedef() {
        let result = extract(C_SOURCE);
        let typedefs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Typedef).collect();
        assert_eq!(typedefs.len(), 1, "should extract 1 typedef");
        assert_eq!(typedefs[0].name, "my_int");
        assert_eq!(typedefs[0].start_line, Some(3));
        assert_eq!(typedefs[0].language, Some(Language::C));
        assert_eq!(typedefs[0].project, "proj");
        assert_eq!(typedefs[0].file_path.as_deref(), Some("test.c"));
        assert!(typedefs[0].is_global);
    }

    #[test]
    fn typedef_duplicate_name_disambiguated() {
        // 模拟 flex 生成代码的 #ifdef/#else 同名 typedef（WRF lexer.c 碰撞）。
        // tree-sitter 不处理预处理器，会同时看到两个 typedef 节点。
        let src = r#"typedef int flex_uint16_t;
typedef unsigned short int flex_uint16_t;
"#;
        let result = extract(src);
        let typedefs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Typedef && n.name == "flex_uint16_t")
            .collect();
        assert_eq!(typedefs.len(), 2, "should extract 2 typedefs");
        // 第一个无消歧符
        assert!(
            !typedefs[0].qualified_name.contains('#'),
            "first typedef should have no disambiguator: {}",
            typedefs[0].qualified_name
        );
        // 第二个含 #L 行号消歧符
        assert!(
            typedefs[1].qualified_name.contains("#L"),
            "second typedef should have #L disambiguator: {}",
            typedefs[1].qualified_name
        );
        // 两个 FQN 不同
        assert_ne!(
            typedefs[0].qualified_name, typedefs[1].qualified_name,
            "FQNs must differ to avoid collision"
        );
    }

    #[test]
    fn extracts_global_var() {
        let result = extract(C_SOURCE);
        let globals: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::GlobalVar)
            .collect();
        assert_eq!(globals.len(), 1, "should extract 1 global variable");
        assert_eq!(globals[0].name, "global_var");
        assert_eq!(globals[0].start_line, Some(4));
        assert_eq!(globals[0].language, Some(Language::C));
    }

    #[test]
    fn extracts_functions() {
        let result = extract(C_SOURCE);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 2, "should extract 2 functions (add, main)");
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"add"));
        assert!(names.contains(&"main"));
    }

    #[test]
    fn function_has_signature_and_lines() {
        let result = extract(C_SOURCE);
        let add = result
            .nodes
            .iter()
            .find(|n| n.name == "add")
            .expect("add function should exist");
        assert_eq!(add.start_line, Some(5));
        assert_eq!(add.end_line, Some(7));
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn extracts_calls() {
        let result = extract(C_SOURCE);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(callees.contains(&"add"), "should extract call to add");
        assert!(callees.contains(&"printf"), "should extract call to printf");
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(C_SOURCE);
        let add_call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "add")
            .expect("call to add should exist");
        assert_eq!(add_call.line, 9);
        assert_eq!(add_call.args.len(), 2, "add(1, 2) should have 2 args");
    }

    #[test]
    fn creates_contains_and_defines_edges() {
        let result = extract(C_SOURCE);
        let contains_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Contains).count();
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(contains_count, node_count, "each node should have a CONTAINS edge");
        assert_eq!(defines_count, node_count, "each node should have a DEFINES edge");
    }

    #[test]
    fn edges_reference_file_and_node_ids() {
        let result = extract(C_SOURCE);
        for edge in &result.edges {
            assert_eq!(edge.source, "test.c", "edge source should be the file path");
            assert!(!edge.target.is_empty(), "edge target should be a node id");
            assert_eq!(edge.project, "proj");
        }
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract(C_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert_eq!(add.qualified_name, "proj.test.c.add");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.nodes.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.calls.is_empty());
        assert!(result.is_empty());
    }

    #[test]
    fn extracts_struct_definition() {
        let src = "struct Point { int x; int y; };";
        let result = extract(src);
        let structs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Struct).collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Point");
    }

    #[test]
    fn extracts_enum_definition() {
        let src = "enum Color { RED, GREEN, BLUE };";
        let result = extract(src);
        let enums: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Enum).collect();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Color");
    }

    #[test]
    fn struct_without_body_is_not_extracted() {
        // `struct Point p;` is a declaration, not a definition.
        let src = "struct Point p;";
        let result = extract(src);
        let structs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Struct).collect();
        assert_eq!(structs.len(), 0, "struct without body should not be extracted");
    }

    #[test]
    fn handles_pointer_function_declarator() {
        let src = "int* alloc(int n) { return 0; }";
        let result = extract(src);
        let funcs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Function).collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "alloc");
    }

    #[test]
    fn handles_extern_linkage_block() {
        let src = r#"extern "C" {
            int c_func(int x);
        }"#;
        let result = extract(src);
        // The function declaration inside extern block should be found via recursion.
        // (It's a declaration, not a definition, so no Function node, but no crash.)
        // Verify no panic occurs and the result is returned.
        assert_eq!(result.language, Language::C);
    }

    #[test]
    fn multiple_global_vars_in_one_declaration() {
        let src = "int a = 1, b = 2, c = 3;";
        let result = extract(src);
        let globals: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::GlobalVar)
            .collect();
        assert_eq!(globals.len(), 3, "should extract 3 global variables");
        let names: Vec<_> = globals.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn nested_call_expressions() {
        let src = "int main() { printf(format_str(add(1))); }";
        let result = extract(src);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(callees.contains(&"printf"), "should find printf call");
        assert!(callees.contains(&"add"), "should find nested add call");
    }

    #[test]
    fn field_expression_call() {
        let src = "int main() { obj.method(); }";
        let result = extract(src);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(callees.contains(&"method"), "should extract method name from field expression");
    }

    #[test]
    fn result_language_is_c() {
        let result = extract(C_SOURCE);
        assert_eq!(result.language, Language::C);
        assert_eq!(result.file_path, "test.c");
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        // Spec: C 函数内调用生成非 None caller_qn (点分 FQN 格式)。
        let src = "int caller(void) {\n    callee();\n    return 0;\n}\n";
        let ext = CExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.c", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.c.caller"),
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
    fn read_in_function_has_dotted_fqn_reader_qn() {
        // Spec: C 函数内 identifier 读取提取 (BR-TRACE-005)。
        let src = "int caller(int x) {\n    return x;\n}\n";
        let ext = CExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.c", "proj")
            .expect("extraction should succeed");
        let read = result
            .reads
            .iter()
            .find(|r| r.var_name == "x")
            .expect("should find a read of x");
        assert_eq!(
            read.reader_qn.as_deref(),
            Some("proj.tmp.demo.main.c.caller"),
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
    fn write_in_function_init_declarator_has_dotted_fqn_writer_qn() {
        // Spec: C 函数内 init_declarator 写入提取 (BR-TRACE-006)。
        let src = "void caller(void) {\n    int y = 1;\n}\n";
        let ext = CExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.c", "proj")
            .expect("extraction should succeed");
        let write = result
            .writes
            .iter()
            .find(|w| w.var_name == "y")
            .expect("should find a write of y");
        assert_eq!(write.var_name, "y");
        assert_eq!(
            write.writer_qn.as_deref(),
            Some("proj.tmp.demo.main.c.caller"),
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
        // Spec: C 函数内 assignment_expression 写入提取 (BR-TRACE-006)。
        let src = "void caller(void) {\n    int y;\n    y = 2;\n}\n";
        let ext = CExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.c", "proj")
            .expect("extraction should succeed");
        // `int y;` declares y (no init_declarator, so no write from declaration);
        // `y = 2;` is an assignment_expression write.
        let assignment_write = result
            .writes
            .iter()
            .find(|w| w.var_name == "y")
            .expect("should find a write of y from assignment");
        assert_eq!(
            assignment_write.writer_qn.as_deref(),
            Some("proj.tmp.demo.main.c.caller"),
            "writer_qn should be the dotted FQN of the enclosing function"
        );
    }

    #[test]
    fn declaration_position_identifier_not_a_read() {
        // Spec: C 声明位置的 identifier 不被误识别为读取。
        // `int x = 1;` inside a function: x is the declarator (write target),
        // not a read. x MUST NOT appear in ReadInfo.
        let src = "void caller(void) {\n    int x = 1;\n}\n";
        let ext = CExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.c", "proj")
            .expect("extraction should succeed");
        let x_reads: Vec<_> = result.reads.iter().filter(|r| r.var_name == "x").collect();
        assert!(
            x_reads.is_empty(),
            "declarator-position x must NOT appear in ReadInfo: {:?}",
            x_reads
        );
        // x should appear as a WriteInfo target (init_declarator write).
        let x_writes: Vec<_> = result.writes.iter().filter(|w| w.var_name == "x").collect();
        assert!(
            !x_writes.is_empty(),
            "declarator-position x SHOULD appear in WriteInfo"
        );
    }

    #[test]
    fn top_level_declaration_no_reads_or_writes() {
        // Spec: C 顶层声明的 identifier 不生成读写记录 (current_func 为 None)。
        let src = "int g = 0;\n";
        let ext = CExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.c", "proj")
            .expect("extraction should succeed");
        assert!(
            result.reads.is_empty(),
            "top-level declaration must not produce ReadInfo: {:?}",
            result.reads
        );
        assert!(
            result.writes.is_empty(),
            "top-level declaration must not produce WriteInfo: {:?}",
            result.writes
        );
    }

    #[test]
    fn cpp_overloaded_methods_get_line_disambiguator() {
        let src = "\
class Container {
public:
    int* begin() { return data_; }
    const int* begin() const { return data_; }
    int* end() { return data_ + size_; }
    const int* end() const { return data_ + size_; }
private:
    int data_[10];
    int size_ = 0;
};
";
        let result = extract(src);
        let ends: Vec<_> = result.nodes.iter().filter(|n| n.name == "end").collect();
        assert_eq!(ends.len(), 2, "should extract two `end` methods");
        assert_ne!(ends[0].qualified_name, ends[1].qualified_name);
        for e in &ends {
            assert!(
                e.qualified_name.contains("#Container_L"),
                "expected #Container_L<line>: {}",
                e.qualified_name
            );
        }
    }
}
