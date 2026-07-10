// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `tool-map` service: list MCP tools and their handler functions.

use serde::Serialize;

#[cfg(feature = "api-review")]
use crate::analysis::api_review::{ApiReviewer, ToolEntry};
use crate::cli::error::CliError;
#[cfg(feature = "api-review")]
use crate::kit::{Kit, StorageKey};
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "api-review"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "cli", feature = "api-review"))]
use sdforge::service_api;

/// JSON-serializable tool-map output.
#[cfg(feature = "api-review")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolMapOutput {
    pub project: String,
    pub tool_map: Vec<ToolEntry>,
}

/// Core logic — resolves storage, runs tool_map, prints JSON.
#[cfg(feature = "api-review")]
fn tool_map_core(kit: &Kit, project: &str) -> Result<(), CliError> {
    let storage = kit.require::<StorageKey>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let tool_map: Vec<ToolEntry> = reviewer.tool_map(project)?;
    let output = ToolMapOutput {
        project: project.to_string(),
        tool_map,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Maps `CliError` to `ApiError` at the service boundary.
#[cfg(all(feature = "cli", feature = "api-review"))]
fn to_api_error(e: CliError) -> ApiError {
    match e {
        CliError::InvalidInput(msg) => ApiError::InvalidInput {
            message: msg,
            field: None,
            value: None,
        },
        other => ApiError::internal_error(format!("{other}"), "tool_map_error"),
    }
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "api-review"))]
#[service_api(
    name = "tool_map",
    version = "0.3.2",
    description = "List MCP tools and their handler functions.",
    cli = true
)]
async fn tool_map(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    tool_map_core(kit, &project).map_err(to_api_error)?;
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "api-review"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_tool_map_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    #[test]
    fn core_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let result = tool_map_core(&kit, "demo");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_with_tool() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
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
}
