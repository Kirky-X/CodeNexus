//! Python language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-python's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_definition` (top-level) → [`NodeLabel::Function`]
//! - `function_definition` (inside class) → [`NodeLabel::Method`]
//! - `class_definition` → [`NodeLabel::Class`]
//!
//! Note: nested `function_definition` (def inside another def) is NOT promoted
//! to a Function node — its body is still traversed for calls/reads. This
//! aligns with gitnexus which only indexes module-level functions as Function
//! (P2-5 fix: previously over-extracted 280 vs gitnexus 170).
//!
//! # Extracted records
//!
//! - `import_statement` / `import_from_statement` → [`ImportInfo`]
//! - `call` → [`CallInfo`]
//! - `assignment` → [`AssignInfo`]

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{AssignInfo, CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// Python language tree-sitter extractor (Adapter pattern).
pub struct PythonExtractor {
    _priv: (),
}

impl PythonExtractor {
    /// Creates a new `PythonExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for PythonExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for PythonExtractor {
    fn language(&self) -> Language {
        Language::Python
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Python);
        // TODO: implement reads/writes extraction for Python (BR-TRACE-005/006).
        // `result.reads` and `result.writes` are left empty for now; downstream
        // resolution gracefully produces no Reads/Writes edges when absent.
        let mut parser = ParserFactory::create_parser(Language::Python)?;
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
    current_parent: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function_definition" => {
            extract_function(node, source, ctx, result);
            // Pass the function's name as the enclosing function for body
            // traversal, so calls inside it can be attributed to it.
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
        "class_definition" => {
            extract_class(node, source, ctx, result);
            // 把类名纳入 current_parent，使不同类的同名方法生成不同 FQN
            // （修复 P0 python-static-class-methods 碰撞）。
            let class_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            let combined = combine_scope(ctx.current_parent, class_name.as_deref());
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: None,
                current_parent: combined.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "import_statement" => {
            extract_import(node, source, result);
        }
        "import_from_statement" => {
            extract_import_from(node, source, result);
        }
        "call" => {
            extract_call(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "assignment" => {
            extract_assignment(node, source, result);
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
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    // Determine if this is a method (inside a class) or a function.
    let is_method = is_inside_class(node);
    // P2-5: skip nested `def` (def inside another def) — its body is still
    // traversed by visit_children, but no Function node is created. This
    // aligns with gitnexus which does not index nested defs (codenexus
    // previously over-extracted 280 vs gitnexus 170). Functions inside
    // if/try/with blocks at module scope ARE still indexed (only direct
    // function_definition ancestors trigger the skip).
    if !is_method && has_ancestor_function(node) {
        return;
    }
    let label = if is_method {
        NodeLabel::Method
    } else {
        NodeLabel::Function
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = function_signature(node, source);
    let mut builder = ModelNode::builder(label, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Python)
        .project(ctx.project)
        .is_global(!is_method);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_class(
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
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Class, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Python)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.nodes.push(model_node);

    // P2-2: Create EXTENDS edges for each base class.
    // `class Child(Parent1, Parent2):` → superclasses field is an
    // argument_list whose named children are the base expressions.
    // Target is a best-effort FQN based on the current file/scope; cross-file
    // base classes may not resolve until a future resolver enhancement.
    if let Some(superclasses) = node.child_by_field_name("superclasses") {
        for i in 0..superclasses.named_child_count() as u32 {
            if let Some(base) = superclasses.named_child(i) {
                // Skip keyword arguments like `metaclass=Meta`.
                if base.kind() == "keyword_argument" {
                    continue;
                }
                if let Some(parent_name) = base_class_name(base, source) {
                    let parent_qn = make_qn(
                        ctx.file_path,
                        &parent_name,
                        ctx.project,
                        ctx.current_parent,
                    );
                    result.edges.push(Edge::new(
                        qn.clone(),
                        parent_qn,
                        EdgeType::Extends,
                        ctx.project,
                    ));
                }
            }
        }
    }
}

/// P2-2: Extracts the base class name from a `superclasses` entry.
/// Handles plain identifiers (`Foo`), attribute access (`module.Foo`), and
/// call expressions (`Meta()` used as a base).
fn base_class_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "attribute" => {
            // `module.BaseClass` → use the attribute (rightmost) name.
            let attr = node.child_by_field_name("attribute")?;
            node_text(attr, source).map(String::from)
        }
        "call" => {
            // `Meta()` as a base — unwrap to the function identifier.
            let func = node.child_by_field_name("function")?;
            base_class_name(func, source)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    // import_statement has one or more dotted_name children.
    // e.g. `import os` -> dotted_name "os"
    // e.g. `import os.path` -> dotted_name "os.path"
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "dotted_name" {
                if let Some(name) = dotted_name_text(child, source) {
                    result.imports.push(ImportInfo {
                        source_file: name,
                        imported_names: Vec::new(),
                        line: node.start_position().row as u32 + 1,
                    });
                }
            }
        }
    }
}

fn extract_import_from(node: Node, source: &str, result: &mut ExtractResult) {
    // import_from_statement: `from typing import List, Dict`
    // The first dotted_name is the module, subsequent ones are imported names.
    let mut source_module = None;
    let mut names = Vec::new();
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "dotted_name" {
                if source_module.is_none() {
                    source_module = dotted_name_text(child, source);
                } else if let Some(n) = dotted_name_text(child, source) {
                    names.push(n);
                }
            } else if child.kind() == "aliased_import" {
                // e.g. `import numpy as np` in a from import
                if let Some(name) = aliased_import_name(child, source) {
                    names.push(name);
                }
            } else if child.kind() == "wildcard_import" {
                // `from module import *`
                names.push("*".to_string());
            }
        }
    }
    let Some(source_module) = source_module else {
        return;
    };
    result.imports.push(ImportInfo {
        source_file: source_module,
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
        .map(|name| make_qn(ctx.file_path, name, ctx.project, None));
    result.calls.push(CallInfo {
        caller_qn,
        callee_name: callee,
        line: node.start_position().row as u32 + 1,
        args,
    });
}

fn extract_assignment(node: Node, source: &str, result: &mut ExtractResult) {
    let Some(left_node) = node.child_by_field_name("left") else {
        return;
    };
    let Some(target) = assignment_target_name(left_node, source) else {
        return;
    };
    let right_node = node.child_by_field_name("right");
    let (source_name, is_return_assign) = match right_node {
        Some(v) => {
            let is_call = v.kind() == "call";
            let name = if is_call {
                v.child_by_field_name("function")
                    .and_then(|f| callee_name(f, source))
                    .unwrap_or_default()
            } else {
                // Only capture simple identifiers/attributes as source names.
                // Complex expressions (subscripts, binary ops, etc.) would
                // produce FQNs with invalid characters (brackets, quotes).
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

/// Returns the enclosing scope kind for a function_definition:
/// - `Class` if the function is (anywhere) inside a class_definition body
///   (methods, including those nested in if/try/with blocks inside a class).
/// - `Function` if the function is nested inside another function_definition.
/// - `Module` if the function is at module top level.
///
/// The walk stops at the first `class_definition` or `function_definition`
/// ancestor encountered — a function inside a method counts as nested
/// (Function scope), not as a class method.
enum FunctionScope {
    Class,
    Function,
    Module,
}

fn function_scope(node: Node) -> FunctionScope {
    let mut cur = node.parent();
    while let Some(p) = cur {
        match p.kind() {
            "class_definition" => return FunctionScope::Class,
            "function_definition" => return FunctionScope::Function,
            _ => cur = p.parent(),
        }
    }
    FunctionScope::Module
}

/// Backwards-compatible predicate kept for any external callers.
fn is_inside_class(node: Node) -> bool {
    matches!(function_scope(node), FunctionScope::Class)
}

/// Returns true if the node has a `function_definition` ancestor (i.e. it is
/// nested inside another function). Used by P2-5 to skip nested defs.
fn has_ancestor_function(node: Node) -> bool {
    matches!(function_scope(node), FunctionScope::Function)
}

fn function_signature(node: Node, source: &str) -> Option<String> {
    // Use the first line of the function as the signature (def line).
    let start = node.start_position();
    let end = node.end_position();
    if start.row == end.row {
        node_text(node, source).map(String::from)
    } else {
        // Extract just the `def name(params):` part from the first line.
        let line_end = source
            .lines()
            .nth(start.row)
            .map(|l| l.len())
            .unwrap_or(0);
        let start_byte = node.start_byte();
        let line_end_byte = start_byte + line_end;
        if line_end_byte <= source.len() {
            Some(source[start_byte..line_end_byte].to_string())
        } else {
            node_text(node, source).map(String::from)
        }
    }
}

fn dotted_name_text(node: Node, source: &str) -> Option<String> {
    // A dotted_name is composed of identifier children joined by dots.
    let text = node_text(node, source)?;
    Some(text.to_string())
}

fn aliased_import_name(node: Node, source: &str) -> Option<String> {
    // aliased_import has a `name` field (the original) and an `alias` field.
    if let Some(alias) = node.child_by_field_name("alias") {
        return node_text(alias, source).map(String::from);
    }
    node.child_by_field_name("name")
        .and_then(|n| node_text(n, source).map(String::from))
}

fn callee_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "attribute" => {
            // e.g. `obj.method()` -> extract the attribute name.
            let attr = node.child_by_field_name("attribute")?;
            node_text(attr, source).map(String::from)
        }
        "call" => {
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
        "identifier" => node_text(node, source).map(String::from),
        "attribute" => {
            // e.g. `self.x = ...` -> extract "x"
            let attr = node.child_by_field_name("attribute")?;
            node_text(attr, source).map(String::from)
        }
        "tuple" | "list" | "pattern_list" => {
            // Extract the first identifier in the tuple.
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if let Some(name) = assignment_target_name(child, source) {
                        return Some(name);
                    }
                }
            }
            None
        }
        "subscript" => {
            // e.g. `arr[0] = ...` -> extract "arr"
            let value = node.child_by_field_name("value")?;
            assignment_target_name(value, source)
        }
        _ => {
            // Fallback: only accept simple identifier text. Complex
            // expressions (calls, binary ops, etc.) would produce FQNs
            // with invalid characters (brackets, quotes, commas) that
            // corrupt CSV imports.
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
    FqnGenerator::generate(project, file_path, name, Language::Python, parent)
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

/// Disambiguate FQN by appending `#L{line}` when the same FQN already exists
/// in `result.nodes`. Handles same-name methods/functions in the same scope.
/// Mirrors the helper in c.rs / fortran.rs.
fn dedupe_qn(qn: String, line: u32, result: &ExtractResult) -> String {
    if result.nodes.iter().any(|n| n.qualified_name == qn) {
        format!("{qn}#L{line}")
    } else {
        qn
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

    const PYTHON_SOURCE: &str = r#"import os
from typing import List

def add(a, b):
    return a + b

class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    
    def distance(self):
        return self.x + self.y

result = add(1, 2)
"#;

    fn extract(source: &str) -> ExtractResult {
        let ext = PythonExtractor::new();
        ext.extract(source, "test.py", "proj").expect("extraction should succeed")
    }

    #[test]
    fn language_returns_python() {
        assert_eq!(PythonExtractor::new().language(), Language::Python);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = PythonExtractor::default();
        assert_eq!(ext.language(), Language::Python);
    }

    #[test]
    fn extracts_imports() {
        let result = extract(PYTHON_SOURCE);
        assert_eq!(result.imports.len(), 2, "should extract 2 imports");
        assert_eq!(result.imports[0].source_file, "os");
        assert_eq!(result.imports[1].source_file, "typing");
        assert!(
            result.imports[1]
                .imported_names
                .contains(&"List".to_string()),
            "from typing import List should have List in imported_names"
        );
    }

    #[test]
    fn extracts_top_level_function() {
        let result = extract(PYTHON_SOURCE);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 1, "should extract 1 top-level function (add)");
        assert_eq!(funcs[0].name, "add");
        assert_eq!(funcs[0].language, Some(Language::Python));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.py"));
        assert!(funcs[0].is_global, "top-level function should be global");
    }

    #[test]
    fn extracts_class() {
        let result = extract(PYTHON_SOURCE);
        let classes: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Class).collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Point");
    }

    #[test]
    fn p2_2_extends_edge_for_single_inheritance() {
        let src = r#"class Parent:
    pass

class Child(Parent):
    pass
"#;
        let result = extract(src);
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(extends.len(), 1, "should create 1 EXTENDS edge: {:?}", extends);
        // Source = Child FQN, Target = Parent FQN (best-effort, same file).
        assert!(
            extends[0].source.contains("Child"),
            "EXTENDS source should be Child FQN: {}",
            extends[0].source
        );
        assert!(
            extends[0].target.contains("Parent"),
            "EXTENDS target should be Parent FQN: {}",
            extends[0].target
        );
    }

    #[test]
    fn p2_2_extends_edge_for_multiple_bases() {
        let src = r#"class Base1:
    pass
class Base2:
    pass
class Derived(Base1, Base2):
    pass
"#;
        let result = extract(src);
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(extends.len(), 2, "should create 2 EXTENDS edges: {:?}", extends);
    }

    #[test]
    fn p2_2_extends_edge_skips_keyword_argument() {
        let src = r#"class Meta:
    pass
class Foo(metaclass=Meta):
    pass
"#;
        let result = extract(src);
        let extends: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        // `metaclass=Meta` is a keyword_argument, not a base class.
        assert_eq!(extends.len(), 0, "should skip keyword_argument: {:?}", extends);
    }

    #[test]
    fn extracts_methods() {
        let result = extract(PYTHON_SOURCE);
        let methods: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Method).collect();
        let names: Vec<_> = methods.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"__init__"), "should extract __init__ method: {:?}", names);
        assert!(names.contains(&"distance"), "should extract distance method: {:?}", names);
        assert!(!methods[0].is_global, "methods should not be global");
    }

    #[test]
    fn extracts_call_to_add() {
        let result = extract(PYTHON_SOURCE);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"add"),
            "should extract call to add: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(PYTHON_SOURCE);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "add")
            .expect("call to add should exist");
        assert_eq!(call.line, 15);
        assert_eq!(call.args.len(), 2, "add(1, 2) should have 2 args");
    }

    #[test]
    fn extracts_assignment() {
        let result = extract(PYTHON_SOURCE);
        let assign = result
            .assignments
            .iter()
            .find(|a| a.target_name == "result")
            .expect("should find `result = add(1, 2)` assignment");
        assert_eq!(assign.source_name, "add");
        assert!(
            assign.is_return_assign,
            "assignment from function call should be return assign"
        );
    }

    #[test]
    fn creates_contains_and_defines_edges() {
        let result = extract(PYTHON_SOURCE);
        let contains_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Contains).count();
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(contains_count, node_count);
        assert_eq!(defines_count, node_count);
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract(PYTHON_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert_eq!(add.qualified_name, "proj.test.py.add");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn function_has_signature() {
        let result = extract(PYTHON_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert!(add.signature.is_some(), "function should have a signature");
        assert!(add.signature.as_deref().unwrap().contains("add"));
    }

    #[test]
    fn handles_from_import_with_multiple_names() {
        let src = "from typing import List, Dict, Optional\n";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "typing");
        assert_eq!(result.imports[0].imported_names.len(), 3);
        assert!(result.imports[0].imported_names.contains(&"List".to_string()));
        assert!(result.imports[0].imported_names.contains(&"Dict".to_string()));
        assert!(result.imports[0].imported_names.contains(&"Optional".to_string()));
    }

    #[test]
    fn handles_wildcard_import() {
        let src = "from os import *\n";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "os");
        assert!(result.imports[0].imported_names.contains(&"*".to_string()));
    }

    #[test]
    fn handles_dotted_import() {
        let src = "import os.path\n";
        let result = extract(src);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "os.path");
    }

    #[test]
    fn handles_method_call() {
        let src = "class A:\n    def foo(self):\n        self.bar()\n";
        let result = extract(src);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(callees.contains(&"bar"), "should extract self.bar() call");
    }

    #[test]
    fn handles_attribute_assignment() {
        let src = "class A:\n    def foo(self):\n        self.x = 5\n";
        let result = extract(src);
        let assign = result
            .assignments
            .iter()
            .find(|a| a.target_name == "x")
            .expect("should find self.x = 5 assignment");
        assert!(!assign.is_return_assign, "5 is not a call");
    }

    #[test]
    fn result_language_is_python() {
        let result = extract(PYTHON_SOURCE);
        assert_eq!(result.language, Language::Python);
        assert_eq!(result.file_path, "test.py");
    }

    #[test]
    fn nested_function_definitions() {
        // P2-5: nested `def inner` (def inside another def) is NOT promoted to
        // a Function node — only the top-level `outer` is. gitnexus applies
        // the same rule (codenexus previously over-extracted 280 vs 170).
        // The inner function's body is still traversed for calls/reads.
        let src = "def outer():\n    def inner():\n        return 1\n    return inner()\n";
        let result = extract(src);
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"outer"), "should extract top-level outer function");
        assert!(
            !names.contains(&"inner"),
            "nested inner function must NOT be promoted to a Function node (P2-5)"
        );
        // The call to inner() inside outer() must still be captured.
        let inner_call = result.calls.iter().find(|c| c.callee_name == "inner");
        assert!(inner_call.is_some(), "call to inner() should still be recorded");
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        // Spec: Python 函数内调用生成非 None caller_qn (点分 FQN 格式)。
        let src = "def caller():\n    callee()\n";
        let ext = PythonExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.py", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.py.caller"),
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
        let src = "callee()\n";
        let ext = PythonExtractor::new();
        let result = ext.extract(src, "main.py", "proj").expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find top-level call to callee");
        assert!(call.caller_qn.is_none(), "top-level call should have None caller_qn");
    }
}
