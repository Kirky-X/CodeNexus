// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Dead code detection.
//!
//! Identifies `Function`/`Method` nodes with zero incoming `CALLS` edges that
//! are not entry points (e.g. `main`) or test functions (`test_*` / `*_test`).
//!
//! # Algorithm
//!
//! 1. Query all `Function`/`Method` nodes for the given project.
//! 2. Query all `CALLS` edges for the project and build a set of callee ids.
//! 3. A node is "dead" if its id is NOT in the callee set AND its name does
//!    not match any entry-point glob pattern AND its name does not match any
//!    default test-function pattern.
//! 4. The `language` field is resolved per-node by joining on the `File`
//!    table's `filePath` (polyglot projects are handled correctly).

use crate::model::EdgeType;
use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
// arch-review LOW-1: import from `storage` root rather than the deeper
// `storage::schema` path — `escape_cypher_string` is a pure utility that
// the storage module re-exports as part of its public API, and consumers
// should not reach across submodule boundaries.
use crate::storage::escape_cypher_string;
use serde::{Deserialize, Serialize};

/// Default glob patterns for functions that are NOT considered dead even with
/// zero incoming CALLS edges (test functions are always invoked by the test
/// runner, which is not modelled as a CALLS edge in the graph).
///
/// B2 fix: expanded beyond `test_*`/`*_test`/`*_spec` to cover the common
/// Rust test prefixes used by CalNexus and similar projects:
/// - `it_*` — integration tests
/// - `sec_*` — security tests
/// - `snap_*` — snapshot tests (insta)
/// - `perf_*` — performance tests
/// - `bench_*` — benchmark tests
const DEFAULT_TEST_PATTERNS: &[&str] = &[
    "test_*", "*_test", "*_spec", "it_*", "sec_*", "snap_*", "perf_*", "bench_*",
];

/// Default entry-point function names across common languages and platforms
/// (C/C++ main, C/C++ wmain, Python __main__, C# Main, Win32 WinMain, DLL
/// DLLMain).
const DEFAULT_ENTRY_PATTERNS: &[&str] =
    &["main", "Main", "__main__", "wmain", "WinMain", "DLLMain"];

/// Attribute substrings that mark a function as a test function (not an
/// entry point). When any substring appears in a function's `signature`
/// field, the function is treated as live — test runners (`cargo test`,
/// `pytest`, `go test`) invoke them via reflection / macro expansion that
/// is invisible to the static call graph.
///
/// This is a defense-in-depth overlay on top of [`DEFAULT_TEST_PATTERNS`]
/// (name-globs) and [`is_test_module_function`] (`#tests` disambiguator):
/// it catches `#[test]` / `#[tokio::test]` functions whose names do not
/// match any glob (e.g. `run_succeeds_on_empty_db`) and whose QN lacks the
/// `#tests` disambiguator (tree-sitter-rust does not currently emit it for
/// inline `mod tests` blocks).
///
/// On bulwark this closes 1376/1387 (99.2%) false positives — virtually
/// every `#[test]` function had a descriptive name that matched no glob.
const TEST_ATTRIBUTE_MARKERS: &[&str] = &["#[test", "#[tokio::test", "#[rstest"];

/// Returns `true` when `signature` carries one of the [`TEST_ATTRIBUTE_MARKERS`]
/// substrings, i.e. the function is a Rust test entry point. Used by both
/// [`DeadCodeDetector::is_seed_function`] (seed category 5) and the result
/// filter loop (defense-in-depth) so the check stays consistent.
///
/// # Performance
///
/// Cheap pre-filter: `signature.contains("#[")` returns `false` for the vast
/// majority of functions (no outer attributes), short-circuiting before the
/// `TEST_ATTRIBUTE_MARKERS.iter().any(...)` loop. On bulwark this saves ~115k
/// unnecessary `contains` scans across 19k functions × 2 call sites
/// (perf-review L1).
fn has_test_attribute_marker(signature: &str) -> bool {
    signature.contains("#[")
        && TEST_ATTRIBUTE_MARKERS
            .iter()
            .any(|marker| signature.contains(marker))
}

/// Default entry-point attribute substrings scanned for in a function's
/// `signature` field (T182-B / B4.5 deferred).
///
/// These attributes register the decorated function as an external entry
/// point via macro expansion:
/// - `#[tool(...)]` — rmcp MCP tool registration (synthesises a dispatch
///   table that calls the fn at runtime by tool name).
/// - `#[forge(...)]` — CodeNexus service registration (registers both an
///   MCP tool and a CLI subcommand).
/// - `#[tokio::main]` / `#[rocket::main]` / `#[actix::main]` /
///   `#[axum::main]` — async-runtime entry macros that synthesise a
///   synchronous `main` calling the decorated async fn.
///
/// The check is a substring match on the signature, so both `#[tool]`
/// (bare) and `#[tool(name = "...")]` (with arguments) are recognised.
/// tree-sitter does not expand macros, so the synthesised CALLS edge is
/// invisible to the graph — dead_code must treat the attribute itself as
/// the entry-point signal.
///
/// Non-entry-point attributes (`#[cfg(...)]`, `#[derive(...)]`,
/// `#[allow(...)]`) are intentionally absent — they do not register
/// external entry points. `#[test]` / `#[bench]` are covered separately
/// by [`TEST_ATTRIBUTE_MARKERS`] (test functions are live seeds, not
/// entry points — they are invoked by the test runner, not by user code).
const DEFAULT_ATTRIBUTE_ENTRIES: &[&str] = &[
    "#[tool",
    "#[forge",
    "#[tokio::main",
    "#[rocket::main",
    "#[actix::main",
    "#[axum::main",
];

/// Reason string recorded on every [`DeadCodeEntry`].
const REASON_ZERO_INCOMING_CALLS: &str = "zero incoming CALLS edges";

