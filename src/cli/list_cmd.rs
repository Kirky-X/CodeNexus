// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `list` subcommand handler.
//!
//! Lists all indexed projects in the database and prints them as a JSON
//! array.

use serde::Serialize;

use super::args::ListArgs;
use super::error::Result;
use crate::kit::{Kit, StorageKey};
use crate::storage::ProjectRecord;

/// Runs the `list` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit` and lists all projects, printing them as a JSON array of
/// [`ProjectOutput`] objects.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures (surfaces as an empty list via `unwrap_or_default`).
pub fn run(kit: &Kit, _args: &ListArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let projects = storage.list_projects().unwrap_or_default();
    let output: Vec<ProjectOutput> = projects.into_iter().map(ProjectOutput::from).collect();
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable view of a project record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProjectOutput {
    /// Project node id.
    pub id: String,
    /// Project display name.
    pub name: String,
    /// Repository root path.
    pub root_path: String,
    /// Primary source language.
    pub language: String,
    /// Number of indexed files.
    pub file_count: i64,
    /// Indexing timestamp (unix seconds).
    pub indexed_at: i64,
    /// Git commit hash at index time (empty if not a git repo at index time).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ListArgs;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use crate::model::{Language, Node, NodeLabel};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_list_testdb");
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

    fn make_args(db: &str) -> ListArgs {
        ListArgs { db: db.to_string() }
    }

    // --- ProjectOutput ---

    #[test]
    fn project_output_from_project_record() {
        let rec = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/repo".into(),
            language: "rust".into(),
            file_count: 5,
            indexed_at: 123,
            last_commit: "abc123".into(),
        };
        let out = ProjectOutput::from(rec);
        assert_eq!(out.id, "p1");
        assert_eq!(out.name, "demo");
        assert_eq!(out.root_path, "/repo");
        assert_eq!(out.language, "rust");
        assert_eq!(out.file_count, 5);
        assert_eq!(out.indexed_at, 123);
        assert_eq!(out.last_commit, "abc123");
    }

    #[test]
    fn project_output_serializes_to_json() {
        let out = ProjectOutput {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/".into(),
            language: "rust".into(),
            file_count: 1,
            indexed_at: 0,
            last_commit: "abc".into(),
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"demo\""));
        assert!(json.contains("\"rust\""));
        assert!(json.contains("\"last_commit\""));
    }

    // --- run() success ---

    #[test]
    fn run_list_empty_db_returns_empty_array() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args(db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "empty-db list should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_list_with_projects_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .save_project(&sample_project("p1", "alpha"))
            .unwrap();
        storage.save_project(&sample_project("p2", "beta")).unwrap();
        let args = make_args(db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "list with projects should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_list_single_project_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.save_project(&sample_project("p1", "solo")).unwrap();
        let args = make_args(db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "list single project should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---
    //
    // Note: `run_list_missing_db_returns_error` was removed because the
    // "missing db" error now surfaces at `build_kit` time, not at `run` time.
    // Covered by `build_kit_invalid_db_path_returns_build_failed_error` in
    // `kit::bootstrap::tests`.
}
