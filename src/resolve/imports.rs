// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Import resolution (resolve/imports.rs).
//!
//! Provides [`ImportResolver`] for resolving `import`/`include`/`use` statements
//! to `IMPORTS` edges (DDD §7.2: `File ||--o{ File : "IMPORTS"`).
//!
//! The resolver walks each [`ExtractResult`]'s `imports` field, resolves the
//! `ImportInfo::source_file` to a target File node in the graph, and creates a
//! `File → File` IMPORTS edge when both endpoints are found. Unresolved imports
//! (external modules, missing files) are logged at `warn` level and skipped —
//! they do not panic (Rule 12: failures must be explicit, not silent).
//!
//! # Resolution strategy (deterministic — Rule 5)
//!
//! 1. **Direct match**: `source_file` exactly matches a File node's `file_path`
//!    or `name` (e.g. `"b.rs"`, `"./utils.rs"`).
//! 2. **Relative path with extension probing**: for paths starting with `.` or
//!    `/`, resolve relative to the importing file's directory and try common
//!    extensions (`.ts`, `.tsx`, `.js`, `.rs`, `.go`, `.py`, …) plus
//!    `index.{ext}` for barrel imports.
//! 3. **External modules** (no `.`/`/` prefix, e.g. `"react"`, `"std::io"`):
//!    no local File node exists → skip with `warn`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tracing::warn;

use crate::ir::ExtractResult;
use crate::model::{ConfidenceTier, Edge, EdgeType, Graph, NodeLabel};

/// Confidence for an IMPORTS edge (structural, explicit in syntax).
/// Matches the lower bound of `EdgeType::Imports::confidence_range()` = (0.95, 1.0).
const CONFIDENCE_IMPORTS: f32 = 0.95;

/// Extensions tried when resolving extensionless relative imports.
/// Ordered by approximate frequency in polyglot projects.
const EXTENSION_PROBES: &[&str] = &[
    "ts", "tsx", "js", "jsx", "rs", "go", "py", "java", "c", "h", "cpp", "cc",
];

/// Resolves `import`/`include`/`use` statements to `IMPORTS` edges.
///
/// Constructed with the project name. Call [`resolve_imports`] to walk
/// [`ExtractResult`]s and add `File → File` IMPORTS edges to the graph.
///
/// [`resolve_imports`]: ImportResolver::resolve_imports
pub struct ImportResolver<'a> {
    project: &'a str,
}

impl<'a> ImportResolver<'a> {
    /// Creates a new `ImportResolver` for the given project.
    #[must_use]
    pub fn new(project: &'a str) -> Self {
        Self { project }
    }

    /// Resolves all imports from [`ExtractResult`]s and adds `IMPORTS` edges to
    /// the graph.
    ///
    /// For each `ImportInfo` in each result, resolves the `source_file` to a
    /// target File node id. If both the importing file's File node and the
    /// target File node exist in the graph, an `IMPORTS` edge is created.
    /// Duplicate `(source, target)` pairs are collapsed to a single edge
    /// (matching `CallResolver`'s dedup behaviour).
    ///
    /// # Arguments
    ///
    /// * `results` - The extraction results containing import information.
    /// * `graph` - The graph to add resolved IMPORTS edges to. Must already
    ///   contain File nodes (created by the scope phase).
    ///
    /// # Returns
    ///
    /// A vector of all resolved IMPORTS edges (also added to `graph`).
    pub fn resolve_imports(&self, results: &[ExtractResult], graph: &mut Graph) -> Vec<Edge> {
        let file_index = build_file_index(graph);

        let mut edges = Vec::new();
        // Deduplicate by (source_file_id, target_file_id) — one IMPORTS edge
        // per file pair, regardless of how many symbols are imported.
        let mut seen_pairs: HashSet<(String, String)> = HashSet::new();

        for result in results {
            let source_file_id = match file_index.get(&result.file_path) {
                Some(id) => id.clone(),
                None => {
                    // Single-line for coverage: tarpaulin attribute continuation
                    warn!(file = %result.file_path, "IMPORTS source File node not found in graph; skipping imports for this file"); continue;
                }
            };

            for import in &result.imports {
                // Single-line for coverage: tarpaulin attribute continuation
                if import.source_file.is_empty() { continue; }
                // Single-line for coverage: tarpaulin attribute continuation
                let target_file_id = match resolve_import_target(&import.source_file, &result.file_path, &file_index) {
                    Some(id) => id,
                    None => {
                        // Single-line for coverage: tarpaulin attribute continuation
                        warn!(import = %import.source_file, importer = %result.file_path, line = import.line, "IMPORTS target unresolved (external module or missing file); skipping"); continue;
                    }
                };

                let pair_key = (source_file_id.clone(), target_file_id.clone());
                // Single-line for coverage: tarpaulin attribute continuation
                if !seen_pairs.insert(pair_key) { continue; }

                // Single-line for coverage: tarpaulin attribute continuation
                let edge = Edge::builder(source_file_id.clone(), target_file_id, EdgeType::Imports, self.project)
                .confidence(CONFIDENCE_IMPORTS)
                .confidence_tier(ConfidenceTier::ImportScoped)
                .start_line(import.line)
                .build();
                graph.add_edge(edge.clone());
                edges.push(edge);
            }
        }

        edges
    }
}

