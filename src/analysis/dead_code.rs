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
use crate::storage::schema::escape_cypher_string;
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
/// `#[allow(...)]`, `#[test]`, `#[bench]`) are intentionally absent —
/// `#[test]` / `#[bench]` are already covered by `DEFAULT_TEST_PATTERNS`
/// name-globs, and the others do not register external entry points.
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
    /// 5. Trait impl method (B3 — `#<TypeName>` disambiguator, when `check_dynamic_dispatch`)
    /// 6. Exported functions (`isExported=true`, when `check_exported`)
    /// 7. FFI entries (signature contains `extern "C"` / `#[no_mangle]`, when `check_ffi`)
    /// 8. Re-export targets (B7 — `REEXPORTS` edge targets, always checked)
    /// 9. T182-B: attribute-marked entry points (signature contains any
    ///    `attribute_entries` substring, e.g. `#[tool` / `#[forge` /
    ///    `#[tokio::main`)
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
        // 5. B3: Trait impl method (`#<TypeName>` disambiguator)
        if self.config.check_dynamic_dispatch && is_trait_impl_method(&func.qualified_name) {
            return true;
        }
        // 6. Exported functions (batch-prefetched)
        if self.config.check_exported && self.prefetch.is_exported(&func.id) {
            return true;
        }
        // 7. FFI entries (batch-prefetched)
        if self.config.check_ffi && self.prefetch.is_ffi_entry(&func.id) {
            return true;
        }
        // 8. B7: Re-export targets (batch-prefetched). Always checked —
        // `pub use` / `export ... from` makes the symbol reachable from
        // outside the current crate/module, so it's a live entry point
        // regardless of `check_exported` (which gates `pub fn`, a
        // different liveness path).
        if self.prefetch.is_reexport_target(&func.id) {
            return true;
        }
        // 9. T182-B: attribute-marked entry points. Macro expansion
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
                reason: REASON_ZERO_INCOMING_CALLS.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("dead_code_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Returns the `dyn Storage` capability from `kit`.
    fn storage(
        kit: &AsyncKit<AsyncReady>,
    ) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
        kit.require::<StorageModule>().expect("require_storage")
    }

    /// Creates a Function node via direct Cypher.
    fn create_function(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create function");
    }

    /// Creates a Function node with `isExported = true` and optional `signature`.
    #[allow(clippy::too_many_arguments)]
    fn create_function_with_flags(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
        is_exported: bool,
        signature: &str,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '{}', returnType: '', \
             isExported: {}, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
            escape_cypher_string(signature),
            is_exported,
        );
        storage
            .execute(&cypher)
            .expect("create function with flags");
    }

    /// Creates a Method node via direct Cypher.
    fn create_method(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Method {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parameterCount: 0, parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create method");
    }

    /// Creates a CALLS edge from `caller_id` to `callee_id`.
    fn create_calls_edge(
        kit: &AsyncKit<AsyncReady>,
        edge_id: &str,
        caller_id: &str,
        callee_id: &str,
        project: &str,
    ) {
        create_edge(kit, edge_id, caller_id, callee_id, project, "CALLS");
    }

    /// Creates a CodeRelation edge of `edge_type` (DDL string, e.g. `"USAGE"`)
    /// from `source_id` to `target_id`.
    fn create_edge(
        kit: &AsyncKit<AsyncReady>,
        edge_id: &str,
        source_id: &str,
        target_id: &str,
        project: &str,
        edge_type: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', type: '{}', \
             confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: '{}'}});",
            escape_cypher_string(edge_id),
            escape_cypher_string(source_id),
            escape_cypher_string(target_id),
            escape_cypher_string(edge_type),
            escape_cypher_string(project),
        );
        storage.execute(&cypher).expect("create edge");
    }

    /// Creates a File node (for language resolution).
    fn create_file(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        file_path: &str,
        language: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:File {{id: '{}', project: '{}', name: '{}', filePath: '{}', \
             language: '{}', hash: '', lineCount: 0}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(file_path.split('/').next_back().unwrap_or("file")),
            escape_cypher_string(file_path),
            escape_cypher_string(language),
        );
        storage.execute(&cypher).expect("create file");
    }

    // --- glob_match unit tests ---

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "main2"));
    }

    #[test]
    fn glob_match_prefix_wildcard() {
        assert!(glob_match("test_*", "test_foo"));
        assert!(glob_match("test_*", "test_"));
        assert!(!glob_match("test_*", "foo_test"));
    }

    #[test]
    fn glob_match_suffix_wildcard() {
        assert!(glob_match("*_test", "foo_test"));
        assert!(glob_match("*_test", "_test"));
        assert!(!glob_match("*_test", "test_foo"));
    }

    #[test]
    fn glob_match_middle_wildcard() {
        assert!(glob_match("test_*_spec", "test_foo_spec"));
        assert!(!glob_match("test_*_spec", "test_foo"));
    }

    #[test]
    fn glob_match_star_only() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    // --- DeadCodeDetector tests ---

    #[test]
    fn detect_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        assert!(result.is_empty(), "empty DB should yield no dead code");
    }

    #[test]
    fn detect_finds_dead_function() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `foo` has no incoming CALLS edges; `main` also has no incoming
        // edges but is excluded as an entry point.
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/main.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"foo"), "foo should be dead: {:?}", names);
        assert!(
            !names.contains(&"main"),
            "main should be excluded: {:?}",
            names
        );
    }

    // T182-B: Function nodes whose signature carries an entry-point
    // attribute (`#[tool(...)]` / `#[forge(...)]` / `#[tokio::main]` /
    // `#[rocket::main]` / `#[actix::main]` / `#[axum::main]` etc.) must
    // NOT be reported dead. These attributes register the function as an
    // external entry point via macro expansion (e.g. rmcp `#[tool]` /
    // CodeNexus `#[forge]` register MCP tools; `#[tokio::main]` synthesizes
    // a synchronous `main` that calls the async fn). tree-sitter does not
    // expand macros, so the synthesised CALLS edge is invisible to the
    // graph — dead_code must treat the attribute itself as the entry-point
    // signal (B4.5 deferred task, T045/T046 spec).
    #[test]
    fn b_tool_attribute_marked_functions_treated_as_live() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `#[tool(name = "query")]` registers `query_mcp` as an MCP tool —
        // macro expansion synthesises a dispatch table that calls it.
        create_function_with_flags(
            &kit,
            "f_tool",
            "demo",
            "query_mcp",
            "demo.query_mcp",
            "/src/service/query.rs",
            75,
            false,
            "#[tool(name = \"query\")]\nasync fn query_mcp() {}",
        );
        // `#[forge(name = "architecture", cli = true)]` is the CodeNexus
        // equivalent — `#[forge]` registers both an MCP tool and a CLI
        // subcommand.
        create_function_with_flags(
            &kit,
            "f_forge",
            "demo",
            "architecture",
            "demo.architecture",
            "/src/service/architecture.rs",
            63,
            false,
            "#[forge(name = \"architecture\", cli = true)]\nasync fn architecture() {}",
        );
        // Control: plain private function with no attribute, no incoming
        // CALLS edges → IS dead.
        create_function_with_flags(
            &kit,
            "f_plain",
            "demo",
            "unused_helper",
            "demo.unused_helper",
            "/src/lib.rs",
            100,
            false,
            "fn unused_helper() {}",
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"query_mcp"),
            "T182-B: #[tool]-marked query_mcp must NOT be dead: {:?}",
            names
        );
        assert!(
            !names.contains(&"architecture"),
            "T182-B: #[forge]-marked architecture must NOT be dead: {:?}",
            names
        );
        assert!(
            names.contains(&"unused_helper"),
            "T182-B: plain unused_helper IS dead: {:?}",
            names
        );
    }

    // T182-B: Verifies that the common async-runtime / web-framework entry
    // attributes (`#[tokio::main]`, `#[rocket::main]`, `#[actix::main]`,
    // `#[axum::main]`) are also recognised as entry-point seeds. These
    // macros synthesise a synchronous `main` that calls the decorated async
    // fn, so the decorated fn has no static CALLS edge in the graph.
    #[test]
    fn b_async_runtime_entry_attributes_treated_as_live() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_tokio",
            "demo",
            "run",
            "demo.run",
            "/src/main.rs",
            10,
            false,
            "#[tokio::main]\nasync fn run() {}",
        );
        create_function_with_flags(
            &kit,
            "f_rocket",
            "demo",
            "rocket_main",
            "demo.rocket_main",
            "/src/main.rs",
            20,
            false,
            "#[rocket::main]\nasync fn rocket_main() {}",
        );
        create_function_with_flags(
            &kit,
            "f_actix",
            "demo",
            "actix_main",
            "demo.actix_main",
            "/src/main.rs",
            30,
            false,
            "#[actix::main]\nasync fn actix_main() {}",
        );
        create_function_with_flags(
            &kit,
            "f_axum",
            "demo",
            "axum_main",
            "demo.axum_main",
            "/src/main.rs",
            40,
            false,
            "#[axum::main]\nasync fn axum_main() {}",
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        for expected in ["run", "rocket_main", "actix_main", "axum_main"] {
            assert!(
                !names.contains(&expected),
                "T182-B: async-runtime entry attribute should keep {expected} live: {:?}",
                names
            );
        }
    }

    // T182-B: Verifies the attribute seed check is a substring match (not
    // exact match), so both `#[tool]` (bare) and `#[tool(...)]` (with
    // arguments) are recognised. Also verifies that `#[cfg(...)]` /
    // `#[derive(...)]` (non-entry-point attributes) do NOT falsely mark a
    // function as live.
    #[test]
    fn b_attribute_seed_uses_substring_match_and_ignores_non_entry_attributes() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Bare `#[tool]` without arguments — substring `#[tool` matches.
        create_function_with_flags(
            &kit,
            "f_bare",
            "demo",
            "bare_tool",
            "demo.bare_tool",
            "/src/m.rs",
            1,
            false,
            "#[tool]\nfn bare_tool() {}",
        );
        // `#[cfg(test)]` + `#[derive(Debug)]` are NOT entry-point attributes
        // — this function should still be reported dead.
        create_function_with_flags(
            &kit,
            "f_cfg",
            "demo",
            "cfg_only",
            "demo.cfg_only",
            "/src/m.rs",
            10,
            false,
            "#[cfg(test)]\n#[derive(Debug)]\nfn cfg_only() {}",
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"bare_tool"),
            "T182-B: bare #[tool] should mark bare_tool as live: {:?}",
            names
        );
        assert!(
            names.contains(&"cfg_only"),
            "T182-B: #[cfg]/#[derive] should NOT mark cfg_only as live: {:?}",
            names
        );
    }

    // B7: a Function with no incoming CALLS edges but targeted by a
    // REEXPORTS edge (File→Function, created by `resolve/imports.rs`
    // for `pub use` / `export ... from`) must NOT be reported dead —
    // the symbol is reachable from outside the current crate/module.
    #[test]
    fn detect_excludes_reexport_targets() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `bar` is re-exported: REEXPORTS edge from a File node to `bar`.
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 1);
        create_function(&kit, "f_qux", "demo", "qux", "demo.qux", "/src/lib.rs", 50);
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/main.rs", "rust");
        // REEXPORTS edge: file2 re-exports bar from file1.
        create_edge(&kit, "e_reexport", "file2", "f_bar", "demo", "REEXPORTS");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"bar"),
            "bar is re-exported, must NOT be dead: {:?}",
            names
        );
        assert!(
            names.contains(&"qux"),
            "qux has no incoming edges and is not re-exported, must be dead: {:?}",
            names
        );
    }

    // B7: BatchPrefetch correctly loads REEXPORTS edge targets into
    // `reexport_target_ids` and `is_reexport_target` returns true for them.
    #[test]
    fn batch_prefetch_loads_reexport_targets() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 1);
        create_function(&kit, "f_baz", "demo", "baz", "demo.baz", "/src/lib.rs", 50);
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/main.rs", "rust");
        create_edge(&kit, "e1", "file2", "f_bar", "demo", "REEXPORTS");

        let storage = storage(&kit);
        let functions = load_all_functions(&*storage, "demo").expect("load_all_functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &DeadCodeConfig::default(), &functions)
                .expect("BatchPrefetch::load");
        assert!(
            prefetch.is_reexport_target("f_bar"),
            "f_bar is a REEXPORTS target"
        );
        assert!(
            !prefetch.is_reexport_target("f_baz"),
            "f_baz is NOT a REEXPORTS target"
        );
        assert_eq!(
            prefetch.reexport_target_ids_len(),
            1,
            "exactly 1 reexport target"
        );
    }

    #[test]
    fn detect_excludes_entry_points() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_file(&kit, "file1", "demo", "/src/main.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"main"), "main is an entry point");
        assert!(names.contains(&"foo"), "foo is not an entry point");
    }

    #[test]
    fn detect_excludes_test_functions() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `test_foo` matches the default `test_*` pattern and is excluded
        // even though it has zero incoming CALLS edges.
        create_function(
            &kit,
            "f_test_foo",
            "demo",
            "test_foo",
            "demo.test_foo",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_foo_test",
            "demo",
            "foo_test",
            "demo.foo_test",
            "/src/lib.rs",
            10,
        );
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 20);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"test_foo"), "test_foo matches test_*");
        assert!(!names.contains(&"foo_test"), "foo_test matches *_test");
        assert!(names.contains(&"foo"), "foo is dead");
    }

    #[test]
    fn detect_handles_method_nodes() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // A Method node with zero incoming CALLS edges is dead code.
        create_method(
            &kit,
            "m_1",
            "demo",
            "helper",
            "demo.Class.helper",
            "/src/lib.rs",
            5,
        );
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"helper"), "Method helper should be dead");
    }

    #[test]
    fn detect_excludes_functions_with_incoming_calls() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `main` calls `foo`; `foo` has an incoming CALLS edge and is NOT dead.
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_calls_edge(&kit, "e1", "f_main", "f_foo", "demo");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"foo"), "foo is called by main, not dead");
        assert!(!names.contains(&"main"), "main is an entry point");
    }

    #[test]
    fn detect_resolves_language_from_file_table() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.py", 1);
        create_file(&kit, "file1", "demo", "/src/lib.py", "python");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(entry.language, "python");
    }

    #[test]
    fn detect_includes_reason_field() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(entry.reason, "zero incoming CALLS edges");
    }

    #[test]
    fn detect_all_dead_when_no_edges_and_no_entries() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/b.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        // No entry patterns, no CALLS edges → everything is dead.
        let result = detector.detect("demo", &[]).expect("detect");
        assert_eq!(result.len(), 2, "both functions should be dead");
    }

    #[test]
    fn detect_filters_by_project() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        create_function(&kit, "f_b", "other", "b", "other.b", "/src/b.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a"), "a is in demo project");
        assert!(!names.contains(&"b"), "b is in other project");
    }

    // --- T002: DeadCodeConfig / Confidence tests ---

    #[test]
    fn dead_code_config_default_values() {
        let cfg = DeadCodeConfig::default();
        // Entry patterns default to the 6 multi-language entry points.
        assert_eq!(
            cfg.entry_patterns,
            vec![
                "main".to_string(),
                "Main".to_string(),
                "__main__".to_string(),
                "wmain".to_string(),
                "WinMain".to_string(),
                "DLLMain".to_string(),
            ]
        );
        // Test patterns mirror DEFAULT_TEST_PATTERNS (B2: expanded to 8).
        assert_eq!(cfg.test_patterns.len(), 8);
        assert!(cfg.test_patterns.contains(&"test_*".to_string()));
        assert!(cfg.test_patterns.contains(&"*_test".to_string()));
        assert!(cfg.test_patterns.contains(&"*_spec".to_string()));
        assert!(cfg.test_patterns.contains(&"it_*".to_string()));
        assert!(cfg.test_patterns.contains(&"sec_*".to_string()));
        assert!(cfg.test_patterns.contains(&"snap_*".to_string()));
        assert!(cfg.test_patterns.contains(&"perf_*".to_string()));
        assert!(cfg.test_patterns.contains(&"bench_*".to_string()));
        // Exported / FFI checks are on by default.
        assert!(cfg.check_exported, "check_exported should default to true");
        assert!(cfg.check_ffi, "check_ffi should default to true");
        // B3.5: Dynamic-dispatch (trait impl recognition) is ON by default,
        // aligning with rustc's dead_code lint which treats trait impls as
        // reachable via vtable.
        assert!(
            cfg.check_dynamic_dispatch,
            "check_dynamic_dispatch should default to true (B3.5)"
        );
        assert!(!cfg.check_reflection);
        // Edge types must include all variants used for "used" detection
        // per R-dead_code-001.
        assert!(cfg.edge_types.contains(&EdgeType::Calls));
        assert!(cfg.edge_types.contains(&EdgeType::FfiCalls));
        assert!(cfg.edge_types.contains(&EdgeType::Implements));
        assert!(cfg.edge_types.contains(&EdgeType::HandlesRoute));
        assert!(cfg.edge_types.contains(&EdgeType::Usage));
        assert!(cfg.edge_types.contains(&EdgeType::Tests));
        assert!(cfg.edge_types.contains(&EdgeType::UsesType));
        assert!(cfg.edge_types.contains(&EdgeType::HttpCalls));
        assert!(cfg.edge_types.contains(&EdgeType::AsyncCalls));
    }

    #[test]
    fn confidence_serializes_high_medium_low() {
        // Variant name is the JSON representation (serde default).
        assert_eq!(
            serde_json::to_string(&Confidence::High).unwrap(),
            "\"High\""
        );
        assert_eq!(
            serde_json::to_string(&Confidence::Medium).unwrap(),
            "\"Medium\""
        );
        assert_eq!(serde_json::to_string(&Confidence::Low).unwrap(), "\"Low\"");
        // Roundtrip every variant.
        for c in [Confidence::High, Confidence::Medium, Confidence::Low] {
            let json = serde_json::to_string(&c).unwrap();
            let parsed: Confidence = serde_json::from_str(&json).unwrap();
            assert_eq!(c, parsed, "roundtrip failed for {json}");
        }
    }

    #[test]
    fn confidence_rejects_invalid_variant() {
        assert!(serde_json::from_str::<Confidence>("\"Critical\"").is_err());
        assert!(serde_json::from_str::<Confidence>("\"high\"").is_err());
    }

    #[test]
    fn detect_sets_confidence_high_for_zero_incoming() {
        // Until T007 refines scoring, zero-incoming entries are High.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(entry.confidence, Confidence::High);
    }

    #[test]
    fn with_config_accepts_custom_config() {
        // with_config must not panic and must produce a working detector.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            entry_patterns: vec![],
            test_patterns: vec![],
            check_exported: false,
            check_dynamic_dispatch: false,
            check_reflection: false,
            check_ffi: false,
            attribute_entries: vec![],
            edge_types: vec![EdgeType::Calls],
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        assert_eq!(result.len(), 1, "a should be dead with empty patterns");
    }

    // --- T003: multi-edge-type reference detection tests ---

    #[test]
    fn detect_usage_edge_prevents_dead_code() {
        // B5: a USAGE edge propagates reachability from a seed source to its
        // target. `bar` is configured as an entry-pattern seed; `foo` is
        // reachable from `bar` via USAGE, so neither is dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
        // bar uses foo → foo is reachable from seed bar.
        create_edge(&kit, "e1", "f_bar", "f_foo", "demo", "USAGE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["bar"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"foo"),
            "foo reachable from seed bar via USAGE"
        );
        assert!(!names.contains(&"bar"), "bar is a seed (entry pattern)");
    }

    #[test]
    fn detect_handles_route_edge_prevents_dead_code() {
        // B5: a HANDLES_ROUTE edge propagates reachability from a seed source
        // to its target. `reg` is configured as an entry-pattern seed;
        // `handler` is reachable from `reg` via HANDLES_ROUTE.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_handler",
            "demo",
            "handler",
            "demo.handler",
            "/src/lib.rs",
            1,
        );
        create_function(&kit, "f_reg", "demo", "reg", "demo.reg", "/src/lib.rs", 5);
        // reg -> handler (HANDLES_ROUTE) → handler reachable from seed reg.
        create_edge(&kit, "e1", "f_reg", "f_handler", "demo", "HANDLES_ROUTE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["reg"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"handler"),
            "handler reachable from seed reg via HANDLES_ROUTE"
        );
        assert!(!names.contains(&"reg"), "reg is a seed (entry pattern)");
    }

    #[test]
    fn detect_tests_edge_prevents_dead_code() {
        // B5: a TESTS edge propagates reachability from a seed source to its
        // target. `ttest` is configured as an entry-pattern seed; `target` is
        // reachable from `ttest` via TESTS.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_target",
            "demo",
            "target",
            "demo.target",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_ttest",
            "demo",
            "ttest",
            "demo.ttest",
            "/src/lib.rs",
            5,
        );
        // ttest tests target → target reachable from seed ttest.
        create_edge(&kit, "e1", "f_ttest", "f_target", "demo", "TESTS");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["ttest"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"target"),
            "target reachable from seed ttest via TESTS"
        );
        assert!(!names.contains(&"ttest"), "ttest is a seed (entry pattern)");
    }

    #[test]
    fn detect_all_edge_types_exhaustive_no_incoming_is_dead() {
        // R-dead_code-001: a function with no incoming edges of ANY type is dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_lone",
            "demo",
            "lone",
            "demo.lone",
            "/src/lib.rs",
            1,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"lone"),
            "lone has zero incoming edges → dead"
        );
    }

    #[test]
    fn has_incoming_edge_returns_true_for_existing_edge() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
        create_edge(&kit, "e1", "f_a", "f_b", "demo", "USAGE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            detector
                .has_incoming_edge("f_b", EdgeType::Usage)
                .expect("has_incoming_edge"),
            "f_b should have USAGE incoming edge"
        );
    }

    #[test]
    fn has_incoming_edge_returns_false_for_missing_edge() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            !detector
                .has_incoming_edge("f_a", EdgeType::Calls)
                .expect("has_incoming_edge"),
            "f_a has no CALLS incoming edge"
        );
    }

    #[test]
    fn has_incoming_edge_distinguishes_edge_types() {
        // A function with a USAGE edge but no CALLS edge: USAGE=true, CALLS=false.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
        create_edge(&kit, "e1", "f_a", "f_b", "demo", "USAGE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            detector
                .has_incoming_edge("f_b", EdgeType::Usage)
                .expect("has_incoming_edge"),
            "f_b has USAGE edge"
        );
        assert!(
            !detector
                .has_incoming_edge("f_b", EdgeType::Calls)
                .expect("has_incoming_edge"),
            "f_b has no CALLS edge"
        );
    }

    #[test]
    fn load_referenced_ids_collects_targets_across_multiple_edge_types() {
        // Verifies the single IN-clause query captures targets across all
        // configured edge types (CALLS, USAGE, TESTS) in one pass.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_tgt_calls",
            "demo",
            "tgt_calls",
            "demo.tgt_calls",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_tgt_usage",
            "demo",
            "tgt_usage",
            "demo.tgt_usage",
            "/src/lib.rs",
            5,
        );
        create_function(
            &kit,
            "f_tgt_tests",
            "demo",
            "tgt_tests",
            "demo.tgt_tests",
            "/src/lib.rs",
            10,
        );
        create_function(
            &kit,
            "f_src_a",
            "demo",
            "src_a",
            "demo.src_a",
            "/src/lib.rs",
            20,
        );
        create_function(
            &kit,
            "f_src_b",
            "demo",
            "src_b",
            "demo.src_b",
            "/src/lib.rs",
            25,
        );
        create_function(
            &kit,
            "f_src_c",
            "demo",
            "src_c",
            "demo.src_c",
            "/src/lib.rs",
            30,
        );
        create_edge(&kit, "e1", "f_src_a", "f_tgt_calls", "demo", "CALLS");
        create_edge(&kit, "e2", "f_src_b", "f_tgt_usage", "demo", "USAGE");
        create_edge(&kit, "e3", "f_src_c", "f_tgt_tests", "demo", "TESTS");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let referenced = detector
            .load_referenced_ids("demo")
            .expect("load_referenced_ids");
        assert!(
            referenced.contains("f_tgt_calls"),
            "CALLS target should be referenced"
        );
        assert!(
            referenced.contains("f_tgt_usage"),
            "USAGE target should be referenced"
        );
        assert!(
            referenced.contains("f_tgt_tests"),
            "TESTS target should be referenced"
        );
        assert_eq!(
            referenced.len(),
            3,
            "exactly 3 targets should be referenced"
        );
    }

    // --- T004: exported function detection tests ---

    #[test]
    fn detect_excludes_exported_functions() {
        // R-dead_code-002: isExported=true with no incoming edges → NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_pub",
            "demo",
            "pub_fn",
            "demo.pub_fn",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        create_function(
            &kit,
            "f_priv",
            "demo",
            "priv_fn",
            "demo.priv_fn",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"pub_fn"),
            "exported pub_fn should NOT be dead"
        );
        assert!(names.contains(&"priv_fn"), "private priv_fn should be dead");
    }

    #[test]
    fn detect_includes_exported_when_check_exported_false() {
        // When check_exported=false, exported functions ARE dead code.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_pub",
            "demo",
            "pub_fn",
            "demo.pub_fn",
            "/src/lib.rs",
            1,
            true,
            "",
        );

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            check_exported: false,
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"pub_fn"),
            "with check_exported=false, pub_fn IS dead"
        );
    }

    #[test]
    fn batch_prefetch_exported_ids_distinguishes_pub_from_priv() {
        // Replaces `is_exported_returns_correct_value` (arch-1): the detector
        // no longer exposes a per-function `is_exported` method. `BatchPrefetch`
        // bulk-loads all exported ids in two Cypher round-trips; callers
        // check liveness via `HashSet::contains`.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_pub",
            "demo",
            "pub_fn",
            "demo.pub_fn",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        create_function(
            &kit,
            "f_priv",
            "demo",
            "priv_fn",
            "demo.priv_fn",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(prefetch.is_exported("f_pub"), "f_pub should be exported");
        assert!(
            !prefetch.is_exported("f_priv"),
            "f_priv should NOT be exported"
        );
    }

    // --- T005: FFI entry point detection tests ---

    #[test]
    fn detect_excludes_ffi_entry_extern_c() {
        // R-dead_code-003: signature with `extern "C"` → NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_ffi",
            "demo",
            "ffi_fn",
            "demo.ffi_fn",
            "/src/lib.rs",
            1,
            false,
            r#"pub extern "C" fn ffi_fn(x: i32) -> i32"#,
        );
        create_function(
            &kit,
            "f_plain",
            "demo",
            "plain",
            "demo.plain",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"ffi_fn"),
            "ffi_fn is an FFI entry → not dead"
        );
        assert!(names.contains(&"plain"), "plain has no FFI markers → dead");
    }

    #[test]
    fn detect_excludes_ffi_entry_no_mangle() {
        // R-dead_code-003: signature with `#[no_mangle]` → NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_nm",
            "demo",
            "native_fn",
            "demo.native_fn",
            "/src/lib.rs",
            1,
            false,
            "#[no_mangle]\npub fn native_fn() -> u32",
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"native_fn"),
            "native_fn has #[no_mangle] → not dead"
        );
    }

    #[test]
    fn detect_includes_ffi_when_check_ffi_false() {
        // When check_ffi=false, FFI functions ARE dead code.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_ffi",
            "demo",
            "ffi_fn",
            "demo.ffi_fn",
            "/src/lib.rs",
            1,
            false,
            r#"extern "C" fn ffi_fn()"#,
        );

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            check_ffi: false,
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"ffi_fn"),
            "with check_ffi=false, ffi_fn IS dead"
        );
    }

    #[test]
    fn batch_prefetch_distinguishes_ffi_from_plain() {
        // Replaces the old `is_ffi_entry_distinguishes_ffi_from_plain` test
        // (arch-1: `is_ffi_entry` was removed; FFI detection now goes
        // through `BatchPrefetch::load`).
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_ffi",
            "demo",
            "ffi_fn",
            "demo.ffi_fn",
            "/src/lib.rs",
            1,
            false,
            r#"extern "C" fn ffi_fn()"#,
        );
        create_function(
            &kit,
            "f_plain",
            "demo",
            "plain",
            "demo.plain",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(
            prefetch.ffi_entry_ids.contains("f_ffi"),
            "f_ffi should be FFI entry"
        );
        assert!(
            !prefetch.ffi_entry_ids.contains("f_plain"),
            "f_plain should NOT be FFI entry"
        );
    }

    // --- T006: expanded entry point pattern tests ---

    #[test]
    fn detect_excludes_all_default_entry_patterns() {
        // R-dead_code-004: all 6 default entry patterns must be excluded.
        for entry_name in ["main", "Main", "__main__", "wmain", "WinMain", "DLLMain"] {
            let db = fresh_db_path();
            let kit = build_kit_for_db(&db);
            create_function(
                &kit,
                "f_entry",
                "demo",
                entry_name,
                &format!("demo.{entry_name}"),
                "/src/lib.rs",
                1,
            );
            // Also create a control function that IS dead.
            create_function(
                &kit,
                "f_dead",
                "demo",
                "dead_fn",
                "demo.dead_fn",
                "/src/lib.rs",
                5,
            );

            let storage = storage(&kit);
            let detector = DeadCodeDetector::new(&*storage);
            // Pass empty entry_patterns — config defaults should still apply.
            let result = detector.detect("demo", &[]).expect("detect");
            let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
            assert!(
                !names.contains(&entry_name),
                "{entry_name} should be excluded by default config patterns"
            );
            assert!(names.contains(&"dead_fn"), "dead_fn should still be dead");
        }
    }

    #[test]
    fn detect_excludes_custom_entry_patterns_parameter() {
        // R-dead_code-004: custom entry_patterns parameter still works.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_h",
            "demo",
            "handler",
            "demo.handler",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_d",
            "demo",
            "dead_fn",
            "demo.dead_fn",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["handler"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"handler"),
            "handler matches custom pattern"
        );
        assert!(names.contains(&"dead_fn"), "dead_fn is still dead");
    }

    #[test]
    fn detect_merges_parameter_and_config_entry_patterns() {
        // Both the parameter and config patterns are checked.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // "main" is in config defaults; "custom_entry" is passed via parameter.
        create_function(&kit, "f_m", "demo", "main", "demo.main", "/src/lib.rs", 1);
        create_function(
            &kit,
            "f_c",
            "demo",
            "custom_entry",
            "demo.custom_entry",
            "/src/lib.rs",
            5,
        );
        create_function(
            &kit,
            "f_d",
            "demo",
            "dead_fn",
            "demo.dead_fn",
            "/src/lib.rs",
            10,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["custom_entry"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"main"), "main excluded by config");
        assert!(
            !names.contains(&"custom_entry"),
            "custom_entry excluded by parameter"
        );
        assert!(names.contains(&"dead_fn"), "dead_fn is dead");
    }

    // --- T007: confidence scoring tests ---

    #[test]
    fn detect_confidence_high_for_zero_incoming_edges() {
        // R-dead_code-005: no incoming edges of ANY type → High.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(
            entry.confidence,
            Confidence::High,
            "zero incoming edges → High"
        );
    }

    #[test]
    fn detect_confidence_medium_for_non_calls_edge_only() {
        // R-dead_code-005: has USAGE but no CALLS → Medium.
        // Config with edge_types=[Calls] only: USAGE doesn't count as "used".
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
        // bar uses foo → foo has a USAGE incoming edge.
        create_edge(&kit, "e1", "f_bar", "f_foo", "demo", "USAGE");

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            edge_types: vec![EdgeType::Calls],
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        // foo is dead because USAGE is not in config.edge_types.
        let foo_entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead (USAGE not in config.edge_types)");
        assert_eq!(
            foo_entry.confidence,
            Confidence::Medium,
            "USAGE but no CALLS → Medium"
        );
    }

    #[test]
    fn detect_confidence_low_for_calls_edge_with_empty_config() {
        // R-dead_code-005: has CALLS but config doesn't check CALLS → Low.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
        // bar calls foo → foo has a CALLS incoming edge.
        create_calls_edge(&kit, "e1", "f_bar", "f_foo", "demo");

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            edge_types: vec![], // empty: nothing counts as "used"
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        let foo_entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead (empty edge_types)");
        assert_eq!(
            foo_entry.confidence,
            Confidence::Low,
            "has CALLS incoming edge → Low"
        );
    }

    #[test]
    fn detect_confidence_serializes_in_dead_code_entry() {
        // Confidence field must appear in serialized JSON.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        let json = serde_json::to_string(entry).expect("serialize");
        assert!(
            json.contains("\"confidence\":\"High\""),
            "JSON should contain confidence field: {json}"
        );
    }

    // --- Additional coverage tests (targeting uncovered lines) ---

    #[test]
    fn batch_prefetch_exported_ids_empty_for_nonexistent_id() {
        // Replaces `is_exported_returns_false_for_nonexistent_id` (arch-1).
        // `BatchPrefetch::load` only returns ids that actually exist in the
        // Function/Method tables — nonexistent ids are simply absent from
        // the returned HashSet.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);

        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(
            !prefetch.is_exported("nonexistent_id"),
            "nonexistent id should not be in exported_ids"
        );
        assert!(
            prefetch.exported_ids_len() == 0,
            "empty db → empty exported_ids"
        );
    }

    #[test]
    fn batch_prefetch_ffi_entry_ids_empty_for_nonexistent_id() {
        // Replaces `is_ffi_entry_returns_false_for_nonexistent_id` (arch-1).
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);

        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(
            !prefetch.ffi_entry_ids.contains("nonexistent_id"),
            "nonexistent id should not be in ffi_entry_ids"
        );
        assert!(
            prefetch.ffi_entry_ids_len() == 0,
            "empty db → empty ffi_entry_ids"
        );
    }

    // --- Additional coverage: glob_helper edge cases ---

    #[test]
    fn glob_match_returns_false_when_pattern_char_but_empty_text() {
        // Line 469: `(Some(_), None) => false` — non-`*` pattern char with
        // empty text cannot match.
        assert!(!glob_match("a", ""));
        assert!(!glob_match("abc", ""));
    }

    #[test]
    fn glob_match_returns_false_when_first_char_mismatches() {
        // Line 470: `*pc == *tc` evaluates false → short-circuits to false.
        assert!(!glob_match("a", "b"));
        assert!(!glob_match("xa", "yb"));
    }

    // --- Additional coverage: load_referenced_ids early return ---

    #[test]
    fn load_referenced_ids_returns_empty_when_edge_types_empty() {
        // Line 285-287: `if self.config.edge_types.is_empty() { return Ok(HashSet::new()) }`
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
        create_calls_edge(&kit, "e1", "f_a", "f_b", "demo");

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            edge_types: vec![],
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let referenced = detector
            .load_referenced_ids("demo")
            .expect("load_referenced_ids");
        assert!(
            referenced.is_empty(),
            "empty edge_types → empty referenced set"
        );
    }

    // --- Additional coverage: Method label iteration in is_exported / is_ffi_entry ---

    /// Creates a Method node with `isExported` and `signature` flags.
    #[allow(clippy::too_many_arguments)]
    fn create_method_with_flags(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
        is_exported: bool,
        signature: &str,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Method {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '{}', returnType: '', \
             isExported: {}, docstring: '', content: '', parameterCount: 0, parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
            escape_cypher_string(signature),
            is_exported,
        );
        storage.execute(&cypher).expect("create method with flags");
    }

    #[test]
    fn batch_prefetch_exported_ids_includes_method_label() {
        // Replaces `is_exported_checks_method_label` (arch-1). Verifies
        // `BatchPrefetch::load` picks up exported Method nodes, not just
        // Function nodes.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method_with_flags(
            &kit,
            "m_pub",
            "demo",
            "method_pub",
            "demo.Class.method_pub",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        create_method(
            &kit,
            "m_priv",
            "demo",
            "method_priv",
            "demo.Class.method_priv",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(
            prefetch.is_exported("m_pub"),
            "m_pub should be exported (Method label)"
        );
        assert!(
            !prefetch.is_exported("m_priv"),
            "m_priv should NOT be exported (Method label)"
        );
    }

    #[test]
    fn batch_prefetch_ffi_entry_ids_includes_method_label() {
        // Replaces `is_ffi_entry_checks_method_label` (arch-1).
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method_with_flags(
            &kit,
            "m_ffi",
            "demo",
            "method_ffi",
            "demo.Class.method_ffi",
            "/src/lib.rs",
            1,
            false,
            r#"extern "C" fn method_ffi()"#,
        );
        create_method(
            &kit,
            "m_plain",
            "demo",
            "method_plain",
            "demo.Class.method_plain",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(
            prefetch.ffi_entry_ids.contains("m_ffi"),
            "m_ffi should be FFI entry (Method label)"
        );
        assert!(
            !prefetch.ffi_entry_ids.contains("m_plain"),
            "m_plain should NOT be FFI entry (Method label)"
        );
    }

    #[test]
    fn detect_excludes_exported_method_nodes() {
        // Integration: an exported Method with zero incoming edges is NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method_with_flags(
            &kit,
            "m_pub",
            "demo",
            "method_pub",
            "demo.Class.method_pub",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        create_method(
            &kit,
            "m_priv",
            "demo",
            "method_priv",
            "demo.Class.method_priv",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"method_pub"),
            "exported Method should NOT be dead"
        );
        assert!(
            names.contains(&"method_priv"),
            "non-exported Method should be dead"
        );
    }

    #[test]
    fn detect_excludes_ffi_method_nodes() {
        // Integration: a Method with FFI signature and zero incoming edges is NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method_with_flags(
            &kit,
            "m_ffi",
            "demo",
            "method_ffi",
            "demo.Class.method_ffi",
            "/src/lib.rs",
            1,
            false,
            r#"extern "C" fn method_ffi()"#,
        );
        create_method(
            &kit,
            "m_plain",
            "demo",
            "method_plain",
            "demo.Class.method_plain",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"method_ffi"),
            "FFI Method should NOT be dead"
        );
        assert!(
            names.contains(&"method_plain"),
            "non-FFI Method should be dead"
        );
    }

    // --- B0: #tests disambiguator recognition (CalNexus 66% false positives) ---

    #[test]
    fn detect_excludes_functions_inside_mod_tests_block() {
        // B0 fix: In Rust, `mod tests { fn foo() {} }` produces a QN with
        // `#tests` disambiguator (e.g. `demo.src.lib.rs.foo#tests`). These are
        // test-module-scoped functions and should NOT be flagged as dead.
        // This was the largest false-positive source on CalNexus (239/360 = 66%).
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_tests_foo",
            "demo",
            "foo",
            "demo.src.lib.rs.foo#tests",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_tests_bar",
            "demo",
            "bar",
            "demo.src.lib.rs.bar#tests_ConfigurableMockDomain",
            "/src/lib.rs",
            10,
        );
        // Control: a non-test function with no incoming edges IS dead.
        create_function(
            &kit,
            "f_plain",
            "demo",
            "plain",
            "demo.plain",
            "/src/lib.rs",
            20,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"foo"),
            "foo inside mod tests (#tests disambiguator) should NOT be dead"
        );
        assert!(
            !names.contains(&"bar"),
            "bar inside mod tests (#tests_ConfigurableMockDomain) should NOT be dead"
        );
        assert!(names.contains(&"plain"), "plain (no disambiguator) is dead");
    }

    // --- B2: expanded test patterns (it_*/sec_*/snap_*/perf_*/bench_*) ---

    #[test]
    fn detect_excludes_expanded_test_prefix_patterns() {
        // B2 fix: CalNexus uses it_*/sec_*/snap_*/perf_*/bench_* prefixes
        // for integration/security/snapshot/performance/benchmark tests.
        // DEFAULT_TEST_PATTERNS must cover these.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_it",
            "demo",
            "it_cli_001",
            "demo.it_cli_001",
            "/tests/cli.rs",
            1,
        );
        create_function(
            &kit,
            "f_sec",
            "demo",
            "sec_001_injection",
            "demo.sec_001_injection",
            "/tests/sec.rs",
            10,
        );
        create_function(
            &kit,
            "f_snap",
            "demo",
            "snap_001_diff",
            "demo.snap_001_diff",
            "/tests/snap.rs",
            20,
        );
        create_function(
            &kit,
            "f_perf",
            "demo",
            "perf_001_baseline",
            "demo.perf_001_baseline",
            "/tests/perf.rs",
            30,
        );
        create_function(
            &kit,
            "f_bench",
            "demo",
            "bench_decode_small",
            "demo.bench_decode_small",
            "/benches/decode.rs",
            40,
        );
        // Control: a non-test function with no incoming edges IS dead.
        create_function(
            &kit,
            "f_plain",
            "demo",
            "plain",
            "demo.plain",
            "/src/lib.rs",
            50,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"it_cli_001"), "it_* should NOT be dead");
        assert!(
            !names.contains(&"sec_001_injection"),
            "sec_* should NOT be dead"
        );
        assert!(
            !names.contains(&"snap_001_diff"),
            "snap_* should NOT be dead"
        );
        assert!(
            !names.contains(&"perf_001_baseline"),
            "perf_* should NOT be dead"
        );
        assert!(
            !names.contains(&"bench_decode_small"),
            "bench_* should NOT be dead"
        );
        assert!(names.contains(&"plain"), "plain (no test prefix) is dead");
    }

    // --- B3: trait impl method recognition ---

    #[test]
    fn detect_excludes_trait_impl_methods_when_dynamic_dispatch_enabled() {
        // B3 fix: Trait impl methods (e.g. `impl Display for X { fn fmt() {} }`)
        // produce Method nodes with disambiguator `#Display`, `#ReplHelper`, etc.
        // These are called via dynamic dispatch and should NOT be flagged as dead
        // when check_dynamic_dispatch=true.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method(
            &kit,
            "m_fmt",
            "demo",
            "fmt",
            "demo.src.lib.rs.fmt#Display",
            "/src/lib.rs",
            5,
        );
        create_method(
            &kit,
            "m_complete",
            "demo",
            "complete",
            "demo.src.repl.rs.complete#ReplHelper",
            "/src/repl.rs",
            15,
        );
        // Control: a non-trait-impl method with no incoming edges IS dead.
        create_method(
            &kit,
            "m_plain",
            "demo",
            "plain_method",
            "demo.src.lib.rs.plain_method",
            "/src/lib.rs",
            25,
        );

        let storage = storage(&kit);
        let config = DeadCodeConfig {
            check_dynamic_dispatch: true,
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, config);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"fmt"),
            "trait impl fmt#Display should NOT be dead when check_dynamic_dispatch=true"
        );
        assert!(
            !names.contains(&"complete"),
            "trait impl complete#ReplHelper should NOT be dead when check_dynamic_dispatch=true"
        );
        assert!(
            names.contains(&"plain_method"),
            "non-trait-impl method is dead"
        );
    }

    #[test]
    fn detect_flags_trait_impl_methods_when_dynamic_dispatch_disabled() {
        // B3: when check_dynamic_dispatch=false (opt-out), trait impl methods
        // ARE flagged as dead. Default is `true` since B3.5, so we must
        // explicitly disable it here.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method(
            &kit,
            "m_fmt",
            "demo",
            "fmt",
            "demo.src.lib.rs.fmt#Display",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let config = DeadCodeConfig {
            check_dynamic_dispatch: false,
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, config);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"fmt"),
            "trait impl fmt#Display IS dead when check_dynamic_dispatch=false (opt-out)"
        );
    }

    // --- B4: integration test file recognition ---

    #[test]
    fn is_integration_test_file_recognizes_rust_tests_dir() {
        // Rust top-level integration tests live in `tests/` directory.
        assert!(is_integration_test_file("tests/numerical_linalg_test.rs"));
        assert!(is_integration_test_file("tests/repl_integration.rs"));
        assert!(is_integration_test_file("tests/helpers/mod.rs"));
    }

    #[test]
    fn is_integration_test_file_recognizes_python_tests_dir() {
        // Python tests live in `tests/` or `test/` directory.
        assert!(is_integration_test_file("tests/test_foo.py"));
        assert!(is_integration_test_file("test/test_bar.py"));
        assert!(is_integration_test_file("src/tests/test_baz.py"));
    }

    #[test]
    fn is_integration_test_file_recognizes_jvm_src_test_dir() {
        // Java/Kotlin/Scala tests live in `src/test/`.
        assert!(is_integration_test_file(
            "src/test/java/com/example/FooTest.java"
        ));
        assert!(is_integration_test_file("src/test/kotlin/FooTest.kt"));
    }

    #[test]
    fn is_integration_test_file_rejects_production_source() {
        // Production source files must NOT be flagged as integration tests.
        assert!(!is_integration_test_file("src/lib.rs"));
        assert!(!is_integration_test_file("src/main.rs"));
        assert!(!is_integration_test_file("src/cli.rs"));
        assert!(!is_integration_test_file("src/domains/numerical.rs"));
    }

    #[test]
    fn is_integration_test_file_handles_windows_paths() {
        // Windows-style paths should be normalized and recognized.
        assert!(is_integration_test_file("tests\\foo.rs"));
        assert!(is_integration_test_file("src\\tests\\bar.py"));
    }

    #[test]
    fn detect_excludes_integration_test_functions_in_tests_dir() {
        // B4 fix: Functions in `tests/` directory are integration tests
        // discovered by `cargo test` / `pytest` / `go test`. They have no
        // static CALLS edge and should NOT be flagged as dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Integration test with descriptive name (no test_*/it_* prefix).
        create_function(
            &kit,
            "f_it",
            "demo",
            "eig_end_to_end_returns_json_with_values_and_vectors",
            "demo.tests.numerical_linalg_test.rs.eig_end_to_end_returns_json_with_values_and_vectors",
            "tests/numerical_linalg_test.rs",
            58,
        );
        create_function(
            &kit,
            "f_it2",
            "demo",
            "repl_infrastructure_present",
            "demo.tests.repl_integration.rs.repl_infrastructure_present",
            "tests/repl_integration.rs",
            157,
        );
        // Control: a production function with no incoming edges IS dead.
        create_function(
            &kit,
            "f_prod",
            "demo",
            "unused_helper",
            "demo.src.lib.rs.unused_helper",
            "src/lib.rs",
            100,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"eig_end_to_end_returns_json_with_values_and_vectors"),
            "integration test in tests/ should NOT be dead (B4)"
        );
        assert!(
            !names.contains(&"repl_infrastructure_present"),
            "integration test in tests/ should NOT be dead (B4)"
        );
        assert!(
            names.contains(&"unused_helper"),
            "production function with no incoming edges IS dead"
        );
    }

    // ===== B5: worklist reachability propagation =====

    /// B5 core: verifies that worklist propagation marks indirectly
    /// reachable functions as live, while unreachable functions (even if
    /// they have incoming edges from dead functions) are correctly flagged
    /// as dead.
    ///
    /// Graph:
    /// ```text
    /// entry -> a -> b   (reachable chain from entry seed)
    /// c -> d            (unreachable: c is not a seed, d has incoming
    ///                    edge from c but c itself is dead)
    /// ```
    ///
    /// Without B5 (single-layer `referenced_ids` check), `d` would be
    /// incorrectly marked live because `c -> d` exists. With B5, `d` is
    /// dead because `c` is unreachable from any seed.
    #[test]
    fn test_reachability_propagation_basic() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // entry -> a -> b (reachable chain)
        create_function(
            &kit,
            "f_entry",
            "demo",
            "entry",
            "demo.entry",
            "/src/main.rs",
            1,
        );
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 10);
        // c -> d (unreachable chain)
        create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/lib.rs", 20);
        create_function(&kit, "f_d", "demo", "d", "demo.d", "/src/lib.rs", 30);
        create_file(&kit, "file1", "demo", "/src/main.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/lib.rs", "rust");
        create_calls_edge(&kit, "e1", "f_entry", "f_a", "demo");
        create_calls_edge(&kit, "e2", "f_a", "f_b", "demo");
        create_calls_edge(&kit, "e3", "f_c", "f_d", "demo");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["entry"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        // Reachable from entry: entry (seed), a (entry->a), b (a->b).
        assert!(!names.contains(&"entry"), "entry is seed: {:?}", names);
        assert!(!names.contains(&"a"), "a reachable from entry: {:?}", names);
        assert!(!names.contains(&"b"), "b reachable via a: {:?}", names);
        // Unreachable: c (no incoming edges, not a seed), d (only reachable
        // via c which is itself dead).
        assert!(names.contains(&"c"), "c not reachable: {:?}", names);
        assert!(
            names.contains(&"d"),
            "B5: d is dead even though c->d exists (c is unreachable): {:?}",
            names
        );
    }

    /// B5: trait impl methods are seeds when `check_dynamic_dispatch=true`.
    /// Verifies the trait impl method is in `live_set` and any function it
    /// calls is also reachable.
    #[test]
    fn test_reachability_with_trait_impl() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Trait impl method (B3 seed) calls a free function.
        // qualified_name has `#Display` disambiguator.
        create_method(
            &kit,
            "m_fmt",
            "demo",
            "fmt",
            "demo.src.lib.rs.fmt#Display",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_helper",
            "demo",
            "helper",
            "demo.helper",
            "/src/lib.rs",
            10,
        );
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        create_calls_edge(&kit, "e1", "m_fmt", "f_helper", "demo");

        let storage = storage(&kit);
        // Default config has check_dynamic_dispatch=true (B3.5).
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        // Trait impl method is a seed (B3).
        assert!(
            !names.contains(&"fmt"),
            "B5: trait impl method fmt#Display is seed: {:?}",
            names
        );
        // helper is reachable from fmt (seed) via CALLS edge.
        assert!(
            !names.contains(&"helper"),
            "B5: helper reachable from trait impl seed: {:?}",
            names
        );
    }

    /// B5: private unused functions (no incoming edges, not a seed) are
    /// correctly flagged as dead. This is the most basic case — verifies
    /// the analyzer does not over-approximate the live set.
    #[test]
    fn test_reachability_excludes_private_unused() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Private unused function (no incoming edges, not exported, not FFI,
        // not a test, not an entry, not a trait impl).
        create_function(
            &kit,
            "f_unused",
            "demo",
            "unused",
            "demo.unused",
            "/src/lib.rs",
            1,
        );
        // main is the entry seed.
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/main.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"unused"),
            "B5: private unused function IS dead: {:?}",
            names
        );
        assert!(
            !names.contains(&"main"),
            "B5: main is entry seed, NOT dead: {:?}",
            names
        );
    }

    // ===== perf-1 + arch-1: BatchPrefetch + DRY consolidation =====

    /// perf-1: `BatchPrefetch::load` returns ALL exported function ids in
    /// a single Cypher query (vs the previous 2N pattern where
    /// `is_exported` was called per function with 2 label queries each).
    #[test]
    fn batch_prefetch_loads_all_exported_ids_in_one_query() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Create 5 functions, 3 of them marked isExported=true.
        create_function_with_flags(
            &kit,
            "f1",
            "demo",
            "f1",
            "demo.f1",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        create_function_with_flags(
            &kit,
            "f2",
            "demo",
            "f2",
            "demo.f2",
            "/src/lib.rs",
            5,
            true,
            "",
        );
        create_function_with_flags(
            &kit,
            "f3",
            "demo",
            "f3",
            "demo.f3",
            "/src/lib.rs",
            10,
            true,
            "",
        );
        create_function_with_flags(
            &kit,
            "f4",
            "demo",
            "f4",
            "demo.f4",
            "/src/lib.rs",
            15,
            false,
            "",
        );
        create_function_with_flags(
            &kit,
            "f5",
            "demo",
            "f5",
            "demo.f5",
            "/src/lib.rs",
            20,
            false,
            "",
        );
        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(prefetch.is_exported("f1"));
        assert!(prefetch.is_exported("f2"));
        assert!(prefetch.is_exported("f3"));
        assert!(!prefetch.is_exported("f4"), "f4 is not exported");
        assert!(!prefetch.is_exported("f5"), "f5 is not exported");
        assert_eq!(prefetch.exported_ids.len(), 3);
    }

    /// perf-1: `BatchPrefetch::load` returns ALL FFI entry ids in a single
    /// Cypher query (vs the previous 2N pattern).
    #[test]
    fn batch_prefetch_loads_all_ffi_entries_in_one_query() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // 3 FFI entries with different FFI markers.
        create_function_with_flags(
            &kit,
            "f1",
            "demo",
            "f1",
            "demo.f1",
            "/src/lib.rs",
            1,
            false,
            r#"pub extern "C" fn f1(x: i32) -> i32"#,
        );
        create_function_with_flags(
            &kit,
            "f2",
            "demo",
            "f2",
            "demo.f2",
            "/src/lib.rs",
            5,
            false,
            "#[no_mangle]\npub fn f2() -> i32",
        );
        create_function_with_flags(
            &kit,
            "f3",
            "demo",
            "f3",
            "demo.f3",
            "/src/lib.rs",
            10,
            false,
            r#"extern "C" { fn f3(); }"#,
        );
        // 1 non-FFI function as control.
        create_function_with_flags(
            &kit,
            "f4",
            "demo",
            "f4",
            "demo.f4",
            "/src/lib.rs",
            15,
            false,
            "pub fn f4() {}",
        );
        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        assert!(prefetch.ffi_entry_ids.contains("f1"), "f1 has extern \"C\"");
        assert!(prefetch.ffi_entry_ids.contains("f2"), "f2 has #[no_mangle]");
        assert!(prefetch.ffi_entry_ids.contains("f3"), "f3 has extern \"C\"");
        assert!(!prefetch.ffi_entry_ids.contains("f4"), "f4 is not FFI");
        assert_eq!(prefetch.ffi_entry_ids.len(), 3);
    }

    /// perf-1: `BatchPrefetch::load` returns ALL outgoing edges grouped by
    /// source id, so `propagate()` can do an O(1) HashMap lookup per pop
    /// instead of an O(1) Cypher round-trip per pop.
    #[test]
    fn batch_prefetch_loads_outgoing_edges_grouped_by_source() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
        create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/lib.rs", 10);
        // f_a calls f_b and f_c (2 outgoing edges from f_a).
        create_calls_edge(&kit, "e1", "f_a", "f_b", "demo");
        create_calls_edge(&kit, "e2", "f_a", "f_c", "demo");
        // f_b calls f_c (1 outgoing edge from f_b).
        create_calls_edge(&kit, "e3", "f_b", "f_c", "demo");
        let storage = storage(&kit);
        let config = DeadCodeConfig::default();
        let functions = load_all_functions(&*storage, "demo").expect("functions");
        let prefetch =
            BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
        let a_targets = prefetch
            .outgoing_edges("f_a")
            .expect("f_a has outgoing edges");
        assert_eq!(a_targets.len(), 2, "f_a calls f_b and f_c");
        assert!(a_targets.contains(&"f_b".to_string()));
        assert!(a_targets.contains(&"f_c".to_string()));
        let b_targets = prefetch
            .outgoing_edges("f_b")
            .expect("f_b has outgoing edges");
        assert_eq!(b_targets.len(), 1, "f_b calls f_c");
        assert!(b_targets.contains(&"f_c".to_string()));
        assert!(
            prefetch.outgoing_edges("f_c").is_none(),
            "f_c has no outgoing edges"
        );
    }

    /// perf-1 + arch-1 regression: `detect()` returns correct results after
    /// batch prefetch refactor. Re-verifies B5 reachability propagation
    /// with batch-prefetched edges (3-query pattern instead of 4N+V).
    ///
    /// Graph:
    /// ```text
    /// entry -> a -> b -> c   (reachable chain from entry seed)
    /// d                       (unreachable: no incoming edges, not a seed)
    /// ```
    #[test]
    fn detect_uses_batch_prefetch_for_reachability_propagation() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_entry",
            "demo",
            "entry",
            "demo.entry",
            "/src/main.rs",
            1,
        );
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 10);
        create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/lib.rs", 20);
        // d is unreachable (no incoming edges, not a seed).
        create_function(&kit, "f_d", "demo", "d", "demo.d", "/src/lib.rs", 30);
        create_file(&kit, "file1", "demo", "/src/main.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/lib.rs", "rust");
        create_calls_edge(&kit, "e1", "f_entry", "f_a", "demo");
        create_calls_edge(&kit, "e2", "f_a", "f_b", "demo");
        create_calls_edge(&kit, "e3", "f_b", "f_c", "demo");
        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["entry"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"entry"), "entry is seed");
        assert!(!names.contains(&"a"), "a reachable from entry");
        assert!(!names.contains(&"b"), "b reachable via a");
        assert!(!names.contains(&"c"), "c reachable via b");
        assert!(names.contains(&"d"), "d is unreachable (dead)");
    }

    /// perf-1 + arch-1 regression: `detect()` correctly identifies
    /// exported functions as live when they have no incoming edges
    /// (using batch-prefetched `exported_ids`).
    #[test]
    fn detect_uses_batch_prefetch_for_exported_function_liveness() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `pub_exported` is marked isExported=true and has no incoming
        // CALLS edges. With `check_exported=true` (default), it should be
        // treated as a seed and not flagged as dead.
        create_function_with_flags(
            &kit,
            "f_pub",
            "demo",
            "pub_exported",
            "demo.pub_exported",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        // `unused_private` is not exported, has no incoming edges, is not
        // a test/entry/trait-impl — IS dead.
        create_function(
            &kit,
            "f_priv",
            "demo",
            "unused_private",
            "demo.unused_private",
            "/src/lib.rs",
            10,
        );
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"pub_exported"),
            "exported function is a seed (live)"
        );
        assert!(
            names.contains(&"unused_private"),
            "non-exported function with no incoming edges IS dead"
        );
    }
}