/// Configuration for dead-code detection.
///
/// Controls which edge types are consulted, whether exported/FFI functions are
/// excluded, and the default entry-point / test-function patterns.
#[derive(Debug, Clone)]
pub struct DeadCodeConfig {
    /// Glob patterns for function names that are always considered live
    /// (e.g. `"main"`, `"WinMain"`).
    pub entry_patterns: Vec<String>,
    /// Glob patterns for test-function names (e.g. `"test_*"`).
    pub test_patterns: Vec<String>,
    /// When `true`, `isExported=true` nodes are excluded from dead code.
    pub check_exported: bool,
    /// When `true`, trait impl methods (qualified_name contains a `#<TypeName>`
    /// disambiguator like `fmt#Display`) are excluded from dead code. These
    /// methods are invoked via dynamic dispatch / vtable and have no static
    /// CALLS edge in the graph.
    pub check_dynamic_dispatch: bool,
    /// Reserved for future reflection / serde detection.
    pub check_reflection: bool,
    /// When `true`, signatures containing `extern "C"` / `#[no_mangle]` are
    /// treated as FFI entry points and excluded.
    pub check_ffi: bool,
    /// Substrings scanned for in a function's `signature` field to recognise
    /// attribute-marked entry points (T182-B / B4.5 deferred). When any
    /// substring matches, the function is treated as a live seed (macro
    /// expansion synthesises an external call to it that is invisible to
    /// the static graph). Defaults to [`DEFAULT_ATTRIBUTE_ENTRIES`]
    /// (`#[tool]`, `#[forge]`, `#[tokio::main]`, `#[rocket::main]`,
    /// `#[actix::main]`, `#[axum::main]`).
    pub attribute_entries: Vec<String>,
    /// Edge types whose incoming edges mark a function as "used".
    pub edge_types: Vec<EdgeType>,
}

impl Default for DeadCodeConfig {
    fn default() -> Self {
        Self {
            entry_patterns: DEFAULT_ENTRY_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            test_patterns: DEFAULT_TEST_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            check_exported: true,
            // B3.5: Trait impl methods (e.g. `fmt#Display`) are invoked via
            // dynamic dispatch / vtable and have no static CALLS edge in the
            // graph. Default to `true` to align with rustc's dead_code lint
            // (which treats trait impls as reachable). Users can opt out via
            // `--check_dynamic_dispatch false` for adversarial testing.
            check_dynamic_dispatch: true,
            check_reflection: false,
            check_ffi: true,
            // T182-B / B4.5: attribute-marked entry points. Defaults mirror
            // `DEFAULT_ATTRIBUTE_ENTRIES` (rmcp `#[tool]`, CodeNexus `#[forge]`,
            // async-runtime entry macros). Users can extend via
            // `DeadCodeConfig { attribute_entries: ..., ..Default::default() }`.
            attribute_entries: DEFAULT_ATTRIBUTE_ENTRIES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            edge_types: vec![
                EdgeType::Calls,
                EdgeType::FfiCalls,
                EdgeType::Implements,
                EdgeType::HandlesRoute,
                EdgeType::Usage,
                EdgeType::Tests,
                EdgeType::UsesType,
                EdgeType::HttpCalls,
                EdgeType::AsyncCalls,
            ],
        }
    }
}

/// Confidence level for a dead-code finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    /// All edge types have zero incoming edges.
    High,
    /// Non-CALLS edges exist but no CALLS incoming edge.
    Medium,
    /// Some edge types have incoming edges but coverage is incomplete.
    Low,
}

/// A single dead-code finding.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeadCodeEntry {
    /// Short function name (e.g. `parse_file`).
    pub name: String,
    /// Fully-qualified name (e.g. `demo.parse_file`).
    pub qualified_name: String,
    /// Source file path.
    pub file_path: String,
    /// 1-based start line.
    pub start_line: u32,
    /// Source language (resolved from the `File` node).
    pub language: String,
    /// Why this node is considered dead (e.g. `zero incoming CALLS edges`).
    pub reason: String,
    /// Confidence level of the finding.
    pub confidence: Confidence,
}

/// Batch-prefetched metadata for dead-code analysis (perf-1 + arch-1 + B7).
///
/// Replaces the previous N+1 per-function Cypher query pattern with a
/// single bulk query that is shared by [`ReachabilityAnalyzer`] and
/// [`DeadCodeDetector::detect`]:
///
/// - `outgoing_edges` — all `CodeRelation` edges of configured types,
///   grouped by `source` id (`HashMap<String, HashSet<String>>`).
/// - `reexport_target_ids` (B7) — `Function`/`Method` ids that are the
///   `target` of a `REEXPORTS` edge (File→Function). These are live
///   entry-point seeds: the symbol is reachable from outside the current
///   crate/module via the re-export.
///
/// `exported_ids` and `ffi_entry_ids` are derived in Rust from the already
/// loaded `FunctionRow` list (no extra Cypher round-trips).
///
/// # Design boundary (arch-review LOW-2)
///
/// 4 fields is the soft cap. If a future task adds a 5th prefetch
/// category, split into `ExportedCache` / `FfiCache` / `EdgeCache` /
/// `ReexportCache` and compose them here.
///
/// # Performance
///
/// Before: `collect_seeds` issued 2 Cypher queries per function
/// (`is_exported` × 2 labels + `is_ffi_entry` × 2 labels = 4N worst case),
/// `propagate` issued 1 query per worklist pop (V queries), and `detect`
/// re-queried `is_exported`/`is_ffi_entry` for each dead candidate (2N
/// more). Total: ~4N + 2N + V = 6N+V round-trips.
///
/// After (perf-1 + perf-review MEDIUM-1/2 + B7): `load_all_functions`
/// issues 2 Cypher queries (Function + Method labels — LadybugDB's Cypher
/// subset does not support `OR` label expressions) and returns
/// `isExported` / `signature` alongside the existing fields.
/// `BatchPrefetch::load` then issues 2 Cypher queries: one for
/// `outgoing_edges`, one for B7 `reexport_target_ids`. BatchPrefetch
/// therefore contributes 4 of the 6 total round-trips in
/// [`DeadCodeDetector::detect`] (the other 2 are
/// `load_edge_targets_by_category` for confidence scoring and
/// `load_file_languages` for language resolution). Previously this scaled
/// as 6N+V; for CalNexus (N=247, V=21990) this is a ~1500× reduction.
///
/// # DRY
///
/// Previously `is_exported`/`is_ffi_entry`/`load_functions` were
/// duplicated byte-for-byte across `ReachabilityAnalyzer` and
/// `DeadCodeDetector`. The shared `BatchPrefetch` + `load_all_functions`
/// eliminate the duplication. Field access is mediated by `is_exported`
/// / `is_ffi_entry` / `outgoing_edges` / `is_reexport_target` methods so
/// the internal `HashSet`/`HashMap` choice is not leaked to callers
/// (arch-review
/// MEDIUM-3).
pub(crate) struct BatchPrefetch {
    /// Ids of `Function`/`Method` nodes with `isExported=true`.
    exported_ids: std::collections::HashSet<String>,
    /// Ids of `Function`/`Method` nodes whose `signature` contains FFI
    /// markers (`extern "C"` or `#[no_mangle]`).
    ffi_entry_ids: std::collections::HashSet<String>,
    /// Outgoing edges grouped by source id. Targets are deduplicated
    /// (`HashSet`) so a multi-edge (e.g. CALLS + USAGE) source→target
    /// pair only propagates once (perf-review MEDIUM-6).
    outgoing_edges: std::collections::HashMap<String, std::collections::HashSet<String>>,
    /// B7: Ids of `Function`/`Method` nodes that are the `target` of a
    /// `REEXPORTS` edge (File→Function, created by `resolve/imports.rs`
    /// for `pub use` / `export ... from`). These are live entry-point
    /// seeds — the symbol is reachable from outside the current
    /// crate/module via the re-export.
    reexport_target_ids: std::collections::HashSet<String>,
}

