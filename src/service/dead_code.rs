// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `dead-code` service: detect unreferenced functions in a project.

use serde::Serialize;

#[cfg(feature = "analysis")]
use crate::analysis::dead_code::{DeadCodeConfig, DeadCodeDetector, DeadCodeEntry};
#[cfg(feature = "analysis")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(test, feature = "cli", feature = "analysis"))]
use crate::model::EdgeType;
#[cfg(all(feature = "cli", feature = "analysis"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "analysis"))]
use crate::service::error::to_api_error;
#[cfg(feature = "analysis")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "analysis"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::forge;

/// JSON-serializable dead-code output.
#[cfg(feature = "analysis")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeadCodeOutput {
    pub project: String,
    pub dead_code: Vec<DeadCodeEntry>,
}

/// Builds a [`DeadCodeConfig`] from CLI parameters.
///
/// `edge_types` is a comma-separated list of UPPERCASE DDL edge type strings
/// (e.g. `"CALLS,USAGE,TESTS"`). An empty string means "use the default edge
/// types" from [`DeadCodeConfig::default`].
#[cfg(feature = "analysis")]
fn build_dead_code_config(
    check_exported: bool,
    check_ffi: bool,
    edge_types: &str,
) -> DeadCodeConfig {
    let default = DeadCodeConfig::default();
    let final_edge_types =
        crate::model::edge_type::parse_edge_type_list(edge_types, &default.edge_types);
    DeadCodeConfig {
        check_exported,
        check_ffi,
        edge_types: final_edge_types,
        ..default
    }
}

/// Runs dead-code detection with config and returns the output (testable core).
///
/// `entry` is a comma-separated list of extra entry-point patterns;
/// empty string means no extra patterns (defaults to `main`, `Main`, `__main__`).
#[cfg(feature = "analysis")]
pub fn run_dead_code(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    entry: &str,
    check_exported: bool,
    check_ffi: bool,
    edge_types: &str,
) -> Result<DeadCodeOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let config = build_dead_code_config(check_exported, check_ffi, edge_types);
    let detector = DeadCodeDetector::with_config(&*storage, config);
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
    Ok(DeadCodeOutput {
        project: project.to_string(),
        dead_code: entries,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "analysis"))]
#[forge(
    name = "dead_code",
    version = "0.3.2",
    description = "Detect unreferenced (dead) functions in a project.",
    cli = true
)]
async fn dead_code(
    project: String,
    entry: String,
    check_exported: bool,
    check_ffi: bool,
    edge_types: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_dead_code(&kit, &project, &entry, check_exported, check_ffi, &edge_types)
        .map_err(|e| to_api_error(e, "dead_code_error"))?;
    let json = serde_json::to_string(&output)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "dead_code_error"))?;
    println!("{json}");
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "analysis"))]
mod tests {
    use super::*;
    use crate::analysis::dead_code::Confidence;
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
    fn run_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let result = run_dead_code(&kit, "demo", "", true, true, "");
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_returns_dead_function() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_foo', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create foo");
        let result = run_dead_code(&kit, "demo", "", true, true, "");
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_with_custom_entry_patterns() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_main', project: 'demo', name: 'main', qualifiedName: 'demo.main', filePath: '/src/main.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create main");
        let result = run_dead_code(&kit, "demo", "custom_entry,other_entry", true, true, "");
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
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

    // ===== T036: run_dead_code with config parameters =====

    #[test]
    fn run_dead_code_with_check_exported_excludes_exported() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_pub', project: 'demo', name: 'pub_fn', qualifiedName: 'demo.pub_fn', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create exported");
        storage.execute("CREATE (:Function {id: 'f_priv', project: 'demo', name: 'priv_fn', qualifiedName: 'demo.priv_fn', filePath: '/src/lib.rs', startLine: 6, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create private");

