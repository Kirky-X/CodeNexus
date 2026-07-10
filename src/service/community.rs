// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `community` service: detect communities in the CALLS graph via Louvain.

use serde::Serialize;
use serde_json::Value;

#[cfg(feature = "community")]
use crate::analysis::community::{Community, CommunityDetector};
use crate::cli::error::CliError;
use crate::kit::StorageKey;
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable community detection output.
#[cfg(feature = "community")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CommunityOutput {
    pub project: String,
    pub resolution: f64,
    pub communities: Vec<Community>,
}

/// Maps `CliError` to `ApiError` at the service boundary.
#[cfg(all(feature = "cli", feature = "community"))]
fn to_api_error(e: CliError) -> ApiError {
    match e {
        CliError::InvalidInput(msg) => ApiError::InvalidInput {
            message: msg,
            field: None,
            value: None,
        },
        other => ApiError::internal_error(format!("{other}"), "community_error"),
    }
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "community"))]
#[service_api(
    name = "community",
    version = "0.3.2",
    description = "Detect communities in the CALLS graph via Louvain modularity optimization.",
    cli = true
)]
async fn community(project: String, resolution: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage = kit
        .require::<StorageKey>()
        .map_err(|e| wrap_error("Failed to resolve storage capability", e))?;

    let mut detector = CommunityDetector::new(&*storage, &project);
    if !resolution.is_empty() {
        let res = resolution
            .parse::<f64>()
            .map_err(|_| ApiError::InvalidInput {
                message: format!("invalid resolution '{resolution}' (expected a positive number)"),
                field: Some("resolution".to_string()),
                value: Some(Value::String(resolution)),
            })?;
        detector = detector.with_resolution(res);
    }
    let communities = detector
        .detect_communities()
        .map_err(|e| to_api_error(e.into()))?;
    let output = CommunityOutput {
        project,
        resolution: detector.resolution(),
        communities,
    };
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(all(test, feature = "community"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, Kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_community_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn community_core(kit: &Kit, project: &str, resolution: Option<f64>) -> Result<(), CliError> {
        let storage = kit.require::<StorageKey>()?;
        let mut detector = CommunityDetector::new(&*storage, project);
        if let Some(res) = resolution {
            detector = detector.with_resolution(res);
        }
        let communities = detector.detect_communities()?;
        let output = CommunityOutput {
            project: project.to_string(),
            resolution: detector.resolution(),
            communities,
        };
        let json = serde_json::to_string(&output)?;
        println!("{json}");
        Ok(())
    }

    #[test]
    fn community_core_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let result = community_core(&kit, "demo", None);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn community_core_returns_communities() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create c");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge 1");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b', target: 'f_c', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge 2");
        let result = community_core(&kit, "demo", None);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn community_core_with_resolution() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");
        let result = community_core(&kit, "demo", Some(2.0));
        assert!(
            result.is_ok(),
            "run with resolution should succeed: {:?}",
            result.err()
        );
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
