// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `diagram` service: render a self-contained interactive architecture HTML
//! from the indexed graph.
//!
//! Pipeline (absorbed from archify): `ArchitectureOverview` → typed IR →
//! deterministic layout/routing/SVG → geometric quality gates → optional
//! git evidence verification → template fill → atomic delivery with a
//! repair receipt on stdout.

use serde::Serialize;

#[cfg(feature = "diagram")]
use crate::analysis::architecture::{ArchitectureAnalyzer, ArchitectureOverview};
#[cfg(feature = "diagram")]
use crate::diagnostics::{Diagnostic, EvidenceSummary, Receipt, Severity};
#[cfg(feature = "diagram")]
use crate::diagram::{from_overview, render, verify, DiagramError, QualityProfile};
#[cfg(feature = "diagram")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(feature = "diagram")]
use crate::service::project::resolve_project_id;
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use crate::service::runtime::kit;
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use sdforge::forge;
#[cfg(all(feature = "diagram", any(feature = "cli", feature = "mcp")))]
use sdforge::prelude::ApiError;

/// JSON-serializable `diagram` command output (the delivery receipt).
#[cfg(feature = "diagram")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiagramOutput {
    pub project: String,
    pub output_path: String,
    pub receipt: Receipt,
}

/// Runs the diagram pipeline against an injected Kit (testable core).
///
/// # Errors
///
/// Returns [`CodeNexusError`] for unknown projects, invalid `--quality`
/// values, empty `--output`, or quality-gate blocks (showcase).
#[cfg(feature = "diagram")]
#[allow(clippy::too_many_arguments)]
pub fn run_diagram(
    kit: &crate::kit::AsyncKit<crate::kit::AsyncReady>,
    project: &str,
    output_path: &str,
    quality: &str,
    repo_root: &str,
    repo_url: &str,
    title: &str,
    locale: &str,
) -> Result<DiagramOutput, CodeNexusError> {
    let profile = QualityProfile::parse(quality)
        .map_err(|bad| CodeNexusError::InvalidInput(format!("unknown quality profile: {bad}")))?;
    if output_path.trim().is_empty() {
        return Err(CodeNexusError::InvalidInput(
            "--output is required (path of the HTML artifact to write)".to_string(),
        ));
    }

    let storage = kit.require::<crate::kit::StorageModule>()?;
    let project_id = resolve_project_id(&*storage, project)?;
    let overview: ArchitectureOverview =
        ArchitectureAnalyzer::new(&*storage).overview(&project_id)?;
    let layer_map = ArchitectureAnalyzer::new(&*storage).module_layer_map(&project_id)?;

    let doc_title = if title.trim().is_empty() {
        format!("{project} — architecture")
    } else {
        title.to_string()
    };
    let mut doc = from_overview(&overview, &layer_map, &doc_title);
    doc.meta.locale = if locale == "zh-CN" { "zh-CN" } else { "en" }.to_string();

    // Optional git evidence; failures become diagnostics (showcase blocks).
    let revision = storage
        .get_project(&project_id)
        .ok()
        .flatten()
        .map(|record| record.last_commit)
        .unwrap_or_default();
    let (evidence_report, evidence_diagnostics) = if repo_root.trim().is_empty() {
        (
            None,
            vec![evidence_diagnostic(
                "diagram/evidence-skipped",
                "no --repo_root given; source badges omitted",
            )],
        )
    } else if revision.is_empty() {
        (
            None,
            vec![evidence_diagnostic(
                "diagram/evidence-skipped",
                "project has no indexed commit; re-index with --force to pin a revision",
            )],
        )
    } else {
        match verify(
            std::path::Path::new(repo_root),
            &revision,
            &doc.components
                .iter()
                .flat_map(|c| c.sources.iter())
                .cloned()
                .collect::<Vec<_>>(),
            repo_url,
        ) {
            Ok(report) => (Some(report), Vec::new()),
            Err(err) => (
                None,
                vec![Diagnostic {
                    code: "diagram/evidence-invalid".to_string(),
                    severity: Severity::Error,
                    subject: repo_root.to_string(),
                    message: format!("git evidence verification failed: {err}"),
                    evidence: serde_json::json!({ "revision": revision }),
                    supported_fixes: vec![
                        "Re-run codenexus index at the current HEAD, then re-render".to_string(),
                        "Check --repo_url matches `git remote get-url origin`".to_string(),
                    ],
                }],
            ),
        }
    };

    let rendered = render(&doc, profile, evidence_report.as_ref()).map_err(map_diagram_error)?;
    if profile == QualityProfile::Showcase
        && evidence_diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    {
        return Err(CodeNexusError::InvalidInput(format!(
            "diagram blocked by evidence gate: {}",
            evidence_diagnostics
                .iter()
                .map(|d| d.code.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )));
    }

    let artifact = crate::diagram::write_atomically(
        std::path::Path::new(output_path),
        rendered.html.as_bytes(),
    )
    .map_err(CodeNexusError::Io)?;

    let evidence_summary = evidence_report.map(|report| EvidenceSummary {
        verified: report.verified,
        repository: report.repository,
        revision: report.revision,
        reference_count: report.references.len() as u32,
    });
    let mut diagnostics = rendered.diagnostics;
    diagnostics.extend(evidence_diagnostics);

    Ok(DiagramOutput {
        project: project.to_string(),
        output_path: output_path.to_string(),
        receipt: Receipt {
            command: "diagram".to_string(),
            specification: rendered.specification,
            artifact,
            validation: rendered.validation,
            evidence: evidence_summary,
            diagnostics,
        },
    })
}

#[cfg(feature = "diagram")]
fn evidence_diagnostic(code: &str, message: &str) -> Diagnostic {
    Diagnostic {
        code: code.to_string(),
        severity: Severity::Warning,
        subject: "evidence".to_string(),
        message: message.to_string(),
        evidence: serde_json::json!({}),
        supported_fixes: vec![
            "Provide --repo_root (the git worktree root of the indexed project)".to_string(),
        ],
    }
}

#[cfg(feature = "diagram")]
fn map_diagram_error(err: DiagramError) -> CodeNexusError {
    match err {
        DiagramError::QualityGate(diagnostics) => CodeNexusError::InvalidInput(format!(
            "diagram blocked by quality gate: {}",
            diagnostics
                .iter()
                .map(|d| d.code.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )),
        DiagramError::Template(placeholder) => CodeNexusError::Internal(format!(
            "diagram template missing placeholder {placeholder}"
        )),
    }
}

/// CLI wrapper — writes the artifact and prints the receipt to stdout.
#[cfg(all(feature = "cli", feature = "diagram"))]
#[forge(
    name = "diagram",
    version = "0.3.12",
    description = "Render a self-contained interactive architecture HTML from the indexed graph.",
    cli = true
)]
#[allow(clippy::too_many_arguments)]
async fn diagram(
    project: String,
    output: String,
    quality: String,
    repo_root: String,
    repo_url: String,
    title: String,
    locale: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let out = run_diagram(
        &kit, &project, &output, &quality, &repo_root, &repo_url, &title, &locale,
    )
    .map_err(|e| to_api_error(e, "diagram_error"))?;
    let json = serde_json::to_string(&out.receipt)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "diagram_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns the receipt (the HTML goes to `--output`).
#[cfg(all(feature = "mcp", feature = "diagram"))]
#[forge(
    name = "diagram",
    version = "0.3.12",
    tool_name = "diagram",
    description = "Render a self-contained interactive architecture HTML from the indexed graph."
)]
#[allow(clippy::too_many_arguments)]
async fn diagram_mcp(
    project: String,
    output: String,
    quality: String,
    repo_root: String,
    repo_url: String,
    title: String,
    locale: String,
) -> Result<Receipt, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_diagram(
        &kit, &project, &output, &quality, &repo_root, &repo_url, &title, &locale,
    )
    .map_err(|e| to_api_error(e, "diagram_error"))
    .map(|out| out.receipt)
}

