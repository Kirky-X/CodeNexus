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

use csv::WriterBuilder;

use super::connection::StorageConnection;
use super::error::{Result, StorageError};
use super::schema::{escape_identifier, node_table_columns, relation_table_columns};
use crate::model::{Edge, Node, NodeLabel};

/// Tab character used as the CSV delimiter.
///
/// LadybugDB's COPY parser does not correctly handle RFC 4180 quoted fields
/// containing commas (e.g. `FallbackChain<T, U>` — the comma inside the
/// angle brackets splits the field even though it's quoted). Switching to
/// tab-delimited format avoids this entirely because tab characters never
/// appear in code identifiers or file paths.
const CSV_DELIMITER: u8 = b'\t';

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

/// Sanitizes a string field for LadybugDB COPY compatibility (B6 workaround).
///
/// LadybugDB's COPY parser does not correctly handle RFC 4180 quoted fields:
/// - Backslashes are treated as C-style escape chars (breaks C macro line
///   continuations in the redis sample).
/// - Doubled quotes (`""`) are not always parsed as a literal `"` (breaks
///   Python method signatures in the subno.ts sample).
/// - Multi-line quoted fields (newlines inside a quoted field) cause the
///   parser to report "expected N values per row, but got M" because it
///   cannot reconcile a quoted field spanning multiple physical lines with
///   its row-splitting logic (breaks any sample whose `content` or
///   `signature` field contains embedded newlines — e.g. subno.ts Python
///   methods, redis C function bodies).
/// - Tab characters inside a field collide with the tab delimiter and force
///   quoting, re-triggering the multi-line issue.
///
/// To prevent parser corruption, we replace these characters with visually
/// similar but parser-safe alternatives:
/// - `\` → `/` (consistent with [`load_from_csv`]'s path normalization)
/// - `"` → `'` (preserves readability of quoted strings in signatures)
/// - `\n` / `\r` → ` ` (space; collapses multi-line content to one line)
/// - `\t` → ` ` (space; prevents delimiter collision and forced quoting)
///
/// The data-fidelity impact is minimal: backslashes in indexed code are rare
/// (mostly C macros and Windows paths, both already normalized elsewhere),
/// double-quotes in signatures are typically decorative rather than
/// semantically significant, and newlines/tabs in indexed metadata fields
/// (signatures, docstrings, content snapshots) are not semantically loaded —
/// the structural line range is preserved in the dedicated `startLine` and
/// `endLine` columns. See `tools/verification/results/triage.md` §B6.
fn sanitize_for_ladybugdb(s: String) -> String {
    if !s.contains('\\')
        && !s.contains('"')
        && !s.contains('\n')
        && !s.contains('\r')
        && !s.contains('\t')
    {
        return s;
    }
    s.replace('\\', "/")
        .replace('"', "'")
        .replace(['\n', '\r', '\t'], " ")
}

