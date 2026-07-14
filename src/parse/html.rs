// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! HTML language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-html's syntax tree into CodeNexus nodes and edges.
//!
//! HTML is a markup language, so extraction is **structural** rather than
//! function/class based. Each `element` node becomes a [`NodeLabel::Property`]
//! node, with the tag name as the display name. When an element has an `id`
//! attribute, the id is appended to the name as `tagname#id` for disambiguation.
//!
//! # Extracted node types
//!
//! - `element` → [`NodeLabel::Property`] (tag name from `start_tag` or
//!   `self_closing_tag` → first `tag_name` child)
//! - `doctype` → no node (structural noise, not a semantic symbol)
//!
//! # FQN pattern
//!
//! `project.file_path.tagname` (e.g. `proj.src.index.html.div`)
//!
//! # Known limitations
//!
//! - Class attributes are not used for disambiguation (only `id`).
//! - Nested elements produce flat FQNs (no parent-child path in the FQN).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor};
use super::parser_factory::ParserFactory;

/// HTML language tree-sitter extractor (Adapter pattern).
pub struct HtmlExtractor {
    _priv: (),
}

impl HtmlExtractor {
    /// Creates a new `HtmlExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for HtmlExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for HtmlExtractor {
    fn language(&self) -> Language {
        Language::Html
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Html);
        let mut parser = ParserFactory::create_parser(Language::Html)?;
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

/// Immutable traversal context passed between visit_node/visit_children.
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "element" => {
            extract_element(node, source, ctx, result);
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

fn extract_element(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(tag_name) = element_tag_name(node, source) else {
        return;
    };
    let id = element_id(node, source);
    let display_name = match &id {
        Some(id_val) => format!("{tag_name}#{id_val}"),
        None => tag_name.clone(),
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &tag_name, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Property, display_name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Html)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the tag name from an `element` node by locating its `start_tag`
/// or `self_closing_tag` child and reading the first `tag_name` child within.
fn element_tag_name(node: Node, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            match child.kind() {
                "start_tag" | "self_closing_tag" => {
                    if let Some(name) = tag_name_from_tag(child, source) {
                        return Some(name);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Returns the text of the first `tag_name` child within a `start_tag` or
/// `self_closing_tag` node.
fn tag_name_from_tag(tag_node: Node, source: &str) -> Option<String> {
    for i in 0..tag_node.named_child_count() as u32 {
        if let Some(child) = tag_node.named_child(i) {
            if child.kind() == "tag_name" {
                return node_text(child, source).map(String::from);
            }
        }
    }
    None
}

/// Extracts the `id` attribute value from an `element` node, if present.
///
/// Walks the `start_tag`/`self_closing_tag` children looking for an
/// `attribute` whose `attribute_name` text equals `"id"`, then reads the
/// value from the following `attribute_value` child (whether direct or
/// nested inside a `quoted_attribute_value`).
fn element_id(node: Node, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            match child.kind() {
                "start_tag" | "self_closing_tag" => {
                    if let Some(id) = id_from_tag(child, source) {
                        return Some(id);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Searches the attributes of a `start_tag`/`self_closing_tag` for an `id`
/// attribute and returns its value.
fn id_from_tag(tag_node: Node, source: &str) -> Option<String> {
    for i in 0..tag_node.named_child_count() as u32 {
        if let Some(child) = tag_node.named_child(i) {
            if child.kind() == "attribute" {
                if let Some(val) = id_from_attribute(child, source) {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// Returns the value of an `attribute` node if its name is `id`.
fn id_from_attribute(attr_node: Node, source: &str) -> Option<String> {
    let mut name: Option<String> = None;
    let mut value: Option<String> = None;
    for i in 0..attr_node.named_child_count() as u32 {
        if let Some(child) = attr_node.named_child(i) {
            match child.kind() {
                "attribute_name" => {
                    name = node_text(child, source).map(String::from);
                }
                "attribute_value" => {
                    value = node_text(child, source).map(String::from);
                }
                "quoted_attribute_value" => {
                    // quoted_attribute_value wraps an attribute_value child.
                    for j in 0..child.named_child_count() as u32 {
                        if let Some(inner) = child.named_child(j) {
                            if inner.kind() == "attribute_value" {
                                value = node_text(inner, source).map(String::from);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if name.as_deref() == Some("id") {
        value
    } else {
        None
    }
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Html, None)
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
        let ext = HtmlExtractor::new();
        ext.extract(source, "test.html", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_html() {
        assert_eq!(HtmlExtractor::new().language(), Language::Html);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = HtmlExtractor::default();
        assert_eq!(ext.language(), Language::Html);
    }

    #[test]
    fn extracts_simple_element() {
        let result = extract("<html></html>\n");
        let elements: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(
            elements.len(),
            1,
            "should extract 1 element: {:?}",
            result.nodes
        );
        assert_eq!(elements[0].name, "html");
        assert_eq!(elements[0].language, Some(Language::Html));
        assert_eq!(elements[0].project, "proj");
        assert!(elements[0].is_global);
    }

    #[test]
    fn extracts_nested_elements() {
        let result = extract("<html><body><p>hi</p></body></html>\n");
        let names: Vec<_> = result
            .nodes
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            names.contains(&"html"),
            "should contain html: {:?}",
            names
        );
        assert!(
            names.contains(&"body"),
            "should contain body: {:?}",
            names
        );
        assert!(names.contains(&"p"), "should contain p: {:?}", names);
        assert_eq!(
            result.nodes.len(),
            3,
            "should extract 3 elements: {:?}",
            result.nodes
        );
    }

    #[test]
    fn extracts_element_with_id() {
        let result = extract("<div id=\"main\"></div>\n");
        let div = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("div"))
            .expect("should find div element");
        assert_eq!(div.name, "div#main");
    }

    #[test]
    fn extracts_element_with_single_quoted_id() {
        let result = extract("<div id='content'></div>\n");
        let div = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("div"))
            .expect("should find div element");
        assert_eq!(div.name, "div#content");
    }

    #[test]
    fn extracts_self_closing_tag() {
        let result = extract("<br/>\n");
        let elements: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(elements.len(), 1, "should extract 1 self-closing element");
        assert_eq!(elements[0].name, "br");
    }

    #[test]
    fn extracts_self_closing_tag_with_id() {
        let result = extract("<input id=\"email\"/>\n");
        let input = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("input"))
            .expect("should find input element");
        assert_eq!(input.name, "input#email");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn doctype_produces_no_node() {
        let result = extract("<!DOCTYPE html>\n<html></html>\n");
        let elements: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(elements.len(), 1, "doctype should not produce a node");
        assert_eq!(elements[0].name, "html");
    }

    #[test]
    fn result_language_is_html() {
        let result = extract("<html></html>\n");
        assert_eq!(result.language, Language::Html);
        assert_eq!(result.file_path, "test.html");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("<html><body></body></html>\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_tagname() {
        let result = extract("<html></html>\n");
        let html = result
            .nodes
            .iter()
            .find(|n| n.name == "html")
            .unwrap();
        assert_eq!(html.qualified_name, "proj.test.html.html");
    }

    #[test]
    fn duplicate_tag_names_get_disambiguated_fqn() {
        let result = extract("<div></div><div></div>\n");
        let divs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.name == "div")
            .collect();
        assert_eq!(divs.len(), 2, "should extract 2 div elements");
        assert_ne!(
            divs[0].qualified_name, divs[1].qualified_name,
            "duplicate tag names should have distinct FQNs"
        );
    }

    #[test]
    fn element_with_non_id_attribute_no_id_in_name() {
        let result = extract("<a href=\"link.html\">text</a>\n");
        let a = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("a"))
            .expect("should find a element");
        assert_eq!(a.name, "a", "non-id attribute should not affect name");
    }

    #[test]
    fn element_with_multiple_attributes_extracts_id() {
        let result = extract("<input type=\"text\" id=\"username\" name=\"user\"/>\n");
        let input = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("input"))
            .expect("should find input element");
        assert_eq!(input.name, "input#username");
    }

    #[test]
    fn element_with_class_but_no_id() {
        let result = extract("<div class=\"container\"></div>\n");
        let div = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("div"))
            .expect("should find div element");
        assert_eq!(div.name, "div", "class attribute should not affect name");
    }

    #[test]
    fn comment_only_html_produces_no_nodes() {
        let result = extract("<!-- just a comment -->\n");
        assert!(
            result.nodes.is_empty(),
            "comment-only should produce no nodes: {:?}",
            result.nodes
        );
    }

    #[test]
    fn deeply_nested_elements_all_extracted() {
        let result = extract("<html><body><div><p><span>text</span></p></div></body></html>\n");
        let names: Vec<_> = result.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"html"), "should contain html");
        assert!(names.contains(&"body"), "should contain body");
        assert!(names.contains(&"div"), "should contain div");
        assert!(names.contains(&"p"), "should contain p");
        assert!(names.contains(&"span"), "should contain span");
        assert_eq!(result.nodes.len(), 5, "should extract 5 elements");
    }

    #[test]
    fn self_closing_tag_without_attributes() {
        let result = extract("<br/>\n");
        let br = result
            .nodes
            .iter()
            .find(|n| n.name == "br")
            .expect("should find br element");
        assert_eq!(br.name, "br");
    }

    #[test]
    fn element_with_empty_id_attribute() {
        let result = extract("<div id=\"\"></div>\n");
        let div = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("div"))
            .expect("should find div element");
        assert!(
            div.name.starts_with("div"),
            "element with empty id should still be extracted"
        );
    }

    #[test]
    fn text_content_does_not_create_extra_nodes() {
        let result = extract("<p>hello world</p>\n");
        let elements: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(elements.len(), 1, "text content should not create extra nodes");
        assert_eq!(elements[0].name, "p");
    }

    #[test]
    fn element_with_data_attributes() {
        let result = extract("<div data-id=\"42\" data-name=\"test\"></div>\n");
        let div = result
            .nodes
            .iter()
            .find(|n| n.name.starts_with("div"))
            .expect("should find div element");
        assert_eq!(div.name, "div", "data attributes should not affect name");
    }
}
