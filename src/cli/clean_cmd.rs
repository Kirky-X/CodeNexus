// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `clean` subcommand handler.
//!
//! Removes a project and all its nodes/edges from the database.

use serde::Serialize;

use super::args::CleanArgs;
use super::error::{CliError, Result};
use crate::kit::{Kit, StorageKey};

/// Runs the `clean` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, looks up the project by name (falling back to id), deletes it,
/// and prints a JSON object with the deleted count.
///
/// # Errors
///
/// Returns [`CliError::ProjectNotFound`] if no project matches `args.project`.
/// Returns [`CliError::Storage`] for database failures.
pub fn run(kit: &Kit, args: &CleanArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;

    // Find the project by name first, then by id.
    let projects = storage.list_projects().unwrap_or_default();
    let project_id = projects
        .iter()
        .find(|p| p.name == args.project)
        .map(|p| p.id.clone())
        .or_else(|| {
            // Fall back to exact id match.
            if projects.iter().any(|p| p.id == args.project) {
                Some(args.project.clone())
            } else {
                None
            }
        });

    let project_id = match project_id {
        Some(id) => id,
        None => return Err(CliError::ProjectNotFound(args.project.clone())),
    };

    storage.delete_project(&project_id)?;
    let output = CleanOutput {
        project: args.project.clone(),
        project_id,
        deleted: 1,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable clean-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CleanOutput {
    /// The project name (or id) that was supplied.
    pub project: String,
    /// The resolved project id that was deleted.
    pub project_id: String,
    /// Number of projects deleted (always 1 on success).
    pub deleted: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::CleanArgs;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use crate::model::{Language, Node, NodeLabel};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_clean_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// Builds a sample Project node.
    fn sample_project(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Project, name, name)
            .id(id)
            .language(Language::Rust)
            .properties(serde_json::json!({
                "rootPath": "/repo/".to_string() + name,
                "fileCount": 10,
                "indexedAt": 1_700_000_000,
            }))
            .build()
    }

    fn make_args(project: &str, db: &str) -> CleanArgs {
        CleanArgs {
            project: project.to_string(),
            db: db.to_string(),
        }
    }

    // --- CleanOutput ---

    #[test]
    fn clean_output_serializes_to_json() {
        let out = CleanOutput {
            project: "demo".into(),
            project_id: "p1".into(),
            deleted: 1,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"deleted\":1"));
    }

    // --- run() success ---

    #[test]
    fn run_clean_by_name_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .save_project(&sample_project("p1", "alpha"))
            .unwrap();
        storage.save_project(&sample_project("p2", "beta")).unwrap();
        let args = make_args("alpha", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "clean by name should succeed: {:?}",
            result.err()
        );

        // Verify alpha is gone but beta remains (same Kit's storage).
        let projects = storage.list_projects().unwrap_or_default();
        assert_eq!(projects.len(), 1, "only beta should remain");
        assert_eq!(projects[0].name, "beta");
    }

    #[test]
    fn run_clean_by_id_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .save_project(&sample_project("p1", "alpha"))
            .unwrap();
        let args = make_args("p1", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "clean by id should succeed: {:?}",
            result.err()
        );

        let projects = storage.list_projects().unwrap_or_default();
        assert!(projects.is_empty(), "project should be gone");
    }

    #[test]
    fn run_clean_last_project_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.save_project(&sample_project("p1", "solo")).unwrap();
        let args = make_args("solo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "clean last project should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---

    #[test]
    fn run_clean_missing_project_returns_exit_code_2() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .save_project(&sample_project("p1", "alpha"))
            .unwrap();
        let args = make_args("nonexistent", db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("missing project should error");
        assert_eq!(err.exit_code(), 2, "ProjectNotFound → exit 2");
    }

    #[test]
    fn run_clean_empty_db_returns_project_not_found() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("demo", db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("empty db should error");
        assert_eq!(err.exit_code(), 2, "ProjectNotFound → exit 2");
    }

    // Note: `run_clean_missing_db_returns_error` was removed because the
    // "missing db" error now surfaces at `build_kit` time, not at `run` time.
    // Covered by `build_kit_invalid_db_path_returns_build_failed_error` in
    // `kit::bootstrap::tests`.
}
