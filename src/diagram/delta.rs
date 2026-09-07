// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Architecture semantic delta (Before / Delta / After + machine receipt).
//!
//! Ports archify's compare discipline: canonicalize both sides, hash the
//! canonical forms, and classify entity-level changes with a stable shape so
//! PR review can consume the receipt instead of eyeballing two diagrams.
//! Comparisons are IR-level (module architecture facts); symbol-level diffs
//! remain `detect_changes`' job.

use serde::{Deserialize, Serialize};

use super::ir::{DiagramComponent, DiagramDocument, EdgeVariant};
use super::quality::QualityProfile;
use super::render::{render, DiagramError};

use crate::diagnostics::HashInfo;

/// Version of the comparator contract (bump on receipt shape changes).
pub const COMPARATOR_VERSION: &str = "1";

/// Sorts components and connections so structurally identical documents are
/// byte-identical regardless of construction order.
#[must_use]
pub fn canonicalize(doc: &DiagramDocument) -> DiagramDocument {
    let mut clone = doc.clone();
    clone.components.sort_by(|a, b| a.id.cmp(&b.id));
    clone.connections.sort_by(|a, b| {
        (a.from.as_str(), a.to.as_str(), a.label.as_str(), a.variant).cmp(&(
            b.from.as_str(),
            b.to.as_str(),
            b.label.as_str(),
            b.variant,
        ))
    });
    clone
}

fn canonical_hash(doc: &DiagramDocument) -> HashInfo {
    let json = serde_json::to_string(&canonicalize(doc)).unwrap_or_default();
    HashInfo {
        algorithm: "blake3".to_string(),
        hash: blake3::hash(json.as_bytes()).to_string(),
        bytes: json.len() as u64,
    }
}

/// Whether a change alters topology (entities appearing/disappearing) or
/// only semantics of an existing entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Entity added or removed.
    Topology,
    /// Existing entity's attributes changed.
    Semantic,
}

/// Kind of entity-level change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Present in head only.
    Added,
    /// Present in base only.
    Removed,
    /// Present in both, attributes differ.
    Changed,
}

/// A component-level change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentChange {
    pub kind: ChangeKind,
    pub id: String,
    /// JSON Pointers of the differing fields (changed only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_fields: Vec<String>,
    pub classification: Classification,
}

/// A connection-level change, keyed by `(from, to)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionChange {
    pub kind: ChangeKind,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_fields: Vec<String>,
    pub classification: Classification,
}

/// Full entity-level diff between two architecture IRs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaReport {
    pub components: Vec<ComponentChange>,
    pub connections: Vec<ConnectionChange>,
}

impl DeltaReport {
    /// True when both sides are structurally and semantically identical.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.components.is_empty() && self.connections.is_empty()
    }

    /// Total number of changes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.components.len() + self.connections.len()
    }
}

fn component_field_diffs(base: &DiagramComponent, head: &DiagramComponent) -> Vec<String> {
    let mut fields = Vec::new();
    if base.label != head.label {
        fields.push("/label".to_string());
    }
    if base.sublabel != head.sublabel {
        fields.push("/sublabel".to_string());
    }
    if base.tag != head.tag {
        fields.push("/tag".to_string());
    }
    if base.r#type != head.r#type {
        fields.push("/type".to_string());
    }
    fields
}

fn connection_field_diffs(base: (&str, EdgeVariant), head: (&str, EdgeVariant)) -> Vec<String> {
    let mut fields = Vec::new();
    if base.0 != head.0 {
        fields.push("/label".to_string());
    }
    if base.1 != head.1 {
        fields.push("/variant".to_string());
    }
    fields
}

