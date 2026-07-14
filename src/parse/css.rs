// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CSS language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-css's syntax tree into CodeNexus nodes and edges.
//!
//! CSS is a styling language, so extraction is **structural** rather than
//! function/class based. Each `rule_set` node becomes a [`NodeLabel::Class`]
//! node named after its first selector, and each `media_statement` becomes a
//! [`NodeLabel::Namespace`] node identified by line number (since media
//! queries have no single name).
//!
//! # Extracted node types
//!
//! - `rule_set` → [`NodeLabel::Class`] (selector text from `selectors` →
//!   first named child)
//! - `media_statement` → [`NodeLabel::Namespace`] (no name; uses line number
//!   as `media_L{line}`)
//!
//! # FQN pattern
//!
//! `project.file_path.selector` (e.g. `proj.src.style.css.body`)
//!
//! # Known limitations
//!
//! - Only the first selector in a selector list is used as the name
//!   (`a, button { ... }` → name is `a`).
//! - Compound selectors (`div.foo`) are used verbatim as the name text.
//! - Nested rule sets (CSS nesting) are extracted with flat FQNs.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor};
use super::parser_factory::ParserFactory;

/// CSS language tree-sitter extractor (Adapter pattern).
pub struct CssExtractor {
    _priv: (),
}

impl CssExtractor {
    /// Creates a new `CssExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for CssExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for CssExtractor {
    fn language(&self) -> Language {
        Language::Css
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Css);
        let mut parser = ParserFactory::create_parser(Language::Css)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        let ctx = VisitContext { file_path, project };
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

/// Immutable traversal context passed between visit_node/visit_children.
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "rule_set" => {
            extract_rule_set(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "media_statement" => {
            extract_media_statement(node, source, ctx, result);
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

fn extract_rule_set(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(selector_text) = rule_set_selector(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &selector_text, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Class, selector_text, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Css)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_media_statement(
    node: Node,
    _source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    let line = node.start_position().row as u32 + 1;
    let name = format!("media_L{line}");
    let qn = dedupe_qn(make_qn(ctx.file_path, &name, ctx.project), line, result);
    let model_node = ModelNode::builder(NodeLabel::Namespace, name, qn)
        .file_path(ctx.file_path)
        .start_line(line)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Css)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the first selector text from a `rule_set` node.
///
/// `rule_set` has a `selectors` child which contains one or more selector
/// nodes (e.g. `class_selector`, `tag_name`, `id_selector`). This function
/// returns the text of the first named child of `selectors`.
fn rule_set_selector(node: Node, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "selectors" {
                if let Some(first) = child.named_child(0) {
                    return node_text(first, source).map(String::from);
                }
            }
        }
    }
    None
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Css, None)
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
        let ext = CssExtractor::new();
        ext.extract(source, "test.css", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_css() {
        assert_eq!(CssExtractor::new().language(), Language::Css);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = CssExtractor::default();
        assert_eq!(ext.language(), Language::Css);
    }

    #[test]
    fn extracts_tag_selector_rule() {
        let result = extract("body { color: red; }\n");
        let rules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(
            rules.len(),
            1,
            "should extract 1 rule_set: {:?}",
            result.nodes
        );
        assert_eq!(rules[0].name, "body");
        assert_eq!(rules[0].language, Some(Language::Css));
        assert_eq!(rules[0].project, "proj");
        assert!(rules[0].is_global);
    }

    #[test]
    fn extracts_class_selector_rule() {
        let result = extract(".foo { color: blue; }\n");
        let rule = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Class)
            .expect("should find class rule");
        assert_eq!(rule.name, ".foo");
    }

    #[test]
    fn extracts_id_selector_rule() {
        let result = extract("#main { color: green; }\n");
        let rule = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Class)
            .expect("should find id rule");
        assert_eq!(rule.name, "#main");
    }

    #[test]
    fn extracts_multiple_rules() {
        let result = extract("body { color: red; }\n.foo { color: blue; }\n");
        let rules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(rules.len(), 2, "should extract 2 rule_sets");
        let names: Vec<_> = rules.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"body"), "should contain body: {:?}", names);
        assert!(names.contains(&".foo"), "should contain .foo: {:?}", names);
    }

    #[test]
    fn extracts_media_statement() {
        let result = extract("@media screen { body { color: red; } }\n");
        let media: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Namespace)
            .collect();
        assert_eq!(
            media.len(),
            1,
            "should extract 1 media_statement: {:?}",
            result.nodes
        );
        assert!(
            media[0].name.starts_with("media_L"),
            "media name should start with media_L: {}",
            media[0].name
        );
    }

    #[test]
    fn media_statement_contains_nested_rule() {
        let result = extract("@media screen { body { color: red; } }\n");
        let body_rule = result
            .nodes
            .iter()
            .find(|n| n.label == NodeLabel::Class && n.name == "body");
        assert!(
            body_rule.is_some(),
            "media_statement should contain nested body rule: {:?}",
            result.nodes
        );
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_css() {
        let result = extract("body { color: red; }\n");
        assert_eq!(result.language, Language::Css);
        assert_eq!(result.file_path, "test.css");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("body { color: red; }\n.foo { color: blue; }\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_selector() {
        let result = extract("body { color: red; }\n");
        let body = result.nodes.iter().find(|n| n.name == "body").unwrap();
        assert_eq!(body.qualified_name, "proj.test.css.body");
    }

    #[test]
    fn duplicate_selectors_get_disambiguated_fqn() {
        let result = extract("body { color: red; }\nbody { color: blue; }\n");
        let bodies: Vec<_> = result.nodes.iter().filter(|n| n.name == "body").collect();
        assert_eq!(bodies.len(), 2, "should extract 2 body rules");
        assert_ne!(
            bodies[0].qualified_name, bodies[1].qualified_name,
            "duplicate selectors should have distinct FQNs"
        );
    }
}
