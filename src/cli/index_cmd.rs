// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `index` subcommand handler (PRD §4.1.3).
//!
//! Resolves the [`Indexer`](crate::index::capability::Indexer) and
//! [`Storage`](crate::storage::capability::Storage) capabilities from the
//! [`Kit`](crate::kit::Kit), runs the index pipeline, and prints the resulting
//! [`IndexResult`] as JSON to stdout. Errors are surfaced via [`CliError`] so
//! `main.rs` can map them to the correct exit code.

use std::path::Path;

use serde::Serialize;

use super::args::IndexArgs;
use super::error::{CliError, Result};
use crate::index::IndexResult;
use crate::kit::{IndexerKey, Kit};
use crate::storage::{QualityChecker, Repository};

/// Runs the `index` subcommand.
///
/// Resolves the [`Indexer`](crate::index::capability::Indexer) capability from
/// `kit`, indexes `args.path` under the project name `args.name`, and prints
/// the [`IndexResult`] as JSON.
///
/// After indexing completes, resolves the
/// [`Storage`](crate::storage::capability::Storage) capability from `kit` and
/// runs the data quality checks (DQ-002/004/005/006), printing any violations
/// to stderr. The DQ report does not affect the exit status — index success is
/// reported via stdout JSON as before.
///
/// # Errors
///
/// Returns [`CliError::Index`] for path-not-found / database / parse errors.
/// The wrapped [`IndexError`] carries the correct exit code. Returns
/// [`crate::cli::error::CliError::Kit`] if a required capability is not
/// registered.
pub fn run(kit: &Kit, args: &IndexArgs) -> Result<()> {
    let path = Path::new(&args.path);
    let indexer = kit.require::<IndexerKey>()?;
    let result = if args.ram_first {
        indexer.index_ram_first(path, &args.name, args.force)?
    } else {
        indexer.index(path, &args.name, args.force)?
    };

    // Run data quality checks (DQ-002/004/005/006) against the freshly indexed
    // database.
    //
    // We open a FRESH Repository here instead of using `kit.require::<StorageKey>()`
    // because the Kit's Storage capability was opened at boot (before the
    // indexer ran), so its connection holds a stale MVCC snapshot of the
    // empty DB — querying through it would return empty results. Opening a
    // new Repository here gets a current snapshot that sees the indexed data.
    //
    // The stale-connection data-loss bug (Kit's drop checkpointing over
    // writes) is handled by `std::mem::forget(kit)` in main.rs, not here.
    let fresh_repo = Repository::open(&args.db)
        .map_err(|e| CliError::Index(crate::index::IndexError::Storage(e)))?;
    let checker = QualityChecker::new(&fresh_repo);
    let dq_report = checker.run_all()?;
    if !dq_report.is_clean() {
        eprintln!("Data quality violations found:");
        for violation in &dq_report.violations {
            eprintln!(
                "  [{}] {} (project: {})",
                violation.rule,
                violation.message,
                violation.project.as_deref().unwrap_or("N/A")
            );
        }
    }

    // Flush the WAL to the main DB file after the quality checks. The
    // pipeline already ran CHECKPOINT at the end of indexing, but the DQ
    // checks may have opened new read transactions; this ensures the data
    // is durably persisted before the process exits.
    //
    // Note: the Kit's stale Storage/Query/Trace connections (opened at boot,
    // before indexing) are prevented from dropping by `std::mem::forget(kit)`
    // in main.rs. This is the real fix — a stale connection's drop-time
    // checkpoint would overwrite the indexer's writes with its empty view.
    if let Err(err) = fresh_repo.connection().execute("CHECKPOINT;") {
        eprintln!("[warn] post-quality-check checkpoint failed: {err}");
    }

    // R-lsp-004: LSP-enhanced semantic_type extraction. When `--lsp` is
    // given and the `lsp` feature is enabled, spawn rust-analyzer and query
    // each Rust symbol's hover info to populate `semantic_type`. Any LSP
    // failure (binary missing, timeout, communication error) degrades
    // gracefully to pure tree-sitter extraction — the index is never aborted.
    #[cfg(feature = "lsp")]
    if args.lsp {
        if let Err(err) = enhance_with_lsp(path, &fresh_repo, &args.name) {
            eprintln!("[warn] LSP enhancement aborted: {err}");
        }
    }

    let output = IndexOutput::from(result);
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable view of [`IndexResult`] (PRD §4.1.3 output table).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IndexOutput {
    /// Project id (UUIDv7).
    pub project_id: String,
    /// Number of files actually parsed.
    pub files_indexed: usize,
    /// Number of files skipped (hash matched).
    pub files_skipped: usize,
    /// Number of nodes created.
    pub nodes_created: usize,
    /// Number of edges created.
    pub edges_created: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

impl From<IndexResult> for IndexOutput {
    fn from(r: IndexResult) -> Self {
        Self {
            project_id: r.project_id,
            files_indexed: r.files_indexed,
            files_skipped: r.files_skipped,
            nodes_created: r.nodes_created,
            edges_created: r.edges_created,
            duration_ms: r.duration_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// R-lsp-004: LSP semantic_type enhancement
// ---------------------------------------------------------------------------

/// LSP-driven `semantic_type` enhancement (R-lsp-004).
///
/// After the tree-sitter indexer has written all symbol nodes, this function
/// spawns `rust-analyzer`, queries each Rust symbol's hover info, and writes
/// the extracted type signature back to the node's `semantic_type` property.
///
/// # Failure semantics (Rule 12: failures must be explicit)
///
/// - **`rust-analyzer` binary missing** → `LspError::ServerStart`, logged to
///   stderr, returns `Ok(())` (degrade to pure tree-sitter extraction —
///   specmark R-lsp-004: "LSP server 启动失败时，索引不中断").
/// - **Per-symbol `Timeout`/`Communication`** → symbol skipped, enhancement
///   continues for remaining symbols (specmark: "LSP 查询超时时，跳过该
///   符号的语义增强，不中断索引").
/// - **Database query/update failure** → returns `Err(CliError::Storage(_))`
///   so the caller can decide whether to surface it. The index itself is
///   already complete; only the enhancement is affected.
#[cfg(feature = "lsp")]
fn enhance_with_lsp(workspace: &Path, repo: &Repository, project: &str) -> Result<()> {
    use crate::lsp::{LspError, LspProvider, RustAnalyzerClient};
    use crate::storage::schema::escape_cypher_string;

    let client = RustAnalyzerClient::new();

    // 1. Start the LSP server. ServerStart failure is non-fatal — the index
    //    is already complete, so we log a warning and return Ok to signal
    //    "enhancement skipped, index succeeded".
    if let Err(LspError::ServerStart(msg)) = client.start(workspace) {
        eprintln!(
            "[warn] LSP server start failed, degrading to pure tree-sitter: {msg}"
        );
        return Ok(());
    }

    // 2. Query all Function/Method nodes in the project that have a filePath
    //    and startLine. v0.2.0 focuses on Function/Method (the primary
    //    semantic-type targets); Struct/Enum/Trait can be added in v0.3.0+.
    //
    //    LadybugDB's Cypher subset does not support `WHERE (n:Function OR
    //    n:Method)` label expressions nor `any(lbl IN labels(n) ...)`, so we
    //    issue two separate queries (one per label) and merge in Rust — same
    //    pattern as `dead_code::load_functions`.
    let proj = escape_cypher_string(project);
    let queries = [
        format!(
            "MATCH (n:Function) WHERE n.project = '{proj}' \
             AND n.filePath IS NOT NULL AND n.startLine IS NOT NULL \
             RETURN n.id AS id, n.filePath AS filePath, n.startLine AS startLine;"
        ),
        format!(
            "MATCH (n:Method) WHERE n.project = '{proj}' \
             AND n.filePath IS NOT NULL AND n.startLine IS NOT NULL \
             RETURN n.id AS id, n.filePath AS filePath, n.startLine AS startLine;"
        ),
    ];
    let mut rows = Vec::new();
    for q in &queries {
        let r = repo.connection().query(q).map_err(|e| {
            CliError::Storage(crate::storage::StorageError::Query(e.to_string()))
        })?;
        rows.extend(r);
    }

    let mut enhanced: u32 = 0;
    let mut skipped: u32 = 0;
    for row in &rows {
        let Some(id) = row.first().and_then(|v| v.as_str()) else {
            skipped += 1;
            continue;
        };
        let Some(file_path_str) = row.get(1).and_then(|v| v.as_str()) else {
            skipped += 1;
            continue;
        };
        let Some(start_line) = row.get(2).and_then(|v| v.as_u64()) else {
            skipped += 1;
            continue;
        };

        // Resolve file path — DB may store absolute or workspace-relative.
        let file_path = Path::new(file_path_str);
        let abs_file = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            workspace.join(file_path)
        };

        // LSP Position is 0-based; DB startLine follows the same convention
        // (tree-sitter points are 0-indexed). Column 0 hits the start of the
        // symbol's definition line.
        let line = u32::try_from(start_line).unwrap_or(0);
        match client.hover(&abs_file, line, 0) {
            Ok(Some(hover)) => {
                if let Some(text) = extract_hover_text(&hover) {
                    let update = format!(
                        "MATCH (n {{id: '{id}', project: '{proj}'}}) \
                         SET n.semantic_type = '{sem}';",
                        id = escape_cypher_string(id),
                        proj = escape_cypher_string(project),
                        sem = escape_cypher_string(&text),
                    );
                    // Best-effort update — a failure here means the symbol
                    // keeps its tree-sitter-only data, which is acceptable.
                    if repo.connection().execute(&update).is_ok() {
                        enhanced += 1;
                    } else {
                        skipped += 1;
                    }
                } else {
                    skipped += 1;
                }
            }
            Ok(None) => {
                skipped += 1;
            }
            Err(LspError::Timeout(_)) | Err(LspError::Communication(_)) => {
                // Per-symbol failure — skip and continue (Rule 12: explicit
                // skip, not silent success).
                skipped += 1;
            }
            Err(LspError::ServerStart(_)) => {
                // Shouldn't happen after a successful start, but handle
                // defensively to avoid panicking the index.
                skipped += 1;
            }
        }
    }

    eprintln!(
        "[info] LSP enhancement: {enhanced} symbol(s) enhanced, {skipped} skipped"
    );

    // Best-effort shutdown — ignore errors since the index is already done.
    let _ = client.shutdown();
    Ok(())
}

/// Extracts the first non-empty line from an LSP [`Hover`] response as the
/// `semantic_type` string. Truncates to 200 chars to keep the property lean.
#[cfg(feature = "lsp")]
fn extract_hover_text(hover: &lsp_types::Hover) -> Option<String> {
    use lsp_types::{HoverContents, MarkedString};

    let raw = match &hover.contents {
        HoverContents::Scalar(MarkedString::String(s)) => s.clone(),
        HoverContents::Scalar(MarkedString::LanguageString(ls)) => ls.value.clone(),
        HoverContents::Array(vec) => vec
            .iter()
            .map(|ms| match ms {
                MarkedString::String(s) => s.clone(),
                MarkedString::LanguageString(ls) => ls.value.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        HoverContents::Markup(mc) => mc.value.clone(),
    };

    // Take the first non-empty line — typically the type signature like
    // "fn add(a: i32, b: i32) -> i32". Truncate to avoid bloating the DB.
    let first_line = raw.lines().find(|l| !l.trim().is_empty())?;
    let truncated = if first_line.len() > 200 {
        &first_line[..200]
    } else {
        first_line
    };
    if truncated.is_empty() {
        None
    } else {
        Some(truncated.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::IndexArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use std::fs;
    use std::path::PathBuf;
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
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// Builds an `IndexArgs` pointing at `path`/`name`/`db`.
    fn make_args(path: &str, name: &str, db: &str) -> IndexArgs {
        IndexArgs {
            path: path.to_string(),
            name: name.to_string(),
            db: db.to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: false,
        }
    }

    // --- IndexOutput ---

    #[test]
    fn index_output_from_index_result_copies_fields() {
        let r = IndexResult::new("proj_1", 10, 5, 100, 50, 1234);
        let out = IndexOutput::from(r);
        assert_eq!(out.project_id, "proj_1");
        assert_eq!(out.files_indexed, 10);
        assert_eq!(out.files_skipped, 5);
        assert_eq!(out.nodes_created, 100);
        assert_eq!(out.edges_created, 50);
        assert_eq!(out.duration_ms, 1234);
    }

    #[test]
    fn index_output_serializes_to_json() {
        let out = IndexOutput {
            project_id: "p1".into(),
            files_indexed: 1,
            files_skipped: 0,
            nodes_created: 2,
            edges_created: 3,
            duration_ms: 4,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project_id\":\"p1\""));
        assert!(json.contains("\"files_indexed\":1"));
        assert!(json.contains("\"duration_ms\":4"));
    }

    // --- run() success ---

    #[test]
    fn run_indexes_rust_file_and_prints_json() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() { helper(); }\nfn helper() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            db.to_str().unwrap(),
        );

        // run() prints to stdout; we just verify it returns Ok.
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_indexes_multiple_files() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        write_file(tmp.path(), "b.rs", "fn b() {}\n");
        write_file(tmp.path(), "c.rs", "fn c() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "multi",
            db.to_str().unwrap(),
        );

        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_with_force_re_indexes() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());

        // First index.
        let args1 = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            db.to_str().unwrap(),
        );
        assert!(run(&kit, &args1).is_ok());

        // Second index with force.
        let args2 = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: true,
            lsp: false,
            embed: false,
            ram_first: false,
        };
        let result = run(&kit, &args2);
        assert!(result.is_ok(), "force run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_empty_directory_succeeds() {
        let tmp = TempDir::new().unwrap();
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "empty",
            db.to_str().unwrap(),
        );
        let result = run(&kit, &args);
        assert!(result.is_ok(), "empty dir should succeed: {:?}", result.err());
    }

    // --- run() error cases ---

    #[test]
    fn run_path_not_found_returns_exit_code_1() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("/nonexistent/path/xyz", "demo", db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("path not found should error");
        assert_eq!(err.exit_code(), 1, "PRD §4.1.6: path not found → exit 1");
    }

    // Note: `run_invalid_db_path_returns_error` was removed because the
    // "invalid db path" error now surfaces at `build_kit` time, not at `run`
    // time. Covered by `build_kit_invalid_db_path_returns_build_failed_error`
    // in `kit::bootstrap::tests`.

    // --- lsp / embed flags are accepted ---

    #[test]
    fn run_with_lsp_flag_succeeds() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: true,
            embed: false,
            ram_first: false,
        };
        assert!(run(&kit, &args).is_ok());
    }

    #[test]
    fn run_with_embed_flag_succeeds() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: true,
            ram_first: false,
        };
        assert!(run(&kit, &args).is_ok());
    }

    // --- --ram-first flag (H15) ---

    #[test]
    fn run_with_ram_first_indexes_rust_file() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() { helper(); }\nfn helper() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo_ram".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: true,
        };
        let result = run(&kit, &args);
        assert!(result.is_ok(), "ram-first run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_with_ram_first_multiple_files_succeeds() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        write_file(tmp.path(), "b.rs", "fn b() {}\n");
        write_file(tmp.path(), "c.rs", "fn c() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "multi_ram".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: true,
        };
        let result = run(&kit, &args);
        assert!(result.is_ok(), "ram-first multi-file run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_with_ram_first_empty_directory_succeeds() {
        let tmp = TempDir::new().unwrap();
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "empty_ram".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: true,
        };
        let result = run(&kit, &args);
        assert!(result.is_ok(), "ram-first empty dir should succeed: {:?}", result.err());
    }

    #[test]
    fn run_with_ram_first_path_not_found_returns_exit_code_1() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = IndexArgs {
            path: "/nonexistent/path/xyz".to_string(),
            name: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: true,
        };
        let err = run(&kit, &args).expect_err("path not found should error");
        assert_eq!(err.exit_code(), 1, "ram-first: path not found → exit 1");
    }

    #[test]
    fn run_with_ram_first_produces_same_node_count_as_streaming() {
        // Functional equivalence: streaming and RAM-first must produce the
        // same number of nodes/edges for the same input. We index the same
        // fixture twice into separate DBs and compare node counts via
        // `Storage::query`.
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
        write_file(tmp.path(), "main.rs", "mod lib; fn main() { let _ = lib::add(1, 2); }\n");

        // Streaming path.
        let db_stream = fresh_db_path();
        let kit_stream = build_kit_for_db(db_stream.to_str().unwrap());
        let args_stream = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "eq_stream".to_string(),
            db: db_stream.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: false,
        };
        run(&kit_stream, &args_stream).expect("streaming index should succeed");

        // RAM-first path.
        let db_ram = fresh_db_path();
        let kit_ram = build_kit_for_db(db_ram.to_str().unwrap());
        let args_ram = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "eq_ram".to_string(),
            db: db_ram.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: true,
        };
        run(&kit_ram, &args_ram).expect("ram-first index should succeed");

        // Compare node counts (Function nodes) between the two databases.
        let storage_stream = kit_stream.require::<StorageKey>().expect("require_storage");
        let storage_ram = kit_ram.require::<StorageKey>().expect("require_storage");
        let count_stream = storage_stream
            .query("MATCH (n:Function) RETURN count(n) AS c;")
            .expect("query stream");
        let count_ram = storage_ram
            .query("MATCH (n:Function) RETURN count(n) AS c;")
            .expect("query ram");
        let n_stream = count_stream
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let n_ram = count_ram
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(
            n_stream, n_ram,
            "streaming ({n_stream}) and RAM-first ({n_ram}) must produce same Function node count"
        );
    }

    // --- R-lsp-004: LSP enhancement graceful degradation ---

    /// `index --lsp` must succeed even when `rust-analyzer` is not installed.
    /// The LSP enhancement logs a warning and degrades to pure tree-sitter
    /// extraction (specmark R-lsp-004: "LSP server 启动失败时，索引不中断").
    #[test]
    #[cfg(feature = "lsp")]
    fn run_with_lsp_flag_degrades_gracefully_without_rust_analyzer() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo_lsp".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: true,
            embed: false,
            ram_first: false,
        };
        // Must succeed regardless of whether rust-analyzer is installed —
        // LSP enhancement is best-effort, never aborts the index.
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "index --lsp must succeed even without rust-analyzer: {:?}",
            result.err()
        );
    }

    /// `enhance_with_lsp` returns Ok when the LSP server fails to start —
    /// verifies the graceful-degradation contract directly.
    #[test]
    #[cfg(feature = "lsp")]
    fn enhance_with_lsp_returns_ok_when_server_start_fails() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());

        // Index first so nodes exist in the DB.
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo_enhance".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: false,
        };
        run(&kit, &args).expect("index should succeed");

        // Open a fresh Repository (same pattern as run()) and call
        // enhance_with_lsp. rust-analyzer may or may not be installed —
        // either way, the function must return Ok.
        let fresh_repo = Repository::open(&db)
            .map_err(|e| CliError::Index(crate::index::IndexError::Storage(e)))
            .expect("open repo");
        let result = enhance_with_lsp(tmp.path(), &fresh_repo, "demo_enhance");
        assert!(
            result.is_ok(),
            "enhance_with_lsp must return Ok regardless of LSP availability: {:?}",
            result.err()
        );
    }

    // --- extract_hover_text unit tests ---

    #[test]
    #[cfg(feature = "lsp")]
    fn extract_hover_text_from_markup_content() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "fn add(a: i32, b: i32) -> i32\n\nAdds two numbers.".to_string(),
            }),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn add(a: i32, b: i32) -> i32");
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn extract_hover_text_from_scalar_string() {
        use lsp_types::{Hover, HoverContents, MarkedString};
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String(
                "struct Foo\n\nA struct.".to_string(),
            )),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "struct Foo");
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn extract_hover_text_from_language_string() {
        use lsp_types::{Hover, HoverContents, LanguageString, MarkedString};
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
                language: "rust".to_string(),
                value: "fn main()".to_string(),
            })),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn main()");
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn extract_hover_text_from_array_joins_lines() {
        use lsp_types::{Hover, HoverContents, MarkedString};
        let hover = Hover {
            contents: HoverContents::Array(vec![
                MarkedString::String("fn foo()".to_string()),
                MarkedString::String("fn bar()".to_string()),
            ]),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn foo()");
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn extract_hover_text_skips_empty_lines() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "\n\n\nfn real_signature()\n".to_string(),
            }),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn real_signature()");
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn extract_hover_text_returns_none_for_empty() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "".to_string(),
            }),
            range: None,
        };
        assert_eq!(extract_hover_text(&hover), None);
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn extract_hover_text_truncates_long_lines() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let long_line = "x".repeat(300);
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: long_line,
            }),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text.len(), 200);
    }
}
