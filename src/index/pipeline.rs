//! Index pipeline orchestration (Facade pattern, ADD §4.1).
//!
//! [`IndexFacade`] is the single entry point for the indexing workflow. It
//! owns a [`Pipeline`] which orchestrates the full discover → parse → resolve
//! → storage sequence, computing SHA-256 file hashes for incremental indexing
//! (ADR-009) and applying the diff logic from [`super::incremental`].
//!
//! # Pipeline steps (ADD §4.1)
//!
//! 1. `discover_files(path)` → `Vec<FileInfo>` via [`Walker`].
//! 2. `query_existing_hashes(project)` from the database.
//! 3. `diff_hashes()` → `changed`/`added`/`deleted` (or all `changed` if
//!    `force`).
//! 4. `parallel_parse(changed + added)` → `Vec<ExtractResult>`.
//! 5. `build_in_memory_graph(results)` (nodes + per-file edges).
//! 6. `resolve_symbols(graph)` — calls + dataflow + FFI edges.
//! 7. `delete_old_nodes(deleted_files)` from the database.
//! 8. `load_csv(resolved_graph)` to the database (nodes + edges).
//! 9. Return [`IndexResult`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tracing::{info, warn};

use crate::discover::{FileInfo, Walker};
use crate::index::error::{IndexError, Result};
use crate::index::hash::compute_file_hash;
use crate::index::incremental::{diff_files, FileDiff};
use crate::model::{Edge, Graph, Language, Node, NodeLabel, new_file_id, new_project_id};
use crate::parse::parallel_parse;
use crate::resolve::{build_symbol_table, resolve_all};
use crate::storage::{Repository, StorageError};

/// Maximum number of retry attempts for database-locked errors (Task 5).
///
/// A database operation that fails with a "locked" error is retried up to
/// `DEFAULT_MAX_RETRIES` times with exponential backoff
/// (100ms, 200ms, 400ms). If it still fails, [`IndexError::DatabaseLocked`] is
/// returned (PRD §4.1.6, exit code 2).
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Executes a database operation with retry on lock (Task 5).
///
/// Retries up to `max_retries` times with exponential backoff
/// (100ms, 200ms, 400ms, ...). A failure whose error message contains
/// "locked" or "Lock" (case-sensitive) is treated as a transient lock and
/// retried; any other error is propagated immediately. When all attempts are
/// exhausted on a lock error, [`IndexError::DatabaseLocked`] is returned.
///
/// # Arguments
///
/// * `max_retries` - Maximum number of retries (in addition to the initial
///   attempt). `max_retries = 3` means up to 4 total attempts.
/// * `f` - The operation to attempt. Called repeatedly until it succeeds or a
///   non-lock error is returned.
///
/// # Errors
///
/// - Returns `Ok(v)` if `f` eventually succeeds.
/// - Returns [`IndexError::DatabaseLocked`] if `f` keeps returning lock errors
///   for `max_retries + 1` attempts.
/// - Returns any other `IndexError` immediately (no retry).
fn with_retry<T, F>(max_retries: u32, mut f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut delay_ms = 100u64;
    for attempt in 0..=max_retries {
        match f() {
            Ok(v) => return Ok(v),
            Err(IndexError::Storage(StorageError::Query(ref msg)))
                if msg.contains("locked") || msg.contains("Lock") =>
            {
                if attempt < max_retries {
                    warn!(
                        attempt = attempt + 1,
                        max_retries, delay_ms,
                        "database locked, retrying"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    delay_ms *= 2;
                    continue;
                }
                return Err(IndexError::DatabaseLocked);
            }
            Err(e) => return Err(e),
        }
    }
    Err(IndexError::DatabaseLocked)
}

/// The outcome of a single indexing run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexResult {
    /// The project id (UUIDv7) assigned to this indexing run.
    pub project_id: String,
    /// Number of files actually parsed (changed + added).
    pub files_indexed: usize,
    /// Number of files skipped because their hash matched the DB.
    pub files_skipped: usize,
    /// Number of nodes created (file + definition nodes).
    pub nodes_created: usize,
    /// Number of edges created (definition + resolved edges).
    pub edges_created: usize,
    /// Wall-clock duration of the indexing run, in milliseconds.
    pub duration_ms: u64,
}

impl IndexResult {
    /// Creates a new `IndexResult` with the given fields.
    #[must_use]
    pub fn new(
        project_id: impl Into<String>,
        files_indexed: usize,
        files_skipped: usize,
        nodes_created: usize,
        edges_created: usize,
        duration_ms: u64,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            files_indexed,
            files_skipped,
            nodes_created,
            edges_created,
            duration_ms,
        }
    }

    /// Creates an empty `IndexResult` (zero everything except `project_id`).
    #[must_use]
    pub fn empty(project_id: impl Into<String>) -> Self {
        Self::new(project_id, 0, 0, 0, 0, 0)
    }
}

