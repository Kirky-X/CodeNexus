// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CSV batch loading (ADR-014).
//!
//! Generates RFC 4180-compliant CSV for node and edge tables using the [`csv`]
//! crate, then loads them into LadybugDB via `COPY FROM`. This is the
//! recommended bulk-load path for large indexing runs (ADR-014).
//!
//! # Node → column mapping
//!
//! Each [`NodeLabel`] has a distinct column layout (see [`node_table_columns`]).
//! The [`node_to_row`] helper extracts values from a [`Node`], pulling
//! table-specific fields (e.g. `hash`, `content`, `parameterCount`) from the
//! node's `properties` JSON when they don't have a dedicated struct field.

use std::io::Write;
use std::path::Path;

use csv::Writer;

use super::connection::StorageConnection;
use super::error::{Result, StorageError};
use super::schema::{escape_identifier, node_table_columns, relation_table_columns};
use crate::model::{Edge, Node, NodeLabel};

/// CSV batch loader for node and edge tables (ADR-014).
///
/// Stateless — the struct exists primarily for API symmetry with
/// [`crate::storage::Repository`] and to group the CSV-related functions.
#[derive(Debug, Clone, Default)]
pub struct CsvLoader;

impl CsvLoader {
    /// Creates a new [`CsvLoader`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Convenience wrapper around [`write_nodes_csv`] that writes the CSV to
    /// `path` on disk.
    pub fn write_nodes_file(&self, nodes: &[Node], label: NodeLabel, path: &Path) -> Result<()> {
        let csv = write_nodes_csv(nodes, label);
        std::fs::write(path, csv)?;
        Ok(())
    }

    /// Convenience wrapper around [`write_edges_csv`] that writes the CSV to
    /// `path` on disk.
    pub fn write_edges_file(&self, edges: &[Edge], path: &Path) -> Result<()> {
        let csv = write_edges_csv(edges);
        std::fs::write(path, csv)?;
        Ok(())
    }
}

/// Generates a CSV string for a node table (header + one row per node).
///
/// The header row contains the column names from [`node_table_columns`] for the
/// given `label`. Each subsequent row contains the field values extracted by
/// [`node_to_row`]. The output is RFC 4180-compliant: fields containing
/// commas, quotes, or newlines are properly escaped.
#[must_use]
pub fn write_nodes_csv(nodes: &[Node], label: NodeLabel) -> String {
    let columns = node_table_columns(label);
    let mut writer = Writer::from_writer(Vec::new());
    // Header
    writer.write_record(columns).expect("csv header write");
    // Rows
    for node in nodes {
        let row = node_to_row(node, label);
        writer.write_record(&row).expect("csv row write");
    }
    let bytes = writer.into_inner().expect("csv flush");
    String::from_utf8(bytes).expect("csv utf8")
}

/// Generates a CSV string for the `CodeRelation` table (header + one row per
/// edge).
///
/// Deduplicates edges by their primary-key id ([`edge_id`]) to prevent
/// primary-key conflicts during `COPY FROM`. Duplicate edges arise when
/// distinct call sites resolve to the same `{source, target, type, line}`
/// tuple (e.g. chained `.project(project)` calls in different files sharing
/// the same start line). When duplicates are skipped, a warning is printed
/// to stderr reporting the count (BR-INDEX-005, fail-loud principle).
#[must_use]
pub fn write_edges_csv(edges: &[Edge]) -> String {
    let columns = relation_table_columns();
    let mut writer = Writer::from_writer(Vec::new());
    writer.write_record(columns).expect("csv header write");
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut skipped = 0usize;
    for edge in edges {
        let id = edge_id(edge);
        if !seen.insert(id) {
            skipped += 1;
            continue;
        }
        let row = edge_to_row(edge);
        writer.write_record(&row).expect("csv row write");
    }
    if skipped > 0 {
        eprintln!(
            "warning: skipped {skipped} duplicate edge(s) during CSV generation (edges: {}, unique: {})",
            edges.len(),
            edges.len() - skipped
        );
    }
    let bytes = writer.into_inner().expect("csv flush");
    String::from_utf8(bytes).expect("csv utf8")
}

