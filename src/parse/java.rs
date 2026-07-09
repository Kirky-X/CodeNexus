// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Java language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-java's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `class_declaration` → [`NodeLabel::Class`]
//! - `interface_declaration` → [`NodeLabel::Interface`]
//! - `enum_declaration` → [`NodeLabel::Enum`]
//! - `method_declaration` → [`NodeLabel::Method`] (enclosing class name used
//!   as disambiguator in the FQN)
//! - `constructor_declaration` → [`NodeLabel::Method`]
//!
//! # Extracted records
//!
//! - `import_declaration` → [`ImportInfo`]
//! - `method_invocation` → [`CallInfo`]
//!
//! # Known limitations
//!
//! - Java generics type parameters are not deeply analyzed (only the type
//!   name is extracted, per the parsing spec Out-of-Scope).
//! - Annotation arguments are not extracted.
//! - Nested classes are extracted but their FQN does not include the outer
//!   class name (only the file path + innermost name).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;
use super::dedupe_qn;

/// Java language tree-sitter extractor (Adapter pattern).
pub struct JavaExtractor {
    _priv: (),
}

impl JavaExtractor {
    /// Creates a new `JavaExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for JavaExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for JavaExtractor {
    fn language(&self) -> Language {
        Language::Java
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Java);
        let mut parser = ParserFactory::create_parser(Language::Java)?;
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
    /// The enclosing class/interface/enum name, used as the FQN disambiguator
    /// for methods so same-name methods in different classes produce distinct
    /// FQNs (ADR-003).
    current_parent: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "class_declaration" => {
            extract_class(node, source, ctx, result, NodeLabel::Class);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "interface_declaration" => {
            extract_class(node, source, ctx, result, NodeLabel::Interface);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "enum_declaration" => {
            extract_class(node, source, ctx, result, NodeLabel::Enum);
            let name = type_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: ctx.current_func,
                current_parent: name.as_deref(),
            };
            visit_children(node, source, &child_ctx, result);
        }
        "method_declaration" | "constructor_declaration" => {
            extract_method(node, source, ctx, result);
            let name = method_name(node, source);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_func: name.as_deref(),
                current_parent: ctx.current_parent,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "import_declaration" => {
            extract_import(node, source, result);
        }
        "method_invocation" => {
            extract_call(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "object_creation_expression" => {
            // BUG-J1: `new Foo()` must be extracted as a call to the
            // constructor. gitnexus counts `new Foo()` as a CALLS edge.
            extract_object_creation(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "explicit_constructor_invocation" => {
            // BUG-J1: `super()` / `this()` in constructor bodies.
            extract_explicit_constructor(node, source, ctx, result);
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

/// Extracts a class/interface/enum declaration. `label` selects the node label.
fn extract_class(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
    label: NodeLabel,
) {
    let Some(name) = type_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source).map(signature_first_line).map(String::from);
    let mut builder = ModelNode::builder(label, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Java)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);

    extract_heritage(node, source, ctx, &qn, result);
}

/// Extracts EXTENDS and IMPLEMENTS edges from a class/interface declaration.
///
/// tree-sitter-java exposes heritage differently for classes vs interfaces:
/// - `class_declaration` fields: `superclass` (extends), `interfaces` (implements)
/// - `interface_declaration`: `extends_interfaces` is a named **child**, not a
///   field (per node-types.json), so it must be found by iterating named children.
///
/// Target FQNs are best-effort (same-file scope); cross-file resolution is
/// deferred to the type resolver.
fn extract_heritage(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    class_qn: &str,
    result: &mut ExtractResult,
) {
    // EXTENDS: class `extends Bar` (field `superclass`)
    if let Some(superclass) = node.child_by_field_name("superclass") {
        for_each_type_name(superclass, source, &mut |parent_name| {
            let parent_qn = make_qn(ctx.file_path, &parent_name, ctx.project, None);
            result.edges.push(Edge::new(
                class_qn.to_string(),
                parent_qn,
                EdgeType::Extends,
                ctx.project,
            ));
        });
    }

    // IMPLEMENTS: class `implements Runnable` (field `interfaces`)
    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        for_each_type_name(interfaces, source, &mut |iface_name| {
            let iface_qn = make_qn(ctx.file_path, &iface_name, ctx.project, None);
            result.edges.push(Edge::new(
                class_qn.to_string(),
                iface_qn,
                EdgeType::Implements,
                ctx.project,
            ));
        });
    }

    // EXTENDS: interface `extends Bar, Baz`. `extends_interfaces` is a named
    // child of `interface_declaration` (not a field), so scan named children.
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "extends_interfaces" {
                for_each_type_name(child, source, &mut |parent_name| {
                    let parent_qn = make_qn(ctx.file_path, &parent_name, ctx.project, None);
                    result.edges.push(Edge::new(
                        class_qn.to_string(),
                        parent_qn,
                        EdgeType::Extends,
                        ctx.project,
                    ));
                });
            }
        }
    }
}

/// Recursively walks wrapper nodes (`superclass`, `super_interfaces`,
/// `extends_interfaces`, `type_list`) and invokes `f` for each concrete type
/// name found (`type_identifier`, `scoped_type_identifier`, `generic_type`).
fn for_each_type_name<F: FnMut(String)>(node: Node, source: &str, f: &mut F) {
    match node.kind() {
        "type_identifier" | "identifier" | "scoped_type_identifier" => {
            if let Some(text) = node_text(node, source) {
                f(text.to_string());
            }
        }
        "generic_type" => {
            // `List<String>` — extract the raw type name (first child).
            if let Some(child) = node.named_child(0) {
                for_each_type_name(child, source, f);
            }
        }
        "superclass" | "super_interfaces" | "extends_interfaces" | "type_list" => {
            for i in 0..node.named_child_count() as u32 {
                if let Some(child) = node.named_child(i) {
                    for_each_type_name(child, source, f);
                }
            }
        }
        _ => {}
    }
}

fn extract_method(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(name) = method_name(node, source) else {
        return;
    };
    // The enclosing class name is used as the FQN disambiguator so methods in
    // different classes with the same name produce distinct FQNs (ADR-003).
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, ctx.current_parent),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source).map(signature_first_line).map(String::from);
    let mut builder = ModelNode::builder(NodeLabel::Method, name, qn.clone())
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Java)
        .project(ctx.project)
        .is_global(false);
    if let Some(parent) = ctx.current_parent {
        builder = builder.parent_qn(parent);
    }
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    // BUG-J2: methods do NOT get a DEFINES edge. DEFINES semantics is
    // file -> top-level definition; methods are class members, not
    // top-level. Only class/interface/enum (extract_class) create DEFINES.
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    // import_declaration has no named fields (per node-types.json); its
    // children are `asterisk` (wildcard), `identifier`, or `scoped_identifier`.
    let line = node.start_position().row as u32 + 1;
    let mut is_wildcard = false;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "asterisk" {
                is_wildcard = true;
                break;
            }
        }
    }

    let path = if let Some(name_node) = node.child_by_field_name("name") {
        node_text(name_node, source).map(String::from)
    } else {
        let mut found = None;
        for i in 0..node.named_child_count() as u32 {
            if let Some(child) = node.named_child(i) {
                if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                    found = node_text(child, source).map(String::from);
                    break;
                }
            }
        }
        found
    };

    if let Some(p) = path {
        let imported_names = if is_wildcard {
            Vec::new()
        } else {
            p.rsplit('.').next().map(|n| vec![n.to_string()]).unwrap_or_default()
        };
        result.imports.push(ImportInfo {
            source_file: p,
            imported_names,
            line,
        });
    }
}

