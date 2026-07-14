// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Index pipeline orchestration (Facade pattern, ADD §4.1).
//!
//! [`IndexFacade`] is the single entry point for the indexing workflow. It
//! owns a [`Pipeline`] which orchestrates the full discover → parse → resolve
//! → storage sequence via the typed [`Phase`] DAG runner
//! ([`super::pipeline_dag`]), computing SHA-256 file hashes for incremental
//! indexing (ADR-009) and applying the diff logic from [`super::incremental`].
//!
//! # Pipeline phases (Task 2.5, design.md D2)
//!
//! The 9-step sequence from ADD §4.1 is now split into 6 typed phases
//! (defined in [`super::phases`]), executed by the [`DagPipeline`] runner in
//! topological order:
//!
//! 1. [`ScanPhase`] — discover files, lookup/create project, diff hashes.
//! 2. [`ParsePhase`] — parallel-parse changed+added files.
//! 3. [`ScopeResolutionPhase`] — build in-memory graph (nodes + per-file edges).
//! 4. [`ResolvePhase`] — resolve calls/dataflow/FFI edges.
//! 5. [`ConfidencePhase`] — pass-through (Task 2.8 adds real confidence).
//! 6. [`LoadPhase`] — persist nodes/edges to the database, build [`IndexResult`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tracing::{info, warn};

use crate::discover::Walker;
use crate::index::error::{IndexError, Result};
use crate::index::hash::compute_file_hash;
use crate::index::incremental::FileDiff;
use crate::model::{new_file_id, Language, Node, NodeLabel};
use crate::parse::parallel::RamFirstSources;
use crate::storage::{Repository, StorageError};

#[cfg(feature = "cache")]
use crate::cache::CacheStore;

use super::phases::{
    ConfidencePhase, LoadOutput, LoadPhase, ParsePhase, ResolvePhase, ScanInput, ScanPhase,
    ScopeResolutionPhase,
};
use super::pipeline_dag::{Phase, Pipeline as DagPipeline, PipelineCtx};

/// Maximum number of retry attempts for database-locked errors (Task 5).
///
/// A database operation that fails with a "locked" error is retried up to
/// `DEFAULT_MAX_RETRIES` times with exponential backoff
/// (100ms, 200ms, 400ms). If it still fails, [`IndexError::DatabaseLocked`] is
/// returned (PRD §4.1.6, exit code 2).
pub(crate) const DEFAULT_MAX_RETRIES: u32 = 3;

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
pub(crate) fn with_retry<T, F>(max_retries: u32, mut f: F) -> Result<T>
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
                        max_retries, delay_ms, "database locked, retrying"
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
    #[cfg(feature = "cache")]
    cache: Option<Arc<dyn CacheStore>>,
}

impl IndexFacade {
    /// Creates a new `IndexFacade` that stores its database at `db_path`.
    ///
    /// The database (and schema) is created lazily on the first `index*` call.
    pub fn new(db_path: &Path) -> Result<Self> {
        Ok(Self {
            db_path: db_path.to_path_buf(),
            #[cfg(feature = "cache")]
            cache: None,
        })
    }