/// Entity-level diff between `base` and `head`.
#[must_use]
pub fn compare(base: &DiagramDocument, head: &DiagramDocument) -> DeltaReport {
    use std::collections::BTreeMap;

    let mut components = Vec::new();
    let base_by_id: BTreeMap<&str, &DiagramComponent> =
        base.components.iter().map(|c| (c.id.as_str(), c)).collect();
    let head_by_id: BTreeMap<&str, &DiagramComponent> =
        head.components.iter().map(|c| (c.id.as_str(), c)).collect();
    for (id, base_component) in &base_by_id {
        match head_by_id.get(id) {
            None => components.push(ComponentChange {
                kind: ChangeKind::Removed,
                id: (*id).to_string(),
                changed_fields: Vec::new(),
                classification: Classification::Topology,
            }),
            Some(head_component) => {
                let changed = component_field_diffs(base_component, head_component);
                if !changed.is_empty() {
                    components.push(ComponentChange {
                        kind: ChangeKind::Changed,
                        id: (*id).to_string(),
                        changed_fields: changed,
                        classification: Classification::Semantic,
                    });
                }
            }
        }
    }
    for id in head_by_id.keys() {
        if !base_by_id.contains_key(id) {
            components.push(ComponentChange {
                kind: ChangeKind::Added,
                id: (*id).to_string(),
                changed_fields: Vec::new(),
                classification: Classification::Topology,
            });
        }
    }
    components.sort_by(|a, b| a.id.cmp(&b.id));

    // Connections compare by (from, to); parallel connections collapse to
    // their most severe variant before diffing (mirrors IR construction).
    fn collapse(doc: &DiagramDocument) -> BTreeMap<(&str, &str), (&str, EdgeVariant)> {
        let mut map: BTreeMap<(&str, &str), (&str, EdgeVariant)> = BTreeMap::new();
        for conn in &doc.connections {
            let entry = (conn.label.as_str(), conn.variant);
            match map.get(&(conn.from.as_str(), conn.to.as_str())) {
                Some((_, existing)) if existing.severity() >= conn.variant.severity() => {}
                _ => {
                    map.insert((conn.from.as_str(), conn.to.as_str()), entry);
                }
            }
        }
        map
    }
    let base_conns = collapse(base);
    let head_conns = collapse(head);

    let mut connections = Vec::new();
    for (key, base_conn) in &base_conns {
        match head_conns.get(key) {
            None => connections.push(ConnectionChange {
                kind: ChangeKind::Removed,
                from: key.0.to_string(),
                to: key.1.to_string(),
                changed_fields: Vec::new(),
                classification: Classification::Topology,
            }),
            Some(head_conn) => {
                let changed = connection_field_diffs(*base_conn, *head_conn);
                if !changed.is_empty() {
                    connections.push(ConnectionChange {
                        kind: ChangeKind::Changed,
                        from: key.0.to_string(),
                        to: key.1.to_string(),
                        changed_fields: changed,
                        classification: Classification::Semantic,
                    });
                }
            }
        }
    }
    for key in head_conns.keys() {
        if !base_conns.contains_key(key) {
            connections.push(ConnectionChange {
                kind: ChangeKind::Added,
                from: key.0.to_string(),
                to: key.1.to_string(),
                changed_fields: Vec::new(),
                classification: Classification::Topology,
            });
        }
    }
    connections
        .sort_by(|a, b| (a.from.as_str(), a.to.as_str()).cmp(&(b.from.as_str(), b.to.as_str())));

    DeltaReport {
        components,
        connections,
    }
}

/// Machine-readable receipt written alongside the Delta HTML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaReceipt {
    pub comparator_version: String,
    pub base: HashInfo,
    pub head: HashInfo,
    pub changes: DeltaReport,
    /// Reserved for geometry-only change detection (v1 compares IR facts
    /// only, so this is always false).
    pub presentation_changed: bool,
}

/// Rendered delta artifact: HTML plus its receipt JSON.
pub struct DeltaRendered {
    pub html: String,
    pub receipt_json: String,
}

