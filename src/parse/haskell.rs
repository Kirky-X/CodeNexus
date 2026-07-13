// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Haskell language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-haskell's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `function` → [`NodeLabel::Function`] (name extracted from first `variable`)
//! - `signature` → [`NodeLabel::TypeAlias`] (signature only, no body)
//! - `data_type` → [`NodeLabel::Struct`] (name field)
//! - `newtype` → [`NodeLabel::TypeAlias`] (name field)
//! - `type_synomym` → [`NodeLabel::TypeAlias`] (name field)
//! - `module` → [`NodeLabel::Module`] (name field)
//!
//! # Extracted records
//!
//! - `import` → [`ImportInfo`] (extracts module name)
//!
//! # Known limitations
//!
//! - Haskell export lists are not analyzed; all top-level definitions are
//!   treated as exported.
//! - `signature` and `function` for the same name produce two separate
//!   nodes (TypeAlias and Function); the FQN de-duplication logic appends
//!   `#L{line}` to the second one.
//! - Type class declarations (`class Foo a where ...`) are not specially
//!   handled (they may appear as `class` nodes but are not extracted here).

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::dedupe_qn;
use super::error::{ParseError, Result};
use super::extractor::{ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// Haskell language tree-sitter extractor (Adapter pattern).
pub struct HaskellExtractor {
    _priv: (),
}

impl HaskellExtractor {
    /// Creates a new `HaskellExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for HaskellExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for HaskellExtractor {
    fn language(&self) -> Language {
        Language::Haskell
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Haskell);
        let mut parser = ParserFactory::create_parser(Language::Haskell)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        let ctx = VisitContext {
            file_path,
            project,
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
    current_parent: Option<&'a str>,
}

fn visit_node(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    match node.kind() {
        "function" => {
            extract_function(node, source, ctx, result);
            let child_ctx = VisitContext {
                file_path: ctx.file_path,
                project: ctx.project,
                current_parent: ctx.current_parent,
            };
            visit_children(node, source, &child_ctx, result);
        }
        "signature" => {
            extract_type_signature(node, source, ctx, result);
        }
        "data_type" => {
            extract_data_type(node, source, ctx, result);
        }
        "newtype" => {
            extract_new_type(node, source, ctx, result);
        }
        "type_synomym" => {
            extract_type_alias(node, source, ctx, result);
        }
        "module" => {
            extract_module(node, source, ctx, result);
            visit_children(node, source, ctx, result);
        }
        "import" => {
            extract_import(node, source, result);
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
        .language(Language::Haskell)
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

fn extract_type_signature(
    node: Node,
    source: &str,
    ctx: &VisitContext<'_>,
    result: &mut ExtractResult,
) {
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
    let mut builder = ModelNode::builder(NodeLabel::TypeAlias, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Haskell)
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

fn extract_data_type(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = haskell_type_name(node, source) else {
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
    let mut builder = ModelNode::builder(NodeLabel::Struct, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Haskell)
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

fn extract_new_type(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = haskell_type_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::TypeAlias, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Haskell)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_type_alias(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = haskell_type_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::TypeAlias, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Haskell)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

fn extract_module(node: Node, source: &str, ctx: &VisitContext<'_>, result: &mut ExtractResult) {
    let Some(name) = module_name(node, source) else {
        return;
    };
    let qn = dedupe_qn(
        make_qn(ctx.file_path, &name, ctx.project, None),
        node.start_position().row as u32 + 1,
        result,
    );
    let model_node = ModelNode::builder(NodeLabel::Module, name, qn)
        .file_path(ctx.file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Haskell)
        .project(ctx.project)
        .is_global(true)
        .is_exported(true)
        .build();
    add_definition_edges(ctx.file_path, ctx.project, &model_node, result);
    result.push_node(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_import(node: Node, source: &str, result: &mut ExtractResult) {
    let line = node.start_position().row as u32 + 1;
    if let Some(module) = import_module_name(node, source) {
        result.imports.push(ImportInfo {
            source_file: module,
            imported_names: Vec::new(),
            line,
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the function name from a `function` or `type_signature` node by
/// finding the first `variable` named child (direct or one level nested).
fn function_name(node: Node, source: &str) -> Option<String> {
    // Strategy 1: direct "name" field
    if let Some(name_node) = node.child_by_field_name("name") {
        return node_text(name_node, source).map(String::from);
    }
    // Strategy 2: first "variable" named child (direct)
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "variable" {
                return node_text(child, source).map(String::from);
            }
            // Strategy 3: look inside wrapper nodes
            for j in 0..child.named_child_count() as u32 {
                if let Some(grandchild) = child.named_child(j) {
                    if grandchild.kind() == "variable" {
                        return node_text(grandchild, source).map(String::from);
                    }
                }
            }
        }
    }
    None
}

/// Extracts the type name from `data_type`, `new_type`, or `type_alias` nodes.
fn haskell_type_name(node: Node, source: &str) -> Option<String> {
    // Strategy 1: direct "name" field
    if let Some(name_node) = node.child_by_field_name("name") {
        return node_text(name_node, source).map(String::from);
    }
    // Strategy 2: search named children for type_constructor or name-like nodes
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if let Some(name) = find_name_in_child(child, source) {
                return Some(name);
            }
        }
    }
    None
}

/// Recursively searches a child node for the first name-like node
/// (`type_constructor`, `name`, `constructor`).
fn find_name_in_child(node: Node, source: &str) -> Option<String> {
    let kind = node.kind();
    if kind == "type_constructor" || kind == "constructor" || kind == "name" {
        return node_text(node, source).map(String::from);
    }
    // Try "name" field on this child
    if let Some(name_node) = node.child_by_field_name("name") {
        return node_text(name_node, source).map(String::from);
    }
    // Look one level deeper
    for i in 0..node.named_child_count() as u32 {
        if let Some(grandchild) = node.named_child(i) {
            let gk = grandchild.kind();
            if gk == "type_constructor" || gk == "constructor" || gk == "name" {
                return node_text(grandchild, source).map(String::from);
            }
        }
    }
    None
}

fn module_name(node: Node, source: &str) -> Option<String> {
    // Strategy 1: "name" field
    if let Some(name_node) = node.child_by_field_name("name") {
        return node_text(name_node, source).map(String::from);
    }
    // Strategy 2: first named child (module_id or module_name)
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            let kind = child.kind();
            if kind == "module_id" || kind == "module_name" || kind == "identifier" {
                return node_text(child, source).map(String::from);
            }
        }
    }
    None
}

fn import_module_name(node: Node, source: &str) -> Option<String> {
    // Strategy 1: "module" field
    if let Some(module_node) = node.child_by_field_name("module") {
        return node_text(module_node, source).map(String::from);
    }
    // Strategy 2: first module_name child
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            let kind = child.kind();
            if kind == "module_name" || kind == "module_id" || kind == "identifier" {
                return node_text(child, source).map(String::from);
            }
        }
    }
    None
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

/// Returns the first line of a signature string.
fn signature_first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Haskell, parent)
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
        let ext = HaskellExtractor::new();
        ext.extract(source, "test.hs", "proj")
            .expect("extraction should succeed")
    }

    #[test]
    fn language_returns_haskell() {
        assert_eq!(HaskellExtractor::new().language(), Language::Haskell);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = HaskellExtractor::default();
        assert_eq!(ext.language(), Language::Haskell);
    }

    #[test]
    fn extracts_function() {
        let result = extract("foo x = x + 1\n");
        let funcs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Function)
            .collect();
        assert!(
            funcs.len() >= 1,
            "should extract at least 1 function: {:?}",
            result.nodes
        );
        assert_eq!(funcs[0].name, "foo");
        assert_eq!(funcs[0].language, Some(Language::Haskell));
        assert_eq!(funcs[0].project, "proj");
        assert_eq!(funcs[0].file_path.as_deref(), Some("test.hs"));
        assert!(funcs[0].is_global);
    }

    #[test]
    fn extracts_type_signature() {
        let result = extract("foo :: Int -> Int\n");
        let sigs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias)
            .collect();
        assert!(
            sigs.len() >= 1,
            "should extract type signature: {:?}",
            result.nodes
        );
        assert_eq!(sigs[0].name, "foo");
    }

    #[test]
    fn extracts_data_type() {
        let result = extract("data Foo = FooCon\n");
        let structs: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Struct)
            .collect();
        assert!(
            structs.len() >= 1,
            "should extract data type: {:?}",
            result.nodes
        );
        assert_eq!(structs[0].name, "Foo");
    }

    #[test]
    fn extracts_new_type() {
        let result = extract("newtype Foo = Foo Int\n");
        let aliases: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias && n.name == "Foo")
            .collect();
        assert!(
            aliases.len() >= 1,
            "should extract newtype: {:?}",
            result.nodes
        );
    }

    #[test]
    fn extracts_type_alias() {
        let result = extract("type Foo = Int\n");
        let aliases: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::TypeAlias && n.name == "Foo")
            .collect();
        assert!(
            aliases.len() >= 1,
            "should extract type alias: {:?}",
            result.nodes
        );
    }

    #[test]
    fn extracts_module() {
        let result = extract("module Foo where\n");
        let modules: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Module)
            .collect();
        assert!(
            modules.len() >= 1,
            "should extract module: {:?}",
            result.nodes
        );
        assert_eq!(modules[0].name, "Foo");
    }

    #[test]
    fn extracts_import() {
        let result = extract("import Data.List\n");
        assert!(
            result.imports.len() >= 1,
            "should extract import: {:?}",
            result.imports
        );
        assert_eq!(result.imports[0].source_file, "Data.List");
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn result_language_is_haskell() {
        let result = extract("foo x = x\n");
        assert_eq!(result.language, Language::Haskell);
        assert_eq!(result.file_path, "test.hs");
    }

    #[test]
    fn creates_defines_edges() {
        let result = extract("foo x = x\n");
        let defines_count = result
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Defines)
            .count();
        let node_count = result.nodes.len();
        assert!(
            defines_count >= node_count,
            "should create DEFINES edges for nodes: {defines_count} vs {node_count}"
        );
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract("foo x = x\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert_eq!(foo.qualified_name, "proj.test.hs.foo");
    }

    #[test]
    fn function_has_signature() {
        let result = extract("foo x = x + 1\n");
        let foo = result.nodes.iter().find(|n| n.name == "foo").unwrap();
        assert!(foo.signature.is_some(), "function should have a signature");
        assert!(foo.signature.as_deref().unwrap().contains("foo"));
    }
}
