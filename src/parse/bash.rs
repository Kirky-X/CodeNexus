// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Bash language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-bash's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function_definition` → [`NodeLabel::Function`] (name extracted from the
//!   `command_name` child)
//! - `variable_assignment` (top-level) → [`NodeLabel::GlobalVar`]
//!
//! # Not extracted
//!
//! - `command` nodes at the top level are intentionally skipped — they are
//!   too noisy (every shell command would become a node) and do not
//!   correspond to reusable definitions.
//!
//! # Known limitations
//!
//! - Bash has no module/import system, so no [`ImportInfo`] records are
//!   produced.
//! - Function calls inside command pipelines are not captured as
//!   [`CallInfo`] (bash command resolution is too dynamic for static
//!   extraction).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor};
use super::parser_factory::ParserFactory;

/// Bash language tree-sitter extractor (Adapter pattern).
pub struct BashExtractor {
    _priv: (),
}

impl BashExtractor {
    /// Creates a new `BashExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for BashExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for BashExtractor {
    fn language(&self) -> Language {
        Language::Bash
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Bash);
        let mut parser = ParserFactory::create_parser(Language::Bash)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        let ctx = VisitContext {
            file_path,
            project,
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
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function_definition" => {
            extract_function(node, source, ctx, result);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "variable_assignment" => {
            if is_top_level(node) {
                extract_global_var(node, source, ctx, result);
            }
            visit_children(node, source, ctx, result);
        }
        "command" => {
            // Top-level commands are too noisy; skip extraction and recursion.
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
    let Some(name) = function_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let signature = node_text(node, source)
        .map(signature_first_line)
        .map(String::from);
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Bash)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_global_var(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(raw_name) = node_text(name_node, source).map(String::from) else {
        return;
    };
    // Strip a leading `$` if present (variable_name in assignment context
    // usually has no `$`, but strip defensively).
    let name = raw_name.trim_start_matches('$').to_string();
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::GlobalVar, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .language(Language::Bash)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the function name from a `function_definition` node.
///
/// tree-sitter-bash exposes the name as a `command_name` child (for
/// `function foo { }`) or via the `name` field. This function tries the
/// `name` field first, then scans for a `command_name` named child, and
/// finally falls back to the first `word` child (for `foo() { }`).
fn function_name(node: Node, source: &str) -> Option<String> {
    // Try the `name` field first (covers `function foo { }`).
    if let Some(name_node) = node.child_by_field_name("name") {
        if let Some(text) = node_text(name_node, source).map(String::from) {
            return Some(text);
        }
    }
    // Fall back to scanning for a `command_name` named child.
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "command_name" {
                return node_text(child, source).map(String::from);
            }
        }
    }
    // Fall back to the first `word` child (covers `foo() { }`).
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "word" {
                return node_text(child, source).map(String::from);
            }
        }
    }
    None
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

/// Returns the first line of a signature string (the `foo() {` line).
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// Returns true if `node`'s direct parent is the `program` root.
fn is_top_level(node: Node) -> bool {
    node.parent()
        .map(|p| p.kind() == "program")
        .unwrap_or(false)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Bash, parent)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    // DEFINES edge: file -> definition (matches the Go/C extractor pattern).
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
        let ext = BashExtractor::new();
        ext.extract(source, "test.sh", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_bash() {
        assert_eq!(BashExtractor::new().language(), Language::Bash);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = BashExtractor::default();
        assert_eq!(ext.language(), Language::Bash);
    }

    #[test]
    fn extracts_function_definition() {
        let result = extract("foo() { echo hi; }\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(
            funcs.len(),
            1,
            "should extract 1 function: {:?}",
            result.nodes
        );
        assert_eq!(funcs[0].name, "foo");
        assert_eq!(funcs[0].language, Some(Language::Bash));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.sh"));
        assert!(funcs[0].is_global, "top-level function should be global");
        assert!(funcs[0].is_exported, "top-level function should be exported");
    }

    #[test]
    fn extracts_function_with_function_keyword() {
        let result = extract("function bar { echo hi; }\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(
            funcs.len(),
            1,
            "should extract 1 function with keyword: {:?}",
            result.nodes
        );
        assert_eq!(funcs[0].name, "bar");
    }

    #[test]
    fn extracts_multiple_functions() {
        let result = extract("foo() { echo 1; }\nbar() { echo 2; }\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert_eq!(funcs.len(), 2, "should extract 2 functions");
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn extracts_global_variable() {
        let result = extract("FOO=bar\n");
        let globals: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::GlobalVar)
            .collect();
        assert_eq!(
            globals.len(),
            1,
            "should extract 1 global variable: {:?}",
            result.nodes
        );
        assert_eq!(globals[0].name, "FOO");
        assert_eq!(globals[0].language, Some(Language::Bash));
        assert!(globals[0].is_global);
    }

    #[test]
    fn extracts_multiple_global_variables() {
        let result = extract("A=1\nB=2\nC=3\n");
        let globals: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::GlobalVar)
            .collect();
        assert_eq!(globals.len(), 3, "should extract 3 global variables");
        let names: Vec<_> = globals.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
        assert!(names.contains(&"C"));
    }

    #[test]
    fn command_not_extracted_as_node() {
        // Top-level commands should not produce any nodes.
        let result = extract("echo hello\necho world\n");
        assert!(
            result.nodes.is_empty(),
            "commands should not produce nodes: {:?}",
            result.nodes
        );
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_bash() {
        let result = extract("foo() { echo hi; }\n");
        assert_eq!(result.language, Language::Bash);
        assert_eq!(result.file_path, "test.sh");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("foo() { echo hi; }\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract("foo() { echo hi; }\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.sh.foo");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("greet() { echo hi; }\n");
        let greet = result.nodes.iter().find(|n| n.name == "greet").unwrap();
        assert!(greet.signature.is_some(), "function should have a signature");
        assert!(greet.signature.as_deref().unwrap().contains("greet"));
    }

    #[test]
    fn mixed_functions_and_variables() {
        let result = extract("NAME=value\nsetup() { echo setup; }\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        let globals: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::GlobalVar)
            .collect();
        assert_eq!(funcs.len(), 1, "should extract 1 function");
        assert_eq!(globals.len(), 1, "should extract 1 global variable");
        assert_eq!(funcs[0].name, "setup");
        assert_eq!(globals[0].name, "NAME");
    }
}
