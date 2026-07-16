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
#[cfg(feature = "api-review")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::{to_api_error, wrap_error};
#[cfg(feature = "api-review")]
use crate::service::project::resolve_project_id;
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

/// Core logic — resolves storage, runs api_impact, returns structured output.
///
/// `endpoint` semantics:
/// - Non-empty → trace callers for that specific endpoint only.
/// - Empty string (`""`) → sentinel: trace callers for **all** endpoints
///   discovered via [`ApiReviewer::route_map`]. This lets the CLI expose
///   "analyse every endpoint" without an `Option<T>` parameter (sdforge 0.4.2
///   does not support `Option<T>` CLI args — see `SENTINEL_DEFAULTS` in
///   `main.rs` for how the CLI layer injects `""` when the user omits
///   `--endpoint`).
#[cfg(feature = "api-review")]
fn api_impact_core(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    endpoint: &str,
) -> Result<ApiImpactOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let project_id = resolve_project_id(&*storage, project)?;
    let reviewer = ApiReviewer::new(&*storage);
    let impact: Vec<ImpactEntry> = if endpoint.is_empty() {
        // Sentinel: empty endpoint → analyse all endpoints via route_map.
        let routes = reviewer.route_map(&project_id)?;
        let mut all: Vec<ImpactEntry> = Vec::new();
        for route in routes {
            let entries = reviewer.api_impact(&project_id, &route.path)?;
            all.extend(entries);
        }
        all
    } else {
        reviewer.api_impact(&project_id, endpoint)?
    };
    Ok(ApiImpactOutput {
        project: project.to_string(),
        endpoint: endpoint.to_string(),
        impact,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
///
/// `endpoint` is a `String` (not `Option<String>`) because sdforge 0.4.2's
/// `#[forge]` macro cannot parse `Option<T>` CLI args. An empty string `""`
/// is the sentinel for "all endpoints"; `main.rs` injects `""` via clap
/// `default_value` when the user omits `--endpoint`.
#[cfg(all(feature = "cli", feature = "api-review"))]
#[forge(
    name = "api_impact",
    version = "0.3.3",
    description = "Trace callers affected by changing an API endpoint.",
    cli = true
)]
async fn api_impact(project: String, endpoint: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = api_impact_core(&kit, &project, &endpoint)
        .map_err(|e| to_api_error(e, "api_impact_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "api-review"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use crate::storage::capability::Storage;
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

    fn seed_project(storage: &dyn Storage, id: &str, name: &str) {
        storage
            .execute(&format!(
                "CREATE (:Project {{id: '{id}', name: '{name}', rootPath: '/demo', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc'}});"
            ))
            .expect("create project");
    }

    #[test]
    fn core_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let result = api_impact_core(&kit, "demo", "/api/users");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_with_endpoint_and_handler() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        seed_project(&*storage, "demo", "demo");
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

    #[test]
    fn core_with_empty_endpoint_traverses_all_endpoints() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        // Endpoint 1: /api/users
        storage.execute("CREATE (:Endpoint {id: 'e1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', expectedSchema: '', parentQn: ''});").expect("create endpoint 1");
        storage.execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler 1");
        storage.execute("CREATE (:CodeRelation {id: 'he1', source: 'h1', target: 'e1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create handles edge 1");
        // Endpoint 2: /api/orders
        storage.execute("CREATE (:Endpoint {id: 'e2', project: 'demo', name: '/api/orders', qualifiedName: '/api/orders', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/orders', expectedSchema: '', parentQn: ''});").expect("create endpoint 2");
        storage.execute("CREATE (:Handler {id: 'h2', project: 'demo', name: 'list_orders', qualifiedName: 'list_orders', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler 2");
        storage.execute("CREATE (:CodeRelation {id: 'he2', source: 'h2', target: 'e2', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create handles edge 2");
        // Caller for handler 1
        storage.execute("CREATE (:Function {id: 'c1', project: 'demo', name: 'caller_a', qualifiedName: 'demo.caller_a', filePath: '/src/a.rs', startLine: 10, endLine: 15, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:CodeRelation {id: 'ce1', source: 'c1', target: 'h1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 10, project: 'demo'});").expect("create calls edge");

        let output = api_impact_core(&kit, "demo", "").expect("core should succeed");
        // Empty endpoint = sentinel for "all endpoints" → should include
        // impact from /api/users (caller_a). Without the sentinel logic,
        // api_impact finds no endpoint matching "" and returns empty.
        assert!(
            !output.impact.is_empty(),
            "empty endpoint should traverse all endpoints, got empty impact"
        );
        assert!(
            output
                .impact
                .iter()
                .any(|e| e.affected_caller == "caller_a"),
            "should include caller_a from /api/users endpoint"
        );
    }

    #[test]
    fn core_with_empty_endpoint_no_endpoints_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        // No endpoints created — empty endpoint should still succeed, just
        // with an empty impact list.
        let output = api_impact_core(&kit, "demo", "").expect("core should succeed");
        assert!(
            output.impact.is_empty(),
            "no endpoints → empty impact even with sentinel"
        );
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    fn api_impact_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
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

    #[serial_test::serial(kit_init)]
    #[test]
    fn api_impact_wrapper_succeeds_with_empty_endpoint() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        // Empty endpoint = sentinel for "all endpoints".
        let result = rt.block_on(api_impact("demo".to_string(), "".to_string()));
        assert!(
            result.is_ok(),
            "wrapper should succeed with empty endpoint: {:?}",
            result.err()
        );

        reset_kit_for_testing();
    }
}
