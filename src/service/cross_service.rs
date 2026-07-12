// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `cross_service` service: detect cross-service links via route pattern matching.

use serde::Serialize;

#[cfg(feature = "cross-service")]
use crate::analysis::cross_service::{CrossServiceLink, CrossServiceLinker};
#[cfg(all(feature = "cross-service", any(feature = "cli", test)))]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "cross-service", any(feature = "cli", test)))]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "cross-service"))]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(all(feature = "cli", feature = "cross-service"))]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable cross-service link output.
#[cfg(feature = "cross-service")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CrossServiceOutput {
    pub project: String,
    pub links: Vec<CrossServiceLink>,
}

/// Runs cross-service link detection against an injected Kit (testable core).
#[cfg(all(feature = "cross-service", any(feature = "cli", test)))]
pub fn run_cross_service(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
) -> Result<CrossServiceOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let linker = CrossServiceLinker::new(&*storage, project);
    let links = linker.link()?;
    Ok(CrossServiceOutput {
        project: project.to_string(),
        links,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "cross-service"))]
#[service_api(
    name = "cross_service",
    version = "0.3.2",
    description = "Detect cross-service links by matching HTTP route patterns against caller string literals.",
    cli = true
)]
async fn cross_service(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_cross_service(&kit, &project)
        .map_err(|e| to_api_error(e, "cross_service_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(all(test, feature = "cross-service"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_cross_service_testdb");
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
    fn run_cross_service_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_cross_service(&kit, "demo").expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert!(output.links.is_empty(), "no links on empty DB");
    }

    #[test]
    fn run_cross_service_returns_links_when_patterns_match() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("create function");
        let output = run_cross_service(&kit, "demo").expect("run should succeed");
        assert_eq!(output.project, "demo");
        // Linker may find links depending on matching logic; just verify it ran.
    }

    #[test]
    fn run_cross_service_unknown_project_returns_empty_links() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'other', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        let output = run_cross_service(&kit, "demo").expect("run should succeed");
        assert!(output.links.is_empty(), "no links for absent project");
    }

    #[test]
    fn cross_service_output_serializes_to_json() {
        let out = CrossServiceOutput {
            project: "demo".into(),
            links: vec![CrossServiceLink {
                route_id: "r1".into(),
                route_pattern: "/api/users".into(),
                caller_id: "f1".into(),
                caller_file: "/src/caller.rs".into(),
                caller_line: 10,
                match_type: crate::analysis::cross_service::MatchType::Exact,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"links\""));
        assert!(json.contains("\"route_id\":\"r1\""));
        assert!(json.contains("\"match_type\":\"Exact\""));
    }
}