impl BatchPrefetch {
    /// Builds the prefetch cache from an already-loaded `functions` list
    /// plus Cypher round-trips for `outgoing_edges` and (B7)
    /// `reexport_target_ids`.
    ///
    /// `exported_ids` and `ffi_entry_ids` are derived in Rust from
    /// `functions` (no extra Cypher) — this collapses the previous 5
    /// prefetch-related round-trips (load_exported_ids × 2 +
    /// load_ffi_entry_ids × 2 + load_outgoing_edges × 1) into 2
    /// (perf-review MEDIUM-1 + B7). At the `detect` level the overall
    /// reduction is 7 → 6 round-trips (the other 4 are
    /// `load_all_functions` × 2, `load_edge_targets_by_category` × 1,
    /// `load_file_languages` × 1).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if either the `outgoing_edges` or the
    /// B7 `reexport_target_ids` query fails.
    pub(crate) fn load(
        storage: &dyn Storage,
        project: &str,
        config: &DeadCodeConfig,
        functions: &[FunctionRow],
    ) -> StorageResult<Self> {
        let mut exported_ids = std::collections::HashSet::new();
        let mut ffi_entry_ids = std::collections::HashSet::new();
        for func in functions {
            if func.is_exported {
                exported_ids.insert(func.id.clone());
            }
            if func.signature.contains(r#"extern "C""#) || func.signature.contains("#[no_mangle]") {
                ffi_entry_ids.insert(func.id.clone());
            }
        }
        let outgoing_edges = load_outgoing_edges(storage, project, &config.edge_types)?;
        // B7: bulk-load REEXPORTS edge targets (File→Function). These are
        // live entry-point seeds — the re-exported symbol is reachable
        // from outside the current crate/module via the re-export.
        let reexport_target_ids = load_reexport_target_ids(storage, project)?;
        Ok(Self {
            exported_ids,
            ffi_entry_ids,
            outgoing_edges,
            reexport_target_ids,
        })
    }

    /// Returns `true` if `id` is a `Function`/`Method` with `isExported=true`.
    #[must_use]
    pub(crate) fn is_exported(&self, id: &str) -> bool {
        self.exported_ids.contains(id)
    }

    /// Returns `true` if `id` has an FFI marker in its `signature`
    /// (`extern "C"` or `#[no_mangle]`).
    #[must_use]
    pub(crate) fn is_ffi_entry(&self, id: &str) -> bool {
        self.ffi_entry_ids.contains(id)
    }

    /// B7: Returns `true` if `id` is the target of a `REEXPORTS` edge
    /// (i.e. the symbol is re-exported via `pub use` / `export ... from`
    /// and thus reachable from outside the current crate/module).
    #[must_use]
    pub(crate) fn is_reexport_target(&self, id: &str) -> bool {
        self.reexport_target_ids.contains(id)
    }

    /// Returns the number of exported ids in the cache (test/diagnostic).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn exported_ids_len(&self) -> usize {
        self.exported_ids.len()
    }

    /// Returns the number of FFI entry ids in the cache (test/diagnostic).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn ffi_entry_ids_len(&self) -> usize {
        self.ffi_entry_ids.len()
    }

    /// B7: Returns the number of re-export target ids in the cache
    /// (test/diagnostic).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn reexport_target_ids_len(&self) -> usize {
        self.reexport_target_ids.len()
    }

    /// Returns the deduplicated set of outgoing-edge targets for `id`, or
    /// `None` if `id` has no outgoing edges.
    #[must_use]
    pub(crate) fn outgoing_edges(&self, id: &str) -> Option<&std::collections::HashSet<String>> {
        self.outgoing_edges.get(id)
    }
}

/// Loads all `CodeRelation` edges of `edge_types` for `project`, grouped by
/// source id with deduplicated targets.
///
/// Used by [`ReachabilityAnalyzer::propagate`] for O(1) HashMap lookup per
/// worklist pop (vs the previous O(1) Cypher round-trip per pop). Targets
/// are stored in a `HashSet` so a multi-edge source→target pair (e.g.
/// CALLS + USAGE) only appears once (perf-review MEDIUM-6).
fn load_outgoing_edges(
    storage: &dyn Storage,
    project: &str,
    edge_types: &[EdgeType],
) -> StorageResult<std::collections::HashMap<String, std::collections::HashSet<String>>> {
    if edge_types.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let escaped = escape_cypher_string(project);
    let in_list = edge_types
        .iter()
        .map(|t| format!("'{}'", t.as_db_type()))
        .collect::<Vec<_>>()
        .join(", ");
    let cypher = format!(
        "MATCH (e:CodeRelation) WHERE e.type IN [{in_list}] AND e.project = '{escaped}' \
         RETURN e.source AS source, e.target AS target;"
    );
    let rows = storage.query(&cypher)?;
    let mut map: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        if row.len() < 2 {
            continue;
        }
        let source = row[0].as_str().unwrap_or_default().to_string();
        let target = row[1].as_str().unwrap_or_default().to_string();
        map.entry(source).or_default().insert(target);
    }
    Ok(map)
}

