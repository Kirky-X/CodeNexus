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
use crate::model::{Edge, Graph, Node, NodeLabel, new_project_id};
use crate::parse::parallel_parse;
use crate::resolve::{build_symbol_table, resolve_all};
use crate::storage::Repository;

use super::pipeline::IndexResult;
use super::pipeline_dag::{Phase, PhaseError, PipelineCtx};

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
    /// All edges to persist (definition + resolved).
    pub all_edges: Vec<Edge>,
    /// Number of files parsed (for IndexResult).
    pub files_parsed: usize,
    /// Number of files skipped (for IndexResult).
    pub files_skipped: usize,
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
pub struct ParsePhase;

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

        // Parallel-parse the files.
        let parse_result = parallel_parse(&to_parse, &scan.project_id);

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

        // Resolve calls + dataflow + FFI edges.
        let symbol_table = build_symbol_table(&parse.results, project_id);
        let resolved_edges = resolve_all(&parse.results, &symbol_table, project_id, &mut graph);
        all_edges.extend(resolved_edges);

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

        // Step 7: delete old nodes for deleted files and changed files.
        for deleted_path in &scan.diff.deleted {
            if let Err(err) = self.repo.delete_file_nodes(deleted_path, project_id) {
                warn!(file = %deleted_path, error = %err, "failed to delete file nodes");
            }
        }
        for changed_file in &scan.diff.changed {
            if let Err(err) = self
                .repo
                .delete_file_nodes(&changed_file.relative_path, project_id)
            {
                warn!(
                    file = %changed_file.relative_path,
                    error = %err,
                    "failed to delete changed file nodes"
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
    let project_node = Node::builder(NodeLabel::Project, project_name, project_name)
        .id(project_id)
        .properties(serde_json::json!({
            "rootPath": root.display().to_string(),
            "fileCount": disk_files.len() as i64,
            "indexedAt": now_unix_seconds(),
        }))
        .build();
    repo.save_project(&project_node)?;
    Ok(())
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
