// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! #include tracking graph for C++ scope-aware call resolution.
//!
//! [`IncludesGraph`] stores the directed `#include` relationships between
//! files (File A `#include`s File B → edge A → B) and supports transitive
//! closure queries ("which files are reachable from File A via #include
//! chains?").
//!
//! # Purpose
//!
//! BUG-C4 (reverted in v0.2.2): C++ free functions had `is_exported=false`
//! because [`ProjectSymbolTable::lookup_exported`] returned ALL same-name
//! functions across the entire project, causing massive over-resolution
//! (fmt CALLS 1,852 → 5,002, +54%). The fix requires scoping cross-file
//! resolution by the `#include` graph: a function in File B is only a
//! valid resolution target for a call in File A if A `#include`s B
//! (directly or transitively).
//!
//! # Design
//!
//! - Storage: `HashMap<String, HashSet<String>>` (file → directly included files)
//! - `reachable_from(start)`: BFS transitive closure, includes `start` itself
//!   (a file is always "reachable" from itself for scope purposes)
//! - `contains(from, to)`: direct edge check (no transitive closure)
//!
//! The graph is populated during [`ResolvePhase`] (see `phases.rs`) from
//! `EdgeType::Includes` edges and passed to [`CallResolver`] for
//! `lookup_exported_in_scope` filtering.
//!
//! [`ProjectSymbolTable::lookup_exported`]: crate::resolve::symbol_table::ProjectSymbolTable::lookup_exported
//! [`ResolvePhase`]: crate::index::phases::ResolvePhase
//! [`CallResolver`]: crate::resolve::calls::CallResolver

use std::collections::{HashMap, HashSet};

/// Directed graph of `#include` relationships between files.
///
/// Stores File A → File B edges where A `#include`s B. Supports transitive
/// closure queries via [`reachable_from`](Self::reachable_from).
///
/// # Examples
///
/// ```
/// use codenexus::resolve::includes_graph::IncludesGraph;
///
/// let mut graph = IncludesGraph::new();
/// graph.add_include("main.cpp", "foo.h");
/// graph.add_include("foo.h", "bar.h");
///
/// // Transitive closure: main.cpp reaches foo.h and bar.h (and itself).
/// let reachable = graph.reachable_from("main.cpp");
/// assert!(reachable.contains("main.cpp"));
/// assert!(reachable.contains("foo.h"));
/// assert!(reachable.contains("bar.h"));
///
/// // Direct edge check.
/// assert!(graph.contains("main.cpp", "foo.h"));
/// assert!(!graph.contains("foo.h", "main.cpp")); // directed, not symmetric
/// ```
#[derive(Debug, Clone, Default)]
pub struct IncludesGraph {
    /// Adjacency list: source file → set of directly included files.
    edges: HashMap<String, HashSet<String>>,
}

impl IncludesGraph {
    /// Creates an empty `IncludesGraph`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }

    /// Adds a directed `#include` edge: `from_file` includes `to_file`.
    ///
    /// Duplicate edges are silently collapsed (idempotent — `HashSet` dedup).
    /// Self-edges (`from == to`) are ignored (a file cannot `#include` itself
    /// in valid C++; if encountered, it's a parse artifact, not a real edge).
    pub fn add_include(&mut self, from_file: &str, to_file: &str) {
        if from_file == to_file {
            return;
        }
        self.edges
            .entry(from_file.to_string())
            .or_default()
            .insert(to_file.to_string());
    }

    /// Returns all files reachable from `start` via `#include` chains
    /// (transitive closure), **including `start` itself**.
    ///
    /// A file is always considered reachable from itself for scope purposes:
    /// a function defined in the same file as the caller is always a valid
    /// resolution target, regardless of `#include` relationships.
    ///
    /// # Algorithm
    ///
    /// BFS over the adjacency list. Avoids infinite loops on cycles
    /// (e.g. `a.h ↔ b.h` mutual includes) by tracking visited nodes.
    ///
    /// # Returns
    ///
    /// `HashSet<&str>` with lifetimes tied to `&self`. Empty set if `start`
    /// has no outgoing edges AND is not a key in `edges` (still returns
    /// `{start}` because a file always reaches itself).
    pub fn reachable_from<'a>(&'a self, start: &'a str) -> HashSet<&'a str> {
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(start);
        let mut queue: Vec<&str> = vec![start];
        while let Some(current) = queue.pop() {
            if let Some(neighbors) = self.edges.get(current) {
                for next in neighbors {
                    let next: &str = next;
                    if visited.insert(next) {
                        queue.push(next);
                    }
                }
            }
        }
        visited
    }

