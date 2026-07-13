// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `community` service: detect communities in the CALLS graph via Louvain.

use serde::Serialize;
use serde_json::Value;

#[cfg(feature = "community")]
use crate::analysis::community::{Community, CommunityDetector};
#[cfg(all(feature = "community", any(feature = "cli", test)))]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "community", any(feature = "cli", test)))]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "community"))]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(all(feature = "cli", feature = "community"))]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::forge;

/// JSON-serializable community detection output.
#[cfg(feature = "community")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CommunityOutput {
    pub project: String,
    pub resolution: f64,
    pub communities: Vec<Community>,
}

/// Runs community detection against an injected Kit (testable core).
#[cfg(all(feature = "community", any(feature = "cli", test)))]
pub fn run_community(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    resolution: Option<f64>,
) -> Result<CommunityOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let mut detector = CommunityDetector::new(&*storage, project);
    if let Some(res) = resolution {
        detector = detector.with_resolution(res);
    }
    let communities = detector.detect_communities()?;
    Ok(CommunityOutput {
        project: project.to_string(),
        resolution: detector.resolution(),
        communities,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "community"))]
#[forge(
    name = "community",
    version = "0.3.2",
    description = "Detect communities in the CALLS graph via Louvain modularity optimization.",
    cli = true
)]
async fn community(project: String, resolution: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let res = if resolution.is_empty() {
        None
    } else {
        Some(resolution.parse::<f64>().map_err(|_| ApiError::InvalidInput {
            message: format!("invalid resolution '{resolution}' (expected a positive number)"),
            field: Some("resolution".to_string()),
            value: Some(Value::String(resolution)),
        })?)
    };
    let output = run_community(&kit, &project, res)
        .map_err(|e| to_api_error(e, "community_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(all(test, feature = "community"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_community_testdb");
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
    fn run_community_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_community(&kit, "demo", None).expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert!(output.communities.is_empty(), "no communities on empty DB");
    }

    #[test]
    fn run_community_detects_communities_with_edges() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create c");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge 1");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b', target: 'f_c', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge 2");
        let output = run_community(&kit, "demo", None).expect("run should succeed");
        assert_eq!(output.project, "demo");
    }

    #[test]
    fn run_community_with_custom_resolution() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");
        let output = run_community(&kit, "demo", Some(2.0)).expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert_eq!(output.resolution, 2.0);
    }

    #[test]
    fn run_community_unknown_project_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'other', name: 'a', qualifiedName: 'other.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        let output = run_community(&kit, "demo", None).expect("run should succeed");
        assert!(output.communities.is_empty(), "no communities for absent project");
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

    // ===== run_community: additional edge cases =====

    #[test]
    fn run_community_with_two_separate_clusters() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        // Cluster A: a1 → a2 → a3
        storage.execute("CREATE (:Function {id: 'f_a1', project: 'demo', name: 'a1', qualifiedName: 'demo.a1', filePath: '/src/a1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a1");
        storage.execute("CREATE (:Function {id: 'f_a2', project: 'demo', name: 'a2', qualifiedName: 'demo.a2', filePath: '/src/a2.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a2");
        storage.execute("CREATE (:Function {id: 'f_a3', project: 'demo', name: 'a3', qualifiedName: 'demo.a3', filePath: '/src/a3.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a3");
        // Cluster B: b1 → b2 → b3
        storage.execute("CREATE (:Function {id: 'f_b1', project: 'demo', name: 'b1', qualifiedName: 'demo.b1', filePath: '/src/b1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b1");
        storage.execute("CREATE (:Function {id: 'f_b2', project: 'demo', name: 'b2', qualifiedName: 'demo.b2', filePath: '/src/b2.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b2");
        storage.execute("CREATE (:Function {id: 'f_b3', project: 'demo', name: 'b3', qualifiedName: 'demo.b3', filePath: '/src/b3.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b3");
        // Internal edges
        storage.execute("CREATE (:CodeRelation {id: 'e_a1', source: 'f_a1', target: 'f_a2', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("a1->a2");
        storage.execute("CREATE (:CodeRelation {id: 'e_a2', source: 'f_a2', target: 'f_a3', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("a2->a3");
        storage.execute("CREATE (:CodeRelation {id: 'e_b1', source: 'f_b1', target: 'f_b2', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("b1->b2");
        storage.execute("CREATE (:CodeRelation {id: 'e_b2', source: 'f_b2', target: 'f_b3', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("b2->b3");

        let output = run_community(&kit, "demo", None).expect("run should succeed");
        assert_eq!(output.project, "demo");
    }

    #[test]
    fn run_community_with_low_resolution() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");

        let output = run_community(&kit, "demo", Some(0.5)).expect("run should succeed");
        assert_eq!(output.resolution, 0.5);
    }

    #[test]
    fn run_community_serializes_output_with_communities() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");

        let output = run_community(&kit, "demo", None).expect("run should succeed");
        let json = serde_json::to_string(&output).expect("should serialize");
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"resolution\""));
        assert!(json.contains("\"communities\""));
    }
}
