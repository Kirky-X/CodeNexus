// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `rename` service: propose graph + text edits for renaming a symbol.
//!
//! Produces a two-part plan:
//! 1. **Graph edits** — Cypher `SET` on the symbol node's `name` and
//!    `qualifiedName`.
//! 2. **Text edits** — word-boundary find/replace of the old name in source
//!    files under `path`.
//!
//! When `apply` is true, changes are written to disk; otherwise the plan is
//! printed as JSON (dry-run).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::kit::{AsyncKit, AsyncReady, StorageModule, TraceModule};
use crate::model::{Graph, Node, NodeId};
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error, wrap_kit_error};
#[cfg(feature = "cli")]
use crate::service::runtime::kit;
use crate::storage::schema::{escape_cypher_string, escape_identifier};
use crate::trace::TraceError;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// Resolves a symbol name to a node id by matching `name` first, then
/// `qualified_name`.
fn resolve_start_id(graph: &Graph, symbol: &str) -> Option<NodeId> {
    let by_name: Vec<&Node> = graph.nodes.values().filter(|n| n.name == symbol).collect();
    if by_name.len() == 1 {
        return Some(by_name[0].id.clone());
    }
    let by_qn: Vec<&Node> = graph
        .nodes
        .values()
        .filter(|n| n.qualified_name == symbol)
        .collect();
    if by_qn.len() == 1 {
        return Some(by_qn[0].id.clone());
    }
    by_name.first().map(|n| n.id.clone())
}

/// Computes the new qualified name by replacing the trailing old name segment.
fn compute_new_qn(old_qn: &str, old_name: &str, new_name: &str) -> String {
    if let Some(stripped) = old_qn.strip_suffix(old_name) {
        if stripped.is_empty() || stripped.ends_with('.') {
            return format!("{stripped}{new_name}");
        }
    }
    old_qn.to_string()
}

/// Returns `true` if `s` is a valid identifier `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Collects candidate files for text edits: the symbol's own file plus all
/// neighbor files in the loaded subgraph, filtered to those under `root`.
fn collect_candidate_files(graph: &Graph, start_id: &NodeId, root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let root_canonical = root.to_path_buf();
    let collect =
        |node: &Node, files: &mut Vec<PathBuf>, seen: &mut std::collections::HashSet<PathBuf>| {
            if let Some(fp) = &node.file_path {
                let p = PathBuf::from(fp);
                if p.is_absolute() {
                    if p.starts_with(&root_canonical) && seen.insert(p.clone()) {
                        files.push(p);
                    }
                } else if seen.insert(root_canonical.join(&p)) {
                    files.push(root_canonical.join(p));
                }
            }
        };
    if let Some(node) = graph.get_node(start_id) {
        collect(node, &mut files, &mut seen);
    }
    for edge in graph.edges.iter() {
        if edge.source == *start_id {
            if let Some(n) = graph.get_node(&edge.target) {
                collect(n, &mut files, &mut seen);
            }
        } else if edge.target == *start_id {
            if let Some(n) = graph.get_node(&edge.source) {
                collect(n, &mut files, &mut seen);
            }
        }
    }
    files
}

/// Scans each candidate file for word-boundary occurrences of `old_name` and
/// returns the proposed text edits.
fn scan_text_edits(
    root: &Path,
    old_name: &str,
    new_name: &str,
    files: &[PathBuf],
) -> Result<Vec<TextEdit>, CodeNexusError> {
    let mut edits: Vec<TextEdit> = Vec::new();
    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (line_idx, line) in content.lines().enumerate() {
            for occurrence in find_word_occurrences(line, old_name) {
                edits.push(TextEdit {
                    file_path: file_to_rel_string(file, root),
                    line: line_idx + 1,
                    column: occurrence + 1,
                    old_text: old_name.to_string(),
                    new_text: new_name.to_string(),
                });
            }
        }
    }
    Ok(edits)
}

/// Finds the byte offsets of all word-boundary occurrences of `needle` in
/// `haystack`.
fn find_word_occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut positions: Vec<usize> = Vec::new();
    if needle.is_empty() {
        return positions;
    }
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle_bytes {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok =
                i + needle.len() == bytes.len() || !is_ident_byte(bytes[i + needle.len()]);
            if before_ok && after_ok {
                positions.push(i);
            }
            i += needle.len();
        } else {
            i += 1;
        }
    }
    positions
}

/// Returns `true` if `b` is an identifier byte `[A-Za-z0-9_]`.
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Converts an absolute file path to a string relative to `root` if possible.
fn file_to_rel_string(file: &Path, root: &Path) -> String {
    file.strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| file.to_string_lossy().into_owned())
}