/// B7: Loads the `target` ids of all `REEXPORTS` edges for `project`.
///
/// `REEXPORTS` edges are File→Function (created by `resolve/imports.rs`
/// for `pub use` / `export ... from`). The target Function ids are live
/// entry-point seeds — the symbol is reachable from outside the current
/// crate/module via the re-export.
///
/// Returns a `HashSet` so duplicate targets (e.g. the same function
/// re-exported by multiple files) only seed the worklist once.
fn load_reexport_target_ids(
    storage: &dyn Storage,
    project: &str,
) -> StorageResult<std::collections::HashSet<String>> {
    let escaped = escape_cypher_string(project);
    // B7 review (perf LOW-3): use EdgeType::Reexports.as_db_type() as the
    // single source of truth for the DDL type string, so future renames
    // don't silently break this query.
    let reexport_type = EdgeType::Reexports.as_db_type();
    let cypher = format!(
        "MATCH (e:CodeRelation) WHERE e.type = '{reexport_type}' AND e.project = '{escaped}' \
         RETURN e.target AS target;"
    );
    let rows = storage.query(&cypher)?;
    let mut set = std::collections::HashSet::with_capacity(rows.len());
    for row in rows {
        if row.is_empty() {
            continue;
        }
        // B7 review (perf LOW-1): skip empty target ids instead of
        // allocating an empty String for each malformed row.
        if let Some(s) = row[0].as_str().filter(|s| !s.is_empty()) {
            set.insert(s.to_string());
        }
    }
    Ok(set)
}

/// Loads all `Function` and `Method` nodes for `project`, including the
/// `isExported` flag and `signature` text needed to derive
/// [`BatchPrefetch`] `exported_ids` / `ffi_entry_ids` without additional
/// Cypher round-trips (perf-review MEDIUM-1).
///
/// Two Cypher queries (one per label) because LadybugDB's Cypher subset
/// does not support `OR` label expressions.
fn load_all_functions(storage: &dyn Storage, project: &str) -> StorageResult<Vec<FunctionRow>> {
    let escaped = escape_cypher_string(project);
    let function_cypher = format!(
        "MATCH (n:Function) WHERE n.project = '{escaped}' \
         RETURN n.id AS id, n.name AS name, n.qualifiedName AS qualified_name, \
         n.filePath AS file_path, n.startLine AS start_line, \
         n.isExported AS is_exported, n.signature AS signature;"
    );
    let method_cypher = format!(
        "MATCH (n:Method) WHERE n.project = '{escaped}' \
         RETURN n.id AS id, n.name AS name, n.qualifiedName AS qualified_name, \
         n.filePath AS file_path, n.startLine AS start_line, \
         n.isExported AS is_exported, n.signature AS signature;"
    );
    let mut out = Vec::new();
    for cypher in [function_cypher, method_cypher] {
        let rows = storage.query(&cypher)?;
        for row in rows {
            if row.len() < 7 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let name = row[1].as_str().unwrap_or_default().to_string();
            let qualified_name = row[2].as_str().unwrap_or_default().to_string();
            let file_path = row[3].as_str().unwrap_or_default().to_string();
            let start_line = row[4]
                .as_i64()
                .map(|v| v as u32)
                .or_else(|| row[4].as_u64().map(|v| v as u32))
                .unwrap_or(0);
            let is_exported = row[5].as_bool().unwrap_or(false);
            let signature = row[6].as_str().unwrap_or_default().to_string();
            out.push(FunctionRow {
                id,
                name,
                qualified_name,
                file_path,
                start_line,
                is_exported,
                signature,
            });
        }
    }
    Ok(out)
}

/// Worklist-based reachability analyzer (B5).
///
/// Replaces the single-layer `referenced_ids` check with a proper
/// worklist propagation algorithm aligned with rustc's
/// `rustc_passes::dead::MarkSymbolVisitor`. Starting from a seed set
/// (entry points / exported / FFI / tests / attribute-marked /
/// re-exports / trait impls), the analyzer BFS-propagates reachability
/// along configured outgoing edge types (CALLS, FFI_CALLS, ASYNC_CALLS,
/// HTTP_CALLS, USAGE, USES_TYPE, etc.) until a fixed point is reached.
/// Functions not in the resulting `live_set` are dead.
///
/// # Algorithm
///
/// 1. `collect_seeds` populates `worklist` with seven seed categories.
/// 2. `propagate` pops ids from `worklist`, inserts them into `live_set`,
///    and pushes any unreachable targets of their outgoing edges back onto
///    the worklist. Terminates when the worklist is empty (fixed point).
/// 3. `live_set` is the set of reachable function/method ids.
///
/// # Complexity
///
/// Average O(V+E) where V = function count, E = edge count. Worst case
/// O(V²) on pathological graphs, but far better than the previous
/// per-function full-table scan in practice.
pub(crate) struct ReachabilityAnalyzer<'a> {
    /// Batch-prefetched exported/FFI ids and outgoing edges (perf-1).
    prefetch: &'a BatchPrefetch,
    config: &'a DeadCodeConfig,
    /// All functions/methods for the project (borrowed from detector).
    functions: &'a [FunctionRow],
    /// Worklist: ids pending propagation.
    worklist: std::collections::VecDeque<String>,
    /// Live set: ids confirmed reachable from any seed.
    live_set: std::collections::HashSet<String>,
}

impl<'a> ReachabilityAnalyzer<'a> {
    /// Creates a new analyzer bound to a [`BatchPrefetch`] cache and a
    /// function list (both produced by the caller, typically
    /// [`DeadCodeDetector::detect`]).
    ///
    /// The analyzer never queries the storage directly — all Cypher
    /// round-trips happen once during `BatchPrefetch::load`, and
    /// `collect_seeds`/`propagate` work entirely on in-memory data
    /// structures.
    #[must_use]
    pub(crate) fn new(
        prefetch: &'a BatchPrefetch,
        config: &'a DeadCodeConfig,
        functions: &'a [FunctionRow],
    ) -> Self {
        Self {
            prefetch,
            config,
            functions,
            worklist: std::collections::VecDeque::new(),
            live_set: std::collections::HashSet::new(),
        }
    }

