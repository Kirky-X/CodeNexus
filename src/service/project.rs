// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Shared project resolution helper.
//!
//! [`resolve_project_id`] resolves a user-supplied project identifier (which
//! may be either a project **name** or a project **id**) to the canonical
//! project id stored in the graph. All analysis service commands should call
//! this before passing the project identifier to an analyzer so that users can
//! pass either `--project <NAME>` or `--project <ID>`.

use crate::service::error::CodeNexusError;
use crate::storage::capability::Storage;
use crate::storage::is_table_missing_error;

/// Resolves a project identifier (name or id) to the canonical project id.
///
/// Name lookup takes precedence over id lookup: if a project's name collides
/// with another project's id, the name match wins (consistent with `clean`).
///
/// # Errors
///
/// Returns [`CodeNexusError::ProjectNotFound`] if no project matches either
/// the name or the id, or if the Project table is missing (uninitialized DB)
/// — the latter is reported as `ProjectNotFound` rather than a raw storage
/// error so the user gets an actionable "project not found" message instead
/// of a confusing "Binder exception".
pub fn resolve_project_id(storage: &dyn Storage, project: &str) -> Result<String, CodeNexusError> {
    let projects = match storage.list_projects() {
        Ok(projects) => projects,
        Err(err) if is_table_missing_error(&err) => {
            return Err(CodeNexusError::ProjectNotFound(project.to_string()));
        }
        Err(err) => return Err(err.into()),
    };
    let project_id = projects
        .iter()
        .find(|p| p.name == project)
        .map(|p| p.id.clone())
        .or_else(|| {
            if projects.iter().any(|p| p.id == project) {
                Some(project.to_string())
            } else {
                None
            }
        });
    project_id.ok_or_else(|| CodeNexusError::ProjectNotFound(project.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_project_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> crate::kit::AsyncKit<crate::kit::AsyncReady> {
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
    fn resolve_by_name_returns_id() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "proj_abc", "my_project");
        let id = resolve_project_id(&*storage, "my_project").expect("should resolve");
        assert_eq!(id, "proj_abc");
    }

    #[test]
    fn resolve_by_id_returns_id() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "proj_abc", "my_project");
        let id = resolve_project_id(&*storage, "proj_abc").expect("should resolve");
        assert_eq!(id, "proj_abc");
    }

    #[test]
    fn resolve_unknown_returns_project_not_found() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "proj_abc", "my_project");
        let err = resolve_project_id(&*storage, "nonexistent").expect_err("should error");
        assert!(matches!(err, CodeNexusError::ProjectNotFound(_)));
    }

    #[test]
    fn resolve_name_takes_precedence_over_id() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        // Project A has name "p2", project B has id "p2"
        seed_project(&*storage, "p1", "p2");
        seed_project(&*storage, "p2", "other");
        let id = resolve_project_id(&*storage, "p2").expect("should resolve");
        // Name match wins → returns p1 (whose name is "p2")
        assert_eq!(id, "p1");
    }

    #[test]
    fn resolve_empty_db_returns_project_not_found() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        let err = resolve_project_id(&*storage, "anything").expect_err("should error");
        assert!(matches!(err, CodeNexusError::ProjectNotFound(_)));
    }

    #[test]
    fn resolve_when_project_table_dropped_returns_project_not_found() {
        // Simulates uninitialized/corrupted DB where Project table is gone.
        // Should return ProjectNotFound (not raw StorageError) so the user
        // gets an actionable message instead of "Binder exception".
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage
            .execute("DROP TABLE Project;")
            .expect("drop project table");
        let err = resolve_project_id(&*storage, "anything").expect_err("should error");
        assert!(
            matches!(err, CodeNexusError::ProjectNotFound(_)),
            "expected ProjectNotFound, got: {err:?}"
        );
    }
}
