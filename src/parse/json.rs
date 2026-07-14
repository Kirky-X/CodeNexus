// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! JSON language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-json's syntax tree into CodeNexus nodes and edges.
//!
//! JSON is a data format, so extraction is **structural** rather than
//! function/class based. Each top-level `pair` node becomes a
//! [`NodeLabel::Property`] node, with the key (minus surrounding quotes) as
//! the display name. Only pairs at depth 1 (direct children of the root
//! object) are extracted to avoid noise from deeply nested data.
//!
//! # Extracted node types
//!
//! - `pair` → [`NodeLabel::Property`] (key text from the `key` field, which
//!   is a `string` node; surrounding quotes are stripped)
//!
//! # FQN pattern
//!
//! `project.file_path.key` (e.g. `proj.src.config.json.name`)
//!
//! # Known limitations
//!
//! - Only top-level pairs (depth 1) are extracted; nested pairs are ignored
//!   to keep the node count proportional to the file's conceptual size.
//! - Array elements are not extracted.
//! - Duplicate keys at the top level produce disambiguated FQNs via
//!   `#L{line}` suffix.

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor};
use super::parser_factory::ParserFactory;

/// JSON language tree-sitter extractor (Adapter pattern).
pub struct JsonExtractor {
    _priv: (),
}

impl JsonExtractor {
    /// Creates a new `JsonExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for JsonExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for JsonExtractor {
    fn language(&self) -> Language {
        Language::Json
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Json);
        let mut parser = ParserFactory::create_parser(Language::Json)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        let ctx = VisitContext { file_path, project };
        // Only extract top-level pairs (depth 1): walk the document's named
        // children, and for each `object` child, extract its `pair` children
        // without recursing into nested values.
        for i in 0..root.named_child_count() as u32 {
            if let Some(child) = root.named_child(i) {
                if child.kind() == "object" {
                    extract_top_level_pairs(child, source, &ctx, &mut result);
                }
            }
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tree-walking helpers
// ---------------------------------------------------------------------------

/// Immutable traversal context passed between extraction helpers.
struct VisitContext<'a> {
    file_path: &'a str,
    project: &'a str,
}

/// Extracts all `pair` children of a top-level `object` node.
///
/// This does **not** recurse into nested objects, ensuring only depth-1 pairs
/// are extracted (per the parsing spec: "Only extract top-level pairs to
/// avoid noise").
fn extract_top_level_pairs(
    object_node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
    for i in 0..object_node.named_child_count() as u32 {
        if let Some(child) = object_node.named_child(i) {
            if child.kind() == "pair" {
                extract_pair(child, source, ctx, result);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Definition extractors
// ---------------------------------------------------------------------------

fn extract_pair(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(key) = pair_key(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &key, ctx.project),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Property, key, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Json)
        .project(ctx.project)
        .is_global(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the key text from a `pair` node.
///
/// The `pair` node has a `key` field which is a `string` node. The string
/// node's text includes surrounding quotes (e.g. `"name"`), which are
/// stripped to produce the bare key (e.g. `name`).
fn pair_key(node: Node, source: &str) -> Option<String> {
    let key_node = node.child_by_field_name("key")?;
    let raw = node_text(key_node, source)?;
    // Strip surrounding quotes: `"name"` -> `name`.
    let trimmed = raw.trim_matches('"');
    Some(trimmed.to_string())
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Json, None)
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
        let ext = JsonExtractor::new();
        ext.extract(source, "test.json", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_json() {
        assert_eq!(JsonExtractor::new().language(), Language::Json);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = JsonExtractor::default();
        assert_eq!(ext.language(), Language::Json);
    }

    #[test]
    fn extracts_single_pair() {
        let result = extract("{\"name\": \"value\"}\n");
        let pairs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(pairs.len(), 1, "should extract 1 pair: {:?}", result.nodes);
        assert_eq!(pairs[0].name, "name");
        assert_eq!(pairs[0].language, Some(Language::Json));
        assert_eq!(pairs[0].project, "proj");
        assert!(pairs[0].is_global);
    }

    #[test]
    fn extracts_multiple_pairs() {
        let result = extract("{\"a\": 1, \"b\": 2, \"c\": 3}\n");
        let pairs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(pairs.len(), 3, "should extract 3 pairs");
        let names: Vec<_> = pairs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"a"), "should contain a: {:?}", names);
        assert!(names.contains(&"b"), "should contain b: {:?}", names);
        assert!(names.contains(&"c"), "should contain c: {:?}", names);
    }

    #[test]
    fn does_not_extract_nested_pairs() {
        let result = extract("{\"outer\": {\"inner\": 1}, \"top\": 2}\n");
        let pairs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Property)
            .collect();
        assert_eq!(
            pairs.len(),
            2,
            "should extract only 2 top-level pairs: {:?}",
            result.nodes
        );
        let names: Vec<_> = pairs.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"outer"),
            "should contain outer: {:?}",
            names
        );
        assert!(names.contains(&"top"), "should contain top: {:?}", names);
        assert!(
            !names.contains(&"inner"),
            "should NOT contain nested inner: {:?}",
            names
        );
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn empty_object_returns_empty_result() {
        let result = extract("{}\n");
        assert!(result.is_empty(), "empty object should produce no nodes");
    }

    #[test]
    fn result_language_is_json() {
        let result = extract("{\"name\": \"value\"}\n");
        assert_eq!(result.language, Language::Json);
        assert_eq!(result.file_path, "test.json");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("{\"a\": 1, \"b\": 2}\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert_eq!(defines_count, node_count, "one DEFINES edge per node");
    }

    #[test]
    fn qualified_name_uses_file_path_and_key() {
        let result = extract("{\"name\": \"value\"}\n");
        let pair = result.nodes.iter().find(|n| n.name == "name").unwrap();
        assert_eq!(pair.qualified_name, "proj.test.json.name");
    }

    #[test]
    fn duplicate_keys_get_disambiguated_fqn() {
        let result = extract("{\"a\": 1, \"a\": 2}\n");
        let pairs: Vec<_> = result.nodes.iter().filter(|n| n.name == "a").collect();
        assert_eq!(pairs.len(), 2, "should extract 2 pairs with key 'a'");
        assert_ne!(
            pairs[0].qualified_name, pairs[1].qualified_name,
            "duplicate keys should have distinct FQNs"
        );
    }

    #[test]
    fn array_top_level_returns_empty_result() {
        let result = extract("[1, 2, 3]\n");
        assert!(
            result.is_empty(),
            "array at top level should produce no nodes"
        );
    }
}