#[cfg(all(test, feature = "cli", feature = "diagram"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_diagram_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Seeds two modules (src/api controller, src/db) with a CALLS edge.
    fn seed_two_modules(storage: &dyn Storage) {
        storage.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/demo', language: 'rust', fileCount: 2, indexedAt: 1000, lastCommit: 'abc'});").expect("project");
        storage.execute("CREATE (:Function {id: 'f_ctrl', project: 'demo', name: 'list_users', qualifiedName: 'demo.list_users', filePath: '/src/api/h.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("ctrl");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("route");
        storage.execute("CREATE (:CodeRelation {id: 'e_hr', source: 'f_ctrl', target: 'r1', type: 'HANDLES_ROUTE', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("handles");
        storage.execute("CREATE (:Function {id: 'f_db', project: 'demo', name: 'query_users', qualifiedName: 'demo.query_users', filePath: '/src/db/q.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("db");
        storage.execute("CREATE (:CodeRelation {id: 'e_call', source: 'f_ctrl', target: 'f_db', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("call");
    }

    use crate::storage::capability::Storage;

    #[test]
    fn run_diagram_writes_artifact_and_receipt() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_two_modules(&*storage);
        let out_dir = TempDir::new().unwrap();
        let target = out_dir.path().join("arch.html");

        let out = run_diagram(
            &kit,
            "demo",
            target.to_str().unwrap(),
            "standard",
            "",
            "",
            "",
            "en",
        )
        .expect("run_diagram should succeed");
        assert!(target.exists(), "artifact written");
        assert!(out.receipt.validation.check_count > 0);
        assert_eq!(out.receipt.validation.quality_profile, "standard");
        let html = std::fs::read(&target).unwrap();
        let text = String::from_utf8(html).unwrap();
        assert!(text.contains("data-node-id=\"src-api\""), "{text}");
        assert!(text.contains("static index facts"), "truth boundary footer");
    }

    #[test]
    fn run_diagram_is_deterministic() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_two_modules(&*storage);
        let out_dir = TempDir::new().unwrap();
        let t1 = out_dir.path().join("one.html");
        let t2 = out_dir.path().join("two.html");
        run_diagram(
            &kit,
            "demo",
            t1.to_str().unwrap(),
            "standard",
            "",
            "",
            "",
            "en",
        )
        .expect("first run");
        run_diagram(
            &kit,
            "demo",
            t2.to_str().unwrap(),
            "standard",
            "",
            "",
            "",
            "en",
        )
        .expect("second run");
        assert_eq!(
            std::fs::read(&t1).unwrap(),
            std::fs::read(&t2).unwrap(),
            "same DB → byte-identical artifact"
        );
    }

    #[test]
    fn run_diagram_types_controller_module_as_interface() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_two_modules(&*storage);
        let out_dir = TempDir::new().unwrap();
        let target = out_dir.path().join("arch.html");
        let out = run_diagram(
            &kit,
            "demo",
            target.to_str().unwrap(),
            "standard",
            "",
            "",
            "",
            "en",
        )
        .expect("run_diagram");
        let html = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
        assert!(
            html.contains("node-interface\" data-node-id=\"src-api\""),
            "controller module typed interface: {}",
            html
        );
    }

    #[test]
    fn run_diagram_rejects_unknown_profile_and_empty_output() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_two_modules(&*storage);
        let out_dir = TempDir::new().unwrap();
        let target = out_dir.path().join("a.html");
        let bad_quality = run_diagram(
            &kit,
            "demo",
            target.to_str().unwrap(),
            "fancy",
            "",
            "",
            "",
            "en",
        );
        assert!(matches!(bad_quality, Err(CodeNexusError::InvalidInput(_))));
        let empty_output = run_diagram(&kit, "demo", "", "standard", "", "", "", "en");
        assert!(matches!(empty_output, Err(CodeNexusError::InvalidInput(_))));
    }

    // ===== forge wrapper smoke (kit initialized) =====

    #[serial_test::serial(kit_init)]
    #[test]
    fn diagram_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_two_modules(&*storage);
        let out_dir = TempDir::new().unwrap();
        let target = out_dir.path().join("arch.html");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(diagram(
            "demo".to_string(),
            target.to_str().unwrap().to_string(),
            "standard".to_string(),
            String::new(),
            String::new(),
            String::new(),
            "en".to_string(),
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }
}