fn extract_call(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    // method_invocation has a `name` field (the method name) and an optional
    // `object` field (the receiver expression).
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(callee) = node_text(name_node, source).map(String::from) else {
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

/// Extracts `new Foo()` (object_creation_expression) as a call to the
/// constructor (BUG-J1). The callee name is the type name.
fn extract_object_creation(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Some(callee) = node_text(type_node, source).map(String::from) else {
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

/// Extracts `super()` / `this()` (explicit_constructor_invocation) as a call
/// (BUG-J1). The callee name is "super" or "this".
fn extract_explicit_constructor(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let text = node_text(node, source).unwrap_or("");
    let callee = if text.trim_start().starts_with("super") {
        "super"
    } else if text.trim_start().starts_with("this") {
        "this"
    } else {
        return;
    };
    let args = call_arguments(node, source);
    let caller_qn = ctx
        .current_func
        .map(|name| make_qn(ctx.file_path, name, ctx.project, ctx.current_parent));
    result.calls.push(CallInfo {
        caller_qn,
        callee_name: callee.to_string(),
        line: node.start_position().row as u32 + 1,
        args,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the `name` field from a class/interface/enum declaration.
fn type_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
}

fn method_name(node: Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    node_text(name_node, source).map(String::from)
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

/// Returns the first line of a signature string.
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Java, parent)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    // DEFINES edge: file -> definition (matches the Python/C/Go pattern).
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
        let ext = JavaExtractor::new();
        ext.extract(source, "test.java", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_java() {
        assert_eq!(JavaExtractor::new().language(), Language::Java);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = JavaExtractor::default();
        assert_eq!(ext.language(), Language::Java);
    }

    #[test]
    fn extracts_class_declaration() {
        let result = extract("class Foo {}\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 1, "should extract 1 class: {:?}", result.nodes);
        assert_eq!(classes[0].name, "Foo");
        assert_eq!(classes[0].language, Some(Language::Java));
        assert_eq!(classes[0].project, "proj");
        assert_eq!(classes[0].file_path.as_deref(), Some("test.java"));
        assert!(classes[0].is_global, "top-level class should be global");
    }

    #[test]
    fn extracts_interface_declaration() {
        let result = extract("interface Bar { void baz(); }\n");
        let ifaces: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Interface)
            .collect();
        assert_eq!(ifaces.len(), 1, "should extract 1 interface: {:?}", result.nodes);
        assert_eq!(ifaces[0].name, "Bar");
    }

    #[test]
    fn extracts_enum_declaration() {
        let result = extract("enum Color { RED, GREEN }\n");
        let enums: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Enum)
            .collect();
        assert_eq!(enums.len(), 1, "should extract 1 enum: {:?}", result.nodes);
        assert_eq!(enums[0].name, "Color");
    }

    #[test]
    fn extracts_method_declaration() {
        let result = extract("class Foo { void bar() {} }\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "should extract 1 method: {:?}", result.nodes);
        assert_eq!(methods[0].name, "bar");
        assert!(!methods[0].is_global, "method should not be global");
        // The enclosing class name is used as the FQN disambiguator.
        assert!(
            methods[0].qualified_name.contains("Foo"),
            "FQN should contain class name: {}",
            methods[0].qualified_name
        );
        assert_eq!(methods[0].parent_qn.as_deref(), Some("Foo"));
    }

    #[test]
    fn method_fqn_is_disambiguated_by_class_name() {
        // Two methods named bar in different classes should produce distinct FQNs.
        let src = "class A { void bar() {} }\nclass B { void bar() {} }\n";
        let result = extract(src);
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method && n.name == "bar")
            .collect();
        assert_eq!(methods.len(), 2, "should extract 2 bar methods");
        assert_ne!(
            methods[0].qualified_name, methods[1].qualified_name,
            "methods in different classes must have distinct FQNs"
        );
    }

    #[test]
    fn extracts_constructor_declaration() {
        let result = extract("class Foo { Foo() {} }\n");
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(methods.len(), 1, "constructor should be a Method: {:?}", result.nodes);
        assert_eq!(methods[0].name, "Foo");
    }

    #[test]
    fn extracts_import() {
        let result = extract("import java.util.List;\n");
        assert_eq!(result.imports.len(), 1, "should extract 1 import");
        assert_eq!(result.imports[0].source_file, "java.util.List");
    }

    #[test]
    fn extracts_multiple_imports() {
        let result = extract(
            "import java.util.List;\nimport java.util.Map;\n",
        );
        assert_eq!(result.imports.len(), 2, "should extract 2 imports: {:?}", result.imports);
        let paths: Vec<_> = result.imports.iter().map(|i| i.source_file.as_str()).collect();
        assert!(paths.contains(&"java.util.List"), "should import List: {:?}", paths);
        assert!(paths.contains(&"java.util.Map"), "should import Map: {:?}", paths);
    }

    #[test]
    fn extracts_static_import() {
        let result = extract("import static java.util.Math.PI;\n");
        assert_eq!(result.imports.len(), 1, "should extract static import");
        assert_eq!(result.imports[0].source_file, "java.util.Math.PI");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_java() {
        let result = extract("class Foo {}\n");
        assert_eq!(result.language, Language::Java);
        assert_eq!(result.file_path, "test.java");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("class Foo {}\n");
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract("class Foo {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.java.Foo");
    }

    #[test]
    fn class_has_signature() {
        let result = extract("public class Foo implements Runnable {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert!(foo.signature.is_some(), "class should have a signature");
        assert!(foo.signature.as_deref().unwrap().contains("Foo"));
    }

    #[test]
    fn method_has_signature() {
        let result = extract("class Foo { public int bar(int x) { return x; } }\n");
        let bar = result.nodes.iter().find(|n| n.name == "bar").unwrap();
        assert!(bar.signature.is_some(), "method should have a signature");
        assert!(bar.signature.as_deref().unwrap().contains("bar"));
    }

    #[test]
    fn extracts_method_invocation() {
        let result = extract(
            "class Foo { void run() { doSomething(); } }\n",
        );
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"doSomething"),
            "should extract call to doSomething: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(
            "class Foo { void run() { doSomething(1, 2); } }\n",
        );
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "doSomething")
            .expect("should find call to doSomething");
        assert_eq!(call.args.len(), 2, "doSomething(1, 2) should have 2 args");
    }

    #[test]
    fn nested_class_extracts_inner_class() {
        let result = extract("class Outer { class Inner {} }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(classes.len(), 2, "should extract outer + inner class");
        let names: Vec<_> = classes.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Outer"));
        assert!(names.contains(&"Inner"));
    }

    #[test]
    fn class_with_method_with_body_extracts_both() {
        let result = extract("class Foo { void bar() { System.out.println(\"hi\"); } }\n");
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        let methods: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Method)
            .collect();
        assert_eq!(classes.len(), 1, "should extract class Foo");
        assert_eq!(methods.len(), 1, "should extract method bar");
        assert_eq!(classes[0].name, "Foo");
        assert_eq!(methods[0].name, "bar");
    }

    #[test]
    fn extracts_extends_edge() {
        let result = extract("class Foo extends Bar {}\n");
        let extends_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(
            extends_edges.len(),
            1,
            "should extract 1 EXTENDS edge: {:?}",
            result.edges
        );
        assert!(
            extends_edges[0].source.contains("Foo"),
            "source should be Foo: {}",
            extends_edges[0].source
        );
        assert!(
            extends_edges[0].target.contains("Bar"),
            "target should be Bar: {}",
            extends_edges[0].target
        );
    }

    #[test]
    fn extracts_implements_edge() {
        let result = extract("class Foo implements Runnable {}\n");
        let impl_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(
            impl_edges.len(),
            1,
            "should extract 1 IMPLEMENTS edge: {:?}",
            result.edges
        );
        assert!(impl_edges[0].source.contains("Foo"));
        assert!(impl_edges[0].target.contains("Runnable"));
    }

    #[test]
    fn extracts_multiple_implements() {
        let result = extract("class Foo implements A, B {}\n");
        let impl_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .collect();
        assert_eq!(
            impl_edges.len(),
            2,
            "should extract 2 IMPLEMENTS edges: {:?}",
            result.edges
        );
    }

    #[test]
    fn extracts_extends_and_implements() {
        let result = extract("class Foo extends Bar implements Baz {}\n");
        let extends_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .count();
        let impl_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Implements)
            .count();
        assert_eq!(extends_count, 1, "should have 1 EXTENDS");
        assert_eq!(impl_count, 1, "should have 1 IMPLEMENTS");
    }

    #[test]
    fn interface_extends_interface() {
        let result = extract("interface Foo extends Bar {}\n");
        let extends_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Extends)
            .collect();
        assert_eq!(
            extends_edges.len(),
            1,
            "interface extends should produce EXTENDS edge: {:?}",
            result.edges
        );
    }

    #[test]
    fn class_is_exported() {
        let result = extract("public class Foo {}\n");
        let foo = result.nodes.iter().find(|n| n.name == "Foo").unwrap();
        assert!(foo.is_exported, "Java class should be exported for cross-file resolution");
    }

    #[test]
    fn import_populates_imported_names() {
        let result = extract("import java.util.List;\nclass Foo {}\n");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source_file, "java.util.List");
        assert!(
            result.imports[0].imported_names.contains(&"List".to_string()),
            "imported_names should contain 'List', got: {:?}",
            result.imports[0].imported_names
        );
    }

    #[test]
    fn wildcard_import_skips_names() {
        let result = extract("import java.util.*;\nclass Foo {}\n");
        assert_eq!(result.imports.len(), 1);
        assert!(
            result.imports[0].imported_names.is_empty(),
            "wildcard import should not populate imported_names, got: {:?}",
            result.imports[0].imported_names
        );
    }

    #[test]
    fn method_does_not_create_defines_edge() {
        // BUG-J2: DEFINES edge semantics is file -> top-level definition.
        // Methods are not top-level (they live inside a class), so they must
        // NOT produce a DEFINES edge. gitnexus only creates DEFINES for
        // class/interface/enum. The previous code created one DEFINES per
        // method, inflating the DEFINES count ~5x on gson.
        let result = extract("class Foo { void bar() {} void baz() {} }\n");
        let defines_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .collect();
        assert_eq!(
            defines_edges.len(),
            1,
            "only the class should have a DEFINES edge, got {:?}",
            defines_edges
        );
    }

    #[test]
    fn extracts_object_creation_as_call() {
        // BUG-J1: `new Foo()` is an object_creation_expression which was not
        // handled as a call. gitnexus counts `new Foo()` as a CALLS edge to
        // the constructor. Without this, ~77% of Java CALLS were missed on
        // gson (CN=1,415 vs GN=6,081).
        let result = extract("class Foo { Foo make() { return new Foo(); } }\n");
        let new_calls: Vec<_> = result
            .calls
            .iter()
            .filter(|c| c.callee_name == "Foo")
            .collect();
        assert_eq!(
            new_calls.len(),
            1,
            "should extract `new Foo()` as a call to Foo: {:?}",
            result.calls
        );
    }

    #[test]
    fn extracts_explicit_constructor_invocation_as_call() {
        // BUG-J1: `super()` and `this()` are explicit_constructor_invocation
        // nodes which were not handled as calls.
        let result = extract("class Foo { Foo() { super(); } }\n");
        let super_calls: Vec<_> = result
            .calls
            .iter()
            .filter(|c| c.callee_name == "super")
            .collect();
        assert_eq!(
            super_calls.len(),
            1,
            "should extract `super()` as a call: {:?}",
            result.calls
        );
    }
}
