// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;

use serde::Serialize;

use crate::kit::{AsyncKit, AsyncReady, StorageModule};
use crate::model::NodeLabel;
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(feature = "cli")]
use crate::service::runtime::kit;
use crate::storage::schema::escape_identifier;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// One external package's supply-chain summary.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SupplyPackage {
    pub name: String,
    pub ecosystem: String,
    pub language: String,
    /// Number of distinct project files with a DEPENDS_ON edge to the package.
    pub dependents: usize,
    /// The import paths that reference the package (deduplicated, sorted).
    pub import_paths: Vec<String>,
}

/// JSON-serializable `supply` output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SupplyOutput {
    pub project: String,
    pub total_packages: usize,
    pub total_edges: usize,
    /// Sorted by `dependents` descending, then name ascending.
    pub packages: Vec<SupplyPackage>,
}

/// Core logic — aggregates the external-dependency view (testable core).
pub fn run_supply(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
) -> Result<SupplyOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let table = escape_identifier(NodeLabel::ExternalPackage.table_name());

    let mut packages: BTreeMap<String, SupplyPackage> = BTreeMap::new();
    let node_rows = storage.query(&format!(
        "MATCH (n:{table}) RETURN n.id AS id, n.name AS name, \
         n.ecosystem AS ecosystem, n.language AS language;"
    ))?;
    for row in &node_rows {
        let id = row.first().and_then(|v| v.as_str()).unwrap_or_default();
        let name = row.get(1).and_then(|v| v.as_str()).unwrap_or_default();
        let ecosystem = row.get(2).and_then(|v| v.as_str()).unwrap_or("other");
        let language = row.get(3).and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() || name.is_empty() {
            continue;
        }
        packages
            .entry(id.to_string())
            .or_insert_with(|| SupplyPackage {
                name: name.to_string(),
                ecosystem: ecosystem.to_string(),
                language: language.to_string(),
                dependents: 0,
                import_paths: Vec::new(),
            });
    }

    let edge_rows = storage.query(
        "MATCH (r:CodeRelation) WHERE r.type = 'DEPENDS_ON' \
         RETURN r.source AS source, r.target AS target, r.reason AS reason;",
    )?;
    let mut total_edges = 0usize;
    let mut seen_pairs: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    for row in &edge_rows {
        let source = row.first().and_then(|v| v.as_str()).unwrap_or_default();
        let target = row.get(1).and_then(|v| v.as_str()).unwrap_or_default();
        let reason = row.get(2).and_then(|v| v.as_str()).unwrap_or_default();
        if target.is_empty() || source.is_empty() {
            continue;
        }
        if !seen_pairs.insert((source.to_string(), target.to_string())) {
            continue;
        }
        total_edges += 1;
        if let Some(pkg) = packages.get_mut(target) {
            pkg.dependents += 1;
            if let Some(import_path) = reason.strip_prefix("import: ") {
                if !import_path.is_empty() && !pkg.import_paths.iter().any(|p| p == import_path) {
                    pkg.import_paths.push(import_path.to_string());
                }
            }
        }
    }
    for pkg in packages.values_mut() {
        pkg.import_paths.sort();
    }

    let mut list: Vec<SupplyPackage> = packages.into_values().collect();
    list.sort_by(|a, b| {
        b.dependents
            .cmp(&a.dependents)
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(SupplyOutput {
        project: project.to_string(),
        total_packages: list.len(),
        total_edges,
        packages: list,
    })
}

/// CLI wrapper — prints the supply view to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "supply",
    version = "0.4.0",
    description = "Supply-chain view: list ExternalPackage nodes (unresolved imports) with ecosystem, language, dependent file count, and import paths. Params: project — validate project exists (empty = skip).",
    cli = true
)]
async fn supply(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_supply(&kit, &project).map_err(|e| to_api_error(e, "supply_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_kit() -> (TempDir, AsyncKit<AsyncReady>) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("supply_db");
        let config = KitBootstrapConfig::new(db);
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .unwrap();
        (dir, kit)
    }

    fn seed_supply_graph(kit: &AsyncKit<AsyncReady>) {
        let storage = kit.require::<StorageModule>().unwrap();
        // Two packages: serde_json (3 dependents) and leftpad (1 dependent).
        storage
            .execute("CREATE (:File {id: 'f1', project: 'demo', name: 'a.rs', filePath: '/src/a.rs', language: 'rust', hash: '', lineCount: 0});")
            .unwrap();
        storage
            .execute("CREATE (:File {id: 'f2', project: 'demo', name: 'b.rs', filePath: '/src/b.rs', language: 'rust', hash: '', lineCount: 0});")
            .unwrap();
        storage
            .execute("CREATE (:ExternalPackage {id: 'extpkg:demo:serde_json', name: 'serde_json', ecosystem: 'crates', language: 'rust', project: 'demo'});")
            .unwrap();
        storage
            .execute("CREATE (:ExternalPackage {id: 'extpkg:demo:leftpad', name: 'leftpad', ecosystem: 'npm', language: 'javascript', project: 'demo'});")
            .unwrap();
        let dependents = [("f1", 0), ("f2", 1), ("f3", 2)];
        for (file, i) in dependents {
            if file == "f3" {
                storage
                    .execute("CREATE (:File {id: 'f3', project: 'demo', name: 'c.rs', filePath: '/src/c.rs', language: 'rust', hash: '', lineCount: 0});")
                    .unwrap();
            }
            storage
                .execute(&format!(
                    "CREATE (:CodeRelation {{id: 'e_serde_{i}', source: '{file}', target: 'extpkg:demo:serde_json', type: 'DEPENDS_ON', confidence: 0.9, confidenceTier: 'ImportScoped', reason: 'import: serde_json::de', startLine: {i}, project: 'demo'}});"
                ))
                .unwrap();
        }
        storage
            .execute("CREATE (:CodeRelation {id: 'e_lp', source: 'f1', target: 'extpkg:demo:leftpad', type: 'DEPENDS_ON', confidence: 0.9, confidenceTier: 'ImportScoped', reason: 'import: leftpad', startLine: 9, project: 'demo'});")
            .unwrap();
    }

    #[test]
    fn supply_aggregates_sorted_by_dependents() {
        let (_dir, kit) = fresh_kit();
        seed_supply_graph(&kit);
        let out = run_supply(&kit, "demo").expect("supply runs");
        assert_eq!(out.total_packages, 2);
        // 3 serde_json edges + 1 leftpad edge (deduplicated by file pair).
        assert_eq!(out.total_edges, 4);
        assert_eq!(out.packages[0].name, "serde_json");
        assert_eq!(out.packages[0].dependents, 3);
        assert_eq!(out.packages[1].name, "leftpad");
        assert_eq!(out.packages[1].dependents, 1);
        assert!(out.packages[0]
            .import_paths
            .contains(&"serde_json::de".to_string()));
    }

    #[test]
    fn supply_on_empty_db_returns_zeroes() {
        let (_dir, kit) = fresh_kit();
        let out = run_supply(&kit, "").expect("supply runs");
        assert_eq!(out.total_packages, 0);
        assert_eq!(out.total_edges, 0);
        assert!(out.packages.is_empty());
    }
}
