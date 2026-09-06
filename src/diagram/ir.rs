// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Typed JSON IR for architecture diagrams.
//!
//! Field names align with archify's architecture schema subset so the IR
//! stays interoperable, while the component-type vocabulary is CodeNexus's
//! own: [`ComponentType`] values are derived from real layer facts
//! ([`LayerInfo`]), never from name heuristics pretending to know a module
//! is "a database".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::analysis::architecture::ArchitectureOverview;

/// IR schema version. Bump only on breaking IR shape changes.
pub const SCHEMA_VERSION: u8 = 1;

/// Closed presentation vocabulary for component kinds.
///
/// Derived from the layer classification already produced by
/// `ArchitectureAnalyzer` (Controller/Service/Repository/Model), so every
/// classification states its graph evidence instead of guessing from names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentType {
    /// Module whose members handle routes (Controller layer).
    Interface,
    /// Module whose members implement business logic (Service layer).
    Service,
    /// Module whose members persist or query data (Repository layer).
    Storage,
    /// Module whose members define data shapes (Model layer).
    Model,
}

impl ComponentType {
    /// Fixed layout row order: entry points first, data shapes last.
    #[must_use]
    pub fn row_order(self) -> u8 {
        match self {
            ComponentType::Interface => 0,
            ComponentType::Service => 1,
            ComponentType::Storage => 2,
            ComponentType::Model => 3,
        }
    }

    fn from_layer(layer: &str) -> Self {
        match layer {
            "Controller" => ComponentType::Interface,
            "Repository" => ComponentType::Storage,
            "Model" => ComponentType::Model,
            // "Service" and anything unrecognized default to Service.
            _ => ComponentType::Service,
        }
    }
}

/// Closed visual vocabulary for connection semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeVariant {
    /// Static CALLS dependency between modules.
    Primary,
    /// Cross-service link (HTTP/gRPC/MQ/event bus) detected from route and
    /// fetch facts.
    Async,
    /// Dependency participating in a cycle.
    Error,
}

impl EdgeVariant {
    /// Severity used when deduplicating parallel connections: the most
    /// alarming fact wins (`Error > Async > Primary`).
    #[must_use]
    pub fn severity(self) -> u8 {
        match self {
            EdgeVariant::Primary => 0,
            EdgeVariant::Async => 1,
            EdgeVariant::Error => 2,
        }
    }
}

/// A git-verifiable source reference attached to a component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// Repo-relative POSIX path of the source file.
    pub path: String,
    /// Optional start line (1-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Optional end line (inclusive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// Optional short label describing the reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One rendered node: an indexed module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramComponent {
    /// Stable id (module path slug).
    pub id: String,
    /// Presentation kind derived from the dominant layer.
    pub r#type: ComponentType,
    /// Display label (module path).
    pub label: String,
    /// Secondary line (member file count).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
    /// Dominant layer name, verbatim from the layer analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Up to 3 git-verifiable source files.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceRef>,
}

/// One rendered edge between two components.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramConnection {
    /// Source component id.
    pub from: String,
    /// Target component id.
    pub to: String,
    /// Edge annotation (protocol for async links).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Visual semantics.
    pub variant: EdgeVariant,
}

/// Repository coordinates for source-evidence verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRef {
    /// Repository URL (e.g. `https://github.com/owner/repo`).
    pub url: String,
    /// Pinned commit revision (40-hex SHA when available).
    pub revision: String,
}

/// Document metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramMeta {
    /// Diagram title.
    pub title: String,
    /// Optional subtitle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Chrome locale (`en` or `zh-CN`).
    pub locale: String,
    /// Repository coordinates, when the indexed project is a git checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepositoryRef>,
}

/// Root of the architecture-diagram IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramDocument {
    /// IR schema version (see [`SCHEMA_VERSION`]).
    pub schema_version: u8,
    /// Always `"architecture"` in this change.
    pub diagram_type: String,
    /// Document metadata.
    pub meta: DiagramMeta,
    /// Nodes, sorted by `id`.
    pub components: Vec<DiagramComponent>,
    /// Edges, sorted by `(from, to, label, variant)`.
    pub connections: Vec<DiagramConnection>,
}

/// Maps a module path to its slug id: `/`-separated segments joined with `-`,
/// with any character outside `[a-zA-Z0-9_-]` replaced by `-`, and leading or
/// trailing dashes trimmed (module paths like `/src/api` carry a leading
/// separator).
#[must_use]
pub fn module_slug(module_name: &str) -> String {
    let slug: String = module_name
        .chars()
        .map(|c| match c {
            '/' => '-',
            c if c.is_ascii_alphanumeric() || c == '_' || c == '-' => c,
            _ => '-',
        })
        .collect();
    slug.trim_matches('-').to_string()
}

/// Max source references attached to one component (archify parity).
const MAX_SOURCES: usize = 3;

