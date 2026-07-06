// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `dead-code` subcommand handler (T005, v0.1.5).
//!
//! Resolves the [`Storage`](crate::storage::capability::Storage) capability
//! from the [`Kit`](crate::kit::Kit), constructs a
//! [`DeadCodeDetector`](crate::analysis::dead_code::DeadCodeDetector), and
//! prints the detected dead-code entries as a JSON array.

use super::args::DeadCodeArgs;
use super::error::Result;
use crate::analysis::dead_code::{DeadCodeDetector, DeadCodeEntry};
use crate::kit::{Kit, StorageKey};

/// Runs the `dead-code` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, runs [`DeadCodeDetector::detect`], and prints the result as a
/// JSON object `{ project, dead_code: [...] }`.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &DeadCodeArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let detector = DeadCodeDetector::new(&*storage);
    // Default entry-point patterns + any user-supplied patterns.
    let mut entry_patterns: Vec<&str> = vec!["main", "Main", "__main__"];
    if let Some(extra) = &args.entry {
        entry_patterns.extend(extra.iter().map(|s| s.as_str()));
    }
    let entries = detector.detect(&args.project, &entry_patterns)?;
    let output = DeadCodeOutput {
        project: args.project.clone(),
        dead_code: entries,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// JSON-serializable dead-code output.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct DeadCodeOutput {
    /// The queried project name.
    pub project: String,
    /// The list of dead-code entries.
    pub dead_code: Vec<DeadCodeEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::DeadCodeArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("dead_code_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> DeadCodeArgs {
        DeadCodeArgs {
            project: project.to_string(),
            db: db.to_string(),
            entry: None,
        }
    }

    #[test]
    fn run_dead_code_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_dead_code_returns_dead_function() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_foo', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create foo");
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_dead_code_with_custom_entry_patterns() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_main', project: 'demo', name: 'main', qualifiedName: 'demo.main', filePath: '/src/main.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create main");
        let args = DeadCodeArgs {
            project: "demo".to_string(),
            db: db.to_str().unwrap().to_string(),
            entry: Some(vec!["custom_entry".to_string()]),
        };
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn dead_code_output_serializes_to_json() {
        let out = DeadCodeOutput {
            project: "demo".into(),
            dead_code: vec![DeadCodeEntry {
                name: "foo".into(),
                qualified_name: "demo.foo".into(),
                file_path: "/src/lib.rs".into(),
                start_line: 1,
                language: "rust".into(),
                reason: "zero incoming CALLS edges".into(),
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"dead_code\""));
        assert!(json.contains("\"foo\""));
    }
}
