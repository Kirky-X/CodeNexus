// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Status command: list indexed projects with staleness check.

use std::path::Path;

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
use sdforge::service_api;

/// JSON-serializable status output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatusOutput {
    pub projects: Vec<ProjectOutput>,
}

/// JSON-serializable view of a project with staleness info.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProjectOutput {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub language: String,
    pub file_count: i64,
    pub indexed_at: i64,
    pub last_commit: String,
    pub current_head: String,
    pub stale: bool,
}

impl ProjectOutput {
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

/// Returns the current `HEAD` commit hash of the git repo at `root`, or an
/// empty string if `root` is not a git repo (or git is unavailable).
#[cfg(any(feature = "cli", test))]
pub(crate) fn git_head_commit(root: &Path) -> String {
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

/// Computes staleness: true if both commits are non-empty and differ.
#[cfg(any(feature = "cli", test))]
fn is_stale(last_commit: &str, current_head: &str) -> bool {
    !last_commit.is_empty() && !current_head.is_empty() && last_commit != current_head
}

/// Runs status against an injected Kit (testable core).
#[cfg(any(feature = "cli", test))]
pub fn run_status(kit: &AsyncKit<AsyncReady>) -> Result<StatusOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let projects = storage.list_projects()?;
    let output = StatusOutput {
        projects: projects
            .into_iter()
            .map(|p| {
                let current_head = git_head_commit(Path::new(&p.root_path));
                let stale = is_stale(&p.last_commit, &current_head);
                ProjectOutput::from_record(p, current_head, stale)
            })
            .collect(),
    };
    Ok(output)
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "status",
    version = "0.3.2",
    description = "List all indexed projects and check their staleness.",
    cli = true
)]
async fn status() -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_status(&kit).map_err(|e| to_api_error(e, "status_error"))?;
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
        let path = dir.path().join("svc_status_testdb");
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
    fn run_status_returns_empty_on_fresh_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_status(&kit).expect("run should succeed");
        assert!(output.projects.is_empty());
    }

    #[test]
    fn run_status_returns_project_with_staleness() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage
            .execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/nonexistent', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc123'});")
            .expect("create project");
        let output = run_status(&kit).expect("run should succeed");
        assert_eq!(output.projects.len(), 1);
        let p = &output.projects[0];
        assert_eq!(p.id, "p1");
        assert_eq!(p.name, "demo");
        assert_eq!(p.last_commit, "abc123");
        // rootPath is not a git repo, so current_head is empty, stale is false.
        assert!(p.current_head.is_empty(), "non-git dir should have empty head");
        assert!(!p.stale, "should not be stale when current_head is empty");
    }

    #[test]
    fn is_stale_returns_false_when_both_empty() {
        assert!(!is_stale("", ""));
    }

    #[test]
    fn is_stale_returns_false_when_last_commit_empty() {
        assert!(!is_stale("", "abc"));
    }

    #[test]
    fn is_stale_returns_false_when_current_head_empty() {
        assert!(!is_stale("abc", ""));
    }

    #[test]
    fn is_stale_returns_true_when_commits_differ() {
        assert!(is_stale("abc", "def"));
    }

    #[test]
    fn is_stale_returns_false_when_commits_match() {
        assert!(!is_stale("abc", "abc"));
    }

    #[test]
    fn git_head_commit_returns_empty_for_non_git_dir() {
        let dir = TempDir::new().unwrap();
        let head = git_head_commit(dir.path());
        assert!(head.is_empty(), "non-git dir should return empty string");
    }

    #[test]
    fn project_output_from_record_maps_all_fields() {
        let rec = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/demo".into(),
            language: "rust".into(),
            file_count: 42,
            indexed_at: 1234567890,
            last_commit: "deadbeef".into(),
        };
        let out = ProjectOutput::from_record(rec, "cafebabe".into(), true);
        assert_eq!(out.id, "p1");
        assert_eq!(out.name, "demo");
        assert_eq!(out.root_path, "/demo");
        assert_eq!(out.language, "rust");
        assert_eq!(out.file_count, 42);
        assert_eq!(out.indexed_at, 1234567890);
        assert_eq!(out.last_commit, "deadbeef");
        assert_eq!(out.current_head, "cafebabe");
        assert!(out.stale);
    }

    #[test]
    fn status_output_serializes_to_json() {
        let output = StatusOutput {
            projects: vec![ProjectOutput {
                id: "p1".into(),
                name: "demo".into(),
                root_path: "/demo".into(),
                language: "rust".into(),
                file_count: 10,
                indexed_at: 1000,
                last_commit: "abc".into(),
                current_head: "def".into(),
                stale: true,
            }],
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"projects\""));
        assert!(json.contains("\"stale\":true"));
        assert!(json.contains("\"current_head\":\"def\""));
    }

    // --- CLI wrapper tests ---

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn status_returns_error_when_kit_not_initialized() {
        use crate::service::runtime::{reset_kit_for_testing, KIT_TEST_MUTEX};
        let _lock = KIT_TEST_MUTEX.lock().unwrap();
        reset_kit_for_testing();
        let result = status().await;
        assert!(
            result.is_err(),
            "status should error when kit is not initialized"
        );
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn status_succeeds_when_kit_initialized() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing, KIT_TEST_MUTEX};
        let _lock = KIT_TEST_MUTEX.lock().unwrap();
        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit should succeed");
        let result = status().await;
        reset_kit_for_testing();
        assert!(result.is_ok(), "status should succeed: {:?}", result.err());
    }
}
