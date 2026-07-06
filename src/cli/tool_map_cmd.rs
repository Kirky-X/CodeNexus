// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `api-tool-map` subcommand handler (T008, v0.2.0).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs an
//! [`ApiReviewer`](crate::analysis::api_review::ApiReviewer), and prints the
//! tool map as a JSON object.

use super::args::ToolMapArgs;
use super::error::Result;
use crate::analysis::api_review::{ApiReviewer, ToolEntry};
use crate::kit::{Kit, StorageKey};

/// Runs the `api-tool-map` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, runs [`ApiReviewer::tool_map`], and prints the result as a
/// JSON object `{ project, tool_map: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &ToolMapArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let tool_map: Vec<ToolEntry> = reviewer.tool_map(&args.project)?;
    let output = ToolMapOutput {
        project: args.project.clone(),
        tool_map,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable tool-map output.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ToolMapOutput {
    /// The queried project name.
    pub project: String,
    /// The list of tool entries.
    pub tool_map: Vec<ToolEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ToolMapArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tool_map_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> ToolMapArgs {
        ToolMapArgs {
            project: project.to_string(),
            db: db.to_string(),
        }
    }

    #[test]
    fn run_tool_map_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_tool_map_with_tool() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Tool {id: 't1', project: 'demo', name: 'query', qualifiedName: 'query', filePath: '', toolType: 'mcp', parentQn: ''});").expect("create tool");
        storage.execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'query_handler', qualifiedName: 'query_handler', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create handler");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'h1', target: 't1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn tool_map_output_serializes_to_json() {
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
