// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `api-route-map` subcommand handler (T008, v0.2.0).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs an
//! [`ApiReviewer`](crate::analysis::api_review::ApiReviewer), and prints the
//! route map as a JSON object.

use super::args::RouteMapArgs;
use super::error::Result;
use crate::analysis::api_review::{ApiReviewer, RouteEntry};
use crate::kit::{Kit, StorageKey};

/// Runs the `api-route-map` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, runs [`ApiReviewer::route_map`], and prints the result as a
/// JSON object `{ project, route_map: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &RouteMapArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let route_map: Vec<RouteEntry> = reviewer.route_map(&args.project)?;
    let output = RouteMapOutput {
        project: args.project.clone(),
        route_map,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable route-map output.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct RouteMapOutput {
    /// The queried project name.
    pub project: String,
    /// The list of route entries.
    pub route_map: Vec<RouteEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::RouteMapArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("route_map_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> RouteMapArgs {
        RouteMapArgs {
            project: project.to_string(),
            db: db.to_string(),
        }
    }

    #[test]
    fn run_route_map_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_route_map_returns_route() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'h1', target: 'r1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn route_map_output_serializes_to_json() {
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