/// Renders the Before/Delta/After HTML and its machine receipt.
///
/// Before/After diagrams reuse the deterministic renderer (standard
/// profile); added/removed nodes are outlined client-side via the change
/// lists, so no geometry annotation is baked into the SVG.
///
/// # Errors
///
/// Propagates [`DiagramError`] when either side fails its quality gates.
pub fn render_delta(
    base: &DiagramDocument,
    head: &DiagramDocument,
    title: &str,
) -> Result<DeltaRendered, DiagramError> {
    let base_svg = render(base, QualityProfile::Standard, None)?.html;
    let head_svg = render(head, QualityProfile::Standard, None)?.html;
    // The full viewer pages are too heavy to inline; extract just the SVG.
    let before = extract_svg(&base_svg);
    let after = extract_svg(&head_svg);

    let report = compare(base, head);
    let receipt = DeltaReceipt {
        comparator_version: COMPARATOR_VERSION.to_string(),
        base: canonical_hash(base),
        head: canonical_hash(head),
        changes: report.clone(),
        presentation_changed: false,
    };
    let receipt_json = serde_json::to_string_pretty(&receipt).unwrap_or_default();

    let html = build_delta_html(&report, &receipt, before, after, title);
    Ok(DeltaRendered { html, receipt_json })
}

/// Extracts the `<svg>…</svg>` fragment from a rendered viewer page.
fn extract_svg(page: &str) -> &str {
    let start = page.find("<svg").unwrap_or(0);
    let end = page
        .rfind("</svg>")
        .map_or(page.len(), |pos| pos + "</svg>".len());
    &page[start..end]
}

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

