// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! List command: list all indexed projects.

use serde::Serialize;

#[cfg(any(feature = "cli", test))]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(any(feature = "cli", test))]
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(feature = "cli")]
use crate::service::runtime::kit;
use crate::storage::ProjectRecord;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::forge;

/// JSON-serializable view of a project record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProjectOutput {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub language: String,
    pub file_count: i64,
    pub indexed_at: i64,
    pub last_commit: String,
}

impl From<ProjectRecord> for ProjectOutput {
    fn from(p: ProjectRecord) -> Self {
        Self {
            id: p.id,
            name: p.name,
            root_path: p.root_path,
            language: p.language,
            file_count: p.file_count,
            indexed_at: p.indexed_at,
            last_commit: p.last_commit,
        }
    }
}

/// Runs list against an injected Kit (testable core).
#[cfg(any(feature = "cli", test))]
pub fn run_list(kit: &AsyncKit<AsyncReady>) -> Result<Vec<ProjectOutput>, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let projects = storage.list_projects()?;
    Ok(projects.into_iter().map(ProjectOutput::from).collect())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "list",
    version = "0.3.2",
    description = "List all indexed projects in the database.",
    cli = true
)]
async fn list() -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_list(&kit).map_err(|e| to_api_error(e, "list_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_list_testdb");
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
    fn run_list_returns_empty_on_fresh_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_list(&kit).expect("run should succeed");
        assert!(output.is_empty(), "fresh DB has no projects");
    }

    #[test]
    fn run_list_returns_projects_after_insert() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage
            .execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/demo', language: 'rust', fileCount: 10, indexedAt: 1000, lastCommit: 'abc123'});")
            .expect("create project");
        let output = run_list(&kit).expect("run should succeed");
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].id, "p1");
        assert_eq!(output[0].name, "demo");
        assert_eq!(output[0].root_path, "/demo");
        assert_eq!(output[0].language, "rust");
        assert_eq!(output[0].file_count, 10);
        assert_eq!(output[0].indexed_at, 1000);
        assert_eq!(output[0].last_commit, "abc123");
    }

    #[test]
    fn project_output_from_project_record_maps_all_fields() {
        let record = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/demo".into(),
            language: "rust".into(),
            file_count: 42,
            indexed_at: 1234567890,
            last_commit: "deadbeef".into(),
        };
        let output: ProjectOutput = record.into();
        assert_eq!(output.id, "p1");
        assert_eq!(output.name, "demo");
        assert_eq!(output.root_path, "/demo");
        assert_eq!(output.language, "rust");
        assert_eq!(output.file_count, 42);
        assert_eq!(output.indexed_at, 1234567890);
        assert_eq!(output.last_commit, "deadbeef");
    }

    #[test]
    fn project_output_serializes_to_json() {
        let output = ProjectOutput {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/demo".into(),
            language: "rust".into(),
            file_count: 10,
            indexed_at: 1000,
            last_commit: "abc123".into(),
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"id\":\"p1\""));
        assert!(json.contains("\"name\":\"demo\""));
        assert!(json.contains("\"root_path\":\"/demo\""));
        assert!(json.contains("\"language\":\"rust\""));
        assert!(json.contains("\"file_count\":10"));
        assert!(json.contains("\"indexed_at\":1000"));
        assert!(json.contains("\"last_commit\":\"abc123\""));
    }
}
