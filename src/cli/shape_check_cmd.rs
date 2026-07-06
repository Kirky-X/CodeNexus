// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `api-shape-check` subcommand handler (T008, v0.2.0).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs an
//! [`ApiReviewer`](crate::analysis::api_review::ApiReviewer), and prints the
//! schema violations as a JSON object.

use super::args::ShapeCheckArgs;
use super::error::Result;
use crate::analysis::api_review::{ApiReviewer, ShapeViolation};
use crate::kit::{Kit, StorageKey};

/// Runs the `api-shape-check` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, runs [`ApiReviewer::shape_check`], and prints the result as a
/// JSON object `{ project, violations: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &ShapeCheckArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let reviewer = ApiReviewer::new(&*storage);
    let violations: Vec<ShapeViolation> = reviewer.shape_check(&args.project)?;
    let output = ShapeCheckOutput {
        project: args.project.clone(),
        violations,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable shape-check output.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ShapeCheckOutput {
    /// The queried project name.
    pub project: String,
    /// The list of schema violations.
    pub violations: Vec<ShapeViolation>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ShapeCheckArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shape_check_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> ShapeCheckArgs {
        ShapeCheckArgs {
            project: project.to_string(),
            db: db.to_string(),
        }
    }

    #[test]
    fn run_shape_check_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_shape_check_with_endpoint() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Endpoint {id: 'e1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', expectedSchema: '{\"name\":\"string\"}', parentQn: ''});").expect("create endpoint");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn shape_check_output_serializes_to_json() {
        let out = ShapeCheckOutput {
            project: "demo".into(),
            violations: vec![ShapeViolation {
                endpoint: "/api/users".into(),
                expected_schema: r#"{"name":"string"}"#.into(),
                actual_schema: r#"{"name":"number"}"#.into(),
                severity: "mismatch".into(),
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"violations\""));
        assert!(json.contains("\"mismatch\""));
    }
}
