// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Deterministic SVG writer.
//!
//! Attributes are emitted in a fixed order and every coordinate is checked
//! finite before serialization, so identical inputs render byte-identical
//! output with a single `<svg>` root and no `NaN`/`Infinity` anywhere.
//! Nodes and edges carry `data-*` semantic attributes — the viewer's focus,
//! reach, and route-probe features select on these, never on geometry.

use std::fmt::Write as _;

use super::ir::{ComponentType, DiagramDocument, EdgeVariant};
use super::layout::Layout;
use super::route::RoutedEdge;
use super::text::fit_label;

/// Escape XML special characters in text and attribute values.
fn esc(value: &str) -> String {
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

fn fmt_coord(value: f64) -> String {
    // Round to 2 decimals and strip trailing zeros for stable, compact output.
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == rounded.trunc() {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.2}")
    }
}

fn css_class(kind: ComponentType) -> &'static str {
    match kind {
        ComponentType::Interface => "node-interface",
        ComponentType::Service => "node-service",
        ComponentType::Storage => "node-storage",
        ComponentType::Model => "node-model",
    }
}

/// Label budget in measurement units before truncation (box width / 8px per
/// unit minus padding).
const LABEL_UNIT_BUDGET: f64 = 28.0;

/// Renders the diagram SVG. Panics via assertion if any coordinate is
/// non-finite — that is a routing bug, not user input error.
#[must_use]
pub fn render_svg(doc: &DiagramDocument, layout: &Layout, edges: &[RoutedEdge]) -> String {
    let mut out = String::with_capacity(16 * 1024);
    let (vx, vy, vw, vh) = layout.view_box;
    assert!(
        [vx, vy, vw, vh].iter().all(|v| v.is_finite()),
        "non-finite view box"
    );
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\" \
         class=\"arch-diagram\" role=\"img\" aria-label=\"{}\">",
        fmt_coord(vx),
        fmt_coord(vy),
        fmt_coord(vw),
        fmt_coord(vh),
        esc(&doc.meta.title)
    );

    // Edges first so nodes render on top.
    out.push_str("<g class=\"edges\">");
    for edge in edges {
        assert!(
            edge.points
                .iter()
                .all(|(x, y)| x.is_finite() && y.is_finite()),
            "non-finite point on edge {} -> {}",
            edge.from,
            edge.to
        );
        let points = edge
            .points
            .iter()
            .map(|(x, y)| format!("{},{}", fmt_coord(*x), fmt_coord(*y)))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = write!(
            out,
            "<g class=\"edge edge-{}\" data-edge-from=\"{}\" data-edge-to=\"{}\" data-variant=\"{}\">",
            edge_variant_name(edge.variant),
            esc(&edge.from),
            esc(&edge.to),
            edge_variant_name(edge.variant),
        );
        let _ = write!(out, "<polyline points=\"{points}\"/>");
        if !edge_label(doc, edge).is_empty() {
            let label = edge_label(doc, edge);
            let _ = write!(
                out,
                "<text class=\"edge-label\" x=\"{}\" y=\"{}\">{}</text>",
                fmt_coord(edge.label_anchor.0),
                fmt_coord(edge.label_anchor.1 - 4.0),
                esc(&label)
            );
        }
        out.push_str("</g>");
    }
    out.push_str("</g>");

    // Nodes.
    out.push_str("<g class=\"nodes\">");
    let by_id: std::collections::BTreeMap<&str, &super::layout::Placement> = layout
        .placements
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    for component in &doc.components {
        let Some(place) = by_id.get(component.id.as_str()) else {
            continue;
        };
        let _ = write!(
            out,
            "<g class=\"node {}\" data-node-id=\"{}\">",
            css_class(component.r#type),
            esc(&component.id)
        );
        let _ = write!(
            out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"10\"/>",
            fmt_coord(place.x),
            fmt_coord(place.y),
            fmt_coord(place.w),
            fmt_coord(place.h)
        );
        let label = fit_label(&component.label, LABEL_UNIT_BUDGET);
        let _ = write!(
            out,
            "<text class=\"node-label\" x=\"{}\" y=\"{}\">{}</text>",
            fmt_coord(place.x + place.w / 2.0),
            fmt_coord(place.y + place.h / 2.0 - 4.0),
            esc(&label)
        );
        if let Some(sublabel) = &component.sublabel {
            let sub = fit_label(sublabel, LABEL_UNIT_BUDGET);
            let _ = write!(
                out,
                "<text class=\"node-sublabel\" x=\"{}\" y=\"{}\">{}</text>",
                fmt_coord(place.x + place.w / 2.0),
                fmt_coord(place.y + place.h / 2.0 + 18.0),
                esc(&sub)
            );
        }
        if let Some(tag) = &component.tag {
            let _ = write!(
                out,
                "<text class=\"node-tag\" x=\"{}\" y=\"{}\">{}</text>",
                fmt_coord(place.x + place.w / 2.0),
                fmt_coord(place.y + 18.0),
                esc(tag)
            );
        }
        out.push_str("</g>");
    }
    out.push_str("</g>");

    // Legend: one entry per component type actually present.
    out.push_str("<g class=\"legend\" data-role=\"legend\">");
    let mut legend_y = 24.0;
    for kind in [
        ComponentType::Interface,
        ComponentType::Service,
        ComponentType::Storage,
        ComponentType::Model,
    ] {
        if !doc.components.iter().any(|c| c.r#type == kind) {
            continue;
        }
        let _ = write!(
            out,
            "<g class=\"legend-entry legend-{}\"><rect x=\"16\" y=\"{}\" width=\"12\" height=\"12\" rx=\"3\"/><text x=\"34\" y=\"{}\">{}</text></g>",
            css_class(kind),
            fmt_coord(legend_y),
            fmt_coord(legend_y + 10.0),
            esc(kind_name(kind))
        );
        legend_y += 20.0;
    }
    out.push_str("</g>");

    out.push_str("</svg>");
    out
}

