// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use serde::Serialize;

use crate::diagram::delta::{ChangeKind, DeltaReport};
use crate::diagram::ir::{ComponentType, DiagramDocument};

/// snake_case group name matching ComponentType's serde rename_all.
fn component_type_group(t: ComponentType) -> &'static str {
    match t {
        ComponentType::Interface => "interface",
        ComponentType::Service => "service",
        ComponentType::Storage => "storage",
        ComponentType::Model => "model",
    }
}

/// Snapshot schema version.
pub const SNAPSHOT_VERSION: u8 = 1;

/// Entity-level change state carried into the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    /// No delta information or unchanged.
    None,
    /// Present in head only.
    Added,
    /// Present in base only.
    Removed,
    /// Present in both, attributes differ.
    Changed,
}

/// Viewer snapshot graph node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewerNode {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
    /// Layer tag or component type (used for viewer grouping/colors).
    pub group: String,
    pub change: ChangeState,
}

/// Viewer snapshot graph edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewerEdge {
    pub source: String,
    pub target: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub change: ChangeState,
}

/// Engine-agnostic graph snapshot for graph-viewer's snapshot mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewerSnapshot {
    pub version: u8,
    pub nodes: Vec<ViewerNode>,
    pub edges: Vec<ViewerEdge>,
}

impl ChangeState {
    fn from_kind(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Added => ChangeState::Added,
            ChangeKind::Removed => ChangeState::Removed,
            ChangeKind::Changed => ChangeState::Changed,
        }
    }
}

