// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Orthogonal edge routing with automatic port spread.
//!
//! Ports sit on the four box sides; edges pick sides from the relative
//! position of their endpoints, ports on a contested side spread evenly, and
//! every polyline is strictly orthogonal (each segment horizontal or
//! vertical). Reverse pairs (A→B next to B→A) shift their mid-channel so the
//! two runs stay visually distinct, and cycle (`Error`) edges detour through
//! a channel below the boxes — mirroring archify's outside-bridge channel.

use std::collections::BTreeMap;

use super::ir::{DiagramDocument, EdgeVariant};
use super::layout::Placement;

/// Stub length leaving/entering a port.
pub const STUB: f64 = 8.0;
/// Vertical clearance of the cycle channel below the boxes.
pub const CHANNEL_CLEARANCE: f64 = 24.0;
/// Extra per-edge offset so multiple channels don't stack.
const CHANNEL_STEP: f64 = 12.0;
/// Mid-channel shift applied to the second edge of a reverse pair.
const REVERSE_SHIFT: f64 = 16.0;

/// A routed edge: orthogonal polyline plus label anchor.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedEdge {
    /// Source component id.
    pub from: String,
    /// Target component id.
    pub to: String,
    /// Visual semantics carried over from the IR.
    pub variant: EdgeVariant,
    /// Polyline points, guaranteed finite and orthogonal.
    pub points: Vec<(f64, f64)>,
    /// Label anchor (middle of the longest segment).
    pub label_anchor: (f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

fn port_point(placement: &Placement, side: Side, fraction: f64) -> (f64, f64) {
    match side {
        Side::Top => (placement.x + placement.w * fraction, placement.y),
        Side::Bottom => (
            placement.x + placement.w * fraction,
            placement.y + placement.h,
        ),
        Side::Left => (placement.x, placement.y + placement.h * fraction),
        Side::Right => (
            placement.x + placement.w,
            placement.y + placement.h * fraction,
        ),
    }
}

fn pick_sides(a: &Placement, b: &Placement) -> (Side, Side) {
    let dx = (b.x + b.w / 2.0) - (a.x + a.w / 2.0);
    let dy = (b.y + b.h / 2.0) - (a.y + a.h / 2.0);
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            (Side::Right, Side::Left)
        } else {
            (Side::Left, Side::Right)
        }
    } else if dy >= 0.0 {
        (Side::Bottom, Side::Top)
    } else {
        (Side::Top, Side::Bottom)
    }
}

