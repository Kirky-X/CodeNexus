// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Regex language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-regex's syntax tree into CodeNexus nodes and edges.
//!
//! Regular expressions have no functions or classes — the entire file is a
//! single pattern. This extractor produces one [`NodeLabel::Const`] node
//! representing the whole regex pattern, named `regex_pattern`.
//!
//! # Extracted node types
//!
//! - `pattern` (root) → [`NodeLabel::Const`] (name = `regex_pattern`)
//!
//! # FQN pattern
//!
//! `project.file_path.regex_pattern` (e.g. `proj.src.email.regex.regex_pattern`)
//!
//! # Known limitations
//!
//! - Only one node is produced per file (the root pattern).
//! - Sub-expressions (capturing groups, character classes) are not extracted
//!   individually, as they are not independently addressable symbols.

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor};
use super::parser_factory::ParserFactory;

/// The fixed name used for the single regex pattern node.
const PATTERN_NAME: &str = "regex_pattern";

/// Regex language tree-sitter extractor (Adapter pattern).
pub struct RegexExtractor {
    _priv: (),
}

impl RegexExtractor {
    /// Creates a new `RegexExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for RegexExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for RegexExtractor {
    fn language(&self) -> Language {
        Language::Regex
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Regex);
        let mut parser = ParserFactory::create_parser(Language::Regex)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        // The root node of a regex file is `pattern`. Extract it as a single
        // Const node. If the root is not `pattern` (e.g. empty/malformed
        // input), produce no nodes.
        if root.kind() == "pattern" {
            let qn = dedupe_qn(
                make_qn(file_path, PATTERN_NAME, project),
                root.start_position().row as u32 + 1,
                &result,
            );
            let model_node = ModelNode::builder(NodeLabel::Const, PATTERN_NAME, qn)
                .file_path(file_path)
                .start_line(root.start_position().row as u32 + 1)
                .end_line(root.end_position().row as u32 + 1)
                .language(Language::Regex)
                .project(project)
                .is_global(true)
                .build();
            add_definition_edges(file_path, project, &model_node, &mut result);
            result.push_node(model_node);
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Regex, None)
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
        let ext = RegexExtractor::new();
        ext.extract(source, "test.regex", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_regex() {
        assert_eq!(RegexExtractor::new().language(), Language::Regex);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = RegexExtractor::default();
        assert_eq!(ext.language(), Language::Regex);
    }

    #[test]
    fn extracts_simple_pattern() {
        let result = extract("abc\n");
        let nodes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Const)
            .collect();
        assert_eq!(
            nodes.len(),
            1,
            "should extract 1 pattern node: {:?}",
            result.nodes
        );
        assert_eq!(nodes[0].name, "regex_pattern");
        assert_eq!(nodes[0].language, Some(Language::Regex));
        assert_eq!(nodes[0].project, "proj");
        assert!(nodes[0].is_global);
    }

    #[test]
    fn extracts_complex_pattern() {
        let result = extract("^[a-zA-Z0-9]+@example\\.com$\n");
        assert_eq!(
            result.nodes.len(),
            1,
            "should extract exactly 1 node for any pattern"
        );
        assert_eq!(result.nodes[0].name, "regex_pattern");
    }

    #[test]
    fn result_language_is_regex() {
        let result = extract("abc\n");
        assert_eq!(result.language, Language::Regex);
        assert_eq!(result.file_path, "test.regex");
    }

    #[test]
    fn creates_defines_edge() {
        let result = extract("abc\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        assert_eq!(defines_count, 1, "should create one DEFINES edge");
    }

    #[test]
    fn qualified_name_uses_file_path_and_pattern_name() {
        let result = extract("abc\n");
        let node = &result.nodes[0];
        assert_eq!(
            node.qualified_name, "proj.test.regex.regex_pattern",
            "FQN should be project.file_path.regex_pattern"
        );
    }

    #[test]
    fn pattern_has_correct_line_range() {
        let result = extract("abc");
        let node = &result.nodes[0];
        assert_eq!(node.start_line, Some(1), "start line should be 1");
        assert_eq!(node.end_line, Some(1), "end line should be 1");
    }

    #[test]
    fn empty_source_produces_no_node_or_handles_gracefully() {
        let result = extract("");
        // An empty source may or may not produce a pattern node depending on
        // the grammar's error recovery. Either way, extraction should not
        // panic and should return a valid result.
        assert_eq!(result.language, Language::Regex);
        // If a node was produced, it must be named regex_pattern.
        for node in &result.nodes {
            assert_eq!(node.name, "regex_pattern");
        }
    }
}
