// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Typed [`Phase`] implementations for the indexing pipeline (Task 2.5).
//!
//! Refactors the 9-step sequence in [`super::pipeline`] into 6 typed phases
//! executed by the [`DagPipeline`](super::pipeline_dag::Pipeline) runner:
//!
//! 1. [`ScanPhase`] — discover files, lookup/create project, diff hashes.
//! 2. [`ParsePhase`] — parallel-parse changed+added files.
//! 3. [`ScopeResolutionPhase`] — build in-memory graph (nodes + per-file edges).
//! 4. [`ResolvePhase`] — resolve calls/dataflow/FFI edges.
//! 5. [`ConfidencePhase`] — pass-through (Task 2.8 adds real confidence scoring).
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

use tracing::{info, warn};

use crate::discover::{FileInfo, Walker};
use crate::index::error::IndexError;
use crate::index::incremental::{diff_files, FileDiff};
use crate::index::pipeline::{
    build_file_nodes, now_unix_seconds, with_retry, DEFAULT_MAX_RETRIES,
};
use crate::ir::ExtractResult;
use crate::model::{ConfidenceTier, Edge, EdgeType, Graph, Language, Node, NodeLabel, new_project_id};
use crate::parse::parallel::{parallel_parse, parallel_parse_ram_first, RamFirstSources};
use crate::resolve::{
    build_symbol_table, prune_dangling_type_edges_vec, resolve_all, IncludesGraph, resolve_include,
};
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
    /// The in-memory graph (nodes + per-file edges).
    pub graph: Graph,
    /// All definition + file nodes collected so far.
    pub all_nodes: Vec<Node>,
    /// All edges collected so far (definition + per-file).
    pub all_edges: Vec<Edge>,
    /// Mapping from absolute file path → relative path (for normalizing
    /// filePath fields on Parameter/Variable nodes in [`ResolvePhase`]).
    pub path_to_rel: HashMap<String, String>,
}

/// Typed output of [`ResolvePhase`], consumed by [`LoadPhase`].
pub struct ResolveOutput {
    /// All nodes to persist (definition + file + parameter + variable).
    pub all_nodes: Vec<Node>,
    /// All edges to persist (definition + resolved + INCLUDES).
    pub all_edges: Vec<Edge>,
    /// Number of files parsed (for IndexResult).
    pub files_parsed: usize,
    /// Number of files skipped (for IndexResult).
    pub files_skipped: usize,
    /// C++ `#include` graph for scope-aware call resolution (BUG-C4 fix).
    /// Populated by `build_includes_edges`; consumed by `CallResolver` (T005).
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
/// (preserving the exact variant and exit code, Rule 12).
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
}

