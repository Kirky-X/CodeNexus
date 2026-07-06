// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `community` subcommand handler (T009, v0.2.0).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs a
//! [`CommunityDetector`](crate::analysis::community::CommunityDetector),
//! runs Louvain modularity optimization on the project's `CALLS` graph, and
//! prints the detected communities as a JSON object.

use super::args::CommunityArgs;
use super::error::Result;
use crate::analysis::community::{Community, CommunityDetector};
use crate::kit::{Kit, StorageKey};

/// Runs the `community` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, builds a [`CommunityDetector`] scoped to `args.project`,
/// optionally applies `--resolution`, runs [`CommunityDetector::detect_communities`],
/// and prints the result as a JSON object `{ project, resolution, communities: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &CommunityArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let mut detector = CommunityDetector::new(&*storage, &args.project);
    if let Some(res) = args.resolution {
        detector = detector.with_resolution(res);
    }
    let communities: Vec<Community> = detector.detect_communities()?;
    let output = CommunityOutput {
        project: args.project.clone(),
        resolution: detector.resolution(),
        communities,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable community detection output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommunityOutput {
    /// The queried project name.
    pub project: String,
    /// The Louvain resolution (γ) used for this run.
    pub resolution: f64,
    /// The detected communities (sorted by size descending).
    pub communities: Vec<Community>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::CommunityArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("community_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> CommunityArgs {
        CommunityArgs {
            project: project.to_string(),
            db: db.to_string(),
            resolution: None,
        }
    }

    #[test]
    fn run_community_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_community_returns_communities() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        // 3 functions, 2 CALLS edges (a→b, b→c) → 1 community of size 3.
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create c");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge 1");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b', target: 'f_c', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge 2");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_community_with_resolution_flag() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");
        let args = CommunityArgs {
            project: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            resolution: Some(2.0),
        };
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run with resolution should succeed: {:?}", result.err());
    }

    #[test]
    fn community_output_serializes_to_json() {
        let out = CommunityOutput {
            project: "demo".into(),
            resolution: 1.0,
            communities: vec![Community {
                id: 0,
                members: vec!["demo.a".into(), "demo.b".into()],
                modularity: 0.5,
                size: 2,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"resolution\":1.0"));
        assert!(json.contains("\"communities\""));
        assert!(json.contains("\"size\":2"));
    }
}
