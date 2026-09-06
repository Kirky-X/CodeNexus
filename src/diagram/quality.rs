// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Geometric quality gates, applied to the final artifact.
//!
//! Following archify's discipline, checks audit the *rendered output* (the
//! SVG string) plus its geometry inputs, and every failure is a
//! [`Diagnostic`] with a stable rule code, the exact subject, measured
//! evidence, and curated fixes — never a bare message. `standard` treats
//! warnings as advisory; `showcase` escalates them to errors and fails
//! closed.

use crate::diagnostics::{Diagnostic, Severity};

use super::ir::DiagramDocument;
use super::layout::Layout;
use super::route::RoutedEdge;
use super::text::text_units;

/// Quality gate profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityProfile {
    /// Warnings are advisory; artifact ships.
    Standard,
    /// Warnings escalate to errors; any error blocks delivery.
    Showcase,
}

impl QualityProfile {
    /// Parses `--quality` values (empty means standard, house sentinel).
    ///
    /// # Errors
    ///
    /// Returns the offending value for unknown profiles.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "standard" => Ok(QualityProfile::Standard),
            "showcase" => Ok(QualityProfile::Showcase),
            other => Err(other.to_string()),
        }
    }

    /// Label-clearance threshold in px (`4` for showcase, `2` otherwise).
    #[must_use]
    pub fn label_clearance_px(self) -> f64 {
        match self {
            QualityProfile::Standard => 2.0,
            QualityProfile::Showcase => 4.0,
        }
    }
}

/// Escalates warnings to errors under `Showcase` (fail-closed gating).
#[must_use]
pub fn apply_profile(diagnostics: Vec<Diagnostic>, profile: QualityProfile) -> Vec<Diagnostic> {
    match profile {
        QualityProfile::Standard => diagnostics,
        QualityProfile::Showcase => diagnostics
            .into_iter()
            .map(|d| Diagnostic {
                severity: if d.severity == Severity::Warning {
                    Severity::Error
                } else {
                    d.severity
                },
                ..d
            })
            .collect(),
    }
}

/// Runs every geometric gate. `svg` is the rendered artifact; the geometry
/// inputs back the positional checks.
#[must_use]
pub fn check_artifact(
    svg: &str,
    doc: &DiagramDocument,
    layout: &Layout,
    edges: &[RoutedEdge],
    profile: QualityProfile,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    check_single_svg(svg, &mut diagnostics);
    check_finite_svg(svg, &mut diagnostics);
    check_orthogonal_arrows(edges, &mut diagnostics);
    check_route_rhythm(edges, &mut diagnostics);
    check_label_clearance(doc, layout, edges, profile, &mut diagnostics);
    check_relationship_crossings(edges, &mut diagnostics);
    check_legend_clearance(layout, edges, &mut diagnostics);
    diagnostics
}

fn check_single_svg(svg: &str, out: &mut Vec<Diagnostic>) {
    let roots = svg.matches("<svg").count();
    if roots != 1 {
        out.push(Diagnostic {
            code: "diagram/single-svg".to_string(),
            severity: Severity::Error,
            subject: "svg".to_string(),
            message: format!("expected exactly 1 svg root, found {roots}"),
            evidence: serde_json::json!({ "svg_roots": roots }),
            supported_fixes: vec![
                "Re-render from the diagram IR; never concatenate SVG fragments".to_string(),
            ],
        });
    }
}

fn check_finite_svg(svg: &str, out: &mut Vec<Diagnostic>) {
    let mut bad = 0_usize;
    if svg.contains("NaN") {
        bad += 1;
    }
    if svg.contains("Infinity") {
        bad += 1;
    }
    if bad > 0 {
        out.push(Diagnostic {
            code: "diagram/finite-svg".to_string(),
            severity: Severity::Error,
            subject: "svg".to_string(),
            message: "SVG contains non-finite coordinates (NaN or Infinity)".to_string(),
            evidence: serde_json::json!({ "offending_tokens": bad }),
            supported_fixes: vec![
                "Fix the layout/router source; do not post-process the SVG string".to_string(),
            ],
        });
    }
}