/// Applies the graph edit via Cypher `SET`.
fn apply_graph_edit(kit: &AsyncKit<AsyncReady>, edit: &GraphEdit) -> Result<(), CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let table = escape_identifier(edit.label.as_str());
    let cypher = format!(
        "MATCH (n:{table}) WHERE n.id = '{id}' SET n.name = '{new_name}', n.qualifiedName = '{new_qn}';",
        id = escape_cypher_string(&edit.node_id),
        new_name = escape_cypher_string(&edit.new_name),
        new_qn = escape_cypher_string(&edit.new_qualified_name),
    );
    storage.execute(&cypher)?;
    Ok(())
}

/// Applies text edits by writing the replaced content to each file.
fn apply_text_edits(edits: &[TextEdit]) -> Result<(), CodeNexusError> {
    let mut by_file: std::collections::HashMap<String, Vec<&TextEdit>> =
        std::collections::HashMap::new();
    for e in edits {
        by_file.entry(e.file_path.clone()).or_default().push(e);
    }
    for (file_path, file_edits) in by_file {
        let path = PathBuf::from(&file_path);
        let content = std::fs::read_to_string(&path)?;
        let new_content = apply_replacements(&content, &file_edits);
        std::fs::write(&path, new_content)?;
    }
    Ok(())
}

/// Applies word-boundary replacements to `content` at each (line, column)
/// position in `edits`.
fn apply_replacements(content: &str, edits: &[&TextEdit]) -> String {
    let mut sorted: Vec<&TextEdit> = edits.to_vec();
    sorted.sort_by(|a, b| b.line.cmp(&a.line).then_with(|| b.column.cmp(&a.column)));
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    for edit in sorted {
        let line_idx = edit.line.saturating_sub(1);
        if line_idx >= lines.len() {
            continue;
        }
        let line = &mut lines[line_idx];
        let col = edit.column.saturating_sub(1);
        let end = col + edit.old_text.len();
        if end <= line.len() && line[col..end] == edit.old_text {
            line.replace_range(col..end, &edit.new_text);
        }
    }
    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// JSON-serializable rename output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RenameOutput {
    /// The original symbol name or FQN.
    pub from: String,
    /// The new symbol name.
    pub to: String,
    /// `true` when `apply` was not passed (plan only, no writes).
    pub dry_run: bool,
    /// The proposed graph edit.
    pub graph_edit: GraphEdit,
    /// The proposed text edits (may be empty if `path` was not given).
    pub text_edits: Vec<TextEdit>,
}

/// JSON-serializable view of a graph node rename.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphEdit {
    pub node_id: String,
    pub label: String,
    pub old_name: String,
    pub new_name: String,
    pub old_qualified_name: String,
    pub new_qualified_name: String,
}

