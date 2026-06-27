// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `query` subcommand handler (PRD §4.4).
//!
//! Calls [`QueryEngine::cypher`] and prints the [`QueryResult`] as a JSON
//! array of row objects (column name → value).

use serde::Serialize;

use super::args::QueryArgs;
use super::error::Result;
use crate::kit::{Kit, QueryKey};
use crate::query::QueryResult;

/// Runs the `query` subcommand.
///
/// Resolves the [`QueryEngine`](crate::query::capability::QueryEngine)
/// capability from `kit`, executes `args.cypher`, and prints the result as a
/// JSON object with `columns` and `rows` fields.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Query capability is
/// not registered. Returns [`crate::cli::error::CliError::Query`] for invalid
/// or failing Cypher.
pub fn run(kit: &Kit, args: &QueryArgs) -> Result<()> {
    let query = kit.require::<QueryKey>()?;
    let result = query.cypher(&args.cypher)?;
    let output = QueryOutput::from(result);
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable view of [`QueryResult`].
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QueryOutput {
    /// Column names returned by the query.
    pub columns: Vec<String>,
    /// Rows, each a vector of JSON values.
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Wall-clock execution duration in milliseconds.
    pub duration_ms: u64,
}

impl From<QueryResult> for QueryOutput {
    fn from(r: QueryResult) -> Self {
        Self {
            columns: r.columns,
            rows: r.rows,
            duration_ms: r.duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::QueryArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_query_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// Seeds the database with a small fixture (one Project + two Functions).
    fn seed(kit: &Kit) {
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0});").expect("create project");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'main', qualifiedName: 'demo.main', filePath: '/src/main.rs', startLine: 1, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f1");
        storage.execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'helper', qualifiedName: 'demo.helper', filePath: '/src/main.rs', startLine: 11, endLine: 20, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f2");
    }

    fn make_args(cypher: &str, db: &str) -> QueryArgs {
        QueryArgs {
            cypher: cypher.to_string(),
            db: db.to_string(),
            project: None,
        }
    }

    // --- QueryOutput ---

    #[test]
    fn query_output_from_query_result_copies_fields() {
        let r = QueryResult {
            columns: vec!["name".to_string()],
            rows: vec![vec![serde_json::json!("alpha")]],
            duration_ms: 5,
        };
        let out = QueryOutput::from(r);
        assert_eq!(out.columns, vec!["name"]);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0][0], serde_json::json!("alpha"));
        assert_eq!(out.duration_ms, 5);
    }

    #[test]
    fn query_output_serializes_to_json() {
        let out = QueryOutput {
            columns: vec!["a".into()],
            rows: vec![vec![serde_json::json!(1)]],
            duration_ms: 0,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"columns\""));
        assert!(json.contains("\"rows\""));
        assert!(json.contains("\"duration_ms\""));
    }

    // --- run() success ---

    #[test]
    fn run_executes_count_query() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed(&kit);
        let args = make_args(
            "MATCH (f:Function) RETURN count(f) AS cnt;",
            db.to_str().unwrap(),
        );
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_executes_return_query() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed(&kit);
        let args = make_args(
            "MATCH (f:Function) RETURN f.name AS name ORDER BY f.name;",
            db.to_str().unwrap(),
        );
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_empty_query_result_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed(&kit);
        let args = make_args(
            "MATCH (f:Function) WHERE f.name = 'nonexistent' RETURN f.name;",
            db.to_str().unwrap(),
        );
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "empty result should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---

    #[test]
    fn run_invalid_cypher_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed(&kit);
        let args = make_args("MATCH (a RETURN a;", db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("invalid cypher should error");
        assert_eq!(err.exit_code(), 2, "query errors → exit 2");
    }

    #[test]
    fn run_with_project_filter_succeeds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed(&kit);
        let args = QueryArgs {
            cypher: "MATCH (f:Function) RETURN f.name AS name;".to_string(),
            db: db.to_str().unwrap().to_string(),
            project: Some("demo".to_string()),
        };
        let result = run(&kit, &args);
        assert!(
            result.is_ok(),
            "run with project should succeed: {:?}",
            result.err()
        );
    }

    // Note: `run_missing_db_returns_error` was removed because the "missing db"
    // error now surfaces at `build_kit` time, not at `run` time. Covered by
    // `build_kit_invalid_db_path_returns_build_failed_error` in
    // `kit::bootstrap::tests`.
}