fn check_orthogonal_arrows(edges: &[RoutedEdge], out: &mut Vec<Diagnostic>) {
    for edge in edges {
        for pair in edge.points.windows(2) {
            if pair[0].0 != pair[1].0 && pair[0].1 != pair[1].1 {
                out.push(Diagnostic {
                    code: "diagram/orthogonal-arrows".to_string(),
                    severity: Severity::Error,
                    subject: format!("{}->{}", edge.from, edge.to),
                    message: "edge contains a diagonal segment".to_string(),
                    evidence: serde_json::json!({
                        "segment": [pair[0], pair[1]],
                    }),
                    supported_fixes: vec![
                        "Re-route with route_edges; diagonal segments are a router bug".to_string(),
                    ],
                });
            }
        }
    }
}

const MIN_SEGMENT_PX: f64 = 8.0;

fn check_route_rhythm(edges: &[RoutedEdge], out: &mut Vec<Diagnostic>) {
    for edge in edges {
        for pair in edge.points.windows(2) {
            let len = (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1);
            if len < MIN_SEGMENT_PX {
                out.push(Diagnostic {
                    code: "diagram/route-rhythm".to_string(),
                    severity: Severity::Warning,
                    subject: format!("{}->{}", edge.from, edge.to),
                    message: format!(
                        "segment shorter than {MIN_SEGMENT_PX}px creates visual jitter"
                    ),
                    evidence: serde_json::json!({
                        "min_segment_px": (len * 100.0).round() / 100.0,
                        "segment": [pair[0], pair[1]],
                    }),
                    supported_fixes: vec![
                        "Increase grid gaps or reduce parallel connections between the pair"
                            .to_string(),
                    ],
                });
            }
        }
    }
}

