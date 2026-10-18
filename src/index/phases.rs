// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Typed [`Phase`] implementations for the indexing pipeline.
//!
//! Refactors the 9-step sequence in [`super::pipeline`] into 6 typed phases
//! executed by the [`DagPipeline`](super::pipeline_dag::Pipeline) runner:
//!
//! 1. [`ScanPhase`] — discover files, lookup/create project, diff hashes.
//! 2. [`ParsePhase`] — parallel-parse changed+added files.
//! 3. [`ScopeResolutionPhase`] — build in-memory graph (nodes + per-file edges).
//! 4. [`ResolvePhase`] — resolve calls/dataflow/FFI edges.
//! 5. [`ConfidencePhase`] — pass-through (real confidence scoring not yet implemented).
//! 6. [`LoadPhase`] — persist nodes/edges to the database, build [`IndexResult`].
//!
//! # Input wiring
//!
//! [`ScanPhase`] is the root phase: its typed [`ScanInput`] is inserted into
//! [`PipelineCtx`] externally. All other phases use `Input = ()` and read dep
//! outputs from the context via [`PipelineCtx::get`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, warn};

use crate::discover::{FileInfo, Walker};
use crate::index::error::IndexError;
use crate::index::incremental::{diff_files_with_hints, FileDiff, FileHashHint};
use crate::index::pipeline::{build_file_nodes, now_unix_seconds, with_retry, DEFAULT_MAX_RETRIES};
use crate::ir::ExtractResult;
use crate::model::{
    new_project_id, ConfidenceTier, Edge, EdgeType, Graph, Language, Node, NodeLabel,
};
use crate::parse::parallel::{parallel_parse, parallel_parse_ram_first, RamFirstSources};
use crate::resolve::{build_symbol_table, resolve_all, resolve_include, IncludesGraph};
use crate::storage::Repository;

use super::pipeline::IndexResult;
use super::pipeline_dag::{Phase, PhaseError, PipelineCtx};

/// Confidence for an INCLUDES edge (structural, explicit in syntax).
/// Matches the lower bound of `EdgeType::Includes::confidence_range()` = (0.95, 1.0).
const CONFIDENCE_INCLUDES: f32 = 0.95;

// ---------------------------------------------------------------------------
// Phase I/O structs
// ---------------------------------------------------------------------------

/// Typed input for [`ScanPhase`] (root phase, externally provided).
pub struct ScanInput {
    /// Repository root to index.
    pub path: PathBuf,
    /// Project display name (also used as DB project key).
    pub project_name: String,
    /// When `true`, every disk file is re-parsed regardless of hash.
    pub force: bool,
    /// Pipeline start time (for duration calculation in [`LoadPhase`]).
    pub start: Instant,
}

/// Typed output of [`ScanPhase`], consumed by [`ParsePhase`] and [`LoadPhase`].
pub struct ScanOutput {
    /// The project id (existing or newly generated).
    pub project_id: String,
    /// The project display name.
    pub project_name: String,
    /// The repository root path.
    pub root_path: PathBuf,
    /// All files discovered on disk.
    pub disk_files: Vec<FileInfo>,
    /// The diff of changed/added/deleted/unchanged files.
    pub diff: FileDiff,
    /// Pipeline start time (passed through to [`LoadPhase`]).
    pub start: Instant,
}

/// Typed output of [`ParsePhase`], consumed by [`ScopeResolutionPhase`] and
/// [`ResolvePhase`].
pub struct ParseOutput {
    /// Per-file extraction results.
    pub results: Vec<ExtractResult>,
    /// Per-file parse errors (file path, error message).
    pub errors: Vec<(String, String)>,
    /// Number of files successfully parsed.
    pub files_parsed: usize,
    /// The files that were parsed (changed + added).
    pub to_parse: Vec<FileInfo>,
}

/// Typed output of [`ScopeResolutionPhase`], consumed by [`ResolvePhase`].
pub struct ScopeOutput {
    /// The in-memory graph — single source of truth for nodes + edges
    /// (L3 memory-overflow fix: removed `all_nodes`/`all_edges` duplicates).
    pub graph: Graph,
    /// Mapping from absolute file path → relative path (for normalizing
    /// filePath fields on Parameter/Variable nodes in [`ResolvePhase`]).
    pub path_to_rel: HashMap<String, String>,
}

/// Typed output of [`ResolvePhase`], consumed by [`LoadPhase`].
pub struct ResolveOutput {
    /// The in-memory graph — single source of truth for nodes + edges
    /// (L3 memory-overflow fix: removed `all_nodes`/`all_edges` duplicates;
    /// `graph` is the only copy, with resolved edges added and dangling
    /// type edges pruned in place by `resolve_all`).
    pub graph: Graph,
    /// Number of files parsed (for IndexResult).
    pub files_parsed: usize,
    /// Number of files skipped (for IndexResult).
    pub files_skipped: usize,
    /// C++ `#include` graph for scope-aware call resolution.
    /// Populated by `build_includes_edges`; consumed by `CallResolver`.
    pub includes_graph: IncludesGraph,
}

/// Typed output of [`LoadPhase`], extracted by [`Pipeline::run`].
pub struct LoadOutput {
    /// The final indexing result.
    pub index_result: IndexResult,
}

// ---------------------------------------------------------------------------
// Helper: convert IndexError to PhaseError
// ---------------------------------------------------------------------------

/// Boxes an [`IndexError`] into a [`PhaseError::ExecutionFailed`] so the
/// pipeline runner can carry it to the caller, which downcasts it back
/// (preserving the exact variant and exit code).
fn phase_err(phase: &'static str, e: IndexError) -> PhaseError {
    PhaseError::ExecutionFailed {
        phase,
        inner: Box::new(e),
    }
}

// ---------------------------------------------------------------------------
// Phase 1: ScanPhase
// ---------------------------------------------------------------------------

/// Phase 1: discover files, lookup/create project id, diff hashes.
///
/// Replaces original steps 1–3 of the pipeline.
pub struct ScanPhase {
    /// Shared repository handle (Arc-cloned from Pipeline).
    pub repo: Arc<Repository>,
    /// Daemon fast-path hints (path → size/mtime/hash of the previous run).
    /// `Some` on the daemon's long-lived `IndexFacade` so consecutive
    /// incremental batches skip re-reading unchanged files; `None` for
    /// one-shot CLI runs (empty map semantics).
    pub hints: Option<Arc<std::sync::Mutex<HashMap<String, FileHashHint>>>>,
}

