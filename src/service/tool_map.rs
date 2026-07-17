// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `tool-map` service: list MCP tools and their handler functions.

use serde::Serialize;

#[cfg(feature = "api-review")]
use crate::analysis::api_review::{ApiReviewer, ToolEntry};
#[cfg(feature = "api-review")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::to_api_error;
#[cfg(feature = "api-review")]
use crate::service::error::CodeNexusError;
#[cfg(feature = "api-review")]
use crate::service::project::resolve_project_id;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::forge;
#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::prelude::ApiError;

/// JSON-serializable tool-map output.
#[cfg(feature = "api-review")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolMapOutput {
    pub project: String,
    pub tool_map: Vec<ToolEntry>,
}

/// Core logic — resolves storage, runs tool_map, prints JSON.
#[cfg(feature = "api-review")]
fn tool_map_core(kit: &AsyncKit<AsyncReady>, project: &str) -> Result<(), CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let project_id = resolve_project_id(&*storage, project)?;
    let reviewer = ApiReviewer::new(&*storage);
    let tool_map: Vec<ToolEntry> = reviewer.tool_map(&project_id)?;
    let output = ToolMapOutput {
        project: project.to_string(),
        tool_map,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "api-review"))]
#[forge(
    name = "tool_map",
    version = "0.3.5",
    description = "List MCP tools and their handler functions.",
    cli = true
)]
async fn tool_map(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    tool_map_core(&kit, &project).map_err(|e| to_api_error(e, "tool_map_error"))?;
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
        let path = dir.path().join("svc_tool_map_testdb");
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
        let result = tool_map_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_with_tool() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Tool {id: 't1', project: 'demo', name: 'query', qualifiedName: 'query', filePath: '', toolType: 'mcp', parentQn: ''});").expect("create tool");
        storage.execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'query_handler', qualifiedName: 'query_handler', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'h1', target: 't1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");
        let result = tool_map_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn output_serializes_to_json() {
        let out = ToolMapOutput {
            project: "demo".into(),
            tool_map: vec![ToolEntry {
                tool_name: "query".into(),
                handler_id: "h1".into(),
                handler_name: "query_handler".into(),
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"tool_map\""));
        assert!(json.contains("\"query\""));
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    fn tool_map_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(tool_map("demo".to_string()));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[test]
    fn tool_map_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(tool_map("demo".to_string()));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
