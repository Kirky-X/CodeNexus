// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `architecture` service: high-level project structure overview.

use serde::Serialize;

use crate::analysis::architecture::{ArchitectureAnalyzer, ArchitectureOverview};
use crate::service::error::{CodeNexusError, to_api_error};
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
use crate::service::error::kit_not_initialized;
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::service_api;

/// JSON-serializable architecture output.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArchitectureOutput {
    pub project: String,
    pub overview: ArchitectureOverview,
}

/// Core logic — resolves storage, runs overview, prints JSON.
fn architecture_core(kit: &AsyncKit<AsyncReady>, project: &str) -> Result<(), CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let analyzer = ArchitectureAnalyzer::new(&*storage);
    let overview: ArchitectureOverview = analyzer.overview(project)?;
    let output = ArchitectureOutput {
        project: project.to_string(),
        overview,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "analysis"))]
#[service_api(
    name = "architecture",
    version = "0.3.2",
    description = "Show high-level architecture overview of a project.",
    cli = true
)]
async fn architecture(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    architecture_core(&kit, &project).map_err(|e| to_api_error(e, "architecture_error"))?;
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "analysis"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_architecture_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    #[test]
    fn core_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let result = architecture_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_returns_languages() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage
            .execute("CREATE (:File {id: 'f1', project: 'demo', name: 'main.rs', filePath: '/src/main.rs', language: 'rust', hash: '', lineCount: 0});")
            .expect("create file");
        let result = architecture_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_with_routes() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage
            .execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});")
            .expect("create route");
        storage
            .execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create handler");
        storage
            .execute("CREATE (:CodeRelation {id: 'e1', source: 'h1', target: 'r1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});")
            .expect("create edge");
        let result = architecture_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn output_serializes_to_json() {
        let out = ArchitectureOutput {
            project: "demo".into(),
            overview: ArchitectureOverview {
                languages: vec![],
                packages: vec![],
                entry_points: vec![],
                routes: vec![],
                hotspots: vec![],
            },
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"overview\""));
    }
}