impl Phase for ScanPhase {
    type Input = ScanInput;
    type Output = ScanOutput;
    const NAME: &'static str = "scan";
    fn deps() -> &'static [&'static str] {
        &[]
    }

    fn run(&self, input: Self::Input, _ctx: &mut PipelineCtx) -> Result<Self::Output, PhaseError> {
        let ScanInput {
            path,
            project_name,
            force,
            start,
        } = input;

        // Step 1: discover files on disk.
        let disk_files = Walker::new(&path)
            .discover()
            .map_err(|e| phase_err(Self::NAME, IndexError::from(e)))?;

        // Assign a project id — reuse existing or generate new.
        let project_id = lookup_or_create_project_id(&self.repo, &project_name)
            .map_err(|e| phase_err(Self::NAME, e))?;

        // Step 2: query existing hashes from the DB (with retry on lock).
        let db_hashes = with_retry(DEFAULT_MAX_RETRIES, || {
            self.repo
                .get_all_file_hashes(&project_id)
                .map_err(IndexError::from)
        })
        .unwrap_or_default();

        // Step 3: diff hashes → changed/added/deleted/unchanged, reusing
        // (size, mtime) hints from the previous run when available.
        let hint_map = self
            .hints
            .as_ref()
            .and_then(|m| m.lock().ok().map(|guard| guard.clone()));
        let (diff, fresh_hints) = diff_files_with_hints(
            &disk_files,
            &db_hashes,
            force,
            &hint_map.unwrap_or_default(),
        )
        .map_err(|e| phase_err(Self::NAME, IndexError::Io(e)))?;
        // Publish fresh hints for the next incremental batch.
        if let Some(m) = &self.hints {
            if let Ok(mut guard) = m.lock() {
                *guard = fresh_hints;
            }
        }

        Ok(ScanOutput {
            project_id,
            project_name,
            root_path: path,
            disk_files,
            diff,
            start,
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 2: ParsePhase
// ---------------------------------------------------------------------------

/// Phase 2: parallel-parse changed + added files.
///
/// Replaces original step 4 of the pipeline. Parse failures are logged and
/// skipped (PRD §4.1.6) — they do not abort the pipeline.
///
/// # RAM-first mode (H15)
///
/// When the caller ([`IndexFacade::index_ram_first`]) provides LZ4-compressed
/// source bytes via [`PipelineCtx`], files are decompressed from in-memory
/// buffers instead of read from disk. The buffers are built by
/// [`IndexFacade::index_ram_first`] before the DAG runs and inserted into the
/// context under [`ParsePhase::RAM_FIRST_KEY`].
///
/// # L7-1 memory-overflow fix
///
/// The compressed buffers are NO LONGER held as a `ParsePhase` struct field.
/// Previously `ram_first_compressed: Option<RamFirstSources>` was moved into
/// `ParsePhase` at registration time and held until `dag.run()` returned (i.e.
/// until LoadPhase finished). For a 10k-file repository this kept ~30-40 GB
/// of LZ4-compressed source bytes alive long after ParsePhase::run had
/// finished using them.
///
/// The buffers now live in [`PipelineCtx`] under [`ParsePhase::RAM_FIRST_KEY`]
/// and are removed via `ctx.remove` at the END of `ParsePhase::run`, so they
/// are dropped immediately after parsing — before ScopeResolutionPhase,
/// ResolvePhase, and LoadPhase run. This cuts peak RSS by ~30-40 GB for
/// large repositories.
///
/// [`IndexFacade::index_ram_first`]: super::pipeline::IndexFacade::index_ram_first
#[derive(Default)]
pub struct ParsePhase;

impl ParsePhase {
    /// [`PipelineCtx`] key under which the caller stores the optional
    /// LZ4-compressed source buffers (`Option<RamFirstSources>`) for RAM-first
    /// mode. [`ParsePhase::run`] removes this entry when it finishes so the
    /// buffers are dropped before subsequent phases run (L7-1).
    pub const RAM_FIRST_KEY: &'static str = "ram_first_compressed";
}

impl Phase for ParsePhase {
    type Input = ();
    type Output = ParseOutput;
    const NAME: &'static str = "parse";
    fn deps() -> &'static [&'static str] {
        &["scan"]
    }

    fn run(&self, _: Self::Input, ctx: &mut PipelineCtx) -> Result<Self::Output, PhaseError> {
        // L7-1 memory-overflow fix: take ownership of the compressed buffers
        // via `ctx.remove` (NOT `ctx.get`) so they are dropped at the end of
        // this `run` call. Previously the buffers were a `ParsePhase` struct
        // field held until `dag.run()` returned (i.e. until LoadPhase
        // finished), keeping ~30-40 GB of LZ4-compressed source bytes alive
        // for 4 phases longer than necessary.
        //
        // `flatten()` turns `Option<Option<RamFirstSources>>` (outer from
        // `remove`, inner from the stored `Option`) into `Option<RamFirstSources>`.
        // When the key is absent (non-RAM-first mode never inserts it) or the
        // stored value is `None`, `compressed` is `None` → streaming disk-read
        // path is used.
        //
        // Order: `remove` MUST precede the immutable `get` of `scan` below —
        // mixing a mutable borrow (`remove`) with an active immutable borrow
        // (`get`) triggers E0502. This mirrors the borrow-ordering established
        // by ResolvePhase::run (L6-1).
        let compressed = ctx
            .remove::<Option<RamFirstSources>>(Self::RAM_FIRST_KEY)
            .flatten();

        let scan = ctx
            .get::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;

        // Build the list of files to parse: changed + added.
        let mut to_parse: Vec<FileInfo> = scan.diff.changed.clone();
        to_parse.extend(scan.diff.added.iter().cloned());

        // H15: RAM-first path uses LZ4-compressed in-memory buffers; the
        // streaming path reads from disk.
        let parse_result = match compressed {
            Some(c) => parallel_parse_ram_first(&to_parse, &c, &scan.project_id),
            None => parallel_parse(&to_parse, &scan.project_id),
        };
        // `c` (the RamFirstSources) is dropped here at the end of `match`,
        // freeing ~30-40 GB for large repos before ScopeResolutionPhase runs.

        // PRD §4.1.6: parse failures are logged and skipped.
        for (file_path, error_msg) in &parse_result.errors {
            warn!(file = %file_path, error = %error_msg, "parse failed, skipping file");
        }

        Ok(ParseOutput {
            results: parse_result.results,
            errors: parse_result.errors,
            files_parsed: parse_result.files_parsed,
            to_parse,
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 3: ScopeResolutionPhase
// ---------------------------------------------------------------------------

/// Phase 3: build the in-memory graph (definition nodes + per-file edges).
///
/// Replaces original step 5 of the pipeline. Merges per-file extraction
/// results into a single [`Graph`], normalizing node ids to FQNs and
/// rewriting edge endpoints to match stored node ids.
pub struct ScopeResolutionPhase;

impl Phase for ScopeResolutionPhase {
    type Input = ();
    type Output = ScopeOutput;
    const NAME: &'static str = "scope";
    fn deps() -> &'static [&'static str] {
        &["scan", "parse"]
    }

    fn run(&self, _: Self::Input, ctx: &mut PipelineCtx) -> Result<Self::Output, PhaseError> {
        // L7-2 memory-overflow fix: take ownership of `ParseOutput` via
        // `ctx.remove` (instead of `ctx.get`) so we can clear the per-file
        // `edges` Vec after cloning them into the graph. The edges are never
        // read by any subsequent phase — `ResolvePhase` and its sub-resolvers
        // use `graph.edges`, `result.nodes`, and the intermediate records
        // (calls/imports/assignments/reads/writes), but NEVER `result.edges`
        // (verified via `grep -rn 'result\.edges\|r\.edges' src/resolve`).
        // For large repos (5 M edges × ~200 bytes) this frees ~1 GB of
        // duplicated edge data before ResolvePhase runs.
        //
        // Order: `remove` (mutable borrow) MUST precede `get` (immutable
        // borrow) to satisfy E0502 — same pattern as ResolvePhase::run (L6-1)
        // and ParsePhase::run (L7-1).
        let mut parse = ctx
            .remove::<ParseOutput>("parse")
            .ok_or(PhaseError::MissingInput("parse"))?;
        let scan = ctx
            .get::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;

        // Clone project_id so the immutable borrow of `scan` ends before the
        // `ctx.insert("parse", ...)` mutable borrow below. Pre-L7-2 used
        // `let project_id = &scan.project_id;` which kept `scan` (and thus
        // `ctx`) borrowed for the entire function, preventing `ctx.insert`.
        let project_id = scan.project_id.clone();
        let mut graph = Graph::new();

        // Add File nodes for every parsed file (used by incremental indexing).
        let file_nodes = build_file_nodes(&scan.diff, &project_id);
        for file_node in &file_nodes {
            graph.add_node(file_node.clone());
        }
        // `scan` is no longer used after this point — NLL releases the
        // immutable borrow of `ctx`, allowing `ctx.insert` later.

        // Build mapping from absolute file path → File node id (file_<uuid>).
        // rel→id map built once (first node wins, matching the previous inner
        // `break` semantics); the nested per-file scan made this O(F × F_nodes).
        let mut rel_to_file_id: HashMap<&str, &str> = HashMap::with_capacity(file_nodes.len());
        for fn_node in &file_nodes {
            rel_to_file_id
                .entry(fn_node.name.as_str())
                .or_insert(fn_node.id.as_str());
        }
        let mut path_to_file_id: HashMap<&str, &str> = HashMap::new();
        for file in parse.to_parse.iter() {
            let Some(abs) = file.path.to_str() else {
                continue;
            };
            if let Some(id) = rel_to_file_id.get(file.relative_path.as_str()) {
                path_to_file_id.insert(abs, id);
            }
        }

        // Build mapping from absolute file path → relative path.
        let path_to_rel: HashMap<String, String> = parse
            .to_parse
            .iter()
            .filter_map(|f| {
                f.path
                    .to_str()
                    .map(|p| (p.to_string(), f.relative_path.clone()))
            })
            .collect();

        // Merge per-file extraction results into the graph.
        // L3 memory-overflow fix: graph is the single source of truth; no
        // parallel `all_nodes`/`all_edges` Vecs are built. Nodes/edges are
        // moved into the graph (not cloned twice).
        for result in &parse.results {
            let rel_path = path_to_rel
                .get(result.file_path.as_str())
                .map(|s| s.as_str())
                .unwrap_or(result.file_path.as_str());

            // Build per-file remap: old node id (UUID) → new id (FQN).
            let mut id_remap: HashMap<String, String> = HashMap::new();
            for node in &result.nodes {
                let mut g = node.clone();
                if !matches!(
                    g.label,
                    NodeLabel::Project | NodeLabel::File | NodeLabel::Folder
                ) {
                    let old_id = g.id.clone();
                    g.id = node.qualified_name.clone();
                    if old_id != g.id {
                        id_remap.insert(old_id, g.id.clone());
                    }
                }
                if let Some(fp) = g.file_path.as_mut() {
                    *fp = rel_path.to_string();
                }
                if g.project.is_empty() {
                    g.project = project_id.clone();
                }
                graph.add_node(g);
            }
            for edge in &result.edges {
                let mut e = edge.clone();
                if let Some(file_id) = path_to_file_id.get(e.source.as_str()) {
                    e.source = (*file_id).to_string();
                }
                if let Some(new_target) = id_remap.get(&e.target) {
                    e.target = new_target.clone();
                }
                graph.add_edge(e);
            }
        }
        // `path_to_file_id` (which held `&str` borrows into `parse.to_parse`)
        // is no longer used — NLL releases the immutable borrow of
        // `parse.to_parse`, allowing `&mut parse.results` below.

        // L7-2: clear per-file edges and seen_qns from parse.results. The
        // edges have been cloned into `graph`; `seen_qns` was used only by
        // `push_node` during extraction. No subsequent phase reads either.
        // `clear()` + `shrink_to_fit()` releases the backing allocation,
        // not just the length — important for large repos where the Vec was
        // grown to capacity during extraction.
        for result in &mut parse.results {
            result.edges.clear();
            result.edges.shrink_to_fit();
            result.seen_qns.clear();
            result.seen_qns.shrink_to_fit();
        }

        // Put parse back into ctx for ResolvePhase (which removes it again
        // via `ctx.remove::<ParseOutput>("parse")` — L6-4).
        ctx.insert("parse", parse);

        Ok(ScopeOutput { graph, path_to_rel })
    }
}

// ---------------------------------------------------------------------------
// Helper: build INCLUDES edges for C++ #include
// ---------------------------------------------------------------------------

/// Builds `EdgeType::Includes` edges for C++ `#include` directives and
/// populates an [`IncludesGraph`] for downstream scope-aware resolution.
///
/// C++ `#include` is handled separately from other
/// languages' import statements. [`ImportResolver`] skips C++ results (no
/// IMPORTS edges for C++); this function builds INCLUDES edges instead.
/// Other languages (TS/Rust/Python/Go/Java) are unaffected — they continue
/// to produce IMPORTS edges via `ImportResolver`.
///
/// # Arguments
///
/// * `results` - Parse results (only C++ results are processed; others skipped).
/// * `graph` - The graph to add INCLUDES edges to (must contain File nodes
///   created by [`ScopeResolutionPhase`]).
/// * `path_to_rel` - Absolute→relative path mapping (from [`ScopeOutput`]).
/// * `project_id` - The project id for edge construction.
///
/// # Returns
///
/// An [`IncludesGraph`] populated for `lookup_exported_in_scope`. INCLUDES
/// edges are added directly to `graph` (moved, not cloned — fix).
fn build_includes_edges(
    results: &[ExtractResult],
    graph: &mut Graph,
    path_to_rel: &HashMap<String, String>,
    project_id: &str,
) -> IncludesGraph {
    let mut includes_graph = IncludesGraph::new();

    #[cfg(feature = "lang-cpp")]
    {
        // Build file_path → node_id index from File nodes (file_path is relative,
        // normalized by ScopeResolutionPhase). Used for both source and target
        // File node id lookup.
        let mut file_index: HashMap<String, String> = HashMap::new();
        for node in graph.nodes_by_label(NodeLabel::File) {
            if let Some(fp) = &node.file_path {
                file_index
                    .entry(fp.clone())
                    .or_insert_with(|| node.id.clone());
            }
        }

        // All relative file paths — used by resolve_include for suffix matching.
        // resolve_include matches #include paths as suffixes of these paths.
        let all_files: Vec<String> = file_index.keys().cloned().collect();

        // Build rel→abs reverse map for IncludesGraph keys. IncludesGraph stores
        // ABSOLUTE paths (matching SymbolEntry.file_path and CallResolver's
        // caller_file, both from result.file_path) so that lookup_exported_in_scope
        // can filter entries by file_path without format conversion.
        let mut rel_to_abs: HashMap<&String, &String> = HashMap::new();
        for (abs, rel) in path_to_rel {
            rel_to_abs.insert(rel, abs);
        }

        for result in results {
            // Only C++ #include produces INCLUDES edges.
            // C and other languages are handled by ImportResolver as IMPORTS.
            if result.language != Language::Cpp {
                continue;
            }

            // Convert absolute file_path to relative for File node lookup.
            // In tests, file_path may already be relative (path_to_rel won't
            // have it as a key, so fall back to the original).
            let source_rel = path_to_rel
                .get(&result.file_path)
                .cloned()
                .unwrap_or_else(|| result.file_path.clone());

            let Some(source_file_id) = file_index.get(&source_rel).cloned() else {
                warn!(
                    file = %result.file_path,
                    "INCLUDES source File node not found; skipping #include edges for this file"
                );
                continue;
            };

            // IncludesGraph source key: absolute path (result.file_path).
            // In tests where path_to_rel is empty, result.file_path is already
            // the "absolute" path (relative path used as-is).
            let source_abs = &result.file_path;

            for import in &result.imports {
                if import.source_file.is_empty() {
                    continue;
                }

                // resolve_include does suffix matching on all_files (relative
                // paths). Returns a relative path that exists in all_files.
                let Some(target_rel) =
                    resolve_include(&import.source_file, &source_rel, &all_files)
                else {
                    // System header (<iostream>) or external lib — no match.
                    // Not warned: system headers are common and expected.
                    continue;
                };

                // target_rel came from all_files (file_index keys), so this
                // lookup is guaranteed to succeed. The guard is defensive only.
                let Some(target_file_id) = file_index.get(&target_rel).cloned() else {
                    continue;
                };

                let edge = Edge::builder(
                    source_file_id.clone(),
                    target_file_id.clone(),
                    EdgeType::Includes,
                    project_id,
                )
                .confidence(CONFIDENCE_INCLUDES)
                .confidence_tier(ConfidenceTier::ImportScoped)
                .start_line(import.line)
                .build();
                // L6 memory-overflow fix: move `edge` into the graph (no
                // clone). The previous `graph.add_edge(edge.clone()); edges.push(edge);`
                // double-allocated every INCLUDES edge — the returned Vec was
                // immediately discarded by ResolvePhase. Now `build_includes_edges`
                // returns only `IncludesGraph`, so the edge is moved directly.
                graph.add_edge(edge);

                // IncludesGraph target key: convert target_rel back to absolute
                // via rel_to_abs map. In tests (path_to_rel empty), fallback to
                // target_rel (which is already the "absolute" path in test context).
                let target_abs = rel_to_abs
                    .get(&target_rel)
                    .map(|s| s.to_string())
                    .unwrap_or(target_rel.clone());
                includes_graph.add_include(source_abs, &target_abs);
            }
        }
    } // end #[cfg(feature = "lang-cpp")]

    #[cfg(not(feature = "lang-cpp"))]
    {
        let _ = (results, graph, path_to_rel, project_id);
    }

    includes_graph
}

// ---------------------------------------------------------------------------
// Phase 4: ResolvePhase
// ---------------------------------------------------------------------------

/// Phase 4: resolve symbols (calls + dataflow + FFI edges).
///
/// Replaces original step 6 of the pipeline. Clones the graph from
/// [`ScopeResolutionPhase`] (the context is immutable during `run`, so
/// mutation requires ownership) and runs the resolvers on it.
pub struct ResolvePhase {
    /// Shared repository handle (Arc-cloned from Pipeline) — used to load
    /// incremental enrichment (symbols of unchanged files) from the database.
    pub repo: Arc<Repository>,
}

impl Phase for ResolvePhase {
    type Input = ();
    type Output = ResolveOutput;
    const NAME: &'static str = "resolve";
    fn deps() -> &'static [&'static str] {
        &["scan", "parse", "scope"]
    }

    fn run(&self, _: Self::Input, ctx: &mut PipelineCtx) -> Result<Self::Output, PhaseError> {
        // L6 memory-overflow fix: take ownership of `ScopeOutput` via
        // `ctx.remove` instead of `ctx.get` + `graph.clone()`.
        //
        // `ResolvePhase` is the sole consumer of `scope` (LoadPhase deps are
        // `["scan", "resolve"]` — it does NOT read `scope`). The previous
        // `let mut graph = scope.graph.clone();` deep-cloned the entire
        // `Graph` (nodes HashMap + edges Vec), which for large repos is
        // multi-GB and was the dominant memory-pressure point after L1–L5.
        //
        // Removing `scope` from the ctx also frees `ScopeOutput.path_to_rel`
        // and the (now-moved) `Graph` for the remainder of the pipeline,
        // cutting peak RSS by roughly one full Graph worth of memory.
        //
        // Safety of `remove`: if a future phase adds `scope` to its `deps()`,
        // `topo_sort` will still order it after `resolve`, but the
        // `ctx.get::<ScopeOutput>("scope")` call in that phase will return
        // `None` → `MissingInput`. This is fail-loud and will be
        // caught the first time the new phase is exercised.
        //
        // Order: `remove` first so the mutable borrow ends before the
        // immutable `get` borrows below. Borrowing `scan`/`parse` first would
        // conflict with `remove`'s mutable borrow (E0502).
        let scope = ctx
            .remove::<ScopeOutput>("scope")
            .ok_or(PhaseError::MissingInput("scope"))?;
        // L6 memory-overflow fix: take ownership of `ParseOutput` via
        // `ctx.remove` so its `results: Vec<ExtractResult>` is freed when
        // `ResolvePhase` returns. `LoadPhase` deps are
        // `["scan", "resolve", "confidence"]` — it does NOT read `parse`, so
        // removing it here is safe. Keeping `parse` alive until pipeline end
        // holds every `ExtractResult` (AST snapshots, per-file node/edge
        // lists) in memory alongside the resolved `Graph`, doubling peak RSS
        // for large repos.
        //
        // Order: both `remove`s go first (mutable borrow), then `get` for
        // `scan` (immutable borrow) — mixing them triggers E0502.
        let parse = ctx
            .remove::<ParseOutput>("parse")
            .ok_or(PhaseError::MissingInput("parse"))?;

        let scan = ctx
            .get::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;

        let project_id = &scan.project_id;

        // L3 memory-overflow fix: graph is the single source of truth — no
        // parallel `all_nodes`/`all_edges` Vecs are cloned. Resolved edges
        // are added directly to `graph` by `build_includes_edges` and
        // `resolve_all`; dangling type edges are pruned in place by
        // `resolve_all`'s internal `prune_dangling_type_edges(graph)` call.
        //
        // fix: `graph` is MOVED out of `scope` (no clone). The previous
        // `scope.graph.clone()` was the largest single allocation in the
        // pipeline for large repos.
        let mut graph = scope.graph;

        // Build INCLUDES edges for C++ #include directives.
        // C++ #include is handled separately from IMPORTS (which is for
        // TS/Rust/Python/Go/Java) because #include establishes a scope-visible
        // relationship that CallResolver uses for cross-file call resolution
        // ImportResolver skips C++ results, so no IMPORTS edges
        // are created for C++ — only INCLUDES edges here.
        //
        // MUST run before resolve_all so the IncludesGraph is available to
        // CallResolver for scope-aware lookup_exported_in_scope.
        //
        // fix: `build_includes_edges` adds edges to `graph` directly; the
        // returned Vec is ignored (previously extended into `all_edges`).
        // fix: `build_includes_edges` now returns only `IncludesGraph`
        // (the Vec was always discarded — pure waste).
        let includes_graph =
            build_includes_edges(&parse.results, &mut graph, &scope.path_to_rel, project_id);

        // Resolve calls + dataflow + FFI edges. `TypeResolver::resolve_types`
        // internally builds a path mapping between `result.file_path` (absolute
        // in production) and graph nodes' `file_path` (relative, normalized by
        // ScopeResolutionPhase) so that `imports_map` and `lookup_in_file`
        // lookups succeed despite the path-format difference. See
        // TypeResolver::resolve_types for details.
        //
        // includes_graph is passed to CallResolver for scope-aware
        // call resolution Files with #include edges use
        // lookup_exported_in_scope; others use lookup_exported (backward compat).
        //
        // fix: `resolve_all` adds resolved edges to `graph` directly and
        // prunes dangling type edges via `prune_dangling_type_edges(graph)`.
        // fix: `resolve_all` now returns `()` — no master Vec<Edge> is
        // built. Each sub-resolver's Vec is dropped immediately after its
        // edges are cloned into `graph`.
        let mut symbol_table = build_symbol_table(&parse.results, project_id);

        // Incremental enrichment (cross-file edge-loss fix): on incremental
        // runs only changed/added files are parsed, so unchanged files'
        // symbols are absent from the symbol table and File/function ids are
        // absent from the graph. LoadPhase deletes all edges touching changed
        // files' nodes and the resolvers re-create them from the enriched
        // table — without enrichment, every incremental run net-lost
        // CALLS/DataFlows/Type edges from changed files into unchanged files
        // (plus IMPORTS edges targeting them). Enrichment loads those
        // symbols/ids from the DB, excluding the (stale) rows of
        // changed/added/deleted files. Enriched data stays OUT of the graph —
        // LoadPhase saves `graph.nodes_view()`, and re-persisting unchanged
        // files would defeat incremental indexing.
        let enrichment = {
            let mut excluded: std::collections::HashSet<String> =
                scan.diff.deleted.iter().cloned().collect();
            excluded.extend(scan.diff.changed.iter().map(|f| f.relative_path.clone()));
            excluded.extend(scan.diff.added.iter().map(|f| f.relative_path.clone()));

            let mut enr = crate::resolve::ResolveEnrichment::default();
            let file_rows = match self.repo.get_file_id_rows(project_id) {
                Ok(rows) => rows,
                Err(err) => {
                    warn!(error = %err, "failed to load file ids for incremental enrichment");
                    Vec::new()
                }
            };
            let symbol_rows = match self.repo.get_symbol_rows(project_id) {
                Ok(rows) => rows,
                Err(err) => {
                    warn!(
                        error = %err,
                        "failed to load symbol rows for incremental enrichment; \
                         proceeding without it (cross-file edges into unchanged files \
                         would be dropped)"
                    );
                    Vec::new()
                }
            };

            // 1. File ids of unchanged files (IMPORTS resolution).
            for (path, id) in file_rows {
                if !excluded.contains(&path) {
                    enr.extra_file_index.insert(path, id);
                }
            }
            // 2. Function/Method ids of unchanged files, grouped under their
            // (persisted) File id (REEXPORTS resolution).
            for row in &symbol_rows {
                if excluded.contains(&row.file_path) {
                    continue;
                }
                if matches!(row.label, NodeLabel::Function | NodeLabel::Method) {
                    if let Some(file_id) = enr.extra_file_index.get(&row.file_path) {
                        enr.extra_func_index
                            .entry(file_id.clone())
                            .or_default()
                            .entry(row.name.clone())
                            .or_insert_with(|| row.id.clone());
                    }
                }
            }
            // 3. Symbol table entries of unchanged files (CALLS/DataFlows/
            // Type resolution).
            crate::resolve::enrich_symbol_table_from_rows(
                &mut symbol_table,
                &symbol_rows,
                &excluded,
                project_id,
            );
            enr
        };

        resolve_all(
            &parse.results,
            &symbol_table,
            project_id,
            &mut graph,
            &includes_graph,
            &enrichment,
        );

        // fix: Parameter and Variable nodes created during dataflow
        // resolution are already in `graph.nodes`. Rewrite their
        // `file_path` in place (absolute → relative) instead of cloning them
        // into a separate `all_nodes` Vec. This eliminates the second copy
        // of Parameter/Variable nodes that previously existed in `all_nodes`.
        for label in [NodeLabel::Parameter, NodeLabel::Variable] {
            let path_to_rel = &scope.path_to_rel;
            graph.for_each_node_with_label_mut(label, |n| {
                if let Some(fp) = n.file_path.as_mut() {
                    if let Some(rel) = path_to_rel.get(fp) {
                        *fp = rel.clone();
                    }
                }
            });
        }

        Ok(ResolveOutput {
            graph,
            files_parsed: parse.files_parsed,
            files_skipped: scan.diff.unchanged.len(),
            includes_graph,
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 5: ConfidencePhase (pass-through placeholder)
// ---------------------------------------------------------------------------

/// Phase 5: confidence scoring (pass-through placeholder).
///
/// Real confidence tier assignment is not yet implemented; for now this
/// phase is a no-op that ensures LoadPhase runs after ResolvePhase.
pub struct ConfidencePhase;

impl Phase for ConfidencePhase {
    type Input = ();
    type Output = ();
    const NAME: &'static str = "confidence";
    fn deps() -> &'static [&'static str] {
        &["resolve"]
    }

    fn run(&self, _: Self::Input, _ctx: &mut PipelineCtx) -> Result<Self::Output, PhaseError> {
        // Pass-through — real confidence scoring not yet implemented.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 6: LoadPhase
// ---------------------------------------------------------------------------

/// Counts from the pre-persist edge integrity pass
/// ([`enforce_edge_integrity`]).
#[derive(Debug, Default, PartialEq, Eq)]
struct EdgeIntegrityStats {
    /// Edges whose missing `#`-suffixed source was re-attributed to the
    /// existing parent entity.
    repaired: usize,
    /// Edges dropped as duplicates of an earlier edge under the persisted
    /// primary-key id (`{source}_{target}_{type}_{line}`) — repair can
    /// collapse distinct `#`-variant sources onto one parent, and the CSV
    /// writer deduplicates per chunk only, so a global pass here is what
    /// actually prevents `COPY FROM` primary-key conflicts.
    deduplicated: usize,
    /// Edges dropped because an endpoint exists neither in the graph nor in
    /// the database (unresolvable external types, generic params).
    dropped: usize,
}

/// Repairs and drops edges whose endpoints reference nonexistent nodes
/// (storage quality rule DQ-004), so no orphan edge reaches `CodeRelation`.
///
/// Three passes, run BEFORE the load transaction:
///
/// 1. **Repair** — a missing source carrying a `#` disambiguator (e.g. the
///    TS writer QN `validateSnapshot#nodes`, composed from a
///    `variable_declarator` context while the extracted function node is
///    plain `validateSnapshot`) names a sub-entity of a real node. The edge
///    is re-attributed to that parent (`#`-stripped qualified name).
///    Sources without `#` are unresolvable symbol references (`impl KitExt
///    for AsyncKit` with external AsyncKit, `impl ErasedPhase for P` with
///    generic param `P`) — re-pointing them at the enclosing File node would
///    fabricate semantics, so they fall through to the drop pass.
/// 2. **Dedup** — the persisted primary key is the ambiguous join
///    `{source}_{target}_{type}_{line}`: FQN segments contain `_`, so two
///    edges with different `(source, target)` pairs can render the same id
///    STRING, while the CSV writer deduplicates by the exact
///    `(source, target, type, line)` TUPLE and happily emits both — the
///    collision then aborts `COPY FROM`. Dedup by id string here closes
///    that gap (and collapses repair-induced duplicates).
/// 3. **Gate** — an edge whose endpoint exists neither in the graph nor in
///    the database is dropped. The DB lookup keeps incremental runs intact:
///    cross-file edges into unchanged files resolve against nodes persisted
///    by earlier runs, which the in-memory graph alone does not contain.
///    Nodes under `excluded_paths` (deleted/changed this run) don't count —
///    the transaction is about to remove them.
///
/// Why before the transaction: interleaving read statements into the write
/// transaction aborts it at index scale (COMMIT then fails with "no active
/// transaction"), so the DB lookups run against the pre-transaction
/// snapshot instead. Semantics stay exact because the final node set is
/// (old nodes NOT under `excluded_paths`) ∪ new graph nodes. The window
/// between snapshot and COMMIT assumes the engine-level single writer (CLI
/// one-shot runs; daemon batches are serialized the same way).
///
/// All counts are returned so the caller logs them — dropping must stay
/// visible, not silent. Every repaired/dropped edge additionally emits a
/// `debug!` record with its identity.
fn enforce_edge_integrity<E: crate::storage::connection::CypherExecutor>(
    graph: &mut Graph,
    conn: &E,
    excluded_paths: &[String],
    project: &str,
) -> Result<EdgeIntegrityStats, IndexError> {
    let mut stats = EdgeIntegrityStats::default();

    // Pass 1 — repair misattributed `#`-suffixed sources to their parent.
    // The parent entity is the `#`-stripped qualified name (`fn#var` →
    // `fn`); `FqnGenerator::parent_qn` would strip the whole `fn#var` dot
    // segment instead, yielding the file path.
    for edge in &mut graph.edges {
        if graph.nodes.contains_key(&edge.source) {
            continue;
        }
        let Some(hash_pos) = edge.source.rfind('#') else {
            continue;
        };
        let parent = edge.source[..hash_pos].to_string();
        if graph.nodes.contains_key(&parent) {
            debug!(
                event = "edge_repair",
                from = %edge.source,
                to = %parent,
                kind = ?edge.edge_type,
                "re-attributed edge source to its parent entity"
            );
            edge.source = parent;
            stats.repaired += 1;
        }
    }

    // Passes 2+3 — one retain pass for both dedup and the endpoint gate
    // (each `retain_edges` rebuilds the adjacency index, so folding them
    // halves that cost). `missing` is collected over ALL edges — a superset
    // of the post-dedup set, which is all the DB lookup needs.
    // Owned set: the `retain_edges` closure cannot borrow `graph.nodes`
    // while `graph` is mutably borrowed.
    let node_ids: std::collections::HashSet<String> = graph.nodes.keys().cloned().collect();
    let missing: std::collections::HashSet<String> = graph
        .edges
        .iter()
        .flat_map(|e| [&e.source, &e.target])
        .filter(|id| !node_ids.contains(id.as_str()))
        .cloned()
        .collect();
    let db_live = if missing.is_empty() {
        std::collections::HashMap::new()
    } else {
        // Edge endpoints carry no label, so every node table must be
        // consulted; `live_db_nodes` materializes the id list per label
        // lazily.
        live_db_nodes(
            conn,
            |_label| missing.iter().cloned().collect::<Vec<String>>(),
            excluded_paths,
            project,
        )?
    };
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    graph.retain_edges(|edge| {
        if !seen_ids.insert(crate::storage::loader::edge_id(edge)) {
            stats.deduplicated += 1;
            return false;
        }
        let endpoints_ok = [&edge.source, &edge.target]
            .iter()
            .all(|id| node_ids.contains(id.as_str()) || db_live.contains_key(id.as_str()));
        if !endpoints_ok {
            debug!(
                event = "edge_drop",
                source = %edge.source,
                target = %edge.target,
                kind = ?edge.edge_type,
                "dropped edge with unresolvable endpoints"
            );
            stats.dropped += 1;
        }
        endpoints_ok
    });
    Ok(stats)
}

/// Returns the subset of `ids` that exist as node ids in the pre-transaction
/// database snapshot and will still exist after it, mapped to their
/// `filePath` (`None` when the label has no such column).
///
/// `excluded_paths` are file paths scheduled for deletion this run: nodes
/// under them exist in the snapshot but not in the final graph, so a hit
/// whose `filePath` is in that list does not count. The filter runs
/// client-side — Cypher `NOT IN` with NULL `filePath` rows would drop them
/// from the result (three-valued logic) — and read statements must stay out
/// of the write transaction anyway (at index scale they abort it before
/// COMMIT).
///
/// Liveness rules per label, mirroring what the delete pass will do:
/// - Labels WITH a `filePath` column: a row is live only when its
///   `filePath` is present and outside `excluded_paths` (NULL filePath
///   never matches the path-keyed delete, but such rows are parser debris —
///   treating them as live would let them mask the re-inserted node).
/// - Labels WITHOUT one (e.g. `ExternalPackage`): never path-deleted, so
///   existence alone implies survival. Skipping those tables would wrongly
///   drop edges that legitimately point at such nodes (e.g. `DEPENDS_ON` →
///   `ExternalPackage`).
///
/// Rows are scoped to `project`, matching the delete pass's predicate —
/// same-path rows of other projects must not count.
///
/// The result doubles as the node-side survivor set: a graph node found
/// here already exists in the DB and is not scheduled for deletion, so
/// re-inserting it would collide on the primary key (external-package nodes
/// are re-emitted on every run that parses one of their imports).
///
/// `ids_for` supplies the id list per label lazily, so callers that know a
/// node's label (the survivor check) only ever query that label's table,
/// and callers that don't (edge endpoints) can consult every table without
/// cloning the id set up front.
fn live_db_nodes<E, F>(
    conn: &E,
    ids_for: F,
    excluded_paths: &[String],
    project: &str,
) -> Result<std::collections::HashMap<String, Option<String>>, IndexError>
where
    E: crate::storage::connection::CypherExecutor,
    F: Fn(NodeLabel) -> Vec<String>,
{
    // Existing-tables precheck (same pattern as
    // `delete_file_nodes_batch_on`): a missing table is the only tolerated
    // condition, decided BEFORE any lookup so no statement error is ever
    // swallowed — a failed statement aborts a transaction and the real
    // cause would only resurface as a confusing COMMIT failure.
    let existing_tables: std::collections::HashSet<String> = conn
        .query_cypher("CALL show_tables() RETURN name;")?
        .iter()
        .filter_map(|row| row.first().and_then(|v| v.as_str()).map(String::from))
        .collect();
    let project_escaped = crate::storage::schema::escape_cypher_string(project);
    let excluded: std::collections::HashSet<&str> =
        excluded_paths.iter().map(String::as_str).collect();
    let mut found = std::collections::HashMap::new();
    // Per-query round-trip overhead dominates these point lookups (~20ms on
    // a debug build), so few large IN lists beat many small ones.
    const CHUNK: usize = 4000;
    for label in NodeLabel::all() {
        let ids = ids_for(label);
        // The `Project` table has no `project` column (it IS the project)
        // and is never a legitimate edge endpoint — skip it along with its
        // predicate, like the filePath-less labels below.
        if ids.is_empty()
            || !existing_tables.contains(label.table_name())
            || !crate::storage::schema::node_table_columns(label).contains(&"project")
        {
            continue;
        }
        let has_file_path = crate::storage::schema::node_table_columns(label).contains(&"filePath");
        let table = crate::storage::schema::escape_identifier(label.table_name());
        let projection = if has_file_path {
            "n.id, n.filePath"
        } else {
            "n.id"
        };
        for chunk in ids.chunks(CHUNK) {
            let id_list = chunk
                .iter()
                .map(|id| format!("'{}'", crate::storage::schema::escape_cypher_string(id)))
                .collect::<Vec<_>>()
                .join(", ");
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.id IN [{id_list}] AND n.project = '{project_escaped}' RETURN {projection};"
            );
            // Fail-loud: missing tables were pre-checked, so any error here
            // is real and must propagate (see the precheck comment).
            let rows = conn.query_cypher(&cypher).map_err(IndexError::from)?;
            for row in rows {
                let Some(id) = row.first().and_then(|v| v.as_str()) else {
                    continue;
                };
                let file_path = if has_file_path {
                    match row.get(1).and_then(|v| v.as_str()) {
                        // NULL or excluded filePath → not live (see doc).
                        Some(fp) if !excluded.contains(fp) => Some(fp.to_string()),
                        _ => continue,
                    }
                } else {
                    None
                };
                found.insert(id.to_string(), file_path);
            }
        }
    }
    Ok(found)
}

/// Phase 6: persist nodes and edges to the database, build [`IndexResult`].
///
/// Replaces original steps 7–9 of the pipeline.
pub struct LoadPhase {
    /// Shared repository handle (Arc-cloned from Pipeline).
    pub repo: Arc<Repository>,
}

impl Phase for LoadPhase {
    type Input = ();
    type Output = LoadOutput;
    const NAME: &'static str = "load";
    fn deps() -> &'static [&'static str] {
        &["scan", "resolve", "confidence"]
    }

    fn run(&self, _: Self::Input, ctx: &mut PipelineCtx) -> Result<Self::Output, PhaseError> {
        // L7-3 memory-overflow fix: remove scan and resolve from ctx (instead
        // of get) so they are freed when LoadPhase returns, not when the
        // caller drops `ctx`. LoadPhase is the last phase in the DAG — no
        // subsequent phase reads `scan` or `resolve`. Keeping them in ctx
        // after LoadPhase finishes holds `disk_files: Vec<FileInfo>` (10k+
        // entries × ~100 bytes) and the resolved `Graph` alive until the
        // caller extracts `LoadOutput` and drops ctx, which for large repos
        // adds ~1-2 GB of needless post-pipeline RSS.
        //
        // Order: both `remove`s go first (mutable borrow of ctx), then the
        // owned values are used via shared references for the rest of the
        // function — same borrow pattern as ResolvePhase::run (L6-1) and
        // ScopeResolutionPhase::run (L7-2).
        let scan = ctx
            .remove::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;
        let mut resolve = ctx
            .remove::<ResolveOutput>("resolve")
            .ok_or(PhaseError::MissingInput("resolve"))?;

        let project_id = &scan.project_id;
        let project_name = &scan.project_name;
        let root = &scan.root_path;
        let disk_files = &scan.disk_files;
        // L3 memory-overflow fix: graph is the single source of truth — no
        // `all_nodes`/`all_edges` Vecs are borrowed from ResolveOutput. The
        // graph's node/edge counts are used for IndexResult reporting.
        // Mutable: the integrity pass below rewrites/drops edges in place
        // right before persistence, so the reported edge count is the
        // persisted count.
        let graph = &mut resolve.graph;

        // Steps 7–8: the whole load — batch-delete + project/definition/edge
        // writes — runs on ONE dedicated connection inside an explicit
        // transaction (`BEGIN TRANSACTION` on entry, `COMMIT` on success,
        // `ROLLBACK` on any error), making an index run an atomic unit: the
        // graph is either fully updated or unchanged. This replaces the old
        // statement-per-connection behavior where a swallowed delete failure
        // left duplicate/ghost symbols that no later run repaired, and a
        // crash mid-load left a partially replaced graph.
        //
        // Transaction state is per-connection — `StorageConnection::execute`
        // opens a fresh connection per statement and cannot join one — hence
        // the `_on` primitive variants running against the supplied `WriteTx`.
        //
        // Note: lbug 0.20 allows re-inserting a primary key deleted earlier
        // in the SAME transaction (guard test
        // `lbug_allows_delete_then_insert_same_pk_in_one_tx`), which an index
        // run legitimately does for the Project id and changed-file FQN ids.
        //
        // Failure is fatal: rollback and abort. That is safe for convergence
        // — a rolled-back load leaves the previous consistent graph, and the
        // next diff re-derives the same work.
        let mut paths_to_delete: Vec<String> = scan.diff.deleted.clone();
        paths_to_delete.extend(scan.diff.changed.iter().map(|f| f.relative_path.clone()));

        // DQ-004 hardening: repair/drop orphan-endpoint edges BEFORE the
        // transaction. The gate's DB lookups must stay out of the write
        // transaction (interleaved reads abort it at index scale — see
        // `enforce_edge_integrity`); running them against the pre-transaction
        // snapshot with `paths_to_delete` excluded is semantically exact.
        let integrity =
            enforce_edge_integrity(graph, self.repo.connection(), &paths_to_delete, project_id)
                .map_err(|e| phase_err("load", e))?;
        if integrity.repaired > 0 || integrity.deduplicated > 0 || integrity.dropped > 0 {
            warn!(
                event = "edge_integrity",
                repaired = integrity.repaired,
                deduplicated = integrity.deduplicated,
                dropped = integrity.dropped,
                "edge integrity pass: repaired misattributed sources, \
                 deduplicated persisted-id collisions, dropped edges \
                 with unresolvable endpoints"
            );
        }
        let has_edges = !graph.edges.is_empty();

        // Node-side survivor skip: a graph node that already exists in the
        // DB and is not scheduled for deletion must not be re-inserted —
        // COPY FROM treats the existing primary key as a conflict.
        // filePath-less labels (e.g. ExternalPackage) are re-emitted on
        // every run that parses one of their imports and are never
        // path-deleted, so incremental runs hit this on every batch. Corollary:
        // a survivor's persisted properties are frozen (the re-extracted
        // node is discarded); this is safe because those labels' attributes
        // are deterministic functions of (project, package name). Nodes
        // know their own label, so each id is only checked against its own
        // table (one query per label-chunk instead of per label × chunk).
        // First index of this project: nothing is persisted under its id,
        // so nothing can survive — skip the lookup entirely (the survivor
        // check exists purely to avoid colliding with persisted rows).
        let project_exists = self
            .repo
            .get_project(project_id)
            .map_err(IndexError::from)
            .map_err(|e| phase_err("load", e))?
            .is_some();
        let survivors = if project_exists {
            // Full-coverage run (no unchanged files): every filePath-bearing
            // row is deleted in the transaction below, so only column-less
            // labels (ExternalPackage etc.) can possibly survive — skip the
            // per-label lookups for everything else.
            let full_coverage = scan.diff.unchanged.is_empty();
            let mut ids_by_label: HashMap<NodeLabel, Vec<String>> = HashMap::new();
            for node in graph.nodes_view() {
                if full_coverage
                    && crate::storage::schema::node_table_columns(node.label).contains(&"filePath")
                {
                    continue;
                }
                ids_by_label
                    .entry(node.label)
                    .or_default()
                    .push(node.id.clone());
            }
            live_db_nodes(
                self.repo.connection(),
                |label| ids_by_label.get(&label).cloned().unwrap_or_default(),
                &paths_to_delete,
                project_id,
            )
            .map_err(|e| phase_err("load", e))?
        } else {
            HashMap::new()
        };
        let survivor_count = survivors.len();
        if survivor_count > 0 {
            info!(
                event = "node_survivors_skipped",
                count = survivor_count,
                "skipped re-inserting nodes already persisted and not \
                 scheduled for deletion"
            );
        }

        let result: std::result::Result<(), IndexError> = self.repo.in_write_transaction(|tx| {
            // Step 7: batch-delete old nodes for deleted + changed files.
            //
            // The batch path collapses N per-file passes (each ~21 Cypher
            // queries over the node-label set) into a single
            // `WHERE n.filePath IN [...]` pass, keeping the query count
            // fixed regardless of how many files changed — the
            // `incremental_500_of_1000` SLO fix.
            if !paths_to_delete.is_empty() {
                Repository::delete_file_nodes_batch_on(tx, &paths_to_delete, project_id)
                    .map_err(IndexError::from)?;
            }

            // Step 8: persist project node, definition nodes, and edges —
            // streamed from the graph (iterator-based callers MUST use the
            // `_stream`/`_on` variants; for large repos streaming avoids
            // ~10 MB of peak RSS). Survivors are filtered out above.
            save_project_node_on(tx, &self.repo, project_id, project_name, root, disk_files)?;
            save_nodes_by_label_on(
                tx,
                graph
                    .nodes_view()
                    .filter(|n| !survivors.contains_key(&n.id)),
            )?;
            if has_edges {
                Repository::save_edges_stream_on(tx, graph.edges_view())
                    .map_err(IndexError::from)?;
            }
            Ok(())
        });
        result.map_err(|e| phase_err(Self::NAME, e))?;

        // Step 9: build the IndexResult.
        let duration_ms = scan.start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let files_indexed = resolve.files_parsed;
        let files_skipped = resolve.files_skipped;
        let nodes_created = graph.node_count() - survivor_count;
        let edges_created = graph.edge_count();

        info!(
            event = "index_completed",
            project = %project_name,
            path = %root.display(),
            files_indexed = files_indexed,
            files_skipped = files_skipped,
            nodes_created = nodes_created,
            edges_created = edges_created,
            duration_ms = duration_ms,
            "indexing completed"
        );

        let files_per_second = if duration_ms > 0 {
            (files_indexed as f64 * 1000.0 / duration_ms as f64).round() as u64
        } else {
            0
        };
        info!(
            event = "performance",
            files_per_second = files_per_second,
            files_indexed = files_indexed,
            duration_ms = duration_ms,
            "indexing performance metrics"
        );

        Ok(LoadOutput {
            index_result: IndexResult::new(
                project_id.clone(),
                files_indexed,
                files_skipped,
                nodes_created,
                edges_created,
                duration_ms,
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Free helper functions (converted from Pipeline methods)
// ---------------------------------------------------------------------------

/// Looks up an existing project id by name, or generates a new one.
///
/// Treats the project *name* as the stable identifier across re-indexes.
/// If a project with this name already exists in the DB, reuses its id;
/// otherwise generates a fresh `proj_<uuid>` id.
fn lookup_or_create_project_id(
    repo: &Repository,
    project_name: &str,
) -> std::result::Result<String, IndexError> {
    let projects = repo.list_projects().unwrap_or_default();
    for project in projects {
        if project.name == project_name {
            return Ok(project.id);
        }
    }
    Ok(new_project_id())
}

/// Saves (or re-saves) the project node.
///
/// Only saves the project node if it does not already exist — this preserves
/// the File nodes (and their hashes) from prior runs, which the incremental
/// indexer depends on.
///
/// Canonicalizes `root` to an absolute path before writing it into the
/// Project node's `rootPath` property. This prevents downstream staleness
/// checks (`status`/`dead_code`) from running `git rev-parse HEAD` against the
/// process's CWD when the caller passed a relative path like `.`. Falls back
/// to the original path on `canonicalize` failure so dry-run tests with
/// non-existent paths still work.
fn save_project_node_on<E: crate::storage::connection::CypherExecutor>(
    conn: &E,
    repo: &Repository,
    project_id: &str,
    project_name: &str,
    root: &Path,
    disk_files: &[FileInfo],
) -> std::result::Result<(), IndexError> {
    // If the project node already exists, delete only the Project row — on
    // the SAME transaction connection as the re-insert (the existence check
    // reads committed state via `repo`, which is fine: the tx only needs the
    // DELETE and CREATE to be co-located for the PK replace to work).
    if repo.get_project(project_id)?.is_some() {
        let cypher = format!(
            "MATCH (p:Project {{id: '{}'}}) DELETE p;",
            crate::storage::schema::escape_cypher_string(project_id),
        );
        conn.execute_cypher(&cypher)?;
    }
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let last_commit = git_head_commit(&canonical_root);
    let project_node = Node::builder(NodeLabel::Project, project_name, project_name)
        .id(project_id)
        .properties(serde_json::json!({
            "rootPath": canonical_root.display().to_string(),
            "fileCount": disk_files.len() as i64,
            "indexedAt": now_unix_seconds(),
            "lastCommit": last_commit,
        }))
        .build();
    Repository::save_project_on(conn, &project_node)?;
    Ok(())
}

/// Returns the current `HEAD` commit hash of the git repo at `root`, or an
/// empty string if `root` is not a git repo (or git is unavailable).
///
/// Used to populate the `lastCommit` field on Project nodes for staleness
/// tracking (H9).
fn git_head_commit(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Groups nodes by label and bulk-saves each group.
///
/// Deduplicates by id within each label group (LadybugDB's COPY rejects
/// duplicate primary keys). Project nodes are skipped (saved via
/// [`save_project_node`]).
///
/// L3 memory-overflow fix: accepts `impl Iterator<Item = &Node>` instead of
/// `&[Node]`, so callers can stream from [`Graph::nodes_view`] without
/// building a separate `Vec<Node>` copy. The internal `by_label` collection is
/// built per-label (transient), not for all nodes at once.
///
/// L6-3 memory-overflow fix: dedup is now performed inside
/// [`Repository::save_nodes`] (via [`write_nodes_csv_stream`]'s HashSet by
/// node id), so this function no longer builds a `deduped: Vec<Node>` clone
/// per label group. For large repos (100k+ nodes), eliminating the per-group
/// Vec clone avoids ~10 MB of peak RSS. The `by_label: HashMap<NodeLabel,
/// Vec<&Node>>` collection is retained because each label maps to a distinct
/// CSV file (and thus a distinct `save_nodes` call) — the grouping is
/// structurally necessary, only the per-group dedup Vec was waste.
fn save_nodes_by_label_on<'a, E: crate::storage::connection::CypherExecutor>(
    conn: &E,
    nodes: impl Iterator<Item = &'a Node>,
) -> std::result::Result<(), IndexError> {
    let mut by_label: HashMap<NodeLabel, Vec<&Node>> = HashMap::new();
    for node in nodes {
        by_label.entry(node.label).or_default().push(node);
    }
    for (label, group) in by_label {
        if group.is_empty() {
            continue;
        }
        // Project nodes are saved via save_project_node; skip them here.
        if label == NodeLabel::Project {
            continue;
        }
        // L6-3: pass `group.iter().copied()` (Iterator<Item = &Node>) directly
        // to `Repository::save_nodes_stream` (now accepts `impl IntoIterator<Item =
        // &Node>`). Dedup is performed inside `write_nodes_csv_stream`
        // (HashSet by node id), so no `deduped: Vec<Node>` clone is needed
        // here. `with_retry` re-creates the iterator on each attempt (cheap —
        // `group.iter()` is a single pointer borrow).
        //
        // L4: renamed from `save_nodes` to `save_nodes_stream` — iterator-based
        // callers MUST use the inherent `_stream` method directly; the trait
        // method's `&[Node]` signature does not accept `impl Iterator`.
        // No per-statement retry: the transaction holds the single-writer
        // lock for the whole span, so foreign-writer conflicts cannot surface
        // mid-transaction. Any failure propagates and rolls the load back.
        Repository::save_nodes_stream_on(conn, group.iter().copied(), label)
            .map_err(IndexError::from)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Compile-time Send + Sync assertions
// ---------------------------------------------------------------------------

#[cfg(test)]
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ScanPhase>();
    assert_send_sync::<ParsePhase>();
    assert_send_sync::<ScopeResolutionPhase>();
    assert_send_sync::<ResolvePhase>();
    assert_send_sync::<ConfidencePhase>();
    assert_send_sync::<LoadPhase>();
    assert_send_sync::<ScanInput>();
    assert_send_sync::<ScanOutput>();
    assert_send_sync::<ParseOutput>();
    assert_send_sync::<ScopeOutput>();
    assert_send_sync::<ResolveOutput>();
    assert_send_sync::<LoadOutput>();
};

#[cfg(all(test, feature = "lang-cpp", feature = "lang-typescript"))]
mod tests {
    use super::*;

    // --- git_head_commit ---

    #[test]
    fn git_head_commit_non_git_dir_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A plain TempDir is not a git repo → git rev-parse fails → empty string.
        let commit = git_head_commit(tmp.path());
        assert_eq!(commit, "", "non-git dir should return empty commit hash");
    }

    #[test]
    fn git_head_commit_codenexus_repo_returns_nonempty() {
        // The CodeNexus project itself is a git repo (we're running tests in it).
        // This verifies the success path: git found → rev-parse HEAD → trim.
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let commit = git_head_commit(manifest_dir);
        // If git is not installed, this returns empty — skip the assertion.
        if let Ok(output) = std::process::Command::new("git").arg("--version").output() {
            if output.status.success() {
                assert!(
                    !commit.is_empty(),
                    "CodeNexus repo should have a HEAD commit"
                );
                // A git commit hash is 40 hex chars (SHA-1) or 64 (SHA-256).
                assert!(
                    commit.len() == 40 || commit.len() == 64,
                    "commit hash has unexpected length {}: {commit}",
                    commit.len()
                );
            }
        }
    }

    // --- phase_err helper ---

    #[test]
    fn phase_err_wraps_index_error_as_execution_failed() {
        let err = IndexError::PathNotFound("/no/such/dir".to_string());
        let phase = phase_err("scan", err);
        match phase {
            PhaseError::ExecutionFailed { phase, inner } => {
                assert_eq!(phase, "scan");
                assert!(
                    inner.to_string().contains("/no/such/dir"),
                    "inner error should carry original message: {inner}"
                );
            }
            other => panic!("expected ExecutionFailed, got {other:?}"),
        }
    }

    // --- build_includes_edges(C++ #include → INCLUDES edges) ---

    /// Creates a File node with id = file_path = name (mirrors imports.rs tests).
    fn make_file_node(path: &str, project: &str, lang: Language) -> Node {
        Node::builder(NodeLabel::File, path, path)
            .id(path)
            .project(project)
            .file_path(path)
            .language(lang)
            .build()
    }

    #[test]
    fn build_includes_edges_for_cpp() {
        // C++ main.cpp #include "foo.h" → 1 INCLUDES edge main.cpp → foo.h.
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj", Language::Cpp));
        graph.add_node(make_file_node("src/foo.h", "proj", Language::Cpp));

        let mut main_result = ExtractResult::new("src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "foo.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        // fix: `build_includes_edges` now returns only `IncludesGraph`
        // (edges are moved into `graph`).
        let includes_graph = build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        let edges: Vec<&Edge> = graph
            .edges_view()
            .filter(|e| e.edge_type == EdgeType::Includes)
            .collect();
        assert_eq!(edges.len(), 1, "should create 1 INCLUDES edge");
        assert_eq!(edges[0].edge_type, EdgeType::Includes);
        assert_eq!(edges[0].source, "src/main.cpp");
        assert_eq!(edges[0].target, "src/foo.h");
        assert_eq!(
            edges[0].confidence_tier,
            ConfidenceTier::ImportScoped,
            "INCLUDES edges use ImportScoped tier (matches IMPORTS pattern)"
        );
        assert!(
            includes_graph.contains("src/main.cpp", "src/foo.h"),
            "IncludesGraph should have direct edge main.cpp → foo.h"
        );
    }

    #[test]
    fn build_includes_edges_skips_non_cpp_languages() {
        // TypeScript import should NOT produce INCLUDES edges (only C++ does).
        // TS imports are handled by ImportResolver as IMPORTS edges.
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.ts", "proj", Language::TypeScript));
        graph.add_node(make_file_node("src/utils.ts", "proj", Language::TypeScript));

        let mut ts_result = ExtractResult::new("src/main.ts", Language::TypeScript);
        ts_result.imports.push(crate::ir::ImportInfo {
            source_file: "./utils.ts".to_string(),
            imported_names: vec!["foo".to_string()],
            line: 1,
            is_reexport: false,
        });
        let results = vec![ts_result];

        let path_to_rel = HashMap::new();
        // fix: `build_includes_edges` now returns only `IncludesGraph`.
        let includes_graph = build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        let includes_edges: Vec<&Edge> = graph
            .edges_view()
            .filter(|e| e.edge_type == EdgeType::Includes)
            .collect();
        assert!(
            includes_edges.is_empty(),
            "non-C++ languages should NOT produce INCLUDES edges"
        );
        assert!(
            includes_graph.is_empty(),
            "IncludesGraph should be empty for non-C++ results"
        );
    }

    #[test]
    fn build_includes_edges_skips_system_headers() {
        // C++ #include <iostream> — system header, no matching File node.
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj", Language::Cpp));
        // No "iostream" file in the project.

        let mut main_result = ExtractResult::new("src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "iostream".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        // fix: `build_includes_edges` now returns only `IncludesGraph`.
        let includes_graph = build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        let includes_edges: Vec<&Edge> = graph
            .edges_view()
            .filter(|e| e.edge_type == EdgeType::Includes)
            .collect();
        assert!(
            includes_edges.is_empty(),
            "system header #include <iostream> should not produce INCLUDES edge"
        );
        assert!(includes_graph.is_empty());
    }

    #[test]
    fn build_includes_edges_populates_transitive_graph() {
        // Chained #include: main.cpp → foo.h → bar.h.
        // IncludesGraph should have both edges; reachable_from("main.cpp")
        // should include foo.h and bar.h (transitive closure).
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj", Language::Cpp));
        graph.add_node(make_file_node("src/foo.h", "proj", Language::Cpp));
        graph.add_node(make_file_node("src/bar.h", "proj", Language::Cpp));

        let mut main_result = ExtractResult::new("src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "foo.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let mut foo_result = ExtractResult::new("src/foo.h", Language::Cpp);
        foo_result.imports.push(crate::ir::ImportInfo {
            source_file: "bar.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result, foo_result];

        let path_to_rel = HashMap::new();
        // fix: `build_includes_edges` now returns only `IncludesGraph`.
        let includes_graph = build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        let includes_edges: Vec<&Edge> = graph
            .edges_view()
            .filter(|e| e.edge_type == EdgeType::Includes)
            .collect();
        assert_eq!(includes_edges.len(), 2, "should create 2 INCLUDES edges");
        assert!(includes_graph.contains("src/main.cpp", "src/foo.h"));
        assert!(includes_graph.contains("src/foo.h", "src/bar.h"));

        // Transitive closure: main.cpp reaches foo.h, bar.h, and itself.
        let reachable = includes_graph.reachable_from("src/main.cpp");
        assert!(reachable.contains("src/main.cpp"));
        assert!(reachable.contains("src/foo.h"));
        assert!(reachable.contains("src/bar.h"));
    }

    #[test]
    fn build_includes_edges_with_absolute_paths() {
        // Production scenario: result.file_path is absolute, path_to_rel
        // maps it to relative, graph File nodes use relative paths.
        // IncludesGraph keys should be ABSOLUTE (matching SymbolEntry.file_path
        // and caller_file, both from result.file_path).
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj", Language::Cpp));
        graph.add_node(make_file_node("src/foo.h", "proj", Language::Cpp));

        let mut main_result = ExtractResult::new("/home/dev/proj/src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "foo.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result];

        let mut path_to_rel = HashMap::new();
        path_to_rel.insert(
            "/home/dev/proj/src/main.cpp".to_string(),
            "src/main.cpp".to_string(),
        );
        path_to_rel.insert(
            "/home/dev/proj/src/foo.h".to_string(),
            "src/foo.h".to_string(),
        );

        let includes_graph = build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        let includes_edges: Vec<&Edge> = graph
            .edges_view()
            .filter(|e| e.edge_type == EdgeType::Includes)
            .collect();
        assert_eq!(
            includes_edges.len(),
            1,
            "absolute source path should resolve via path_to_rel"
        );
        assert_eq!(includes_edges[0].source, "src/main.cpp");
        assert_eq!(includes_edges[0].target, "src/foo.h");
        // IncludesGraph uses absolute paths (for SymbolEntry.file_path matching).
        assert!(
            includes_graph.contains("/home/dev/proj/src/main.cpp", "/home/dev/proj/src/foo.h"),
            "IncludesGraph keys should be absolute paths (matching SymbolEntry.file_path)"
        );
    }

    #[test]
    fn build_includes_edges_partial_path_include() {
        // C++ #include "fmt/format.h" → include/fmt/format.h (suffix match).
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj", Language::Cpp));
        graph.add_node(make_file_node(
            "include/fmt/format.h",
            "proj",
            Language::Cpp,
        ));

        let mut main_result = ExtractResult::new("src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "fmt/format.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        let _includes_graph = build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        let includes_edges: Vec<&Edge> = graph
            .edges_view()
            .filter(|e| e.edge_type == EdgeType::Includes)
            .collect();
        assert_eq!(
            includes_edges.len(),
            1,
            "partial-path #include should resolve via suffix matching"
        );
        assert_eq!(includes_edges[0].target, "include/fmt/format.h");
    }

    #[test]
    fn build_includes_edges_skips_when_source_file_not_in_graph() {
        // C++ result exists but no File node for it in the graph.
        // Should skip gracefully (no panic, no edges).
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/foo.h", "proj", Language::Cpp));
        // No File node for "src/main.cpp".

        let mut main_result = ExtractResult::new("src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "foo.h".to_string(),
            imported_names: vec![],
            line: 1,
            is_reexport: false,
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        let includes_graph = build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        let includes_edges: Vec<&Edge> = graph
            .edges_view()
            .filter(|e| e.edge_type == EdgeType::Includes)
            .collect();
        assert!(
            includes_edges.is_empty(),
            "no edges when source File node not in graph"
        );
        assert!(includes_graph.is_empty());
    }

    // --- enforce_edge_integrity (DQ-004 hardening) ---

    /// Creates a minimal Function node with explicit id (definition nodes
    /// persist `qualified_name` as their primary key).
    fn make_def_node(id: &str, project: &str) -> Node {
        Node::builder(NodeLabel::Function, "sym", id)
            .id(id)
            .project(project)
            .build()
    }

    #[test]
    fn integrity_repairs_misattributed_hash_source_to_parent() {
        // TS writer_qn shape: call attributed to `fn#var` (variable_declarator
        // context) while the extracted function node is plain `fn`. The gate
        // must re-attribute the edge to the parent entity, not drop it.
        let mut graph = Graph::new();
        graph.add_node(make_def_node(
            "proj.src.lib.snapshot.ts.validateSnapshot",
            "proj",
        ));
        graph.add_node(make_def_node(
            "proj.src.lib.i18nCatalog.ts.translate",
            "proj",
        ));
        graph.add_edge(Edge::new(
            "proj.src.lib.snapshot.ts.validateSnapshot#nodes",
            "proj.src.lib.i18nCatalog.ts.translate",
            EdgeType::Calls,
            "proj",
        ));

        let repo = Repository::in_memory().expect("in_memory repository");
        let stats = enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj")
            .expect("enforce_edge_integrity");

        assert_eq!(stats.repaired, 1, "source should be repaired to parent");
        assert_eq!(stats.dropped, 0, "repaired edge must be kept");
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(
            graph.edges[0].source, "proj.src.lib.snapshot.ts.validateSnapshot",
            "edge source must equal the real function node id"
        );
    }

    #[test]
    fn integrity_drops_implements_from_unresolvable_source() {
        // `impl KitExt for AsyncKit` with external AsyncKit: no node exists,
        // no `#` in the source (not a sub-entity attribution), parent is the
        // File node — re-pointing there would fabricate semantics. Drop.
        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.kit.bootstrap.rs.KitExt", "proj"));
        graph.add_edge(Edge::new(
            "proj.src.kit.bootstrap.rs.AsyncKit",
            "proj.src.kit.bootstrap.rs.KitExt",
            EdgeType::Implements,
            "proj",
        ));

        let repo = Repository::in_memory().expect("in_memory repository");
        let stats = enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj")
            .expect("enforce_edge_integrity");

        assert_eq!(stats.repaired, 0);
        assert_eq!(stats.dropped, 1, "orphan-source edge must be dropped");
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn integrity_keeps_edge_when_endpoint_exists_in_db() {
        // Incremental safety: the target node was loaded for an unchanged
        // file's enrichment in production — here simulated by a node that
        // lives only in the DB, not in the graph. The edge must survive.
        let repo = Repository::in_memory().expect("in_memory repository");
        // The DB target carries a filePath (production definition nodes all
        // do); NULL-filePath rows on filePath-bearing labels are debris and
        // deliberately not treated as live.
        let db_target = Node::builder(NodeLabel::Function, "sym", "proj.src.other.rs.TypeX")
            .id("proj.src.other.rs.TypeX")
            .project("proj")
            .file_path("src/other.rs")
            .build();
        repo.save_nodes_stream(std::iter::once(&db_target), NodeLabel::Function)
            .expect("save db target");

        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.a.rs.Caller", "proj"));
        graph.add_edge(Edge::new(
            "proj.src.a.rs.Caller",
            "proj.src.other.rs.TypeX",
            EdgeType::UsesType,
            "proj",
        ));

        let stats = enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj")
            .expect("enforce_edge_integrity");

        assert_eq!(
            stats.dropped, 0,
            "DB-resolvable endpoint must keep the edge"
        );
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn integrity_noop_when_all_endpoints_exist() {
        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.a.rs.Caller", "proj"));
        graph.add_node(make_def_node("proj.src.a.rs.Callee", "proj"));
        graph.add_edge(Edge::new(
            "proj.src.a.rs.Caller",
            "proj.src.a.rs.Callee",
            EdgeType::Calls,
            "proj",
        ));

        let repo = Repository::in_memory().expect("in_memory repository");
        let stats = enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj")
            .expect("enforce_edge_integrity");

        assert_eq!(stats.repaired, 0);
        assert_eq!(stats.dropped, 0);
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(graph.edges[0].source, "proj.src.a.rs.Caller");
        assert_eq!(graph.edges[0].target, "proj.src.a.rs.Callee");
    }

    #[test]
    fn integrity_dedupes_colliding_ids_after_repair() {
        // Two `#`-variant sources repairing onto the same parent produce
        // identical persisted id strings (`{src}_{tgt}_{type}_{line}`) —
        // the CSV writer dedups by the exact (source, target, type, line)
        // TUPLE and would emit both, tripping `COPY FROM`'s primary-key
        // constraint. The gate's string-level dedup catches them.
        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.lib.rs.kit", "proj"));
        graph.add_node(make_def_node("proj.src.dead_code.rs.storage", "proj"));
        for variant in ["#a", "#b"] {
            let mut edge = Edge::new(
                format!("proj.src.lib.rs.kit{variant}"),
                "proj.src.dead_code.rs.storage",
                EdgeType::DataFlows,
                "proj",
            );
            edge.start_line = Some(144);
            graph.add_edge(edge);
        }

        let repo = Repository::in_memory().expect("in_memory repository");
        let stats = enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj")
            .expect("enforce_edge_integrity");

        assert_eq!(stats.repaired, 2, "both `#`-variants repair to the parent");
        assert_eq!(
            stats.deduplicated, 1,
            "second repaired edge is a PK collision"
        );
        assert_eq!(stats.dropped, 0);
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn integrity_ignores_db_endpoint_under_excluded_path() {
        // The DB snapshot predates the transaction: a node under a path that
        // this run deletes/rewrites exists in the snapshot but not in the
        // final graph. The gate must not treat it as a live endpoint.
        let repo = Repository::in_memory().expect("in_memory repository");
        let doomed = Node::builder(NodeLabel::Function, "sym", "proj.src.gone.rs.TypeX")
            .id("proj.src.gone.rs.TypeX")
            .project("proj")
            .file_path("src/gone.rs")
            .build();
        repo.save_nodes_stream(std::iter::once(&doomed), NodeLabel::Function)
            .expect("save doomed target");

        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.a.rs.Caller", "proj"));
        graph.add_edge(Edge::new(
            "proj.src.a.rs.Caller",
            "proj.src.gone.rs.TypeX",
            EdgeType::UsesType,
            "proj",
        ));

        let excluded = vec!["src/gone.rs".to_string()];
        let stats = enforce_edge_integrity(&mut graph, repo.connection(), &excluded, "proj")
            .expect("enforce_edge_integrity");

        assert_eq!(
            stats.dropped, 1,
            "endpoint under deleted path must not count"
        );
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn integrity_keeps_depends_on_to_external_package() {
        // Regression: ExternalPackage has no filePath column, so its rows
        // are never path-deleted — existence alone makes it a live endpoint.
        // Skipping filePath-less labels here would silently drop every
        // DEPENDS_ON edge that points at an external package.
        let repo = Repository::in_memory().expect("in_memory repository");
        let extpkg = Node::builder(NodeLabel::ExternalPackage, "sdforge", "extpkg:proj:sdforge")
            .id("extpkg:proj:sdforge")
            .project("proj")
            .build();
        repo.save_nodes_stream(std::iter::once(&extpkg), NodeLabel::ExternalPackage)
            .expect("save extpkg");

        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.a.rs.Caller", "proj"));
        graph.add_edge(Edge::new(
            "proj.src.a.rs.Caller",
            "extpkg:proj:sdforge",
            EdgeType::DependsOn,
            "proj",
        ));

        let stats =
            enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj").expect("integrity");
        assert_eq!(
            stats.dropped, 0,
            "edge to persisted extpkg node must survive"
        );
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn live_db_nodes_survivor_semantics() {
        // Survivor set for the node-side skip: exists + not scheduled for
        // deletion → present (None filePath for column-less labels); under
        // an excluded path → absent.
        let repo = Repository::in_memory().expect("in_memory repository");
        let mut keep = make_def_node("proj.src.keep.rs.Sym", "proj");
        keep.file_path = Some("src/keep.rs".to_string());
        let mut doomed = make_def_node("proj.src.gone.rs.Sym", "proj");
        doomed.file_path = Some("src/gone.rs".to_string());
        let extpkg = Node::builder(NodeLabel::ExternalPackage, "sdforge", "extpkg:proj:sdforge")
            .id("extpkg:proj:sdforge")
            .project("proj")
            .build();
        for node in [&keep, &doomed, &extpkg] {
            repo.save_nodes_stream(std::iter::once(node), node.label)
                .expect("save node");
        }

        let excluded = vec!["src/gone.rs".to_string()];
        // A NULL-filePath row on a filePath-bearing label is debris: never
        // matched by the path-keyed delete, and never counted as live.
        let orphaned = make_def_node("proj.src.nullfp.rs.Sym", "proj");
        repo.save_nodes_stream(std::iter::once(&orphaned), NodeLabel::Function)
            .expect("save null-filePath node");

        let mut ids: std::collections::HashMap<NodeLabel, Vec<String>> =
            std::collections::HashMap::new();
        for id in [
            "proj.src.keep.rs.Sym",
            "proj.src.gone.rs.Sym",
            "proj.src.nullfp.rs.Sym",
            "extpkg:proj:sdforge",
        ] {
            let label = if id.starts_with("extpkg:") {
                NodeLabel::ExternalPackage
            } else {
                NodeLabel::Function
            };
            ids.entry(label).or_default().push(id.to_string());
        }
        let live = live_db_nodes(
            repo.connection(),
            |label| ids.get(&label).cloned().unwrap_or_default(),
            &excluded,
            "proj",
        )
        .expect("live_db_nodes");

        assert_eq!(
            live.get("proj.src.keep.rs.Sym").map(|fp| fp.as_deref()),
            Some(Some("src/keep.rs")),
            "survivor outside deleted paths"
        );
        assert!(
            !live.contains_key("proj.src.gone.rs.Sym"),
            "node under deleted path is not a survivor"
        );
        assert!(
            !live.contains_key("proj.src.nullfp.rs.Sym"),
            "NULL filePath is not live"
        );
        assert_eq!(
            live.get("extpkg:proj:sdforge"),
            Some(&None),
            "filePath-less label is always live"
        );
    }

    #[test]
    fn integrity_repair_requires_hash_and_existing_parent() {
        // No repair when the source has no `#` (even though the File-parent
        // exists) — IMPLEMENTS from an external type must not be re-pointed
        // at the file. And no repair when the `#`-parent is itself missing.
        let repo = Repository::in_memory().expect("in_memory repository");

        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.kit.bootstrap.rs", "proj")); // File node
        graph.add_edge(Edge::new(
            "proj.src.kit.bootstrap.rs.AsyncKit",
            "proj.src.kit.bootstrap.rs.KitExt",
            EdgeType::Implements,
            "proj",
        ));
        let stats =
            enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj").expect("integrity");
        assert_eq!(stats.repaired, 0, "no `#` → no parent repair");
        assert_eq!(stats.dropped, 1, "unresolvable → dropped");

        let mut graph = Graph::new();
        graph.add_node(make_def_node("proj.src.a.ts.other", "proj"));
        graph.add_edge(Edge::new(
            "proj.src.a.ts.missingFn#var",
            "proj.src.a.ts.other",
            EdgeType::Calls,
            "proj",
        ));
        let stats =
            enforce_edge_integrity(&mut graph, repo.connection(), &[], "proj").expect("integrity");
        assert_eq!(stats.repaired, 0, "missing parent → no repair");
        assert_eq!(stats.dropped, 1);
    }
}