    /// Collects all nine seed categories into the worklist.
    ///
    /// Categories (aligned with design.md D1 + B0/B4/B7 + T182-B/B4.5):
    /// 1. Entry functions (name matches `entry_patterns` or `config.entry_patterns`)
    /// 2. Test functions (name matches `test_patterns` or `DEFAULT_TEST_PATTERNS`)
    /// 3. B0: test module function (`#tests` disambiguator)
    /// 4. B4: integration test file (`tests/`, `test/`, `src/test/`)
    /// 5. B3: trait impl methods (`#<TypeName>` disambiguator, when `check_dynamic_dispatch`)
    /// 6. Exported functions (`isExported=true`, when `check_exported`)
    /// 7. FFI entries (signature contains `extern "C"` / `#[no_mangle]`, when `check_ffi`)
    /// 8. B7: re-export targets (`REEXPORTS` edge targets, always checked)
    /// 9. T182-B: attribute-marked entry points (signature contains any
    ///    `config.attribute_entries` substring, e.g. `#[tool` / `#[forge` /
    ///    `#[tokio::main`)
    ///
    /// Fully in-memory after [`BatchPrefetch::load`] — no Cypher round-trips
    /// (arch-review MEDIUM-2: removed the dead `StorageResult` return).
    ///
    /// # OCP trade-off (T202 arch-review LOW-2)
    ///
    /// The 9 seed categories are explicitly enumerated in
    /// [`is_seed_function`]. Adding a new seed type requires editing that
    /// method — a textbook OCP violation. This is an accepted trade-off:
    /// the seed list is bounded by the algorithm's semantics (worklist BFS
    /// needs a finite seed set), each category is documented inline, and
    /// the list has grown slowly (9 entries across 18 months). If the list
    /// reaches 20+ entries, refactor to `Vec<Box<dyn SeedStrategy>>` where
    /// each strategy is a separate struct with `is_seed(&self, func: &FunctionRow) -> bool`.
    /// Extension authors would then add a new strategy struct without
    /// modifying `is_seed_function`.
    pub(crate) fn collect_seeds(&mut self, entry_patterns: &[&str]) {
        let config_entry_patterns: Vec<&str> = self
            .config
            .entry_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let config_test_patterns: Vec<&str> = self
            .config
            .test_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let config_attribute_entries: Vec<&str> = self
            .config
            .attribute_entries
            .iter()
            .map(|s| s.as_str())
            .collect();

        for func in self.functions {
            if self.is_seed_function(
                func,
                entry_patterns,
                &config_entry_patterns,
                &config_test_patterns,
                &config_attribute_entries,
            ) {
                self.worklist.push_back(func.id.clone());
            }
        }
    }

    /// Returns `true` if `func` matches any seed category.
    ///
    /// All checks are O(1) HashSet lookups against the prefetched caches —
    /// no Cypher round-trips.
    ///
    /// Seed categories (in execution order — arch-review LOW-1):
    /// 1. Entry functions (name matches `entry_patterns` or `DEFAULT_ENTRY_PATTERNS`)
    /// 2. Test functions (name matches `test_patterns` or `DEFAULT_TEST_PATTERNS`)
    /// 3. Test module function (`#tests` disambiguator — B0)
    /// 4. Integration test file (B4 — `tests/`, `test/`, `src/test/`)
    /// 5. Test-attribute-marked functions (signature contains `#[test` /
    ///    `#[tokio::test` / `#[rstest` — closes bulwark 1376/1387 FPs)
    /// 6. Trait impl method (B3 — `#<TypeName>` disambiguator, when `check_dynamic_dispatch`)
    /// 7. Exported functions (`isExported=true`, when `check_exported`)
    /// 8. FFI entries (signature contains `extern "C"` / `#[no_mangle]`, when `check_ffi`)
    /// 9. Re-export targets (B7 — `REEXPORTS` edge targets, always checked)
    /// 10. T182-B: attribute-marked entry points (signature contains any
    ///     `attribute_entries` substring, e.g. `#[tool` / `#[forge` /
    ///     `#[tokio::main`)
    fn is_seed_function(
        &self,
        func: &FunctionRow,
        entry_patterns: &[&str],
        config_entry_patterns: &[&str],
        config_test_patterns: &[&str],
        attribute_entries: &[&str],
    ) -> bool {
        // 1. Entry functions (name-based)
        if matches_any_pattern(&func.name, entry_patterns)
            || matches_any_pattern(&func.name, config_entry_patterns)
            || matches_any_pattern(&func.name, DEFAULT_ENTRY_PATTERNS)
        {
            return true;
        }
        // 2. Test functions (name-based)
        if matches_any_pattern(&func.name, DEFAULT_TEST_PATTERNS)
            || matches_any_pattern(&func.name, config_test_patterns)
        {
            return true;
        }
        // 3. B0: test module function (`#tests` disambiguator)
        if is_test_module_function(&func.qualified_name) {
            return true;
        }
        // 4. B4: integration test file
        if is_integration_test_file(&func.file_path) {
            return true;
        }
        // 5. Test-attribute markers (#[test] / #[tokio::test] / #[rstest]).
        //    tree-sitter-rust tokenises outer attributes as sibling
        //    `attribute_item` nodes; `collect_function_signature` prepends
        //    them to the signature text, so a substring match on the
        //    signature reliably detects test decoration regardless of the
        //    function name or QN disambiguator. On bulwark this is the
        //    single largest FP reducer (1376/1387 = 99.2% of findings).
        if has_test_attribute_marker(&func.signature) {
            return true;
        }
        // 6. B3: Trait impl method (`#<TypeName>` disambiguator)
        if self.config.check_dynamic_dispatch && is_trait_impl_method(&func.qualified_name) {
            return true;
        }
        // 7. Exported functions (batch-prefetched)
        if self.config.check_exported && self.prefetch.is_exported(&func.id) {
            return true;
        }
        // 8. FFI entries (batch-prefetched)
        if self.config.check_ffi && self.prefetch.is_ffi_entry(&func.id) {
            return true;
        }
        // 9. B7: Re-export targets (batch-prefetched). Always checked —
        // `pub use` / `export ... from` makes the symbol reachable from
        // outside the current crate/module, so it's a live entry point
        // regardless of `check_exported` (which gates `pub fn`, a
        // different liveness path).
        if self.prefetch.is_reexport_target(&func.id) {
            return true;
        }
        // 10. T182-B: attribute-marked entry points. Macro expansion
        // synthesises an external call to the decorated fn that is
        // invisible to the static graph (tree-sitter does not expand
        // macros), so the attribute itself is the entry-point signal.
        // Substring match recognises both `#[tool]` (bare) and
        // `#[tool(name = "...")]` (with arguments). Non-entry-point
        // attributes (`#[cfg]`, `#[derive]`, `#[allow]`) are intentionally
        // absent from `DEFAULT_ATTRIBUTE_ENTRIES`.
        if attribute_entries
            .iter()
            .any(|attr| func.signature.contains(attr))
        {
            return true;
        }
        false
    }

