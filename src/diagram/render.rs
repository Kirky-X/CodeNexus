// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Render orchestration: IR → layout → routing → SVG → gates → template.
//!
//! The pipeline is fail-closed: under the `showcase` profile any gate error
//! aborts rendering and no HTML is produced, so a failing candidate never
//! replaces the last good artifact.

use std::collections::BTreeMap;

use thiserror::Error;

use super::evidence::EvidenceReport;
use super::ir::DiagramDocument;
use super::layout::{layout_components, Layout};
use super::quality::{apply_profile, check_artifact, QualityProfile};
use super::route::route_edges;
use super::svg::render_svg;

use crate::diagnostics::{Diagnostic, HashInfo, ValidationSummary};

const TEMPLATE: &str = include_str!("template.html");

/// Errors from the render stage.
#[derive(Debug, Error)]
pub enum DiagramError {
    /// A quality gate blocked delivery under the active profile.
    #[error("quality gate blocked delivery: {} blocking diagnostics", .0.len())]
    QualityGate(Vec<Diagnostic>),
    /// The template was missing a required placeholder (programmer error).
    #[error("template placeholder missing: {0}")]
    Template(String),
}

/// Successful render output before delivery.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedDiagram {
    /// Self-contained HTML artifact.
    pub html: String,
    /// Hash of the canonical IR specification (BLAKE3 of the serialized IR).
    pub specification: HashInfo,
    /// Gate summary.
    pub validation: ValidationSummary,
    /// All gate diagnostics (post-profile escalation).
    pub diagnostics: Vec<Diagnostic>,
}

/// Number of distinct gates executed (kept in sync with `check_artifact`).
const GATE_COUNT: u32 = 7;

/// Renders the self-contained HTML for `doc` under `profile`.
///
/// # Errors
///
/// Returns [`DiagramError::QualityGate`] when escalated diagnostics contain
/// an error (showcase fail-closed).
pub fn render(
    doc: &DiagramDocument,
    profile: QualityProfile,
    evidence: Option<&EvidenceReport>,
) -> Result<RenderedDiagram, DiagramError> {
    let layout = layout_components(&doc.components);
    let edges = route_edges(doc, &layout);
    render_with(doc, &layout, &edges, profile, evidence)
}

/// Gate entry with caller-supplied geometry (used by tests to inject
/// violating geometry without depending on the auto-layout).
pub fn render_with(
    doc: &DiagramDocument,
    layout: &Layout,
    edges: &[super::route::RoutedEdge],
    profile: QualityProfile,
    evidence: Option<&EvidenceReport>,
) -> Result<RenderedDiagram, DiagramError> {
    let spec_json = serde_json::to_string(doc)
        .map_err(|_| DiagramError::Template("ir serialization".to_string()))?;
    let specification = HashInfo {
        algorithm: "blake3".to_string(),
        hash: blake3::hash(spec_json.as_bytes()).to_string(),
        bytes: spec_json.len() as u64,
    };

    let svg = render_svg(doc, layout, edges);
    let diagnostics = apply_profile(check_artifact(&svg, doc, layout, edges, profile), profile);
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostics::Severity::Error)
        .count() as u32;
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostics::Severity::Warning)
        .count() as u32;
    if errors > 0 {
        return Err(DiagramError::QualityGate(diagnostics));
    }
    let blocking_codes: std::collections::BTreeSet<&str> =
        diagnostics.iter().map(|d| d.code.as_str()).collect();
    let validation = ValidationSummary {
        checks_passed: GATE_COUNT - blocking_codes.len() as u32,
        check_count: GATE_COUNT,
        errors,
        warnings,
        quality_profile: match profile {
            QualityProfile::Standard => "standard".to_string(),
            QualityProfile::Showcase => "showcase".to_string(),
        },
    };

    let html = fill_template(doc, &svg, evidence).map_err(DiagramError::Template)?;
    Ok(RenderedDiagram {
        html,
        specification,
        validation,
        diagnostics,
    })
}

