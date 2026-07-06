// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `architecture` subcommand handler (T006, v0.1.6).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs an
//! [`ArchitectureAnalyzer`](crate::analysis::architecture::ArchitectureAnalyzer),
//! and prints the overview as a JSON object.

use super::args::ArchitectureArgs;
use super::error::Result;
use crate::analysis::architecture::{ArchitectureAnalyzer, ArchitectureOverview};
use crate::kit::{Kit, StorageKey};

/// Runs the `architecture` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, runs [`ArchitectureAnalyzer::overview`], and prints the result
/// as a JSON object `{ project, overview: { languages, packages, entry_points,
/// routes, hotspots } }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &ArchitectureArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let analyzer = ArchitectureAnalyzer::new(&*storage);
    let overview: ArchitectureOverview = analyzer.overview(&args.project)?;
    let output = ArchitectureOutput {
        project: args.project.clone(),
        overview,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable architecture output.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ArchitectureOutput {
    /// The queried project name.
    pub project: String,
    /// The architecture overview.
    pub overview: ArchitectureOverview,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ArchitectureArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("arch_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> ArchitectureArgs {
        ArchitectureArgs {
            project: project.to_string(),
            db: db.to_string(),
        }
    }

    #[test]
    fn run_architecture_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_architecture_returns_languages() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .execute("CREATE (:File {id: 'f1', project: 'demo', name: 'main.rs', filePath: '/src/main.rs', language: 'rust', hash: '', lineCount: 0});")
            .expect("create file");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_architecture_with_routes() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});")
            .expect("create route");
        storage
            .execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create handler");
        storage
            .execute("CREATE (:CodeRelation {id: 'e1', source: 'h1', target: 'r1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});")
            .expect("create edge");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn architecture_output_serializes_to_json() {
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