    /// Propagates reachability from the worklist to a fixed point.
    ///
    /// Pops each id, inserts it into `live_set` (skipping if already
    /// present), looks up outgoing edges from the prefetched
    /// [`BatchPrefetch::outgoing_edges`] map, and pushes any
    /// unreachable targets back onto the worklist.
    ///
    /// # Performance
    ///
    /// O(V + E) total — each worklist pop is an O(1) HashMap lookup, vs
    /// the previous O(1) Cypher round-trip per pop (which is O(1) in
    /// query count but ~1ms in wall time, so V pops = V ms on CalNexus).
    pub(crate) fn propagate(&mut self) {
        while let Some(id) = self.worklist.pop_front() {
            // Skip if already live (perf-review LOW-1: avoid `id.clone()`
            // by checking containment before inserting).
            if self.live_set.contains(&id) {
                continue;
            }
            // O(1) HashMap lookup vs O(1) Cypher round-trip per pop.
            if let Some(targets) = self.prefetch.outgoing_edges(&id) {
                for target in targets {
                    if !self.live_set.contains(target) {
                        self.worklist.push_back(target.clone());
                    }
                }
            }
            // Move `id` into live_set without cloning.
            self.live_set.insert(id);
        }
    }

    /// Consumes the analyzer and returns ownership of the live set,
    /// avoiding a full `HashSet::clone` in the caller (perf-review
    /// MEDIUM-3).
    #[must_use]
    pub(crate) fn into_live_set(self) -> std::collections::HashSet<String> {
        self.live_set
    }
}

/// Detects dead code (zero-indegree CALLS functions) for a project.
pub struct DeadCodeDetector<'a> {
    storage: &'a dyn Storage,
    config: DeadCodeConfig,
}

