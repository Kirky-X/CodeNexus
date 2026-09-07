// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Deterministic grid layout.
//!
//! Rows follow the fixed [`ComponentType`] order (interface → service →
//! storage → model, empty rows compressed); columns follow component id
//! order inside each row. Identical components always produce an identical
//! [`Layout`].

use super::ir::DiagramComponent;
use super::text::text_units;

/// Grid cell width in px (upper bound for a component box).
pub const CELL_W: f64 = 260.0;
/// Grid cell height in px.
pub const CELL_H: f64 = 100.0;
/// Horizontal gap between cells.
pub const GAP_X: f64 = 60.0;
/// Vertical gap between rows.
pub const GAP_Y: f64 = 80.0;
/// Canvas margin around the grid.
pub const MARGIN: f64 = 40.0;
/// Reserved top strip hosting the legend (kept clear of the grid; the
/// quality gate `diagram/legend-clearance` enforces the same strip).
pub const LEGEND_STRIP_H: f64 = 132.0;
/// Minimum view box side, mirroring archify's floor (320×240).
const MIN_VIEW_W: f64 = 320.0;
const MIN_VIEW_H: f64 = 240.0;
/// Horizontal padding added around the measured label width.
const LABEL_PADDING_PX: f64 = 32.0;
/// Advance width in px of one measurement unit (archify's metric).
const PX_PER_UNIT: f64 = 8.0;
/// Lower bound for a component box width.
const MIN_BOX_W: f64 = 140.0;

/// A positioned component rectangle.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// Component id.
    pub id: String,
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Box width.
    pub w: f64,
    /// Box height.
    pub h: f64,
}

/// Computed geometry: placements plus the SVG view box.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// One placement per component, sorted by component id.
    pub placements: Vec<Placement>,
    /// `(x, y, width, height)` view box.
    pub view_box: (f64, f64, f64, f64),
}

/// Derives a component box width from measured label advance width,
/// clamped to `[MIN_BOX_W, CELL_W]`.
fn box_width(component: &DiagramComponent) -> f64 {
    let label_units = text_units(&component.label);
    let sublabel_units = component.sublabel.as_deref().map_or(0.0, text_units);
    let widest = label_units.max(sublabel_units);
    (widest * PX_PER_UNIT + LABEL_PADDING_PX).clamp(MIN_BOX_W, CELL_W)
}

/// Lays out `components` on the deterministic grid.
#[must_use]
pub fn layout_components(components: &[DiagramComponent]) -> Layout {
    // Bucket ids by component type, keeping the fixed row order.
    let mut rows: Vec<Vec<&DiagramComponent>> = vec![Vec::new(); 4];
    for component in components {
        rows[usize::from(component.r#type.row_order())].push(component);
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.id.cmp(&b.id));
    }
    // Compress empty rows so a diagram with only services doesn't start
    // three rows down.
    let rows: Vec<&Vec<&DiagramComponent>> = rows.iter().filter(|row| !row.is_empty()).collect();

    let mut placements = Vec::with_capacity(components.len());
    for (row_idx, row) in rows.iter().enumerate() {
        let y = MARGIN + LEGEND_STRIP_H + row_idx as f64 * (CELL_H + GAP_Y);
        for (col_idx, component) in row.iter().enumerate() {
            let x = MARGIN + col_idx as f64 * (CELL_W + GAP_X);
            placements.push(Placement {
                id: component.id.clone(),
                x,
                y,
                w: box_width(component),
                h: CELL_H,
            });
        }
    }
    placements.sort_by(|a, b| a.id.cmp(&b.id));

    let cols = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let used_rows = rows.len();
    let content_w = if cols == 0 {
        0.0
    } else {
        cols as f64 * CELL_W + (cols - 1) as f64 * GAP_X
    };
    let content_h = if used_rows == 0 {
        0.0
    } else {
        used_rows as f64 * CELL_H + (used_rows - 1) as f64 * GAP_Y
    };
    let view_box = (
        0.0,
        0.0,
        (content_w + 2.0 * MARGIN).max(MIN_VIEW_W),
        (LEGEND_STRIP_H + content_h + 2.0 * MARGIN).max(MIN_VIEW_H),
    );

    Layout {
        placements,
        view_box,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::ir::{ComponentType, DiagramComponent};

    fn component(id: &str, kind: ComponentType) -> DiagramComponent {
        DiagramComponent {
            id: id.to_string(),
            r#type: kind,
            label: id.to_string(),
            sublabel: Some("1 files".to_string()),
            tag: None,
            sources: vec![],
        }
    }

    #[test]
    fn empty_components_yield_minimum_view_box() {
        let layout = layout_components(&[]);
        assert!(layout.placements.is_empty());
        assert_eq!(layout.view_box, (0.0, 0.0, 320.0, 240.0));
    }

    #[test]
    fn identical_components_layout_identically() {
        let components = vec![
            component("src-b", ComponentType::Service),
            component("src-a", ComponentType::Interface),
        ];
        let l1 = layout_components(&components);
        let l2 = layout_components(&components);
        assert_eq!(l1, l2, "layout must be deterministic");
    }

    #[test]
    fn rows_follow_type_order_and_columns_follow_ids() {
        let components = vec![
            component("svc-b", ComponentType::Service),
            component("iface", ComponentType::Interface),
            component("svc-a", ComponentType::Service),
        ];
        let layout = layout_components(&components);
        let by_id = |id: &str| layout.placements.iter().find(|p| p.id == id).unwrap();
        let iface = by_id("iface");
        let svc_a = by_id("svc-a");
        let svc_b = by_id("svc-b");
        assert!(
            iface.y + CELL_H + GAP_Y <= svc_a.y,
            "interface row above service row"
        );
        assert_eq!(svc_a.y, svc_b.y, "same-type components share a row");
        assert!(
            svc_a.x + CELL_W + GAP_X <= svc_b.x,
            "columns follow id order (svc-a left of svc-b)"
        );
    }

    #[test]
    fn placements_never_overlap() {
        let components = vec![
            component("a", ComponentType::Interface),
            component("b", ComponentType::Service),
            component("c", ComponentType::Storage),
            component("d", ComponentType::Model),
            component("e", ComponentType::Service),
        ];
        let layout = layout_components(&components);
        for (i, p1) in layout.placements.iter().enumerate() {
            for p2 in &layout.placements[i + 1..] {
                let separated = p1.x + p1.w <= p2.x
                    || p2.x + p2.w <= p1.x
                    || p1.y + p1.h <= p2.y
                    || p2.y + p2.h <= p1.y;
                assert!(separated, "{p1:?} overlaps {p2:?}");
            }
        }
    }

    #[test]
    fn box_width_measures_labels_and_clamps() {
        let mut long = component("wide", ComponentType::Service);
        long.label = "x".repeat(100);
        assert_eq!(box_width(&long), CELL_W, "clamped to cell width");
        let mut cjk = component("cjk", ComponentType::Service);
        cjk.label = "中文".to_string(); // 4 units * 8px + 32px = 64 → min
        assert_eq!(box_width(&cjk), MIN_BOX_W, "clamped up to minimum");
    }
}