/// JSON-serializable view of a single text replacement.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TextEdit {
    /// File path (relative to `path` if possible).
    pub file_path: String,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column (byte offset within the line + 1).
    pub column: usize,
    /// The text to be replaced (the old name).
    pub old_text: String,
    /// The replacement text (the new name).
    pub new_text: String,
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "rename",
    version = "0.3.3",
    description = "Propose graph + text edits for renaming a symbol.",
    cli = true
)]
async fn rename(from: String, to: String, path: String, apply: bool) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;

    if !is_valid_identifier(&to) {
        return Err(ApiError::InvalidInput {
            message: format!(
                "new name '{to}' is not a valid identifier (allowed: [A-Za-z_][A-Za-z0-9_]*)"
            ),
            field: Some("to".to_string()),
            value: Some(Value::String(to)),
        });
    }
    let path_opt = if path.is_empty() {
        None
    } else {
        Some(path.as_str())
    };
    if apply && path_opt.is_none() {
        return Err(ApiError::InvalidInput {
            message: "apply requires path (text edits need a codebase root to scan)".to_string(),
            field: Some("path".to_string()),
            value: None,
        });
    }

    let trace = kit
        .require::<TraceModule>()
        .map_err(|e| wrap_kit_error("Failed to resolve trace capability", e))?;
    let graph = trace.load_graph(&from, 2).map_err(|e| match e {
        TraceError::SymbolNotFound(s) => ApiError::NotFound {
            resource: "symbol".to_string(),
            resource_id: Some(s),
        },
        other => wrap_error("Failed to load graph", other),
    })?;
    let start_id = resolve_start_id(&graph, &from).ok_or_else(|| ApiError::NotFound {
        resource: "symbol".to_string(),
        resource_id: Some(from.clone()),
    })?;
    let symbol_node = graph
        .get_node(&start_id)
        .ok_or_else(|| ApiError::NotFound {
            resource: "symbol".to_string(),
            resource_id: Some(from.clone()),
        })?;

    let old_name = symbol_node.name.clone();
    let old_qn = symbol_node.qualified_name.clone();
    let new_qn = compute_new_qn(&old_qn, &old_name, &to);

    let graph_edit = GraphEdit {
        node_id: start_id.clone(),
        label: symbol_node.label.to_string(),
        old_name: old_name.clone(),
        new_name: to.clone(),
        old_qualified_name: old_qn.clone(),
        new_qualified_name: new_qn.clone(),
    };

    let text_edits = match path_opt {
        Some(root) => {
            let candidate_files = collect_candidate_files(&graph, &start_id, Path::new(root));
            scan_text_edits(Path::new(root), &old_name, &to, &candidate_files)
                .map_err(|e| to_api_error(e, "rename_error"))?
        }
        None => Vec::new(),
    };

    if apply {
        apply_graph_edit(&kit, &graph_edit).map_err(|e| to_api_error(e, "rename_error"))?;
        apply_text_edits(&text_edits).map_err(|e| to_api_error(e, "rename_error"))?;
    }

    let output = RenameOutput {
        from,
        to,
        dry_run: !apply,
        graph_edit,
        text_edits,
    };
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use crate::model::{Edge, EdgeType, Node, NodeLabel};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_rename_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &str) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Core logic mirroring the service function, taking explicit params
    /// (no RenameArgs) so tests can exercise error paths without the
    /// `#[forge]` macro wrapper.
    fn rename_core(
        kit: &AsyncKit<AsyncReady>,
        from: &str,
        to: &str,
        path: Option<&str>,
        apply: bool,
    ) -> Result<(), CodeNexusError> {
        if !is_valid_identifier(to) {
            return Err(CodeNexusError::InvalidInput(format!(
                "new name '{to}' is not a valid identifier (allowed: [A-Za-z_][A-Za-z0-9_]*)"
            )));
        }
        if apply && path.is_none() {
            return Err(CodeNexusError::InvalidInput(
                "--apply requires --path (text edits need a codebase root to scan)".into(),
            ));
        }

        let trace = kit.require::<TraceModule>()?;
        let graph = trace.load_graph(from, 2)?;
        let start_id = resolve_start_id(&graph, from)
            .ok_or_else(|| TraceError::SymbolNotFound(from.to_string()))?;
        let symbol_node = graph
            .get_node(&start_id)
            .ok_or_else(|| TraceError::SymbolNotFound(from.to_string()))?;

        let old_name = symbol_node.name.clone();
        let old_qn = symbol_node.qualified_name.clone();
        let new_qn = compute_new_qn(&old_qn, &old_name, to);

        let graph_edit = GraphEdit {
            node_id: start_id.clone(),
            label: symbol_node.label.to_string(),
            old_name: old_name.clone(),
            new_name: to.to_string(),
            old_qualified_name: old_qn.clone(),
            new_qualified_name: new_qn.clone(),
        };

        let text_edits = match path {
            Some(root) => {
                let candidate_files = collect_candidate_files(&graph, &start_id, Path::new(root));
                scan_text_edits(Path::new(root), &old_name, to, &candidate_files)?
            }
            None => Vec::new(),
        };

        if apply {
            apply_graph_edit(kit, &graph_edit)?;
            apply_text_edits(&text_edits)?;
        }

        let output = RenameOutput {
            from: from.to_string(),
            to: to.to_string(),
            dry_run: !apply,
            graph_edit,
            text_edits,
        };
        let json = serde_json::to_string(&output)?;
        println!("{json}");
        Ok(())
    }

    // --- is_valid_identifier ---

    #[test]
    fn is_valid_identifier_accepts_simple() {
        assert!(is_valid_identifier("foo"));
        assert!(is_valid_identifier("_bar"));
        assert!(is_valid_identifier("foo_bar_123"));
    }

    #[test]
    fn is_valid_identifier_rejects_empty() {
        assert!(!is_valid_identifier(""));
    }

    #[test]
    fn is_valid_identifier_rejects_digit_start() {
        assert!(!is_valid_identifier("1foo"));
    }

    #[test]
    fn is_valid_identifier_rejects_special_chars() {
        assert!(!is_valid_identifier("foo-bar"));
        assert!(!is_valid_identifier("foo bar"));
    }

    // --- compute_new_qn ---

    #[test]
    fn compute_new_qn_replaces_trailing_segment() {
        assert_eq!(compute_new_qn("demo.foo", "foo", "bar"), "demo.bar");
    }

    #[test]
    fn compute_new_qn_when_qn_equals_name() {
        assert_eq!(compute_new_qn("foo", "foo", "bar"), "bar");
    }

    #[test]
    fn compute_new_qn_preserves_file_extension_in_qn() {
        assert_eq!(compute_new_qn("proj.a.py.A", "A", "B"), "proj.a.py.B");
    }

    #[test]
    fn compute_new_qn_no_match_returns_unchanged() {
        assert_eq!(compute_new_qn("demo.other", "foo", "bar"), "demo.other");
    }

    #[test]
    fn compute_new_qn_does_not_replace_substring() {
        assert_eq!(compute_new_qn("demo.foobar", "bar", "baz"), "demo.foobar");
    }

    // --- find_word_occurrences ---

    #[test]
    fn find_word_occurrences_simple() {
        let positions = find_word_occurrences("foo + foo = foo", "foo");
        assert_eq!(positions, vec![0, 6, 12]);
    }

    #[test]
    fn find_word_occurrences_respects_word_boundaries() {
        let positions = find_word_occurrences("foobar foo foobaz", "foo");
        assert_eq!(positions, vec![7]);
    }

    #[test]
    fn find_word_occurrences_at_line_start_and_end() {
        let positions = find_word_occurrences("foo (foo)", "foo");
        assert_eq!(positions, vec![0, 5]);
    }

    #[test]
    fn find_word_occurrences_underscore_is_part_of_word() {
        let positions = find_word_occurrences("foo foo_bar _foo", "foo");
        assert_eq!(positions, vec![0]);
    }

    #[test]
    fn find_word_occurrences_empty_needle() {
        assert!(find_word_occurrences("abc", "").is_empty());
    }

    // --- file_to_rel_string ---

    #[test]
    fn file_to_rel_string_strips_root_prefix() {
        let root = Path::new("/repo");
        let file = Path::new("/repo/src/main.rs");
        assert_eq!(file_to_rel_string(file, root), "src/main.rs");
    }

    #[test]
    fn file_to_rel_string_keeps_absolute_when_not_under_root() {
        let root = Path::new("/other");
        let file = Path::new("/repo/src/main.rs");
        assert_eq!(file_to_rel_string(file, root), "/repo/src/main.rs");
    }

    // --- apply_replacements ---

    #[test]
    fn apply_replacements_single_edit() {
        let edits = [TextEdit {
            file_path: "x".into(),
            line: 1,
            column: 1,
            old_text: "foo".into(),
            new_text: "bar".into(),
        }];
        let refs: Vec<&TextEdit> = edits.iter().collect();
        let result = apply_replacements("foo + baz", &refs);
        assert_eq!(result, "bar + baz");
    }

    #[test]
    fn apply_replacements_multiple_edits_same_line() {
        let edits = [
            TextEdit {
                file_path: "x".into(),
                line: 1,
                column: 1,
                old_text: "foo".into(),
                new_text: "bar".into(),
            },
            TextEdit {
                file_path: "x".into(),
                line: 1,
                column: 7,
                old_text: "foo".into(),
                new_text: "bar".into(),
            },
        ];
        let refs: Vec<&TextEdit> = edits.iter().collect();
        let result = apply_replacements("foo + foo", &refs);
        assert_eq!(result, "bar + bar");
    }

    #[test]
    fn apply_replacements_preserves_trailing_newline() {
        let edits = [TextEdit {
            file_path: "x".into(),
            line: 1,
            column: 1,
            old_text: "foo".into(),
            new_text: "bar".into(),
        }];
        let refs: Vec<&TextEdit> = edits.iter().collect();
        let result = apply_replacements("foo\n", &refs);
        assert_eq!(result, "bar\n");
    }

    // --- GraphEdit serialization ---

    #[test]
    fn graph_edit_serializes_to_json() {
        let edit = GraphEdit {
            node_id: "id1".into(),
            label: "Function".into(),
            old_name: "foo".into(),
            new_name: "bar".into(),
            old_qualified_name: "demo.foo".into(),
            new_qualified_name: "demo.bar".into(),
        };
        let json = serde_json::to_string(&edit).unwrap();
        assert!(json.contains("\"old_name\":\"foo\""));
        assert!(json.contains("\"new_name\":\"bar\""));
    }

    // --- resolve_start_id ---

    #[test]
    fn resolve_start_id_by_name() {
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("id1")
                .build(),
        );
        assert_eq!(resolve_start_id(&graph, "foo").as_deref(), Some("id1"));
    }

    #[test]
    fn resolve_start_id_by_qualified_name() {
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("id1")
                .build(),
        );
        assert_eq!(resolve_start_id(&graph, "demo.foo").as_deref(), Some("id1"));
    }

    // --- collect_candidate_files ---

    #[test]
    fn collect_candidate_files_includes_symbol_and_neighbors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let file_a = root.join("a.rs");
        let file_b = root.join("b.rs");
        std::fs::write(&file_a, "fn foo() {}\n").unwrap();
        std::fs::write(&file_b, "fn bar() {}\n").unwrap();

        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("f1")
                .file_path(file_a.to_str().unwrap())
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Function, "bar", "demo.bar")
                .id("f2")
                .file_path(file_b.to_str().unwrap())
                .build(),
        );
        graph.add_edge(Edge::new("f1", "f2", EdgeType::Calls, "demo"));

        let files = collect_candidate_files(&graph, &"f1".to_string(), root);
        assert_eq!(
            files.len(),
            2,
            "should include both symbol file and neighbor file"
        );
    }

    #[test]
    fn collect_candidate_files_with_relative_file_path_joins_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("f1")
                .file_path("src/main.rs")
                .build(),
        );
        let files = collect_candidate_files(&graph, &"f1".to_string(), root);
        assert_eq!(
            files.len(),
            1,
            "relative file_path should be joined with root"
        );
        assert_eq!(files[0], root.join("src/main.rs"));
    }

    #[test]
    fn collect_candidate_files_includes_incoming_edge_source() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let file_a = root.join("a.rs");
        let file_b = root.join("b.rs");
        std::fs::write(&file_a, "fn foo() {}\n").unwrap();
        std::fs::write(&file_b, "fn bar() {}\n").unwrap();

        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("f1")
                .file_path(file_a.to_str().unwrap())
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Function, "bar", "demo.bar")
                .id("f2")
                .file_path(file_b.to_str().unwrap())
                .build(),
        );
        graph.add_edge(Edge::new("f2", "f1", EdgeType::Calls, "demo"));

        let files = collect_candidate_files(&graph, &"f1".to_string(), root);
        assert_eq!(
            files.len(),
            2,
            "should include both f1's file and f2's file (incoming edge source)"
        );
    }

    // --- scan_text_edits ---

    #[test]
    fn scan_text_edits_finds_word_occurrences_in_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        std::fs::write(&file, "fn foo() { foo(); }\n").unwrap();
        let files = vec![file];
        let edits = scan_text_edits(tmp.path(), "foo", "bar", &files).unwrap();
        assert_eq!(edits.len(), 2, "should find both 'foo' occurrences");
        assert_eq!(edits[0].old_text, "foo");
        assert_eq!(edits[0].new_text, "bar");
        assert_eq!(edits[0].line, 1);
        assert_eq!(edits[0].column, 4);
        assert_eq!(edits[0].file_path, "a.rs");
    }

    #[test]
    fn scan_text_edits_skips_unreadable_files() {
        let tmp = TempDir::new().unwrap();
        let files = vec![tmp.path().join("nonexistent.rs")];
        let edits = scan_text_edits(tmp.path(), "foo", "bar", &files).unwrap();
        assert!(edits.is_empty(), "unreadable files should be skipped");
    }

    // --- apply_text_edits ---

    #[test]
    fn apply_text_edits_writes_replacements_to_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        std::fs::write(&file, "fn foo() {}\n").unwrap();
        let edits = vec![TextEdit {
            file_path: file.to_string_lossy().into_owned(),
            line: 1,
            column: 4,
            old_text: "foo".to_string(),
            new_text: "bar".to_string(),
        }];
        let result = apply_text_edits(&edits);
        assert!(
            result.is_ok(),
            "apply_text_edits should succeed: {:?}",
            result.err()
        );
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            content, "fn bar() {}\n",
            "file should have foo replaced with bar"
        );
    }

    #[test]
    fn apply_text_edits_handles_multiple_files() {
        let tmp = TempDir::new().unwrap();
        let file_a = tmp.path().join("a.rs");
        let file_b = tmp.path().join("b.rs");
        std::fs::write(&file_a, "fn foo() {}\n").unwrap();
        std::fs::write(&file_b, "let foo = 1;\n").unwrap();
        let edits = vec![
            TextEdit {
                file_path: file_a.to_string_lossy().into_owned(),
                line: 1,
                column: 4,
                old_text: "foo".to_string(),
                new_text: "bar".to_string(),
            },
            TextEdit {
                file_path: file_b.to_string_lossy().into_owned(),
                line: 1,
                column: 5,
                old_text: "foo".to_string(),
                new_text: "bar".to_string(),
            },
        ];
        let result = apply_text_edits(&edits);
        assert!(
            result.is_ok(),
            "apply_text_edits should succeed: {:?}",
            result.err()
        );
        assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "fn bar() {}\n");
        assert_eq!(std::fs::read_to_string(&file_b).unwrap(), "let bar = 1;\n");
    }

    // --- rename_core error cases ---

    #[test]
    fn core_invalid_new_name_returns_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err =
            rename_core(&kit, "foo", "1bad", None, false).expect_err("invalid name should error");
        assert_eq!(err.exit_code(), 2, "InvalidInput → exit 2");
    }

    #[test]
    fn core_apply_without_path_returns_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = rename_core(&kit, "foo", "bar", None, true)
            .expect_err("apply without path should error");
        assert_eq!(err.exit_code(), 2, "InvalidInput → exit 2");
    }

    #[test]
    fn core_missing_symbol_returns_trace_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = rename_core(&kit, "nonexistent", "bar", None, false)
            .expect_err("missing symbol should error");
        assert_eq!(err.exit_code(), 2, "TraceError::SymbolNotFound → exit 2");
    }

    // --- rename_core success ---

    #[test]
    fn core_dry_run_succeeds_with_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        let result = rename_core(&kit, "a", "b", None, false);
        assert!(
            result.is_ok(),
            "dry-run rename should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn core_apply_updates_graph_name() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        let tmp = TempDir::new().unwrap();
        let result = rename_core(&kit, "a", "b", Some(tmp.path().to_str().unwrap()), true);
        assert!(
            result.is_ok(),
            "apply rename should succeed: {:?}",
            result.err()
        );
        let rows = storage
            .query("MATCH (n:Function) WHERE n.id = 'f_a' RETURN n.name AS name;")
            .unwrap();
        let name = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(name, "b", "graph name should be updated to 'b'");
    }

    // --- resolve_start_id: multiple matches ---

    #[test]
    fn resolve_start_id_multiple_name_matches_falls_back_to_a_match() {
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.a.foo")
                .id("id1")
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.b.foo")
                .id("id2")
                .build(),
        );
        // Two nodes named "foo"; no QN match → falls back to some node with
        // this name. HashMap iteration order is non-deterministic, so we only
        // assert that one of the two matching ids is returned.
        let result = resolve_start_id(&graph, "foo");
        assert!(
            matches!(result.as_deref(), Some("id1") | Some("id2")),
            "expected Some(\"id1\") or Some(\"id2\"), got {result:?}"
        );
    }

    #[test]
    fn resolve_start_id_multiple_names_but_single_qn_match() {
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.a")
                .id("id1")
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.b")
                .id("id2")
                .build(),
        );
        // Two "foo" by name, but "demo.a" matches by QN → returns id1.
        assert_eq!(resolve_start_id(&graph, "demo.a").as_deref(), Some("id1"));
    }

    #[test]
    fn resolve_start_id_no_match_returns_none() {
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("id1")
                .build(),
        );
        assert!(resolve_start_id(&graph, "bar").is_none());
    }

    // --- collect_candidate_files: edge cases ---

    #[test]
    fn collect_candidate_files_excludes_absolute_paths_not_under_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("f1")
                .file_path("/other/path/a.rs")
                .build(),
        );
        let files = collect_candidate_files(&graph, &"f1".to_string(), root);
        assert!(
            files.is_empty(),
            "absolute path not under root should be excluded"
        );
    }

    #[test]
    fn collect_candidate_files_with_nonexistent_start_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("f1")
                .file_path("a.rs")
                .build(),
        );
        let files = collect_candidate_files(&graph, &"nonexistent".to_string(), root);
        assert!(files.is_empty(), "nonexistent start_id should return empty");
    }

    #[test]
    fn collect_candidate_files_node_without_file_path() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("f1")
                .build(),
        );
        let files = collect_candidate_files(&graph, &"f1".to_string(), root);
        assert!(
            files.is_empty(),
            "node without file_path should return empty"
        );
    }

    // --- apply_replacements: edge cases ---

    #[test]
    fn apply_replacements_skips_edits_beyond_file_lines() {
        let edits = [TextEdit {
            file_path: "x".into(),
            line: 100,
            column: 1,
            old_text: "foo".into(),
            new_text: "bar".into(),
        }];
        let refs: Vec<&TextEdit> = edits.iter().collect();
        let result = apply_replacements("foo\n", &refs);
        assert_eq!(result, "foo\n", "out-of-bounds edit should be skipped");
    }

    #[test]
    fn apply_replacements_skips_when_text_mismatch() {
        let edits = [TextEdit {
            file_path: "x".into(),
            line: 1,
            column: 1,
            old_text: "foo".into(),
            new_text: "bar".into(),
        }];
        let refs: Vec<&TextEdit> = edits.iter().collect();
        let result = apply_replacements("baz\n", &refs);
        assert_eq!(result, "baz\n", "mismatched edit should be skipped");
    }

    #[test]
    fn apply_replacements_no_trailing_newline_preserved() {
        let edits = [TextEdit {
            file_path: "x".into(),
            line: 1,
            column: 1,
            old_text: "foo".into(),
            new_text: "bar".into(),
        }];
        let refs: Vec<&TextEdit> = edits.iter().collect();
        let result = apply_replacements("foo", &refs);
        assert_eq!(result, "bar", "no trailing newline should be preserved");
    }

    // --- apply_text_edits: error path ---

    #[test]
    fn apply_text_edits_fails_on_unreadable_file() {
        let edits = vec![TextEdit {
            file_path: "/nonexistent/path/file.rs".to_string(),
            line: 1,
            column: 1,
            old_text: "foo".to_string(),
            new_text: "bar".to_string(),
        }];
        let result = apply_text_edits(&edits);
        assert!(result.is_err(), "should fail on unreadable file");
    }

    // --- find_word_occurrences: not found ---

    #[test]
    fn find_word_occurrences_needle_not_found() {
        assert!(find_word_occurrences("hello world", "foo").is_empty());
    }

    // --- is_valid_identifier: single char ---

    #[test]
    fn is_valid_identifier_single_char() {
        assert!(is_valid_identifier("a"));
        assert!(is_valid_identifier("_"));
        assert!(!is_valid_identifier("1"));
    }

    // --- is_ident_byte ---

    #[test]
    fn is_ident_byte_classifies_correctly() {
        assert!(is_ident_byte(b'a'));
        assert!(is_ident_byte(b'Z'));
        assert!(is_ident_byte(b'0'));
        assert!(is_ident_byte(b'_'));
        assert!(!is_ident_byte(b'-'));
        assert!(!is_ident_byte(b' '));
        assert!(!is_ident_byte(b'.'));
    }

    // --- scan_text_edits: multiple files ---

    #[test]
    fn scan_text_edits_across_multiple_files() {
        let tmp = TempDir::new().unwrap();
        let file_a = tmp.path().join("a.rs");
        let file_b = tmp.path().join("b.rs");
        std::fs::write(&file_a, "fn foo() {}\n").unwrap();
        std::fs::write(&file_b, "let foo = 1;\n").unwrap();
        let files = vec![file_a, file_b];
        let edits = scan_text_edits(tmp.path(), "foo", "bar", &files).unwrap();
        assert_eq!(edits.len(), 2, "should find 'foo' in both files");
    }

    // --- rename_core: dry run with path but no matching files ---

    #[test]
    fn core_dry_run_with_path_succeeds_without_text_edits() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/nonexistent/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        let tmp = TempDir::new().unwrap();
        let result = rename_core(&kit, "a", "b", Some(tmp.path().to_str().unwrap()), false);
        assert!(
            result.is_ok(),
            "dry-run with path should succeed: {:?}",
            result.err()
        );
    }

    // --- rename_core: apply path with file not under root (text edits empty, graph edit applied) ---

    #[test]
    fn core_apply_with_file_not_under_root_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        // filePath is absolute but NOT under the passed root → collect_candidate_files
        // excludes it → text_edits empty → apply_text_edits is a no-op.
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        let tmp = TempDir::new().unwrap();
        let result = rename_core(&kit, "a", "b", Some(tmp.path().to_str().unwrap()), true);
        assert!(
            result.is_ok(),
            "apply with file not under root should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn core_dry_run_with_qualified_name_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();

        // Resolve by qualified name instead of short name
        let result = rename_core(&kit, "demo.a", "b", None, false);
        assert!(
            result.is_ok(),
            "dry-run by qualified name should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn core_apply_updates_qualified_name_in_graph() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        let tmp = TempDir::new().unwrap();
        let result = rename_core(&kit, "a", "b", Some(tmp.path().to_str().unwrap()), true);
        assert!(result.is_ok(), "apply should succeed: {:?}", result.err());

        let rows = storage
            .query("MATCH (n:Function) WHERE n.id = 'f_a' RETURN n.qualifiedName AS qn;")
            .unwrap();
        let qn = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(qn, "demo.b", "qualified name should be updated to 'demo.b'");
    }

    #[test]
    fn core_dry_run_with_neighbor_files_collects_candidates() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        let tmp = TempDir::new().unwrap();
        let file_a = tmp.path().join("a.rs");
        let file_b = tmp.path().join("b.rs");
        std::fs::write(&file_a, "fn a() { b(); }\n").unwrap();
        std::fs::write(&file_b, "fn b() {}\n").unwrap();
        storage.execute(format!(
            "CREATE (:Function {{id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '{}', startLine: 1, endLine: 1, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});",
            file_a.to_str().unwrap()
        ).as_str()).unwrap();
        storage.execute(format!(
            "CREATE (:Function {{id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '{}', startLine: 1, endLine: 1, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});",
            file_b.to_str().unwrap()
        ).as_str()).unwrap();
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").unwrap();

        // Dry-run: should find 'a' in both a.rs and b.rs (neighbor file)
        let result = rename_core(
            &kit,
            "a",
            "renamed_a",
            Some(tmp.path().to_str().unwrap()),
            false,
        );
        assert!(
            result.is_ok(),
            "dry-run with neighbors should succeed: {:?}",
            result.err()
        );
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn rename_wrapper_fails_with_invalid_identifier() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(rename(
            "foo".to_string(),
            "1bad".to_string(),
            String::new(),
            false,
        ));
        let err = result.expect_err("invalid identifier should error");
        assert!(
            matches!(err, ApiError::InvalidInput { .. }),
            "expected InvalidInput, got {err:?}"
        );

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn rename_wrapper_fails_with_apply_without_path() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(rename(
            "foo".to_string(),
            "bar".to_string(),
            String::new(),
            true,
        ));
        let err = result.expect_err("apply without path should error");
        assert!(
            matches!(err, ApiError::InvalidInput { .. }),
            "expected InvalidInput, got {err:?}"
        );

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn rename_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(rename(
            "foo".to_string(),
            "bar".to_string(),
            String::new(),
            false,
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn rename_wrapper_succeeds_dry_run() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create symbol");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(rename(
            "a".to_string(),
            "b".to_string(),
            String::new(),
            false,
        ));
        assert!(
            result.is_ok(),
            "dry-run rename should succeed: {:?}",
            result.err()
        );

        reset_kit_for_testing();
    }

    // Covers the wrapper apply=true success path (lines 351-354):
    // apply_graph_edit + apply_text_edits through the #[forge] wrapper.
    // Uses an absolute filePath NOT under the root so that
    // collect_candidate_files excludes it → text_edits empty →
    // apply_text_edits is a no-op (same pattern as core_apply_updates_graph_name).
    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn rename_wrapper_succeeds_with_apply() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create symbol");
        init_kit(kit).expect("init_kit");

        let tmp = TempDir::new().unwrap();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(rename(
            "a".to_string(),
            "b".to_string(),
            tmp.path().to_string_lossy().into_owned(),
            true,
        ));
        assert!(
            result.is_ok(),
            "apply rename should succeed: {:?}",
            result.err()
        );

        // Verify the graph was updated.
        let rows = storage
            .query("MATCH (n:Function) WHERE n.id = 'f_a' RETURN n.name AS name;")
            .unwrap();
        let name = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(name, "b", "graph name should be updated to 'b'");

        reset_kit_for_testing();
    }

    // Covers the wrapper symbol-not-found error path (lines 311-321):
    // load_graph returns SymbolNotFound → ApiError::NotFound.
    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn rename_wrapper_fails_with_symbol_not_found() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(rename(
            "nonexistent_symbol".to_string(),
            "bar".to_string(),
            String::new(),
            false,
        ));
        let err = result.expect_err("missing symbol should error");
        assert!(
            matches!(err, ApiError::NotFound { .. }),
            "expected NotFound, got {err:?}"
        );

        reset_kit_for_testing();
    }

    // Covers the wrapper dry-run with path (lines 342-349):
    // text_edits are computed but not applied.
    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn rename_wrapper_succeeds_dry_run_with_path() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");
        let tmp = TempDir::new().unwrap();
        let file_a = tmp.path().join("a.rs");
        std::fs::write(&file_a, "fn a() {}\n").unwrap();
        storage.execute(format!(
            "CREATE (:Function {{id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '{}', startLine: 1, endLine: 1, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});",
            file_a.to_str().unwrap()
        ).as_str()).expect("create symbol");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(rename(
            "a".to_string(),
            "b".to_string(),
            tmp.path().to_string_lossy().into_owned(),
            false,
        ));
        assert!(
            result.is_ok(),
            "dry-run with path should succeed: {:?}",
            result.err()
        );

        // Verify the file was NOT modified (dry run).
        let content = std::fs::read_to_string(&file_a).unwrap();
        assert!(
            content.contains("fn a()"),
            "file should be unchanged in dry run: {content}"
        );

        reset_kit_for_testing();
    }
}
