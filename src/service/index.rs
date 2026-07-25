// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Index command: execute the full index pipeline.

use std::path::Path;

use serde::Serialize;

use crate::index::IndexResult;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, IndexerModule};
#[cfg(any(feature = "cli", feature = "mcp", feature = "lsp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp", feature = "lsp", test))]
use crate::storage::{QualityChecker, Repository};

#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_kit_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::storage::StorageConfig;

#[cfg(feature = "lsp")]
use crate::lsp::LspProvider;
#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

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

#[cfg(feature = "lsp")]
fn build_lsp_providers() -> Vec<(&'static str, Box<dyn LspProvider>)> {
    use crate::lsp::{
        ClangdClient, FortlsClient, GoplsClient, JdtlsClient, PyrightClient, RustAnalyzerClient,
        TypeScriptLanguageClient,
    };

    vec![
        ("rs", Box::new(RustAnalyzerClient::new())),
        ("py", Box::new(PyrightClient::new())),
        ("c", Box::new(ClangdClient::new())),
        ("cpp", Box::new(ClangdClient::new())),
        ("go", Box::new(GoplsClient::new())),
        ("ts", Box::new(TypeScriptLanguageClient::new())),
        ("f90", Box::new(FortlsClient::new())),
        ("java", Box::new(JdtlsClient::new())),
    ]
}

#[cfg(feature = "lsp")]
fn build_symbol_queries(project: &str) -> [String; 2] {
    use crate::storage::schema::escape_cypher_string;

    let proj = escape_cypher_string(project);
    // P-09: pre-merge optimization kept as two separate queries because
    // LadybugDB's Cypher subset does NOT support `UNION` / `UNION ALL`
    // (see `src/analysis/architecture.rs` line ~38: "LadybugDB does not
    // support `UNION`, so we issue one query per table and merge in Rust").
    // A single `MATCH (n) WHERE n:Function OR n:Method ...` would also not
    // work — LadybugDB does not support multi-label `WHERE (n:A OR n:B)`
    // expressions either (same source). The two queries are issued
    // sequentially and their result rows are merged in Rust via
    // `rows.extend(r)` in `enhance_with_lsp`. The per-query overhead is one
    // round-trip to the embedded LadybugDB process — negligible compared to
    // the per-symbol LSP `hover` cost that dominates this function.
    [
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
    ]
}

#[cfg(feature = "lsp")]
fn select_provider_for_ext<'a>(
    ext_map: &std::collections::HashMap<&'static str, &'a dyn LspProvider>,
    ext: &str,
) -> Option<&'a dyn LspProvider> {
    // P-02: HashMap O(1) lookup replaces the pre-P-02 linear `.iter().find`
    // over 8 providers that ran once per symbol (100k+ symbols → 100k+ scans).
    // M2/P-07: returns `None` for unknown extensions instead of falling back
    // to `providers[0]` — callers (the hover loop) skip symbols whose file
    // extension is not wired to a real LSP server (e.g. .md, .toml). Pre-M2
    // started rust-analyzer for .md files, wasting ~300 MB-2 GB of RSS.
    ext_map.get(ext).copied()
}

#[cfg(feature = "lsp")]
fn build_semantic_type_update(id: &str, project: &str, text: &str) -> String {
    use crate::storage::schema::escape_cypher_string;

    format!(
        "MATCH (n {{id: '{id}', project: '{proj}'}}) \
         SET n.semantic_type = '{sem}';",
        id = escape_cypher_string(id),
        proj = escape_cypher_string(project),
        sem = escape_cypher_string(text),
    )
}

/// Extracts `(id, file_path_str, start_line)` from a Cypher query result row.
/// Returns `None` if any field is missing or has the wrong type.
#[cfg(feature = "lsp")]
fn extract_lsp_row_fields(row: &[serde_json::Value]) -> Option<(String, String, u64)> {
    let id = row.first().and_then(|v| v.as_str())?.to_string();
    let file_path_str = row.get(1).and_then(|v| v.as_str())?.to_string();
    let start_line = row.get(2).and_then(|v| v.as_u64())?;
    Some((id, file_path_str, start_line))
}

/// Resolves a file path to an absolute path, joining with `workspace` if the
/// path is relative.
#[cfg(feature = "lsp")]
fn resolve_abs_file_path(workspace: &Path, file_path_str: &str) -> std::path::PathBuf {
    let file_path = Path::new(file_path_str);
    if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        workspace.join(file_path)
    }
}