impl<'a> DeadCodeDetector<'a> {
    /// Creates a new detector backed by the given storage capability, using
    /// the default [`DeadCodeConfig`].
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self::with_config(storage, DeadCodeConfig::default())
    }

    /// Creates a new detector with the supplied [`DeadCodeConfig`].
    #[must_use]
    pub fn with_config(storage: &'a dyn Storage, config: DeadCodeConfig) -> Self {
        Self { storage, config }
    }

    /// Returns the dead-code entries for `project`.
    ///
    /// `entry_patterns` are glob patterns (using `*` as the only wildcard)
    /// for function names that should NOT be considered dead even with zero
    /// incoming CALLS edges (e.g. `"main"`, `"__main__"`). Test-function
    /// patterns (`test_*`, `*_test`, `*_spec`) are always excluded.
    ///
    /// # Performance (perf-1 + perf-review MEDIUM-1 + B7)
    ///
    /// All Cypher round-trips are batched:
    ///
    /// - `load_all_functions` (2 queries: Function + Method labels) —
    ///   also returns `isExported` / `signature` so `exported_ids` /
    ///   `ffi_entry_ids` are derived in Rust without extra queries.
    /// - `BatchPrefetch::load` (2 queries: outgoing_edges + B7
    ///   reexport_target_ids).
    /// - `load_edge_targets_by_category` (1 query: confidence scoring).
    /// - `load_file_languages` (1 query: language resolution).
    ///
    /// Total: 6 Cypher round-trips, independent of project size.
    /// Previously this scaled as 6N+V (N=function count, V=edge count).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if any Cypher query fails.
    pub fn detect(
        &self,
        project: &str,
        entry_patterns: &[&str],
    ) -> StorageResult<Vec<DeadCodeEntry>> {
        // (a) Load all Function/Method nodes for the project (2 Cypher
        // round-trips). Includes isExported + signature so the prefetch
        // cache can derive exported_ids / ffi_entry_ids in Rust.
        let functions = load_all_functions(self.storage, project)?;

        // (b) Batch-prefetch outgoing_edges (1 Cypher) + B7 reexport_target_ids
        // (1 Cypher); derive exported_ids / ffi_entry_ids from `functions`
        // (perf-review MEDIUM-1: collapses 4 Cypher round-trips into 0 by
        // reusing the already-loaded rows). Shared between ReachabilityAnalyzer
        // and the filter loop below — eliminates the DRY violation where
        // is_exported / is_ffi_entry / load_functions were duplicated
        // across the two structs.
        let prefetch = BatchPrefetch::load(self.storage, project, &self.config, &functions)?;

        // (c) B5: Worklist reachability propagation from seeds.
        // Replaces single-layer `referenced_ids` check with proper BFS
        // propagation aligned with rustc's `MarkSymbolVisitor`. Functions
        // not in `live_set` are dead (subject to additional B0-B4 filters
        // below as defense-in-depth).
        let mut analyzer = ReachabilityAnalyzer::new(&prefetch, &self.config, &functions);
        analyzer.collect_seeds(entry_patterns);
        analyzer.propagate();
        // perf-review MEDIUM-3: consume the analyzer instead of cloning
        // the live_set (avoids one O(V) HashSet allocation + copy).
        let live_set = analyzer.into_live_set();

        // (d) Load all edge targets split by CALLS vs non-CALLS for confidence.
        let (calls_targets, non_calls_targets) = self.load_edge_targets_by_category(project)?;

        // (e) Build a filePath -> language map from the File table.
        let file_languages = self.load_file_languages(project)?;

        // (f) Filter: zero-indegree + not an entry point + not a test function.
        let config_entry_patterns: Vec<&str> = self
            .config
            .entry_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let config_test_patterns: Vec<&str> = self
            .config
            .test_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        // T182-B: config.attribute_entries substring list for defense-in-depth
        // (seeds already short-circuit liveness via `is_seed_function` above;
        // this gate exists so a future code path that bypasses the worklist
        // still respects attribute-marked entry points).
        let config_attribute_entries: Vec<&str> = self
            .config
            .attribute_entries
            .iter()
            .map(|s| s.as_str())
            .collect();
        let mut entries = Vec::new();
        // perf-review LOW-1: hoist the constant reason string out of the
        // loop to avoid re-allocating an identical String per dead candidate
        // (dead candidates are typically < 100, but the clone is still
        // cheaper than a fresh to_string + heap allocation each iteration).
        let reason_zero_incoming = REASON_ZERO_INCOMING_CALLS.to_string();
        for func in &functions {
            // B5: primary gate — function is live if reachable from any seed.
            // The worklist propagation is the sole determinant of liveness;
            // `referenced_ids` is retained only for confidence scoring.
            if live_set.contains(&func.id) {
                continue;
            }
            if matches_any_pattern(&func.name, entry_patterns)
                || matches_any_pattern(&func.name, &config_entry_patterns)
            {
                continue;
            }
            if matches_any_pattern(&func.name, DEFAULT_TEST_PATTERNS)
                || matches_any_pattern(&func.name, &config_test_patterns)
            {
                continue;
            }
            // B0 fix: Functions inside `mod tests` blocks have a `#tests` (or
            // `#tests_<MockName>`) disambiguator in their qualified_name (e.g.
            // `demo.src.lib.rs.foo#tests`). These are test-module-scoped and
            // should NOT be flagged as dead. This was the largest false-positive
            // source on CalNexus (239/360 = 66% of all findings).
            if is_test_module_function(&func.qualified_name) {
                continue;
            }
            // B4 fix: Integration tests live in well-known test directories
            // (e.g. Rust `tests/*.rs`, Python `tests/test_*.py`, Java
            // `src/test/java/`). They are discovered and invoked by the
            // language's test runner (`cargo test`, `pytest`, `go test`) and
            // have no static CALLS edge in the graph. On CalNexus, this
            // eliminated 8/11 remaining false positives (integration tests
            // with descriptive names that don't match any test_* glob).
            if is_integration_test_file(&func.file_path) {
                continue;
            }
            // Defense-in-depth: `#[test]` / `#[tokio::test]` / `#[rstest]`
            // attribute markers. Mirrors `is_seed_function` category 5 —
            // catches test functions whose names match no glob and whose QN
            // lacks the `#tests` disambiguator (the common case for inline
            // `mod tests` blocks in production source files like
            // `src/service/foo.rs`). On bulwark this is the dominant FP
            // reducer: 1376/1387 = 99.2% of findings were `#[test]` fns.
            if has_test_attribute_marker(&func.signature) {
                continue;
            }
            // Use batch-prefetched sets instead of per-function Cypher
            // queries (perf-1: was 2N Cypher round-trips, now 0).
            if self.config.check_exported && prefetch.is_exported(&func.id) {
                continue;
            }
            if self.config.check_ffi && prefetch.is_ffi_entry(&func.id) {
                continue;
            }
            // B3 fix: Trait impl methods (e.g. `fmt#Display`, `complete#ReplHelper`)
            // have a `#<TypeName>` disambiguator. When check_dynamic_dispatch=true,
            // treat them as live — they are called via dynamic dispatch / vtable
            // and have no static CALLS edge in the graph.
            if self.config.check_dynamic_dispatch && is_trait_impl_method(&func.qualified_name) {
                continue;
            }
            // T182-B defense-in-depth: attribute-marked entry points. Mirrors
            // `is_seed_function` category 9 — keeps the filter loop
            // self-consistent if `live_set` ever diverges from the seed list
            // (e.g. via a future config that disables worklist seeding but
            // keeps the filter loop). Substring match on `func.signature`.
            if config_attribute_entries
                .iter()
                .any(|attr| func.signature.contains(attr))
            {
                continue;
            }
            let language = file_languages
                .get(&func.file_path)
                .cloned()
                .unwrap_or_default();
            let confidence = if calls_targets.contains(&func.id) {
                Confidence::Low
            } else if non_calls_targets.contains(&func.id) {
                Confidence::Medium
            } else {
                Confidence::High
            };
            entries.push(DeadCodeEntry {
                name: func.name.clone(),
                qualified_name: func.qualified_name.clone(),
                file_path: func.file_path.clone(),
                start_line: func.start_line,
                language,
                reason: reason_zero_incoming.clone(),
                confidence,
            });
        }
        // Stable order by qualified name for deterministic output.
        entries.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(entries)
    }

    /// Loads the set of node ids that are targets of any edge type listed in
    /// [`DeadCodeConfig::edge_types`] for `project`.
    ///
    /// A function is "used" if it appears as the `target` of at least one
    /// CodeRelation edge whose type is in the configured set (CALLS, USAGE,
    /// HANDLES_ROUTE, TESTS, etc.).
    ///
    /// B5 note: Production code now uses `ReachabilityAnalyzer` for
    /// reachability propagation. This method is retained for test coverage
    /// of the Cypher edge-loading pattern (also used by
    /// `load_edge_targets_by_category`).
    #[cfg(test)]
    fn load_referenced_ids(
        &self,
        project: &str,
    ) -> StorageResult<std::collections::HashSet<String>> {
        if self.config.edge_types.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let escaped = escape_cypher_string(project);
        // EdgeType::as_db_type returns static UPPERCASE DDL strings from a
        // controlled enum, so they are safe to embed in Cypher without extra
        // escaping. Collapsing the former per-type loop into a single `IN`
        // query reduces 9 round-trips (default config) to 1.
        let in_list = self
            .config
            .edge_types
            .iter()
            .map(|t| format!("'{}'", t.as_db_type()))
            .collect::<Vec<_>>()
            .join(", ");
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type IN [{in_list}] AND e.project = '{escaped}' \
             RETURN e.target AS target;"
        );
        let mut set = std::collections::HashSet::new();
        let rows = self.storage.query(&cypher)?;
        for row in rows {
            if let Some(target) = row.first().and_then(|v| v.as_str()) {
                set.insert(target.to_string());
            }
        }
        Ok(set)
    }

    /// Loads all CodeRelation targets for `project`, split into two sets:
    /// - `calls_targets`: ids that are targets of `CALLS` edges.
    /// - `non_calls_targets`: ids that are targets of any non-`CALLS` edge.
    ///
    /// Used for confidence scoring: a dead function with only non-CALLS
    /// incoming edges gets `Medium` confidence; one with CALLS gets `Low`.
    fn load_edge_targets_by_category(
        &self,
        project: &str,
    ) -> StorageResult<(
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    )> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.project = '{escaped}' \
             RETURN e.target AS target, e.type AS type;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut calls_targets = std::collections::HashSet::new();
        let mut non_calls_targets = std::collections::HashSet::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let target = row[0].as_str().unwrap_or_default().to_string();
            let edge_type = row[1].as_str().unwrap_or_default();
            if edge_type == "CALLS" {
                calls_targets.insert(target);
            } else {
                non_calls_targets.insert(target);
            }
        }
        Ok((calls_targets, non_calls_targets))
    }

    /// Checks whether `func_id` has any incoming edge of `edge_type`.
    ///
    /// No project filter is applied — `func_id` is globally unique.
    ///
    /// Unlike [`load_referenced_ids`](Self::load_referenced_ids), which builds a
    /// project-wide set of referenced targets aggregated across every configured
    /// edge type, this method answers a single-func, single-edge-type question.
    /// Currently exercised by unit tests in `mod tests`; reserved for future
    /// single-edge-type diagnostics (e.g. "is this function tested?" via
    /// `EdgeType::Tests`) when `load_referenced_ids` is overkill.
    #[allow(
        dead_code,
        reason = "exercised by unit tests; reserved for future diagnostics"
    )]
    fn has_incoming_edge(&self, func_id: &str, edge_type: EdgeType) -> StorageResult<bool> {
        let escaped_id = escape_cypher_string(func_id);
        let type_str = edge_type.as_db_type();
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = '{type_str}' AND e.target = '{escaped_id}' \
             RETURN e.id AS id LIMIT 1;"
        );
        Ok(!self.storage.query(&cypher)?.is_empty())
    }

    /// Builds a `filePath -> language` map from the `File` table for `project`.
    fn load_file_languages(
        &self,
        project: &str,
    ) -> StorageResult<std::collections::HashMap<String, String>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (f:File) WHERE f.project = '{escaped}' \
             RETURN f.filePath AS file_path, f.language AS language;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let file_path = row[0].as_str().unwrap_or_default().to_string();
            let language = row[1].as_str().unwrap_or_default().to_string();
            map.insert(file_path, language);
        }
        Ok(map)
    }
}

