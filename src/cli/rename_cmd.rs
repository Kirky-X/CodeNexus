// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `rename` subcommand handler (H8).
//!
//! Proposes a two-part rename plan for a symbol:
//!
//! 1. **Graph edits** — direct Cypher `SET` on the symbol node's `name` and
//!    `qualifiedName`. Applied when `--apply` is passed; safe because the
//!    node id is stable across the rename.
//! 2. **Text edits** — word-boundary find/replace of the old name in source
//!    files under `--path`. Each occurrence is listed for review; `--apply`
//!    writes them to disk.
//!
//! `--dry-run` (the default) prints the plan as JSON without touching the
//! database or the filesystem.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::args::RenameArgs;
use super::error::{CliError, Result};
use crate::kit::{Kit, StorageKey, TraceKey};
use crate::model::{Graph, Node, NodeId};
use crate::storage::schema::escape_cypher_string;
use crate::trace::TraceError;

/// Runs the `rename` subcommand.
///
/// # Errors
///
/// Returns [`CliError::Trace`] if the symbol is not found. Returns
/// [`CliError::InvalidInput`] if `--apply` is passed without `--path` (text
/// edits require a codebase root) or if the new name is not a valid
/// identifier. Returns [`CliError::Kit`] if a required capability is not
/// registered.
pub fn run(kit: &Kit, args: &RenameArgs) -> Result<()> {
    if !is_valid_identifier(&args.to) {
        return Err(CliError::InvalidInput(format!(
            "new name '{}' is not a valid identifier (allowed: [A-Za-z_][A-Za-z0-9_]*)",
            args.to
        )));
    }
    if args.apply && args.path.is_none() {
        return Err(CliError::InvalidInput(
            "--apply requires --path (text edits need a codebase root to scan)".into(),
        ));
    }

    let trace = kit.require::<TraceKey>()?;
    let graph = trace.load_graph(&args.from, 2)?;
    let start_id = resolve_start_id(&graph, &args.from)
        .ok_or_else(|| TraceError::SymbolNotFound(args.from.clone()))?;
    let symbol_node = graph
        .get_node(&start_id)
        .ok_or_else(|| TraceError::SymbolNotFound(args.from.clone()))?;

    let old_name = symbol_node.name.clone();
    let old_qn = symbol_node.qualified_name.clone();
    let new_qn = compute_new_qn(&old_qn, &old_name, &args.to);

    let graph_edit = GraphEdit {
        node_id: start_id.clone(),
        label: symbol_node.label.to_string(),
        old_name: old_name.clone(),
        new_name: args.to.clone(),
        old_qualified_name: old_qn.clone(),
        new_qualified_name: new_qn.clone(),
    };

    let text_edits = match &args.path {
        Some(root) => {
            let candidate_files = collect_candidate_files(&graph, &start_id, Path::new(root));
            scan_text_edits(Path::new(root), &old_name, &args.to, &candidate_files)?
        }
        None => Vec::new(),
    };

    if args.apply {
        apply_graph_edit(kit, &graph_edit)?;
        apply_text_edits(&text_edits)?;
    }

    let output = RenameOutput {
        from: args.from.clone(),
        to: args.to.clone(),
        dry_run: !args.apply,
        graph_edit,
        text_edits,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

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
///
/// If `old_qn` ends with `old_name`, the suffix is replaced. Otherwise the QN
/// is left unchanged (the rename only affects the short name).
fn compute_new_qn(old_qn: &str, old_name: &str, new_name: &str) -> String {
    if let Some(stripped) = old_qn.strip_suffix(old_name) {
        // Only replace if the preceding char is a separator (`.`) or the QN
        // equals the old name outright — avoids replacing substrings.
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
fn collect_candidate_files(
    graph: &Graph,
    start_id: &NodeId,
    root: &Path,
) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let root_canonical = root.to_path_buf();
    let collect = |node: &Node, files: &mut Vec<PathBuf>, seen: &mut std::collections::HashSet<PathBuf>| {
        if let Some(fp) = &node.file_path {
            let p = PathBuf::from(fp);
            // Accept both absolute and relative paths; the caller will join
            // with root if needed.
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
/// returns the proposed text edits (each replacing `old_name` with `new_name`).
fn scan_text_edits(
    root: &Path,
    old_name: &str,
    new_name: &str,
    files: &[PathBuf],
) -> Result<Vec<TextEdit>> {
    let mut edits: Vec<TextEdit> = Vec::new();
    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue, // skip binary / unreadable files
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
/// `haystack`. A word boundary means the character before/after the match is
/// not `[A-Za-z0-9_]`.
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
            let after_ok = i + needle.len() == bytes.len() || !is_ident_byte(bytes[i + needle.len()]);
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
fn apply_graph_edit(kit: &Kit, edit: &GraphEdit) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let table = edit.label.as_str();
    // Update name and qualifiedName for the node.
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
fn apply_text_edits(edits: &[TextEdit]) -> Result<()> {
    // Group edits by file.
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
/// position in `edits`, replacing `old_text` with `new_text`.
fn apply_replacements(content: &str, edits: &[&TextEdit]) -> String {
    // Sort edits by line then column (descending so byte offsets don't shift).
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
        // Replace the word at (line, col) — we know it's `old_text` length.
        let end = col + edit.old_text.len();
        if end <= line.len() && line[col..end] == edit.old_text {
            line.replace_range(col..end, &edit.new_text);
        }
    }
    // Rejoin with newlines. Note: this normalizes trailing newline to a single
    // `\n` if the original ended with one.
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
    /// `true` when `--apply` was not passed (plan only, no writes).
    pub dry_run: bool,
    /// The proposed graph edit.
    pub graph_edit: GraphEdit,
    /// The proposed text edits (may be empty if `--path` was not given).
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
    /// File path (relative to `--path` if possible).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::RenameArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use crate::model::{Edge, EdgeType, Node, NodeLabel};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_rename_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    fn make_args(from: &str, to: &str, db: &str, path: Option<&str>, apply: bool) -> RenameArgs {
        RenameArgs {
            from: from.to_string(),
            to: to.to_string(),
            db: db.to_string(),
            path: path.map(String::from),
            apply,
        }
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
        // ADR-001: QN retains file extension: proj.a.py.A
        assert_eq!(compute_new_qn("proj.a.py.A", "A", "B"), "proj.a.py.B");
    }

    #[test]
    fn compute_new_qn_no_match_returns_unchanged() {
        // QN does not end with old name → leave unchanged.
        assert_eq!(compute_new_qn("demo.other", "foo", "bar"), "demo.other");
    }

    #[test]
    fn compute_new_qn_does_not_replace_substring() {
        // "foobar" ends with "bar" but "foobar" is not a separator-delimited suffix.
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
        // Only the standalone "foo" at position 7 matches.
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
        // "foo" at 0 matches; "foo" in "foo_bar" at 4 does not (underscore follows);
        // "foo" in "_foo" at 9 does not (underscore precedes).
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
        assert_eq!(
            resolve_start_id(&graph, "demo.foo").as_deref(),
            Some("id1")
        );
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
        assert_eq!(files.len(), 2, "should include both symbol file and neighbor file");
    }

    // --- run() error cases ---

    #[test]
    fn run_invalid_new_name_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("foo", "1bad", db.to_str().unwrap(), None, false);
        let err = run(&kit, &args).expect_err("invalid name should error");
        assert_eq!(err.exit_code(), 2, "InvalidInput → exit 2");
    }

    #[test]
    fn run_apply_without_path_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("foo", "bar", db.to_str().unwrap(), None, true);
        let err = run(&kit, &args).expect_err("--apply without --path should error");
        assert_eq!(err.exit_code(), 2, "InvalidInput → exit 2");
    }

    #[test]
    fn run_missing_symbol_returns_trace_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("nonexistent", "bar", db.to_str().unwrap(), None, false);
        let err = run(&kit, &args).expect_err("missing symbol should error");
        assert_eq!(err.exit_code(), 2, "TraceError::SymbolNotFound → exit 2");
    }

    // --- run() success ---

    #[test]
    fn run_dry_run_succeeds_with_symbol() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        let args = make_args("a", "b", db.to_str().unwrap(), None, false);
        let result = run(&kit, &args);
        assert!(result.is_ok(), "dry-run rename should succeed: {:?}", result.err());
    }

    #[test]
    fn run_apply_updates_graph_name() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().unwrap();
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        // --apply requires --path; pass an empty temp dir (no source files to edit,
        // so text_edits will be empty, but the graph edit still applies).
        let tmp = TempDir::new().unwrap();
        let args = make_args("a", "b", db.to_str().unwrap(), Some(tmp.path().to_str().unwrap()), true);
        let result = run(&kit, &args);
        assert!(result.is_ok(), "apply rename should succeed: {:?}", result.err());
        // Verify the name was updated in the graph.
        let rows = storage
            .query("MATCH (n:Function) WHERE n.id = 'f_a' RETURN n.name AS name;")
            .unwrap();
        let name = rows.first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(name, "b", "graph name should be updated to 'b'");
    }

    // --- collect_candidate_files: relative file_path branch (lines 162-163) ---

    #[test]
    fn collect_candidate_files_with_relative_file_path_joins_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mut graph = Graph::new();
        graph.add_node(
            Node::builder(NodeLabel::Function, "foo", "demo.foo")
                .id("f1")
                .file_path("src/main.rs") // relative path
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

    // --- collect_candidate_files: incoming edge branch (lines 175-177) ---

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
        // Edge: f2 calls f1 — f1 is the TARGET (exercises the
        // `else if edge.target == *start_id` branch).
        graph.add_edge(Edge::new("f2", "f1", EdgeType::Calls, "demo"));

        let files = collect_candidate_files(&graph, &"f1".to_string(), root);
        assert_eq!(
            files.len(),
            2,
            "should include both f1's file and f2's file (incoming edge source)"
        );
    }

    // --- scan_text_edits: reading files and creating edits (lines 194-205) ---

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
        // "fn " is 3 bytes → first 'foo' at byte offset 3 → column 4.
        assert_eq!(edits[0].column, 4);
        // file_path should be relative to root.
        assert_eq!(edits[0].file_path, "a.rs");
    }

    #[test]
    fn scan_text_edits_skips_unreadable_files() {
        let tmp = TempDir::new().unwrap();
        let files = vec![tmp.path().join("nonexistent.rs")];
        let edits = scan_text_edits(tmp.path(), "foo", "bar", &files).unwrap();
        assert!(edits.is_empty(), "unreadable files should be skipped");
    }

    // --- apply_text_edits: writing replacements (lines 272, 275-278) ---

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
        assert!(result.is_ok(), "apply_text_edits should succeed: {:?}", result.err());
        assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "fn bar() {}\n");
        assert_eq!(std::fs::read_to_string(&file_b).unwrap(), "let bar = 1;\n");
    }
}
