// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `api-impact` service: trace callers affected by changing an endpoint.

use serde::Serialize;

#[cfg(feature = "api-review")]
use crate::analysis::api_review::{ApiReviewer, ImpactEntry};
#[cfg(feature = "api-review")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::to_api_error;
#[cfg(feature = "api-review")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::forge;
#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::prelude::ApiError;

/// JSON-serializable api-impact output.
#[cfg(feature = "api-review")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ApiImpactOutput {
    pub project: String,
    pub endpoint: String,
    pub impact: Vec<ImpactEntry>,
}

/// Core logic — resolves storage, runs api_impact, prints JSON.
#[cfg(feature = "api-review")]
fn api_impact_core(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    endpoint: &str,
) -> Result<(), CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let impact: Vec<ImpactEntry> = reviewer.api_impact(project, endpoint)?;
    let output = ApiImpactOutput {
        project: project.to_string(),
        endpoint: endpoint.to_string(),
        impact,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "api-review"))]
#[forge(
    name = "api_impact",
    version = "0.3.2",
    description = "Trace callers affected by changing an API endpoint.",
    cli = true
)]
async fn api_impact(project: String, endpoint: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    api_impact_core(&kit, &project, &endpoint).map_err(|e| to_api_error(e, "api_impact_error"))?;
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "api-review"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_api_impact_testdb");
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
        let result = api_impact_core(&kit, "demo", "/api/users");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_with_endpoint_and_handler() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Endpoint {id: 'e1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', expectedSchema: '', parentQn: ''});").expect("create endpoint");
        storage.execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:CodeRelation {id: 'he1', source: 'h1', target: 'e1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create handles edge");
        let result = api_impact_core(&kit, "demo", "/api/users");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn output_serializes_to_json() {
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

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    fn api_impact_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(api_impact("demo".to_string(), "/api/users".to_string()));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[test]
    fn api_impact_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(api_impact("demo".to_string(), "/api/users".to_string()));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