/// Internal row representation for a Function/Method node.
///
/// `is_exported` and `signature` are loaded alongside the identity fields
/// so [`BatchPrefetch`] can derive `exported_ids` / `ffi_entry_ids` in
/// Rust without additional Cypher round-trips (perf-review MEDIUM-1).
pub(crate) struct FunctionRow {
    id: String,
    name: String,
    qualified_name: String,
    file_path: String,
    start_line: u32,
    is_exported: bool,
    signature: String,
}

/// Returns `true` if `name` matches any of the glob `patterns`.
///
/// Supports `*` as the only wildcard (matches any sequence of characters,
/// including the empty sequence). All other characters match literally.
fn matches_any_pattern(name: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| glob_match(p, name))
}

/// Returns `true` if `qualified_name` has a `#tests` or `#tests_*` disambiguator,
/// indicating the function lives inside a `mod tests` block (B0 fix).
///
/// Examples:
/// - `demo.src.lib.rs.foo#tests` → `true`
/// - `demo.src.lib.rs.bar#tests_ConfigurableMockDomain` → `true`
/// - `demo.src.lib.rs.fmt#Display` → `false` (trait impl, not test module)
/// - `demo.src.lib.rs.plain` → `false` (no disambiguator)
fn is_test_module_function(qualified_name: &str) -> bool {
    let Some(idx) = qualified_name.rfind('#') else {
        return false;
    };
    let disambiguator = &qualified_name[idx + 1..];
    disambiguator == "tests" || disambiguator.starts_with("tests_")
}

/// Returns `true` if `qualified_name` has a `#<TypeName>` disambiguator that is
/// NOT a `#tests` marker, indicating a trait impl method (B3 fix).
///
/// Examples:
/// - `demo.src.lib.rs.fmt#Display` → `true` (impl Display)
/// - `demo.src.repl.rs.complete#ReplHelper` → `true` (impl ReplHelper)
/// - `demo.src.lib.rs.foo#tests` → `false` (test module, not trait impl)
/// - `demo.src.lib.rs.plain` → `false` (no disambiguator)
fn is_trait_impl_method(qualified_name: &str) -> bool {
    let Some(idx) = qualified_name.rfind('#') else {
        return false;
    };
    let disambiguator = &qualified_name[idx + 1..];
    !disambiguator.is_empty() && disambiguator != "tests" && !disambiguator.starts_with("tests_")
}

/// Returns `true` if `file_path` indicates an integration test file (B4 fix).
///
/// Integration tests are discovered and invoked by the language's test runner
/// (e.g. `cargo test` for Rust, `pytest` for Python, `go test` for Go) and
/// have no static CALLS edge in the graph. They live in well-known
/// directories:
///
/// - Rust: `tests/*.rs` (top-level integration tests)
/// - Python: `tests/test_*.py` or `test/test_*.py`
/// - Go: `*_test.go` (handled by file suffix, not directory)
/// - Java: `src/test/java/*.java`
///
/// This function checks if the path starts with (or contains) a test
/// directory marker. The check is intentionally conservative — it only
/// matches well-known test directory layouts to avoid false negatives on
/// production code that happens to live under a `tests/` subdirectory.
///
/// Examples:
/// - `tests/numerical_linalg_test.rs` → `true` (Rust integration test)
/// - `tests/repl_integration.rs` → `true` (Rust integration test)
/// - `src/lib.rs` → `false` (production source)
/// - `src/test/java/FooTest.java` → `true` (Java test)
/// - `tests/helpers/mod.rs` → `true` (Rust test helper module)
fn is_integration_test_file(file_path: &str) -> bool {
    // perf-review LOW-2: short-circuit the `replace` allocation when the
    // path contains no backslashes (the common case on Linux/macOS).
    let normalized: std::borrow::Cow<'_, str> = if file_path.contains('\\') {
        std::borrow::Cow::Owned(file_path.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(file_path)
    };
    // Rust: top-level `tests/` directory (integration tests).
    // Python: `tests/` or `test/` directory.
    // Java/JVM: `src/test/` directory.
    // Check both `starts_with` (relative path from project root) and
    // `contains` (in case the path is absolute or has a project prefix).
    if normalized.starts_with("tests/") || normalized.contains("/tests/") {
        return true;
    }
    if normalized.starts_with("test/") || normalized.contains("/test/") {
        // Avoid false positives on `test/` substrings in production paths
        // like `latest/` or `contest/`. Only match if the segment is a
        // proper directory boundary.
        // `contains("/test/")` already enforces directory boundaries.
        return true;
    }
    if normalized.contains("/src/test/") {
        return true;
    }
    false
}

/// Simple glob matcher where `*` matches any sequence of characters.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_helper(&p, &t)
}

fn glob_helper(p: &[char], t: &[char]) -> bool {
    match (p.first(), t.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some('*'), None) => glob_helper(&p[1..], t),
        (Some('*'), Some(_)) => glob_helper(&p[1..], t) || glob_helper(p, &t[1..]),
        // Non-`*` pattern char with empty text: cannot match.
        (Some(_), None) => false,
        (Some(pc), Some(tc)) => *pc == *tc && glob_helper(&p[1..], &t[1..]),
    }
}

// T202 arch-review MEDIUM-2: tests extracted to `tests.rs` to keep the
// implementation file focused. The test module accesses internals via
// `use super::*` (re-exports below are not needed because `mod.rs` still
// holds the implementation — when this module is further split into
// submodules, each submodule's `pub(super)` items will flow through here).
#[cfg(test)]
mod tests;
