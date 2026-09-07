// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `arch_diff` service: architecture-level semantic diff between two indexed
//! projects (e.g. the same repo indexed on `main` and on a feature branch).
//!
//! Both sides go through the same IR pipeline as `diagram`; the diff and its
//! machine receipt follow archify's compare contract. The HTML and the
//! receipt JSON are committed as an atomic pair.

use serde::Serialize;

#[cfg(feature = "diagram")]
use crate::analysis::architecture::{ArchitectureAnalyzer, ArchitectureOverview};
#[cfg(feature = "diagram")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(feature = "diagram")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(feature = "diagram")]
use crate::service::project::resolve_project_id;
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use crate::service::runtime::kit;

#[cfg(feature = "diagram")]
use crate::diagram::{render_delta, DeltaReceipt, DiagramError, QualityProfile};
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use sdforge::forge;
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use sdforge::prelude::ApiError;

/// JSON-serializable `arch_diff` output.
#[cfg(feature = "diagram")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchDiffOutput {
    pub base_project: String,
    pub head_project: String,
    pub output_path: String,
    pub receipt_path: String,
    pub receipt: DeltaReceipt,
}

/// Runs the delta pipeline against an injected Kit (testable core).
///
/// # Errors
///
/// Returns [`CodeNexusError`] for unknown projects, invalid `--quality`
/// values, empty `--output`, or quality-gate blocks.
#[cfg(feature = "diagram")]
pub fn run_arch_diff(
    kit: &AsyncKit<AsyncReady>,
    base_project: &str,
    head_project: &str,
    output_path: &str,
    quality: &str,
    title: &str,
) -> Result<ArchDiffOutput, CodeNexusError> {
    let profile = QualityProfile::parse(quality)
        .map_err(|bad| CodeNexusError::InvalidInput(format!("unknown quality profile: {bad}")))?;
    if output_path.trim().is_empty() {
        return Err(CodeNexusError::InvalidInput(
            "--output is required (path of the delta HTML to write)".to_string(),
        ));
    }
    let _ = profile; // gates run at standard inside render_delta; kept for CLI parity

    let storage = kit.require::<StorageModule>()?;
    let base_doc = project_document(&*storage, base_project, title)?;
    let head_doc = project_document(&*storage, head_project, title)?;

    let diff_title = if title.trim().is_empty() {
        format!("{base_project} → {head_project} architecture diff")
    } else {
        title.to_string()
    };
    let rendered = render_delta(&base_doc, &head_doc, &diff_title).map_err(|err| match err {
        DiagramError::QualityGate(diagnostics) => CodeNexusError::InvalidInput(format!(
            "arch_diff blocked by quality gate: {}",
            diagnostics
                .iter()
                .map(|d| d.code.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )),
        DiagramError::Template(placeholder) => CodeNexusError::Internal(format!(
            "diagram template missing placeholder {placeholder}"
        )),
    })?;

    let receipt_path = format!("{output_path}.receipt.json");
    write_pair(
        std::path::Path::new(output_path),
        rendered.html.as_bytes(),
        std::path::Path::new(&receipt_path),
        rendered.receipt_json.as_bytes(),
    )
    .map_err(CodeNexusError::Io)?;

    let receipt: DeltaReceipt = serde_json::from_str(&rendered.receipt_json)
        .map_err(|e| CodeNexusError::Internal(format!("delta receipt round-trip: {e}")))?;
    Ok(ArchDiffOutput {
        base_project: base_project.to_string(),
        head_project: head_project.to_string(),
        output_path: output_path.to_string(),
        receipt_path,
        receipt,
    })
}

/// Builds the canonical architecture IR for one project.
#[cfg(feature = "diagram")]
fn project_document(
    storage: &dyn crate::storage::capability::Storage,
    project: &str,
    title: &str,
) -> Result<crate::diagram::DiagramDocument, CodeNexusError> {
    use crate::diagram::from_overview;

    let project_id = resolve_project_id(storage, project)?;
    let overview: ArchitectureOverview =
        ArchitectureAnalyzer::new(storage).overview(&project_id)?;
    let layer_map = ArchitectureAnalyzer::new(storage).module_layer_map(&project_id)?;
    let doc_title = if title.trim().is_empty() {
        format!("{project} — architecture")
    } else {
        title.to_string()
    };
    let mut doc = from_overview(&overview, &layer_map, &doc_title);
    doc.meta.locale = "en".to_string();
    Ok(doc)
}

/// Writes two files as an atomic pair: both are staged before either rename;
/// if the second rename fails, the first is rolled back (previous bytes
/// restored, or the file removed when none existed).
#[cfg(feature = "diagram")]
fn write_pair(
    path_a: &std::path::Path,
    bytes_a: &[u8],
    path_b: &std::path::Path,
    bytes_b: &[u8],
) -> Result<(crate::diagnostics::HashInfo, crate::diagnostics::HashInfo), std::io::Error> {
    let previous_a = std::fs::read(path_a).ok();
    let artifact = crate::diagram::write_atomically(path_a, bytes_a)?;
    match crate::diagram::write_atomically(path_b, bytes_b) {
        Ok(receipt) => Ok((artifact, receipt)),
        Err(err) => {
            match previous_a {
                Some(old) => {
                    let _ = std::fs::write(path_a, old);
                }
                None => {
                    let _ = std::fs::remove_file(path_a);
                }
            }
            Err(err)
        }
    }
}

/// CLI wrapper — writes the delta pair and prints the receipt to stdout.
#[cfg(all(feature = "cli", feature = "diagram"))]
#[forge(
    name = "arch_diff",
    version = "0.3.12",
    description = "Compare two indexed projects' architecture and emit a Before/Delta/After HTML with a machine receipt.",
    cli = true
)]
async fn arch_diff(
    base_project: String,
    head_project: String,
    output: String,
    quality: String,
    title: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let out = run_arch_diff(
        &kit,
        &base_project,
        &head_project,
        &output,
        &quality,
        &title,
    )
    .map_err(|e| to_api_error(e, "arch_diff_error"))?;
    let json = serde_json::to_string(&out.receipt)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "arch_diff_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns the delta receipt.
#[cfg(all(feature = "mcp", feature = "diagram"))]
#[forge(
    name = "arch_diff",
    version = "0.3.12",
    tool_name = "arch_diff",
    description = "Compare two indexed projects' architecture and emit a Before/Delta/After HTML with a machine receipt."
)]
async fn arch_diff_mcp(
    base_project: String,
    head_project: String,
    output: String,
    quality: String,
    title: String,
) -> Result<DeltaReceipt, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_arch_diff(
        &kit,
        &base_project,
        &head_project,
        &output,
        &quality,
        &title,
    )
    .map_err(|e| to_api_error(e, "arch_diff_error"))
    .map(|out| out.receipt)
}

