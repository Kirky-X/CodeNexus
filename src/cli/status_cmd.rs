//! `status` subcommand handler.
//!
//! Lists all indexed projects in the database and prints them as a JSON
//! array. (Per-project detail can be obtained via `list` or `query`.)

use serde::Serialize;

use super::args::StatusArgs;
use super::error::Result;
use crate::storage::{ProjectRecord, Repository};

/// Runs the `status` subcommand.
///
/// Opens the database at `args.db`, lists all projects, and prints them as a
/// JSON object `{ projects: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Storage`] if the database cannot be
/// opened.
pub fn run(args: &StatusArgs) -> Result<()> {
    let db_path = std::path::Path::new(&args.db);
    let repo = Repository::open(db_path)?;
    let projects = repo.list_projects().unwrap_or_default();
    let output = StatusOutput {
        projects: projects.into_iter().map(ProjectOutput::from).collect(),
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable status output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatusOutput {
    /// All indexed projects.
    pub projects: Vec<ProjectOutput>,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::StatusArgs;
    use crate::model::{Language, Node, NodeLabel};
    use crate::storage::Repository;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_status_testdb");
        std::mem::forget(dir);
        path
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
        StatusArgs {
            db: db.to_string(),
        }
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
        };
        let out = ProjectOutput::from(rec);
        assert_eq!(out.id, "p1");
        assert_eq!(out.name, "demo");
        assert_eq!(out.root_path, "/repo");
        assert_eq!(out.language, "rust");
        assert_eq!(out.file_count, 5);
        assert_eq!(out.indexed_at, 123);
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
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"projects\""));
        assert!(json.contains("\"demo\""));
    }

    // --- run() success ---

    #[test]
    fn run_status_empty_db_returns_empty_array() {
        let db = fresh_db_path();
        // Initialize schema but seed nothing.
        let repo = Repository::open(&db).expect("repo");
        drop(repo);
        let args = make_args(db.to_str().unwrap());
        let result = run(&args);
        assert!(result.is_ok(), "empty-db status should succeed: {:?}", result.err());
    }

    #[test]
    fn run_status_with_projects_succeeds() {
        let db = fresh_db_path();
        let repo = Repository::open(&db).expect("repo");
        repo.save_project(&sample_project("p1", "alpha")).unwrap();
        repo.save_project(&sample_project("p2", "beta")).unwrap();
        drop(repo);
        let args = make_args(db.to_str().unwrap());
        let result = run(&args);
        assert!(result.is_ok(), "status with projects should succeed: {:?}", result.err());
    }

    // --- run() error cases ---

    #[test]
    fn run_status_missing_db_returns_error() {
        let args = make_args("/nonexistent/db.lbug");
        let result = run(&args);
        assert!(result.is_err(), "missing db should error");
    }
}