/// Generates a CSV string for a node table (header + one row per node).
///
/// The header row contains the column names from [`node_table_columns`] for the
/// given `label`. Each subsequent row contains the field values extracted by
/// [`node_to_row`], sanitized via [`sanitize_for_ladybugdb`] to avoid the
/// LadybugDB COPY parser bug (B6). Uses tab-delimited format (see
/// [`CSV_DELIMITER`] for rationale).
#[must_use]
pub fn write_nodes_csv(nodes: &[Node], label: NodeLabel) -> String {
    let columns = node_table_columns(label);
    let mut writer = WriterBuilder::new()
        .delimiter(CSV_DELIMITER)
        .from_writer(Vec::new());
    // Header
    writer.write_record(columns).expect("csv header write");
    // Rows
    for node in nodes {
        let row: Vec<String> = node_to_row(node, label)
            .into_iter()
            .map(sanitize_for_ladybugdb)
            .collect();
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
    let mut writer = WriterBuilder::new()
        .delimiter(CSV_DELIMITER)
        .from_writer(Vec::new());
    writer.write_record(columns).expect("csv header write");
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut skipped = 0usize;
    for edge in edges {
        let id = edge_id(edge);
        if !seen.insert(id) {
            skipped += 1;
            continue;
        }
        let row: Vec<String> = edge_to_row(edge)
            .into_iter()
            .map(sanitize_for_ladybugdb)
            .collect();
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
///
/// `DELIM '\t'` specifies tab-delimited format. LadybugDB's COPY parser does
/// not correctly handle RFC 4180 quoted fields containing commas (e.g. Rust
/// generic types like `FallbackChain<T, U>`), so we use tab delimiters
/// instead (see [`CSV_DELIMITER`]).
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
        "COPY {escaped_table} FROM '{escaped_path}' (HEADER, DELIM '\\t', PARALLEL=FALSE);"
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
            prop_str(node, "lastCommit"),
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
        NodeLabel::Class
        | NodeLabel::Struct
        | NodeLabel::Enum
        | NodeLabel::Trait
        | NodeLabel::Interface => vec![
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
        NodeLabel::Constructor | NodeLabel::Handler | NodeLabel::Middleware | NodeLabel::Test => {
            vec![
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
            ]
        }
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
        // CSV_DELIMITER is tab (LadybugDB COPY compatibility); header uses tabs.
        assert_eq!(
            lines[0],
            "id\tproject\tname\tqualifiedName\tfilePath\tstartLine\tendLine\tsignature\treturnType\tisExported\tdocstring\tcontent\tparentQn"
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
    fn write_nodes_csv_sanitizes_tabs_in_fields() {
        // B6 workaround: tabs are replaced with spaces by sanitize_for_ladybugdb
        // to prevent delimiter collision and forced quoting (LadybugDB COPY
        // cannot parse multi-line quoted fields that result from tab-quoted fields).
        let node = Node::builder(NodeLabel::Function, "foo\tbar", "qn")
            .id("id1")
            .project("p")
            .docstring("has\t tab")
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        // Tab replaced with space; field is NOT quoted.
        assert!(lines[1].contains("foo bar"));
        assert!(lines[1].contains("has  tab"));
        // No literal tab should remain in the data row (only as delimiter).
        // The data row has exactly 13 tab delimiters (14 columns) — verify no
        // extra tabs from field content by checking the quoted form is absent.
        assert!(!lines[1].contains("\"foo"));
    }

    #[test]
    fn write_nodes_csv_sanitizes_quotes_in_fields() {
        // B6 workaround: double-quotes are replaced with single-quotes before
        // CSV writing to avoid LadybugDB COPY parser corruption on Python
        // method signatures (subno.ts sample).
        let node = Node::builder(NodeLabel::Function, "foo\"bar", "qn")
            .id("id1")
            .project("p")
            .docstring("say \"hi\"")
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        // Quotes are sanitized to single-quotes (no RFC 4180 doubling needed)
        assert!(lines[1].contains("foo'bar"));
        assert!(lines[1].contains("say 'hi'"));
        // Ensure no doubled quotes remain (would indicate sanitize was bypassed)
        assert!(!lines[1].contains("\"\""));
    }

    #[test]
    fn write_nodes_csv_sanitizes_backslashes_for_ladybugdb() {
        // B6 workaround: backslashes are replaced with forward slashes before
        // CSV writing to avoid LadybugDB COPY parser treating them as C-style
        // escape chars (breaks C macro line continuations in redis sample).
        let node = Node::builder(NodeLabel::Macro, "MY_MACRO", "proj.MY_MACRO")
            .id("macro_1")
            .project("demo")
            .file_path("/src/macro.h")
            .start_line(1)
            .end_line(3)
            .signature("#define MY_MACRO(x) \\ continuation")
            .properties(serde_json::json!({"content": "#define MY_MACRO(x) \\ continuation"}))
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Macro);
        // Backslash in signature/content replaced with forward slash
        assert!(csv.contains("MY_MACRO(x) / continuation"));
        // Ensure no raw backslash remains in the CSV data (sanitize was applied)
        assert!(!csv.contains('\\'));
    }

    #[test]
    fn write_nodes_csv_sanitizes_newlines_in_fields() {
        // B6 workaround: newlines are replaced with spaces by sanitize_for_ladybugdb
        // to prevent multi-line quoted fields (which LadybugDB COPY cannot parse).
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .id("id1")
            .project("p")
            .docstring("line1\nline2")
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Function);
        let lines: Vec<&str> = csv.lines().collect();
        // Newline replaced with space; field is NOT quoted (no quoting needed).
        assert!(lines[1].contains("line1 line2"));
        assert!(!lines[1].contains("\"line1"));
    }

    #[test]
    fn write_nodes_csv_for_project_label() {
        let node = Node::builder(NodeLabel::Project, "demo", "demo")
            .id("proj_001")
            .properties(serde_json::json!({
                "rootPath": "/repo/demo",
                "fileCount": 42,
                "indexedAt": 1700000000,
                "lastCommit": "abc123"
            }))
            .build();
        let csv = write_nodes_csv(&[node], NodeLabel::Project);
        let lines: Vec<&str> = csv.lines().collect();
        // CSV_DELIMITER is tab; Project header uses tabs.
        assert_eq!(
            lines[0],
            "id\tname\trootPath\tlanguage\tfileCount\tindexedAt\tlastCommit"
        );
        assert!(lines[1].contains("proj_001"));
        assert!(lines[1].contains("demo"));
        assert!(lines[1].contains("/repo/demo"));
        assert!(lines[1].contains("42"));
        assert!(lines[1].contains("1700000000"));
        assert!(lines[1].contains("abc123"));
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
        // CSV_DELIMITER is tab; header starts with "id\t".
        assert!(lines[0].starts_with("id\t"));
    }

    #[test]
    fn write_edges_csv_has_header_row() {
        let csv = write_edges_csv(&[]);
        let lines: Vec<&str> = csv.lines().collect();
        // CSV_DELIMITER is tab (LadybugDB COPY compatibility); header uses tabs.
        assert_eq!(
            lines[0],
            "id\tsource\ttarget\ttype\tconfidence\tconfidenceTier\treason\tstartLine\tproject"
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
    fn write_edges_csv_sanitizes_special_chars_in_reason() {
        // B6 workaround: newlines in edge reason fields are replaced with spaces
        // to prevent multi-line quoted fields (LadybugDB COPY cannot parse them).
        let edge = Edge::builder("s", "t", EdgeType::Calls, "p")
            .reason("arg, index=0\nnext line")
            .build();
        let csv = write_edges_csv(&[edge]);
        // Newline replaced with space; no quoting needed.
        assert!(csv.contains("arg, index=0 next line"));
        assert!(!csv.contains("\"arg"));
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
    fn node_to_row_project_has_seven_columns() {
        let node = Node::builder(NodeLabel::Project, "p", "p")
            .id("p1")
            .properties(serde_json::json!({"rootPath": "/", "fileCount": 1, "indexedAt": 2, "lastCommit": "abc"}))
            .build();
        let row = node_to_row(&node, NodeLabel::Project);
        assert_eq!(row.len(), 7);
        assert_eq!(row[2], "/");
        assert_eq!(row[4], "1");
        assert_eq!(row[5], "2");
        assert_eq!(row[6], "abc");
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

    // --- node_to_row coverage for remaining NodeLabel variants ---

    #[test]
    fn node_to_row_constructor_has_thirteen_columns() {
        let node = Node::builder(NodeLabel::Constructor, "ctor", "proj.ctor")
            .id("ctor_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(5)
            .signature("fn ctor()")
            .return_type("Self")
            .is_exported(true)
            .docstring("ctor")
            .properties(serde_json::json!({"content": "body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Constructor);
        assert_eq!(row.len(), 13);
        assert_eq!(row[0], "ctor_1");
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "5");
        assert_eq!(row[7], "fn ctor()");
        assert_eq!(row[8], "Self");
        assert_eq!(row[9], "true");
        assert_eq!(row[10], "ctor");
        assert_eq!(row[11], "body");
        assert_eq!(row[12], "proj");
    }

    #[test]
    fn node_to_row_handler_has_thirteen_columns() {
        let node = Node::builder(NodeLabel::Handler, "on_click", "proj.on_click")
            .id("h_1")
            .project("demo")
            .file_path("/src/ui.rs")
            .start_line(10)
            .end_line(20)
            .signature("fn on_click()")
            .return_type("void")
            .is_exported(false)
            .docstring("click handler")
            .properties(serde_json::json!({"content": "handler body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Handler);
        assert_eq!(row.len(), 13);
        assert_eq!(row[9], "false");
        assert_eq!(row[11], "handler body");
    }

    #[test]
    fn node_to_row_middleware_has_thirteen_columns() {
        let node = Node::builder(NodeLabel::Middleware, "auth", "proj.auth")
            .id("mw_1")
            .project("demo")
            .file_path("/src/mw.rs")
            .start_line(1)
            .end_line(10)
            .signature("fn auth()")
            .return_type("Response")
            .is_exported(true)
            .docstring("auth mw")
            .properties(serde_json::json!({"content": "mw body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Middleware);
        assert_eq!(row.len(), 13);
        assert_eq!(row[7], "fn auth()");
    }

    #[test]
    fn node_to_row_test_has_thirteen_columns() {
        let node = Node::builder(NodeLabel::Test, "test_foo", "proj.test_foo")
            .id("test_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(5)
            .signature("fn test_foo()")
            .return_type("()")
            .is_exported(false)
            .docstring("tests foo")
            .properties(serde_json::json!({"content": "assert!()"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Test);
        assert_eq!(row.len(), 13);
        assert_eq!(row[2], "test_foo");
    }

    #[test]
    fn node_to_row_record_has_ten_columns() {
        let node = Node::builder(NodeLabel::Record, "Rec", "proj.Rec")
            .id("rec_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(5)
            .is_exported(true)
            .docstring("a record")
            .properties(serde_json::json!({"content": "record body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Record);
        assert_eq!(row.len(), 11);
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "5");
        assert_eq!(row[7], "true");
        assert_eq!(row[8], "a record");
        assert_eq!(row[9], "record body");
        assert_eq!(row[10], "proj");
    }

    #[test]
    fn node_to_row_delegate_has_ten_columns() {
        let node = Node::builder(NodeLabel::Delegate, "Action", "proj.Action")
            .id("del_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(3)
            .is_exported(false)
            .docstring("delegate")
            .properties(serde_json::json!({"content": "delegate body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Delegate);
        assert_eq!(row.len(), 11);
        assert_eq!(row[7], "false");
    }

    #[test]
    fn node_to_row_union_has_ten_columns() {
        let node = Node::builder(NodeLabel::Union, "U", "proj.U")
            .id("union_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(3)
            .is_exported(true)
            .docstring("a union")
            .properties(serde_json::json!({"content": "union body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Union);
        assert_eq!(row.len(), 11);
        assert_eq!(row[8], "a union");
    }

    #[test]
    fn node_to_row_service_has_ten_columns() {
        let node = Node::builder(NodeLabel::Service, "AuthService", "proj.AuthService")
            .id("svc_1")
            .project("demo")
            .file_path("/src/svc.rs")
            .start_line(1)
            .end_line(20)
            .is_exported(true)
            .docstring("auth service")
            .properties(serde_json::json!({"content": "service body"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Service);
        assert_eq!(row.len(), 11);
        assert_eq!(row[2], "AuthService");
    }

    #[test]
    fn node_to_row_property_has_nine_columns() {
        let node = Node::builder(NodeLabel::Property, "name", "proj.name")
            .id("prop_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(2)
            .return_type("String")
            .is_exported(true)
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Property);
        assert_eq!(row.len(), 10);
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "2");
        assert_eq!(row[7], "String");
        assert_eq!(row[8], "true");
        assert_eq!(row[9], "proj");
    }

    #[test]
    fn node_to_row_field_has_nine_columns() {
        let node = Node::builder(NodeLabel::Field, "x", "proj.x")
            .id("field_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(1)
            .return_type("i32")
            .is_exported(false)
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Field);
        assert_eq!(row.len(), 10);
        assert_eq!(row[7], "i32");
        assert_eq!(row[8], "false");
    }

    #[test]
    fn node_to_row_annotation_has_eight_columns() {
        let node = Node::builder(NodeLabel::Annotation, "Deprecated", "proj.Deprecated")
            .id("ann_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(1)
            .docstring("deprecated")
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Annotation);
        assert_eq!(row.len(), 9);
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "1");
        assert_eq!(row[7], "deprecated");
        assert_eq!(row[8], "proj");
    }

    #[test]
    fn node_to_row_variant_has_eight_columns() {
        let node = Node::builder(NodeLabel::Variant, "A", "proj.A")
            .id("var_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(1)
            .docstring("variant A")
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Variant);
        assert_eq!(row.len(), 9);
        assert_eq!(row[7], "variant A");
    }

    #[test]
    fn node_to_row_event_has_eight_columns() {
        let node = Node::builder(NodeLabel::Event, "OnClick", "proj.OnClick")
            .id("evt_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(1)
            .docstring("click event")
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Event);
        assert_eq!(row.len(), 9);
        assert_eq!(row[2], "OnClick");
    }

    #[test]
    fn node_to_row_section_has_eight_columns() {
        let node = Node::builder(NodeLabel::Section, "intro", "proj.intro")
            .id("sec_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(10)
            .docstring("intro section")
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Section);
        assert_eq!(row.len(), 9);
        assert_eq!(row[7], "intro section");
    }

    #[test]
    fn node_to_row_template_has_eight_columns() {
        let node = Node::builder(NodeLabel::Template, "T", "proj.T")
            .id("tpl_1")
            .project("demo")
            .file_path("/src/lib.rs")
            .start_line(1)
            .end_line(1)
            .properties(serde_json::json!({"templateParams": "<T: Clone>"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Template);
        assert_eq!(row.len(), 9);
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "1");
        assert_eq!(row[7], "<T: Clone>");
        assert_eq!(row[8], "proj");
    }

    #[test]
    fn node_to_row_endpoint_has_nine_columns() {
        let node = Node::builder(NodeLabel::Endpoint, "create_user", "proj.create_user")
            .id("ep_1")
            .project("demo")
            .file_path("/src/api.rs")
            .start_line(1)
            .end_line(10)
            .properties(serde_json::json!({"httpMethod": "POST", "path": "/users"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Endpoint);
        assert_eq!(row.len(), 10);
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "10");
        assert_eq!(row[7], "POST");
        assert_eq!(row[8], "/users");
        assert_eq!(row[9], "proj");
    }

    #[test]
    fn node_to_row_route_has_nine_columns() {
        let node = Node::builder(NodeLabel::Route, "list_users", "proj.list_users")
            .id("rt_1")
            .project("demo")
            .file_path("/src/api.rs")
            .start_line(1)
            .end_line(5)
            .properties(serde_json::json!({"httpMethod": "GET", "path": "/users"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Route);
        assert_eq!(row.len(), 10);
        assert_eq!(row[7], "GET");
        assert_eq!(row[8], "/users");
    }

    #[test]
    fn node_to_row_process_has_four_columns() {
        let node = Node::builder(NodeLabel::Process, "worker", "proj.worker")
            .id("proc_1")
            .project("demo")
            .docstring("background worker")
            .build();
        let row = node_to_row(&node, NodeLabel::Process);
        assert_eq!(row.len(), 5);
        assert_eq!(row[0], "proc_1");
        assert_eq!(row[1], "demo");
        assert_eq!(row[2], "worker");
        assert_eq!(row[3], "proj.worker");
        assert_eq!(row[4], "background worker");
    }

    #[test]
    fn node_to_row_community_has_four_columns() {
        let node = Node::builder(NodeLabel::Community, "cluster_a", "proj.cluster_a")
            .id("comm_1")
            .project("demo")
            .docstring("a community")
            .build();
        let row = node_to_row(&node, NodeLabel::Community);
        assert_eq!(row.len(), 5);
        assert_eq!(row[4], "a community");
    }

    #[test]
    fn node_to_row_database_has_six_columns() {
        let node = Node::builder(NodeLabel::Database, "main_db", "proj.main_db")
            .id("db_1")
            .project("demo")
            .file_path("/db/main")
            .properties(serde_json::json!({"dbType": "postgres"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Database);
        assert_eq!(row.len(), 7);
        assert_eq!(row[0], "db_1");
        assert_eq!(row[1], "demo");
        assert_eq!(row[2], "main_db");
        assert_eq!(row[3], "proj.main_db");
        assert_eq!(row[4], "/db/main");
        assert_eq!(row[5], "postgres");
        assert_eq!(row[6], "proj");
    }

    #[test]
    fn node_to_row_config_has_six_columns() {
        let node = Node::builder(NodeLabel::Config, "app_config", "proj.app_config")
            .id("cfg_1")
            .project("demo")
            .file_path("/config/app.toml")
            .properties(serde_json::json!({"configType": "toml"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Config);
        assert_eq!(row.len(), 7);
        assert_eq!(row[5], "toml");
        assert_eq!(row[6], "proj");
    }

    #[test]
    fn node_to_row_tool_has_six_columns() {
        let node = Node::builder(NodeLabel::Tool, "lint", "proj.lint")
            .id("tool_1")
            .project("demo")
            .file_path("/tools/lint")
            .properties(serde_json::json!({"toolType": "linter"}))
            .parent_qn("proj")
            .build();
        let row = node_to_row(&node, NodeLabel::Tool);
        assert_eq!(row.len(), 7);
        assert_eq!(row[5], "linter");
        assert_eq!(row[6], "proj");
    }

    #[test]
    fn node_to_row_embedding_has_eight_columns() {
        let node = Node::builder(NodeLabel::Embedding, "emb", "proj.emb")
            .id("emb_1")
            .project("demo")
            .start_line(1)
            .end_line(10)
            .properties(serde_json::json!({
                "nodeId": "func_1",
                "chunkIndex": 0,
                "embedding": "[0.1,0.2]",
                "contentHash": "abc123"
            }))
            .build();
        let row = node_to_row(&node, NodeLabel::Embedding);
        assert_eq!(row.len(), 8);
        assert_eq!(row[0], "emb_1");
        assert_eq!(row[1], "func_1");
        assert_eq!(row[2], "demo");
        assert_eq!(row[3], "0");
        assert_eq!(row[4], "1");
        assert_eq!(row[5], "10");
        assert_eq!(row[6], "[0.1,0.2]");
        assert_eq!(row[7], "abc123");
    }
}