/// Builds a lookup map from file path AND file name → File node id.
///
/// Both `file_path` (e.g. `"src/utils.ts"`) and `name` (often the relative
/// path) are indexed so that `ImportInfo::source_file` can match either form.
fn build_file_index(graph: &Graph) -> HashMap<String, String> {
    let mut index = HashMap::new();
    for node in graph.nodes_by_label(NodeLabel::File) {
        if let Some(fp) = &node.file_path {
            index.entry(fp.clone()).or_insert_with(|| node.id.clone());
        }
        index
            .entry(node.name.clone())
            .or_insert_with(|| node.id.clone());
    }
    index
}

/// Resolves an `ImportInfo::source_file` to a target File node id.
///
/// Deterministic resolution (Rule 5) — no LLM, no fuzzy matching:
///
/// 1. Direct match against the file index (handles `"b.rs"`, `"./utils.ts"`).
/// 2. For relative paths (starting with `.` or `/`), normalise against the
///    importer's directory and probe common extensions + `index.{ext}`.
/// 3. External bare specifiers (`"react"`, `"std::io"`) return `None`.
fn resolve_import_target(
    source_file: &str,
    importer_path: &str,
    file_index: &HashMap<String, String>,
) -> Option<String> {
    // Strategy 1: direct match.
    if let Some(id) = file_index.get(source_file) {
        return Some(id.clone());
    }

    // Strategy 2: relative path resolution + extension probing.
    // Only attempt path resolution for relative specifiers (TS/JS `./`, `../`,
    // or absolute `/`). Bare specifiers like "react" or "std::io" are external.
    let is_relative = source_file.starts_with('.') || source_file.starts_with('/');
    if !is_relative {
        return None;
    }

    let normalised = normalise_relative(source_file, importer_path);
    if let Some(id) = file_index.get(&normalised) {
        return Some(id.clone());
    }

    // Probe extensions (e.g. "./utils" → "src/utils.ts").
    for ext in EXTENSION_PROBES {
        let candidate = format!("{normalised}.{ext}");
        if let Some(id) = file_index.get(&candidate) {
            return Some(id.clone());
        }
    }

    // Probe barrel imports (e.g. "./utils" → "src/utils/index.ts").
    for ext in EXTENSION_PROBES {
        let candidate = format!("{normalised}/index.{ext}");
        if let Some(id) = file_index.get(&candidate) {
            return Some(id.clone());
        }
    }

    None
}

