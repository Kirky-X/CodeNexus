// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Clean command: remove a project by name or id.

use serde::Serialize;

#[cfg(any(feature = "cli", test))]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(any(feature = "cli", test))]
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(any(feature = "cli", test))]
use crate::service::project::resolve_project_id;
#[cfg(feature = "cli")]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// JSON-serializable clean-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CleanOutput {
    pub project: String,
    pub project_id: String,
    pub deleted: usize,
}

/// Runs clean against an injected Kit (testable core).
#[cfg(any(feature = "cli", test))]
pub fn run_clean(kit: &AsyncKit<AsyncReady>, project: &str) -> Result<CleanOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let project_id = resolve_project_id(&*storage, project)?;
    storage.delete_project(&project_id)?;
    Ok(CleanOutput {
        project: project.to_string(),
        project_id,
        deleted: 1,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "clean",
    version = "0.3.3",
    description = "Remove a project and its index by name or id.",
    cli = true
)]
async fn clean(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_clean(&kit, &project).map_err(|e| to_api_error(e, "clean_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use crate::storage::capability::Storage;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_clean_testdb");
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
    fn run_clean_by_name_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "p1", "demo");
        let output = run_clean(&kit, "demo").expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert_eq!(output.project_id, "p1");
        assert_eq!(output.deleted, 1);
        assert!(storage.list_projects().unwrap().is_empty());
    }

    #[test]
    fn run_clean_by_id_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "p1", "demo");
        let output = run_clean(&kit, "p1").expect("run should succeed");
        assert_eq!(output.project, "p1");
        assert_eq!(output.project_id, "p1");
        assert_eq!(output.deleted, 1);
        assert!(storage.list_projects().unwrap().is_empty());
    }

    #[test]
    fn run_clean_unknown_project_returns_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_clean(&kit, "nonexistent").expect_err("unknown project should error");
        assert!(matches!(err, CodeNexusError::ProjectNotFound(_)));
    }

    #[test]
    fn run_clean_name_takes_precedence_over_id() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "p1", "p2");
        seed_project(&*storage, "p2", "other");
        let output = run_clean(&kit, "p2").expect("run should succeed");
        assert_eq!(output.project_id, "p1");
        let remaining = storage.list_projects().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "p2");
    }

    #[test]
    fn clean_output_serializes_to_json() {
        let output = CleanOutput {
            project: "demo".into(),
            project_id: "p1".into(),
            deleted: 1,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"project_id\":\"p1\""));
        assert!(json.contains("\"deleted\":1"));
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn clean_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "p1", "demo");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(clean("demo".to_string()));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn clean_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(clean("demo".to_string()));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