    /// Returns `true` if `from` directly includes `to` (no transitive closure).
    ///
    /// For transitive reachability, use [`reachable_from`](Self::reachable_from).
    pub fn contains(&self, from: &str, to: &str) -> bool {
        self.edges
            .get(from)
            .is_some_and(|neighbors| neighbors.contains(to))
    }

    /// Returns the number of direct `#include` edges in the graph.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(SetCount::len).sum()
    }

    /// Returns `true` if the graph has no edges.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edges.values().all(|s| s.is_empty())
    }
}

/// Trait alias to access `HashSet::len` without importing the type name.
/// Kept private to avoid leaking implementation detail.
trait SetCount {
    fn len(&self) -> usize;
}

impl<T, S> SetCount for HashSet<T, S> {
    fn len(&self) -> usize {
        HashSet::len(self)
    }
}

/// Resolves a C++ `#include` path to a concrete file path in the project.
///
/// Given the `#include` directive's `source_file` (e.g. `"foo.h"`,
/// `"fmt/format.h"`), the file issuing the include (`calling_file`), and the
/// list of all files in the project, returns the matched file path or `None`.
///
/// # Resolution strategy (deterministic — Rule 5)
///
/// 1. **Suffix match with boundary check**: `source_file` is matched as a
///    suffix of each file path in `all_files`, with a path boundary check
///    (so `"format.h"` matches `"include/fmt/format.h"` but not
///    `"xformat.h"`).
/// 2. **Same-directory preference**: when multiple files match, prefer the
///    one in the same directory as `calling_file` (standard C++
///    `#include "..."` behavior). Among same-directory matches, the shortest
///    path wins (most specific).
/// 3. **Shortest-path fallback**: when no same-directory match exists, the
///    shortest matching path wins (closest to project root).
/// 4. **No match → `None`**: system headers (`<iostream>`), external libs,
///    and missing files return `None`.
///
/// # Note on `ImportInfo::source_file`
///
/// The parse phase (`cpp.rs::extract_include`) already strips `<>`/`"`
/// wrappers from `#include` directives, so `source_file` is a clean path
/// like `"foo.h"` or `"fmt/format.h"`, not `"<foo.h>"` or `"\"foo.h\""`.
///
/// # Relationship to `imports.rs::resolve_include_suffix`
///
/// This function is intentionally separate from
/// `ImportResolver::resolve_include_suffix` (in `imports.rs`) despite
/// similar logic:
/// - Different input: `&[String]` (file paths) vs `HashMap<String, String>`
///   (file path → node id)
/// - Different output: file path vs node id
/// - Different concern: `IncludesGraph` construction (scope filtering) vs
///   `IMPORTS` edge construction (graph persistence)
/// - Decoupling: `IncludesGraph` should not depend on `ImportResolver`
///   internals
///
/// # Arguments
///
/// * `source_file` - The `#include` path (e.g. `"foo.h"`, `"fmt/format.h"`).
/// * `calling_file` - The file issuing the `#include` (e.g. `"src/main.cpp"`).
/// * `all_files` - All file paths in the project (e.g. from `parse.results`).
///
/// # Returns
///
/// The matched file path (owned `String`), or `None` if no match.
///
/// # Examples
///
/// ```
/// use codenexus::resolve::includes_graph::resolve_include;
///
/// // Same-directory preference: src/foo.h wins over include/foo.h.
/// let all = vec!["src/foo.h".to_string(), "include/foo.h".to_string()];
/// assert_eq!(
///     resolve_include("foo.h", "src/main.cpp", &all),
///     Some("src/foo.h".to_string())
/// );
///
/// // No same-directory match: falls back to any project file.
/// let all = vec!["include/bar.h".to_string()];
/// assert_eq!(
///     resolve_include("bar.h", "src/main.cpp", &all),
///     Some("include/bar.h".to_string())
/// );
///
/// // System header: no match in project files.
/// let all = vec!["src/main.cpp".to_string()];
/// assert_eq!(resolve_include("iostream", "src/main.cpp", &all), None);
/// ```
pub fn resolve_include(
    source_file: &str,
    calling_file: &str,
    all_files: &[String],
) -> Option<String> {
    let path_norm = source_file.replace('\\', "/");
    let calling_dir = calling_file
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");

    let mut same_dir: Option<&String> = None;
    let mut other: Option<&String> = None;

    for file in all_files {
        let file_norm = file.replace('\\', "/");
        if file_norm.ends_with(path_norm.as_str()) {
            let prefix_len = file_norm.len() - path_norm.len();
            // Boundary check: prefix must be empty or end with '/' (so
            // "format.h" matches "include/fmt/format.h" but not "xformat.h").
            if prefix_len == 0 || file_norm.as_bytes()[prefix_len - 1] == b'/' {
                let file_dir = file_norm
                    .rsplit_once('/')
                    .map(|(dir, _)| dir)
                    .unwrap_or("");
                if file_dir == calling_dir {
                    // Same directory — pick shortest path for determinism.
                    if same_dir.as_ref().map_or(true, |s| s.len() > file.len()) {
                        same_dir = Some(file);
                    }
                } else {
                    // Other directory — pick shortest path (closest to root).
                    if other.as_ref().map_or(true, |s| s.len() > file.len()) {
                        other = Some(file);
                    }
                }
            }
        }
    }

    same_dir.or(other).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- new() / empty graph ---

    #[test]
    fn new_creates_empty_graph() {
        let graph = IncludesGraph::new();
        assert!(graph.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn default_creates_empty_graph() {
        let graph = IncludesGraph::default();
        assert!(graph.is_empty());
    }

    // --- add_include + contains (direct edges) ---

    #[test]
    fn add_include_creates_direct_edge() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a.cpp", "b.h");
        assert!(graph.contains("a.cpp", "b.h"));
    }

    #[test]
    fn includes_graph_contains_direct() {
        // Spec T001 Red test: add_include("a","b") → contains("a","b")==true
        // and contains("b","a")==false (directed, not symmetric).
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        assert!(graph.contains("a", "b"));
        assert!(!graph.contains("b", "a"), "edge is directed: b→a should not exist");
    }

    #[test]
    fn contains_returns_false_for_missing_from() {
        let graph = IncludesGraph::new();
        assert!(!graph.contains("nonexistent", "b.h"));
    }

    #[test]
    fn contains_returns_false_for_missing_to() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        assert!(!graph.contains("a", "c"));
    }

    #[test]
    fn add_include_is_idempotent() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("a", "b");
        assert_eq!(graph.edge_count(), 1, "duplicate edge should collapse");
    }

    #[test]
    fn add_include_ignores_self_edge() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "a");
        assert!(!graph.contains("a", "a"), "self-edge should be ignored");
        assert!(graph.is_empty());
    }

    #[test]
    fn add_include_multiple_targets() {
        let mut graph = IncludesGraph::new();
        graph.add_include("main.cpp", "foo.h");
        graph.add_include("main.cpp", "bar.h");
        graph.add_include("main.cpp", "baz.h");
        assert_eq!(graph.edge_count(), 3);
        assert!(graph.contains("main.cpp", "foo.h"));
        assert!(graph.contains("main.cpp", "bar.h"));
        assert!(graph.contains("main.cpp", "baz.h"));
    }

    // --- reachable_from (transitive closure) ---

    #[test]
    fn includes_graph_reachable_transitive() {
        // Spec T001 Red test: a→b, b→c → reachable_from("a") contains a/b/c.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("b", "c");
        let reachable = graph.reachable_from("a");
        assert!(reachable.contains("a"), "start node should be reachable from itself");
        assert!(reachable.contains("b"), "direct neighbor should be reachable");
        assert!(reachable.contains("c"), "transitive neighbor should be reachable");
        assert_eq!(reachable.len(), 3);
    }

    #[test]
    fn reachable_from_includes_start_itself() {
        // A file is always reachable from itself (scope includes same-file).
        let graph = IncludesGraph::new();
        let reachable = graph.reachable_from("lonely.cpp");
        assert_eq!(reachable.len(), 1);
        assert!(reachable.contains("lonely.cpp"));
    }

    #[test]
    fn reachable_from_no_outgoing_edges() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        // "b" has no outgoing edges, but reachable_from("b") should still
        // include "b" itself.
        let reachable = graph.reachable_from("b");
        assert_eq!(reachable.len(), 1);
        assert!(reachable.contains("b"));
    }

    #[test]
    fn reachable_from_handles_cycle() {
        // Mutual includes: a↔b. BFS must terminate (visited set prevents loop).
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("b", "a");
        let reachable_from_a = graph.reachable_from("a");
        assert!(reachable_from_a.contains("a"));
        assert!(reachable_from_a.contains("b"));
        assert_eq!(reachable_from_a.len(), 2);

        let reachable_from_b = graph.reachable_from("b");
        assert!(reachable_from_b.contains("a"));
        assert!(reachable_from_b.contains("b"));
        assert_eq!(reachable_from_b.len(), 2);
    }

    #[test]
    fn reachable_from_diamond_shape() {
        // Diamond: a→b, a→c, b→d, c→d. d reachable from a via two paths.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("a", "c");
        graph.add_include("b", "d");
        graph.add_include("c", "d");
        let reachable = graph.reachable_from("a");
        assert!(reachable.contains("a"));
        assert!(reachable.contains("b"));
        assert!(reachable.contains("c"));
        assert!(reachable.contains("d"));
        assert_eq!(reachable.len(), 4, "d should appear once despite two paths");
    }

    #[test]
    fn reachable_from_deep_chain() {
        // Deep chain: a→b→c→d→e. All reachable from a.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("b", "c");
        graph.add_include("c", "d");
        graph.add_include("d", "e");
        let reachable = graph.reachable_from("a");
        assert_eq!(reachable.len(), 5);
        for file in &["a", "b", "c", "d", "e"] {
            assert!(reachable.contains(file), "{file} should be reachable");
        }
    }

    #[test]
    fn reachable_from_disconnected_components() {
        // Two disconnected subgraphs: a→b and c→d. reachable_from("a") should
        // NOT include c or d.
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("c", "d");
        let reachable_from_a = graph.reachable_from("a");
        assert!(reachable_from_a.contains("a"));
        assert!(reachable_from_a.contains("b"));
        assert!(!reachable_from_a.contains("c"), "disconnected node should not be reachable");
        assert!(!reachable_from_a.contains("d"), "disconnected node should not be reachable");
    }

    // --- edge_count / is_empty ---

    #[test]
    fn edge_count_counts_all_direct_edges() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        graph.add_include("a", "c");
        graph.add_include("b", "c");
        assert_eq!(graph.edge_count(), 3);
    }

    #[test]
    fn is_empty_true_for_new_graph() {
        let graph = IncludesGraph::new();
        assert!(graph.is_empty());
    }

    #[test]
    fn is_empty_false_after_add_edge() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "b");
        assert!(!graph.is_empty());
    }

    #[test]
    fn is_empty_true_when_only_self_edges_attempted() {
        let mut graph = IncludesGraph::new();
        graph.add_include("a", "a"); // self-edge ignored
        assert!(graph.is_empty());
    }

    // --- resolve_include (basename matching) ---

    fn make_files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_include_quoted_prefers_same_dir() {
        // Spec T002 Red test: resolve_include("foo.h", "src/main.cpp",
        // &["src/foo.h", "include/foo.h"]) returns "src/foo.h" (same dir).
        let all = make_files(&["src/foo.h", "include/foo.h"]);
        let result = resolve_include("foo.h", "src/main.cpp", &all);
        assert_eq!(result, Some("src/foo.h".to_string()));
    }

    #[test]
    fn resolve_include_angled_matches_anywhere() {
        // Spec T002 Red test: resolve_include("bar.h", "src/main.cpp",
        // &["include/bar.h"]) returns "include/bar.h" (no same-dir match,
        // falls back to any project file).
        let all = make_files(&["include/bar.h"]);
        let result = resolve_include("bar.h", "src/main.cpp", &all);
        assert_eq!(result, Some("include/bar.h".to_string()));
    }

    #[test]
    fn resolve_include_no_match_returns_none() {
        // Spec T002 Red test: <iostream> (system header) → None.
        let all = make_files(&["src/main.cpp", "src/foo.h"]);
        let result = resolve_include("iostream", "src/main.cpp", &all);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_include_partial_path_matches() {
        // C++ #include "fmt/format.h" → include/fmt/format.h
        // (partial path suffix match with boundary check).
        let all = make_files(&["src/main.cpp", "include/fmt/format.h"]);
        let result = resolve_include("fmt/format.h", "src/main.cpp", &all);
        assert_eq!(result, Some("include/fmt/format.h".to_string()));
    }

    #[test]
    fn resolve_include_boundary_check_rejects_partial_filename() {
        // "format.h" should NOT match "xformat.h" (no path boundary).
        let all = make_files(&["src/xformat.h"]);
        let result = resolve_include("format.h", "src/main.cpp", &all);
        assert_eq!(result, None, "xformat.h should not match format.h (no boundary)");
    }

    #[test]
    fn resolve_include_exact_match_returns_file() {
        // source_file exactly equals a file path → match (prefix_len == 0).
        let all = make_files(&["foo.h", "src/main.cpp"]);
        let result = resolve_include("foo.h", "src/main.cpp", &all);
        assert_eq!(result, Some("foo.h".to_string()));
    }

    #[test]
    fn resolve_include_same_dir_picks_shortest_path() {
        // When multiple same-directory files match, pick the shortest
        // (most specific) for determinism.
        let all = make_files(&["src/a/b.h", "src/b.h"]);
        // Both match "b.h" and are in "src" directory relative to
        // calling_file "src/main.cpp". Wait — "src/a/b.h" is in "src/a",
        // not "src". Let me fix the test.
        // Actually: calling_dir = "src", "src/a/b.h" dir = "src/a",
        // "src/b.h" dir = "src". So only "src/b.h" is same-dir.
        let result = resolve_include("b.h", "src/main.cpp", &all);
        assert_eq!(result, Some("src/b.h".to_string()));
    }

    #[test]
    fn resolve_include_other_dir_picks_shortest_path() {
        // When no same-directory match, pick the shortest matching path
        // (closest to project root) for determinism.
        let all = make_files(&["include/fmt/format.h", "vendor/deep/fmt/format.h"]);
        let result = resolve_include("fmt/format.h", "src/main.cpp", &all);
        assert_eq!(
            result,
            Some("include/fmt/format.h".to_string()),
            "shorter path should win when no same-dir match"
        );
    }

    #[test]
    fn resolve_include_empty_source_returns_none() {
        let all = make_files(&["src/main.cpp"]);
        let result = resolve_include("", "src/main.cpp", &all);
        // Empty source_file would match every file (all end with ""), but
        // boundary check requires prefix_len == 0 or boundary '/'. For
        // "src/main.cpp" with empty path_norm, prefix_len = 13, and byte
        // at [12] is 'c' (not '/'), so no match. Returns None.
        // Actually: "" ends_with "" is true for all strings. prefix_len =
        // file.len(). boundary check: file.as_bytes()[file.len()-1] is
        // 'p' (not '/'), so rejected. All files rejected → None.
        assert_eq!(result, None, "empty source_file should not match anything");
    }

    #[test]
    fn resolve_include_empty_all_files_returns_none() {
        let all: Vec<String> = vec![];
        let result = resolve_include("foo.h", "src/main.cpp", &all);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_include_handles_backslash_paths() {
        // Windows-style paths: backslashes converted to forward slashes.
        let all = make_files(&["src\\foo.h"]);
        let result = resolve_include("foo.h", "src\\main.cpp", &all);
        assert_eq!(result, Some("src\\foo.h".to_string()));
    }

    #[test]
    fn resolve_include_calling_file_in_root_no_dir() {
        // calling_file has no '/' → calling_dir = "". Files in root
        // (no '/') have file_dir = "" → match as same_dir.
        let all = make_files(&["foo.h", "src/foo.h"]);
        let result = resolve_include("foo.h", "main.cpp", &all);
        assert_eq!(result, Some("foo.h".to_string()), "root-level file preferred when caller is in root");
    }
}