/// Builds a deterministic [`DiagramDocument`] from an architecture overview.
///
/// `layer_map` (from `ArchitectureAnalyzer::module_layer_map`) maps module
/// path → dominant layer name. Connections are emitted only when both
/// endpoints exist as components; parallel connections deduplicate to the
/// most severe variant (`Error > Async > Primary`), keeping a protocol label
/// when an async fact contributed.
#[must_use]
pub fn from_overview(
    overview: &ArchitectureOverview,
    layer_map: &BTreeMap<String, String>,
    title: &str,
) -> DiagramDocument {
    let mut components: Vec<DiagramComponent> = overview
        .module_boundaries
        .iter()
        .map(|boundary| {
            let layer = layer_map.get(&boundary.module_name).map(String::as_str);
            DiagramComponent {
                id: module_slug(&boundary.module_name),
                r#type: layer.map_or(ComponentType::Service, ComponentType::from_layer),
                label: boundary.module_name.clone(),
                sublabel: Some(format!("{} files", boundary.members.len())),
                tag: layer.map(str::to_string),
                sources: boundary
                    .members
                    .iter()
                    .take(MAX_SOURCES)
                    .map(|path| SourceRef {
                        path: path.clone(),
                        line: None,
                        end_line: None,
                        label: None,
                    })
                    .collect(),
            }
        })
        .collect();
    components.sort_by(|a, b| a.id.cmp(&b.id));

    // Deduplicate parallel connections by (from, to); the most severe variant
    // wins, and an async protocol label survives if any async fact existed.
    struct Slot {
        variant: EdgeVariant,
        label: String,
    }
    let mut slots: BTreeMap<(String, String), Slot> = BTreeMap::new();
    let mut record = |from: &str, to: &str, variant: EdgeVariant, label: &str| {
        slots
            .entry((from.to_string(), to.to_string()))
            .and_modify(|slot| {
                if variant.severity() > slot.variant.severity() {
                    slot.variant = variant;
                }
                if matches!(variant, EdgeVariant::Async) && slot.label.is_empty() {
                    slot.label = label.to_string();
                }
            })
            .or_insert_with(|| Slot {
                variant,
                label: label.to_string(),
            });
    };
    for dep in &overview.dependency_directions {
        let (from, to) = (module_slug(&dep.from_module), module_slug(&dep.to_module));
        let variant = if dep.is_circular {
            EdgeVariant::Error
        } else {
            EdgeVariant::Primary
        };
        record(&from, &to, variant, "");
    }
    for dep in &overview.cross_service_deps {
        let (from, to) = (module_slug(&dep.from_module), module_slug(&dep.to_module));
        record(&from, &to, EdgeVariant::Async, &dep.protocol);
    }

    let known: std::collections::BTreeSet<&str> =
        components.iter().map(|c| c.id.as_str()).collect();
    let mut connections: Vec<DiagramConnection> = slots
        .into_iter()
        .filter(|((from, to), _)| known.contains(from.as_str()) && known.contains(to.as_str()))
        .map(|((from, to), slot)| DiagramConnection {
            from,
            to,
            label: slot.label,
            variant: slot.variant,
        })
        .collect();
    connections.sort_by(|a, b| {
        (a.from.as_str(), a.to.as_str(), a.label.as_str(), a.variant).cmp(&(
            b.from.as_str(),
            b.to.as_str(),
            b.label.as_str(),
            b.variant,
        ))
    });

    DiagramDocument {
        schema_version: SCHEMA_VERSION,
        diagram_type: "architecture".to_string(),
        meta: DiagramMeta {
            title: title.to_string(),
            subtitle: None,
            locale: "en".to_string(),
            repository: None,
        },
        components,
        connections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::architecture::{CrossServiceDep, DepDirection, LayerInfo, ModuleBoundary};

    fn empty_overview() -> ArchitectureOverview {
        ArchitectureOverview {
            languages: vec![],
            packages: vec![],
            entry_points: vec![],
            routes: vec![],
            hotspots: vec![],
            module_boundaries: vec![],
            dependency_directions: vec![],
            layers: vec![],
            cross_service_deps: vec![],
        }
    }

    fn boundary(name: &str, members: &[&str]) -> ModuleBoundary {
        ModuleBoundary {
            module_name: name.to_string(),
            members: members.iter().map(|s| (*s).to_string()).collect(),
            incoming_deps: 0,
            outgoing_deps: 0,
            cohesion: 1.0,
        }
    }

    #[test]
    fn empty_overview_yields_empty_document() {
        let doc = from_overview(&empty_overview(), &BTreeMap::new(), "empty");
        assert_eq!(doc.schema_version, 1);
        assert_eq!(doc.diagram_type, "architecture");
        assert!(doc.components.is_empty());
        assert!(doc.connections.is_empty());
        assert_eq!(doc.meta.title, "empty");
    }

    #[test]
    fn layer_map_drives_honest_type_mapping() {
        let mut overview = empty_overview();
        overview.module_boundaries = vec![
            boundary("src/api", &["/src/api/handler.rs"]),
            boundary("src/core", &["/src/core/engine.rs"]),
            boundary("src/db", &["/src/db/pool.rs"]),
            boundary("src/types", &["/src/types/model.rs"]),
            boundary("src/util", &["/src/util/math.rs"]),
        ];
        let layers = BTreeMap::from([
            ("src/api".to_string(), "Controller".to_string()),
            ("src/core".to_string(), "Service".to_string()),
            ("src/db".to_string(), "Repository".to_string()),
            ("src/types".to_string(), "Model".to_string()),
        ]);
        let doc = from_overview(&overview, &layers, "t");
        let by_id: BTreeMap<&str, ComponentType> = doc
            .components
            .iter()
            .map(|c| (c.id.as_str(), c.r#type))
            .collect();
        assert_eq!(by_id["src-api"], ComponentType::Interface);
        assert_eq!(by_id["src-core"], ComponentType::Service);
        assert_eq!(by_id["src-db"], ComponentType::Storage);
        assert_eq!(by_id["src-types"], ComponentType::Model);
        assert_eq!(
            by_id["src-util"],
            ComponentType::Service,
            "unclassified defaults to Service"
        );
        let api = &doc.components[0];
        assert_eq!(api.tag.as_deref(), Some("Controller"));
        assert_eq!(api.sublabel.as_deref(), Some("1 files"));
        assert_eq!(api.sources.len(), 1);
        assert_eq!(api.sources[0].path, "/src/api/handler.rs");
    }

    #[test]
    fn module_slug_replaces_separators_and_unsafe_chars() {
        assert_eq!(module_slug("src/api/v1"), "src-api-v1");
        assert_eq!(
            module_slug("/src/api"),
            "src-api",
            "leading separator trimmed"
        );
        assert_eq!(module_slug("weird name"), "weird-name");
        assert_eq!(module_slug("a_b-c"), "a_b-c");
    }

    #[test]
    fn circular_and_cross_service_facts_drive_variants() {
        let mut overview = empty_overview();
        overview.module_boundaries = vec![
            boundary("src/a", &["/src/a.rs"]),
            boundary("src/b", &["/src/b.rs"]),
        ];
        overview.dependency_directions = vec![DepDirection {
            from_module: "src/a".to_string(),
            to_module: "src/b".to_string(),
            is_circular: true,
        }];
        overview.cross_service_deps = vec![CrossServiceDep {
            from_module: "src/a".to_string(),
            to_module: "src/b".to_string(),
            protocol: "HTTP".to_string(),
        }];
        let doc = from_overview(&overview, &BTreeMap::new(), "t");
        assert_eq!(doc.connections.len(), 1, "parallel edges dedupe");
        let conn = &doc.connections[0];
        assert_eq!(conn.variant, EdgeVariant::Error, "Error outranks Async");
        assert_eq!(conn.label, "HTTP", "async protocol label survives merge");
    }

    #[test]
    fn connections_to_unknown_components_are_dropped() {
        let mut overview = empty_overview();
        overview.module_boundaries = vec![boundary("src/a", &["/src/a.rs"])];
        overview.dependency_directions = vec![DepDirection {
            from_module: "src/a".to_string(),
            to_module: "src/ghost".to_string(),
            is_circular: false,
        }];
        let doc = from_overview(&overview, &BTreeMap::new(), "t");
        assert!(doc.connections.is_empty());
    }

    #[test]
    fn document_serializes_expected_field_names() {
        let mut overview = empty_overview();
        overview.module_boundaries = vec![boundary("src/a", &["/src/a.rs"])];
        overview.layers = vec![LayerInfo {
            layer: "Service".to_string(),
            members: vec!["demo.a".to_string()],
        }];
        let doc = from_overview(&overview, &BTreeMap::new(), "t");
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"schema_version\":1"), "{json}");
        assert!(json.contains("\"diagram_type\":\"architecture\""), "{json}");
        assert!(json.contains("\"type\":\"service\""), "{json}");
        assert!(json.contains("\"sources\""), "{json}");
    }

    #[test]
    fn construction_is_deterministic() {
        let mut overview = empty_overview();
        overview.module_boundaries = vec![
            boundary("src/b", &["/src/b1.rs", "/src/b2.rs"]),
            boundary("src/a", &["/src/a.rs"]),
        ];
        let doc1 = from_overview(&overview, &BTreeMap::new(), "t");
        let doc2 = from_overview(&overview, &BTreeMap::new(), "t");
        assert_eq!(
            serde_json::to_string(&doc1).unwrap(),
            serde_json::to_string(&doc2).unwrap()
        );
        let ids: Vec<&str> = doc1.components.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["src-a", "src-b"], "components sorted by id");
    }
}