impl Phase for ScanPhase {
    type Input = ScanInput;
    type Output = ScanOutput;
    const NAME: &'static str = "scan";
    fn deps() -> &'static [&'static str] {
        &[]
    }

    fn run(&self, input: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
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

        // Step 3: diff hashes → changed/added/deleted/unchanged.
        let diff = diff_files(&disk_files, &db_hashes, force)
            .map_err(|e| phase_err(Self::NAME, IndexError::Io(e)))?;

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
/// When [`ram_first_compressed`](Self::ram_first_compressed) is `Some`, files
/// are LZ4-decompressed from in-memory buffers instead of read from disk. The
/// buffers are built by [`IndexFacade::index_ram_first`] before the DAG runs.
///
/// [`IndexFacade::index_ram_first`]: super::pipeline::IndexFacade::index_ram_first
#[derive(Default)]
pub struct ParsePhase {
    /// RAM-first mode: LZ4-compressed source bytes keyed by absolute path.
    /// When `Some`, [`parallel_parse_ram_first`] is used instead of
    /// [`parallel_parse`]. When `None`, the default streaming path reads
    /// files from disk.
    pub ram_first_compressed: Option<RamFirstSources>,
}

impl Phase for ParsePhase {
    type Input = ();
    type Output = ParseOutput;
    const NAME: &'static str = "parse";
    fn deps() -> &'static [&'static str] {
        &["scan"]
    }

    fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
        let scan = ctx
            .get::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;

        // Build the list of files to parse: changed + added.
        let mut to_parse: Vec<FileInfo> = scan.diff.changed.clone();
        to_parse.extend(scan.diff.added.iter().cloned());

        // H15: RAM-first path uses LZ4-compressed in-memory buffers; the
        // streaming path reads from disk.
        let parse_result = match &self.ram_first_compressed {
            Some(compressed) => {
                parallel_parse_ram_first(&to_parse, compressed, &scan.project_id)
            }
            None => parallel_parse(&to_parse, &scan.project_id),
        };

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
/// rewriting edge endpoints to match stored node ids (DQ-004).
pub struct ScopeResolutionPhase;

impl Phase for ScopeResolutionPhase {
    type Input = ();
    type Output = ScopeOutput;
    const NAME: &'static str = "scope";
    fn deps() -> &'static [&'static str] {
        &["scan", "parse"]
    }

    fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
        let scan = ctx
            .get::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;
        let parse = ctx
            .get::<ParseOutput>("parse")
            .ok_or(PhaseError::MissingInput("parse"))?;

        let project_id = &scan.project_id;
        let mut graph = Graph::new();
        let mut all_nodes: Vec<Node> = Vec::new();
        let mut all_edges: Vec<Edge> = Vec::new();

        // Add File nodes for every parsed file (used by incremental indexing).
        let file_nodes = build_file_nodes(&scan.diff, project_id);
        for file_node in &file_nodes {
            graph.add_node(file_node.clone());
        }
        all_nodes.extend(file_nodes.iter().cloned());

        // Build mapping from absolute file path → File node id (file_<uuid>).
        let mut path_to_file_id: HashMap<&str, &str> = HashMap::new();
        for file in parse.to_parse.iter() {
            if let Some(abs) = file.path.to_str() {
                let rel = file.relative_path.as_str();
                for fn_node in &file_nodes {
                    if fn_node.name == rel {
                        path_to_file_id.insert(abs, &fn_node.id);
                        break;
                    }
                }
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
                graph.add_node(g.clone());
                all_nodes.push(g);
            }
            for edge in &result.edges {
                let mut e = edge.clone();
                if let Some(file_id) = path_to_file_id.get(e.source.as_str()) {
                    e.source = (*file_id).to_string();
                }
                if let Some(new_target) = id_remap.get(&e.target) {
                    e.target = new_target.clone();
                }
                graph.add_edge(e.clone());
                all_edges.push(e);
            }
        }

        Ok(ScopeOutput {
            graph,
            all_nodes,
            all_edges,
            path_to_rel,
        })
    }
}

// ---------------------------------------------------------------------------
// Helper: build INCLUDES edges for C++ #include (scheme C, v0.3.0)
// ---------------------------------------------------------------------------

/// Builds `EdgeType::Includes` edges for C++ `#include` directives and
/// populates an [`IncludesGraph`] for downstream scope-aware resolution.
///
/// Scheme C (v0.3.0): C++ `#include` is handled separately from other
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
/// A tuple of `(edges, includes_graph)`:
/// - `edges`: all created INCLUDES edges (also added to `graph`)
/// - `includes_graph`: populated graph for `lookup_exported_in_scope` (T004)
fn build_includes_edges(
    results: &[ExtractResult],
    graph: &mut Graph,
    path_to_rel: &HashMap<String, String>,
    project_id: &str,
) -> (Vec<Edge>, IncludesGraph) {
    let mut edges = Vec::new();
    let mut includes_graph = IncludesGraph::new();

    // Build file_path → node_id index from File nodes (file_path is relative,
    // normalized by ScopeResolutionPhase). Used for both source and target
    // File node id lookup.
    let mut file_index: HashMap<String, String> = HashMap::new();
    for node in graph.nodes_by_label(NodeLabel::File) {
        if let Some(fp) = &node.file_path {
            file_index.entry(fp.clone()).or_insert_with(|| node.id.clone());
        }
    }

    // All relative file paths — used by resolve_include for suffix matching.
    // resolve_include matches #include paths as suffixes of these paths.
    let all_files: Vec<String> = file_index.keys().cloned().collect();

    for result in results {
        // Only C++ #include produces INCLUDES edges (scheme C).
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

        for import in &result.imports {
            if import.source_file.is_empty() {
                continue;
            }

            // resolve_include does suffix matching on all_files (relative
            // paths). Returns a relative path that exists in all_files.
            let Some(target_rel) = resolve_include(&import.source_file, &source_rel, &all_files)
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
            graph.add_edge(edge.clone());
            edges.push(edge);
            includes_graph.add_include(&source_rel, &target_rel);
        }
    }

    (edges, includes_graph)
}