/// Approximate a node label as a box for clearance measurement.
struct LabelBox {
    subject: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

const LABEL_HEIGHT_PX: f64 = 14.0;
const LABEL_HALF_W_UNITS: f64 = 0.5;

fn label_boxes(doc: &DiagramDocument, layout: &Layout) -> Vec<LabelBox> {
    let by_id: std::collections::BTreeMap<&str, &super::layout::Placement> = layout
        .placements
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    doc.components
        .iter()
        .filter_map(|c| {
            let place = by_id.get(c.id.as_str())?;
            let w = text_units(&c.label) * 8.0;
            let half = w * LABEL_HALF_W_UNITS;
            Some(LabelBox {
                subject: c.id.clone(),
                x: place.x + place.w / 2.0 - half,
                y: place.y + place.h / 2.0 - LABEL_HEIGHT_PX / 2.0 - 4.0,
                w,
                h: LABEL_HEIGHT_PX,
            })
        })
        .collect()
}

fn segment_rect_min_distance(x0: f64, y0: f64, x1: f64, y1: f64, rect: &LabelBox) -> f64 {
    // Sample the segment (orthogonal segments make 16 samples exact enough
    // for a px-scale gate) and take the closest distance to the rect.
    let mut min = f64::INFINITY;
    for step in 0..=16_usize {
        let t = f64::from(step as u32) / 16.0;
        let px = x0 + (x1 - x0) * t;
        let py = y0 + (y1 - y0) * t;
        let rx = if px < rect.x {
            rect.x - px
        } else if px > rect.x + rect.w {
            px - (rect.x + rect.w)
        } else {
            0.0
        };
        let ry = if py < rect.y {
            rect.y - py
        } else if py > rect.y + rect.h {
            py - (rect.y + rect.h)
        } else {
            0.0
        };
        min = min.min(rx.hypot(ry));
    }
    min
}

fn check_label_clearance(
    doc: &DiagramDocument,
    layout: &Layout,
    edges: &[RoutedEdge],
    profile: QualityProfile,
    out: &mut Vec<Diagnostic>,
) {
    let threshold = profile.label_clearance_px();
    let labels = label_boxes(doc, layout);
    for edge in edges {
        for pair in edge.points.windows(2) {
            for label in &labels {
                if label.subject == edge.from || label.subject == edge.to {
                    continue; // own endpoints may touch their box edge
                }
                let dist =
                    segment_rect_min_distance(pair[0].0, pair[0].1, pair[1].0, pair[1].1, label);
                if dist < threshold {
                    out.push(Diagnostic {
                        code: "diagram/label-clearance".to_string(),
                        severity: Severity::Warning,
                        subject: format!("{}->{}", edge.from, edge.to),
                        message: format!(
                            "edge passes within {threshold}px of a node label (measured {:.2}px)",
                            dist
                        ),
                        evidence: serde_json::json!({
                            "min_clearance_px": (dist * 100.0).round() / 100.0,
                            "threshold_px": threshold,
                        }),
                        supported_fixes: vec![
                            "Re-route the edge through a wider channel; deleting the label is never a fix"
                                .to_string(),
                        ],
                    });
                }
            }
        }
    }
}

fn segments_cross(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    // Axis-aligned segments only: crossing test per orientation pair.
    let h_or_v = |p: (f64, f64), q: (f64, f64)| p.0 == q.0 || p.1 == q.1;
    if !h_or_v(a, b) || !h_or_v(c, d) {
        return false;
    }
    let a_horiz = a.1 == b.1;
    let c_horiz = c.1 == d.1;
    match (a_horiz, c_horiz) {
        (true, false) => {
            let (xmin, xmax) = (a.0.min(b.0), a.0.max(b.0));
            let (ymin, ymax) = (c.1.min(d.1), c.1.max(d.1));
            xmin <= c.0 && c.0 <= xmax && ymin <= a.1 && a.1 <= ymax
        }
        (false, true) => segments_cross(c, d, a, b),
        _ => false,
    }
}

/// Relationship crossings are counted, then reported when they exceed one
/// per two connections (heuristic visual-noise budget).
fn check_relationship_crossings(edges: &[RoutedEdge], out: &mut Vec<Diagnostic>) {
    let budget = (edges.len() / 2).max(1);
    let mut crossings = 0_usize;
    for (i, e1) in edges.iter().enumerate() {
        for e2 in &edges[i + 1..] {
            for w1 in e1.points.windows(2) {
                for w2 in e2.points.windows(2) {
                    if segments_cross(w1[0], w1[1], w2[0], w2[1]) {
                        crossings += 1;
                    }
                }
            }
        }
    }
    if crossings > budget {
        out.push(Diagnostic {
            code: "diagram/relationship-crossings".to_string(),
            severity: Severity::Warning,
            subject: "edges".to_string(),
            message: format!(
                "{crossings} edge crossings exceed the visual-noise budget of {budget}"
            ),
            evidence: serde_json::json!({ "crossings": crossings, "budget": budget }),
            supported_fixes: vec![
                "Reorder components or reduce cross-row connections to untangle routes".to_string(),
            ],
        });
    }
}

fn check_legend_clearance(layout: &Layout, edges: &[RoutedEdge], out: &mut Vec<Diagnostic>) {
    if layout.placements.is_empty() {
        return;
    }
    // Legend occupies the top-left 200x120 strip (see svg.rs renderer).
    let legend = LabelBox {
        subject: "legend".to_string(),
        x: 16.0,
        y: 12.0,
        w: 200.0,
        h: 120.0,
    };
    let mut collide = false;
    for edge in edges {
        for pair in edge.points.windows(2) {
            if segment_rect_min_distance(pair[0].0, pair[0].1, pair[1].0, pair[1].1, &legend) == 0.0
            {
                collide = true;
            }
        }
    }
    for place in &layout.placements {
        let overlap = place.x < legend.x + legend.w
            && place.x + place.w > legend.x
            && place.y < legend.y + legend.h
            && place.y + place.h > legend.y;
        collide |= overlap;
    }
    if collide {
        out.push(Diagnostic {
            code: "diagram/legend-clearance".to_string(),
            severity: Severity::Warning,
            subject: "legend".to_string(),
            message: "legend strip collides with nodes or routed edges".to_string(),
            evidence: serde_json::json!({ "legend_strip": [legend.x, legend.y, legend.w, legend.h] }),
            supported_fixes: vec![
                "Keep the top-left 200x120 strip free (increase layout margin)".to_string(),
            ],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::ir::{
        ComponentType, DiagramComponent, DiagramConnection, DiagramMeta, EdgeVariant,
    };
    use crate::diagram::layout::layout_components;
    use crate::diagram::layout::Placement;
    use crate::diagram::route::route_edges;

    fn doc() -> DiagramDocument {
        DiagramDocument {
            schema_version: 1,
            diagram_type: "architecture".to_string(),
            meta: DiagramMeta {
                title: "t".to_string(),
                subtitle: None,
                locale: "en".to_string(),
                repository: None,
            },
            components: vec![
                DiagramComponent {
                    id: "a".to_string(),
                    r#type: ComponentType::Service,
                    label: "a".to_string(),
                    sublabel: None,
                    tag: None,
                    sources: vec![],
                },
                DiagramComponent {
                    id: "b".to_string(),
                    r#type: ComponentType::Service,
                    label: "b".to_string(),
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
    fn clean_diagram_passes_all_gates() {
        let document = doc();
        let layout = layout_components(&document.components);
        let edges = route_edges(&document, &layout);
        let svg = crate::diagram::svg::render_svg(&document, &layout, &edges);
        let diagnostics =
            check_artifact(&svg, &document, &layout, &edges, QualityProfile::Showcase);
        assert!(
            diagnostics.is_empty(),
            "clean diagram flagged: {diagnostics:?}"
        );
    }

    #[test]
    fn multiple_svg_roots_and_nan_are_errors() {
        let document = doc();
        let layout = layout_components(&document.components);
        let diagnostics = check_artifact(
            "<svg></svg><svg></svg>NaN Infinity",
            &document,
            &layout,
            &[],
            QualityProfile::Standard,
        );
        let codes: Vec<&str> = diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert!(codes.contains(&"diagram/single-svg"), "{codes:?}");
        assert!(codes.contains(&"diagram/finite-svg"), "{codes:?}");
        assert!(diagnostics.iter().all(|d| d.severity == Severity::Error));
    }

    #[test]
    fn diagonal_segment_trips_orthogonal_gate() {
        let document = doc();
        let layout = layout_components(&document.components);
        let mut bad = route_edges(&document, &layout);
        bad[0].points = vec![(0.0, 0.0), (10.0, 10.0)];
        let diagnostics = check_artifact(
            "<svg></svg>",
            &document,
            &layout,
            &bad,
            QualityProfile::Standard,
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "diagram/orthogonal-arrows"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn micro_segment_trips_rhythm_gate() {
        let document = doc();
        let layout = layout_components(&document.components);
        let mut jittery = route_edges(&document, &layout);
        let p = jittery[0].points.clone();
        let mut pts = vec![p[0], (p[0].0 + 2.0, p[0].1)];
        pts.extend(p.iter().copied().skip(1));
        jittery[0].points = pts;
        let diagnostics = check_artifact(
            "<svg></svg>",
            &document,
            &layout,
            &jittery,
            QualityProfile::Standard,
        );
        assert!(
            diagnostics.iter().any(|d| d.code == "diagram/route-rhythm"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn showcase_escalates_warnings_to_errors() {
        let diagnostics = vec![Diagnostic {
            code: "diagram/route-rhythm".to_string(),
            severity: Severity::Warning,
            subject: "a->b".to_string(),
            message: "x".to_string(),
            evidence: serde_json::json!({}),
            supported_fixes: vec![],
        }];
        let standard = apply_profile(diagnostics.clone(), QualityProfile::Standard);
        assert_eq!(standard[0].severity, Severity::Warning);
        let showcase = apply_profile(diagnostics, QualityProfile::Showcase);
        assert_eq!(showcase[0].severity, Severity::Error);
    }

    #[test]
    fn edge_crossing_the_legend_trips_legend_gate() {
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
        let edges = vec![RoutedEdge {
            from: "a".to_string(),
            to: "a".to_string(),
            variant: EdgeVariant::Primary,
            points: vec![(20.0, 100.0), (100.0, 100.0)],
            label_anchor: (60.0, 96.0),
        }];
        let diagnostics = check_artifact(
            "<svg></svg>",
            &document,
            &layout,
            &edges,
            QualityProfile::Standard,
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "diagram/legend-clearance"),
            "{diagnostics:?}"
        );
    }
}
