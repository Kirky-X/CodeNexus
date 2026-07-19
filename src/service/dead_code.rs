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
#[cfg(feature = "analysis")]
use crate::service::project::resolve_project_id;
#[cfg(all(feature = "cli", feature = "analysis"))]
use crate::service::runtime::kit;
#[cfg(feature = "analysis")]
use crate::service::status::{git_head_commit, is_stale, resolve_project_root};
#[cfg(feature = "analysis")]
use crate::storage::StorageConfig;

#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::forge;
#[cfg(all(feature = "cli", feature = "analysis"))]
use sdforge::prelude::ApiError;

/// JSON-serializable dead-code output.
#[cfg(feature = "analysis")]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeadCodeOutput {
    pub project: String,
    pub dead_code: Vec<DeadCodeEntry>,
    /// Git commit hash captured at index time (Project.lastCommit).
    /// Empty when the project was indexed from a non-git root.
    pub indexed_commit: String,
    /// Current `HEAD` of the project root at query time (empty if not a git
    /// repo or git is unavailable).
    pub current_head: String,
    /// `true` iff both commits are non-empty and differ.
    pub is_stale: bool,
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
    check_dynamic_dispatch: bool,
    edge_types: &str,
) -> DeadCodeConfig {
    let default = DeadCodeConfig::default();
    let final_edge_types =
        crate::model::edge_type::parse_edge_type_list(edge_types, &default.edge_types);
    DeadCodeConfig {
        check_exported,
        check_ffi,
        check_dynamic_dispatch,
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
    check_dynamic_dispatch: bool,
    edge_types: &str,
) -> Result<DeadCodeOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let project_id = resolve_project_id(&*storage, project)?;
    // B6: fetch the full Project record to read `lastCommit` (indexed_commit)
    // and `rootPath` (for `git rev-parse HEAD` at query time). Use the O(1)
    // `get_project` lookup instead of `list_projects + find` (arch-review
    // HIGH-1: previous code triggered a second full scan even though
    // `resolve_project_id` already did one internally).
    let project_record = storage
        .get_project(&project_id)
        .map_err(CodeNexusError::from)?
        .ok_or_else(|| CodeNexusError::ProjectNotFound(project.to_string()))?;
    let indexed_commit = project_record.last_commit.clone();
    // T206: resolve rootPath with fallback for legacy relative paths so
    // `git rev-parse HEAD` runs against the actual project root, not the
    // process CWD. See `status::resolve_project_root` for the heuristic.
    let storage_config = kit.config::<StorageConfig>()?;
    let root = resolve_project_root(&project_record.root_path, &storage_config.db_path);
    let current_head = git_head_commit(&root);
    let stale = is_stale(&indexed_commit, &current_head);
    let config = build_dead_code_config(
        check_exported,
        check_ffi,
        check_dynamic_dispatch,
        edge_types,
    );
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
    let entries = detector.detect(&project_id, &entry_patterns)?;
    Ok(DeadCodeOutput {
        project: project.to_string(),
        dead_code: entries,
        indexed_commit,
        current_head,
        is_stale: stale,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "analysis"))]