/// Routes every connection in `doc` against `layout`.
///
/// Input order is the document's (already deterministic); port fractions are
/// assigned per `(node, side)` in first-seen order, so output is stable.
#[must_use]
pub fn route_edges(doc: &DiagramDocument, layout: &super::layout::Layout) -> Vec<RoutedEdge> {
    let by_id: BTreeMap<&str, &Placement> = layout
        .placements
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();

    // Pass 1: count port demand per (node, side) and detect reverse pairs.
    let mut demand: BTreeMap<(&str, Side), usize> = BTreeMap::new();
    let mut pair_sides: Vec<(Side, Side)> = Vec::new();
    let mut unordered_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for conn in &doc.connections {
        let (Some(a), Some(b)) = (by_id.get(conn.from.as_str()), by_id.get(conn.to.as_str()))
        else {
            continue;
        };
        let (from_side, to_side) = pick_sides(a, b);
        *demand.entry((conn.from.as_str(), from_side)).or_insert(0) += 1;
        *demand.entry((conn.to.as_str(), to_side)).or_insert(0) += 1;
        pair_sides.push((from_side, to_side));
        let mut key = (conn.from.clone(), conn.to.clone());
        if key.0 > key.1 {
            std::mem::swap(&mut key.0, &mut key.1);
        }
        *unordered_counts.entry(key).or_insert(0) += 1;
    }

    // Pass 2: route.
    let mut used_ports: BTreeMap<(&str, Side), usize> = BTreeMap::new();
    let mut error_channel_index: usize = 0;
    let mut routed_index: usize = 0;
    let mut edges = Vec::new();
    for conn in &doc.connections {
        let (Some(a), Some(b)) = (by_id.get(conn.from.as_str()), by_id.get(conn.to.as_str()))
        else {
            continue;
        };
        let (from_side, to_side) = pair_sides[routed_index];
        routed_index += 1;

        let from_idx = used_ports
            .get(&(conn.from.as_str(), from_side))
            .copied()
            .unwrap_or(0);
        let to_idx = used_ports
            .get(&(conn.to.as_str(), to_side))
            .copied()
            .unwrap_or(0);
        *used_ports
            .entry((conn.from.as_str(), from_side))
            .or_insert(0) += 1;
        *used_ports.entry((conn.to.as_str(), to_side)).or_insert(0) += 1;
        let from_frac = (from_idx as f64 + 1.0)
            / (demand
                .get(&(conn.from.as_str(), from_side))
                .copied()
                .unwrap_or(1) as f64
                + 1.0);
        let to_frac = (to_idx as f64 + 1.0)
            / (demand
                .get(&(conn.to.as_str(), to_side))
                .copied()
                .unwrap_or(1) as f64
                + 1.0);

        let mut key = (conn.from.clone(), conn.to.clone());
        if key.0 > key.1 {
            std::mem::swap(&mut key.0, &mut key.1);
        }
        let is_reverse_second = unordered_counts.get(&key).copied().unwrap_or(1) > 1
            && from_side != to_side
            && (from_side == Side::Left || from_side == Side::Right);

        let points = if conn.variant == EdgeVariant::Error {
            let channel = (a.y + a.h).max(b.y + b.h)
                + CHANNEL_CLEARANCE
                + error_channel_index as f64 * CHANNEL_STEP;
            error_channel_index += 1;
            let p0 = port_point(a, Side::Bottom, from_frac);
            let p3 = port_point(b, Side::Bottom, to_frac);
            if p0.0 == p3.0 {
                // Straight vertical run: the horizontal channel leg would be
                // zero-length, so drop it.
                vec![p0, (p0.0, channel), p3]
            } else {
                vec![p0, (p0.0, channel), (p3.0, channel), p3]
            }
        } else {
            match (from_side, to_side) {
                (Side::Right, Side::Left) | (Side::Left, Side::Right) => {
                    let (p0, p3) = (
                        port_point(a, from_side, from_frac),
                        port_point(b, to_side, to_frac),
                    );
                    let mut mid = (p0.0 + p3.0) / 2.0;
                    if is_reverse_second {
                        mid += if from_side == Side::Right {
                            REVERSE_SHIFT
                        } else {
                            -REVERSE_SHIFT
                        };
                    }
                    // Same-row runs have no vertical span: skip the degenerate
                    // middle point so no zero-length segment is emitted.
                    if p0.1 == p3.1 {
                        vec![p0, (mid, p0.1), p3]
                    } else {
                        vec![p0, (mid, p0.1), (mid, p3.1), p3]
                    }
                }
                (Side::Bottom, Side::Top) | (Side::Top, Side::Bottom) => {
                    let (p0, p3) = (
                        port_point(a, from_side, from_frac),
                        port_point(b, to_side, to_frac),
                    );
                    let mut mid = (p0.1 + p3.1) / 2.0;
                    if is_reverse_second {
                        mid += REVERSE_SHIFT;
                    }
                    if p0.0 == p3.0 {
                        vec![p0, (p0.0, mid), p3]
                    } else {
                        vec![p0, (p0.0, mid), (p3.0, mid), p3]
                    }
                }
                // Same-side degenerate pair: loop through a channel below.
                _ => {
                    let channel = (a.y + a.h).max(b.y + b.h)
                        + CHANNEL_CLEARANCE
                        + error_channel_index as f64 * CHANNEL_STEP;
                    error_channel_index += 1;
                    let p0 = port_point(a, from_side, from_frac);
                    let p3 = port_point(b, to_side, to_frac);
                    if p0.0 == p3.0 {
                        vec![p0, (p0.0, channel), p3]
                    } else {
                        vec![p0, (p0.0, channel), (p3.0, channel), p3]
                    }
                }
            }
        };

        let label_anchor = middle_anchor(&points);
        edges.push(RoutedEdge {
            from: conn.from.clone(),
            to: conn.to.clone(),
            variant: conn.variant,
            points,
            label_anchor,
        });
    }
    edges
}

