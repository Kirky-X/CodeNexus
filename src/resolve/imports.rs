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
//! 2. **Rust module paths** (`crate::`, `self::`, `super::`): resolve to
//!    `src/{path}.rs` or `src/{path}/mod.rs`, stripping a trailing symbol
//!    name component if the full path doesn't match (e.g.
//!    `crate::model::Node` → `src/model.rs`).
//! 3. **Relative path with extension probing**: for paths starting with `.` or
//!    `/`, resolve relative to the importing file's directory and try common
//!    extensions (`.ts`, `.tsx`, `.js`, `.rs`, `.go`, `.py`, …) plus
//!    `index.{ext}` for barrel imports.
//! 4. **External modules** (no `.`/`/` prefix, e.g. `"react"`, `"std::io"`):
//!    no local File node exists → skip with `warn`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tracing::warn;

use crate::ir::ExtractResult;
use crate::model::{ConfidenceTier, Edge, EdgeType, Graph, Language, NodeLabel};

/// Confidence for an IMPORTS edge (structural, explicit in syntax).
/// Matches the lower bound of `EdgeType::Imports::confidence_range()` = (0.95, 1.0).
const CONFIDENCE_IMPORTS: f32 = 0.95;

/// Confidence for a REEXPORTS edge (B7). Structural, explicit in syntax —
/// matches the lower bound of `EdgeType::Reexports::confidence_range()` = (0.95, 1.0).
const CONFIDENCE_REEXPORTS: f32 = 0.95;