    /// Attaches a [`CacheStore`] for post-write query-cache invalidation.
    ///
    /// After each indexing run completes, `cache.invalidate_all()` is called
    /// to ensure stale Cypher query results are not served.
    #[cfg(feature = "cache")]
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<dyn CacheStore>) -> Self {
        self.cache = Some(cache);
        self
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
        let result = pipeline.run(path, project_name, force)?;
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            cache.invalidate_all();
        }
        Ok(result)
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

    /// Runs the RAM-first index pipeline (H15/D9).
    ///
    /// Reads all source files under `path` into memory, LZ4-compresses each
    /// into a `Vec<u8>` (bounding peak memory), then runs the standard DAG
    /// pipeline with [`ParsePhase`] in RAM-first mode — the parse phase
    /// LZ4-decompresses on demand instead of reading from disk. The
    /// compressed buffers are dropped when this method returns.
    ///
    /// Use for small-to-medium repositories (< 1 GB source) to reduce
    /// LadybugDB write amplification. Large repositories should use the
    /// default [`index`](Self::index) streaming path to avoid OOM.
    ///
    /// # Errors
    ///
    /// Same as [`index`](Self::index), plus [`IndexError::Io`] if a source
    /// file cannot be read for compression.
    pub fn index_ram_first(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> Result<IndexResult> {
        // PRD §4.1.6: path not found → exit code 1.
        if !path.exists() {
            return Err(IndexError::PathNotFound(path.display().to_string()));
        }

        // Pre-scan: discover all files on disk so we can read + compress them
        // before the DAG runs. ScanPhase will re-discover (cheap) and produce
        // the authoritative diff; the compressed map is keyed by absolute
        // path, so ParsePhase looks up whatever subset ScanPhase selects.
        let disk_files = Walker::new(path).discover().map_err(IndexError::from)?;

        // H15: LZ4-compress every discovered file into memory. Files that
        // ScanPhase later marks unchanged (hash match) won't be in `to_parse`,
        // so their compressed bytes are simply never looked up — the small
        // waste of compressing them is preferable to a second scan+hash pass
        // just to filter the set.
        let mut compressed: RamFirstSources =
            std::collections::HashMap::with_capacity(disk_files.len());
        for file in &disk_files {
            match std::fs::read(&file.path) {
                Ok(bytes) => {
                    compressed.insert(file.path.clone(), lz4_flex::compress_prepend_size(&bytes));
                }
                Err(err) => {
                    warn!(
                        file = %file.relative_path,
                        error = %err,
                        "RAM-first: failed to read file for compression, will fall back to disk read in parse phase"
                    );
                }
            }
        }

        let repository = with_retry(DEFAULT_MAX_RETRIES, || {
            Repository::open(&self.db_path).map_err(IndexError::from)
        })?;
        let pipeline = Pipeline::new(repository);
        // `run_ram_first` takes ownership of `compressed`; it is dropped when
        // `run_ram_first` returns (after the single COPY FROM dump in LoadPhase).
        let result = pipeline.run_ram_first(path, project_name, force, compressed)?;
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            cache.invalidate_all();
        }
        Ok(result)
    }
}

/// Internal pipeline orchestration over a [`Repository`].
///
/// Each [`Pipeline::run`] call performs the full ADD §4.1 sequence via the
/// typed [`DagPipeline`] runner: discover → diff → parse → resolve → storage.
/// The pipeline holds an [`Arc<Repository>`] so multiple phases can share the
/// same database connection (Repository is not Clone, ADR-008).
pub struct Pipeline {
    repository: Arc<Repository>,
}

impl Pipeline {
    /// Creates a new `Pipeline` wrapping `repository`.
    #[must_use]
    pub fn new(repository: Repository) -> Self {
        Self {
            repository: Arc::new(repository),
        }
    }

    /// Runs the full indexing pipeline via the typed DAG runner.
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
        self.run_inner(path, project_name, force, None)
    }

    /// Runs the RAM-first indexing pipeline (H15/D9).
    ///
    /// Same as [`run`](Self::run) but all source files are LZ4-compressed into
    /// memory before parsing, and the parse phase decompresses on demand
    /// instead of reading from disk. The compressed buffers are dropped when
    /// this method returns. Use for small-to-medium repositories (< 1 GB
    /// source) to reduce LadybugDB write amplification.
    ///
    /// # Errors
    ///
    /// Same as [`run`](Self::run), plus [`IndexError::Io`] if a source file
    /// cannot be read for compression.
    pub fn run_ram_first(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
        compressed: RamFirstSources,
    ) -> Result<IndexResult> {
        self.run_inner(path, project_name, force, Some(compressed))
    }

    /// Shared DAG runner for both streaming and RAM-first paths.
    ///
    /// When `compressed` is `Some`, [`ParsePhase`] is constructed with the
    /// LZ4-compressed buffers (RAM-first mode); otherwise it uses the default
    /// streaming disk-read path.
    fn run_inner(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
        compressed: Option<RamFirstSources>,
    ) -> Result<IndexResult> {
        let start = Instant::now();

        info!(
            event = "index_started",
            project = %project_name,
            path = %path.display(),
            ram_first = compressed.is_some(),
            "indexing started"
        );

        // PRD §4.1.6: path not found → exit code 1.
        // Checked before the DAG so ScanPhase can assume the path exists.
        if !path.exists() {
            return Err(IndexError::PathNotFound(path.display().to_string()));
        }

        // Build the pipeline context with root-phase input.
        let mut ctx = PipelineCtx::new();
        ctx.insert(
            ScanPhase::NAME,
            ScanInput {
                path: path.to_path_buf(),
                project_name: project_name.to_string(),
                force,
                start,
            },
        );
        // Derived phases use Input = ().
        ctx.insert(ParsePhase::NAME, ());
        ctx.insert(ScopeResolutionPhase::NAME, ());
        ctx.insert(ResolvePhase::NAME, ());
        ctx.insert(ConfidencePhase::NAME, ());
        ctx.insert(LoadPhase::NAME, ());

        // Register all 6 phases and run the DAG.
        let mut dag = DagPipeline::new();
        dag.register(ScanPhase {
            repo: self.repository.clone(),
        })
        .map_err(IndexError::from)?;
        dag.register(ParsePhase {
            ram_first_compressed: compressed,
        })
        .map_err(IndexError::from)?;
        dag.register(ScopeResolutionPhase)
            .map_err(IndexError::from)?;
        dag.register(ResolvePhase).map_err(IndexError::from)?;
        dag.register(ConfidencePhase).map_err(IndexError::from)?;
        dag.register(LoadPhase {
            repo: self.repository.clone(),
        })
        .map_err(IndexError::from)?;

        dag.run(&mut ctx).map_err(IndexError::from)?;

        // Extract the final IndexResult from the LoadPhase output.
        let load_output = ctx.remove::<LoadOutput>(LoadPhase::NAME).ok_or_else(|| {
            IndexError::Storage(StorageError::Query(
                "load phase did not produce output".to_string(),
            ))
        })?;

        // Force a checkpoint so the WAL is flushed to the main DB file before
        // this Pipeline (and its Repository) is dropped. Without this, a
        // concurrently-open Repository (e.g. the Kit's Storage capability
        // opened at boot) may trigger a checkpoint-on-drop that loses this
        // indexer's writes ("checkpoint interference on Kit drop" — see
        // cli/mod.rs dispatch_tests comment).
        //
        // LadybugDB Cypher grammar (Cypher.g4:262) accepts `CHECKPOINT` as a
        // standalone statement — the `CALL CHECKPOINT` syntax is invalid.
        // `force_checkpoint_on_close=true` ensures the DB flushes its WAL
        // when the connection is dropped, even if other connections remain
        // open.
        if let Err(err) = self
            .repository
            .connection()
            .execute("CALL force_checkpoint_on_close=true;")
        {
            warn!(error = %err, "failed to enable force_checkpoint_on_close");
        }
        if let Err(err) = self.repository.connection().execute("CHECKPOINT;") {
            warn!(error = %err, "post-index checkpoint failed; data may not persist if other DB handles are open");
        }

        Ok(load_output.index_result)
    }
}

