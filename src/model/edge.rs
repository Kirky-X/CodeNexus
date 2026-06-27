// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Edge entity and builder (DDD §5.8).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use super::{EdgeType, NodeId};

/// Confidence tier classifying the strength of an edge based on the
/// caller↔callee file/import scope (design.md D4, T9 H4).
///
/// The tier is a categorical classification complementing the numeric
/// `Edge::confidence` score. Resolvers populate the tier during resolution;
/// `--min-confidence` filtering uses the tier's `default_score()` to map
/// categorical filters to numeric thresholds.
///
/// # Variants
///
/// - [`SameFile`](Self::SameFile): caller and callee are in the same file
///   (highest confidence, default score 0.95).
/// - [`ImportScoped`](Self::ImportScoped): caller imports the callee's module
///   (medium confidence, default score 0.90).
/// - [`Global`](Self::Global): no file/import relationship — global resolution
///   (lowest confidence, default score 0.50).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ConfidenceTier {
    /// Caller and callee are in the same file.
    SameFile,
    /// Caller imports the callee's module (import-scoped resolution).
    ImportScoped,
    /// No file/import relationship — global resolution.
    #[default]
    Global,
}

impl ConfidenceTier {
    /// Returns the default confidence score for this tier (design.md D4).
    ///
    /// - [`SameFile`](Self::SameFile) → 0.95
    /// - [`ImportScoped`](Self::ImportScoped) → 0.90
    /// - [`Global`](Self::Global) → 0.50
    #[must_use]
    pub fn default_score(&self) -> f32 {
        match self {
            Self::SameFile => 0.95,
            Self::ImportScoped => 0.90,
            Self::Global => 0.50,
        }
    }
}

impl ConfidenceTier {
    /// Returns the database representation of this tier (UPPER_SNAKE_CASE),
    /// matching the convention used by [`EdgeType::as_db_type`].
    #[must_use]
    pub fn as_db_type(&self) -> &'static str {
        match self {
            Self::SameFile => "SAME_FILE",
            Self::ImportScoped => "IMPORT_SCOPED",
            Self::Global => "GLOBAL",
        }
    }
}

impl fmt::Display for ConfidenceTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_type())
    }
}

impl FromStr for ConfidenceTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "SAME_FILE" => Ok(Self::SameFile),
            "IMPORT_SCOPED" => Ok(Self::ImportScoped),
            "GLOBAL" => Ok(Self::Global),
            other => Err(format!("unknown ConfidenceTier: {other}")),
        }
    }
}

/// An edge in the code knowledge graph (DDD §5.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node id.
    pub source: NodeId,
    /// Target node id.
    pub target: NodeId,
    /// The edge type (one of 14, DDD §7.2).
    pub edge_type: EdgeType,
    /// Confidence score in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Confidence tier classifying the edge by caller↔callee scope (design.md D4).
    /// Defaults to [`ConfidenceTier::Global`]; resolvers override during resolution.
    #[serde(default)]
    pub confidence_tier: ConfidenceTier,
    /// Human-readable reason for the edge (e.g. evidence).
    pub reason: Option<String>,
    /// Source line where the relation originates.
    pub start_line: Option<u32>,
    /// The project this edge belongs to (multi-project isolation, DDD §2.3).
    pub project: String,
}

impl Edge {
    /// Creates a new edge with default confidence (1.0) and tier
    /// ([`ConfidenceTier::Global`]).
    #[must_use]
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        edge_type: EdgeType,
        project: impl Into<String>,
    ) -> Self {
        Edge {
            source: source.into(),
            target: target.into(),
            edge_type,
            confidence: 1.0,
            confidence_tier: ConfidenceTier::Global,
            reason: None,
            start_line: None,
            project: project.into(),
        }
    }

    /// Creates an [`EdgeBuilder`] with the required fields and default
    /// confidence (1.0).
    pub fn builder(
        source: impl Into<String>,
        target: impl Into<String>,
        edge_type: EdgeType,
        project: impl Into<String>,
    ) -> EdgeBuilder {
        EdgeBuilder {
            edge: Edge::new(source, target, edge_type, project),
        }
    }
}

/// Builder for [`Edge`] using the fluent setter pattern.
#[must_use]
#[derive(Debug, Clone)]
pub struct EdgeBuilder {
    edge: Edge,
}