/// Loads a CSV file into a table via LadybugDB's `COPY FROM` command.
///
/// The CSV file must have a header row whose column names match the target
/// table's columns. The `table` name is escaped via [`escape_identifier`] to
/// handle reserved keywords like `Macro`.
///
/// `PARALLEL=FALSE` is specified because LadybugDB's parallel CSV reader does
/// not support quoted fields containing embedded newlines (e.g. multi-line
/// function signatures produced by the Fortran extractor). The serial reader
/// handles RFC 4180-compliant quoted fields correctly.
///
/// `HEADER` is specified so LadybugDB skips the CSV header row. Without it,
/// the header row (e.g. `id,project,name,...`) is inserted as a data row,
/// producing phantom nodes whose fields are the column names (DQ-005).
pub fn load_from_csv(conn: &StorageConnection, table: &str, csv_path: &Path) -> Result<()> {
    let path_str = csv_path
        .to_str()
        .ok_or_else(|| StorageError::InvalidData(format!("non-utf8 csv path: {csv_path:?}")))?
        .replace('\\', "/");
    // Escape single quotes for safe interpolation into a Cypher string literal.
    // Paths are internally generated (not user-controlled), but this guards
    // against pathological paths containing quotes.
    let escaped_path = path_str.replace('\'', "''");
    let escaped_table = escape_identifier(table);
    let cypher = format!(
        "COPY {escaped_table} FROM '{escaped_path}' (HEADER, PARALLEL=FALSE);"
    );
    conn.execute(&cypher)?;
    Ok(())
}

/// Extracts a string property from a node's `properties` JSON, returning an
/// empty string if absent.
fn prop_str(node: &Node, key: &str) -> String {
    node.properties
        .get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default()
}

/// Extracts an integer property from a node's `properties` JSON, returning an
/// empty string if absent (LadybugDB COPY treats empty as NULL for INT types
/// when the column is nullable; for non-null INT columns the caller should
/// ensure a value is present).
fn prop_int(node: &Node, key: &str) -> String {
    node.properties
        .get(key)
        .and_then(|v| v.as_i64())
        .map(|i| i.to_string())
        .unwrap_or_default()
}

/// Converts an `Option<u32>` to a string (empty if `None`).
fn opt_line(n: Option<u32>) -> String {
    n.map(|v| v.to_string()).unwrap_or_default()
}

/// Converts an `Option<String>` to a string (empty if `None`).
fn opt_str(s: &Option<String>) -> String {
    s.clone().unwrap_or_default()
}

/// Converts an `Option<Language>` to its string representation.
fn opt_lang(lang: &Option<crate::model::Language>) -> String {
    lang.map(|l| l.to_string()).unwrap_or_default()
}