/// Serializes JSON for a `<script>` slot, escaping `<` so embedded content
/// can never close the script tag.
fn script_json<T: serde::Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    json.replace('<', "\\u003c")
}

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Builds the per-node verified-source map consumed by the viewer's SRC
/// badges: node id → references whose declared path matches a component
/// source.
fn fill_template(
    doc: &DiagramDocument,
    svg: &str,
    evidence: Option<&EvidenceReport>,
) -> Result<String, String> {
    let locale = if doc.meta.locale == "zh-CN" {
        "zh-CN"
    } else {
        "en"
    };

    let mut node_sources: BTreeMap<String, Vec<super::evidence::VerifiedSource>> = BTreeMap::new();
    if let Some(report) = evidence {
        for component in &doc.components {
            let mut refs = Vec::new();
            for source in &component.sources {
                if let Some(verified) = report.references.iter().find(|r| r.path == source.path) {
                    refs.push(verified.clone());
                }
            }
            node_sources.insert(component.id.clone(), refs);
        }
    }
    let data = serde_json::json!({
        "nodes": doc.components.iter().map(|c| serde_json::json!({
            "id": c.id,
            "label": c.label,
            "sublabel": c.sublabel,
            "tag": c.tag,
            "type": c.r#type,
        })).collect::<Vec<_>>(),
        "edges": doc.connections.iter().map(|c| serde_json::json!({
            "from": c.from,
            "to": c.to,
            "variant": c.variant,
            "label": c.label,
        })).collect::<Vec<_>>(),
    });
    let evidence_payload = evidence.map(|report| {
        serde_json::json!({
            "verified": report.verified,
            "repository": report.repository,
            "revision": report.revision,
            "node_sources": node_sources,
        })
    });

    let placeholders = [
        "__CNX_TITLE__",
        "__CNX_SVG__",
        "__CNX_DATA__",
        "__CNX_EVIDENCE__",
        "__CNX_LOCALE__",
    ];
    let mut html = TEMPLATE.to_string();
    for placeholder in placeholders {
        if !html.contains(placeholder) {
            return Err(placeholder.to_string());
        }
    }
    html = html
        .replace(
            "__CNX_TITLE__",
            &format!("{} — CodeNexus", escape_html(&doc.meta.title)),
        )
        .replace("__CNX_SVG__", svg)
        .replace("__CNX_DATA__", &script_json(&data))
        .replace("__CNX_EVIDENCE__", &script_json(&evidence_payload))
        .replace("__CNX_LOCALE__", locale);
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::ir::{
        ComponentType, DiagramComponent, DiagramConnection, DiagramMeta, EdgeVariant,
    };
    use crate::diagram::layout::Placement;

    fn doc() -> DiagramDocument {
        DiagramDocument {
            schema_version: 1,
            diagram_type: "architecture".to_string(),
            meta: DiagramMeta {
                title: "demo".to_string(),
                subtitle: None,
                locale: "en".to_string(),
                repository: None,
            },
            components: vec![
                DiagramComponent {
                    id: "a".to_string(),
                    r#type: ComponentType::Interface,
                    label: "api".to_string(),
                    sublabel: Some("2 files".to_string()),
                    tag: Some("Controller".to_string()),
                    sources: vec![],
                },
                DiagramComponent {
                    id: "b".to_string(),
                    r#type: ComponentType::Storage,
                    label: "db".to_string(),
                    sublabel: None,
                    tag: None,
                    sources: vec![],
                },
            ],
            connections: vec![DiagramConnection {
                from: "a".to_string(),
                to: "b".to_string(),
                label: String::new(),
                variant: EdgeVariant::Primary,
            }],
        }
    }

    #[test]
    fn rendering_is_deterministic_and_self_contained() {
        let document = doc();
        let r1 = render(&document, QualityProfile::Standard, None).unwrap();
        let r2 = render(&document, QualityProfile::Standard, None).unwrap();
        assert_eq!(r1.html, r2.html, "byte-identical across runs");
        assert_eq!(r1.specification, r2.specification);
        assert!(r1.html.contains("data-node-id=\"a\""), "semantic attrs");
        assert!(r1.html.contains("static index facts"), "truth boundary");
        assert!(r1.html.contains("id=\"cnx-data\""), "data slot");
        assert!(r1.html.contains("id=\"cnx-evidence\""), "evidence slot");
        assert!(!r1.html.contains("http://cdn"), "no external resources");
        assert!(r1.html.starts_with("<!DOCTYPE html>"));
        assert!(r1.validation.checks_passed == r1.validation.check_count);
    }

    #[test]
    fn showcase_violation_blocks_rendering() {
        let document = doc();
        let layout = Layout {
            placements: vec![Placement {
                id: "a".to_string(),
                x: 400.0,
                y: 40.0,
                w: 200.0,
                h: 100.0,
            }],
            view_box: (0.0, 0.0, 800.0, 300.0),
        };
        // A micro segment trips route-rhythm; showcase escalates it to an
        // error so render must refuse.
        let edges = vec![crate::diagram::route::RoutedEdge {
            from: "a".to_string(),
            to: "a".to_string(),
            variant: EdgeVariant::Primary,
            points: vec![(100.0, 200.0), (103.0, 200.0)],
            label_anchor: (101.5, 196.0),
        }];
        let err = render_with(&document, &layout, &edges, QualityProfile::Showcase, None)
            .expect_err("showcase must block");
        match err {
            DiagramError::QualityGate(diagnostics) => {
                assert!(diagnostics.iter().any(|d| d.code == "diagram/route-rhythm"
                    && d.severity == crate::diagnostics::Severity::Error));
            }
            other => panic!("expected QualityGate, got {other:?}"),
        }
    }

    #[test]
    fn evidence_is_baked_per_node_with_blob_links() {
        let document = doc();
        let report = EvidenceReport {
            verified: true,
            repository: Some("https://github.com/owner/repo".to_string()),
            revision: Some("a".repeat(40)),
            references: vec![super::super::evidence::VerifiedSource {
                path: "/src/api.rs".to_string(),
                label: None,
                line: Some(1),
                end_line: Some(5),
                href: Some("https://github.com/owner/repo/blob/x#/L1-L5".to_string()),
            }],
        };
        let mut with_sources = doc();
        with_sources.components[0].sources = vec![crate::diagram::ir::SourceRef {
            path: "/src/api.rs".to_string(),
            line: Some(1),
            end_line: Some(5),
            label: None,
        }];
        let rendered = render(&with_sources, QualityProfile::Standard, Some(&report)).unwrap();
        assert!(
            rendered.html.contains("node_sources"),
            "{:?}",
            rendered.validation
        );
        assert!(rendered.html.contains("/src/api.rs"));
        assert!(rendered.html.contains("\"verified\":true"));
    }
}
