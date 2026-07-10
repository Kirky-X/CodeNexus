// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Index command: execute the full index pipeline.

use std::path::Path;

use serde::Serialize;

use crate::index::IndexResult;
use crate::kit::{IndexerKey, StorageConfigKey};
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::runtime::kit;
use crate::storage::{QualityChecker, Repository};

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

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
/// Duplicates `crate::cli::index_cmd::enhance_with_lsp` because that function
/// is private. Failures degrade gracefully to pure tree-sitter extraction.
#[cfg(feature = "lsp")]
fn enhance_with_lsp(workspace: &Path, repo: &Repository, project: &str) -> Result<(), ApiError> {
    use crate::lsp::{LspError, LspProvider, RustAnalyzerClient};
    use crate::storage::schema::escape_cypher_string;

    let client = RustAnalyzerClient::new();

    if let Err(LspError::ServerStart(msg)) = client.start(workspace) {
        eprintln!(
            "[warn] LSP server start failed, degrading to pure tree-sitter: {msg}"
        );
        return Ok(());
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
        let r = repo
            .connection()
            .query(q)
            .map_err(|e| wrap_error("LSP query failed", crate::storage::StorageError::Query(e.to_string())))?;
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

    eprintln!(
        "[info] LSP enhancement: {enhanced} symbol(s) enhanced, {skipped} skipped"
    );

    let _ = client.shutdown();
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
//
// The Kit is stored in a static `OnceLock` (see `runtime::kit`), so it will
// never be dropped during the process lifetime. This is functionally
// equivalent to the `std::mem::forget(kit)` call in `main.rs` — the stale
// Storage/Query/Trace connections opened at boot never get a chance to
// checkpoint over the indexer's writes.
#[cfg(feature = "cli")]
#[service_api(
    name = "codenexus",
    version = "0.3.2",
    tool_name = "index",
    description = "Index a codebase into the knowledge graph.",
    cli = true,
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
        .config::<StorageConfigKey>()
        .map_err(|e| wrap_error("Failed to resolve storage config", e))?;
    let storage_config = storage_config.load();
    let db_path = storage_config.db_path.clone();

    let path_ref = Path::new(&path);
    let indexer = kit
        .require::<IndexerKey>()
        .map_err(|e| wrap_error("Failed to resolve indexer capability", e))?;
    let result = if ram_first {
        indexer
            .index_ram_first(path_ref, &name, force)
            .map_err(|e| wrap_error("Index (RAM-first) failed", e))?
    } else {
        indexer
            .index(path_ref, &name, force)
            .map_err(|e| wrap_error("Index failed", e))?
    };

    // Open a FRESH Repository for DQ checks — the Kit's Storage connection
    // was opened at boot (before indexing) and holds a stale MVCC snapshot.
    let fresh_repo = Repository::open(&db_path)
        .map_err(|e| wrap_error("Failed to open fresh repository for DQ checks", e))?;
    let checker = QualityChecker::new(&fresh_repo);
    let dq_report = checker
        .run_all()
        .map_err(|e| wrap_error("Data quality check failed", e))?;
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
        if let Err(err) = enhance_with_lsp(path_ref, &fresh_repo, &name) {
            eprintln!("[warn] LSP enhancement aborted: {err}");
        }
    }

    let output = IndexOutput::from(result);
    let json = serde_json::to_string(&output)
        .map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}
