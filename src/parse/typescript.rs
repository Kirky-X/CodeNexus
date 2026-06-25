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
//! - `interface_declaration` → [`NodeLabel::Trait`] (TS interfaces map to traits)
//! - `enum_declaration` → [`NodeLabel::Enum`]
//! - `type_alias_declaration` → [`NodeLabel::TypeAlias`]
//!
//! # Extracted records
//!
//! - `import_statement` → [`ImportInfo`]
//! - `call_expression` → [`CallInfo`]
//! - `lexical_declaration` / `variable_declaration` → [`AssignInfo`]

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{AssignInfo, CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

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
        // TODO: implement reads/writes extraction for TypeScript (BR-TRACE-005/006).
        // `result.reads` and `result.writes` are left empty for now; downstream
        // resolution gracefully produces no Reads/Writes edges when absent.
        let mut parser = ParserFactory::create_parser(Language::TypeScript)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        for i in 0..root.named_child_count() as u32 {
            if let Some(child) = root.named_child(i) {
                visit_node(child, source, file_path, project, &mut result, None, None);
            }
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
    current_func: Option<&str>,
    current_parent: Option<&str>,
) {
    match node.kind() {
        "function_declaration" => {
            extract_function(node, source, file_path, project, result);
            // Pass the function's name as the enclosing function for body
            // traversal, so calls inside it can be attributed to it.
            let func_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            visit_children(
                node,
                source,
                file_path,
                project,
                result,
                func_name.as_deref(),
                current_parent,
            );
        }
        "class_declaration" => {
            extract_class(node, source, file_path, project, result);
            // Extract the class name and pass it as current_parent so methods
            // inside the class can disambiguate their FQN (ADR-003).
            let class_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            visit_children(
                node,
                source,
                file_path,
                project,
                result,
                current_func,
                class_name.as_deref(),
            );
        }
        "method_definition" => {
            extract_method(node, source, file_path, project, result, current_parent);
            // Pass the method's name as the enclosing function for body
            // traversal, so calls inside it can be attributed to it.
            let func_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            visit_children(
                node,
                source,
                file_path,
                project,
                result,
                func_name.as_deref(),
                current_parent,
            );
        }
        "interface_declaration" => {
            extract_named_item(node, NodeLabel::Trait, source, file_path, project, result);
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
        "enum_declaration" => {
            extract_named_item(node, NodeLabel::Enum, source, file_path, project, result);
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
        "type_alias_declaration" => {
            extract_named_item(node, NodeLabel::TypeAlias, source, file_path, project, result);
        }
        "import_statement" => {
            extract_import(node, source, result);
        }
        "export_statement" => {
            // Recurse into the export to find the declaration inside.
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
        "call_expression" => {
            extract_call(node, source, file_path, project, current_func, current_parent, result);
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
        "lexical_declaration" | "variable_declaration" => {
            extract_variable_declaration(node, source, result);
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
        "variable_declarator" => {
            // When a const/let is assigned an object literal with methods
            // (e.g. `const config = { extractType() {...} }`), pass the
            // variable name as current_parent so methods inside get
            // disambiguated FQNs (ADR-003, Type A collision).
            let var_name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source).map(String::from));
            let parent = var_name.as_deref().or(current_parent);
            visit_children(node, source, file_path, project, result, current_func, parent);
        }
        "assignment_expression" => {
            extract_assignment(node, source, result);
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
        _ => {
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
    }
}

fn visit_children(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
    current_func: Option<&str>,
    current_parent: Option<&str>,
) {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            visit_node(child, source, file_path, project, result, current_func, current_parent);
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
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    let is_exported = is_exported(node);
    let signature = node_text(node, source).map(String::from);
    // ADR-003 (positional fallback): top-level functions (parent is `program`
    // or `export_statement`) use no disambiguator — clean FQN. Nested function
    // declarations (e.g. helper functions inside test callbacks like
    // `describe(() => { function walk() {...} })`) use the line number as a
    // positional disambiguator so same-name nested functions do not collide.
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
    let qn = make_qn(file_path, &name, project, disambiguator.as_deref());
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::TypeScript)
        .project(project)
        .is_exported(is_exported)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
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
    result.nodes.push(model_node);
}

fn extract_method(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
    parent: Option<&str>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    // ADR-003 (positional fallback): when no semantic parent context is
    // available — e.g. methods inside anonymous object literals passed as
    // function arguments like `Object.defineProperty(el, 'boom', { get() {…} })`
    // — use the line number as a positional disambiguator so same-name
    // methods in different anonymous objects get distinct FQNs.
    let disambiguator = match parent {
        Some(p) => Some(p.to_string()),
        None => {
            let line = node.start_position().row as u32 + 1;
            Some(format!("L{line}"))
        }
    };
    let qn = make_qn(file_path, &name, project, disambiguator.as_deref());
    let model_node = ModelNode::builder(NodeLabel::Method, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::TypeScript)
        .project(project)
        .is_global(false)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
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
    result.nodes.push(model_node);
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
    file_path: &str,
    project: &str,
    current_func: Option<&str>,
    current_parent: Option<&str>,
    result: &mut ExtractResult,
) {
    let Some(func_node) = node.child_by_field_name("function") else {
        return;
    };
    let Some(callee) = callee_name(func_node, source) else {
        return;
    };
    let args = call_arguments(node, source);
    let caller_qn = current_func.map(|name| make_qn(file_path, name, project, current_parent));
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

fn extract_variable_declarator(node: Node, source: &str, result: &mut ExtractResult) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(target) = node_text(name_node, source).map(String::from) else {
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
                node_text(v, source).map(String::from).unwrap_or_default()
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
                node_text(v, source).map(String::from).unwrap_or_default()
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

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::TypeScript, parent)
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
    fn creates_contains_and_defines_edges() {
        let result = extract(TS_SOURCE);
        let contains_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Contains).count();
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(contains_count, node_count);
        assert_eq!(defines_count, node_count);
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract(TS_SOURCE);
        let add = result.nodes.iter().find(|n| n.name == "add").unwrap();
        assert_eq!(add.qualified_name, "proj.test.ts.add");
    }

    #[test]
    fn method_has_parent_disambiguator() {
        // ADR-003: class methods must carry the parent class name as a
        // disambiguator so same-name methods in different classes do not
        // collide (e.g. Foo.greet vs Bar.greet).
        let src = "class Foo { greet(): void {} }\nclass Bar { greet(): void {} }\n";
        let result = extract(src);
        let greets: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.name == "greet")
            .collect();
        assert_eq!(greets.len(), 2, "should extract two `greet` methods");
        let fqns: Vec<_> = greets.iter().map(|n| n.qualified_name.as_str()).collect();
        assert!(
            fqns.contains(&"proj.test.ts.greet#Foo"),
            "Foo.greet FQN missing: {fqns:?}"
        );
        assert!(
            fqns.contains(&"proj.test.ts.greet#Bar"),
            "Bar.greet FQN missing: {fqns:?}"
        );
        assert_ne!(greets[0].qualified_name, greets[1].qualified_name);
    }

    #[test]
    fn method_without_parent_uses_line_disambiguator() {
        // ADR-003 (positional fallback): methods in anonymous object literals
        // that are NOT assigned to a variable (e.g. arguments to
        // `Object.defineProperty`) have no semantic parent context. They must
        // be disambiguated by line number so same-name methods (like multiple
        // `get()` getters) do not collide on the primary key.
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
        let gets: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.name == "get")
            .collect();
        assert_eq!(gets.len(), 2, "should extract two `get` methods");
        assert_ne!(
            gets[0].qualified_name, gets[1].qualified_name,
            "line-disambiguated FQNs must be distinct: {:?}",
            gets.iter().map(|n| &n.qualified_name).collect::<Vec<_>>()
        );
        // Each FQN must carry a `#L<line>` suffix.
        for g in &gets {
            assert!(
                g.qualified_name.contains("#L"),
                "expected line disambiguator in FQN: {}",
                g.qualified_name
            );
        }
    }

    #[test]
    fn nested_function_declaration_uses_line_disambiguator() {
        // ADR-003 (positional fallback): nested function declarations (e.g.
        // helper functions inside test callbacks) must use a line-based
        // disambiguator so same-name nested functions do not collide. Top-level
        // functions keep a clean FQN (no disambiguator).
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
        let toplevel = result
            .nodes
            .iter()
            .find(|n| n.name == "topLevel")
            .expect("topLevel function should be extracted");
        assert_eq!(
            toplevel.qualified_name, "proj.test.ts.topLevel",
            "top-level function must have clean FQN (no disambiguator)"
        );
        let helpers: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.name == "helper")
            .collect();
        assert_eq!(helpers.len(), 2, "should extract two `helper` functions");
        assert_ne!(
            helpers[0].qualified_name, helpers[1].qualified_name,
            "line-disambiguated FQNs must be distinct: {:?}",
            helpers.iter().map(|n| &n.qualified_name).collect::<Vec<_>>()
        );
        for h in &helpers {
            assert!(
                h.qualified_name.contains("#L"),
                "expected line disambiguator in nested function FQN: {}",
                h.qualified_name
            );
        }
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
    fn extracts_interface_as_trait() {
        let src = "interface Drawable { draw(): void; }";
        let result = extract(src);
        let traits: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Trait).collect();
        assert_eq!(traits.len(), 1, "interface should map to Trait");
        assert_eq!(traits[0].name, "Drawable");
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
        let src = "export interface Drawable { draw(): void; }";
        let result = extract(src);
        let traits: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Trait).collect();
        assert_eq!(traits.len(), 1);
        assert!(traits[0].is_exported, "exported interface should be marked exported");
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
}
