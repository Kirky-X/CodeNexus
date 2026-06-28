// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `status` subcommand handler.
//!
//! Lists all indexed projects in the database and prints them as a JSON
//! array. For each project, compares the stored `lastCommit` (captured at
//! index time) with the current `HEAD` of the repo at `rootPath` and flags
//! the project as `stale` when they differ (H9).

use std::path::Path;

use serde::Serialize;

use super::args::StatusArgs;
use super::error::Result;
use crate::kit::{Kit, StorageKey};
use crate::storage::ProjectRecord;

/// Runs the `status` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit` and lists all projects. For each project, checks whether the
/// repo's current `HEAD` matches the `lastCommit` stored at index time.
/// Prints the result as a JSON object `{ projects: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures (surfaces as an empty list via `unwrap_or_default`).
pub fn run(kit: &Kit, _args: &StatusArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let projects = storage.list_projects().unwrap_or_default();
    let output = StatusOutput {
        projects: projects
            .into_iter()
            .map(|p| {
                let current_head = git_head_commit(Path::new(&p.root_path));
                let stale = !p.last_commit.is_empty()
                    && !current_head.is_empty()
                    && p.last_commit != current_head;
                ProjectOutput::from_record(p, current_head, stale)
            })
            .collect(),
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Returns the current `HEAD` commit hash of the git repo at `root`, or an
/// empty string if `root` is not a git repo (or git is unavailable).
fn git_head_commit(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// JSON-serializable status output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatusOutput {
    /// All indexed projects.
    pub projects: Vec<ProjectOutput>,
}

/// JSON-serializable view of a project record with staleness info.
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
    /// Current `HEAD` commit hash of the repo at `root_path` (empty if not a
    /// git repo or git is unavailable).
    pub current_head: String,
    /// `true` when `last_commit` and `current_head` are both non-empty and
    /// differ, indicating the index is stale.
    pub stale: bool,
}

impl ProjectOutput {
    /// Builds a [`ProjectOutput`] from a [`ProjectRecord`] plus the computed
    /// `current_head` and `stale` flag.
    fn from_record(rec: ProjectRecord, current_head: String, stale: bool) -> Self {
        Self {
            id: rec.id,
            name: rec.name,
            root_path: rec.root_path,
            language: rec.language,
            file_count: rec.file_count,
            indexed_at: rec.indexed_at,
            last_commit: rec.last_commit,
            current_head,
            stale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::StatusArgs;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use crate::model::{Language, Node, NodeLabel};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_status_testdb");
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

    fn make_args(db: &str) -> StatusArgs {
        StatusArgs { db: db.to_string() }
    }

    // --- ProjectOutput ---

    #[test]
    fn project_output_from_record_with_staleness() {
        let rec = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/repo".into(),
            language: "rust".into(),
            file_count: 5,
            indexed_at: 123,
            last_commit: "abc123".into(),
        };
        let out = ProjectOutput::from_record(rec, "def456".into(), true);
        assert_eq!(out.id, "p1");
        assert_eq!(out.name, "demo");
        assert_eq!(out.root_path, "/repo");
        assert_eq!(out.language, "rust");
        assert_eq!(out.file_count, 5);
        assert_eq!(out.indexed_at, 123);
        assert_eq!(out.last_commit, "abc123");
        assert_eq!(out.current_head, "def456");
        assert!(out.stale);
    }

    #[test]
    fn project_output_not_stale_when_commits_match() {
        let rec = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/repo".into(),
            language: "rust".into(),
            file_count: 5,
            indexed_at: 123,
            last_commit: "abc123".into(),
        };
        let out = ProjectOutput::from_record(rec, "abc123".into(), false);
        assert!(!out.stale);
    }

    #[test]
    fn project_output_not_stale_when_last_commit_empty() {
        let rec = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/repo".into(),
            language: "rust".into(),
            file_count: 5,
            indexed_at: 123,
            last_commit: String::new(),
        };
        let out = ProjectOutput::from_record(rec, "def456".into(), false);
        assert!(!out.stale);
    }

    #[test]
    fn status_output_serializes_to_json() {
        let out = StatusOutput {
            projects: vec![ProjectOutput {
                id: "p1".into(),
                name: "demo".into(),
                root_path: "/".into(),
                language: "rust".into(),
                file_count: 1,
                indexed_at: 0,
                last_commit: "abc".into(),
                current_head: "abc".into(),
                stale: false,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"projects\""));
        assert!(json.contains("\"demo\""));
        assert!(json.contains("\"last_commit\""));
        assert!(json.contains("\"stale\""));
    }

    // --- run() success ---

    #[test]
    fn run_status_empty_db_returns_empty_array() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args(db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "empty-db status should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_status_with_projects_succeeds() {
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
            "status with projects should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---
    //
    // Note: `run_status_missing_db_returns_error` was removed because the
    // "missing db" error now surfaces at `build_kit` time (StorageModuleBuilder
    // fails to open the database), not at `run` time. This is covered by
    // `build_kit_invalid_db_path_returns_build_failed_error` in
    // `kit::bootstrap::tests`.
}