fn build_delta_html(
    report: &DeltaReport,
    receipt: &DeltaReceipt,
    before_svg: &str,
    after_svg: &str,
    title: &str,
) -> String {
    let mut html = String::with_capacity(32 * 1024);
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    html.push_str("<title>");
    html.push_str(&esc(title));
    html.push_str(" — architecture delta</title><style>");
    html.push_str(
        "body{margin:0;font:14px/1.5 -apple-system,'Segoe UI','Noto Sans SC',sans-serif;",
    );
    html.push_str("background:#101014;color:#e8e8ee}");
    html.push_str("h1{font-size:18px;padding:14px 20px;margin:0}");
    html.push_str("h2{font-size:15px;margin:0;padding:10px 20px;border-top:1px solid #33333f}");
    html.push_str(".grid{display:grid;grid-template-columns:1fr 1fr;gap:8px;padding:0 12px}");
    html.push_str(
        ".panel{background:#1a1a22;border:1px solid #33333f;border-radius:8px;overflow:auto}",
    );
    html.push_str("svg{width:100%;height:auto;display:block}");
    html.push_str("svg .node rect{fill:#22222c;stroke:#33333f;stroke-width:1.5}");
    html.push_str("svg text{fill:#e8e8ee;font-size:13px;text-anchor:middle;font-family:inherit}");
    html.push_str("svg .edge polyline{fill:none;stroke:#5a5a72;stroke-width:1.6}");
    html.push_str("svg .edge-async polyline{stroke:#b48ead;stroke-dasharray:6 4}");
    html.push_str("svg .edge-error polyline{stroke:#e06c75;stroke-width:2}");
    html.push_str("svg .node.added rect{stroke:#9ece6a;stroke-width:3}");
    html.push_str("svg .node.removed rect{stroke:#e06c75;stroke-width:3}");
    html.push_str("ul.changes{list-style:none;padding:8px 20px;margin:0}");
    html.push_str("li.change{padding:4px 0;border-bottom:1px solid #22222c}");
    html.push_str(".badge{display:inline-block;border:1px solid #33333f;border-radius:4px;");
    html.push_str("padding:0 6px;font-size:11px;color:#9a9aad;margin-right:8px}");
    html.push_str(".add{color:#9ece6a}.rem{color:#e06c75}.chg{color:#e0af68}");
    html.push_str(".empty{padding:8px 20px;color:#9a9aad}");
    html.push_str("</style></head><body>");
    html.push_str("<h1>");
    html.push_str(&esc(title));
    html.push_str(" — architecture delta</h1>");

    // Delta section (change list) first: reviewers read the summary, then
    // the annotated diagrams.
    html.push_str("<h2>Delta</h2>");
    if report.is_empty() {
        html.push_str("<p class=\"empty\">No architecture changes.</p>");
    } else {
        html.push_str("<ul class=\"changes\">");
        for change in &report.components {
            let (class, label) = match change.kind {
                ChangeKind::Added => ("add", "added"),
                ChangeKind::Removed => ("rem", "removed"),
                ChangeKind::Changed => ("chg", "changed"),
            };
            html.push_str(&format!(
                "<li class=\"change\"><span class=\"badge\">component</span><span class=\"badge\">{}</span><span class=\"{class}\">{}</span> {}",
                change.classification_name(),
                label,
                esc(&change.id)
            ));
            if !change.changed_fields.is_empty() {
                html.push_str(&format!(
                    " <span class=\"badge\">{}</span>",
                    esc(&change.changed_fields.join(", "))
                ));
            }
            html.push_str("</li>");
        }
        for change in &report.connections {
            let (class, label) = match change.kind {
                ChangeKind::Added => ("add", "added"),
                ChangeKind::Removed => ("rem", "removed"),
                ChangeKind::Changed => ("chg", "changed"),
            };
            html.push_str(&format!(
                "<li class=\"change\"><span class=\"badge\">connection</span><span class=\"badge\">{}</span><span class=\"{class}\">{}</span> {} → {}",
                change.classification_name(),
                label,
                esc(&change.from),
                esc(&change.to)
            ));
            if !change.changed_fields.is_empty() {
                html.push_str(&format!(
                    " <span class=\"badge\">{}</span>",
                    esc(&change.changed_fields.join(", "))
                ));
            }
            html.push_str("</li>");
        }
        html.push_str("</ul>");
    }

    html.push_str("<h2>Before</h2><div class=\"grid\"><div class=\"panel\">");
    html.push_str(before_svg);
    html.push_str("</div></div>");
    html.push_str("<h2>After</h2><div class=\"grid\"><div class=\"panel\">");
    html.push_str(after_svg);
    html.push_str("</div></div>");

    html.push_str("<h2>Receipt</h2><pre style=\"padding:0 20px;overflow:auto\">");
    html.push_str(&esc(
        &serde_json::to_string_pretty(receipt).unwrap_or_default()
    ));
    html.push_str("</pre>");
    html.push_str("<p style=\"padding:8px 20px;color:#9a9aad\">Edges reflect static index facts (CALLS / HTTP), not a runtime call graph.</p>");

    // Annotation script: outline added/removed node ids in the diagrams.
    let added: Vec<&str> = report
        .components
        .iter()
        .filter(|c| c.kind == ChangeKind::Added)
        .map(|c| c.id.as_str())
        .collect();
    let removed: Vec<&str> = report
        .components
        .iter()
        .filter(|c| c.kind == ChangeKind::Removed)
        .map(|c| c.id.as_str())
        .collect();
    html.push_str("<script>(function(){");
    html.push_str(&format!(
        "var added={};var removed={};",
        serde_json::to_string(&added).unwrap_or_default(),
        serde_json::to_string(&removed).unwrap_or_default()
    ));
    html.push_str("document.querySelectorAll('svg').forEach(function(svg,i){");
    html.push_str("var ids=i===0?removed:added;");
    html.push_str("ids.forEach(function(id){var g=svg.querySelector('[data-node-id=\"'+id+'\"]');");
    html.push_str("if(g)g.classList.add(i===0?'removed':'added');});});})();</script>");
    html.push_str("</body></html>");

    html
}

impl ComponentChange {
    fn classification_name(&self) -> &'static str {
        match self.classification {
            Classification::Topology => "topology",
            Classification::Semantic => "semantic",
        }
    }
}