/// LSP-driven `semantic_type` enhancement.
///
/// Spawns language servers (rust-analyzer, pyright-langserver, …) based on
/// file extensions found in the project, queries each symbol's hover info,
/// and writes the extracted type signature back to `semantic_type`.
/// Failures degrade gracefully to pure tree-sitter extraction.
///
/// # L7-6 memory-overflow fix — on-demand LSP startup
///
/// Pre-L7-6 this function unconditionally called `provider.start(workspace)`
/// for ALL 8 wired LSP clients (rs, py, c, cpp, go, ts, f90, java) before
/// querying any symbol. Each LSP server (rust-analyzer, pyright, clangd,
/// gopls, tsserver, fortls, jdtls) consumes 300 MB–2 GB of RSS, so a pure
/// Rust project needlessly spawned 7 unused servers — ~3–5 GB of wasted RSS
/// that compounded with the L7-5 buffer pool fix to cause OOM on 70 GB hosts.
///
/// L7-6 reorders the flow: query Function/Method rows FIRST, collect the
/// set of file extensions actually present, and only `start()` the LSP
/// providers whose extension appears in the result set. A pure Rust repo
/// now starts only `rust-analyzer`; a Python repo starts only `pyright`;
/// mixed repos start only the union. `shutdown()` is still called on all
/// 8 providers (the unused ones short-circuit to `Ok(())` per the
/// `LspProvider::shutdown` contract — "safe to call on a client that was
/// never successfully started").
///
/// Memory savings: ~3–5 GB on single-language repos (the common case).
#[cfg(feature = "lsp")]
#[allow(clippy::result_large_err)]
fn enhance_with_lsp(
    workspace: &Path,
    repo: &Repository,
    project: &str,
) -> Result<(), CodeNexusError> {
    use crate::lsp::LspError;

    let providers = build_lsp_providers();

    // L7-6: query Function/Method rows FIRST, then start only the LSP
    // providers whose extension appears in the result set. Pre-L7-6 started
    // all 8 providers up front, wasting ~3–5 GB on single-language repos.
    let queries = build_symbol_queries(project);
    let mut rows = Vec::new();
    for q in &queries {
        let r = repo.connection().query(q).map_err(|e| {
            CodeNexusError::Storage(crate::storage::StorageError::Query(e.to_string()))
        })?;
        rows.extend(r);
    }

    // Pre-extract (id, abs_file, line) tuples and collect the set of
    // extensions that actually need an LSP server. Rows that fail
    // `extract_lsp_row_fields` are counted as skipped (Rule 12: explicit
    // skip, not silent success — same behaviour as pre-L7-6).
    //
    // P-02: build `ext_map` once (HashMap<&str, &dyn LspProvider>) so the
    // hover loop's per-symbol `select_provider_for_ext` is O(1) instead of
    // the pre-P-02 linear scan over 8 providers.
    // P-10: build `provider_exts_set` once (HashSet<&str>) so `collect_active_extensions`
    // is also O(1) per entry instead of O(8).
    let provider_exts_set: std::collections::HashSet<&'static str> =
        providers.iter().map(|(e, _)| *e).collect();
    let ext_map: std::collections::HashMap<&'static str, &dyn LspProvider> = providers
        .iter()
        .map(|(ext, p)| (*ext, p.as_ref()))
        .collect();
    let mut entries: Vec<(String, std::path::PathBuf, u32)> = Vec::with_capacity(rows.len());
    let mut skipped: u32 = 0;
    for row in &rows {
        let Some((id, file_path_str, start_line)) = extract_lsp_row_fields(row) else {
            skipped += 1;
            continue;
        };
        let abs_file = resolve_abs_file_path(workspace, &file_path_str);
        let line = u32::try_from(start_line).unwrap_or(0);
        entries.push((id, abs_file, line));
    }
    let active_exts = collect_active_extensions(&entries, &provider_exts_set);

    // L7-6: only start providers whose extension appears in `active_exts`.
    // Providers for absent extensions short-circuit `shutdown()` to `Ok(())`
    // (LspProvider::shutdown contract: "safe to call on a client that was
    // never successfully started").
    let mut started_count: u32 = 0;
    for (ext, provider) in &providers {
        if !active_exts.contains(ext) {
            continue;
        }
        if let Err(LspError::ServerStart(msg)) = provider.start(workspace) {
            eprintln!("[warn] LSP server start failed (degrading): {msg}");
        } else {
            started_count += 1;
        }
    }
    eprintln!(
        "[info] LSP enhancement: started {started_count} of {} configured servers for {} symbols",
        providers.len(),
        entries.len()
    );

    // If no symbols need enhancement (empty repo / all rows failed to
    // extract), short-circuit — there is nothing to hover. Still call
    // shutdown() on all providers for symmetry.
    let mut enhanced: u32 = 0;
    if !entries.is_empty() {
        for (id, abs_file, line) in &entries {
            let ext = abs_file.extension().and_then(|e| e.to_str()).unwrap_or("");
            // M2/P-07: skip symbols whose extension has no wired LSP provider
            // (.md, .toml, .json). Pre-M2 fell back to providers[0]
            // (rust-analyzer) and wasted 300 MB-2 GB of RSS hovering files
            // LSP can't help with.
            let Some(client) = select_provider_for_ext(&ext_map, ext) else {
                skipped += 1;
                continue;
            };

            match client.hover(abs_file, *line, 0) {
                Ok(Some(hover)) => {
                    if let Some(text) = crate::lsp::extract_hover_text(&hover) {
                        let update = build_semantic_type_update(id, project, &text);
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
                Err(LspError::NotImplemented(_)) => {
                    // `hover` is implemented on all current clients; defensive
                    // skip for future opt-outs (C9 R-lsp-002 pattern).
                    skipped += 1;
                }
            }
        }
    }

    eprintln!("[info] LSP enhancement: {enhanced} symbol(s) enhanced, {skipped} skipped");

    for (_ext, provider) in &providers {
        let _ = provider.shutdown();
    }
    Ok(())
}

/// Collects the set of provider extensions that appear in `entries` (L7-6).
///
/// Given the pre-extracted `(id, abs_file, line)` tuples from the
/// Function/Method query, returns the set of provider extensions whose LSP
/// server must be started.
///
/// # M2/P-07 — skip unknown extensions instead of falling back
///
/// Files whose extension does not match any wired provider (e.g. `.md`,
/// `.toml`, `.json`) are **skipped** — they do not activate any LSP server
/// and the hover loop later skips them via [`select_provider_for_ext`]
/// returning `None`. Pre-M2 fell back to `default_ext` (`"rs"`), which
/// needlessly started `rust-analyzer` for markdown/toml symbols, wasting
/// 300 MB-2 GB of RSS. Skipping is correct: LSP servers cannot hover
/// non-source files anyway, so the fallback hover result was always
/// `Ok(None)` (no semantic_type extracted) — the RSS cost was pure waste.
///
/// # P-10 — HashSet lookup replaces linear scan
///
/// `provider_exts` is now a `HashSet<&str>` instead of a `&[&str]`. The
/// per-entry `contains` is O(1) instead of O(8). For polyglot repos with
/// 100k+ symbols this avoids ~800k string comparisons during routing.
///
/// This is a pure function (no I/O, no LSP, no DB) so the L7-6 routing
/// decision can be unit-tested deterministically without spawning language
/// servers or opening a database.
///
/// # Arguments
///
/// * `entries` - Pre-extracted `(id, abs_file, line)` tuples from
///   `extract_lsp_row_fields`. Only the `abs_file` extension is read.
/// * `provider_exts` - HashSet of wired provider extensions (e.g.
///   `{"rs", "py", "c", "cpp", "go", "ts", "f90", "java"}`).
#[cfg(feature = "lsp")]
fn collect_active_extensions<'a>(
    entries: &[(String, std::path::PathBuf, u32)],
    provider_exts: &std::collections::HashSet<&'a str>,
) -> std::collections::HashSet<&'a str> {
    let mut active: std::collections::HashSet<&'a str> = std::collections::HashSet::new();
    for (_, abs_file, _) in entries {
        let ext = abs_file.extension().and_then(|e| e.to_str()).unwrap_or("");
        // M2/P-07: skip unknown extensions — do NOT fall back to default.
        if let Some(&provider_ext) = provider_exts.get(ext) {
            active.insert(provider_ext);
        }
    }
    active
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
    version = "0.3.5",
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

    // --- Pure function tests (extracted from enhance_with_lsp) ---

    #[cfg(feature = "lsp")]
    #[test]
    fn build_lsp_providers_returns_8_providers() {
        let providers = build_lsp_providers();
        assert_eq!(providers.len(), 8);
        let exts: Vec<&str> = providers.iter().map(|(e, _)| *e).collect();
        assert!(exts.contains(&"rs"));
        assert!(exts.contains(&"py"));
        assert!(exts.contains(&"go"));
        assert!(exts.contains(&"java"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn build_symbol_queries_contains_project_name() {
        let queries = build_symbol_queries("demo");
        assert!(queries[0].contains("demo"));
        assert!(queries[1].contains("demo"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn build_symbol_queries_escapes_special_chars() {
        let queries = build_symbol_queries("demo' OR '1'='1");
        assert!(!queries[0].contains("' OR '1'='1"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn select_provider_for_ext_returns_some_for_rs() {
        // P-02: select_provider_for_ext now does a HashMap lookup and
        // returns Option<&dyn LspProvider>. .rs must map to a provider.
        let providers = build_lsp_providers();
        let ext_map: std::collections::HashMap<&'static str, &dyn LspProvider> = providers
            .iter()
            .map(|(ext, p)| (*ext, p.as_ref()))
            .collect();
        let provider = select_provider_for_ext(&ext_map, "rs").expect("rs must map to a provider");
        let _ = provider.shutdown();
        // Drop providers after shutdown to release the Box<dyn LspProvider>.
        drop(providers);
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn select_provider_for_ext_returns_none_for_unknown() {
        // M2/P-07: unknown extensions (.md, .toml, .json) no longer fall
        // back to providers[0]. select_provider_for_ext returns None so the
        // hover loop skips the symbol — pre-M2 wasted 300 MB-2 GB of RSS
        // starting rust-analyzer for non-source files.
        let providers = build_lsp_providers();
        let ext_map: std::collections::HashMap<&'static str, &dyn LspProvider> = providers
            .iter()
            .map(|(ext, p)| (*ext, p.as_ref()))
            .collect();
        assert!(
            select_provider_for_ext(&ext_map, "md").is_none(),
            "md must NOT map to any provider"
        );
        assert!(
            select_provider_for_ext(&ext_map, "toml").is_none(),
            "toml must NOT map to any provider"
        );
        assert!(
            select_provider_for_ext(&ext_map, "unknown").is_none(),
            "unknown ext must NOT map to any provider"
        );
        // Shutdown all providers (no-op for never-started clients).
        for (_, p) in &providers {
            let _ = p.shutdown();
        }
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn build_semantic_type_update_escapes_id() {
        let update = build_semantic_type_update("id'with'quotes", "demo", "type_text");
        assert!(update.contains(r"id\'with\'quotes"));
    }

    // --- extract_lsp_row_fields tests ---

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_all_fields_when_valid() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Number(serde_json::Number::from(42u64)),
        ];
        let (id, file_path, start_line) =
            extract_lsp_row_fields(&row).expect("valid row should extract");
        assert_eq!(id, "sym_id");
        assert_eq!(file_path, "/src/main.rs");
        assert_eq!(start_line, 42);
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_id_missing() {
        let row = vec![
            serde_json::Value::Null,
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Number(serde_json::Number::from(1u64)),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_file_path_missing() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::Null,
            serde_json::Value::Number(serde_json::Number::from(1u64)),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_start_line_missing() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Null,
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_for_empty_row() {
        let row: Vec<serde_json::Value> = vec![];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_id_not_string() {
        let row = vec![
            serde_json::Value::Number(serde_json::Number::from(123u64)),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Number(serde_json::Number::from(1u64)),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_start_line_not_number() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::String("not_a_number".into()),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    // --- resolve_abs_file_path tests ---

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_returns_absolute_unchanged() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "/src/main.rs");
        assert_eq!(abs, std::path::PathBuf::from("/src/main.rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_joins_relative_with_workspace() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "src/main.rs");
        assert_eq!(abs, std::path::PathBuf::from("/workspace/src/main.rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_handles_empty_string() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "");
        assert_eq!(abs, std::path::PathBuf::from("/workspace"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_handles_relative_with_dots() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "./src/main.rs");
        assert_eq!(abs, std::path::PathBuf::from("/workspace/./src/main.rs"));
    }

    // --- collect_active_extensions (L7-6) tests ---
    //
    // L7-6 memory-overflow fix: only start LSP servers for extensions that
    // actually appear in the indexed symbols. These tests verify the routing
    // decision without spawning any LSP server or opening a database.

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_empty_entries_returns_empty_set() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let active = collect_active_extensions(&[], &provider_exts);
        assert!(
            active.is_empty(),
            "empty entries must produce empty active set"
        );
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_pure_rust_returns_only_rs() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::path::PathBuf::from("/src/main.rs"),
                1,
            ),
            (
                "id2".to_string(),
                std::path::PathBuf::from("/src/lib.rs"),
                10,
            ),
            (
                "id3".to_string(),
                std::path::PathBuf::from("/src/mod.rs"),
                20,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            1,
            "pure Rust repo must activate only rust-analyzer"
        );
        assert!(active.contains("rs"));
        // Critical L7-6 invariant: must NOT start unused LSP servers.
        assert!(!active.contains("py"));
        assert!(!active.contains("c"));
        assert!(!active.contains("cpp"));
        assert!(!active.contains("go"));
        assert!(!active.contains("ts"));
        assert!(!active.contains("f90"));
        assert!(!active.contains("java"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_pure_python_returns_only_py() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::path::PathBuf::from("/src/main.py"),
                1,
            ),
            (
                "id2".to_string(),
                std::path::PathBuf::from("/src/utils.py"),
                5,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            1,
            "pure Python repo must activate only pyright"
        );
        assert!(active.contains("py"));
        assert!(!active.contains("rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_mixed_rs_py_returns_both() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::path::PathBuf::from("/src/main.rs"),
                1,
            ),
            (
                "id2".to_string(),
                std::path::PathBuf::from("/src/script.py"),
                5,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            2,
            "mixed repo must activate exactly the union"
        );
        assert!(active.contains("rs"));
        assert!(active.contains("py"));
        assert!(!active.contains("java"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_unknown_extension_skipped() {
        // M2/P-07: unknown extensions (.md, .toml, .json) are SKIPPED — they
        // do NOT activate any LSP server. Pre-M2 fell back to default ("rs"),
        // needlessly starting rust-analyzer for markdown/toml files (waste of
        // 300 MB-2 GB RSS). Skipping is correct because LSP servers cannot
        // hover non-source files anyway.
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            ("id1".to_string(), std::path::PathBuf::from("/README.md"), 1),
            (
                "id2".to_string(),
                std::path::PathBuf::from("/Cargo.toml"),
                1,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert!(
            active.is_empty(),
            "unknown extensions (.md, .toml) must NOT activate any LSP server; got: {active:?}"
        );
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_cpp_and_c_both_active() {
        // ClangdClient is wired for both "c" and "cpp"; a project with both
        // .c and .cpp files activates both extension keys (which both map
        // to the same ClangdClient, but the routing decision is per-ext).
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::path::PathBuf::from("/src/main.c"),
                1,
            ),
            (
                "id2".to_string(),
                std::path::PathBuf::from("/src/util.cpp"),
                5,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(active.len(), 2);
        assert!(active.contains("c"));
        assert!(active.contains("cpp"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_dedupes_repeated_extensions() {
        // 1000 .rs files should activate exactly one extension ("rs"), not
        // 1000 entries — the HashSet deduplicates.
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries: Vec<(String, std::path::PathBuf, u32)> = (0..1000)
            .map(|i| (format!("id{i}"), std::path::PathBuf::from("/src/mod.rs"), i))
            .collect();
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(active.len(), 1);
        assert!(active.contains("rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_all_eight_exts_active_for_polyglot_repo() {
        // A polyglot repo with all 8 wired extensions activates all 8
        // providers (worst case — no savings vs pre-L7-6, but correctness
        // preserved).
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            ("id1".to_string(), std::path::PathBuf::from("/a.rs"), 1),
            ("id2".to_string(), std::path::PathBuf::from("/b.py"), 1),
            ("id3".to_string(), std::path::PathBuf::from("/c.c"), 1),
            ("id4".to_string(), std::path::PathBuf::from("/d.cpp"), 1),
            ("id5".to_string(), std::path::PathBuf::from("/e.go"), 1),
            ("id6".to_string(), std::path::PathBuf::from("/f.ts"), 1),
            ("id7".to_string(), std::path::PathBuf::from("/g.f90"), 1),
            ("id8".to_string(), std::path::PathBuf::from("/h.java"), 1),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            8,
            "polyglot repo must activate all 8 providers"
        );
    }

    // --- #[forge] index wrapper tests ---

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn index_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            "/nonexistent/path".to_string(),
            "test_project".to_string(),
            false,
            false,
            false,
            false,
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(all(feature = "cli", feature = "lang-rust"))]
    #[test]
    fn index_wrapper_succeeds_via_init_kit() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use crate::service::runtime::{init_kit, reset_kit_for_testing};
        use std::fs;
        use tempfile::TempDir;

        reset_kit_for_testing();

        let src_dir = TempDir::new().unwrap();
        fs::write(src_dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("wrapper_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            src_dir.path().to_str().unwrap().to_string(),
            "wrapper_test".to_string(),
            true,
            false,
            false,
            false,
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    // Covers the `embed=true` deprecation warning branch (lines 296-301).
    #[serial_test::serial(kit_init)]
    #[cfg(all(feature = "cli", feature = "lang-rust"))]
    #[test]
    fn index_wrapper_with_embed_true_emits_deprecation_warning() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use crate::service::runtime::{init_kit, reset_kit_for_testing};
        use std::fs;
        use tempfile::TempDir;

        reset_kit_for_testing();

        let src_dir = TempDir::new().unwrap();
        fs::write(src_dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("embed_warn_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            src_dir.path().to_str().unwrap().to_string(),
            "embed_warn_test".to_string(),
            true,
            false,
            true,
            false,
        ));
        assert!(
            result.is_ok(),
            "wrapper should succeed even with embed=true: {:?}",
            result.err()
        );

        reset_kit_for_testing();
    }

    // Covers the DQ violation reporting branch in index_core (lines 248-258).
    // Pre-populates the DB with a File node that has an empty hash (DQ-006
    // violation), then calls index_core. The DQ check should detect the
    // violation and trigger the `if !dq_report.is_clean()` branch.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_reports_dq_violations_when_present() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use crate::storage::Repository;
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(src_path.join("main.rs"), "fn main() {}\n").unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("dq_violation_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        // Pre-populate the DB with a DQ-006 violation: a File node with empty
        // hash. Use a separate project name to avoid conflicts with the
        // indexer. The data is committed via a fresh Repository connection
        // (LadybugDB auto-commits each execute), so the fresh_repo opened
        // inside index_core will see it.
        let repo = Repository::open(&db_path).expect("open repo for injection");
        repo.connection()
            .execute(
                "CREATE (:File {id: 'bad_file', project: 'ghost_proj', name: 'bad.rs', \
                 filePath: '/bad.rs', language: 'rust', hash: '', lineCount: 0});",
            )
            .expect("insert bad file");

        // Call index_core — the DQ check should detect the empty hash and
        // trigger the `if !dq_report.is_clean()` branch (lines 248-258).
        // DQ violations are reported via eprintln! but don't fail indexing.
        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "test_project",
            false,
            false,
            false,
        )
        .expect("index should succeed even with DQ violations");

        assert!(output.files_indexed >= 1);
    }

    // Covers the `kit.config::<StorageConfig>().map_err(...)` error path
    // (lines 304-306) in the index wrapper. Creates a kit WITHOUT registering
    // StorageModule or setting StorageConfig, then calls the index wrapper.
    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn index_wrapper_fails_when_storage_config_not_registered() {
        use crate::kit::AsyncKit;
        use crate::service::runtime::{init_kit, reset_kit_for_testing};
        use sdforge::prelude::ApiError;

        reset_kit_for_testing();

        // Build an empty kit (no modules registered, no configs set).
        // build() succeeds on an empty dependency graph.
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(AsyncKit::new().build())
            .expect("build empty kit");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            "/nonexistent/path".to_string(),
            "test_project".to_string(),
            false,
            false,
            false,
            false,
        ));
        assert!(result.is_err(), "wrapper should fail without StorageConfig");
        // The error should be an Internal ApiError with a message mentioning
        // storage config (from wrap_kit_error).
        match result.unwrap_err() {
            ApiError::Internal { message, .. } => {
                assert!(
                    message.contains("storage config"),
                    "error should mention storage config: {message}"
                );
            }
            other => panic!("expected Internal error, got {other:?}"),
        }

        reset_kit_for_testing();
    }
}