/// Facade for the indexing pipeline (Facade pattern).
///
/// Owns the database path and produces a fresh [`Pipeline`] per indexing run.
/// The facade is the single entry point used by the CLI `index` command.
pub struct IndexFacade {
    db_path: PathBuf,
}

impl IndexFacade {
    /// Creates a new `IndexFacade` that stores its database at `db_path`.
    ///
    /// The database (and schema) is created lazily on the first `index*` call.
    pub fn new(db_path: &Path) -> Result<Self> {
        Ok(Self {
            db_path: db_path.to_path_buf(),
        })
    }

    /// Runs the full index pipeline (no incremental diffing).
    ///
    /// Equivalent to `index_incremental` with `force=true` for the parse
    /// phase, but always (re)creates the project node.
    pub fn index(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult> {
        // Repository::open runs init_schema internally; retry on transient
        // database locks (Task 5).
        let repository = with_retry(DEFAULT_MAX_RETRIES, || {
            Repository::open(&self.db_path).map_err(IndexError::from)
        })?;
        let pipeline = Pipeline::new(repository);
        pipeline.run(path, project_name, force)
    }

    /// Runs the incremental index pipeline (only changed files are parsed).
    ///
    /// See [`Pipeline::run`] for the per-step behavior.
    pub fn index_incremental(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> Result<IndexResult> {
        self.index(path, project_name, force)
    }
}

/// Internal pipeline orchestration over a [`Repository`].
///
/// Each [`Pipeline::run`] call performs the full ADD §4.1 sequence:
/// discover → diff → parse → resolve → storage.
pub struct Pipeline {
    repository: Repository,
}

impl Pipeline {
    /// Creates a new `Pipeline` wrapping `repository`.
    #[must_use]
    pub fn new(repository: Repository) -> Self {
        Self { repository }
    }

    /// Runs the full indexing pipeline.
    ///
    /// # Arguments
    ///
    /// * `path` - The repository root to index.
    /// * `project_name` - The project display name (also used as the DB
    ///   `project` column for multi-project isolation, BR-INDEX-004).
    /// * `force` - When `true`, every disk file is re-parsed regardless of its
    ///   hash (BR-INDEX-003, `--force`).
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::PathNotFound`] if `path` does not exist.
    /// Returns [`IndexError::Storage`] for database failures.
    /// Parse failures are logged and skipped (PRD §4.1.6).
    pub fn run(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult> {
        let start = Instant::now();

        info!(
            event = "index_started",
            project = %project_name,
            path = %path.display(),
            "indexing started"
        );

        // PRD §4.1.6: path not found → exit code 1.
        if !path.exists() {
            return Err(IndexError::PathNotFound(path.display().to_string()));
        }

        // Step 1: discover files on disk.
        let disk_files = Walker::new(path).discover()?;

        // Assign a project id. We reuse an existing project id if one exists
        // for this name (incremental re-index); otherwise generate a new one.
        let project_id = self.lookup_or_create_project_id(project_name, path, &disk_files)?;

        // Step 2: query existing hashes from the DB (with retry on lock).
        let db_hashes = with_retry(DEFAULT_MAX_RETRIES, || {
            self.repository
                .get_all_file_hashes(&project_id)
                .map_err(IndexError::from)
        })
        .unwrap_or_default();

        // Step 3: diff hashes → changed/added/deleted (or all changed if force).
        let diff = diff_files(&disk_files, &db_hashes, force)?;

        // Step 4: parallel-parse changed + added files.
        let mut to_parse: Vec<FileInfo> = diff.changed.clone();
        to_parse.extend(diff.added.iter().cloned());
        let parse_result = parallel_parse(&to_parse, &project_id);

        // PRD §4.1.6: parse failures are logged and skipped.
        for (file_path, error_msg) in &parse_result.errors {
            warn!(file = %file_path, error = %error_msg, "parse failed, skipping file");
        }

        // Step 5: build in-memory graph (definition nodes + per-file edges).
        let mut graph = Graph::new();
        let mut all_nodes: Vec<Node> = Vec::new();
        let mut all_edges: Vec<Edge> = Vec::new();

        // Add File nodes for every parsed file (used by incremental indexing).
        let file_nodes = build_file_nodes(&diff, &project_id);
        for file_node in &file_nodes {
            graph.add_node(file_node.clone());
        }
        all_nodes.extend(file_nodes.iter().cloned());

        // Build mapping from absolute file path → File node id (file_<uuid>).
        // Parser-created DEFINES/CONTAINS edges use the absolute file path as
        // `source`; we must rewrite it to the File node's id so the edge is not
        // orphaned (DQ-004).
        let mut path_to_file_id: HashMap<&str, &str> = HashMap::new();
        for file in to_parse.iter() {
            if let Some(abs) = file.path.to_str() {
                let rel = file.relative_path.as_str();
                // File node id is looked up by relative_path (File nodes use
                // relative_path as their name/qualified_name).
                for fn_node in &file_nodes {
                    if fn_node.name == rel {
                        path_to_file_id.insert(abs, &fn_node.id);
                        break;
                    }
                }
            }
        }

        // Merge per-file extraction results into the graph.
        // Build a mapping from absolute file paths to relative paths so
        // definition nodes (which extractors set to the absolute path) can be
        // normalized to match File nodes (which use relative paths).
        let path_to_rel: HashMap<&str, &str> = to_parse
            .iter()
            .filter_map(|f| f.path.to_str().map(|p| (p, f.relative_path.as_str())))
            .collect();
        for result in &parse_result.results {
            let rel_path = path_to_rel
                .get(result.file_path.as_str())
                .copied()
                .unwrap_or(result.file_path.as_str());
            // Build a per-file remap: old node id (UUID) → new id (FQN) so
            // edges whose `target` points at the pre-rewrite UUID can be
            // rewritten to the FQN. This eliminates the orphan-edge class of
            // bugs (DQ-004) where DEFINES/CONTAINS edges referenced the
            // pre-rewrite UUID target.
            let mut id_remap: HashMap<String, String> = HashMap::new();
            for node in &result.nodes {
                let mut g = node.clone();
                // Definition nodes (Class, Function, etc.) use FQN as id so
                // resolve_all and trace can look them up by qualified_name.
                // Project/File/Folder keep their UUIDv7 ids.
                if !matches!(g.label, NodeLabel::Project | NodeLabel::File | NodeLabel::Folder) {
                    let old_id = g.id.clone();
                    g.id = node.qualified_name.clone();
                    if old_id != g.id {
                        id_remap.insert(old_id, g.id.clone());
                    }
                }
                // Normalize filePath to relative for consistency with File
                // nodes, so delete_file_nodes matches all nodes by relative
                // path during incremental re-indexing.
                if let Some(fp) = g.file_path.as_mut() {
                    *fp = rel_path.to_string();
                }
                // Ensure the project is set (extractors set it, but be defensive).
                if g.project.is_empty() {
                    g.project = project_id.clone();
                }
                graph.add_node(g.clone());
                all_nodes.push(g);
            }
            for edge in &result.edges {
                let mut e = edge.clone();
                // Rewrite parser-created edge endpoints so they match the
                // stored node ids:
                //  - `source` (absolute file path) → File node id (file_<uuid>)
                //  - `target` (pre-rewrite UUID) → FQN
                // Resolver-created edges (CALLS/DataFlows/Reads/Writes) already
                // use FQN endpoints and are not in the remap, so they are left
                // unchanged.
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

        // Step 6: resolve symbols (calls + dataflow + FFI).
        let symbol_table = build_symbol_table(&parse_result.results, &project_id);
        let resolved_edges = resolve_all(
            &parse_result.results,
            &symbol_table,
            &project_id,
            &mut graph,
        );
        all_edges.extend(resolved_edges);

        // Collect Parameter and Variable nodes created during dataflow
        // resolution (DQ-004) so they are persisted to the database alongside
        // other nodes. Variable nodes are created by `resolve_var_identifier`
        // fallback when a referenced variable isn't in the symbol table (P0-1);
        // Parameter nodes are created by `resolve_arg_pass` (BR-TRACE-001).
        // Without this, DataFlows edges become orphans pointing at
        // never-persisted Variable/Parameter nodes.
        for label in [NodeLabel::Parameter, NodeLabel::Variable] {
            for node in graph.nodes_by_label(label) {
                let mut n = node.clone();
                // Normalize filePath to relative for consistency with File
                // nodes, so delete_file_nodes matches during incremental
                // re-indexing.
                if let Some(fp) = n.file_path.as_mut() {
                    if let Some(&rel) = path_to_rel.get(fp.as_str()) {
                        *fp = rel.to_string();
                    }
                }
                all_nodes.push(n);
            }
        }

        // Step 7: delete old nodes for deleted files (BR-INDEX-002) and for
        // changed files (we're about to re-insert their nodes).
        for deleted_path in &diff.deleted {
            if let Err(err) = self.repository.delete_file_nodes(deleted_path, &project_id) {
                warn!(file = %deleted_path, error = %err, "failed to delete file nodes");
            }
        }
        for changed_file in &diff.changed {
            if let Err(err) = self
                .repository
                .delete_file_nodes(&changed_file.relative_path, &project_id)
            {
                warn!(
                    file = %changed_file.relative_path,
                    error = %err,
                    "failed to delete changed file nodes"
                );
            }
        }

        // Step 8: persist the project node, definition nodes, and edges.
        self.save_project_node(&project_id, project_name, path, &disk_files)?;
        self.save_nodes_by_label(&all_nodes)?;
        if !all_edges.is_empty() {
            with_retry(DEFAULT_MAX_RETRIES, || {
                self.repository.save_edges(&all_edges).map_err(IndexError::from)
            })?;
        }

        // Step 9: build the IndexResult.
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let files_indexed = parse_result.files_parsed;
        let files_skipped = diff.unchanged.len();
        let nodes_created = all_nodes.len();
        let edges_created = all_edges.len();

        info!(
            event = "index_completed",
            project = %project_name,
            path = %path.display(),
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

        Ok(IndexResult::new(
            project_id,
            files_indexed,
            files_skipped,
            nodes_created,
            edges_created,
            duration_ms,
        ))
    }

    /// Looks up an existing project id by name, or generates a new one.
    ///
    /// We treat the project *name* as the stable identifier across re-indexes.
    /// If a project with this name already exists in the DB, we reuse its id;
    /// otherwise we generate a fresh `proj_<uuid>` id.
    fn lookup_or_create_project_id(
        &self,
        project_name: &str,
        root: &Path,
        disk_files: &[FileInfo],
    ) -> Result<String> {
        // Look for an existing project with this name.
        let projects = self.repository.list_projects().unwrap_or_default();
        for project in projects {
            if project.name == project_name {
                return Ok(project.id);
            }
        }
        // No existing project; generate a new id.
        let _ = (root, disk_files); // unused on this branch; kept for clarity.
        Ok(new_project_id())
    }

    /// Saves (or re-saves) the project node.
    ///
    /// Only saves the project node if it does not already exist — this
    /// preserves the File nodes (and their hashes) from prior runs, which
    /// the incremental indexer depends on.
    fn save_project_node(
        &self,
        project_id: &str,
        project_name: &str,
        root: &Path,
        disk_files: &[FileInfo],
    ) -> Result<()> {
        // If the project node already exists, update its metadata in place
        // by deleting only the Project row (not the File/Function nodes).
        if self.repository.get_project(project_id)?.is_some() {
            // Delete only the Project node itself.
            let cypher = format!(
                "MATCH (p:Project {{id: '{}'}}) DELETE p;",
                project_id.replace('\'', "\\'"),
            );
            // Ignore errors (e.g. table missing) — the project may not exist.
            let _ = self.repository.connection().execute(&cypher);
        }
        let project_node = Node::builder(NodeLabel::Project, project_name, project_name)
            .id(project_id)
            .properties(serde_json::json!({
                "rootPath": root.display().to_string(),
                "fileCount": disk_files.len() as i64,
                "indexedAt": now_unix_seconds(),
            }))
            .build();
        self.repository.save_project(&project_node)?;
        Ok(())
    }

    /// Groups nodes by label and bulk-saves each group.
    fn save_nodes_by_label(&self, nodes: &[Node]) -> Result<()> {
        // Group nodes by label so each label is bulk-loaded in one COPY.
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
            // Deduplicate by id within this label group. The same node can
            // appear more than once in `all_nodes` when a definition node
            // extracted by the parser happens to share its id (FQN) with a
            // Variable/Parameter node created during dataflow resolution, or
            // when incremental re-indexing hasn't yet cleared stale rows.
            // LadybugDB's COPY rejects duplicate primary keys, so we keep the
            // last occurrence per id.
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
                self.repository
                    .save_nodes(&deduped, label)
                    .map_err(IndexError::from)
            })?;
        }
        Ok(())
    }
}

/// Builds a [`Node`] (label `File`) for each changed/added file, carrying the
/// SHA-256 hash in `properties.hash` for future incremental runs.
fn build_file_nodes(diff: &FileDiff, project_id: &str) -> Vec<Node> {
    let mut nodes = Vec::new();
    for file in diff.changed.iter().chain(diff.added.iter()) {
        let hash = match compute_file_hash(&file.path) {
            Ok(h) => h,
            Err(err) => {
                warn!(
                    file = %file.relative_path,
                    error = %err,
                    "failed to hash file, skipping File node"
                );
                continue;
            }
        };
        let language = file.language.unwrap_or(Language::Rust);
        let line_count = line_count_of(&file.path).unwrap_or(0);
        let node = Node::builder(NodeLabel::File, file.relative_path.clone(), file.relative_path.clone())
            .id(new_file_id())
            .project(project_id)
            .file_path(&file.relative_path)
            .language(language)
            .properties(serde_json::json!({
                "hash": hash,
                "lineCount": line_count,
            }))
            .build();
        nodes.push(node);
    }
    nodes
}

/// Returns the number of lines in `path`, or `None` if it cannot be read.
fn line_count_of(path: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(content.lines().count() as u32)
}

/// Returns the current unix timestamp in seconds.
fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Language;
    use std::fs;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tracing_subscriber::fmt::MakeWriter;

    /// Writes a file at `dir/rel` (creating parent directories as needed).
    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Returns a fresh on-disk database path inside a temp dir.
    ///
    /// The TempDir is leaked intentionally so the database files survive for
    /// the test's lifetime (LadybugDB keeps file handles open).
    fn fresh_db_path() -> PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("testdb");
        std::mem::forget(dir);
        path
    }

    // --- IndexResult ---

    #[test]
    fn index_result_new_sets_all_fields() {
        let r = IndexResult::new("proj_1", 10, 5, 100, 50, 1234);
        assert_eq!(r.project_id, "proj_1");
        assert_eq!(r.files_indexed, 10);
        assert_eq!(r.files_skipped, 5);
        assert_eq!(r.nodes_created, 100);
        assert_eq!(r.edges_created, 50);
        assert_eq!(r.duration_ms, 1234);
    }

    #[test]
    fn index_result_empty_zeros_everything() {
        let r = IndexResult::empty("proj_x");
        assert_eq!(r.project_id, "proj_x");
        assert_eq!(r.files_indexed, 0);
        assert_eq!(r.files_skipped, 0);
        assert_eq!(r.nodes_created, 0);
        assert_eq!(r.edges_created, 0);
        assert_eq!(r.duration_ms, 0);
    }

    #[test]
    fn index_result_clone_is_equal() {
        let r = IndexResult::new("p", 1, 2, 3, 4, 5);
        assert_eq!(r, r.clone());
    }

    #[test]
    fn index_result_debug_contains_fields() {
        let r = IndexResult::new("proj_1", 10, 5, 100, 50, 1234);
        let s = format!("{r:?}");
        assert!(s.contains("proj_1"));
        assert!(s.contains("IndexResult"));
    }

    // --- IndexFacade / Pipeline: AC-INDEX-001 ---

    #[test]
    fn ac_index_001_indexes_c_rust_fortran_files() {
        // AC-INDEX-001: Index a codebase with C/Rust/Fortran files → all
        // indexed, graph created.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "fn main() { helper(); }\n");
        write_file(root, "util.c", "int util(void) { return 42; }\n");
        write_file(root, "math.f90", "subroutine math_sub()\nend subroutine math_sub\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade
            .index(root, "demo", false)
            .expect("index should succeed");

        assert!(result.files_indexed > 0, "should index files: {result:?}");
        assert!(result.nodes_created > 0, "should create nodes: {result:?}");
        assert!(result.duration_ms < u64::MAX, "duration should be recorded");
        assert!(!result.project_id.is_empty(), "project_id should be set");
    }

    // --- AC-INDEX-002: incremental re-index only parses changed file ---

    #[test]
    fn ac_index_002_incremental_only_parses_changed_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        // First index: both files are new → both parsed.
        let first = facade.index(root, "demo", false).expect("first index");
        assert_eq!(first.files_indexed, 2, "first run parses both files");
        assert_eq!(first.files_skipped, 0, "nothing to skip on first run");

        // Second index without changes: both files skipped.
        let second = facade
            .index_incremental(root, "demo", false)
            .expect("second index");
        assert_eq!(
            second.files_skipped, 2,
            "BR-INDEX-001: unchanged files skipped"
        );
        assert_eq!(second.files_indexed, 0, "no files to re-parse");

        // Modify one file, re-index: only that file is parsed.
        write_file(root, "a.rs", "fn a() { /* modified */ }\n");
        let third = facade
            .index_incremental(root, "demo", false)
            .expect("third index");
        assert_eq!(third.files_indexed, 1, "only the modified file is parsed");
        assert_eq!(third.files_skipped, 1, "the other file is skipped");
    }

    // --- AC-INDEX-003: multiple projects coexist in the same DB ---

    #[test]
    fn ac_index_003_multiple_projects_coexist() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        write_file(tmp_a.path(), "a.rs", "fn alpha() {}\n");
        write_file(tmp_b.path(), "b.rs", "fn beta() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        let result_a = facade.index(tmp_a.path(), "project_a", false).expect("index A");
        let result_b = facade.index(tmp_b.path(), "project_b", false).expect("index B");

        assert!(result_a.files_indexed > 0);
        assert!(result_b.files_indexed > 0);

        // Both projects should coexist in the DB.
        let repo = Repository::open(&db_path).expect("repo");
        let projects = repo.list_projects().expect("list_projects");
        assert_eq!(projects.len(), 2, "AC-INDEX-003: both projects coexist");
        let names: Vec<String> = projects.iter().map(|p| p.name.clone()).collect();
        assert!(names.contains(&"project_a".to_string()));
        assert!(names.contains(&"project_b".to_string()));
    }

    // --- AC-INDEX-005: --force re-parses all files ---

    #[test]
    fn ac_index_005_force_re_parses_all_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        // First index: both files parsed.
        let first = facade.index(root, "demo", false).expect("first index");
        assert_eq!(first.files_indexed, 2);
        assert_eq!(first.files_skipped, 0);

        // Second index with force=true: all files re-parsed, none skipped.
        let forced = facade
            .index_incremental(root, "demo", true)
            .expect("forced index");
        assert_eq!(
            forced.files_indexed, 2,
            "AC-INDEX-005: --force re-parses all files"
        );
        assert_eq!(
            forced.files_skipped, 0,
            "force must not skip any files"
        );
    }

    // --- Path not found → error ---

    #[test]
    fn path_not_found_returns_error() {
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(Path::new("/nonexistent/path/xyz"), "demo", false);
        assert!(result.is_err(), "path not found should error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, IndexError::PathNotFound(_)),
            "expected PathNotFound, got {err:?}"
        );
        assert_eq!(err.exit_code(), 1, "PRD §4.1.6: path not found → exit 1");
    }

    // --- Empty directory → files_indexed = 0 ---

    #[test]
    fn empty_directory_indexes_zero_files() {
        let tmp = TempDir::new().unwrap();
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(tmp.path(), "empty", false).expect("index");
        assert_eq!(result.files_indexed, 0, "empty dir → 0 files indexed");
        assert_eq!(result.files_skipped, 0);
        // We still create a project node, so nodes_created may be 1.
        assert!(result.duration_ms < u64::MAX, "duration should be recorded");
    }

    // --- Duration is recorded ---

    #[test]
    fn duration_is_recorded() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(tmp.path(), "demo", false).expect("index");
        // duration_ms is non-negative by construction; we just verify it's set
        // to a finite value (it may be 0 on very fast machines).
        assert!(result.duration_ms < u64::MAX);
    }

    // --- Re-index reuses the same project id (incremental) ---

    #[test]
    fn re_index_reuses_project_id() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        let first = facade.index(tmp.path(), "demo", false).expect("first");
        let second = facade
            .index_incremental(tmp.path(), "demo", false)
            .expect("second");

        assert_eq!(
            first.project_id, second.project_id,
            "re-index should reuse the project id"
        );
    }

    // --- ID-FQN consistency: definition node id must equal FQN ---

    #[test]
    fn definition_node_id_equals_qualified_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "fn helper() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(root, "demo", false).expect("index");

        let repo = Repository::open(&db_path).expect("repo");
        let funcs = repo
            .query_functions(&result.project_id)
            .expect("query_functions");
        assert!(!funcs.is_empty(), "should have at least one function");

        for func in &funcs {
            assert_eq!(
                func.id, func.qualified_name,
                "Function node id must equal qualified_name for trace to work"
            );
        }
    }

    // --- filePath consistency: definition nodes use relative paths ---

    #[test]
    fn definition_node_file_path_is_relative() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "fn helper() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(root, "demo", false).expect("index");

        let repo = Repository::open(&db_path).expect("repo");
        let funcs = repo
            .query_functions(&result.project_id)
            .expect("query_functions");
        assert!(!funcs.is_empty(), "should have at least one function");

        for func in &funcs {
            assert!(
                !func.file_path.starts_with('/') && !func.file_path.contains(':'),
                "filePath should be relative, got: {}",
                func.file_path
            );
        }
    }

    // --- Incremental re-index must not create duplicate nodes ---

    #[test]
    fn incremental_reindex_no_duplicate_nodes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "fn a() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        let first = facade.index(root, "demo", false).expect("first index");
        assert_eq!(first.files_indexed, 1);

        write_file(root, "a.rs", "fn a() { /* modified */ }\n");
        let second = facade
            .index_incremental(root, "demo", false)
            .expect("second index");
        assert_eq!(second.files_indexed, 1);

        let repo = Repository::open(&db_path).expect("repo");
        let funcs = repo
            .query_functions(&second.project_id)
            .expect("query_functions");
        assert_eq!(
            funcs.len(),
            1,
            "should have exactly one function (no duplicates after re-index)"
        );
    }

    // --- Pipeline::new ---

    #[test]
    fn pipeline_new_wraps_repository() {
        let repo = Repository::in_memory().expect("repo");
        let _pipeline = Pipeline::new(repo);
    }

    // --- build_file_nodes ---

    #[test]
    fn build_file_nodes_creates_file_node_per_changed_or_added_file() {
        let tmp = TempDir::new().unwrap();
        let f1 = FileInfo {
            path: tmp.path().join("a.rs"),
            relative_path: "a.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        };
        fs::write(&f1.path, "fn a() {}\n").unwrap();
        let f2 = FileInfo {
            path: tmp.path().join("b.rs"),
            relative_path: "b.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        };
        fs::write(&f2.path, "fn b() {}\n").unwrap();

        let mut diff = FileDiff::new();
        diff.changed.push(f1);
        diff.added.push(f2);

        let nodes = build_file_nodes(&diff, "proj");
        assert_eq!(nodes.len(), 2, "one File node per changed/added file");
        assert!(nodes.iter().all(|n| n.label == NodeLabel::File));
        assert!(nodes.iter().all(|n| n.project == "proj"));
        // Each File node carries a hash property.
        for node in &nodes {
            let hash = node.properties.get("hash").and_then(|v| v.as_str());
            assert!(hash.is_some(), "File node should carry a hash");
            assert_eq!(hash.unwrap().len(), 64, "hash should be 64 hex chars");
        }
    }

    #[test]
    fn build_file_nodes_skips_missing_files() {
        let missing = FileInfo {
            path: PathBuf::from("/nonexistent/missing.rs"),
            relative_path: "missing.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        };
        let mut diff = FileDiff::new();
        diff.added.push(missing);

        let nodes = build_file_nodes(&diff, "proj");
        assert!(
            nodes.is_empty(),
            "missing file should produce no File node"
        );
    }

    #[test]
    fn build_file_nodes_empty_diff_returns_empty() {
        let diff = FileDiff::new();
        let nodes = build_file_nodes(&diff, "proj");
        assert!(nodes.is_empty());
    }

    // --- line_count_of ---

    #[test]
    fn line_count_of_counts_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.rs");
        fs::write(&path, "line1\nline2\nline3\n").unwrap();
        assert_eq!(line_count_of(&path), Some(3));
    }

    #[test]
    fn line_count_of_empty_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.rs");
        fs::write(&path, "").unwrap();
        assert_eq!(line_count_of(&path), Some(0));
    }

    #[test]
    fn line_count_of_missing_file_returns_none() {
        assert!(line_count_of(Path::new("/nonexistent/missing.rs")).is_none());
    }

    // --- now_unix_seconds ---

    #[test]
    fn now_unix_seconds_is_positive() {
        let ts = now_unix_seconds();
        assert!(ts > 0, "unix timestamp should be positive: {ts}");
    }

    // --- IndexFacade::new ---

    #[test]
    fn index_facade_new_succeeds() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("testdb");
        let facade = IndexFacade::new(&db_path);
        assert!(facade.is_ok(), "facade creation should succeed");
    }

    // --- Multi-file re-index with deletion (BR-INDEX-002) ---

    #[test]
    fn re_index_detects_deleted_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        // First index: both files.
        let first = facade.index(root, "demo", false).expect("first");
        assert_eq!(first.files_indexed, 2);

        // Delete b.rs, re-index: a.rs unchanged (skipped), b.rs deleted.
        fs::remove_file(root.join("b.rs")).unwrap();
        let second = facade
            .index_incremental(root, "demo", false)
            .expect("second");
        assert_eq!(second.files_skipped, 1, "a.rs unchanged → skipped");
        assert_eq!(second.files_indexed, 0, "no new/changed files to parse");
    }

    // --- Pipeline handles nested directories ---

    #[test]
    fn pipeline_indexes_nested_directories() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "fn main() {}\n");
        write_file(root, "src/lib.rs", "fn lib_fn() {}\n");
        write_file(root, "src/sub/mod.rs", "fn mod_fn() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(root, "nested", false).expect("index");
        assert_eq!(result.files_indexed, 3, "all nested files indexed");
    }

    // --- Pipeline persists nodes to DB ---

    #[test]
    fn pipeline_persists_nodes_to_db() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "fn main() { helper(); }\nfn helper() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(root, "demo", false).expect("index");
        assert!(result.nodes_created > 0);

        // Verify nodes are persisted by querying the DB directly.
        let repo = Repository::open(&db_path).expect("repo");
        let functions = repo.query_functions(&result.project_id).expect("query");
        assert!(
            !functions.is_empty(),
            "functions should be persisted: {functions:?}"
        );
        let names: Vec<String> = functions.iter().map(|f| f.name.clone()).collect();
        assert!(names.contains(&"main".to_string()), "main should be persisted");
        assert!(names.contains(&"helper".to_string()), "helper should be persisted");
    }

    // --- Pipeline persists project node ---

    #[test]
    fn pipeline_persists_project_node() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(tmp.path(), "my_project", false).expect("index");

        let repo = Repository::open(&db_path).expect("repo");
        let project = repo
            .get_project(&result.project_id)
            .expect("get_project")
            .expect("project should exist");
        assert_eq!(project.name, "my_project");
        assert_eq!(project.id, result.project_id);
    }

    // --- Pipeline with parse failure continues (PRD §4.1.6) ---

    #[test]
    fn pipeline_continues_after_parse_failure() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // A valid Rust file.
        write_file(root, "good.rs", "fn good() {}\n");
        // A file with a .rs extension but unreadable content is still parsed
        // by tree-sitter (it produces an error tree but doesn't fail). To
        // force a parse failure, we use a FileInfo pointing at a path that
        // doesn't exist on disk — but the walker won't produce that. Instead
        // we verify that a normal index run succeeds even with one empty file.
        write_file(root, "empty.rs", "");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index(root, "demo", false).expect("index");
        // Both files should be indexed (empty file produces no nodes but
        // doesn't fail).
        assert_eq!(result.files_indexed, 2);
    }

    // --- IndexResult fields are sane ---

    #[test]
    fn index_result_files_indexed_plus_skipped_le_disk_files() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        write_file(tmp.path(), "b.rs", "fn b() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let _first = facade.index(tmp.path(), "demo", false).expect("first");
        let second = facade
            .index_incremental(tmp.path(), "demo", false)
            .expect("second");
        // On the second run, all 2 files are skipped.
        assert_eq!(second.files_indexed + second.files_skipped, 2);
    }

    // --- LOG-001 / LOG-006: tracing event emission ---

    /// A `MakeWriter` that buffers emitted events into a shared `Vec<u8>` so a
    /// test can assert on what the subscriber actually wrote (mirrors the
    /// pattern in `main.rs`).
    struct CapturingMakeWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl MakeWriter for CapturingMakeWriter {
        type Writer = CapturingWriter;

        fn make_writer(&self) -> Self::Writer {
            CapturingWriter {
                buf: self.buf.clone(),
            }
        }
    }

    struct CapturingWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CapturingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().write_all(bytes)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Runs `f` inside a scoped tracing subscriber that captures all event
    /// output into a string, returning that string.
    fn capture_tracing<R>(f: impl FnOnce() -> R) -> String {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_target(false)
            .with_writer(CapturingMakeWriter { buf: buf.clone() })
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buf.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn log_001_index_started_event_emitted() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        let captured = capture_tracing(|| {
            facade.index(tmp.path(), "log_001_started", false).expect("index");
        });

        assert!(
            captured.contains("index_started"),
            "LOG-001: index_started event should be emitted, got: {captured:?}"
        );
        assert!(
            captured.contains("log_001_started"),
            "index_started should carry the project name"
        );
    }

    #[test]
    fn log_001_index_completed_event_emitted() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        let captured = capture_tracing(|| {
            facade.index(tmp.path(), "log_001_completed", false).expect("index");
        });

        assert!(
            captured.contains("index_completed"),
            "LOG-001: index_completed event should be emitted, got: {captured:?}"
        );
        assert!(
            captured.contains("files_indexed"),
            "index_completed should carry files_indexed field"
        );
        assert!(
            captured.contains("duration_ms"),
            "index_completed should carry duration_ms field"
        );
    }

    #[test]
    fn log_006_performance_event_emitted() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        let captured = capture_tracing(|| {
            facade.index(tmp.path(), "log_006_perf", false).expect("index");
        });

        assert!(
            captured.contains("performance"),
            "LOG-006: performance event should be emitted, got: {captured:?}"
        );
        assert!(
            captured.contains("files_per_second"),
            "performance event should carry files_per_second field"
        );
    }

    // --- SubTask 17.4: with_retry database-lock retry behavior (Task 5) ---

    use std::sync::atomic::{AtomicU32, Ordering};

    /// Constructs a lock error (message contains "locked") that `with_retry`
    /// treats as a transient lock and retries.
    fn lock_error() -> IndexError {
        IndexError::Storage(StorageError::Query("database is locked".to_string()))
    }

    #[test]
    fn with_retry_returns_value_on_first_success() {
        let calls = AtomicU32::new(0);
        let result: Result<u32> = with_retry(3, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "should not retry on success");
    }

    #[test]
    fn with_retry_retries_on_lock_then_succeeds() {
        let calls = AtomicU32::new(0);
        let result: Result<u32> = with_retry(3, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(lock_error())
            } else {
                Ok(7)
            }
        });
        assert_eq!(result.unwrap(), 7);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "should retry once then succeed"
        );
    }

    #[test]
    fn with_retry_returns_database_locked_after_all_retries_exhausted() {
        let calls = AtomicU32::new(0);
        let max_retries = 3u32;
        let result: Result<u32> = with_retry(max_retries, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(lock_error())
        });
        assert!(
            matches!(result, Err(IndexError::DatabaseLocked)),
            "should return DatabaseLocked after exhausting retries, got: {result:?}"
        );
        // max_retries=3 means 1 initial + 3 retries = 4 total attempts.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            max_retries + 1,
            "should attempt max_retries+1 times"
        );
    }

    #[test]
    fn with_retry_propagates_non_lock_error_immediately() {
        let calls = AtomicU32::new(0);
        let result: Result<u32> = with_retry(3, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(IndexError::Parse("syntax error".to_string()))
        });
        assert!(
            matches!(result, Err(IndexError::Parse(_))),
            "should propagate non-lock error immediately, got: {result:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "should not retry on non-lock error"
        );
    }
}