/// Converts a diagram document (plus optional delta) into a viewer snapshot.
///
/// Without a delta every entity is [`ChangeState::None`]; with one, matching
/// component ids / connection `(from, to)` pairs adopt the delta's change
/// kind. Entity ids are unique by IR construction (sorted, deduplicated).
#[must_use]
pub fn to_viewer_snapshot(doc: &DiagramDocument, delta: Option<&DeltaReport>) -> ViewerSnapshot {
    let component_changes: std::collections::BTreeMap<&str, ChangeKind> = delta
        .map(|d| {
            d.components
                .iter()
                .map(|c| (c.id.as_str(), c.kind))
                .collect()
        })
        .unwrap_or_default();
    let connection_changes: std::collections::BTreeMap<(&str, &str), ChangeKind> = delta
        .map(|d| {
            d.connections
                .iter()
                .map(|c| (c.from.as_str(), c.to.as_str(), c.kind))
                .map(|(from, to, kind)| ((from, to), kind))
                .collect()
        })
        .unwrap_or_default();

    let nodes = doc
        .components
        .iter()
        .map(|c| {
            let change = component_changes
                .get(c.id.as_str())
                .copied()
                .map(ChangeState::from_kind)
                .unwrap_or(ChangeState::None);
            ViewerNode {
                id: c.id.clone(),
                label: c.label.clone(),
                sublabel: c.sublabel.clone(),
                group: c
                    .tag
                    .clone()
                    .unwrap_or_else(|| component_type_group(c.r#type).to_string()),
                change,
            }
        })
        .collect();
    let edges = doc
        .connections
        .iter()
        .map(|conn| {
            let change = connection_changes
                .get(&(conn.from.as_str(), conn.to.as_str()))
                .copied()
                .map(ChangeState::from_kind)
                .unwrap_or(ChangeState::None);
            ViewerEdge {
                source: conn.from.clone(),
                target: conn.to.clone(),
                label: conn.label.clone(),
                change,
            }
        })
        .collect();
    ViewerSnapshot {
        version: SNAPSHOT_VERSION,
        nodes,
        edges,
    }
}

/// Percent-encodes `raw` for use in a URL query value (RFC 3986 unreserved
/// set kept literal, everything else escaped as uppercase `%XX`). Deliberately
/// dependency-free: the payload is a single snapshot JSON value.
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Builds the `<iframe>` stage markup that replaces the SVG pane in the
/// diagram template: the 3D viewer loads the snapshot from a
/// `data:application/json` query value (same document, no extra request).
#[must_use]
pub fn viewer_stage_iframe(viewer_url: &str, snapshot: &ViewerSnapshot) -> String {
    let json = serde_json::to_string(snapshot).unwrap_or_else(|_| "{}".to_string());
    let encoded = percent_encode(&json);
    format!(
        "<iframe id=\"cnx-viewer-frame\" \
         src=\"{viewer_url}/?snapshot=data:application/json;charset=utf-8,{encoded}\" \
         style=\"width:100%;height:100%;border:0\" \
         title=\"CodeNexus 3D graph\"></iframe>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::delta::{Classification, ComponentChange, ConnectionChange};
    use crate::diagram::ir::{
        ComponentType, DiagramComponent, DiagramConnection, DiagramDocument, DiagramMeta,
        EdgeVariant,
    };

    fn doc() -> DiagramDocument {
        DiagramDocument {
            schema_version: 1,
            diagram_type: "architecture".to_string(),
            meta: DiagramMeta {
                title: "test".to_string(),
                subtitle: None,
                locale: "en".to_string(),
                repository: None,
            },
            components: vec![
                DiagramComponent {
                    id: "src-api".to_string(),
                    r#type: ComponentType::Interface,
                    label: "/src/api".to_string(),
                    sublabel: Some("3 files".to_string()),
                    tag: Some("Controller".to_string()),
                    sources: Vec::new(),
                },
                DiagramComponent {
                    id: "src-db".to_string(),
                    r#type: ComponentType::Storage,
                    label: "/src/db".to_string(),
                    sublabel: None,
                    tag: None,
                    sources: Vec::new(),
                },
            ],
            connections: vec![DiagramConnection {
                from: "src-api".to_string(),
                to: "src-db".to_string(),
                label: String::new(),
                variant: EdgeVariant::Primary,
            }],
        }
    }

    #[test]
    fn no_delta_yields_all_none_changes() {
        let snapshot = to_viewer_snapshot(&doc(), None);
        assert_eq!(snapshot.version, 1);
        assert_eq!(snapshot.nodes.len(), 2);
        assert_eq!(snapshot.edges.len(), 1);
        assert!(snapshot.nodes.iter().all(|n| n.change == ChangeState::None));
        assert!(snapshot.edges.iter().all(|e| e.change == ChangeState::None));
        // Group falls back to the component type when no tag is present.
        let db = snapshot.nodes.iter().find(|n| n.id == "src-db").unwrap();
        assert_eq!(db.group, "storage");
        let api = snapshot.nodes.iter().find(|n| n.id == "src-api").unwrap();
        assert_eq!(api.group, "Controller");
    }

    #[test]
    fn delta_kinds_map_onto_nodes_and_edges() {
        let delta = DeltaReport {
            components: vec![
                ComponentChange {
                    kind: ChangeKind::Added,
                    id: "src-api".to_string(),
                    changed_fields: Vec::new(),
                    classification: Classification::Topology,
                },
                ComponentChange {
                    kind: ChangeKind::Removed,
                    id: "src-db".to_string(),
                    changed_fields: Vec::new(),
                    classification: Classification::Topology,
                },
            ],
            connections: vec![ConnectionChange {
                kind: ChangeKind::Changed,
                from: "src-api".to_string(),
                to: "src-db".to_string(),
                changed_fields: vec!["/variant".to_string()],
                classification: Classification::Semantic,
            }],
        };
        let snapshot = to_viewer_snapshot(&doc(), Some(&delta));
        let api = snapshot.nodes.iter().find(|n| n.id == "src-api").unwrap();
        let db = snapshot.nodes.iter().find(|n| n.id == "src-db").unwrap();
        assert_eq!(api.change, ChangeState::Added);
        assert_eq!(db.change, ChangeState::Removed);
        assert_eq!(snapshot.edges[0].change, ChangeState::Changed);
    }

    #[test]
    fn snapshot_json_is_valid_and_ids_unique() {
        let snapshot = to_viewer_snapshot(&doc(), None);
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["version"], 1);
        let ids: Vec<&str> = snapshot.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "ids must be unique");
    }
}