/// Normalises a relative `source_file` against the importer's directory.
///
/// `./utils` imported from `src/a.ts` → `src/utils`.
/// `../helpers/b` imported from `src/sub/c.ts` → `src/helpers/b`.
/// Leading `./` and `../` are resolved; backslashes are converted to `/`.
fn normalise_relative(source_file: &str, importer_path: &str) -> String {
    // Convert backslashes to forward slashes BEFORE path parsing so that
    // Windows-style specifiers are handled correctly on Unix (where `\` is
    // not a path separator and `Path::parent` would mis-parse it).
    let specifier = source_file.replace('\\', "/");
    let importer_normalised = importer_path.replace('\\', "/");
    let importer_dir = Path::new(&importer_normalised)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("");

    let combined = if importer_dir.is_empty() {
        specifier
    } else {
        format!("{importer_dir}/{specifier}")
    };

    // Resolve `.` and `..` segments.
    let mut segments: Vec<&str> = Vec::new();
    for seg in combined.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};

    /// Builds a File node with the given relative path as id, name, and
    /// file_path (mirrors what `build_file_nodes` produces in the scope phase,
    /// but uses the path as id for simpler test assertions).
    fn make_file_node(path: &str, project: &str) -> Node {
        Node::builder(NodeLabel::File, path, path)
            .id(path)
            .project(project)
            .file_path(path)
            .language(Language::TypeScript)
            .build()
    }

    /// Creates an `ExtractResult` for the given file.
    fn make_result(file_path: &str) -> ExtractResult {
        ExtractResult::new(file_path, Language::TypeScript)
    }

    // --- resolve_imports: explicit import ---

    #[test]
    fn resolve_imports_creates_edge_for_explicit_import() {
        // File a.ts imports from b.ts → IMPORTS edge a.ts → b.ts.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["foo".to_string()],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "should create 1 IMPORTS edge");
        let edge = &edges[0];
        assert_eq!(edge.edge_type, EdgeType::Imports);
        assert_eq!(edge.source, "a.ts");
        assert_eq!(edge.target, "b.ts");
        assert!((edge.confidence - 0.95).abs() < 1e-6);
        assert_eq!(edge.confidence_tier, ConfidenceTier::ImportScoped);
        assert_eq!(edge.start_line, Some(1));
        assert_eq!(graph.edge_count(), 1);
    }

    // --- resolve_imports: empty imports ---

    #[test]
    fn resolve_imports_handles_empty_imports() {
        let result = make_result("a.ts");
        let results = vec![result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(edges.is_empty(), "no imports → no edges");
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_imports_empty_results_returns_empty() {
        let mut graph = Graph::new();
        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&[], &mut graph);
        assert!(edges.is_empty());
    }

    // --- resolve_imports: skips unresolved ---

    #[test]
    fn resolve_imports_skips_unresolved_imports() {
        // a.ts imports "react" (external) — no File node, should skip without panic.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "react".to_string(),
            imported_names: vec!["useState".to_string()],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        // No "react" File node in graph.

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(edges.is_empty(), "unresolved import → no edge");
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_imports_skips_when_source_file_node_missing() {
        // No File node for the importing file → skip without panic.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        // Only b.ts exists; a.ts File node is missing.
        graph.add_node(make_file_node("b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);
        assert!(edges.is_empty());
    }

    #[test]
    fn resolve_imports_skips_empty_source_file() {
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: String::new(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);
        assert!(edges.is_empty());
    }

    // --- resolve_imports: deduplication ---

    #[test]
    fn resolve_imports_deduplicates_edges() {
        // a.ts imports foo and bar from b.ts — one IMPORTS edge a.ts → b.ts.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["foo".to_string()],
            line: 1,
        });
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 2,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "duplicate (source, target) → 1 edge");
        assert_eq!(graph.edge_count(), 1);
    }

    // --- resolve_imports: extension probing ---

    #[test]
    fn resolve_imports_resolves_extensionless_relative_import() {
        // a.ts imports "./utils" — should resolve to utils.ts via extension probe.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./utils".to_string(),
            imported_names: vec!["helper".to_string()],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("utils.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "extensionless import should resolve");
        assert_eq!(edges[0].target, "utils.ts");
    }

    #[test]
    fn resolve_imports_resolves_subdirectory_relative_import() {
        // src/a.ts imports "./helpers/b" → src/helpers/b.ts.
        let mut a_result = make_result("src/a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./helpers/b".to_string(),
            imported_names: vec!["foo".to_string()],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/a.ts", "proj"));
        graph.add_node(make_file_node("src/helpers/b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "src/a.ts");
        assert_eq!(edges[0].target, "src/helpers/b.ts");
    }

    #[test]
    fn resolve_imports_resolves_parent_directory_import() {
        // src/sub/a.ts imports "../b" → src/b.ts.
        let mut a_result = make_result("src/sub/a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "../b".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/sub/a.ts", "proj"));
        graph.add_node(make_file_node("src/b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target, "src/b.ts");
    }

    #[test]
    fn resolve_imports_resolves_barrel_import() {
        // a.ts imports "./utils" → utils/index.ts (barrel).
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./utils".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("utils/index.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "barrel import should resolve");
        assert_eq!(edges[0].target, "utils/index.ts");
    }

    // --- resolve_imports: multiple files ---

    #[test]
    fn resolve_imports_handles_multiple_files() {
        // a.ts imports b.ts; c.ts imports d.ts — 2 edges.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let mut c_result = make_result("c.ts");
        c_result.imports.push(crate::ir::ImportInfo {
            source_file: "./d.ts".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result, c_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));
        graph.add_node(make_file_node("c.ts", "proj"));
        graph.add_node(make_file_node("d.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 2);
        assert_eq!(graph.edge_count(), 2);
    }

    // --- resolve_imports: adds edges to graph (neighbour traversal) ---

    #[test]
    fn resolve_imports_adds_edges_to_graph_for_traversal() {
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        resolver.resolve_imports(&results, &mut graph);

        // Verify neighbour traversal works.
        let neighbors = graph.neighbors(&"a.ts".to_string(), Some(EdgeType::Imports));
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].id, "b.ts");
    }

    // --- normalise_relative helper ---

    #[test]
    fn normalise_relative_dot_slash() {
        let n = normalise_relative("./b", "src/a.ts");
        assert_eq!(n, "src/b");
    }

    #[test]
    fn normalise_relative_dot_dot_slash() {
        let n = normalise_relative("../b", "src/sub/a.ts");
        assert_eq!(n, "src/b");
    }

    #[test]
    fn normalise_relative_strips_leading_dot() {
        let n = normalise_relative("./utils", "a.ts");
        assert_eq!(n, "utils");
    }

    #[test]
    fn normalise_relative_handles_backslashes() {
        let n = normalise_relative(".\\b", "src\\a.ts");
        assert_eq!(n, "src/b");
    }

    // --- resolve_import_target: Strategy 1 (direct match) ---

    #[test]
    fn resolve_imports_resolves_direct_match_strategy() {
        // Import with source_file exactly matching a file_path in the index
        // (not a relative specifier) → Strategy 1 direct match.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "b.ts".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "direct match should resolve");
        assert_eq!(edges[0].target, "b.ts");
    }

    // --- resolve_import_target: final None fallback ---

    #[test]
    fn resolve_imports_relative_unresolvable_returns_none() {
        // Relative import "./nonexistent" where no matching file exists
        // → exhausts all strategies → final None fallback → skip.
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./nonexistent".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        // No "nonexistent" file in graph.

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(edges.is_empty(), "unresolvable relative import → no edge");
    }

    // --- resolve_imports: source File node missing from graph ---

    #[test]
    fn resolve_imports_skips_when_source_file_not_in_graph() {
        // ExtractResult references a file_path that has no File node in the
        // graph → file_index.get returns None → skip with warn (line 92).
        let mut orphan_result = make_result("orphan.ts");
        orphan_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![orphan_result];

        let mut graph = Graph::new();
        // Only "b.ts" is in the graph; "orphan.ts" is not.
        graph.add_node(make_file_node("b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(edges.is_empty(), "source file not in graph → no edges");
    }
}