/// Anchor at the middle of the longest segment (best place for an edge label).
fn middle_anchor(points: &[(f64, f64)]) -> (f64, f64) {
    if points.len() < 2 {
        return points.first().copied().unwrap_or((0.0, 0.0));
    }
    let mut longest: Option<[(f64, f64); 2]> = None;
    let mut longest_len = 0.0_f64;
    for window in points.windows(2) {
        let len = (window[1].0 - window[0].0).hypot(window[1].1 - window[0].1);
        if len >= longest_len {
            longest_len = len;
            longest = Some([window[0], window[1]]);
        }
    }
    match longest {
        Some([p0, p1]) => ((p0.0 + p1.0) / 2.0, (p0.1 + p1.1) / 2.0),
        None => points[0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::ir::{ComponentType, DiagramComponent, DiagramConnection};
    use crate::diagram::layout::{layout_components, Layout};

    fn setup(
        components: Vec<DiagramComponent>,
        conns: Vec<(&str, &str, EdgeVariant)>,
    ) -> (DiagramDocument, Layout) {
        let document = DiagramDocument {
            schema_version: 1,
            diagram_type: "architecture".to_string(),
            meta: crate::diagram::ir::DiagramMeta {
                title: "t".to_string(),
                subtitle: None,
                locale: "en".to_string(),
                repository: None,
            },
            components,
            connections: conns
                .into_iter()
                .map(|(from, to, variant)| DiagramConnection {
                    from: from.to_string(),
                    to: to.to_string(),
                    label: String::new(),
                    variant,
                })
                .collect(),
        };
        let layout = layout_components(&document.components);
        (document, layout)
    }

    fn component(id: &str, kind: ComponentType) -> DiagramComponent {
        DiagramComponent {
            id: id.to_string(),
            r#type: kind,
            label: id.to_string(),
            sublabel: None,
            tag: None,
            sources: vec![],
        }
    }

    #[test]
    fn every_segment_is_strictly_orthogonal_and_finite() {
        let (doc, layout) = setup(
            vec![
                component("a", ComponentType::Interface),
                component("b", ComponentType::Service),
                component("c", ComponentType::Storage),
            ],
            vec![
                ("a", "b", EdgeVariant::Primary),
                ("a", "c", EdgeVariant::Primary),
                ("b", "c", EdgeVariant::Async),
            ],
        );
        for edge in route_edges(&doc, &layout) {
            for pair in edge.points.windows(2) {
                assert!(
                    pair[0].0 == pair[1].0 || pair[0].1 == pair[1].1,
                    "non-orthogonal segment {pair:?} in {edge:?}"
                );
                assert!(pair[0].0.is_finite() && pair[0].1.is_finite());
            }
        }
    }

    #[test]
    fn reverse_pair_channels_do_not_coincide() {
        let (doc, layout) = setup(
            vec![
                component("a", ComponentType::Service),
                component("b", ComponentType::Service),
            ],
            vec![
                ("a", "b", EdgeVariant::Primary),
                ("b", "a", EdgeVariant::Primary),
            ],
        );
        let edges = route_edges(&doc, &layout);
        assert_eq!(edges.len(), 2);
        // The mid-channel x of the two runs must differ.
        let mid_x = |e: &RoutedEdge| e.points[1].0;
        assert!(
            (mid_x(&edges[0]) - mid_x(&edges[1])).abs() > 1.0,
            "reverse pair shares a channel: {} vs {}",
            mid_x(&edges[0]),
            mid_x(&edges[1])
        );
    }

    #[test]
    fn error_variant_detours_below_the_boxes() {
        let (doc, layout) = setup(
            vec![
                component("a", ComponentType::Service),
                component("b", ComponentType::Service),
            ],
            vec![("a", "b", EdgeVariant::Error)],
        );
        let edge = &route_edges(&doc, &layout)[0];
        let max_box_bottom = layout
            .placements
            .iter()
            .map(|p| p.y + p.h)
            .fold(0.0_f64, f64::max);
        let channel_y = edge.points[1].1;
        assert!(
            channel_y > max_box_bottom,
            "cycle channel {channel_y} must clear box bottom {max_box_bottom}"
        );
        assert_eq!(edge.label_anchor.1, channel_y, "label rides the channel");
    }

    #[test]
    fn ports_spread_when_one_side_is_contested() {
        let (doc, layout) = setup(
            vec![
                component("hub", ComponentType::Service),
                component("x1", ComponentType::Service),
                component("x2", ComponentType::Service),
            ],
            vec![
                ("x1", "hub", EdgeVariant::Primary),
                ("x2", "hub", EdgeVariant::Primary),
            ],
        );
        let edges = route_edges(&doc, &layout);
        let entry_y = |e: &RoutedEdge| e.points[3].1;
        assert!(
            (entry_y(&edges[0]) - entry_y(&edges[1])).abs() > 1.0,
            "both edges entered hub at the same port"
        );
    }
}