impl EdgeBuilder {
    /// Sets the source node id.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.edge.source = source.into();
        self
    }

    /// Sets the target node id.
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.edge.target = target.into();
        self
    }

    /// Sets the edge type.
    pub fn edge_type(mut self, edge_type: EdgeType) -> Self {
        self.edge.edge_type = edge_type;
        self
    }

    /// Sets the confidence score.
    pub fn confidence(mut self, confidence: f32) -> Self {
        self.edge.confidence = confidence;
        self
    }

    /// Sets the confidence tier.
    pub fn confidence_tier(mut self, tier: ConfidenceTier) -> Self {
        self.edge.confidence_tier = tier;
        self
    }

    /// Sets the reason.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.edge.reason = Some(reason.into());
        self
    }

    /// Sets the source line.
    pub fn start_line(mut self, start_line: u32) -> Self {
        self.edge.start_line = Some(start_line);
        self
    }

    /// Sets the project.
    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.edge.project = project.into();
        self
    }

    /// Builds the [`Edge`].
    #[must_use]
    pub fn build(self) -> Edge {
        self.edge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_required_fields_and_default_confidence() {
        let edge = Edge::new("s", "t", EdgeType::Calls, "proj");
        assert_eq!(edge.source, "s");
        assert_eq!(edge.target, "t");
        assert_eq!(edge.edge_type, EdgeType::Calls);
        assert_eq!(edge.confidence, 1.0);
        assert_eq!(edge.reason, None);
        assert_eq!(edge.start_line, None);
        assert_eq!(edge.project, "proj");
    }

    #[test]
    fn new_accepts_string_and_str() {
        let s = String::from("src");
        let t = String::from("tgt");
        let p = String::from("proj");
        let edge = Edge::new(s, t, EdgeType::Reads, p);
        assert_eq!(edge.source, "src");
        assert_eq!(edge.target, "tgt");
        assert_eq!(edge.project, "proj");
    }

    #[test]
    fn builder_sets_required_fields() {
        let edge = Edge::builder("s", "t", EdgeType::Calls, "proj").build();
        assert_eq!(edge.source, "s");
        assert_eq!(edge.target, "t");
        assert_eq!(edge.edge_type, EdgeType::Calls);
        assert_eq!(edge.project, "proj");
        assert_eq!(edge.confidence, 1.0);
    }

    #[test]
    fn builder_fluent_setters() {
        let edge = Edge::builder("s", "t", EdgeType::Calls, "proj")
            .source("s2")
            .target("t2")
            .edge_type(EdgeType::FfiCalls)
            .confidence(0.85)
            .reason("extern \"C\" declaration match")
            .start_line(42)
            .project("proj2")
            .build();

        assert_eq!(edge.source, "s2");
        assert_eq!(edge.target, "t2");
        assert_eq!(edge.edge_type, EdgeType::FfiCalls);
        assert!((edge.confidence - 0.85).abs() < f32::EPSILON);
        assert_eq!(
            edge.reason.as_deref(),
            Some("extern \"C\" declaration match")
        );
        assert_eq!(edge.start_line, Some(42));
        assert_eq!(edge.project, "proj2");
    }

    #[test]
    fn builder_confidence_override() {
        let edge = Edge::builder("s", "t", EdgeType::Calls, "proj")
            .confidence(0.5)
            .build();
        assert!((edge.confidence - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn serde_roundtrip() {
        let edge = Edge::builder("s", "t", EdgeType::Calls, "proj")
            .confidence(0.9)
            .reason("test")
            .start_line(7)
            .build();

        let json = serde_json::to_string(&edge).unwrap();
        let parsed: Edge = serde_json::from_str(&json).unwrap();
        assert_eq!(edge, parsed);
    }

    #[test]
    fn serde_roundtrip_minimal() {
        let edge = Edge::new("s", "t", EdgeType::Contains, "p");
        let json = serde_json::to_string(&edge).unwrap();
        let parsed: Edge = serde_json::from_str(&json).unwrap();
        assert_eq!(edge, parsed);
    }

    #[test]
    fn clone_is_equal() {
        let edge = Edge::new("s", "t", EdgeType::Calls, "proj");
        let cloned = edge.clone();
        assert_eq!(edge, cloned);
    }

    #[test]
    fn debug_is_non_empty() {
        let edge = Edge::new("s", "t", EdgeType::Calls, "proj");
        let debug = format!("{edge:?}");
        assert!(debug.contains("Edge"));
        assert!(debug.contains("Calls"));
    }

    #[test]
    fn equality() {
        let a = Edge::new("s", "t", EdgeType::Calls, "proj");
        let b = Edge::new("s", "t", EdgeType::Calls, "proj");
        assert_eq!(a, b);

        let c = Edge::new("s", "t", EdgeType::Reads, "proj");
        assert_ne!(a, c);

        let d = Edge::new("s", "t", EdgeType::Calls, "other");
        assert_ne!(a, d);
    }
}