#[forge(
    name = "dead_code",
    version = "0.3.5",
    description = "Detect unreferenced (dead) functions in a project.",
    cli = true
)]
async fn dead_code(
    project: String,
    entry: String,
    check_exported: bool,
    check_ffi: bool,
    check_dynamic_dispatch: bool,
    edge_types: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_dead_code(
        &kit,
        &project,
        &entry,
        check_exported,
        check_ffi,
        check_dynamic_dispatch,
        &edge_types,
    )
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
    use crate::storage::capability::Storage;
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

    fn seed_project(storage: &dyn Storage, id: &str, name: &str) {
        storage
            .execute(&format!(
                "CREATE (:Project {{id: '{id}', name: '{name}', rootPath: '/demo', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc'}});"
            ))
            .expect("create project");
    }

    #[test]
    fn run_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let result = run_dead_code(&kit, "demo", "", true, true, true, "");
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_returns_dead_function() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'f_foo', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create foo");
        let result = run_dead_code(&kit, "demo", "", true, true, true, "");
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn run_with_custom_entry_patterns() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'f_main', project: 'demo', name: 'main', qualifiedName: 'demo.main', filePath: '/src/main.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create main");
        let result = run_dead_code(
            &kit,
            "demo",
            "custom_entry,other_entry",
            true,
            true,
            true,
            "",
        );
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
            indexed_commit: "abc123".into(),
            current_head: "def456".into(),
            is_stale: true,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"dead_code\""));
        assert!(json.contains("\"foo\""));
        assert!(json.contains("\"indexed_commit\":\"abc123\""));
        assert!(json.contains("\"current_head\":\"def456\""));
        assert!(json.contains("\"is_stale\":true"));
    }

    // ===== T036: run_dead_code with config parameters =====

    #[test]
    fn run_dead_code_with_check_exported_excludes_exported() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'f_pub', project: 'demo', name: 'pub_fn', qualifiedName: 'demo.pub_fn', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create exported");
        storage.execute("CREATE (:Function {id: 'f_priv', project: 'demo', name: 'priv_fn', qualifiedName: 'demo.priv_fn', filePath: '/src/lib.rs', startLine: 6, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create private");

        let output =
            run_dead_code(&kit, "demo", "", true, true, true, "").expect("run should succeed");
        let names: Vec<&str> = output.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"pub_fn"),
            "exported fn should be excluded with check_exported=true"
        );
        assert!(names.contains(&"priv_fn"), "private fn should be dead");

        let output2 =
            run_dead_code(&kit, "demo", "", false, true, true, "").expect("run should succeed");
        let names2: Vec<&str> = output2.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names2.contains(&"pub_fn"),
            "exported fn should be dead with check_exported=false"
        );
        assert!(
            names2.contains(&"priv_fn"),
            "private fn should still be dead"
        );
    }

    #[test]
    fn run_dead_code_with_check_ffi_excludes_ffi() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'f_ffi', project: 'demo', name: 'ffi_fn', qualifiedName: 'demo.ffi_fn', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: 'extern \"C\" fn ffi_fn()', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create ffi");
        storage.execute("CREATE (:Function {id: 'f_plain', project: 'demo', name: 'plain', qualifiedName: 'demo.plain', filePath: '/src/lib.rs', startLine: 6, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create plain");

        let output =
            run_dead_code(&kit, "demo", "", true, true, true, "").expect("run should succeed");
        let names: Vec<&str> = output.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"ffi_fn"),
            "FFI fn should be excluded with check_ffi=true"
        );
        assert!(names.contains(&"plain"), "plain fn should be dead");

        let output2 =
            run_dead_code(&kit, "demo", "", true, false, true, "").expect("run should succeed");
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
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'USAGE', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create edge");

        // B5: `a` is passed as an entry-pattern seed; `b` is reachable from
        // `a` via USAGE (in default config) → both alive.
        let output =
            run_dead_code(&kit, "demo", "a", true, true, true, "").expect("run should succeed");
        let names: Vec<&str> = output.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"b"),
            "b should NOT be dead (reachable from seed a via USAGE)"
        );
        assert!(
            !names.contains(&"a"),
            "a should be alive (entry pattern seed)"
        );

        // With CALLS-only config, USAGE edge is not traversed → b unreachable
        // → b dead. `a` is still a seed → alive.
        let output2 = run_dead_code(&kit, "demo", "a", true, true, true, "CALLS")
            .expect("run should succeed");
        let names2: Vec<&str> = output2.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names2.contains(&"b"),
            "b should be dead when only CALLS is checked (USAGE not traversed)"
        );
        assert!(
            !names2.contains(&"a"),
            "a should still be alive (entry pattern seed)"
        );
    }

    #[test]
    fn run_dead_code_returns_output_struct() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let output =
            run_dead_code(&kit, "demo", "", true, true, true, "").expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert!(
            output.dead_code.is_empty(),
            "empty DB should yield empty dead_code"
        );
    }

    // ===== T036: build_dead_code_config unit tests =====

    #[test]
    fn build_dead_code_config_parses_edge_types() {
        let config = build_dead_code_config(true, true, true, "CALLS,USAGE,TESTS");
        assert!(config.check_exported);
        assert!(config.check_ffi);
        assert!(config.check_dynamic_dispatch);
        assert_eq!(config.edge_types.len(), 3);
        assert!(config.edge_types.contains(&EdgeType::Calls));
        assert!(config.edge_types.contains(&EdgeType::Usage));
        assert!(config.edge_types.contains(&EdgeType::Tests));
    }

    #[test]
    fn build_dead_code_config_empty_edge_types_uses_defaults() {
        let config = build_dead_code_config(true, true, true, "");
        assert!(config.check_exported);
        assert!(config.check_ffi);
        let default = DeadCodeConfig::default();
        assert_eq!(config.edge_types, default.edge_types);
    }

    #[test]
    fn build_dead_code_config_skips_invalid_edge_types() {
        let config = build_dead_code_config(false, false, false, "CALLS,INVALID,TESTS");
        assert!(!config.check_exported);
        assert!(!config.check_ffi);
        assert!(!config.check_dynamic_dispatch);
        assert_eq!(config.edge_types.len(), 2);
        assert!(config.edge_types.contains(&EdgeType::Calls));
        assert!(config.edge_types.contains(&EdgeType::Tests));
    }

    #[test]
    fn build_dead_code_config_all_invalid_keeps_defaults() {
        let config = build_dead_code_config(true, true, true, "INVALID1,INVALID2");
        let default = DeadCodeConfig::default();
        assert_eq!(
            config.edge_types, default.edge_types,
            "all-invalid should keep defaults"
        );
    }

    #[test]
    fn build_dead_code_config_trims_whitespace() {
        let config = build_dead_code_config(true, true, true, "  CALLS ,  USAGE  ");
        assert_eq!(config.edge_types.len(), 2);
        assert!(config.edge_types.contains(&EdgeType::Calls));
        assert!(config.edge_types.contains(&EdgeType::Usage));
    }

    // ===== B3.5: check_dynamic_dispatch propagation tests =====

    #[test]
    fn build_dead_code_config_passes_check_dynamic_dispatch_true() {
        let config = build_dead_code_config(true, true, true, "");
        assert!(
            config.check_dynamic_dispatch,
            "check_dynamic_dispatch=true should propagate"
        );
    }

    #[test]
    fn build_dead_code_config_passes_check_dynamic_dispatch_false() {
        let config = build_dead_code_config(true, true, false, "");
        assert!(
            !config.check_dynamic_dispatch,
            "check_dynamic_dispatch=false should propagate"
        );
    }

    #[test]
    fn run_dead_code_with_check_dynamic_dispatch_excludes_trait_impl() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        seed_project(&*storage, "demo", "demo");
        // Trait impl method (e.g. `impl Display for X { fn fmt() {} }`)
        storage.execute("CREATE (:Method {id: 'm_fmt', project: 'demo', name: 'fmt', qualifiedName: 'demo.src.lib.rs.fmt#Display', filePath: '/src/lib.rs', startLine: 5, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create trait impl");

        // With check_dynamic_dispatch=true (B3.5 default), trait impl is NOT dead
        let output =
            run_dead_code(&kit, "demo", "", true, true, true, "").expect("run should succeed");
        let names: Vec<&str> = output.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"fmt"),
            "trait impl fmt#Display should NOT be dead with check_dynamic_dispatch=true"
        );

        // With check_dynamic_dispatch=false (opt-out), trait impl IS dead
        let output2 =
            run_dead_code(&kit, "demo", "", true, true, false, "").expect("run should succeed");
        let names2: Vec<&str> = output2.dead_code.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names2.contains(&"fmt"),
            "trait impl fmt#Display IS dead with check_dynamic_dispatch=false"
        );
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    fn dead_code_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(dead_code(
            "demo".to_string(),
            "".to_string(),
            false,
            false,
            false,
            "".to_string(),
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
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
            false,
            "".to_string(),
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }

    // --- B6: index freshness (indexed_commit / current_head / is_stale) ---

    /// Helper: create a project row with custom `rootPath` and `lastCommit`.
    fn seed_project_with(
        storage: &dyn Storage,
        id: &str,
        name: &str,
        root_path: &str,
        last_commit: &str,
    ) {
        use crate::storage::schema::escape_cypher_string;
        storage
            .execute(&format!(
                "CREATE (:Project {{id: '{}', name: '{}', rootPath: '{}', language: 'rust', fileCount: 0, indexedAt: 1000, lastCommit: '{}'}});",
                escape_cypher_string(id),
                escape_cypher_string(name),
                escape_cypher_string(root_path),
                escape_cypher_string(last_commit),
            ))
            .expect("create project");
    }

    #[test]
    fn test_dead_code_output_includes_indexed_commit_when_set() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        // rootPath points to a non-git directory → current_head empty.
        seed_project_with(&*storage, "demo", "demo", "/nonexistent/path", "abc123");
        let output = run_dead_code(&kit, "demo", "", true, true, true, "").expect("run");
        assert_eq!(output.indexed_commit, "abc123");
        assert_eq!(output.current_head, "", "non-git root → empty current_head");
        assert!(!output.is_stale, "current_head empty → not stale");
    }

    #[test]
    fn test_is_stale_true_when_commit_differs() {
        let tmp = TempDir::new().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(tmp.path())
            .status();
        if status.is_err() || !status.unwrap().success() {
            eprintln!("skipping test: git init failed");
            return;
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        std::fs::write(tmp.path().join("README.md"), "init\n").unwrap();
        if !git(&["add", "."])
            || !git(&[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "init",
            ])
        {
            eprintln!("skipping test: git commit failed");
            return;
        }
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if head.is_empty() {
            eprintln!("skipping test: could not determine HEAD");
            return;
        }

        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        let root = tmp.path().to_string_lossy().into_owned();
        // indexed_commit deliberately differs from current HEAD.
        seed_project_with(&*storage, "demo", "demo", &root, "abc123");
        let output = run_dead_code(&kit, "demo", "", true, true, true, "").expect("run");
        assert_eq!(output.indexed_commit, "abc123");
        assert_eq!(output.current_head, head);
        assert!(output.is_stale, "commits differ → stale");
    }

    #[test]
    fn test_is_stale_false_when_commits_match() {
        let tmp = TempDir::new().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(tmp.path())
            .status();
        if status.is_err() || !status.unwrap().success() {
            eprintln!("skipping test: git init failed");
            return;
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        std::fs::write(tmp.path().join("README.md"), "init\n").unwrap();
        if !git(&["add", "."])
            || !git(&[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "init",
            ])
        {
            eprintln!("skipping test: git commit failed");
            return;
        }
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if head.is_empty() {
            eprintln!("skipping test: could not determine HEAD");
            return;
        }

        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        let root = tmp.path().to_string_lossy().into_owned();
        // indexed_commit == current HEAD → fresh.
        seed_project_with(&*storage, "demo", "demo", &root, &head);
        let output = run_dead_code(&kit, "demo", "", true, true, true, "").expect("run");
        assert_eq!(output.indexed_commit, head);
        assert_eq!(output.current_head, head);
        assert!(!output.is_stale, "commits match → fresh");
    }

    /// T206: legacy indexes stored `rootPath = "."`. Without
    /// [`resolve_project_root`], `git rev-parse HEAD` would run in the
    /// process CWD (which might be a different git repo) and return the
    /// wrong commit, causing false `is_stale=true`. This test verifies the
    /// `db_path`-based fallback resolves the actual project root.
    #[test]
    fn test_dead_code_resolves_relative_rootpath_via_db_path() {
        let project_root = TempDir::new().unwrap();
        let project_root_path = project_root.path().canonicalize().unwrap();

        let status = std::process::Command::new("git")
            .arg("init")
            .arg(&project_root_path)
            .status();
        if status.is_err() || !status.unwrap().success() {
            eprintln!("skipping test: git init failed");
            return;
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&project_root_path)
                .args(args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        std::fs::write(project_root_path.join("README.md"), "init\n").unwrap();
        if !git(&["add", "."])
            || !git(&[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "init",
            ])
        {
            eprintln!("skipping test: git commit failed");
            return;
        }
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root_path)
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if head.is_empty() {
            eprintln!("skipping test: could not determine HEAD");
            return;
        }

        // Create DB at <project_root>/.codenexus/test.lbug — the layout
        // `resolve_project_root`'s fallback expects (db_path → parent →
        // parent = project_root).
        let db_dir = project_root_path.join(".codenexus");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("test.lbug");

        let kit = build_kit_for_db(&db_path);
        let storage = kit.require::<StorageModule>().expect("storage");
        // rootPath deliberately set to "." (legacy). lastCommit = current HEAD
        // so is_stale should be false once the fallback resolves the root.
        seed_project_with(&*storage, "demo", "demo", ".", &head);
        let output = run_dead_code(&kit, "demo", "", true, true, true, "").expect("run");
        assert_eq!(
            output.current_head, head,
            "current_head must be the project's actual HEAD, not the CWD's HEAD"
        );
        assert!(
            !output.is_stale,
            "should not be stale: indexed_commit == current_head after fallback resolution"
        );
    }
}
