// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Index command: execute the full index pipeline.

use std::path::Path;

use serde::Serialize;

use crate::index::IndexResult;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, IndexerModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::storage::{QualityChecker, Repository};

#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_kit_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::storage::StorageConfig;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::forge;

/// JSON-serializable view of [`IndexResult`].
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IndexOutput {
    pub project_id: String,
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub nodes_created: usize,
    pub edges_created: usize,
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

/// LSP-driven `semantic_type` enhancement.
///
/// Spawns language servers (rust-analyzer, pyright-langserver, …) based on
/// file extensions found in the project, queries each symbol's hover info,
/// and writes the extracted type signature back to `semantic_type`.
/// Failures degrade gracefully to pure tree-sitter extraction.
#[cfg(feature = "lsp")]
#[allow(clippy::result_large_err)]
fn enhance_with_lsp(workspace: &Path, repo: &Repository, project: &str) -> Result<(), CodeNexusError> {
    use crate::lsp::{
        ClangdClient, FortlsClient, GoplsClient, JdtlsClient, LspError, LspProvider,
        PyrightClient, RustAnalyzerClient, TypeScriptLanguageClient,
    };
    use crate::storage::schema::escape_cypher_string;

    // Build a map: file_ext → (dyn LspProvider, &str)
    let rust_client = RustAnalyzerClient::new();
    let py_client = PyrightClient::new();
    let clangd_client = ClangdClient::new();
    let gopls_client = GoplsClient::new();
    let ts_client = TypeScriptLanguageClient::new();
    let fortls_client = FortlsClient::new();
    let jdtls_client = JdtlsClient::new();
    let providers: [(&str, &dyn LspProvider); 8] = [
        ("rs", &rust_client),
        ("py", &py_client),
        ("c", &clangd_client),
        ("cpp", &clangd_client),
        ("go", &gopls_client),
        ("ts", &ts_client),
        ("f90", &fortls_client),
        ("java", &jdtls_client),
    ];

    // Start each provider (best-effort; failures degrade to partial enhancement)
    for (_ext, provider) in &providers {
        if let Err(LspError::ServerStart(msg)) = provider.start(workspace) {
            eprintln!("[warn] LSP server start failed (degrading): {msg}");
        }
    }

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
            CodeNexusError::Storage(crate::storage::StorageError::Query(e.to_string()))
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

        let file_path = Path::new(file_path_str);
        let abs_file = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            workspace.join(file_path)
        };

        // Select provider by file extension
        let ext = abs_file.extension().and_then(|e| e.to_str()).unwrap_or("");
        let client: &dyn LspProvider = providers.iter().find(|(e, _)| *e == ext).map(|(_, p)| *p).unwrap_or_else(|| &rust_client);

        let line = u32::try_from(start_line).unwrap_or(0);
        match client.hover(&abs_file, line, 0) {
            Ok(Some(hover)) => {
                if let Some(text) = crate::lsp::extract_hover_text(&hover) {
                    let update = format!(
                        "MATCH (n {{id: '{id}', project: '{proj}'}}) \
                         SET n.semantic_type = '{sem}';",
                        id = escape_cypher_string(id),
                        proj = escape_cypher_string(project),
                        sem = escape_cypher_string(&text),
                    );
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
                skipped += 1;
            }
            Err(LspError::ServerStart(_)) => {
                skipped += 1;
            }
        }
    }

    eprintln!("[info] LSP enhancement: {enhanced} symbol(s) enhanced, {skipped} skipped");

    for (_ext, provider) in &providers {
        let _ = provider.shutdown();
    }
    Ok(())
}

/// Core index pipeline: indexer + DQ checks + CHECKPOINT + optional LSP.
///
/// Separated from the `#[forge]` function for reuse by `import.rs`
/// reindex logic.
#[cfg(any(feature = "cli", feature = "mcp", test))]
#[allow(clippy::result_large_err)]
// `lsp` parameter is only consumed under `feature = "lsp"`; suppress the
// unused-variable warning for builds without the lsp feature.
#[cfg_attr(not(feature = "lsp"), allow(unused_variables))]
pub(crate) fn index_core(
    kit: &AsyncKit<AsyncReady>,
    db_path: &Path,
    path: &str,
    name: &str,
    force: bool,
    lsp: bool,
    ram_first: bool,
) -> Result<IndexOutput, CodeNexusError> {
    let path_ref = Path::new(path);
    let indexer = kit.require::<IndexerModule>()?;
    let result = if ram_first {
        indexer.index_ram_first(path_ref, name, force)?
    } else {
        indexer.index(path_ref, name, force)?
    };

    // Open a FRESH Repository for DQ checks — the Kit's Storage connection
    // was opened at boot (before indexing) and holds a stale MVCC snapshot.
    let fresh_repo = Repository::open(db_path)?;
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

    // Flush the WAL after DQ checks.
    if let Err(err) = fresh_repo.connection().execute("CHECKPOINT;") {
        eprintln!("[warn] post-quality-check checkpoint failed: {err}");
    }

    // R-lsp-004: LSP-enhanced semantic_type extraction.
    #[cfg(feature = "lsp")]
    if lsp {
        if let Err(err) = enhance_with_lsp(path_ref, &fresh_repo, name) {
            eprintln!("[warn] LSP enhancement aborted: {err}");
        }
    }

    Ok(IndexOutput::from(result))
}

