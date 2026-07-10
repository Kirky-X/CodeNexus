// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `route-map` service: list API routes and their handlers.

use serde::Serialize;

#[cfg(feature = "api-review")]
use crate::analysis::api_review::{ApiReviewer, RouteEntry};
use crate::service::error::{CliError, to_api_error};
#[cfg(feature = "api-review")]
use crate::kit::{Kit, StorageKey};
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::service_api;

/// JSON-serializable route-map output.
#[cfg(feature = "api-review")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RouteMapOutput {
    pub project: String,
    pub route_map: Vec<RouteEntry>,
}

/// Core logic — resolves storage, runs route_map, prints JSON.
#[cfg(feature = "api-review")]
fn route_map_core(kit: &Kit, project: &str) -> Result<(), CliError> {
    let storage = kit.require::<StorageKey>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let route_map: Vec<RouteEntry> = reviewer.route_map(project)?;
    let output = RouteMapOutput {
        project: project.to_string(),
        route_map,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "api-review"))]
#[service_api(
    name = "route_map",
    version = "0.3.2",
    description = "List API routes and their handler functions.",
    cli = true
)]
async fn route_map(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    route_map_core(&kit, &project).map_err(|e| to_api_error(e, "route_map_error"))?;
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "api-review"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_route_map_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    #[test]
    fn core_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let result = route_map_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_returns_route() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'h1', target: 'r1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");
        let result = route_map_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn output_serializes_to_json() {
        let out = RouteMapOutput {
            project: "demo".into(),
            route_map: vec![RouteEntry {
                path: "/api/users".into(),
                method: "GET".into(),
                handler_id: "h1".into(),
                handler_name: "list_users".into(),
                middleware: vec![],
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"route_map\""));
        assert!(json.contains("\"/api/users\""));
    }
}