/// B7 review (arch-review MEDIUM-2 + security LOW-3): when a wildcard
/// re-export (`pub use foo::*` / `export * from './mod'`) targets more
/// than this many functions, log a `warn!` so barrel-style modules with
/// 1000+ re-exports are surfaced. The threshold is advisory — edges are
/// still created for all targets; the warning just makes the cost visible
/// for diagnosis.
const WILDCARD_REEXPORT_WARN_THRESHOLD: usize = 100;

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
        // B7: Build (file_id, function_name) → function_id index for REEXPORTS
        // edge creation. Re-exports target specific Function nodes (not File
        // nodes), so we need to resolve `imported_names` to their Function ids.
        let func_index = build_function_index(graph, &file_index);

        let mut edges = Vec::new();
        // Deduplicate by (source_file_id, target_file_id) — one IMPORTS edge
        // per file pair, regardless of how many symbols are imported.
        let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
        // B7: Separate dedup set for REEXPORTS edges keyed by
        // (source_file_id, function_id) — a single file may re-export
        // multiple functions, each producing its own REEXPORTS edge.
        let mut seen_reexport_pairs: HashSet<(String, String)> = HashSet::new();

        for result in results {
            // Scheme C (v0.3.0): C++ #include edges are handled by ResolvePhase
            // as EdgeType::Includes (scope-aware). Skip C++ here to avoid
            // duplicate IMPORTS edges — see phases.rs ResolvePhase::run.
            #[cfg(feature = "lang-cpp")]
            if result.language == Language::Cpp {
                continue;
            }
            // result.file_path is absolute in production (e.g.
            // /home/dev/.../src/lib.rs) but file_index keys are relative
            // (e.g. `src/lib.rs`). find_file_in_index handles this mismatch.
            let (source_file_id, importer_rel_path) = match find_file_in_index(
                &file_index,
                &result.file_path,
            ) {
                Some((id, rel)) => (id, rel),
                None => {
                    // Single-line for coverage: tarpaulin attribute continuation
                    warn!(file = %result.file_path, "IMPORTS source File node not found in graph; skipping imports for this file");
                    continue;
                }
            };

            for import in &result.imports {
                // Single-line for coverage: tarpaulin attribute continuation
                if import.source_file.is_empty() {
                    continue;
                }
                // Single-line for coverage: tarpaulin attribute continuation
                let target_file_id = match resolve_import_target(
                    &import.source_file,
                    &importer_rel_path,
                    &file_index,
                ) {
                    Some(id) => id,
                    None => {
                        // Single-line for coverage: tarpaulin attribute continuation
                        warn!(import = %import.source_file, importer = %result.file_path, line = import.line, "IMPORTS target unresolved (external module or missing file); skipping");
                        continue;
                    }
                };

                // IMPORTS edge: one per (source, target) file pair.
                let pair_key = (source_file_id.clone(), target_file_id.clone());
                // Single-line for coverage: tarpaulin attribute continuation
                if seen_pairs.insert(pair_key) {
                    let edge = Edge::builder(
                        source_file_id.clone(),
                        target_file_id.clone(),
                        EdgeType::Imports,
                        self.project,
                    )
                    .confidence(CONFIDENCE_IMPORTS)
                    .confidence_tier(ConfidenceTier::ImportScoped)
                    .start_line(import.line)
                    .build();
                    graph.add_edge(edge.clone());
                    edges.push(edge);
                }

                // B7: REEXPORTS edge for `pub use` / `export ... from`.
                // Targets are Function nodes in the resolved file. When
                // `imported_names` is non-empty, only those named functions
                // are re-exported; when empty (wildcard `pub use foo::*`),
                // every function in the target file is re-exported.
                if import.is_reexport {
                    let target_func_ids = resolve_reexport_targets(
                        &target_file_id,
                        &import.imported_names,
                        &func_index,
                    );
                    for func_id in target_func_ids {
                        let reexport_key = (source_file_id.clone(), func_id.clone());
                        // Single-line for coverage: tarpaulin attribute continuation
                        if !seen_reexport_pairs.insert(reexport_key) {
                            continue;
                        }
                        let edge = Edge::builder(
                            source_file_id.clone(),
                            func_id,
                            EdgeType::Reexports,
                            self.project,
                        )
                        .confidence(CONFIDENCE_REEXPORTS)
                        .confidence_tier(ConfidenceTier::ImportScoped)
                        .start_line(import.line)
                        .build();
                        graph.add_edge(edge.clone());
                        edges.push(edge);
                    }
                }
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

/// B7: Builds a lookup map from `file_id` → (`function_name` → `function_id`).
///
/// Used by [`resolve_reexport_targets`] to resolve `pub use foo::bar`'s
/// `bar` to its Function node id. `file_id` is the File node id that owns
/// the function (looked up via `file_index` + `Function.file_path`).
///
/// Both `Function` and `Method` labels are indexed — `pub use` can re-export
/// either. When multiple functions share the same name in the same file
/// (e.g. overloaded methods, generics), the first one encountered wins;
/// a `warn!` is emitted so the ambiguity is visible (arch-review LOW-3).
///
/// # Performance (perf-review MEDIUM-1 + MEDIUM-2 + MEDIUM-3)
///
/// - Nested `HashMap<String, HashMap<String, String>>` (was a flat
///   `HashMap<(String, String), String>`) so wildcard lookups are
///   O(target_file_function_count) instead of O(total_functions), and
///   named lookups avoid constructing a `(String, String)` tuple key.
/// - `file_index` keys are pre-normalised (`\` → `/`) once at entry
///   instead of inside the per-function loop, avoiding O(N·F) repeated
///   `String::replace` allocations.
fn build_function_index(
    graph: &Graph,
    file_index: &HashMap<String, String>,
) -> HashMap<String, HashMap<String, String>> {
    use std::collections::hash_map::Entry;
    // perf-review MEDIUM-2: pre-normalise file_index keys once so the
    // per-function suffix match doesn't re-allocate for every key.
    let normalised_index: HashMap<String, String> = file_index
        .iter()
        .map(|(k, v)| (k.replace('\\', "/"), v.clone()))
        .collect();
    let mut index: HashMap<String, HashMap<String, String>> = HashMap::new();
    for label in [NodeLabel::Function, NodeLabel::Method] {
        for node in graph.nodes_by_label(label) {
            let Some(fp) = &node.file_path else {
                continue;
            };
            // Resolve Function.file_path → owning File node id. Try direct
            // match first, then suffix match via the shared helper
            // (arch-review MEDIUM-1: DRY with find_file_in_index).
            let file_id = file_index.get(fp).cloned().or_else(|| {
                find_best_suffix_match(&normalised_index, fp).map(|(_, id)| id.clone())
            });
            let Some(file_id) = file_id else { continue };
            // arch-review LOW-3: log duplicate function names so the
            // "first wins" ambiguity is visible (dead-code may false-negative
            // on the shadowed method).
            match index.entry(file_id).or_default().entry(node.name.clone()) {
                Entry::Occupied(_) => {
                    warn!(file = %fp, name = %node.name, "duplicate function name in file; first wins (dead-code may false-negative)");
                }
                Entry::Vacant(v) => {
                    v.insert(node.id.clone());
                }
            }
        }
    }
    index
}

/// B7: Resolves the Function ids that a re-export statement targets.
///
/// - When `imported_names` is non-empty (e.g. `pub use foo::bar`), returns
///   the Function ids matching those names in `target_file_id`.
/// - When `imported_names` is empty (wildcard `pub use foo::*`), returns
///   every Function id owned by `target_file_id`.
///
/// Names that don't match any function in the target file are silently
/// skipped (the target may be a struct/enum/trait/const, which are not
/// tracked by the function index — dead-code analysis only cares about
/// Function/Method reachability).
///
/// # Performance (perf-review MEDIUM-1 + MEDIUM-3)
///
/// Nested `HashMap<String, HashMap<String, String>>` enables O(1) file
/// lookup + O(K) name lookup (K = `imported_names.len()`), with zero
/// temporary `(String, String)` tuple allocations. Wildcard lookups are
/// O(target_file_function_count) instead of O(total_functions).
fn resolve_reexport_targets(
    target_file_id: &str,
    imported_names: &[String],
    func_index: &HashMap<String, HashMap<String, String>>,
) -> Vec<String> {
    let Some(by_file) = func_index.get(target_file_id) else {
        return Vec::new();
    };
    if imported_names.is_empty() {
        // Wildcard re-export: every function in the target file.
        // arch-review MEDIUM-2 + security LOW-3: warn on barrel-scale
        // re-exports so the cost is visible (advisory — edges still created).
        let count = by_file.len();
        if count > WILDCARD_REEXPORT_WARN_THRESHOLD {
            warn!(
                target = target_file_id,
                count,
                threshold = WILDCARD_REEXPORT_WARN_THRESHOLD,
                "wildcard re-export targets exceed threshold; dead-code may over-approximate liveness"
            );
        }
        by_file.values().cloned().collect()
    } else {
        imported_names
            .iter()
            .filter_map(|name| by_file.get(name).cloned())
            .collect()
    }
}

/// Finds the longest suffix-matching key in `file_index` for `path`, with
/// path-boundary check (Rule 5 determinism).
///
/// B7 review (arch-review MEDIUM-1): extracted as a shared helper so
/// [`find_file_in_index`] and [`build_function_index`] no longer duplicate
/// the suffix-matching algorithm. Both callers need to bridge the absolute
/// (production `file_path`) vs relative (`file_index` keys) gap, and both
/// must pick the LONGEST suffix match for determinism (HashMap iteration
/// order is non-deterministic).
///
/// # Key normalisation (audit LOW-2 + LOW-4)
///
/// `file_index` keys may or may not be pre-normalised — this function
/// defensively normalises each key with `rel.replace('\\', "/")` inside
/// the loop. [`build_function_index`] passes a pre-normalised index (so
/// the `replace` is a no-op allocation there); [`find_file_in_index`]
/// passes the raw `file_index` (so the `replace` is required there).
/// Pre-normalising at every caller would cost O(N) per call for
/// `find_file_in_index` (invoked per ExtractResult), so the defensive
/// in-loop normalisation is kept as the cheaper trade-off. `path` is
/// normalised once at entry.
fn find_best_suffix_match<'a>(
    file_index: &'a HashMap<String, String>,
    path: &str,
) -> Option<(&'a String, &'a String)> {
    let path_norm = path.replace('\\', "/");
    let mut best: Option<(&String, &String)> = None;
    for (rel, id) in file_index {
        // Defensive normalisation: required for find_file_in_index callers
        // (raw file_index), no-op for build_function_index callers
        // (pre-normalised index). See function docstring.
        let rel_norm = rel.replace('\\', "/");
        if path_norm.ends_with(rel_norm.as_str()) {
            let prefix_len = path_norm.len() - rel_norm.len();
            let boundary_ok = prefix_len == 0 || path_norm.as_bytes()[prefix_len - 1] == b'/';
            if boundary_ok && best.as_ref().is_none_or(|(r, _)| rel.len() > r.len()) {
                best = Some((rel, id));
            }
        }
    }
    best
}

/// Finds a File node id and its relative path in the index.
///
/// `result.file_path` is absolute in production (e.g.
/// `/home/dev/projects/CodeNexus/src/lib.rs`) but `file_index` keys are
/// relative paths normalized by the scope phase (e.g. `src/lib.rs`). This
/// function tries direct match first, then suffix match with path boundary
/// check to bridge the absolute/relative gap.
///
/// Returns `(file_node_id, relative_path)` on success. The relative_path is
/// used as `importer_path` in [`resolve_import_target`] so that
/// `normalise_relative` works correctly (it expects relative paths).
fn find_file_in_index(
    file_index: &HashMap<String, String>,
    path: &str,
) -> Option<(String, String)> {
    // Direct match (handles relative paths and test cases).
    if let Some(id) = file_index.get(path) {
        return Some((id.clone(), path.to_string()));
    }
    // Suffix match: path may be absolute while file_index uses relative paths.
    // e.g. path = "/home/dev/projects/CodeNexus/src/lib.rs"
    //      file_index key = "src/lib.rs"
    //
    // Pick the LONGEST suffix match (most specific) for determinism (Rule 5):
    // HashMap iteration order is non-deterministic, so returning the first
    // match would produce different results across runs when multiple keys
    // suffix-match the same path (e.g. "index.ts" and "src/index.ts" both
    // match "/proj/src/index.ts").
    // Boundary check accepts both `/` and `\` for cross-platform support.
    //
    // B7 review (arch-review MEDIUM-1): delegates to find_best_suffix_match
    // to share the algorithm with build_function_index (DRY).
    find_best_suffix_match(file_index, path).map(|(rel, id)| (id.clone(), rel.clone()))
}

/// Resolves an `ImportInfo::source_file` to a target File node id.
///
/// Deterministic resolution (Rule 5) — no LLM, no fuzzy matching:
///
/// 1. Direct match against the file index (handles `"b.rs"`, `"./utils.ts"`).
/// 2. Rust module paths (`crate::`, `self::`, `super::`) resolve to
///    `src/{path}.rs` or `src/{path}/mod.rs`, stripping a trailing symbol
///    name component if the full path doesn't match.
/// 3. For relative paths (starting with `.` or `/`), normalise against the
///    importer's directory and probe common extensions + `index.{ext}`.
/// 4. External bare specifiers (`"react"`, `"std::io"`) return `None`.
fn resolve_import_target(
    source_file: &str,
    importer_path: &str,
    file_index: &HashMap<String, String>,
) -> Option<String> {
    // Strategy 1: direct match.
    if let Some(id) = file_index.get(source_file) {
        return Some(id.clone());
    }

    // Strategy 2: Rust module path resolution (crate::, self::, super::).
    // Rust `use crate::model::Node` produces source_file = "crate::model::Node",
    // which needs module-path-aware resolution to find src/model.rs.
    if let Some(id) = resolve_rust_module_path(source_file, importer_path, file_index) {
        return Some(id);
    }

    // Strategy 3: relative path resolution + extension probing.
    // Only attempt path resolution for relative specifiers (TS/JS `./`, `../`,
    // or absolute `/`). Bare specifiers like "react" or "std::io" are external.
    let is_relative = source_file.starts_with('.') || source_file.starts_with('/');
    if !is_relative {
        // Strategy 4: C/C++ #include suffix matching.
        // Bare filenames like "format.h" or partial paths like "fmt/format.h"
        // that look like file paths (contain a dot) are matched as suffixes of
        // file_index keys, with path boundary check. Prefers same-directory
        // matches (standard #include "..." behavior).
        if source_file.contains('.') {
            if let Some(id) = resolve_include_suffix(source_file, importer_path, file_index) {
                return Some(id);
            }
        }
        // Strategy 5: Java class import resolution.
        // Java imports like "com.google.gson.Gson" are dotted package paths
        // that map to file paths by replacing '.' with '/' and appending
        // '.java'. Returns None for external deps (JDK, Maven artifacts) that
        // have no matching local file.
        if let Some(id) = resolve_java_class_import(source_file, importer_path, file_index) {
            return Some(id);
        }
        return None;
    }

    let normalised = normalise_relative(source_file, importer_path);
    // TS ESM (NodeNext / moduleResolution=bundler) requires `.js`/`.jsx`/`.mjs`/`.cjs`
    // extensions in specifiers even when the source file is `.ts`/`.tsx`. Strip these
    // before probing so `./types/api.js` resolves to `types/api.ts`.
    let normalised = strip_js_style_extension(&normalised);
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

/// Resolves a C/C++ `#include` path by suffix-matching against file_index keys.
///
/// Bare filenames (`"format.h"`) and partial paths (`"fmt/format.h"`) are
/// matched as suffixes of file paths in the index, with a path boundary check
/// (so `"format.h"` matches `"include/fmt/format.h"` but not `"xformat.h"`).
///
/// When multiple files match, the standard C++ `#include "..."` behavior is
/// followed: prefer files in the same directory as the importer, then fall
/// back to the shortest matching path (closest to project root).
fn resolve_include_suffix(
    include_path: &str,
    importer_path: &str,
    file_index: &HashMap<String, String>,
) -> Option<String> {
    let path_norm = include_path.replace('\\', "/");
    let importer_dir = importer_path
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");

    let mut same_dir: Option<(&String, &String)> = None;
    let mut other: Option<(&String, &String)> = None;

    for (rel, id) in file_index {
        let rel_norm = rel.replace('\\', "/");
        if rel_norm.ends_with(path_norm.as_str()) {
            let prefix_len = rel_norm.len() - path_norm.len();
            if prefix_len == 0 || rel_norm.as_bytes()[prefix_len - 1] == b'/' {
                let match_dir = rel_norm.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
                if match_dir == importer_dir {
                    // Same directory — pick shortest path for determinism.
                    if same_dir.as_ref().is_none_or(|(r, _)| r.len() > rel.len()) {
                        same_dir = Some((rel, id));
                    }
                } else {
                    // Other directory — pick shortest path (closest to root).
                    if other.as_ref().is_none_or(|(r, _)| r.len() > rel.len()) {
                        other = Some((rel, id));
                    }
                }
            }
        }
    }

    same_dir.or(other).map(|(_, id)| id.clone())
}

/// Resolves a Java class import by mapping the dotted package path to a file
/// path and suffix-matching against `file_index`.
///
/// Java imports like `com.google.gson.Gson` map to file path
/// `com/google/gson/Gson.java` (replace `.` with `/`, append `.java`). The
/// mapped path is then suffix-matched against file_index keys (which may have
/// a `src/main/java/` prefix in Maven layouts).
///
/// For static imports (`com.google.gson.Gson.fromJson`) where the last
/// component is a member name rather than a class, the parent path is also
/// tried (`com/google/gson/Gson.java`).
///
/// Returns `None` for external dependencies (JDK classes like `java.util.List`,
/// Maven artifacts) that have no matching local file.
fn resolve_java_class_import(
    source_file: &str,
    importer_path: &str,
    file_index: &HashMap<String, String>,
) -> Option<String> {
    // Java imports are dotted package paths: "com.google.gson.Gson".
    // A '/' indicates a file path (handled by strategies 3/4), not a package.
    if source_file.contains('/') || !source_file.contains('.') {
        return None;
    }

    // Full path: com.google.gson.Gson → com/google/gson/Gson.java
    let mapped = format!("{}.java", source_file.replace('.', "/"));
    if let Some(id) = resolve_include_suffix(&mapped, importer_path, file_index) {
        return Some(id);
    }

    // Strip last component (static import member or symbol name):
    // com.google.gson.Gson.fromJson → com/google/gson/Gson.java
    if let Some((parent, _)) = source_file.rsplit_once('.') {
        let mapped = format!("{}.java", parent.replace('.', "/"));
        if let Some(id) = resolve_include_suffix(&mapped, importer_path, file_index) {
            return Some(id);
        }
    }

    None
}

/// Strips `.js`/`.jsx`/`.mjs`/`.cjs` extensions from a normalised path.
///
/// TS ESM (NodeNext / moduleResolution=bundler) requires `.js` extensions in
/// import specifiers even when the source file is `.ts`. This strips the
/// extension so downstream extension probing can find the `.ts`/`.tsx` file.
/// Non-JS extensions (`.ts`, `.tsx`, `.rs`, …) are preserved.
fn strip_js_style_extension(path: &str) -> String {
    for ext in [".js", ".jsx", ".mjs", ".cjs"] {
        if let Some(stripped) = path.strip_suffix(ext) {
            return stripped.to_string();
        }
    }
    path.to_string()
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

/// Resolves a Rust module path (`crate::`, `self::`, `super::` prefix) to a
/// target File node id.
///
/// Rust `use crate::model::Node` produces `source_file = "crate::model::Node"`.
/// The last component (`Node`) is typically a symbol name, not a module, so we
/// try both the full path and the parent path (stripping the last component).
///
/// # Candidates (tried in order)
///
/// For `crate::a::b`:
/// 1. `src/a/b.rs`, `src/a/b/mod.rs` (full path as module)
/// 2. `src/a.rs`, `src/a/mod.rs` (last component is symbol name)
///
/// For `self::b` / `super::b`, the same logic applies but the path is
/// normalised relative to the importer's directory first.
fn resolve_rust_module_path(
    source_file: &str,
    importer_path: &str,
    file_index: &HashMap<String, String>,
) -> Option<String> {
    // Detect Rust module path prefixes. Return None for non-Rust specifiers
    // (e.g. "react", "./b.ts") so Strategy 3 (relative path) can handle them.
    if let Some(module_path) = source_file.strip_prefix("crate::") {
        let path = module_path.replace("::", "/");
        return try_candidates(&rust_crate_candidates(&path), file_index);
    }

    if let Some(module_path) = source_file.strip_prefix("self::") {
        let path = module_path.replace("::", "/");
        let relative = format!("./{path}");
        let normalised = normalise_relative(&relative, importer_path);
        return try_candidates(&rust_relative_candidates(&normalised), file_index);
    }

    if let Some(module_path) = source_file.strip_prefix("super::") {
        let path = module_path.replace("::", "/");
        let relative = format!("../{path}");
        let normalised = normalise_relative(&relative, importer_path);
        return try_candidates(&rust_relative_candidates(&normalised), file_index);
    }

    // Unprefixed Rust module path (e.g. `cli::run` in lib.rs where `mod cli;`
    // is declared). In lib.rs context, `cli::run` is equivalent to
    // `crate::cli::run` (Rust's implicit crate-root-relative path). Try to
    // resolve as internal module: first component is a module name, rest is
    // path/symbol.
    //
    // BUG-FIX: `pub use cli::run` in src/lib.rs produces `source_file =
    // "cli::run"`. Without this branch, all strategies fail and no
    // REEXPORTS edge is created, causing dead_code false positives
    // (cli.rs::run judged dead despite being the crate root entry point via
    // re-export). On CalNexus this was 100% false-positive rate on cli.rs
    // (11 functions) because the B7 re-export seed never fired.
    //
    // Boundary: only attempt if the path contains `::` and doesn't contain
    // `/` or `.` (file path markers). External crates (std, serde, etc.)
    // will fail the candidate lookup and return None — no false-positive
    // edges. `rust_crate_candidates` already tries both the full path
    // (`src/cli/run.rs`) and the parent path (`src/cli.rs`, stripping the
    // trailing symbol name), matching the `crate::` branch's behaviour.
    if source_file.contains("::") && !source_file.contains('/') && !source_file.contains('.') {
        // Defense-in-depth: reject path traversal from tree-sitter input.
        // Candidates are only looked up in `file_index` (project-internal
        // paths), but reject early to keep the invariant explicit.
        if source_file.contains("..") {
            return None;
        }
        let path = source_file.replace("::", "/");
        return try_candidates(&rust_crate_candidates(&path), file_index);
    }

    None
}

/// Returns the first match from a list of candidate file paths.
fn try_candidates(candidates: &[String], file_index: &HashMap<String, String>) -> Option<String> {
    for candidate in candidates {
        if let Some(id) = file_index.get(candidate) {
            return Some(id.clone());
        }
    }
    None
}

/// Generates candidate file paths for `crate::` prefixed module paths.
///
/// `crate::a::b` (path = "a/b") tries:
/// - `src/a/b.rs`, `src/a/b/mod.rs` (full path as module)
/// - `src/a.rs`, `src/a/mod.rs` (last component is symbol name)
fn rust_crate_candidates(path: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    // Full path: the entire path is a module file.
    candidates.push(format!("src/{path}.rs"));
    candidates.push(format!("src/{path}/mod.rs"));
    // Parent path: last component is a symbol name (e.g. `Node` in `model::Node`).
    if let Some((parent, _)) = path.rsplit_once('/') {
        candidates.push(format!("src/{parent}.rs"));
        candidates.push(format!("src/{parent}/mod.rs"));
    }
    candidates
}

/// Generates candidate file paths for `self::`/`super::` prefixed module paths.
///
/// `self::b` (normalised = "src/b") tries:
/// - `src/b.rs`, `src/b/mod.rs` (full path as module)
/// - `src.rs`, `src/mod.rs` — unlikely but covered for single-component parents
///
/// `self::a::b` (normalised = "src/a/b") tries:
/// - `src/a/b.rs`, `src/a/b/mod.rs` (full path as module)
/// - `src/a.rs`, `src/a/mod.rs` (last component is symbol name)
fn rust_relative_candidates(normalised: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    // Full path: the entire normalised path is a module file.
    candidates.push(format!("{normalised}.rs"));
    candidates.push(format!("{normalised}/mod.rs"));
    // Parent path: last component is a symbol name.
    if let Some((parent, _)) = normalised.rsplit_once('/') {
        candidates.push(format!("{parent}.rs"));
        candidates.push(format!("{parent}/mod.rs"));
    }
    candidates
}

#[cfg(all(test, feature = "lang-cpp", feature = "lang-typescript"))]
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

    /// Creates a C++ `ExtractResult` for the given file.
    fn make_result_cpp(file_path: &str) -> ExtractResult {
        ExtractResult::new(file_path, Language::Cpp)
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
        });
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 2,
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
        });
        let mut c_result = make_result("c.ts");
        c_result.imports.push(crate::ir::ImportInfo {
            source_file: "./d.ts".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
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
            is_reexport: false,
        });
        let results = vec![orphan_result];

        let mut graph = Graph::new();
        // Only "b.ts" is in the graph; "orphan.ts" is not.
        graph.add_node(make_file_node("b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(edges.is_empty(), "source file not in graph → no edges");
    }

    // --- resolve_imports: Rust module path resolution (crate::) ---

    #[test]
    fn resolve_imports_resolves_rust_crate_prefix_to_src_file() {
        // src/lib.rs imports `crate::model` → src/model.rs
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "crate::model".to_string(),
            imported_names: vec!["Node".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/model.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "crate:: prefix should resolve to src/model.rs"
        );
        assert_eq!(edges[0].source, "src/lib.rs");
        assert_eq!(edges[0].target, "src/model.rs");
    }

    #[test]
    fn resolve_imports_resolves_rust_crate_prefix_with_symbol_name() {
        // src/lib.rs imports `crate::model::Node` → src/model.rs (Node is a symbol)
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "crate::model::Node".to_string(),
            imported_names: vec!["Node".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/model.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "symbol name should be stripped, resolving to src/model.rs"
        );
        assert_eq!(edges[0].target, "src/model.rs");
    }

    #[test]
    fn resolve_imports_resolves_rust_crate_prefix_to_mod_file() {
        // src/lib.rs imports `crate::model` → src/model/mod.rs
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "crate::model".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/model/mod.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "crate:: prefix should resolve to src/model/mod.rs"
        );
        assert_eq!(edges[0].target, "src/model/mod.rs");
    }

    #[test]
    fn resolve_imports_resolves_rust_crate_nested_module() {
        // src/lib.rs imports `crate::parse::parser` → src/parse/parser.rs
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "crate::parse::parser".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/parse/parser.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "nested crate:: path should resolve");
        assert_eq!(edges[0].target, "src/parse/parser.rs");
    }

    // --- resolve_imports: Rust module path resolution (self::, super::) ---

    #[test]
    fn resolve_imports_resolves_rust_self_prefix() {
        // src/lib.rs imports `self::model` → src/model.rs
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "self::model".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/model.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "self:: prefix should resolve");
        assert_eq!(edges[0].target, "src/model.rs");
    }

    #[test]
    fn resolve_imports_resolves_rust_super_prefix() {
        // src/sub/mod.rs imports `super::model` → src/model.rs
        let mut sub_result = make_result("src/sub/mod.rs");
        sub_result.imports.push(crate::ir::ImportInfo {
            source_file: "super::model".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![sub_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/sub/mod.rs", "proj"));
        graph.add_node(make_file_node("src/model.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "super:: prefix should resolve");
        assert_eq!(edges[0].target, "src/model.rs");
    }

    #[test]
    fn resolve_imports_resolves_rust_super_prefix_with_symbol() {
        // src/sub/mod.rs imports `super::model::Node` → src/model.rs
        let mut sub_result = make_result("src/sub/mod.rs");
        sub_result.imports.push(crate::ir::ImportInfo {
            source_file: "super::model::Node".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![sub_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/sub/mod.rs", "proj"));
        graph.add_node(make_file_node("src/model.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "super:: with symbol should strip last component"
        );
        assert_eq!(edges[0].target, "src/model.rs");
    }

    // --- resolve_imports: Rust external crate skipped ---

    #[test]
    fn resolve_imports_skips_rust_external_crate() {
        // src/lib.rs imports `std::io` (external crate) — no File node, skip.
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "std::io".to_string(),
            imported_names: vec!["Read".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(edges.is_empty(), "external crate (std::io) → no edge");
    }

    // --- resolve_imports: Rust module path resolution (unprefixed, e.g. `cli::run`) ---
    //
    // BUG-FIX: `pub use cli::run` in src/lib.rs (where `mod cli;` is declared)
    // produces `source_file = "cli::run"`. Without unprefixed module path
    // resolution, this fails all strategies and no REEXPORTS edge is created,
    // causing dead_code false positives (cli.rs::run judged dead despite being
    // the crate root entry point via re-export).
    //
    // In lib.rs context, `cli::run` is equivalent to `crate::cli::run` (Rust's
    // implicit crate-root-relative path). The resolver tries `src/cli.rs` etc.

    #[test]
    fn resolve_imports_resolves_unprefixed_rust_module_path_to_src_file() {
        // src/lib.rs `pub use cli::run` → src/cli.rs
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "cli::run".to_string(),
            imported_names: vec!["run".to_string()],
            line: 1,
            is_reexport: true,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/cli.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "unprefixed `cli::run` should resolve to src/cli.rs"
        );
        assert_eq!(edges[0].source, "src/lib.rs");
        assert_eq!(edges[0].target, "src/cli.rs");
    }

    #[test]
    fn resolve_imports_unprefixed_rust_module_path_creates_reexports_edge() {
        // src/lib.rs `pub use cli::run` (is_reexport=true) → REEXPORTS edge
        // targeting the `run` Function node in src/cli.rs.
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "cli::run".to_string(),
            imported_names: vec!["run".to_string()],
            line: 1,
            is_reexport: true,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/cli.rs", "proj"));
        // The `run` Function node in src/cli.rs.
        graph.add_node(
            Node::builder(NodeLabel::Function, "run", "proj.src.cli.rs.run")
                .id("proj.src.cli.rs.run")
                .project("proj")
                .file_path("src/cli.rs")
                .language(Language::Rust)
                .is_exported(true)
                .build(),
        );

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        // 1 IMPORTS edge (lib.rs → cli.rs) + 1 REEXPORTS edge (lib.rs → run Function)
        assert_eq!(edges.len(), 2, "should create IMPORTS + REEXPORTS edges");
        let reexports: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Reexports)
            .collect();
        assert_eq!(reexports.len(), 1, "should create 1 REEXPORTS edge");
        assert_eq!(reexports[0].source, "src/lib.rs");
        assert_eq!(reexports[0].target, "proj.src.cli.rs.run");
    }

    #[test]
    fn resolve_imports_resolves_unprefixed_nested_rust_module_path() {
        // src/lib.rs `pub use cli::sub::run` → src/cli/sub.rs (or src/cli.rs)
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "cli::sub::run".to_string(),
            imported_names: vec!["run".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/cli/sub.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(edges.len(), 1, "nested unprefixed path should resolve");
        assert_eq!(edges[0].target, "src/cli/sub.rs");
    }

    #[test]
    fn resolve_imports_unprefixed_rust_module_path_strips_symbol_name() {
        // src/lib.rs `pub use cli::run` → tries src/cli/run.rs first (full path),
        // then src/cli.rs (parent path, stripping symbol `run`).
        // When only src/cli.rs exists, should resolve via parent path.
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "cli::run".to_string(),
            imported_names: vec!["run".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        // Only src/cli.rs exists (not src/cli/run.rs).
        graph.add_node(make_file_node("src/cli.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "should resolve via parent path (symbol stripped)"
        );
        assert_eq!(edges[0].target, "src/cli.rs");
    }

    #[test]
    fn resolve_imports_unprefixed_external_crate_still_skipped() {
        // `std::io` (external) with no matching src/std.rs → skip.
        // Ensures the unprefixed resolver doesn't create spurious edges for
        // external crates whose names happen to lack `crate::` prefix.
        let mut lib_result = make_result("src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "std::io".to_string(),
            imported_names: vec!["Read".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        // No src/std.rs or src/std/io.rs in graph.

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "external `std::io` → no edge (no false positive)"
        );
    }

    // --- resolve_rust_module_path helper (direct unit tests) ---

    #[test]
    fn resolve_rust_module_path_crate_prefix_resolves_src_file() {
        let mut index = HashMap::new();
        index.insert("src/model.rs".to_string(), "id-model".to_string());

        let result = resolve_rust_module_path("crate::model", "src/lib.rs", &index);
        assert_eq!(result, Some("id-model".to_string()));
    }

    #[test]
    fn resolve_rust_module_path_crate_prefix_with_symbol_strips_last() {
        let mut index = HashMap::new();
        index.insert("src/model.rs".to_string(), "id-model".to_string());

        let result = resolve_rust_module_path("crate::model::Node", "src/lib.rs", &index);
        assert_eq!(result, Some("id-model".to_string()));
    }

    #[test]
    fn resolve_rust_module_path_crate_prefix_resolves_mod_file() {
        let mut index = HashMap::new();
        index.insert("src/model/mod.rs".to_string(), "id-model-mod".to_string());

        let result = resolve_rust_module_path("crate::model", "src/lib.rs", &index);
        assert_eq!(result, Some("id-model-mod".to_string()));
    }

    #[test]
    fn resolve_rust_module_path_external_returns_none() {
        let mut index = HashMap::new();
        index.insert("src/model.rs".to_string(), "id-model".to_string());

        let result = resolve_rust_module_path("std::io", "src/lib.rs", &index);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_rust_module_path_non_rust_returns_none() {
        let index = HashMap::new();

        let result = resolve_rust_module_path("react", "src/lib.ts", &index);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_rust_module_path_no_match_returns_none() {
        let mut index = HashMap::new();
        index.insert("src/other.rs".to_string(), "id-other".to_string());

        let result = resolve_rust_module_path("crate::nonexistent", "src/lib.rs", &index);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_rust_module_path_self_prefix_resolves() {
        let mut index = HashMap::new();
        index.insert("src/model.rs".to_string(), "id-model".to_string());

        let result = resolve_rust_module_path("self::model", "src/lib.rs", &index);
        assert_eq!(result, Some("id-model".to_string()));
    }

    #[test]
    fn resolve_rust_module_path_super_prefix_resolves() {
        let mut index = HashMap::new();
        index.insert("src/model.rs".to_string(), "id-model".to_string());

        let result = resolve_rust_module_path("super::model", "src/sub/mod.rs", &index);
        assert_eq!(result, Some("id-model".to_string()));
    }

    // --- find_file_in_index: absolute vs relative path mismatch ---

    #[test]
    fn find_file_in_index_direct_match_relative() {
        let mut index = HashMap::new();
        index.insert("src/lib.rs".to_string(), "id-lib".to_string());

        let result = find_file_in_index(&index, "src/lib.rs");
        assert_eq!(
            result,
            Some(("id-lib".to_string(), "src/lib.rs".to_string()))
        );
    }

    #[test]
    fn find_file_in_index_absolute_path_suffix_match() {
        // result.file_path is absolute, file_index keys are relative.
        let mut index = HashMap::new();
        index.insert("src/lib.rs".to_string(), "id-lib".to_string());

        let result = find_file_in_index(&index, "/home/dev/projects/CodeNexus/src/lib.rs");
        assert_eq!(
            result,
            Some(("id-lib".to_string(), "src/lib.rs".to_string()))
        );
    }

    #[test]
    fn find_file_in_index_suffix_boundary_check() {
        // Ensure "xsrc/lib.rs" does NOT match "/path/to/src/lib.rs"
        let mut index = HashMap::new();
        index.insert("xsrc/lib.rs".to_string(), "id-xlib".to_string());

        let result = find_file_in_index(&index, "/home/dev/projects/CodeNexus/src/lib.rs");
        assert_eq!(
            result, None,
            "xsrc/lib.rs should not suffix-match src/lib.rs"
        );
    }

    #[test]
    fn find_file_in_index_no_match_returns_none() {
        let mut index = HashMap::new();
        index.insert("src/other.rs".to_string(), "id-other".to_string());

        let result = find_file_in_index(&index, "/home/dev/projects/CodeNexus/src/lib.rs");
        assert_eq!(result, None);
    }

    #[test]
    fn find_file_in_index_multiple_suffix_matches_picks_longest() {
        // Determinism (Rule 5): when multiple keys suffix-match the same path,
        // pick the longest (most specific) to avoid HashMap order non-determinism.
        let mut index = HashMap::new();
        index.insert("index.ts".to_string(), "id-root".to_string());
        index.insert("src/index.ts".to_string(), "id-src".to_string());

        let result = find_file_in_index(&index, "/home/dev/proj/src/index.ts");
        assert_eq!(
            result,
            Some(("id-src".to_string(), "src/index.ts".to_string())),
            "longest suffix match should win for determinism"
        );
    }

    #[test]
    fn find_file_in_index_windows_backslash_boundary() {
        // Windows paths use `\` as separator; boundary check must accept it.
        let mut index = HashMap::new();
        index.insert("src/lib.rs".to_string(), "id-lib".to_string());

        let result = find_file_in_index(&index, r"C:\Users\dev\proj\src\lib.rs");
        assert_eq!(
            result,
            Some(("id-lib".to_string(), "src/lib.rs".to_string())),
            "Windows backslash separator should be accepted in boundary check"
        );
    }

    // --- resolve_imports: absolute importer path (production scenario) ---

    #[test]
    fn resolve_imports_handles_absolute_importer_path() {
        // Production scenario: result.file_path is absolute, but File nodes
        // in graph use relative paths. IMPORTS edge should still be created.
        let mut lib_result = make_result("/home/dev/projects/CodeNexus/src/lib.rs");
        lib_result.imports.push(crate::ir::ImportInfo {
            source_file: "crate::model".to_string(),
            imported_names: vec!["Node".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![lib_result];

        let mut graph = Graph::new();
        // File nodes use relative paths (as normalized by scope phase).
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        graph.add_node(make_file_node("src/model.rs", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "absolute importer path should resolve via suffix match"
        );
        assert_eq!(edges[0].target, "src/model.rs");
    }

    #[test]
    fn resolve_imports_absolute_path_with_relative_ts_import() {
        // TS import with absolute importer path: ./b.ts should still resolve.
        let mut a_result = make_result("/home/dev/projects/subno.ts/src/a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["foo".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/a.ts", "proj"));
        graph.add_node(make_file_node("src/b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "absolute importer + relative TS import should resolve"
        );
        assert_eq!(edges[0].source, "src/a.ts");
        assert_eq!(edges[0].target, "src/b.ts");
    }

    // --- TS ESM .js/.jsx extension stripping (NodeNext / bundler resolution) ---

    #[test]
    fn resolve_imports_strips_js_extension_for_ts_esm() {
        // TS NodeNext ESM requires `.js` in specifiers even for `.ts` files.
        // sdk/typescript/src/client.ts imports "./types/api.js" → types/api.ts
        let mut client_result = make_result("sdk/typescript/src/client.ts");
        client_result.imports.push(crate::ir::ImportInfo {
            source_file: "./types/api.js".to_string(),
            imported_names: vec!["ClientOptions".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![client_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("sdk/typescript/src/client.ts", "proj"));
        graph.add_node(make_file_node("sdk/typescript/src/types/api.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            ".js extension should be stripped, resolving to .ts file"
        );
        assert_eq!(edges[0].target, "sdk/typescript/src/types/api.ts");
    }

    #[test]
    fn resolve_imports_strips_jsx_extension() {
        // React JSX: ./Button.jsx → Button.tsx
        let mut app_result = make_result("src/app.tsx");
        app_result.imports.push(crate::ir::ImportInfo {
            source_file: "./Button.jsx".to_string(),
            imported_names: vec!["Button".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![app_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/app.tsx", "proj"));
        graph.add_node(make_file_node("src/Button.tsx", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            ".jsx extension should be stripped, resolving to .tsx file"
        );
        assert_eq!(edges[0].target, "src/Button.tsx");
    }

    #[test]
    fn resolve_imports_strips_mjs_cjs_extensions() {
        // .mjs / .cjs specifiers → .js target (common in Node ESM projects).
        let mut a_result = make_result("src/a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./config.mjs".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/a.ts", "proj"));
        graph.add_node(make_file_node("src/config.js", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            ".mjs extension should be stripped, resolving to .js file"
        );
        assert_eq!(edges[0].target, "src/config.js");
    }

    #[test]
    fn resolve_imports_strips_cjs_extension() {
        let mut a_result = make_result("src/a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./config.cjs".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/a.ts", "proj"));
        graph.add_node(make_file_node("src/config.js", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            ".cjs extension should be stripped, resolving to .js file"
        );
        assert_eq!(edges[0].target, "src/config.js");
    }

    // --- C++ #include: skipped by ImportResolver (scheme C, v0.3.0) ---
    // C++ #include is handled by ResolvePhase as EdgeType::Includes edges.
    // ImportResolver skips Language::Cpp results entirely — these tests
    // verify that NO IMPORTS edges are created for C++ #include directives,
    // regardless of whether a matching file exists.

    #[test]
    fn resolve_imports_skips_cpp_include_by_basename() {
        // C++ #include "format.h" — previously resolved via suffix matching
        // to IMPORTS edge. Scheme C: skipped entirely (INCLUDES edge built
        // by ResolvePhase instead).
        let mut std_result = make_result_cpp("include/fmt/std.h");
        std_result.imports.push(crate::ir::ImportInfo {
            source_file: "format.h".to_string(),
            imported_names: vec![],
            line: 11,
            is_reexport: false,
        });
        let results = vec![std_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("include/fmt/std.h", "proj"));
        graph.add_node(make_file_node("include/fmt/format.h", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "C++ #include should NOT produce IMPORTS edges (scheme C: handled by ResolvePhase)"
        );
    }

    #[test]
    fn resolve_imports_skips_cpp_include_by_partial_path() {
        // C++ #include "fmt/format.h" — previously resolved via partial path
        // suffix matching. Scheme C: skipped entirely.
        let mut top_result = make_result_cpp("src/main.cpp");
        top_result.imports.push(crate::ir::ImportInfo {
            source_file: "fmt/format.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![top_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj"));
        graph.add_node(make_file_node("include/fmt/format.h", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "C++ partial-path #include should NOT produce IMPORTS edges (scheme C)"
        );
    }

    #[test]
    fn resolve_imports_skips_cpp_system_include() {
        // C++ #include <iostream> — system header, no matching File node.
        // Scheme C: C++ is skipped entirely, so no IMPORTS edge regardless.
        let mut main_result = make_result_cpp("src/main.cpp");
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "iostream".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "C++ system header #include should NOT produce IMPORTS edges (scheme C)"
        );
    }

    #[test]
    fn resolve_imports_skips_cpp_include_even_when_target_exists() {
        // Scheme C regression guard: even when a matching file exists,
        // C++ #include must NOT produce an IMPORTS edge. The INCLUDES edge
        // is built separately by ResolvePhase::build_includes_edges.
        let mut main_result = make_result_cpp("src/main.cpp");
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "foo.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj"));
        graph.add_node(make_file_node("src/foo.h", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "C++ #include must NOT produce IMPORTS edges even when target file exists (scheme C)"
        );
    }

    // --- Java class import resolution (dotted package path) ---

    #[test]
    fn resolve_imports_resolves_java_class_import() {
        // Java import "com.google.gson.Gson" → com/google/gson/Gson.java
        // Dotted package path is mapped to a file path by replacing '.' with
        // '/' and appending '.java', then suffix-matched against file_index.
        let mut importer_result = make_result("src/com/google/gson/GsonBuilder.java");
        importer_result.imports.push(crate::ir::ImportInfo {
            source_file: "com.google.gson.Gson".to_string(),
            imported_names: vec!["Gson".to_string()],
            line: 3,
            is_reexport: false,
        });
        let results = vec![importer_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node(
            "src/com/google/gson/GsonBuilder.java",
            "proj",
        ));
        graph.add_node(make_file_node("src/com/google/gson/Gson.java", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "Java class import should resolve via dotted-path mapping"
        );
        assert_eq!(edges[0].source, "src/com/google/gson/GsonBuilder.java");
        assert_eq!(edges[0].target, "src/com/google/gson/Gson.java");
    }

    #[test]
    fn resolve_imports_resolves_java_class_in_maven_layout() {
        // Maven standard layout: src/main/java/com/google/gson/Gson.java
        // The Java-mapped path "com/google/gson/Gson.java" suffix-matches.
        let mut importer_result = make_result("gson/src/main/java/com/google/gson/Gson.java");
        importer_result.imports.push(crate::ir::ImportInfo {
            source_file: "com.google.gson.JsonElement".to_string(),
            imported_names: vec!["JsonElement".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![importer_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node(
            "gson/src/main/java/com/google/gson/Gson.java",
            "proj",
        ));
        graph.add_node(make_file_node(
            "gson/src/main/java/com/google/gson/JsonElement.java",
            "proj",
        ));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "Java class import should resolve in Maven standard layout"
        );
        assert_eq!(
            edges[0].target,
            "gson/src/main/java/com/google/gson/JsonElement.java"
        );
    }

    #[test]
    fn resolve_imports_skips_java_stdlib_import() {
        // java.util.List — JDK class, no matching File node in the project.
        let mut importer_result = make_result("src/Main.java");
        importer_result.imports.push(crate::ir::ImportInfo {
            source_file: "java.util.List".to_string(),
            imported_names: vec!["List".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![importer_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/Main.java", "proj"));
        // No java/util/List.java in the project.

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert!(
            edges.is_empty(),
            "JDK import (java.util.List) should not resolve to a local file"
        );
    }

    #[test]
    fn resolve_imports_resolves_java_static_import() {
        // import static com.google.gson.Gson.fromJson;
        // source_file = "com.google.gson.Gson.fromJson" — the last component
        // is a static member name, not a class. Strip it and map the parent.
        let mut importer_result = make_result("src/Test.java");
        importer_result.imports.push(crate::ir::ImportInfo {
            source_file: "com.google.gson.Gson.fromJson".to_string(),
            imported_names: vec!["fromJson".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![importer_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/Test.java", "proj"));
        graph.add_node(make_file_node("src/com/google/gson/Gson.java", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        assert_eq!(
            edges.len(),
            1,
            "Java static import should resolve by stripping the member name"
        );
        assert_eq!(edges[0].target, "src/com/google/gson/Gson.java");
    }

    // --- Coverage gap tests: build_file_index, strip_js_style_extension, resolve_java_class_import ---

    #[test]
    fn build_file_index_handles_node_without_file_path() {
        // File node with no file_path → only name is indexed (line 166 None branch).
        let mut graph = Graph::new();
        let node = Node::builder(NodeLabel::File, "name_only.ts", "name_only.ts")
            .id("name_only.ts")
            .project("proj")
            .language(Language::TypeScript)
            .build();
        graph.add_node(node);

        let index = build_file_index(&graph);
        assert_eq!(index.get("name_only.ts"), Some(&"name_only.ts".to_string()));
    }

    #[test]
    fn strip_js_style_extension_preserves_non_js_extensions() {
        assert_eq!(strip_js_style_extension("src/a.ts"), "src/a.ts");
        assert_eq!(strip_js_style_extension("src/a.rs"), "src/a.rs");
        assert_eq!(strip_js_style_extension("src/a.go"), "src/a.go");
    }

    #[test]
    fn resolve_java_class_import_with_slash_returns_none() {
        // Source with '/' is a file path, not a Java package → early None.
        let index = HashMap::new();
        assert_eq!(
            resolve_java_class_import("src/Main.java", "src/App.java", &index),
            None
        );
    }

    #[test]
    fn resolve_java_class_import_without_dot_returns_none() {
        // Source without '.' is not a Java package path → early None.
        let index = HashMap::new();
        assert_eq!(
            resolve_java_class_import("react", "src/App.java", &index),
            None
        );
    }

    #[test]
    fn resolve_include_suffix_prefers_same_directory() {
        // Two files both suffix-match "format.h": one in the same directory
        // as the importer, one in a different directory. Same-dir wins.
        let mut index = HashMap::new();
        index.insert("src/format.h".to_string(), "id-same".to_string());
        index.insert("include/fmt/format.h".to_string(), "id-other".to_string());

        let result = resolve_include_suffix("format.h", "src/main.cpp", &index);
        assert_eq!(result, Some("id-same".to_string()));
    }

    #[test]
    fn resolve_include_suffix_falls_back_to_other_directory() {
        // No same-directory match, only other-directory → same_dir.or(other)
        // returns the other match (line 347 `other` branch).
        let mut index = HashMap::new();
        index.insert("include/fmt/format.h".to_string(), "id-other".to_string());

        let result = resolve_include_suffix("format.h", "src/main.cpp", &index);
        assert_eq!(result, Some("id-other".to_string()));
    }

    #[test]
    fn resolve_include_suffix_returns_none_when_no_match() {
        // No file suffix-matches the include path → returns None.
        let mut index = HashMap::new();
        index.insert("src/utils.h".to_string(), "id-utils".to_string());

        let result = resolve_include_suffix("format.h", "src/main.cpp", &index);
        assert!(result.is_none());
    }

    #[test]
    fn resolve_include_suffix_exact_match_with_no_importer_directory() {
        // Exact match (prefix_len == 0) and importer_path has no '/' →
        // importer_dir = "", match_dir = "" → same_dir match.
        let mut index = HashMap::new();
        index.insert("format.h".to_string(), "id-exact".to_string());

        let result = resolve_include_suffix("format.h", "main.cpp", &index);
        assert_eq!(result, Some("id-exact".to_string()));
    }

    #[test]
    fn normalise_relative_dot_dot_from_root_pops_empty() {
        // `../b` from `a.ts` (root-level file): importer_dir is empty,
        // combined = "../b", segments.pop() on empty stack is a no-op.
        let n = normalise_relative("../b", "a.ts");
        assert_eq!(n, "b");
    }

    #[test]
    fn resolve_include_suffix_picks_shortest_other_dir() {
        // Multiple other-directory matches: shortest path wins (closest to root).
        let mut index = HashMap::new();
        index.insert("lib/fmt/format.h".to_string(), "id-short".to_string());
        index.insert("include/fmt/format.h".to_string(), "id-long".to_string());

        let result = resolve_include_suffix("format.h", "src/main.cpp", &index);
        assert_eq!(result, Some("id-short".to_string()));
    }

    #[test]
    fn strip_js_style_extension_strips_all_js_variants() {
        assert_eq!(strip_js_style_extension("src/a.js"), "src/a");
        assert_eq!(strip_js_style_extension("src/a.jsx"), "src/a");
        assert_eq!(strip_js_style_extension("src/a.mjs"), "src/a");
        assert_eq!(strip_js_style_extension("src/a.cjs"), "src/a");
    }

    // --- B7 review (arch-review HIGH-2): REEXPORTS edge unit tests ---

    /// Builds a Function node with the given id, name, and file_path.
    fn make_function_node(id: &str, name: &str, file_path: &str, project: &str) -> Node {
        Node::builder(NodeLabel::Function, name, format!("proj.{name}"))
            .id(id)
            .project(project)
            .file_path(file_path)
            .language(Language::TypeScript)
            .build()
    }

    /// B7 review: `pub use foo::bar` (is_reexport=true, imported_names=["bar"])
    /// creates exactly one File→Function REEXPORTS edge targeting `bar`'s
    /// Function node id. No REEXPORTS edge for non-reexport imports.
    #[test]
    fn pub_use_creates_reexports_edge_to_function() {
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 1,
            is_reexport: true,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));
        graph.add_node(make_function_node("fn-bar", "bar", "b.ts", "proj"));
        graph.add_node(make_function_node("fn-baz", "baz", "b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        // 1 IMPORTS edge (a.ts → b.ts) + 1 REEXPORTS edge (a.ts → fn-bar).
        let reexports: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Reexports)
            .collect();
        assert_eq!(reexports.len(), 1, "should create 1 REEXPORTS edge");
        assert_eq!(reexports[0].source, "a.ts");
        assert_eq!(
            reexports[0].target, "fn-bar",
            "should target bar's Function id"
        );
        assert!((reexports[0].confidence - 0.95).abs() < 1e-6);
        assert_eq!(reexports[0].confidence_tier, ConfidenceTier::ImportScoped);
    }

    /// B7 review: `pub use foo::*` (is_reexport=true, imported_names=[])
    /// creates one REEXPORTS edge per Function in the target file.
    #[test]
    fn wildcard_pub_use_creates_reexports_edges_to_all_functions() {
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec![], // wildcard
            line: 1,
            is_reexport: true,
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));
        graph.add_node(make_function_node("fn-bar", "bar", "b.ts", "proj"));
        graph.add_node(make_function_node("fn-baz", "baz", "b.ts", "proj"));
        graph.add_node(make_function_node("fn-qux", "qux", "b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        let reexports: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Reexports)
            .collect();
        assert_eq!(
            reexports.len(),
            3,
            "wildcard should create 3 REEXPORTS edges (one per function)"
        );
        let targets: std::collections::HashSet<&str> =
            reexports.iter().map(|e| e.target.as_str()).collect();
        assert!(targets.contains("fn-bar"));
        assert!(targets.contains("fn-baz"));
        assert!(targets.contains("fn-qux"));
    }

    /// B7 review: ordinary `use foo::bar` (is_reexport=false) does NOT
    /// create any REEXPORTS edge — only IMPORTS.
    #[test]
    fn plain_use_does_not_create_reexports_edge() {
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 1,
            is_reexport: false, // ordinary import
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));
        graph.add_node(make_function_node("fn-bar", "bar", "b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        let reexports: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Reexports)
            .collect();
        assert!(
            reexports.is_empty(),
            "plain use must not create REEXPORTS edges"
        );
        assert_eq!(edges.len(), 1, "should still create 1 IMPORTS edge");
    }

    /// B7 review: duplicate `pub use foo::bar` (same source file, same target
    /// function) creates only ONE REEXPORTS edge (dedup via seen_reexport_pairs).
    #[test]
    fn duplicate_pub_use_dedups_reexports_edges() {
        let mut a_result = make_result("a.ts");
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 1,
            is_reexport: true,
        });
        a_result.imports.push(crate::ir::ImportInfo {
            source_file: "./b.ts".to_string(),
            imported_names: vec!["bar".to_string()],
            line: 2,
            is_reexport: true, // duplicate re-export of bar
        });
        let results = vec![a_result];

        let mut graph = Graph::new();
        graph.add_node(make_file_node("a.ts", "proj"));
        graph.add_node(make_file_node("b.ts", "proj"));
        graph.add_node(make_function_node("fn-bar", "bar", "b.ts", "proj"));

        let resolver = ImportResolver::new("proj");
        let edges = resolver.resolve_imports(&results, &mut graph);

        let reexports: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Reexports)
            .collect();
        assert_eq!(
            reexports.len(),
            1,
            "duplicate pub use should dedup to 1 REEXPORTS edge"
        );
    }

    /// B7 review (perf-review MEDIUM-1+MEDIUM-3 + arch-review MEDIUM-1):
    /// `build_function_index` returns a nested `HashMap<file_id, HashMap<name, func_id>>`.
    /// Direct unit test: absolute file_path resolves to relative file_index
    /// key via `find_best_suffix_match`, and the function is indexed under
    /// the resolved file_id.
    #[test]
    fn build_function_index_handles_absolute_and_relative_paths() {
        let mut graph = Graph::new();
        // File node uses relative path (production scope phase output).
        graph.add_node(make_file_node("src/lib.rs", "proj"));
        // Function node uses absolute path (production extractor output).
        graph.add_node(make_function_node(
            "fn-bar",
            "bar",
            "/home/dev/proj/src/lib.rs",
            "proj",
        ));

        let file_index = build_file_index(&graph);
        let func_index = build_function_index(&graph, &file_index);

        // Function should be indexed under the File node's id ("src/lib.rs"),
        // not under the absolute path.
        let lib_id = file_index
            .get("src/lib.rs")
            .expect("file_index has src/lib.rs");
        let by_file = func_index
            .get(lib_id)
            .expect("func_index has the lib file entry");
        assert_eq!(by_file.get("bar"), Some(&"fn-bar".to_string()));
    }

    /// B7 review: `resolve_reexport_targets` with empty `imported_names`
    /// (wildcard) returns all Function ids under `target_file_id`. With
    /// non-empty `imported_names`, returns only the named Function ids.
    #[test]
    fn resolve_reexport_targets_wildcard_and_named() {
        let mut func_index: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut by_file = HashMap::new();
        by_file.insert("bar".to_string(), "fn-bar".to_string());
        by_file.insert("baz".to_string(), "fn-baz".to_string());
        by_file.insert("qux".to_string(), "fn-qux".to_string());
        func_index.insert("file-b".to_string(), by_file);

        // Wildcard: empty imported_names → all 3 functions.
        let wildcard = resolve_reexport_targets("file-b", &[], &func_index);
        assert_eq!(wildcard.len(), 3, "wildcard should return all 3 functions");

        // Named: only bar and qux.
        let named = resolve_reexport_targets(
            "file-b",
            &["bar".to_string(), "qux".to_string()],
            &func_index,
        );
        assert_eq!(named.len(), 2, "named should return 2 functions");
        assert!(named.contains(&"fn-bar".to_string()));
        assert!(named.contains(&"fn-qux".to_string()));

        // Non-existent file → empty.
        let missing = resolve_reexport_targets("file-x", &[], &func_index);
        assert!(missing.is_empty(), "non-existent file should return empty");

        // Non-existent name → empty.
        let missing_name = resolve_reexport_targets("file-b", &["nope".to_string()], &func_index);
        assert!(
            missing_name.is_empty(),
            "non-existent name should return empty"
        );
    }

    /// B7 review (arch-review MEDIUM-1): `find_best_suffix_match` is the
    /// shared helper used by both `find_file_in_index` and
    /// `build_function_index`. Direct unit test verifies the longest-match
    /// determinism and boundary check.
    #[test]
    fn find_best_suffix_match_picks_longest_with_boundary() {
        let mut index = HashMap::new();
        index.insert("index.ts".to_string(), "id-root".to_string());
        index.insert("src/index.ts".to_string(), "id-src".to_string());

        // Longest match wins (determinism — Rule 5).
        let best = find_best_suffix_match(&index, "/home/dev/proj/src/index.ts");
        assert_eq!(best.map(|(_, id)| id.as_str()), Some("id-src"));

        // Boundary check: "xsrc/lib.rs" must NOT suffix-match "src/lib.rs".
        let mut index2 = HashMap::new();
        index2.insert("xsrc/lib.rs".to_string(), "id-xlib".to_string());
        let best2 = find_best_suffix_match(&index2, "/home/dev/proj/src/lib.rs");
        assert!(
            best2.is_none(),
            "xsrc/lib.rs must not boundary-match src/lib.rs"
        );
    }
}
