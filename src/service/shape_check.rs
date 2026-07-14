// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `shape-check` service: validate API endpoint schema consistency.

use serde::Serialize;

#[cfg(feature = "api-review")]
use crate::analysis::api_review::{ApiReviewer, ShapeViolation};
#[cfg(feature = "api-review")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::to_api_error;
#[cfg(feature = "api-review")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::forge;

/// JSON-serializable shape-check output.
#[cfg(feature = "api-review")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ShapeCheckOutput {
    pub project: String,
    pub violations: Vec<ShapeViolation>,
}

/// Core logic — resolves storage, runs shape_check, prints JSON.
#[cfg(feature = "api-review")]
fn shape_check_core(kit: &AsyncKit<AsyncReady>, project: &str) -> Result<(), CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let violations: Vec<ShapeViolation> = reviewer.shape_check(project)?;
    let output = ShapeCheckOutput {
        project: project.to_string(),
        violations,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "api-review"))]
#[forge(
    name = "shape_check",
    version = "0.3.2",
    description = "Validate API endpoint schema consistency.",
    cli = true
)]
async fn shape_check(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    shape_check_core(&kit, &project).map_err(|e| to_api_error(e, "shape_check_error"))?;
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "api-review"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_shape_check_testdb");
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
        let result = shape_check_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_with_endpoint() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Endpoint {id: 'e1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', expectedSchema: '{\"name\":\"string\"}', parentQn: ''});").expect("create endpoint");
        let result = shape_check_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn output_serializes_to_json() {
        let out = ShapeCheckOutput {
            project: "demo".into(),
            violations: vec![ShapeViolation {
                endpoint: "/api/users".into(),
                expected_schema: r#"{"name":"string"}"#.into(),
                actual_schema: r#"{"name":"number"}"#.into(),
                severity: "mismatch".into(),
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"violations\""));
        assert!(json.contains("\"mismatch\""));
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[test]
    fn shape_check_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(shape_check("demo".to_string()));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[test]
    fn shape_check_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(shape_check("demo".to_string()));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