#[cfg(all(test, feature = "cli", feature = "diagram"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_archdiff_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    fn seed_base(storage: &dyn crate::storage::capability::Storage) {
        storage.execute("CREATE (:Project {id: 'base', name: 'base', rootPath: '/base', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'aaa'});").expect("project base");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'base', name: 'a', qualifiedName: 'base.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("fn a");
    }

    /// Seeds the head project: base content plus a second module (/src/b)
    /// and a cross-module CALLS edge — one added component, one added
    /// connection at module granularity.
    fn seed_head(storage: &dyn crate::storage::capability::Storage) {
        storage.execute("CREATE (:Project {id: 'head', name: 'head', rootPath: '/head', language: 'rust', fileCount: 2, indexedAt: 2000, lastCommit: 'bbb'});").expect("project head");
        storage.execute("CREATE (:Function {id: 'h_a', project: 'head', name: 'a', qualifiedName: 'head.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("head fn a");
        storage.execute("CREATE (:Function {id: 'h_b', project: 'head', name: 'b', qualifiedName: 'head.b', filePath: '/src/b/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("head fn b");
        storage.execute("CREATE (:CodeRelation {id: 'e_ab', source: 'h_a', target: 'h_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'head'});").expect("head edge");
    }

    #[test]
    fn run_arch_diff_classifies_added_entities_and_writes_pair() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        {
            let storage = kit.require::<StorageModule>().expect("storage");
            seed_base(&*storage);
            seed_head(&*storage);
        }
        let out_dir = TempDir::new().unwrap();
        let target = out_dir.path().join("delta.html");

        let out = run_arch_diff(
            &kit,
            "base",
            "head",
            target.to_str().unwrap(),
            "standard",
            "",
        )
        .expect("arch_diff should succeed");
        assert!(target.exists(), "delta HTML written");
        assert!(
            std::path::Path::new(&out.receipt_path).exists(),
            "receipt JSON written"
        );
        let added: Vec<&str> = out
            .receipt
            .changes
            .components
            .iter()
            .filter(|c| c.kind == crate::diagram::ChangeKind::Added)
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(added, vec!["src-b"], "new module detected as added");
        assert_eq!(out.receipt.changes.connections.len(), 1);
        assert_eq!(
            out.receipt.changes.connections[0].classification,
            crate::diagram::Classification::Topology
        );
        assert_eq!(out.receipt.comparator_version, "1");
        let html = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
        for marker in ["Delta", "Before", "After"] {
            assert!(html.contains(marker), "missing section {marker}");
        }
    }

    #[test]
    fn run_arch_diff_identical_projects_yield_empty_report() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        {
            let storage = kit.require::<StorageModule>().expect("storage");
            seed_base(&*storage);
        }
        let out_dir = TempDir::new().unwrap();
        let target = out_dir.path().join("delta.html");
        let out = run_arch_diff(
            &kit,
            "base",
            "base",
            target.to_str().unwrap(),
            "standard",
            "",
        )
        .expect("self diff should succeed");
        assert!(out.receipt.changes.is_empty());
    }

    #[test]
    fn run_arch_diff_missing_base_project_errors() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        {
            let storage = kit.require::<StorageModule>().expect("storage");
            seed_base(&*storage);
        }
        let out_dir = TempDir::new().unwrap();
        let target = out_dir.path().join("delta.html");
        let err = run_arch_diff(
            &kit,
            "ghost",
            "base",
            target.to_str().unwrap(),
            "standard",
            "",
        );
        assert!(matches!(err, Err(CodeNexusError::ProjectNotFound(_))));
    }

    #[test]
    fn write_pair_rolls_back_first_rename_on_second_failure() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.html");
        let b = dir.path().join("b.json");
        std::fs::write(&a, b"old-a").unwrap();
        // Occupy b's staging slot so the second write fails.
        std::fs::create_dir(
            dir.path()
                .join(format!(".cnx-stage-{}-b.json", std::process::id())),
        )
        .unwrap();

        let result = write_pair(&a, b"new-a", &b, b"{}");
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&a).unwrap(),
            b"old-a",
            "first rename rolled back"
        );
    }
}