        let output = run_dead_code(&kit, "demo", "", true, true, "").expect("run should succeed");
        let names: Vec<&str> = output.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"pub_fn"),
            "exported fn should be excluded with check_exported=true"
        );
        assert!(names.contains(&"priv_fn"), "private fn should be dead");

        let output2 = run_dead_code(&kit, "demo", "", false, true, "").expect("run should succeed");
        let names2: Vec<&str> = output2.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names2.contains(&"pub_fn"),
            "exported fn should be dead with check_exported=false"
        );
        assert!(names2.contains(&"priv_fn"), "private fn should still be dead");
    }

    #[test]
    fn run_dead_code_with_check_ffi_excludes_ffi() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_ffi', project: 'demo', name: 'ffi_fn', qualifiedName: 'demo.ffi_fn', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: 'extern \"C\" fn ffi_fn()', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create ffi");
        storage.execute("CREATE (:Function {id: 'f_plain', project: 'demo', name: 'plain', qualifiedName: 'demo.plain', filePath: '/src/lib.rs', startLine: 6, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create plain");

        let output = run_dead_code(&kit, "demo", "", true, true, "").expect("run should succeed");
        let names: Vec<&str> = output.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"ffi_fn"),
            "FFI fn should be excluded with check_ffi=true"
        );
        assert!(names.contains(&"plain"), "plain fn should be dead");

        let output2 = run_dead_code(&kit, "demo", "", true, false, "").expect("run should succeed");
        let names2: Vec<&str> = output2.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names2.contains(&"ffi_fn"),
            "FFI fn should be dead with check_ffi=false"
        );
    }

    #[test]
    fn run_dead_code_with_custom_edge_types() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'USAGE', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");

        let output = run_dead_code(&kit, "demo", "", true, true, "").expect("run should succeed");
        let names: Vec<&str> = output.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"b"),
            "b should NOT be dead (USAGE edge in default config)"
        );
        assert!(names.contains(&"a"), "a should be dead");

        let output2 = run_dead_code(&kit, "demo", "", true, true, "CALLS").expect("run should succeed");
        let names2: Vec<&str> = output2.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names2.contains(&"b"),
            "b should be dead when only CALLS is checked"
        );
        assert!(names2.contains(&"a"), "a should still be dead");
    }

    #[test]
    fn run_dead_code_returns_output_struct() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_dead_code(&kit, "demo", "", true, true, "").expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert!(output.dead_code.is_empty(), "empty DB should yield empty dead_code");
    }

    // ===== T036: build_dead_code_config unit tests =====

    #[test]
    fn build_dead_code_config_parses_edge_types() {
        let config = build_dead_code_config(true, true, "CALLS,USAGE,TESTS");
        assert!(config.check_exported);
        assert!(config.check_ffi);
        assert_eq!(config.edge_types.len(), 3);
        assert!(config.edge_types.contains(&EdgeType::Calls));
        assert!(config.edge_types.contains(&EdgeType::Usage));
        assert!(config.edge_types.contains(&EdgeType::Tests));
    }

    #[test]
    fn build_dead_code_config_empty_edge_types_uses_defaults() {
        let config = build_dead_code_config(true, true, "");
        assert!(config.check_exported);
        assert!(config.check_ffi);
        let default = DeadCodeConfig::default();
        assert_eq!(config.edge_types, default.edge_types);
    }

    #[test]
    fn build_dead_code_config_skips_invalid_edge_types() {
        let config = build_dead_code_config(false, false, "CALLS,INVALID,TESTS");
        assert!(!config.check_exported);
        assert!(!config.check_ffi);
        assert_eq!(config.edge_types.len(), 2);
        assert!(config.edge_types.contains(&EdgeType::Calls));
        assert!(config.edge_types.contains(&EdgeType::Tests));
    }

    #[test]
    fn build_dead_code_config_all_invalid_keeps_defaults() {
        let config = build_dead_code_config(true, true, "INVALID1,INVALID2");
        let default = DeadCodeConfig::default();
        assert_eq!(
            config.edge_types, default.edge_types,
            "all-invalid should keep defaults"
        );
    }

    #[test]
    fn build_dead_code_config_trims_whitespace() {
        let config = build_dead_code_config(true, true, "  CALLS ,  USAGE  ");
        assert_eq!(config.edge_types.len(), 2);
        assert!(config.edge_types.contains(&EdgeType::Calls));
        assert!(config.edge_types.contains(&EdgeType::Usage));
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[test]
    fn dead_code_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(dead_code(
            "demo".to_string(),
            "".to_string(),
            false,
            false,
            "".to_string(),
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[test]
    fn dead_code_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(dead_code(
            "demo".to_string(),
            "".to_string(),
            false,
            false,
            "".to_string(),
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
