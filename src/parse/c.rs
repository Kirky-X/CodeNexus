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

use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor, ImportInfo};
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
        // TODO: implement reads/writes extraction for C (BR-TRACE-005/006).
        // `result.reads` and `result.writes` are left empty for now; downstream
        // resolution gracefully produces no Reads/Writes edges when absent.
        let mut parser = ParserFactory::create_parser(Language::C)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        // Walk all named children of the translation_unit.
        for i in 0..root.named_child_count() as u32 {
            let child = match root.named_child(i) {
                Some(c) => c,
                None => continue,
            };
            visit_node(child, source, file_path, project, &mut result);
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tree-walking helpers
// ---------------------------------------------------------------------------

fn visit_node(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
) {
    match node.kind() {
        "function_definition" => {
            extract_function(node, source, file_path, project, result);
            // Recurse into the body to find calls.
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    if child.kind() == "compound_statement" {
                        visit_children(child, source, file_path, project, result);
                    }
                }
            }
        }
        "declaration" => {
            extract_global_var(node, source, file_path, project, result);
            // Always recurse into declarations to find calls inside
            // (e.g. `int x = foo();`).
            visit_children(node, source, file_path, project, result);
        }
        "preproc_include" => {
            extract_include(node, source, result);
        }
        "type_definition" => {
            extract_typedef(node, source, file_path, project, result);
        }
        "struct_specifier" => {
            extract_struct(node, source, file_path, project, result);
            if node.child_by_field_name("body").is_some() {
                visit_children(node, source, file_path, project, result);
            }
        }
        "enum_specifier" => {
            extract_enum(node, source, file_path, project, result);
        }
        "call_expression" => {
            extract_call(node, source, result);
            // Recurse to handle nested calls in arguments.
            visit_children(node, source, file_path, project, result);
        }
        "linkage_specification" => {
            // extern "C" { ... } blocks: recurse to find function definitions.
            visit_children(node, source, file_path, project, result);
        }
        _ => {
            // Recurse into other nodes to find nested definitions/calls.
            visit_children(node, source, file_path, project, result);
        }
    }
}

fn visit_children(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
) {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            visit_node(child, source, file_path, project, result);
        }
    }
}

// ---------------------------------------------------------------------------
// Definition extractors
// ---------------------------------------------------------------------------

fn extract_function(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
) {
    let Some(name) = function_name(node, source) else {
        return;
    };
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    let signature = declarator_signature(node, source);
    let qn = make_qn(file_path, &name);
    let mut builder = ModelNode::builder(NodeLabel::Function, name.clone(), qn.clone())
        .file_path(file_path)
        .start_line(start_line)
        .end_line(end_line)
        .language(Language::C)
        .project(project)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_global_var(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
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
    // A declaration may declare multiple variables; extract each declarator.
    let mut i: u32 = 0;
    while i < node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "init_declarator" {
                if let Some(name) = declarator_name(child, source) {
                    push_global_var(&name, node.start_position().row as u32 + 1, file_path, project, result);
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
                        push_global_var(&name, node.start_position().row as u32 + 1, file_path, project, result);
                    }
                }
            }
        }
    }
}

fn push_global_var(name: &str, line: u32, file_path: &str, project: &str, result: &mut ExtractResult) {
    let qn = make_qn(file_path, name);
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

fn extract_typedef(node: Node, source: &str, file_path: &str, project: &str, result: &mut ExtractResult) {
    // type_definition has a `type` field and a `declarator` field (type_identifier).
    // Walk all children for type_identifier nodes.
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "type_identifier" {
                if let Some(name) = node_text(child, source).map(String::from) {
                    let qn = make_qn(file_path, &name);
                    let model_node = ModelNode::builder(NodeLabel::Typedef, name, qn)
                        .file_path(file_path)
                        .start_line(node.start_position().row as u32 + 1)
                        .language(Language::C)
                        .project(project)
                        .is_global(true)
                        .build();
                    add_definition_edges(file_path, project, &model_node, result);
                    result.nodes.push(model_node);
                }
            }
        }
    }
}

fn extract_struct(node: Node, source: &str, file_path: &str, project: &str, result: &mut ExtractResult) {
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
    let qn = make_qn(file_path, &name);
    let model_node = ModelNode::builder(NodeLabel::Struct, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::C)
        .project(project)
        .is_global(true)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_enum(node: Node, source: &str, file_path: &str, project: &str, result: &mut ExtractResult) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    if node.child_by_field_name("body").is_none() {
        return;
    }
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let qn = make_qn(file_path, &name);
    let model_node = ModelNode::builder(NodeLabel::Enum, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::C)
        .project(project)
        .is_global(true)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
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

fn extract_call(node: Node, source: &str, result: &mut ExtractResult) {
    let Some(func_node) = node.child_by_field_name("function") else {
        return;
    };
    let Some(callee) = callee_name(func_node, source) else {
        return;
    };
    let args = call_arguments(node, source);
    result.calls.push(super::extractor::CallInfo {
        caller_qn: None,
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

fn make_qn(file_path: &str, name: &str) -> String {
    format!("{file_path}::{name}")
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
        assert_eq!(add.qualified_name, "test.c::add");
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
}
