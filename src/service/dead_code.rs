// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `dead-code` service: detect unreferenced functions in a project.

use serde::Serialize;

#[cfg(feature = "analysis")]
use crate::analysis::dead_code::{Confidence, DeadCodeDetector, DeadCodeEntry};
#[cfg(feature = "analysis")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "analysis"))]
use crate::service::error::to_api_error;
#[cfg(feature = "analysis")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "cli", feature = "analysis"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "analysis"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::service_api;

/// JSON-serializable dead-code output.
#[cfg(feature = "analysis")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeadCodeOutput {
    pub project: String,
    pub dead_code: Vec<DeadCodeEntry>,
}

/// Core logic — resolves storage, runs detection, prints JSON.
///
/// `entry` is a comma-separated list of extra entry-point patterns;
/// empty string means no extra patterns (defaults to `main`, `Main`, `__main__`).
#[cfg(feature = "analysis")]
fn dead_code_core(kit: &AsyncKit<AsyncReady>, project: &str, entry: &str) -> Result<(), CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let detector = DeadCodeDetector::new(&*storage);
    let mut entry_patterns: Vec<&str> = vec!["main", "Main", "__main__"];
    let extras: Vec<String> = if entry.is_empty() {
        Vec::new()
    } else {
        entry.split(',').map(|s| s.trim().to_string()).collect()
    };
    for e in &extras {
        entry_patterns.push(e.as_str());
    }
    let entries = detector.detect(project, &entry_patterns)?;
    let output = DeadCodeOutput {
        project: project.to_string(),
        dead_code: entries,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "analysis"))]
#[service_api(
    name = "dead_code",
    version = "0.3.2",
    description = "Detect unreferenced (dead) functions in a project.",
    cli = true
)]
async fn dead_code(project: String, entry: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    dead_code_core(&kit, &project, &entry).map_err(|e| to_api_error(e, "dead_code_error"))?;
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "analysis"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_dead_code_testdb");
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
    fn core_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let result = dead_code_core(&kit, "demo", "");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_returns_dead_function() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_foo', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create foo");
        let result = dead_code_core(&kit, "demo", "");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn core_with_custom_entry_patterns() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_main', project: 'demo', name: 'main', qualifiedName: 'demo.main', filePath: '/src/main.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create main");
        let result = dead_code_core(&kit, "demo", "custom_entry,other_entry");
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn output_serializes_to_json() {
        let out = DeadCodeOutput {
            project: "demo".into(),
            dead_code: vec![DeadCodeEntry {
                name: "foo".into(),
                qualified_name: "demo.foo".into(),
                file_path: "/src/lib.rs".into(),
                start_line: 1,
                language: "rust".into(),
                reason: "zero incoming CALLS edges".into(),
                confidence: Confidence::High,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"dead_code\""));
        assert!(json.contains("\"foo\""));
    }
}