/// CLI wrapper — prints result to stdout as JSON.
//
// The Kit is stored in a static `OnceLock` (see `runtime::kit`), so it is
// never dropped — the stale boot-time connections never checkpoint over the
// indexer's writes (same effect as `std::mem::forget(kit)` in `main.rs`).
#[cfg(feature = "cli")]
#[forge(
    name = "index",
    version = "0.3.2",
    description = "Index a codebase into the knowledge graph.",
    cli = true
)]
async fn index(
    path: String,
    name: String,
    force: bool,
    lsp: bool,
    embed: bool,
    ram_first: bool,
) -> Result<(), ApiError> {
    if embed {
        eprintln!(
            "[warn] --embed flag is deprecated; embedding is controlled by \
             the `embed` cargo feature (rebuild with --features embed to enable)"
        );
    }

    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage_config = kit
        .config::<StorageConfig>()
        .map_err(|e| wrap_kit_error("Failed to resolve storage config", e))?;
    let db_path = storage_config.db_path.clone();

    let output = index_core(&kit, &db_path, &path, &name, force, lsp, ram_first)
        .map_err(|e| to_api_error(e, "index_error"))?;
    let json = serde_json::to_string(&output)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "index_error"))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_output_from_index_result_maps_all_fields() {
        let result = IndexResult {
            project_id: "p1".into(),
            files_indexed: 10,
            files_skipped: 2,
            nodes_created: 100,
            edges_created: 50,
            duration_ms: 5000,
        };
        let output = IndexOutput::from(result);
        assert_eq!(output.project_id, "p1");
        assert_eq!(output.files_indexed, 10);
        assert_eq!(output.files_skipped, 2);
        assert_eq!(output.nodes_created, 100);
        assert_eq!(output.edges_created, 50);
        assert_eq!(output.duration_ms, 5000);
    }

    #[test]
    fn index_output_from_handles_zero_values() {
        let result = IndexResult {
            project_id: "".into(),
            files_indexed: 0,
            files_skipped: 0,
            nodes_created: 0,
            edges_created: 0,
            duration_ms: 0,
        };
        let output = IndexOutput::from(result);
        assert_eq!(output.project_id, "");
        assert_eq!(output.files_indexed, 0);
        assert_eq!(output.nodes_created, 0);
        assert_eq!(output.edges_created, 0);
        assert_eq!(output.duration_ms, 0);
    }

    #[test]
    fn index_output_serializes_to_json() {
        let output = IndexOutput {
            project_id: "p1".into(),
            files_indexed: 10,
            files_skipped: 2,
            nodes_created: 100,
            edges_created: 50,
            duration_ms: 5000,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"project_id\":\"p1\""));
        assert!(json.contains("\"files_indexed\":10"));
        assert!(json.contains("\"files_skipped\":2"));
        assert!(json.contains("\"nodes_created\":100"));
        assert!(json.contains("\"edges_created\":50"));
        assert!(json.contains("\"duration_ms\":5000"));
    }

    #[test]
    fn index_output_from_preserves_large_values() {
        let result = IndexResult {
            project_id: "uuid-v7-12345".into(),
            files_indexed: usize::MAX,
            files_skipped: usize::MAX,
            nodes_created: usize::MAX,
            edges_created: usize::MAX,
            duration_ms: u64::MAX,
        };
        let output = IndexOutput::from(result);
        assert_eq!(output.files_indexed, usize::MAX);
        assert_eq!(output.duration_ms, u64::MAX);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_indexes_rust_project() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "test_project",
            false,
            false,
            false,
        )
        .expect("index should succeed");

        assert!(!output.project_id.is_empty());
        assert!(output.files_indexed >= 1);
        assert!(output.nodes_created > 0);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_force_reindexes_unchanged_files() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_force_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        // First index
        let output1 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "force_project",
            false,
            false,
            false,
        )
        .expect("first index should succeed");
        assert!(output1.files_indexed >= 1);

        // Second index with force=true should reindex even though files are unchanged
        let output2 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "force_project",
            true,
            false,
            false,
        )
        .expect("forced reindex should succeed");
        assert!(output2.files_indexed >= 1);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_handles_empty_directory() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_empty_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "empty_project",
            false,
            false,
            false,
        )
        .expect("index should succeed on empty dir");

        assert_eq!(output.files_indexed, 0);
        assert_eq!(output.nodes_created, 0);
    }

    // Covers line 188: ram_first=true branch in index_core.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_with_ram_first_indexes_rust_project() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_ram_first_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "ram_first_project",
            false,
            false,
            true,
        )
        .expect("ram_first index should succeed");

        assert!(!output.project_id.is_empty());
        assert!(output.files_indexed >= 1);
        assert!(output.nodes_created > 0);
    }

    // Covers line 188: ram_first=true with force=true reindex path.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_with_ram_first_and_force_reindexes() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_ram_force_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        // First index with ram_first=true.
        let output1 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "ram_force_project",
            false,
            false,
            true,
        )
        .expect("first ram_first index should succeed");
        assert!(output1.files_indexed >= 1);

        // Second index with ram_first=true and force=true.
        let output2 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "ram_force_project",
            true,
            false,
            true,
        )
        .expect("forced ram_first reindex should succeed");
        assert!(output2.files_indexed >= 1);
    }
}
