// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `cross-service` subcommand handler (T010, v0.2.0).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs a
//! [`CrossServiceLinker`](crate::analysis::cross_service::CrossServiceLinker),
//! matches HTTP route patterns against caller string literals, and prints
//! the detected links as a JSON object.

use super::args::CrossServiceArgs;
use super::error::Result;
use crate::analysis::cross_service::{CrossServiceLink, CrossServiceLinker};
use crate::kit::{Kit, StorageKey};

/// Runs the `cross-service` subcommand.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &CrossServiceArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let linker = CrossServiceLinker::new(&*storage, &args.project);
    let links: Vec<CrossServiceLink> = linker.link()?;
    let output = CrossServiceOutput {
        project: args.project.clone(),
        links,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable cross-service link output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrossServiceOutput {
    /// The queried project name.
    pub project: String,
    /// The detected cross-service links.
    pub links: Vec<CrossServiceLink>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::CrossServiceArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cross_service_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> CrossServiceArgs {
        CrossServiceArgs {
            project: project.to_string(),
            db: db.to_string(),
        }
    }

    #[test]
    fn run_cross_service_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_cross_service_returns_links() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("create function");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
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