// ---------------------------------------------------------------------------
// Phase 4: ResolvePhase
// ---------------------------------------------------------------------------

/// Phase 4: resolve symbols (calls + dataflow + FFI edges).
///
/// Replaces original step 6 of the pipeline. Clones the graph from
/// [`ScopeResolutionPhase`] (the context is immutable during `run`, so
/// mutation requires ownership) and runs the resolvers on it.
pub struct ResolvePhase;

impl Phase for ResolvePhase {
    type Input = ();
    type Output = ResolveOutput;
    const NAME: &'static str = "resolve";
    fn deps() -> &'static [&'static str] {
        &["scan", "parse", "scope"]
    }

    fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
        let scan = ctx
            .get::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;
        let parse = ctx
            .get::<ParseOutput>("parse")
            .ok_or(PhaseError::MissingInput("parse"))?;
        let scope = ctx
            .get::<ScopeOutput>("scope")
            .ok_or(PhaseError::MissingInput("scope"))?;

        let project_id = &scan.project_id;

        // Clone the graph so we can mutate it (ctx is immutable in run).
        let mut graph = scope.graph.clone();
        let mut all_nodes = scope.all_nodes.clone();
        let mut all_edges = scope.all_edges.clone();

        // Resolve calls + dataflow + FFI edges. `TypeResolver::resolve_types`
        // internally builds a path mapping between `result.file_path` (absolute
        // in production) and graph nodes' `file_path` (relative, normalized by
        // ScopeResolutionPhase) so that `imports_map` and `lookup_in_file`
        // lookups succeed despite the path-format difference. See
        // TypeResolver::resolve_types for details.
        let symbol_table = build_symbol_table(&parse.results, project_id);
        let resolved_edges = resolve_all(&parse.results, &symbol_table, project_id, &mut graph);
        all_edges.extend(resolved_edges);

        // Scheme C (v0.3.0): Build INCLUDES edges for C++ #include directives.
        // C++ #include is handled separately from IMPORTS (which is for
        // TS/Rust/Python/Go/Java) because #include establishes a scope-visible
        // relationship that CallResolver uses for cross-file call resolution
        // (BUG-C4 fix). ImportResolver skips C++ results, so no IMPORTS edges
        // are created for C++ — only INCLUDES edges here.
        let (includes_edges, includes_graph) =
            build_includes_edges(&parse.results, &mut graph, &scope.path_to_rel, project_id);
        all_edges.extend(includes_edges);

        // Prune dangling type-reference edges (Implements/Extends/UsesType)
        // from the persisted collection. resolve_all prunes graph.edges, but
        // all_edges is a separate Vec built from scope.all_edges (parse-phase
        // edges) + resolved_edges — both unpruned. Without this, dangling
        // IMPLEMENTS edges (e.g. `impl Display for Foo`) reach the DB.
        let node_ids: std::collections::HashSet<String> =
            graph.nodes.keys().cloned().collect();
        prune_dangling_type_edges_vec(&mut all_edges, &node_ids);

        // Collect Parameter and Variable nodes created during dataflow
        // resolution (DQ-004) so they are persisted alongside other nodes.
        for label in [NodeLabel::Parameter, NodeLabel::Variable] {
            for node in graph.nodes_by_label(label) {
                let mut n = node.clone();
                if let Some(fp) = n.file_path.as_mut() {
                    if let Some(rel) = scope.path_to_rel.get(fp) {
                        *fp = rel.clone();
                    }
                }
                all_nodes.push(n);
            }
        }

        Ok(ResolveOutput {
            all_nodes,
            all_edges,
            files_parsed: parse.files_parsed,
            files_skipped: scan.diff.unchanged.len(),
            includes_graph,
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 5: ConfidencePhase (pass-through, Task 2.8 adds real impl)
// ---------------------------------------------------------------------------

/// Phase 5: confidence scoring (pass-through placeholder).
///
/// Task 2.8 will add real confidence tier assignment to edges. For now this
/// phase is a no-op that ensures LoadPhase runs after ResolvePhase.
pub struct ConfidencePhase;

impl Phase for ConfidencePhase {
    type Input = ();
    type Output = ();
    const NAME: &'static str = "confidence";
    fn deps() -> &'static [&'static str] {
        &["resolve"]
    }

    fn run(&self, _: Self::Input, _ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
        // Pass-through — real confidence scoring added in Task 2.8.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 6: LoadPhase
// ---------------------------------------------------------------------------

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

    fn run(&self, _: Self::Input, ctx: &PipelineCtx) -> Result<Self::Output, PhaseError> {
        let scan = ctx
            .get::<ScanOutput>("scan")
            .ok_or(PhaseError::MissingInput("scan"))?;
        let resolve = ctx
            .get::<ResolveOutput>("resolve")
            .ok_or(PhaseError::MissingInput("resolve"))?;

        let project_id = &scan.project_id;
        let project_name = &scan.project_name;
        let root = &scan.root_path;
        let disk_files = &scan.disk_files;
        let all_nodes = &resolve.all_nodes;
        let all_edges = &resolve.all_edges;

        // Step 7: batch-delete old nodes for deleted + changed files.
        //
        // The batch path collapses N per-file passes (each ~21 Cypher queries
        // over the node-label set) into a single `WHERE n.filePath IN [...]`
        // pass, keeping the query count fixed regardless of how many files
        // changed. This fixes the `incremental_500_of_1000` SLO regression
        // (33 files/s → target ≥100 files/s) — the per-file delete loop was
        // the dominant cost on incremental re-index.
        let mut paths_to_delete: Vec<String> = scan.diff.deleted.clone();
        paths_to_delete.extend(scan.diff.changed.iter().map(|f| f.relative_path.clone()));
        if !paths_to_delete.is_empty() {
            if let Err(err) = self.repo.delete_file_nodes_batch(&paths_to_delete, project_id) {
                warn!(
                    file_count = paths_to_delete.len(),
                    error = %err,
                    "failed to batch delete file nodes for deleted+changed files"
                );
            }
        }

        // Step 8: persist project node, definition nodes, and edges.
        save_project_node(&self.repo, project_id, project_name, root, disk_files)
            .map_err(|e| phase_err(Self::NAME, e))?;
        save_nodes_by_label(&self.repo, all_nodes).map_err(|e| phase_err(Self::NAME, e))?;
        if !all_edges.is_empty() {
            with_retry(DEFAULT_MAX_RETRIES, || {
                self.repo
                    .save_edges(all_edges)
                    .map_err(IndexError::from)
            })
            .map_err(|e| phase_err(Self::NAME, e))?;
        }

        // Step 9: build the IndexResult.
        let duration_ms = scan
            .start
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let files_indexed = resolve.files_parsed;
        let files_skipped = resolve.files_skipped;
        let nodes_created = all_nodes.len();
        let edges_created = all_edges.len();

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
fn lookup_or_create_project_id(repo: &Repository, project_name: &str) -> std::result::Result<String, IndexError> {
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
fn save_project_node(
    repo: &Repository,
    project_id: &str,
    project_name: &str,
    root: &Path,
    disk_files: &[FileInfo],
) -> std::result::Result<(), IndexError> {
    // If the project node already exists, delete only the Project row.
    if repo.get_project(project_id)?.is_some() {
        let cypher = format!(
            "MATCH (p:Project {{id: '{}'}}) DELETE p;",
            project_id.replace('\'', "\\'"),
        );
        let _ = repo.connection().execute(&cypher);
    }
    let last_commit = git_head_commit(root);
    let project_node = Node::builder(NodeLabel::Project, project_name, project_name)
        .id(project_id)
        .properties(serde_json::json!({
            "rootPath": root.display().to_string(),
            "fileCount": disk_files.len() as i64,
            "indexedAt": now_unix_seconds(),
            "lastCommit": last_commit,
        }))
        .build();
    repo.save_project(&project_node)?;
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
fn save_nodes_by_label(repo: &Repository, nodes: &[Node]) -> std::result::Result<(), IndexError> {
    let mut by_label: HashMap<NodeLabel, Vec<Node>> = HashMap::new();
    for node in nodes {
        by_label.entry(node.label).or_default().push(node.clone());
    }
    for (label, group) in by_label {
        if group.is_empty() {
            continue;
        }
        // Project nodes are saved via save_project_node; skip them here.
        if label == NodeLabel::Project {
            continue;
        }
        // Deduplicate by id within this label group.
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut deduped: Vec<Node> = Vec::with_capacity(group.len());
        for node in group {
            if let Some(&idx) = seen.get(&node.id) {
                deduped[idx] = node;
            } else {
                seen.insert(node.id.clone(), deduped.len());
                deduped.push(node);
            }
        }
        with_retry(DEFAULT_MAX_RETRIES, || {
            repo.save_nodes(&deduped, label).map_err(IndexError::from)
        })?;
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

#[cfg(test)]
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
        if let Ok(output) = std::process::Command::new("git")
            .arg("--version")
            .output()
        {
            if output.status.success() {
                assert!(!commit.is_empty(), "CodeNexus repo should have a HEAD commit");
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

    // --- build_includes_edges (scheme C: C++ #include → INCLUDES edges) ---

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
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        let (edges, includes_graph) =
            build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

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
        });
        let results = vec![ts_result];

        let path_to_rel = HashMap::new();
        let (edges, includes_graph) =
            build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        assert!(
            edges.is_empty(),
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
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        let (edges, includes_graph) =
            build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        assert!(
            edges.is_empty(),
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
        });
        let mut foo_result = ExtractResult::new("src/foo.h", Language::Cpp);
        foo_result.imports.push(crate::ir::ImportInfo {
            source_file: "bar.h".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![main_result, foo_result];

        let path_to_rel = HashMap::new();
        let (edges, includes_graph) =
            build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        assert_eq!(edges.len(), 2, "should create 2 INCLUDES edges");
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
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj", Language::Cpp));
        graph.add_node(make_file_node("src/foo.h", "proj", Language::Cpp));

        let mut main_result = ExtractResult::new("/home/dev/proj/src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "foo.h".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![main_result];

        let mut path_to_rel = HashMap::new();
        path_to_rel.insert(
            "/home/dev/proj/src/main.cpp".to_string(),
            "src/main.cpp".to_string(),
        );

        let (edges, includes_graph) =
            build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        assert_eq!(edges.len(), 1, "absolute source path should resolve via path_to_rel");
        assert_eq!(edges[0].source, "src/main.cpp");
        assert_eq!(edges[0].target, "src/foo.h");
        assert!(includes_graph.contains("src/main.cpp", "src/foo.h"));
    }

    #[test]
    fn build_includes_edges_partial_path_include() {
        // C++ #include "fmt/format.h" → include/fmt/format.h (suffix match).
        let mut graph = Graph::new();
        graph.add_node(make_file_node("src/main.cpp", "proj", Language::Cpp));
        graph.add_node(make_file_node("include/fmt/format.h", "proj", Language::Cpp));

        let mut main_result = ExtractResult::new("src/main.cpp", Language::Cpp);
        main_result.imports.push(crate::ir::ImportInfo {
            source_file: "fmt/format.h".to_string(),
            imported_names: vec![],
            line: 1,
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        let (edges, _includes_graph) =
            build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        assert_eq!(edges.len(), 1, "partial-path #include should resolve via suffix matching");
        assert_eq!(edges[0].target, "include/fmt/format.h");
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
        });
        let results = vec![main_result];

        let path_to_rel = HashMap::new();
        let (edges, includes_graph) =
            build_includes_edges(&results, &mut graph, &path_to_rel, "proj");

        assert!(
            edges.is_empty(),
            "no edges when source File node not in graph"
        );
        assert!(includes_graph.is_empty());
    }
}
