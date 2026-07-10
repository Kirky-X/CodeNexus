//! Type mapping between CodeNexus and gitnexus node/edge vocabularies.
//!
//! Loads `tools/verification/type_map.json` and normalizes raw type strings
//! from either side into a [`CanonicalType`]. Types with no mapping are
//! returned as [`CanonicalType::Unmapped`] so the report can surface the gap
//! rather than silently dropping the count.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Canonical type after normalization.
///
/// - `Comparable`: both sides index this type; counts are directly comparable.
/// - `CodenexusOnly`: CodeNexus parses this, gitnexus does not (informational only).
/// - `GitnexusOnly`: gitnexus indexes this, CodeNexus does not (informational only).
/// - `AnalysisArtifact`: both sides have it but it's an upper-layer analysis
///   product (Community / Process / Route / Tool), excluded from parsing
///   correctness comparison.
/// - `Unmapped`: no entry in type_map.json; the report must surface this gap.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CanonicalType {
    Comparable(String),
    CodenexusOnly(String),
    GitnexusOnly(String),
    AnalysisArtifact(String),
    Unmapped(String),
}

/// Raw shape of `type_map.json`.
#[derive(Debug, Deserialize)]
struct TypeMapFile {
    nodes: CategoryMap,
    edges: CategoryMap,
}

#[derive(Debug, Deserialize)]
struct CategoryMap {
    /// type name → { canonical: String }
    comparable: HashMap<String, CanonicalEntry>,
    /// list of type names
    codenexus_only: Vec<String>,
    /// list of type names
    gitnexus_only: Vec<String>,
    /// list of type names (both sides have, but not parsing outputs)
    analysis_artifact: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CanonicalEntry {
    canonical: String,
}

/// Loaded type_map.json with lookup tables for both nodes and edges.
#[derive(Debug, Clone, Default)]
pub struct TypeMap {
    /// (raw_type) → CanonicalType, for CodeNexus side.
    codenexus_nodes: HashMap<String, CanonicalType>,
    codenexus_edges: HashMap<String, CanonicalType>,
    /// (raw_type) → CanonicalType, for gitnexus side.
    gitnexus_nodes: HashMap<String, CanonicalType>,
    gitnexus_edges: HashMap<String, CanonicalType>,
}

impl TypeMap {
    /// Load and build bidirectional lookup tables from `type_map.json`.
    ///
    /// For `comparable` entries, both sides map to `Comparable(canonical)`.
    /// For `codenexus_only`, the CodeNexus side maps to `CodenexusOnly(name)`
    /// and the gitnexus side has no entry (so gitnexus hits for that name
    /// return `Unmapped`).
    /// For `gitnexus_only`, symmetric.
    /// For `analysis_artifact`, both sides map to `AnalysisArtifact(name)`.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read type_map at {}", path.display()))?;
        let file: TypeMapFile = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse type_map JSON at {}", path.display()))?;

        let mut tm = Self::default();

        // Nodes: comparable (both sides)
        for (name, entry) in &file.nodes.comparable {
            let c = CanonicalType::Comparable(entry.canonical.clone());
            tm.codenexus_nodes.insert(name.clone(), c.clone());
            tm.gitnexus_nodes.insert(name.clone(), c);
        }
        // Nodes: codenexus_only
        for name in &file.nodes.codenexus_only {
            tm.codenexus_nodes
                .insert(name.clone(), CanonicalType::CodenexusOnly(name.clone()));
        }
        // Nodes: gitnexus_only
        for name in &file.nodes.gitnexus_only {
            tm.gitnexus_nodes
                .insert(name.clone(), CanonicalType::GitnexusOnly(name.clone()));
        }
        // Nodes: analysis_artifact (both sides)
        for name in &file.nodes.analysis_artifact {
            let c = CanonicalType::AnalysisArtifact(name.clone());
            tm.codenexus_nodes.insert(name.clone(), c.clone());
            tm.gitnexus_nodes.insert(name.clone(), c);
        }

        // Edges: comparable (both sides)
        for (name, entry) in &file.edges.comparable {
            let c = CanonicalType::Comparable(entry.canonical.clone());
            tm.codenexus_edges.insert(name.clone(), c.clone());
            tm.gitnexus_edges.insert(name.clone(), c);
        }
        // Edges: codenexus_only
        for name in &file.edges.codenexus_only {
            tm.codenexus_edges
                .insert(name.clone(), CanonicalType::CodenexusOnly(name.clone()));
        }
        // Edges: gitnexus_only
        for name in &file.edges.gitnexus_only {
            tm.gitnexus_edges
                .insert(name.clone(), CanonicalType::GitnexusOnly(name.clone()));
        }
        // Edges: analysis_artifact (both sides)
        for name in &file.edges.analysis_artifact {
            let c = CanonicalType::AnalysisArtifact(name.clone());
            tm.codenexus_edges.insert(name.clone(), c.clone());
            tm.gitnexus_edges.insert(name.clone(), c);
        }

        Ok(tm)
    }