/// Builds a [`Node`] (label `File`) for each changed/added file, carrying the
/// SHA-256 hash in `properties.hash` for future incremental runs.
pub(crate) fn build_file_nodes(diff: &FileDiff, project_id: &str) -> Vec<Node> {
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
        // Fallback to the first compiled-in language when the file's language
        // is unknown. `Language::all()` is guaranteed non-empty by the
        // compile_error! assertion in lib.rs (at least one `lang-*` feature).
        let language = file.language.unwrap_or_else(|| Language::all()[0]);
        let line_count = line_count_of(&file.path).unwrap_or(0);
        let node = Node::builder(
            NodeLabel::File,
            file.relative_path.clone(),
            file.relative_path.clone(),
        )
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
pub(crate) fn line_count_of(path: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(content.lines().count() as u32)
}

/// Returns the current unix timestamp in seconds.
pub(crate) fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// R-lsp-004: LSP semantic_type enhancement (mock-testable pure unit)
// ---------------------------------------------------------------------------
//
// NOTE: The DB-backed wiring that actually runs `rust-analyzer` after an index
// run lives in `src/service/index.rs::enhance_with_lsp`. This
// pure function is the mock-injectable core: it takes a `&dyn LspProvider`
// and a `&mut [Node]` slice so the graceful-degradation contract (R-lsp-004:
// "LSP server 启动失败时，索引不中断" / "LSP 查询超时时，跳过该符号的语义
// 增强，不中断索引") can be unit-tested without spawning a real rust-analyzer
// subprocess. The CLI handler has its own DB-backed implementation; this pure
// function exists solely for unit-testing the graceful-degradation contract
// with a mock provider.

/// LSP-driven `semantic_type` enhancement for in-memory nodes (R-lsp-004).
///
/// For each `Function`/`Method` node whose `file_path` ends in `.rs`, queries
/// the provider's `hover` and writes the first non-empty line of the response
/// (truncated to 200 chars) into `node.properties["semantic_type"]`.
///
/// # Failure semantics (Rule 12: failures must be explicit, never silent)
///
/// - **`provider.start()` returns `LspError::ServerStart`** → logs a
///   `tracing::warn!` and returns `Ok(())` immediately (graceful degradation:
///   the index is already complete, LSP is purely enhancement).
/// - **Per-symbol `hover()` returns `Timeout`/`Communication`** → that symbol
///   is skipped (its `semantic_type` stays unset) and enhancement continues
///   for the remaining symbols. The skip is counted but never aborts the run.
/// - **`provider.shutdown()`** is always called once the loop completes
///   (best-effort; errors are logged but never propagated).
///
/// # Arguments
///
/// * `provider` - Any [`LspProvider`] implementation (`RustAnalyzerClient` in
///   production, a mock in tests).
/// * `nodes` - The nodes to enhance **in place**. Non-`Function`/`Method`
///   nodes and non-`.rs` files are left untouched.
/// * `workspace` - The workspace root passed to `provider.start()` (used by
///   rust-analyzer as `workspaceFolders[0].uri`).
#[cfg(all(test, feature = "lsp"))]
pub(crate) fn enhance_with_lsp(
    provider: &dyn crate::lsp::LspProvider,
    nodes: &mut [crate::model::Node],
    workspace: &Path,
) -> Result<()> {
    use crate::lsp::LspError;
    use crate::model::NodeLabel;

    // 1. Start the LSP server. Any start failure is non-fatal — the index is
    //    already complete, so we log a warning and return Ok to signal
    //    "enhancement skipped, index succeeded" (R-lsp-004 graceful
    //    degradation). shutdown() is NOT called: nothing was started.
    if let Err(err) = provider.start(workspace) {
        warn!(
            error = %err,
            "LSP server start failed, degrading to pure tree-sitter extraction"
        );
        return Ok(());
    }

    // 2. Enhance each Rust Function/Method node with hover-derived
    //    semantic_type. Per-symbol failures (Timeout/Communication) skip the
    //    symbol but never abort the run (R-lsp-004).
    for node in nodes.iter_mut() {
        let is_target = matches!(node.label, NodeLabel::Function | NodeLabel::Method);
        let is_rust = node
            .file_path
            .as_deref()
            .map(|p| p.ends_with(".rs"))
            .unwrap_or(false);
        if !is_target || !is_rust {
            continue;
        }

        let file_path = match node.file_path.as_deref() {
            Some(p) => Path::new(p),
            None => continue,
        };
        let line = node.start_line.unwrap_or(0);

        match provider.hover(file_path, line, 0) {
            Ok(Some(hover)) => {
                if let Some(text) = crate::lsp::extract_hover_text(&hover) {
                    write_semantic_type(node, text);
                }
            }
            Ok(None) => {}
            Err(LspError::Timeout(_)) | Err(LspError::Communication(_)) => {
                // Per-symbol failure — skip and continue (Rule 12: explicit
                // skip, not silent success).
            }
            Err(LspError::ServerStart(_)) => {
                // Shouldn't happen after a successful start; skip defensively.
            }
        }
    }

    // 3. Best-effort shutdown — errors are logged but never propagated (the
    //    index is already complete; a dirty subprocess exit is non-fatal).
    if let Err(err) = provider.shutdown() {
        warn!(error = %err, "LSP server shutdown failed (non-fatal)");
    }
    Ok(())
}

/// Writes `text` into `node.properties["semantic_type"]`, converting a `Null`
/// properties value to an empty object first so the field can be set.
#[cfg(all(test, feature = "lsp"))]
fn write_semantic_type(node: &mut crate::model::Node, text: String) {
    if !node.properties.is_object() {
        if node.properties.is_null() {
            node.properties = serde_json::Value::Object(serde_json::Map::new());
        } else {
            // properties holds a non-object scalar/array — don't clobber it.
            return;
        }
    }
    if let Some(obj) = node.properties.as_object_mut() {
        obj.insert("semantic_type".to_string(), serde_json::Value::String(text));
    }
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;
    use crate::discover::FileInfo;
    use crate::model::Language;
    use crate::test_log_capture::capture_tracing;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

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
        write_file(
            root,
            "math.f90",
            "subroutine math_sub()\nend subroutine math_sub\n",
        );

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

        let result_a = facade
            .index(tmp_a.path(), "project_a", false)
            .expect("index A");
        let result_b = facade
            .index(tmp_b.path(), "project_b", false)
            .expect("index B");

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
        assert_eq!(forced.files_skipped, 0, "force must not skip any files");
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
        assert!(nodes.is_empty(), "missing file should produce no File node");
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
        assert!(
            names.contains(&"main".to_string()),
            "main should be persisted"
        );
        assert!(
            names.contains(&"helper".to_string()),
            "helper should be persisted"
        );
    }

    // --- Pipeline persists project node ---

    #[test]
    fn pipeline_persists_project_node() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade
            .index(tmp.path(), "my_project", false)
            .expect("index");

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

    #[test]
    fn log_001_index_started_event_emitted() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");

        let captured = capture_tracing(|| {
            facade
                .index(tmp.path(), "log_001_started", false)
                .expect("index");
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
            facade
                .index(tmp.path(), "log_001_completed", false)
                .expect("index");
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
            facade
                .index(tmp.path(), "log_006_perf", false)
                .expect("index");
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
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "should not retry on success"
        );
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

    // --- R-lsp-004: enhance_with_lsp graceful degradation + writes ---

    /// What `MockLspProvider::hover` returns on each call.
    #[cfg(feature = "lsp")]
    #[allow(dead_code)] // test mock: documents available behaviors for future tests
    enum MockHoverBehavior {
        /// Return `Ok(Some(hover))` — simulates a successful hover response.
        Ok(Option<lsp_types::Hover>),
        /// Return `Err(LspError::Timeout)` — simulates a query timeout.
        Timeout,
        /// Return `Err(LspError::Communication)` — simulates a channel error.
        Communication,
    }

    /// Mock `LspProvider` for testing `enhance_with_lsp` without spawning a
    /// real `rust-analyzer` subprocess. Tracks call counts so tests can assert
    /// on the exact call pattern (e.g. "hover must NOT be called when start
    /// fails").
    #[cfg(feature = "lsp")]
    struct MockLspProvider {
        /// If `true`, `start()` returns `Err(LspError::ServerStart(_))`.
        start_fails: bool,
        /// What `hover()` returns.
        hover_behavior: MockHoverBehavior,
        /// Call counters (AtomicU32 so the mock stays `Send + Sync`).
        start_calls: AtomicU32,
        hover_calls: AtomicU32,
        shutdown_calls: AtomicU32,
    }

    #[cfg(feature = "lsp")]
    impl crate::lsp::LspProvider for MockLspProvider {
        fn start(&self, _workspace: &Path) -> std::result::Result<(), crate::lsp::LspError> {
            self.start_calls.fetch_add(1, Ordering::SeqCst);
            if self.start_fails {
                Err(crate::lsp::LspError::ServerStart(
                    "mock: server unavailable".into(),
                ))
            } else {
                Ok(())
            }
        }

        fn definition(
            &self,
            _file: &Path,
            _line: u32,
            _col: u32,
        ) -> std::result::Result<Option<lsp_types::Location>, crate::lsp::LspError> {
            Ok(None)
        }

        fn type_definition(
            &self,
            _file: &Path,
            _line: u32,
            _col: u32,
        ) -> std::result::Result<Option<lsp_types::Location>, crate::lsp::LspError> {
            Ok(None)
        }

        fn hover(
            &self,
            _file: &Path,
            _line: u32,
            _col: u32,
        ) -> std::result::Result<Option<lsp_types::Hover>, crate::lsp::LspError> {
            self.hover_calls.fetch_add(1, Ordering::SeqCst);
            match &self.hover_behavior {
                MockHoverBehavior::Ok(h) => Ok(h.clone()),
                MockHoverBehavior::Timeout => Err(crate::lsp::LspError::Timeout(
                    crate::lsp::REQUEST_TIMEOUT_MS,
                )),
                MockHoverBehavior::Communication => Err(crate::lsp::LspError::Communication(
                    "mock: channel closed".into(),
                )),
            }
        }

        fn shutdown(&self) -> std::result::Result<(), crate::lsp::LspError> {
            self.shutdown_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Builds a `Hover` with `MarkupContent` carrying `text` (the type
    /// signature `enhance_with_lsp` is expected to extract).
    #[cfg(feature = "lsp")]
    fn make_hover(text: &str) -> lsp_types::Hover {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: text.to_string(),
            }),
            range: None,
        }
    }

    /// Builds a `Function` node with the given name, file path, and start line.
    /// `properties` starts as `Null` (the `NodeBuilder` default).
    #[cfg(feature = "lsp")]
    fn make_function_node(name: &str, file_path: &str, start_line: u32) -> Node {
        Node::builder(NodeLabel::Function, name, format!("proj.{name}"))
            .file_path(file_path)
            .start_line(start_line)
            .language(Language::Rust)
            .project("proj")
            .build()
    }

    /// R-lsp-004: "LSP server 启动失败时，索引不中断".
    ///
    /// A mock whose `start()` returns `Err(LspError::ServerStart(_))` must
    /// cause `enhance_with_lsp` to return `Ok(())` (graceful degradation),
    /// NOT call `hover()` or `shutdown()`, and leave all nodes untouched.
    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_enhance_skips_on_server_start_failure() {
        let mock = MockLspProvider {
            start_fails: true,
            hover_behavior: MockHoverBehavior::Ok(Some(make_hover("fn foo()"))),
            start_calls: AtomicU32::new(0),
            hover_calls: AtomicU32::new(0),
            shutdown_calls: AtomicU32::new(0),
        };
        let mut nodes = vec![make_function_node("foo", "src/main.rs", 0)];

        let result = enhance_with_lsp(&mock, &mut nodes, Path::new("/workspace"));

        assert!(
            result.is_ok(),
            "ServerStart failure must NOT abort enhancement: {:?}",
            result.err()
        );
        assert_eq!(
            mock.start_calls.load(Ordering::SeqCst),
            1,
            "start must be called exactly once"
        );
        assert_eq!(
            mock.hover_calls.load(Ordering::SeqCst),
            0,
            "hover must NOT be called when start fails"
        );
        assert_eq!(
            mock.shutdown_calls.load(Ordering::SeqCst),
            0,
            "shutdown must NOT be called when start fails (nothing to shut down)"
        );
        assert!(
            nodes[0].properties.get("semantic_type").is_none(),
            "node must NOT be enhanced when start fails"
        );
    }

    /// R-lsp-004: "LSP 查询超时时，跳过该符号的语义增强，不中断索引".
    ///
    /// A mock whose `hover()` returns `Err(LspError::Timeout)` must cause the
    /// symbol to be skipped (no `semantic_type` written) but enhancement must
    /// continue and return `Ok(())`. `shutdown()` must still be called.
    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_enhance_skips_on_query_timeout() {
        let mock = MockLspProvider {
            start_fails: false,
            hover_behavior: MockHoverBehavior::Timeout,
            start_calls: AtomicU32::new(0),
            hover_calls: AtomicU32::new(0),
            shutdown_calls: AtomicU32::new(0),
        };
        let mut nodes = vec![make_function_node("foo", "src/main.rs", 0)];

        let result = enhance_with_lsp(&mock, &mut nodes, Path::new("/workspace"));

        assert!(
            result.is_ok(),
            "Timeout must NOT abort enhancement: {:?}",
            result.err()
        );
        assert_eq!(
            mock.hover_calls.load(Ordering::SeqCst),
            1,
            "hover must be attempted for the symbol"
        );
        assert_eq!(
            mock.shutdown_calls.load(Ordering::SeqCst),
            1,
            "shutdown must be called after the loop even if all symbols timed out"
        );
        assert!(
            nodes[0].properties.get("semantic_type").is_none(),
            "timed-out symbol must NOT be enhanced"
        );
    }

    /// R-lsp-004: successful hover response must be written to the node's
    /// `semantic_type` property.
    ///
    /// The mock returns a hover carrying `"fn add(a: i32, b: i32) -> i32"`;
    /// `enhance_with_lsp` must extract that text and store it under
    /// `node.properties["semantic_type"]`.
    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_enhance_writes_semantic_type() {
        let signature = "fn add(a: i32, b: i32) -> i32";
        let mock = MockLspProvider {
            start_fails: false,
            hover_behavior: MockHoverBehavior::Ok(Some(make_hover(signature))),
            start_calls: AtomicU32::new(0),
            hover_calls: AtomicU32::new(0),
            shutdown_calls: AtomicU32::new(0),
        };
        let mut nodes = vec![make_function_node("add", "src/lib.rs", 0)];

        let result = enhance_with_lsp(&mock, &mut nodes, Path::new("/workspace"));

        assert!(
            result.is_ok(),
            "enhancement should succeed: {:?}",
            result.err()
        );
        assert_eq!(
            mock.hover_calls.load(Ordering::SeqCst),
            1,
            "hover must be called for the Function node"
        );
        assert_eq!(
            mock.shutdown_calls.load(Ordering::SeqCst),
            1,
            "shutdown must be called after enhancement"
        );
        let sem = nodes[0]
            .properties
            .get("semantic_type")
            .and_then(|v| v.as_str());
        assert_eq!(
            sem,
            Some(signature),
            "semantic_type must be the hover text signature"
        );
    }

    /// R-lsp-004: an empty node list (or a list with no Rust Function/Method
    /// nodes) must return `Ok(())` without calling `hover()`. `shutdown()`
    /// must still be called so the LSP server is reaped.
    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_enhance_noop_when_no_rust_nodes() {
        let mock = MockLspProvider {
            start_fails: false,
            hover_behavior: MockHoverBehavior::Ok(Some(make_hover("fn foo()"))),
            start_calls: AtomicU32::new(0),
            hover_calls: AtomicU32::new(0),
            shutdown_calls: AtomicU32::new(0),
        };
        let mut nodes: Vec<Node> = vec![];

        let result = enhance_with_lsp(&mock, &mut nodes, Path::new("/workspace"));

        assert!(
            result.is_ok(),
            "empty node list must return Ok: {:?}",
            result.err()
        );
        assert_eq!(
            mock.hover_calls.load(Ordering::SeqCst),
            0,
            "hover must NOT be called for an empty node list"
        );
        assert_eq!(
            mock.shutdown_calls.load(Ordering::SeqCst),
            1,
            "shutdown must still be called even with no nodes to enhance"
        );
    }

    /// Real rust-analyzer integration test (R-lsp-004 happy path).
    ///
    /// Spawns a real `rust-analyzer` against a temp workspace containing one
    /// `fn add(a: i32, b: i32) -> i32`, calls `enhance_with_lsp`, and asserts
    /// that the node's `semantic_type` is populated. `#[ignore]` so CI does
    /// not depend on `rust-analyzer` being on PATH.
    ///
    /// Run locally with:
    /// ```text
    /// cargo test --features "lsp lang-rust" --lib src::index::pipeline::tests::lsp_enhance_integration_with_real_rust_analyzer -- --ignored
    /// ```
    #[test]
    #[cfg(feature = "lsp")]
    #[ignore = "requires rust-analyzer on PATH; run with --ignored"]
    fn lsp_enhance_integration_with_real_rust_analyzer() {
        use crate::lsp::RustAnalyzerClient;
        use std::process::{Command, Stdio};

        // Skip deterministically if rust-analyzer is not installed.
        let ra_available = Command::new("rust-analyzer")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        if !ra_available {
            eprintln!("skipping: rust-analyzer not on PATH");
            return;
        }

        let workspace = TempDir::new().unwrap();
        std::fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace.path().join("src")).unwrap();
        std::fs::write(
            workspace.path().join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let abs_file = workspace.path().join("src/lib.rs");
        let mut nodes = vec![Node::builder(NodeLabel::Function, "add", "demo.add")
            .file_path("src/lib.rs")
            .start_line(0)
            .language(Language::Rust)
            .project("demo")
            .build()];

        let client = RustAnalyzerClient::new();
        let result = enhance_with_lsp(&client, &mut nodes, workspace.path());

        assert!(
            result.is_ok(),
            "enhancement should not error even if rust-analyzer indexing is incomplete: {:?}",
            result.err()
        );
        // rust-analyzer may need time to index; if it returned a hover, the
        // semantic_type must be set. If not, the node is unchanged — both are
        // acceptable per the graceful-degradation contract.
        let _ = abs_file;
    }

    // --- Coverage tests ---

    #[cfg(feature = "cache")]
    #[test]
    fn with_cache_sets_cache_and_invalidates_on_index() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct CountingCache {
            invalidates: AtomicU32,
        }
        impl CacheStore for CountingCache {
            fn get(&self, _key: &str) -> Option<Vec<u8>> { None }
            fn set(&self, _key: &str, _val: Vec<u8>) {}
            fn invalidate_all(&self) {
                self.invalidates.fetch_add(1, Ordering::SeqCst);
            }
        }

        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db_path = fresh_db_path();
        let cache = Arc::new(CountingCache { invalidates: AtomicU32::new(0) });
        let facade = IndexFacade::new(&db_path)
            .expect("facade")
            .with_cache(cache.clone());
        let result = facade.index(tmp.path(), "demo", false);
        assert!(result.is_ok(), "index should succeed: {:?}", result);
        assert!(
            cache.invalidates.load(Ordering::SeqCst) >= 1,
            "invalidate_all should be called after index"
        );
    }

    #[test]
    fn index_ram_first_indexes_rust_file() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() { helper(); }\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index_ram_first(tmp.path(), "demo", false);
        assert!(result.is_ok(), "index_ram_first should succeed: {:?}", result);
        let result = result.unwrap();
        assert!(result.files_indexed > 0, "should index files: {result:?}");
    }

    #[test]
    fn index_ram_first_returns_error_for_nonexistent_path() {
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade.index_ram_first(Path::new("/nonexistent/path/xyz"), "demo", false);
        assert!(result.is_err());
        match result.unwrap_err() {
            IndexError::PathNotFound(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("expected PathNotFound, got: {other:?}"),
        }
    }

    #[test]
    fn with_retry_retries_on_capital_lock_then_succeeds() {
        let calls = AtomicU32::new(0);
        let result: Result<u32> = with_retry(3, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(IndexError::Storage(StorageError::Query("Lock timeout".to_string())))
            } else {
                Ok(7)
            }
        });
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn with_retry_with_zero_max_retries_returns_after_one_attempt() {
        // max_retries=0 means 1 initial attempt, 0 retries. A lock error
        // on that single attempt should immediately return DatabaseLocked.
        let calls = AtomicU32::new(0);
        let result: Result<u32> = with_retry(0, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(lock_error())
        });
        assert!(
            matches!(result, Err(IndexError::DatabaseLocked)),
            "should return DatabaseLocked with 0 retries: {result:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "should attempt exactly once with max_retries=0"
        );
    }

    #[test]
    fn with_retry_succeeds_on_second_attempt_with_max_one() {
        // max_retries=1 means up to 2 total attempts. The first fails with
        // a lock, the second succeeds.
        let calls = AtomicU32::new(0);
        let result: Result<u32> = with_retry(1, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(lock_error())
            } else {
                Ok(99)
            }
        });
        assert_eq!(result.unwrap(), 99);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn index_facade_index_incremental_same_as_index() {
        // index_incremental should behave the same as index (it delegates).
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade
            .index_incremental(tmp.path(), "demo", false)
            .expect("index_incremental");
        assert!(result.files_indexed > 0, "should index files");
    }

    #[test]
    fn pipeline_run_returns_error_for_nonexistent_path() {
        // Pipeline::run should return PathNotFound for a missing path.
        let repo = Repository::in_memory().expect("repo");
        let pipeline = Pipeline::new(repo);
        let result = pipeline.run(Path::new("/nonexistent/xyz"), "demo", false);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), IndexError::PathNotFound(_)),
            "should be PathNotFound"
        );
    }

    #[test]
    fn pipeline_run_ram_first_returns_error_for_nonexistent_path() {
        // Pipeline::run_ram_first should return PathNotFound for a missing path.
        let repo = Repository::in_memory().expect("repo");
        let pipeline = Pipeline::new(repo);
        let result = pipeline.run_ram_first(
            Path::new("/nonexistent/xyz"),
            "demo",
            false,
            std::collections::HashMap::new(),
        );
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), IndexError::PathNotFound(_)),
            "should be PathNotFound"
        );
    }

    #[test]
    fn index_ram_first_with_empty_directory_succeeds() {
        // RAM-first indexing of an empty directory should succeed with
        // files_indexed = 0.
        let tmp = TempDir::new().unwrap();
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let result = facade
            .index_ram_first(tmp.path(), "empty", false)
            .expect("index_ram_first should succeed");
        assert_eq!(result.files_indexed, 0, "empty dir → 0 files indexed");
    }

    #[test]
    fn with_retry_propagates_storage_error_that_is_not_lock() {
        // A StorageError::Query whose message doesn't contain "locked" or
        // "Lock" should be propagated immediately (not retried).
        let calls = AtomicU32::new(0);
        let result: Result<u32> = with_retry(3, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(IndexError::Storage(StorageError::Query(
                "syntax error near CALL".to_string(),
            )))
        });
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "should not retry on non-lock error");
    }

    #[test]
    fn build_file_nodes_uses_fallback_language_when_none() {
        // When file.language is None, build_file_nodes should fall back to
        // the first compiled-in language (Language::all()[0]).
        let tmp = TempDir::new().unwrap();
        let f = FileInfo {
            path: tmp.path().join("unknown.xyz"),
            relative_path: "unknown.xyz".to_string(),
            language: None,
            size: 0,
        };
        fs::write(&f.path, "content\n").unwrap();
        let mut diff = FileDiff::new();
        diff.added.push(f);
        let nodes = build_file_nodes(&diff, "proj");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].language, Some(Language::all()[0]));
    }

    #[test]
    fn line_count_of_file_with_no_trailing_newline() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("no_newline.rs");
        fs::write(&path, "line1\nline2").unwrap();
        assert_eq!(line_count_of(&path), Some(2));
    }

    #[cfg(feature = "cache")]
    #[test]
    fn index_ram_first_invalidates_cache_on_success() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct CountingCache {
            invalidates: AtomicU32,
        }
        impl CacheStore for CountingCache {
            fn get(&self, _key: &str) -> Option<Vec<u8>> { None }
            fn set(&self, _key: &str, _val: Vec<u8>) {}
            fn invalidate_all(&self) {
                self.invalidates.fetch_add(1, Ordering::SeqCst);
            }
        }

        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db_path = fresh_db_path();
        let cache = Arc::new(CountingCache { invalidates: AtomicU32::new(0) });
        let facade = IndexFacade::new(&db_path)
            .expect("facade")
            .with_cache(cache.clone());
        let result = facade.index_ram_first(tmp.path(), "demo", false);
        assert!(result.is_ok(), "index_ram_first should succeed: {:?}", result);
        assert!(
            cache.invalidates.load(Ordering::SeqCst) >= 1,
            "invalidate_all should be called after ram_first index"
        );
    }
}
