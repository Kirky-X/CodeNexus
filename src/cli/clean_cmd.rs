//! `clean` subcommand handler.
//!
//! Removes a project and all its nodes/edges from the database.

use serde::Serialize;

use super::args::CleanArgs;
use super::error::{CliError, Result};
use crate::storage::Repository;

/// Runs the `clean` subcommand.
///
/// Opens the database at `args.db`, looks up the project by name (falling
/// back to id), deletes it, and prints a JSON object with the deleted count.
///
/// # Errors
///
/// Returns [`CliError::ProjectNotFound`] if no project matches `args.project`.
/// Returns [`CliError::Storage`] for database failures.
pub fn run(args: &CleanArgs) -> Result<()> {
    let db_path = std::path::Path::new(&args.db);
    let repo = Repository::open(db_path)?;

    // Find the project by name first, then by id.
    let projects = repo.list_projects().unwrap_or_default();
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

    repo.delete_project(&project_id)?;
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
    use crate::model::{Language, Node, NodeLabel};
    use crate::storage::Repository;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_clean_testdb");
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
        let repo = Repository::open(&db).expect("repo");
        repo.save_project(&sample_project("p1", "alpha")).unwrap();
        repo.save_project(&sample_project("p2", "beta")).unwrap();
        drop(repo);
        let args = make_args("alpha", db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "clean by name should succeed: {:?}",
            result.err()
        );

        // Verify alpha is gone but beta remains.
        let repo = Repository::open(&db).expect("repo");
        let projects = repo.list_projects().unwrap_or_default();
        assert_eq!(projects.len(), 1, "only beta should remain");
        assert_eq!(projects[0].name, "beta");
    }

    #[test]
    fn run_clean_by_id_succeeds() {
        let db = fresh_db_path();
        let repo = Repository::open(&db).expect("repo");
        repo.save_project(&sample_project("p1", "alpha")).unwrap();
        drop(repo);
        let args = make_args("p1", db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "clean by id should succeed: {:?}",
            result.err()
        );

        let repo = Repository::open(&db).expect("repo");
        let projects = repo.list_projects().unwrap_or_default();
        assert!(projects.is_empty(), "project should be gone");
    }

    #[test]
    fn run_clean_last_project_succeeds() {
        let db = fresh_db_path();
        let repo = Repository::open(&db).expect("repo");
        repo.save_project(&sample_project("p1", "solo")).unwrap();
        drop(repo);
        let args = make_args("solo", db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "clean last project should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---

    #[test]
    fn run_clean_missing_project_returns_exit_code_1() {
        let db = fresh_db_path();
        let repo = Repository::open(&db).expect("repo");
        repo.save_project(&sample_project("p1", "alpha")).unwrap();
        drop(repo);
        let args = make_args("nonexistent", db.to_str().unwrap());
        let err = run(&args).expect_err("missing project should error");
        assert_eq!(err.exit_code(), 1, "ProjectNotFound → exit 1");
    }

    #[test]
    fn run_clean_missing_db_returns_error() {
        let args = make_args("demo", "/nonexistent/db.lbug");
        let result = run(&args);
        assert!(result.is_err(), "missing db should error");
    }

    #[test]
    fn run_clean_empty_db_returns_project_not_found() {
        let db = fresh_db_path();
        let repo = Repository::open(&db).expect("repo");
        drop(repo);
        let args = make_args("demo", db.to_str().unwrap());
        let err = run(&args).expect_err("empty db should error");
        assert_eq!(err.exit_code(), 1, "ProjectNotFound → exit 1");
    }
}