/// Builds the column-value vector for a node, in the order specified by
/// [`node_table_columns`].
#[must_use]
pub fn node_to_row(node: &Node, label: NodeLabel) -> Vec<String> {
    match label {
        NodeLabel::Project => vec![
            node.id.clone(),
            node.name.clone(),
            prop_str(node, "rootPath"),
            opt_lang(&node.language),
            prop_int(node, "fileCount"),
            prop_int(node, "indexedAt"),
        ],
        NodeLabel::Folder => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            opt_str(&node.file_path),
        ],
        NodeLabel::File => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            opt_str(&node.file_path),
            opt_lang(&node.language),
            prop_str(node, "hash"),
            prop_int(node, "lineCount"),
        ],
        NodeLabel::Module | NodeLabel::Namespace => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Class | NodeLabel::Struct | NodeLabel::Enum | NodeLabel::Trait | NodeLabel::Interface => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            node.is_exported.to_string(),
            opt_str(&node.docstring),
            prop_str(node, "content"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Impl => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            prop_str(node, "implType"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Function => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            opt_str(&node.signature),
            opt_str(&node.return_type),
            node.is_exported.to_string(),
            opt_str(&node.docstring),
            prop_str(node, "content"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Method => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            opt_str(&node.signature),
            opt_str(&node.return_type),
            node.is_exported.to_string(),
            opt_str(&node.docstring),
            prop_str(node, "content"),
            prop_int(node, "parameterCount"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Variable => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            node.is_global.to_string(),
            opt_str(&node.return_type),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::GlobalVar => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_str(&node.return_type),
            node.is_exported.to_string(),
        ],
        NodeLabel::Parameter => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            prop_str(node, "paramType"),
            prop_int(node, "paramIndex"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Const => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            prop_str(node, "constType"),
            prop_str(node, "constValue"),
            node.is_exported.to_string(),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Static => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_str(&node.return_type),
            node.is_exported.to_string(),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Macro => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            opt_str(&node.signature),
            prop_str(node, "content"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::TypeAlias => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            prop_str(node, "aliasType"),
            node.is_exported.to_string(),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Typedef => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            prop_str(node, "typedefType"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Constructor | NodeLabel::Handler | NodeLabel::Middleware | NodeLabel::Test => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            opt_str(&node.signature),
            opt_str(&node.return_type),
            node.is_exported.to_string(),
            opt_str(&node.docstring),
            prop_str(node, "content"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Record | NodeLabel::Delegate | NodeLabel::Union | NodeLabel::Service => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            node.is_exported.to_string(),
            opt_str(&node.docstring),
            prop_str(node, "content"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Property | NodeLabel::Field => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            opt_str(&node.return_type),
            node.is_exported.to_string(),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Annotation | NodeLabel::Variant | NodeLabel::Event | NodeLabel::Section => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            opt_str(&node.docstring),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Template => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            prop_str(node, "templateParams"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Endpoint | NodeLabel::Route => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            opt_line(node.start_line),
            opt_line(node.end_line),
            prop_str(node, "httpMethod"),
            prop_str(node, "path"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Process | NodeLabel::Community => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.docstring),
        ],
        NodeLabel::Database => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            prop_str(node, "dbType"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Config => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            prop_str(node, "configType"),
            opt_str(&node.parent_qn),
        ],
        NodeLabel::Tool => vec![
            node.id.clone(),
            node.project.clone(),
            node.name.clone(),
            node.qualified_name.clone(),
            opt_str(&node.file_path),
            prop_str(node, "toolType"),
            opt_str(&node.parent_qn),
        ],
        // Embedding rows use the vector-store schema (id, nodeId, project,
        // chunkIndex, startLine, endLine, embedding, contentHash) per DDD §5.9.
        // In practice Embedding rows are inserted via the embed module's
        // dedicated path, not CSV batch load; this arm exists for exhaustiveness.
        NodeLabel::Embedding => vec![
            node.id.clone(),
            prop_str(node, "nodeId"),
            node.project.clone(),
            prop_int(node, "chunkIndex"),
            opt_line(node.start_line),
            opt_line(node.end_line),
            prop_str(node, "embedding"),
            prop_str(node, "contentHash"),
        ],
    }
}

/// Generates the primary-key id for an edge.
///
/// The id is `{source}_{target}_{type}_{start_line}`, with `start_line`
/// defaulting to 0 when `None`. Callers that batch-insert edges must dedup
/// by this id to avoid primary-key conflicts (ADR-014).
#[must_use]
pub fn edge_id(edge: &Edge) -> String {
    format!(
        "{}_{}_{}_{}",
        edge.source,
        edge.target,
        edge.edge_type.as_db_type(),
        edge.start_line.unwrap_or(0)
    )
}

/// Builds the column-value vector for an edge, in the order specified by
/// [`relation_table_columns`].
#[must_use]
pub fn edge_to_row(edge: &Edge) -> Vec<String> {
    let id = edge_id(edge);
    vec![
        id,
        edge.source.clone(),
        edge.target.clone(),
        edge.edge_type.as_db_type().to_string(),
        format!("{:.6}", edge.confidence),
        edge.confidence_tier.as_db_type().to_string(),
        opt_str(&edge.reason),
        opt_line(edge.start_line),
        edge.project.clone(),
    ]
}

/// Writes a CSV string to a temporary file and returns the path.
///
/// Used by tests and by the repository when bulk-loading.
pub(crate) fn write_csv_temp(content: &str, file_name: &str) -> Result<std::path::PathBuf> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(file_name);
    let mut file = std::fs::File::create(&path)?;
    file.write_all(content.as_bytes())?;
    // Leak the tempdir so the file survives for the caller; the OS reclaims it
    // on process exit. This matches the lifetime model used in tests.
    std::mem::forget(dir);
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeType, Language};

    fn sample_function_node() -> Node {
        Node::builder(NodeLabel::Function, "main", "proj.src.main")
            .id("func_001")
            .project("demo")
            .file_path("/src/main.rs")
            .start_line(10)
            .end_line(20)
            .language(Language::Rust)
            .signature("fn main() -> i32")
            .return_type("i32")
            .docstring("Entry point")
            .is_exported(true)
            .parent_qn("proj.src")
            .properties(serde_json::json!({"content": "fn main() { }"}))
            .build()
    }

    #[test]
    fn write_nodes_csv_has_header_row() {
        let csv = write_nodes_csv(&[], NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "id,project,name,qualifiedName,filePath,startLine,endLine,signature,returnType,isExported,docstring,content,parentQn"
        );
    }

    #[test]
    fn write_nodes_csv_has_one_row_per_node() {
        let nodes = vec![sample_function_node()];
        let csv = write_nodes_csv(&nodes, NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2, "expected header + 1 row");
    }

    #[test]
    fn write_nodes_csv_contains_correct_values() {
        let nodes = vec![sample_function_node()];
        let csv = write_nodes_csv(&nodes, NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        let row = lines[1];
        assert!(row.contains("func_001"));
        assert!(row.contains("demo"));
        assert!(row.contains("main"));
        assert!(row.contains("proj.src.main"));
        assert!(row.contains("/src/main.rs"));
        assert!(row.contains("fn main() -> i32"));
        assert!(row.contains("i32"));
        assert!(row.contains("Entry point"));
        assert!(row.contains("fn main() { }"));
    }

    #[test]
    fn write_nodes_csv_escapes_commas_in_fields() {
        let node = Node::builder(NodeLabel::Function, "foo,bar", "qn")
            .id("id1")
            .project("p")
            .docstring("has, comma")
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        // The field with a comma should be quoted
        assert!(lines[1].contains("\"foo,bar\""));
        assert!(lines[1].contains("\"has, comma\""));
    }

    #[test]
    fn write_nodes_csv_escapes_quotes_in_fields() {
        let node = Node::builder(NodeLabel::Function, "foo\"bar", "qn")
            .id("id1")
            .project("p")
            .docstring("say \"hi\"")
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        // Quotes inside fields are doubled per RFC 4180
        assert!(lines[1].contains("\"foo\"\"bar\""));
        assert!(lines[1].contains("\"say \"\"hi\"\"\""));
    }

    #[test]
    fn write_nodes_csv_escapes_newlines_in_fields() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .id("id1")
            .project("p")
            .docstring("line1\nline2")
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Function);
        // The newline is inside a quoted field, so the CSV has 3 lines total
        // (header + 2 lines for the quoted field with embedded newline).
        assert!(csv.contains("\"line1\nline2\""));
    }

    #[test]
    fn write_nodes_csv_for_project_label() {
        let node = Node::builder(NodeLabel::Project, "demo", "demo")
            .id("proj_001")
            .properties(serde_json::json!({
                "rootPath": "/repo/demo",
                "fileCount": 42,
                "indexedAt": 1700000000
            }))
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Project);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "id,name,rootPath,language,fileCount,indexedAt");
        assert!(lines[1].contains("proj_001"));
        assert!(lines[1].contains("demo"));
        assert!(lines[1].contains("/repo/demo"));
        assert!(lines[1].contains("42"));
        assert!(lines[1].contains("1700000000"));
    }

    #[test]
    fn write_nodes_csv_for_file_label() {
        let node = Node::builder(NodeLabel::File, "main.rs", "demo.src.main")
            .id("file_001")
            .project("demo")
            .file_path("/src/main.rs")
            .language(Language::Rust)
            .properties(serde_json::json!({"hash": "abc123", "lineCount": 100}))
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::File);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[1].contains("abc123"));
        assert!(lines[1].contains("100"));
        assert!(lines[1].contains("rust"));
    }

    #[test]
    fn write_nodes_csv_empty_input_returns_header_only() {
        let csv = write_nodes_csv(&[], NodeLabel::Class);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("id,"));
    }

    #[test]
    fn write_edges_csv_has_header_row() {
        let csv = write_edges_csv(&[]);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "id,source,target,type,confidence,confidenceTier,reason,startLine,project"
        );
    }

    #[test]
    fn write_edges_csv_contains_correct_values() {
        let edge = Edge::builder("func_a", "func_b", EdgeType::Calls, "demo")
            .confidence(0.95)
            .reason("direct call")
            .start_line(15)
            .build();
        let csv = write_edges_csv(&[edge]);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("func_a"));
        assert!(lines[1].contains("func_b"));
        assert!(lines[1].contains("CALLS"));
        assert!(lines[1].contains("0.950000"));
        assert!(lines[1].contains("direct call"));
        assert!(lines[1].contains("15"));
        assert!(lines[1].contains("demo"));
    }

    #[test]
    fn write_edges_csv_escapes_special_chars_in_reason() {
        let edge = Edge::builder("s", "t", EdgeType::Calls, "p")
            .reason("arg, index=0\nnext line")
            .build();
        let csv = write_edges_csv(&[edge]);
        assert!(csv.contains("\"arg, index=0\nnext line\""));
    }

    #[test]
    fn node_to_row_function_has_thirteen_columns() {
        let node = sample_function_node();
        let row = node_to_row(&node, NodeLabel::Function);
        assert_eq!(row.len(), 13);
        assert_eq!(row[0], "func_001");
        assert_eq!(row[1], "demo");
        assert_eq!(row[2], "main");
    }

    #[test]
    fn node_to_row_project_has_six_columns() {
        let node = Node::builder(NodeLabel::Project, "p", "p")
            .id("p1")
            .properties(serde_json::json!({"rootPath": "/", "fileCount": 1, "indexedAt": 2}))
            .build();
        let row = node_to_row(&node, NodeLabel::Project);
        assert_eq!(row.len(), 6);
        assert_eq!(row[2], "/");
        assert_eq!(row[4], "1");
        assert_eq!(row[5], "2");
    }

    #[test]
    fn node_to_row_method_has_fourteen_columns() {
        let node = Node::builder(NodeLabel::Method, "m", "qn")
            .id("m1")
            .project("p")
            .properties(serde_json::json!({"content": "x", "parameterCount": 3}))
            .build();
        let row = node_to_row(&node, NodeLabel::Method);
        assert_eq!(row.len(), 14);
        assert_eq!(row[12], "3");
    }

    #[test]
    fn edge_to_row_has_nine_columns() {
        let edge = Edge::new("s", "t", EdgeType::Calls, "p");
        let row = edge_to_row(&edge);
        assert_eq!(row.len(), 9);
        assert_eq!(row[1], "s");
        assert_eq!(row[2], "t");
        assert_eq!(row[3], "CALLS");
        assert_eq!(row[5], "GLOBAL");
    }

    #[test]
    fn load_from_csv_loads_nodes_into_table() {
        let dir = tempfile::tempdir().unwrap();
        let conn = StorageConnection::open(dir.path().join("testdb")).unwrap();
        std::mem::forget(dir);
        conn.init_schema().unwrap();

        let node = sample_function_node();
        let csv = write_nodes_csv(&[node], NodeLabel::Function);
        let csv_path = write_csv_temp(&csv, "functions.csv").unwrap();
        load_from_csv(&conn, "Function", &csv_path).expect("COPY failed");

        let rows = conn
            .query("MATCH (f:Function) RETURN f.name AS name;")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("main"));
    }

    #[test]
    fn load_from_csv_loads_edges_into_table() {
        let dir = tempfile::tempdir().unwrap();
        let conn = StorageConnection::open(dir.path().join("testdb")).unwrap();
        std::mem::forget(dir);
        conn.init_schema().unwrap();

        let edge = Edge::builder("s", "t", EdgeType::Calls, "demo")
            .confidence(0.9)
            .start_line(5)
            .build();
        let csv = write_edges_csv(&[edge]);
        let csv_path = write_csv_temp(&csv, "edges.csv").unwrap();
        load_from_csv(&conn, "CodeRelation", &csv_path).expect("COPY failed");

        let rows = conn
            .query("MATCH (r:CodeRelation) RETURN r.type AS type;")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("CALLS"));
    }

    #[test]
    fn load_from_csv_handles_macro_table() {
        let dir = tempfile::tempdir().unwrap();
        let conn = StorageConnection::open(dir.path().join("testdb")).unwrap();
        std::mem::forget(dir);
        conn.init_schema().unwrap();

        let node = Node::builder(NodeLabel::Macro, "MY_MACRO", "demo.MY_MACRO")
            .id("macro_1")
            .project("demo")
            .start_line(1)
            .end_line(3)
            .signature("#define MY_MACRO(x) x+1")
            .properties(serde_json::json!({"content": "#define MY_MACRO(x) x+1"}))
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Macro);
        let csv_path = write_csv_temp(&csv, "macros.csv").unwrap();
        load_from_csv(&conn, "Macro", &csv_path).expect("COPY Macro failed");

        let rows = conn
            .query("MATCH (m:`Macro`) RETURN m.name AS name;")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("MY_MACRO"));
    }

    #[test]
    fn csv_loader_new_returns_default() {
        let loader = CsvLoader::new();
        let _ = format!("{loader:?}");
    }

    #[test]
    fn edge_id_generates_correct_format() {
        let edge = Edge::builder("src", "tgt", EdgeType::Calls, "p")
            .start_line(42)
            .build();
        assert_eq!(edge_id(&edge), "src_tgt_CALLS_42");
    }

    #[test]
    fn edge_id_defaults_line_to_zero_when_none() {
        let edge = Edge::new("s", "t", EdgeType::Reads, "p");
        assert_eq!(edge_id(&edge), "s_t_READS_0");
    }

    #[test]
    fn write_edges_csv_deduplicates_by_edge_id() {
        // Two edges with the same {source, target, type, start_line} produce
        // the same primary-key id. Only the first should appear in the CSV.
        let edge1 = Edge::builder("a", "b", EdgeType::Calls, "p")
            .confidence(0.9)
            .start_line(10)
            .build();
        let edge2 = Edge::builder("a", "b", EdgeType::Calls, "p")
            .confidence(0.5) // different confidence, same id
            .start_line(10)
            .build();
        let edge3 = Edge::builder("a", "b", EdgeType::Calls, "p")
            .start_line(20) // different line -> different id
            .build();
        let csv = write_edges_csv(&[edge1, edge2, edge3]);
        let lines: Vec<&str> = csv.lines().collect();
        // 1 header + 2 unique data rows (edge1 and edge3; edge2 is dup of edge1)
        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains("0.900000"));
        assert!(lines[2].contains("20"));
    }

    #[test]
    fn write_edges_csv_preserves_all_unique_edges() {
        let edges = vec![
            Edge::new("a", "b", EdgeType::Calls, "p"),
            Edge::new("b", "c", EdgeType::Calls, "p"),
            Edge::new("c", "d", EdgeType::Reads, "p"),
        ];
        let csv = write_edges_csv(&edges);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 4); // header + 3 edges
    }

    #[test]
    fn node_to_row_folder_has_four_columns() {
        let node = Node::builder(NodeLabel::Folder, "src", "proj.src")
            .id("folder_1")
            .project("demo")
            .file_path("/src")
            .build();
        let row = node_to_row(&node, NodeLabel::Folder);
        assert_eq!(row.len(), 4);
        assert_eq!(row[0], "folder_1");
        assert_eq!(row[1], "demo");
        assert_eq!(row[2], "src");
        assert_eq!(row[3], "/src");
    }

    #[test]
    fn node_to_row_module_has_six_columns() {
        let node = Node::builder(NodeLabel::Module, "mymod", "proj.mymod")
            .id("mod_1")
            .project("demo")
            .file_path("/src/mod.rs")
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Module);
        assert_eq!(row.len(), 6);
        assert_eq!(row[3], "proj.mymod");
        assert_eq!(row[5], "proj");
    }

    #[test]
    fn node_to_row_namespace_has_six_columns() {
        let node = Node::builder(NodeLabel::Namespace, "ns", "proj.ns")
            .id("ns_1")
            .project("demo")
            .build();
        let row = node_to_row(&node, NodeLabel::Namespace);
        assert_eq!(row.len(), 6);
    }

    #[test]
    fn node_to_row_class_has_twelve_columns() {
        let node = Node::builder(NodeLabel::Class, "MyClass", "proj.MyClass")
            .id("cls_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(10)
            .is_exported(true)
            .docstring("A class")
            .properties(serde_json::json!({"content": "class body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Class);
        assert_eq!(row.len(), 11);
        assert_eq!(row[7], "true");
        assert_eq!(row[8], "A class");
        assert_eq!(row[9], "class body");
    }

    #[test]
    fn node_to_row_struct_has_twelve_columns() {
        let node = Node::builder(NodeLabel::Struct, "Point", "proj.Point")
            .id("struct_1")
            .project("demo")
            .start_line(1)
            .end_line(5)
            .build();
        let row = node_to_row(&node, NodeLabel::Struct);
        assert_eq!(row.len(), 11);
    }

    #[test]
    fn node_to_row_enum_has_twelve_columns() {
        let node = Node::builder(NodeLabel::Enum, "Color", "proj.Color")
            .id("enum_1")
            .project("demo")
            .build();
        let row = node_to_row(&node, NodeLabel::Enum);
        assert_eq!(row.len(), 11);
    }

    #[test]
    fn node_to_row_trait_has_twelve_columns() {
        let node = Node::builder(NodeLabel::Trait, "Drawable", "proj.Drawable")
            .id("trait_1")
            .project("demo")
            .build();
        let row = node_to_row(&node, NodeLabel::Trait);
        assert_eq!(row.len(), 11);
    }

    #[test]
    fn node_to_row_impl_has_nine_columns() {
        let node = Node::builder(NodeLabel::Impl, "impl Point", "proj.Point.impl")
            .id("impl_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(10)
            .properties(serde_json::json!({"implType": "inherent"}))
            .parent_qn("proj.Point")
            .build();
        let row = node_to_row(&node, NodeLabel::Impl);
        assert_eq!(row.len(), 9);
        assert_eq!(row[7], "inherent");
        assert_eq!(row[8], "proj.Point");
    }

    #[test]
    fn node_to_row_variable_has_nine_columns() {
        let node = Node::builder(NodeLabel::Variable, "x", "proj.x")
            .id("var_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(5)
            .is_global(false)
            .return_type("i32")
            .parent_qn("proj.main")
            .build();
        let row = node_to_row(&node, NodeLabel::Variable);
        assert_eq!(row.len(), 9);
        assert_eq!(row[6], "false");
        assert_eq!(row[7], "i32");
        assert_eq!(row[8], "proj.main");
    }

    #[test]
    fn node_to_row_globalvar_has_eight_columns() {
        let node = Node::builder(NodeLabel::GlobalVar, "PI", "proj.PI")
            .id("gvar_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .return_type("f64")
            .is_exported(true)
            .build();
        let row = node_to_row(&node, NodeLabel::GlobalVar);
        assert_eq!(row.len(), 8);
        assert_eq!(row[6], "f64");
        assert_eq!(row[7], "true");
    }

    #[test]
    fn node_to_row_parameter_has_nine_columns() {
        let node = Node::builder(NodeLabel::Parameter, "param0", "proj.foo.param0")
            .id("param_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(3)
            .properties(serde_json::json!({"paramType": "i32", "paramIndex": 0}))
            .parent_qn("proj.foo")
            .build();
        let row = node_to_row(&node, NodeLabel::Parameter);
        assert_eq!(row.len(), 9);
        assert_eq!(row[6], "i32");
        assert_eq!(row[7], "0");
    }

    #[test]
    fn node_to_row_const_has_nine_columns() {
        let node = Node::builder(NodeLabel::Const, "MAX", "proj.MAX")
            .id("const_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .properties(serde_json::json!({"constType": "i32", "constValue": "42"}))
            .is_exported(true)
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Const);
        assert_eq!(row.len(), 10);
        assert_eq!(row[6], "i32");
        assert_eq!(row[7], "42");
        assert_eq!(row[8], "true");
    }

    #[test]
    fn node_to_row_static_has_eight_columns() {
        let node = Node::builder(NodeLabel::Static, "COUNTER", "proj.COUNTER")
            .id("static_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .return_type("u32")
            .is_exported(false)
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Static);
        assert_eq!(row.len(), 9);
        assert_eq!(row[6], "u32");
        assert_eq!(row[7], "false");
    }

    #[test]
    fn node_to_row_typealias_has_eight_columns() {
        let node = Node::builder(NodeLabel::TypeAlias, "Id", "proj.Id")
            .id("alias_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .properties(serde_json::json!({"aliasType": "u32"}))
            .is_exported(true)
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::TypeAlias);
        assert_eq!(row.len(), 9);
        assert_eq!(row[6], "u32");
        assert_eq!(row[7], "true");
    }

    #[test]
    fn node_to_row_typedef_has_seven_columns() {
        let node = Node::builder(NodeLabel::Typedef, "MyType", "proj.MyType")
            .id("typedef_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .properties(serde_json::json!({"typedefType": "int"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Typedef);
        assert_eq!(row.len(), 8);
        assert_eq!(row[6], "int");
    }

    #[test]
    fn write_nodes_file_writes_csv_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodes.csv");
        let node = sample_function_node();
        CsvLoader::new()
            .write_nodes_file(&[node], NodeLabel::Function, &path)
            .expect("write_nodes_file");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("func_001"));
        assert!(content.contains("main"));
    }

    #[test]
    fn write_edges_file_writes_csv_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edges.csv");
        let edge = Edge::builder("s", "t", EdgeType::Calls, "demo")
            .start_line(1)
            .build();
        CsvLoader::new()
            .write_edges_file(&[edge], &path)
            .expect("write_edges_file");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("CALLS"));
        assert!(content.contains("demo"));
    }
}