impl ConnectionChange {
    fn classification_name(&self) -> &'static str {
        match self.classification {
            Classification::Topology => "topology",
            Classification::Semantic => "semantic",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::ir::{ComponentType, DiagramConnection, DiagramMeta};

    fn component(id: &str, kind: ComponentType, sublabel: &str) -> DiagramComponent {
        DiagramComponent {
            id: id.to_string(),
            r#type: kind,
            label: id.to_string(),
            sublabel: Some(sublabel.to_string()),
            tag: None,
            sources: vec![],
        }
    }

    fn doc(
        components: Vec<DiagramComponent>,
        connections: Vec<DiagramConnection>,
    ) -> DiagramDocument {
        DiagramDocument {
            schema_version: 1,
            diagram_type: "architecture".to_string(),
            meta: DiagramMeta {
                title: "t".to_string(),
                subtitle: None,
                locale: "en".to_string(),
                repository: None,
            },
            components,
            connections,
        }
    }

    #[test]
    fn canonicalize_makes_order_irrelevant() {
        let mut a = doc(
            vec![
                component("b", ComponentType::Service, "1 files"),
                component("a", ComponentType::Service, "1 files"),
            ],
            vec![],
        );
        a.components.reverse();
        let b = doc(
            vec![
                component("a", ComponentType::Service, "1 files"),
                component("b", ComponentType::Service, "1 files"),
            ],
            vec![],
        );
        let ja = serde_json::to_string(&canonicalize(&a)).unwrap();
        let jb = serde_json::to_string(&canonicalize(&b)).unwrap();
        assert_eq!(ja, jb);
        // Canonicalize never mutates values.
        assert_eq!(canonicalize(&b).components[0].id, "a");
    }

    #[test]
    fn compare_classifies_added_removed_changed() {
        let base = doc(
            vec![
                component("keep", ComponentType::Service, "1 files"),
                component("drop", ComponentType::Storage, "2 files"),
            ],
            vec![DiagramConnection {
                from: "keep".to_string(),
                to: "drop".to_string(),
                label: String::new(),
                variant: EdgeVariant::Primary,
            }],
        );
        let mut head = doc(
            vec![
                component("keep", ComponentType::Service, "3 files"),
                component("new", ComponentType::Interface, "1 files"),
            ],
            vec![DiagramConnection {
                from: "keep".to_string(),
                to: "new".to_string(),
                label: String::new(),
                variant: EdgeVariant::Primary,
            }],
        );
        head.components[0].tag = Some("Service".to_string());

        let report = compare(&base, &head);
        let component_ids: Vec<(&str, ChangeKind)> = report
            .components
            .iter()
            .map(|c| (c.id.as_str(), c.kind))
            .collect();
        assert!(component_ids.contains(&("new", ChangeKind::Added)));
        assert!(component_ids.contains(&("drop", ChangeKind::Removed)));
        let changed = report
            .components
            .iter()
            .find(|c| c.id == "keep")
            .expect("changed component");
        assert_eq!(changed.kind, ChangeKind::Changed);
        assert_eq!(changed.classification, Classification::Semantic);
        assert!(changed.changed_fields.contains(&"/sublabel".to_string()));
        assert!(changed.changed_fields.contains(&"/tag".to_string()));
        assert_eq!(report.connections.len(), 2, "one added one removed");
        assert!(report
            .connections
            .iter()
            .all(|c| c.classification == Classification::Topology));
    }

    #[test]
    fn compare_self_yields_empty_report() {
        let document = doc(
            vec![component("a", ComponentType::Service, "1 files")],
            vec![],
        );
        let report = compare(&document, &document);
        assert!(report.is_empty());
        assert_eq!(report.len(), 0);
    }

    #[test]
    fn render_delta_emits_three_sections_and_consistent_receipt() {
        let base = doc(
            vec![component("a", ComponentType::Service, "1 files")],
            vec![],
        );
        let head = doc(
            vec![
                component("a", ComponentType::Service, "1 files"),
                component("b", ComponentType::Storage, "2 files"),
            ],
            vec![],
        );
        let rendered = render_delta(&base, &head, "PR review").expect("render_delta");
        for marker in ["Delta", "Before", "After", "data-node-id=\"b\""] {
            assert!(rendered.html.contains(marker), "missing {marker}");
        }
        let receipt: DeltaReceipt = serde_json::from_str(&rendered.receipt_json).unwrap();
        assert_eq!(receipt.comparator_version, "1");
        assert_eq!(receipt.changes.components.len(), 1, "one added component");
        assert_eq!(receipt.changes.components[0].kind, ChangeKind::Added);
        assert!(!receipt.presentation_changed);
        assert_ne!(receipt.base.hash, receipt.head.hash);
    }
}