    /// Normalize a CodeNexus node label to its canonical type.
    #[must_use]
    pub fn normalize_codenexus_node(&self, raw: &str) -> CanonicalType {
        self.codenexus_nodes
            .get(raw)
            .cloned()
            .unwrap_or_else(|| CanonicalType::Unmapped(raw.to_string()))
    }

    /// Normalize a CodeNexus edge type string (e.g. "CALLS") to canonical.
    #[must_use]
    pub fn normalize_codenexus_edge(&self, raw: &str) -> CanonicalType {
        self.codenexus_edges
            .get(raw)
            .cloned()
            .unwrap_or_else(|| CanonicalType::Unmapped(raw.to_string()))
    }

    /// Normalize a gitnexus node label to its canonical type.
    #[must_use]
    pub fn normalize_gitnexus_node(&self, raw: &str) -> CanonicalType {
        self.gitnexus_nodes
            .get(raw)
            .cloned()
            .unwrap_or_else(|| CanonicalType::Unmapped(raw.to_string()))
    }

    /// Normalize a gitnexus CodeRelation.type string to canonical.
    #[must_use]
    pub fn normalize_gitnexus_edge(&self, raw: &str) -> CanonicalType {
        self.gitnexus_edges
            .get(raw)
            .cloned()
            .unwrap_or_else(|| CanonicalType::Unmapped(raw.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal in-memory TypeMap for unit tests without touching disk.
    fn make_test_map() -> TypeMap {
        let mut tm = TypeMap::default();
        // comparable
        let c = CanonicalType::Comparable("function".to_string());
        tm.codenexus_nodes.insert("Function".to_string(), c.clone());
        tm.gitnexus_nodes.insert("Function".to_string(), c);
        // codenexus_only
        tm.codenexus_nodes.insert(
            "GlobalVar".to_string(),
            CanonicalType::CodenexusOnly("GlobalVar".to_string()),
        );
        // gitnexus_only
        tm.gitnexus_nodes.insert(
            "CodeElement".to_string(),
            CanonicalType::GitnexusOnly("CodeElement".to_string()),
        );
        // analysis_artifact
        let a = CanonicalType::AnalysisArtifact("Community".to_string());
        tm.codenexus_nodes
            .insert("Community".to_string(), a.clone());
        tm.gitnexus_nodes.insert("Community".to_string(), a);
        // edge comparable
        let e = CanonicalType::Comparable("calls".to_string());
        tm.codenexus_edges.insert("CALLS".to_string(), e.clone());
        tm.gitnexus_edges.insert("CALLS".to_string(), e);
        tm
    }

    #[test]
    fn mapped_comparable_normalizes_identically_on_both_sides() {
        let tm = make_test_map();
        let cn = tm.normalize_codenexus_node("Function");
        let gn = tm.normalize_gitnexus_node("Function");
        assert_eq!(
            cn, gn,
            "Function should normalize identically on both sides"
        );
        assert_eq!(cn, CanonicalType::Comparable("function".to_string()));
    }

    #[test]
    fn unmapped_type_returns_unmapped_not_panic() {
        let tm = make_test_map();
        let result = tm.normalize_codenexus_node("Nonexistent");
        assert_eq!(result, CanonicalType::Unmapped("Nonexistent".to_string()));
        // gitnexus side has no entry for GlobalVar (codenexus_only)
        let result = tm.normalize_gitnexus_node("GlobalVar");
        assert_eq!(result, CanonicalType::Unmapped("GlobalVar".to_string()));
    }

    #[test]
    fn codenexus_only_and_gitnexus_only_classified_correctly() {
        let tm = make_test_map();
        // CodeNexus side: GlobalVar is codenexus_only
        assert_eq!(
            tm.normalize_codenexus_node("GlobalVar"),
            CanonicalType::CodenexusOnly("GlobalVar".to_string())
        );
        // gitnexus side: CodeElement is gitnexus_only
        assert_eq!(
            tm.normalize_gitnexus_node("CodeElement"),
            CanonicalType::GitnexusOnly("CodeElement".to_string())
        );
    }

    #[test]
    fn analysis_artifact_classified_on_both_sides() {
        let tm = make_test_map();
        assert_eq!(
            tm.normalize_codenexus_node("Community"),
            CanonicalType::AnalysisArtifact("Community".to_string())
        );
        assert_eq!(
            tm.normalize_gitnexus_node("Community"),
            CanonicalType::AnalysisArtifact("Community".to_string())
        );
    }

    #[test]
    fn edge_normalization_works() {
        let tm = make_test_map();
        assert_eq!(
            tm.normalize_codenexus_edge("CALLS"),
            CanonicalType::Comparable("calls".to_string())
        );
        assert_eq!(
            tm.normalize_gitnexus_edge("CALLS"),
            CanonicalType::Comparable("calls".to_string())
        );
        assert_eq!(
            tm.normalize_codenexus_edge("FFI_CALLS"),
            CanonicalType::Unmapped("FFI_CALLS".to_string())
        );
    }
}
