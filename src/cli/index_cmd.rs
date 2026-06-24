//! `index` subcommand handler (PRD §4.1.3).
//!
//! Calls [`IndexFacade::index`] and prints the resulting [`IndexResult`] as
//! JSON to stdout. Errors are surfaced via [`CliError`] so `main.rs` can map
//! them to the correct exit code.

use std::path::Path;

use serde::Serialize;

use super::args::IndexArgs;
use super::error::Result;
use crate::index::IndexFacade;
use crate::index::IndexResult;

/// Runs the `index` subcommand.
///
/// Opens (or creates) the database at `args.db`, indexes `args.path` under
/// the project name `args.name`, and prints the [`IndexResult`] as JSON.
///
/// # Errors
///
/// Returns [`CliError::Index`] for path-not-found / database / parse errors.
/// The wrapped [`IndexError`] carries the correct exit code.
pub fn run(args: &IndexArgs) -> Result<()> {
    let path = Path::new(&args.path);
    let db_path = Path::new(&args.db);
    let facade = IndexFacade::new(db_path)?;
    let result = facade.index(path, &args.name, args.force)?;
    let output = IndexOutput::from(result);
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable view of [`IndexResult`] (PRD §4.1.3 output table).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IndexOutput {
    /// Project id (UUIDv7).
    pub project_id: String,
    /// Number of files actually parsed.
    pub files_indexed: usize,
    /// Number of files skipped (hash matched).
    pub files_skipped: usize,
    /// Number of nodes created.
    pub nodes_created: usize,
    /// Number of edges created.
    pub edges_created: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

impl From<IndexResult> for IndexOutput {
    fn from(r: IndexResult) -> Self {
        Self {
            project_id: r.project_id,
            files_indexed: r.files_indexed,
            files_skipped: r.files_skipped,
            nodes_created: r.nodes_created,
            edges_created: r.edges_created,
            duration_ms: r.duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::IndexArgs;
    use std::fs;
    use tempfile::TempDir;

    /// Writes a file at `dir/rel` (creating parent directories as needed).
    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Returns a fresh on-disk database path inside a temp dir.
    ///
    /// The TempDir is leaked intentionally so the database files survive for
    /// the test's lifetime (LadybugDB keeps file handles open).
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds an `IndexArgs` pointing at `path`/`name`/`db`.
    fn make_args(path: &str, name: &str, db: &str) -> IndexArgs {
        IndexArgs {
            path: path.to_string(),
            name: name.to_string(),
            db: db.to_string(),
            force: false,
            lsp: false,
            embed: false,
        }
    }

    // --- IndexOutput ---

    #[test]
    fn index_output_from_index_result_copies_fields() {
        let r = IndexResult::new("proj_1", 10, 5, 100, 50, 1234);
        let out = IndexOutput::from(r);
        assert_eq!(out.project_id, "proj_1");
        assert_eq!(out.files_indexed, 10);
        assert_eq!(out.files_skipped, 5);
        assert_eq!(out.nodes_created, 100);
        assert_eq!(out.edges_created, 50);
        assert_eq!(out.duration_ms, 1234);
    }

    #[test]
    fn index_output_serializes_to_json() {
        let out = IndexOutput {
            project_id: "p1".into(),
            files_indexed: 1,
            files_skipped: 0,
            nodes_created: 2,
            edges_created: 3,
            duration_ms: 4,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project_id\":\"p1\""));
        assert!(json.contains("\"files_indexed\":1"));
        assert!(json.contains("\"duration_ms\":4"));
    }

    // --- run() success ---

    #[test]
    fn run_indexes_rust_file_and_prints_json() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() { helper(); }\nfn helper() {}\n");
        let db = fresh_db_path();
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            db.to_str().unwrap(),
        );

        // run() prints to stdout; we just verify it returns Ok.
        let result = run(&args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_indexes_multiple_files() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        write_file(tmp.path(), "b.rs", "fn b() {}\n");
        write_file(tmp.path(), "c.rs", "fn c() {}\n");
        let db = fresh_db_path();
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "multi",
            db.to_str().unwrap(),
        );

        let result = run(&args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_with_force_re_indexes() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();

        // First index.
        let args1 = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            db.to_str().unwrap(),
        );
        assert!(run(&args1).is_ok());

        // Second index with force.
        let args2 = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: true,
            lsp: false,
            embed: false,
        };
        let result = run(&args2);
        assert!(result.is_ok(), "force run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_empty_directory_succeeds() {
        let tmp = TempDir::new().unwrap();
        let db = fresh_db_path();
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "empty",
            db.to_str().unwrap(),
        );
        let result = run(&args);
        assert!(result.is_ok(), "empty dir should succeed: {:?}", result.err());
    }

    // --- run() error cases ---

    #[test]
    fn run_path_not_found_returns_exit_code_1() {
        let db = fresh_db_path();
        let args = make_args("/nonexistent/path/xyz", "demo", db.to_str().unwrap());
        let err = run(&args).expect_err("path not found should error");
        assert_eq!(err.exit_code(), 1, "PRD §4.1.6: path not found → exit 1");
    }

    #[test]
    fn run_invalid_db_path_returns_error() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        // Use a path inside a file (not a directory) to force a storage error.
        let bad_db = tmp.path().join("file_not_dir");
        fs::write(&bad_db, "not a directory").unwrap();
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            bad_db.to_str().unwrap(),
        );
        let result = run(&args);
        assert!(result.is_err(), "bad db path should error");
    }

    // --- lsp / embed flags are accepted but no-ops for now ---

    #[test]
    fn run_with_lsp_flag_succeeds() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: true,
            embed: false,
        };
        assert!(run(&args).is_ok());
    }

    #[test]
    fn run_with_embed_flag_succeeds() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        let db = fresh_db_path();
        let args = IndexArgs {
            path: tmp.path().to_str().unwrap().to_string(),
            name: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            force: false,
            lsp: false,
            embed: true,
        };
        assert!(run(&args).is_ok());
    }
}
