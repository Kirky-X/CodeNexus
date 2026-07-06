// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `api-impact` subcommand handler (T008, v0.2.0).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs an
//! [`ApiReviewer`](crate::analysis::api_review::ApiReviewer), and prints the
//! impact analysis as a JSON object.

use super::args::ApiImpactArgs;
use super::error::Result;
use crate::analysis::api_review::{ApiReviewer, ImpactEntry};
use crate::kit::{Kit, StorageKey};

/// Runs the `api-impact` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, runs [`ApiReviewer::api_impact`], and prints the result as a
/// JSON object `{ project, endpoint, impact: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &ApiImpactArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let impact: Vec<ImpactEntry> = reviewer.api_impact(&args.project, &args.endpoint)?;
    let output = ApiImpactOutput {
        project: args.project.clone(),
        endpoint: args.endpoint.clone(),
        impact,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable api-impact output.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ApiImpactOutput {
    /// The queried project name.
    pub project: String,
    /// The endpoint path or name analysed.
    pub endpoint: String,
    /// The list of impact entries.
    pub impact: Vec<ImpactEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ApiImpactArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("api_impact_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, endpoint: &str, db: &str) -> ApiImpactArgs {
        ApiImpactArgs {
            project: project.to_string(),
            endpoint: endpoint.to_string(),
            db: db.to_string(),
        }
    }

    #[test]
    fn run_api_impact_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", "/api/users", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_api_impact_with_endpoint_and_handler() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Endpoint {id: 'e1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', expectedSchema: '', parentQn: ''});").expect("create endpoint");
        storage.execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:CodeRelation {id: 'he1', source: 'h1', target: 'e1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create handles edge");
        let args = make_args("demo", "/api/users", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn api_impact_output_serializes_to_json() {
        let out = ApiImpactOutput {
            project: "demo".into(),
            endpoint: "/api/users".into(),
            impact: vec![ImpactEntry {
                endpoint: "/api/users".into(),
                affected_caller: "caller_a".into(),
                caller_file: "/src/a.rs".into(),
                caller_line: 10,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"endpoint\":\"/api/users\""));
        assert!(json.contains("\"impact\""));
        assert!(json.contains("\"caller_a\""));
    }
}