fn edge_variant_name(variant: EdgeVariant) -> &'static str {
    match variant {
        EdgeVariant::Primary => "primary",
        EdgeVariant::Async => "async",
        EdgeVariant::Error => "error",
    }
}

fn kind_name(kind: ComponentType) -> &'static str {
    match kind {
        ComponentType::Interface => "Interface (Controller)",
        ComponentType::Service => "Service",
        ComponentType::Storage => "Storage (Repository)",
        ComponentType::Model => "Model",
    }
}

fn edge_label(doc: &DiagramDocument, edge: &RoutedEdge) -> String {
    doc.connections
        .iter()
        .find(|c| c.from == edge.from && c.to == edge.to && c.variant == edge.variant)
        .map_or_else(String::new, |c| c.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::ir::{DiagramComponent, DiagramConnection, DiagramMeta};
    use crate::diagram::layout::{layout_components, Layout};
    use crate::diagram::route::route_edges;

    fn sample_doc() -> DiagramDocument {
        DiagramDocument {
            schema_version: 1,
            diagram_type: "architecture".to_string(),
            meta: DiagramMeta {
                title: "t <demo> & \"stuff\"".to_string(),
                subtitle: None,
                locale: "en".to_string(),
                repository: None,
            },
            components: vec![
                DiagramComponent {
                    id: "src-api".to_string(),
                    r#type: ComponentType::Interface,
                    label: "src/api <v1>".to_string(),
                    sublabel: Some("2 files".to_string()),
                    tag: Some("Controller".to_string()),
                    sources: vec![],
                },
                DiagramComponent {
                    id: "src-db".to_string(),
                    r#type: ComponentType::Storage,
                    label: "src/db".to_string(),
                    sublabel: None,
                    tag: None,
                    sources: vec![],
                },
            ],
            connections: vec![DiagramConnection {
                from: "src-api".to_string(),
                to: "src-db".to_string(),
                label: "HTTP".to_string(),
                variant: EdgeVariant::Async,
            }],
        }
    }

    fn render_sample() -> String {
        let doc = sample_doc();
        let layout = layout_components(&doc.components);
        let edges = route_edges(&doc, &layout);
        render_svg(&doc, &layout, &edges)
    }

    #[test]
    fn rendering_is_byte_identical_across_runs() {
        assert_eq!(render_sample(), render_sample());
    }

    #[test]
    fn output_has_single_root_and_semantic_attributes() {
        let svg = render_sample();
        assert_eq!(svg.matches("<svg").count(), 1, "exactly one svg root");
        assert_eq!(svg.matches("</svg>").count(), 1);
        assert!(svg.contains("data-node-id=\"src-api\""), "{svg}");
        assert!(svg.contains("data-edge-from=\"src-api\""), "{svg}");
        assert!(svg.contains("data-edge-to=\"src-db\""), "{svg}");
        assert!(svg.contains("data-variant=\"async\""), "{svg}");
        assert!(svg.contains("data-role=\"legend\""), "{svg}");
    }

    #[test]
    fn special_characters_are_escaped() {
        let svg = render_sample();
        assert!(svg.contains("src/api &lt;v1&gt;"), "{svg}");
        assert!(svg.contains("&amp;"), "{svg}");
        assert!(svg.contains("&quot;"), "{svg}");
        assert!(!svg.contains("<v1>"), "raw angle brackets leaked");
    }

    #[test]
    fn output_never_contains_nan_or_infinity() {
        let svg = render_sample();
        assert!(!svg.contains("NaN"), "{svg}");
        assert!(!svg.contains("Infinity"), "{svg}");
    }
}
