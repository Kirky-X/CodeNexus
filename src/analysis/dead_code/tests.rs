// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Tests for `dead_code` (extracted from `mod.rs` in T202 arch-review
//! MEDIUM-2). The parent `mod.rs` declares `#[cfg(test)] mod tests;`, so
//! this file's contents are `dead_code::tests::*`. `use super::*` pulls in
//! every item defined in `mod.rs`.

use super::*;
use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
use tempfile::TempDir;

/// Returns a fresh on-disk database path inside a temp dir.
fn fresh_db_path() -> std::path::PathBuf {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("dead_code_testdb");
    std::mem::forget(dir);
    path
}

/// Builds a Kit backed by an on-disk database at `db`.
fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
    let config = KitBootstrapConfig::new(db.to_path_buf());
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(build_kit(&config))
        .expect("build_kit")
}

/// Returns the `dyn Storage` capability from `kit`.
fn storage(kit: &AsyncKit<AsyncReady>) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
    kit.require::<StorageModule>().expect("require_storage")
}

/// Creates a Function node via direct Cypher.
fn create_function(
    kit: &AsyncKit<AsyncReady>,
    id: &str,
    project: &str,
    name: &str,
    qn: &str,
    file: &str,
    line: u32,
) {
    let storage = storage(kit);
    let end_line = line + 10;
    let cypher = format!(
        "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
        escape_cypher_string(id),
        escape_cypher_string(project),
        escape_cypher_string(name),
        escape_cypher_string(qn),
        escape_cypher_string(file),
        line,
        end_line,
    );
    storage.execute(&cypher).expect("create function");
}

/// Creates a Function node with `isExported = true` and optional `signature`.
#[allow(clippy::too_many_arguments)]
fn create_function_with_flags(
    kit: &AsyncKit<AsyncReady>,
    id: &str,
    project: &str,
    name: &str,
    qn: &str,
    file: &str,
    line: u32,
    is_exported: bool,
    signature: &str,
) {
    let storage = storage(kit);
    let end_line = line + 10;
    let cypher = format!(
        "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '{}', returnType: '', \
             isExported: {}, docstring: '', content: '', parentQn: ''}});",
        escape_cypher_string(id),
        escape_cypher_string(project),
        escape_cypher_string(name),
        escape_cypher_string(qn),
        escape_cypher_string(file),
        line,
        end_line,
        escape_cypher_string(signature),
        is_exported,
    );
    storage
        .execute(&cypher)
        .expect("create function with flags");
}

/// Creates a Method node via direct Cypher.
fn create_method(
    kit: &AsyncKit<AsyncReady>,
    id: &str,
    project: &str,
    name: &str,
    qn: &str,
    file: &str,
    line: u32,
) {
    let storage = storage(kit);
    let end_line = line + 10;
    let cypher = format!(
        "CREATE (:Method {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parameterCount: 0, parentQn: ''}});",
        escape_cypher_string(id),
        escape_cypher_string(project),
        escape_cypher_string(name),
        escape_cypher_string(qn),
        escape_cypher_string(file),
        line,
        end_line,
    );
    storage.execute(&cypher).expect("create method");
}

/// Creates a CALLS edge from `caller_id` to `callee_id`.
fn create_calls_edge(
    kit: &AsyncKit<AsyncReady>,
    edge_id: &str,
    caller_id: &str,
    callee_id: &str,
    project: &str,
) {
    create_edge(kit, edge_id, caller_id, callee_id, project, "CALLS");
}

/// Creates a CodeRelation edge of `edge_type` (DDL string, e.g. `"USAGE"`)
/// from `source_id` to `target_id`.
fn create_edge(
    kit: &AsyncKit<AsyncReady>,
    edge_id: &str,
    source_id: &str,
    target_id: &str,
    project: &str,
    edge_type: &str,
) {
    let storage = storage(kit);
    let cypher = format!(
        "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', type: '{}', \
             confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: '{}'}});",
        escape_cypher_string(edge_id),
        escape_cypher_string(source_id),
        escape_cypher_string(target_id),
        escape_cypher_string(edge_type),
        escape_cypher_string(project),
    );
    storage.execute(&cypher).expect("create edge");
}

/// Creates a File node (for language resolution).
fn create_file(
    kit: &AsyncKit<AsyncReady>,
    id: &str,
    project: &str,
    file_path: &str,
    language: &str,
) {
    let storage = storage(kit);
    let cypher = format!(
        "CREATE (:File {{id: '{}', project: '{}', name: '{}', filePath: '{}', \
             language: '{}', hash: '', lineCount: 0}});",
        escape_cypher_string(id),
        escape_cypher_string(project),
        escape_cypher_string(file_path.split('/').next_back().unwrap_or("file")),
        escape_cypher_string(file_path),
        escape_cypher_string(language),
    );
    storage.execute(&cypher).expect("create file");
}

// --- glob_match unit tests ---

#[test]
fn glob_match_exact() {
    assert!(glob_match("main", "main"));
    assert!(!glob_match("main", "main2"));
}

#[test]
fn glob_match_prefix_wildcard() {
    assert!(glob_match("test_*", "test_foo"));
    assert!(glob_match("test_*", "test_"));
    assert!(!glob_match("test_*", "foo_test"));
}

#[test]
fn glob_match_suffix_wildcard() {
    assert!(glob_match("*_test", "foo_test"));
    assert!(glob_match("*_test", "_test"));
    assert!(!glob_match("*_test", "test_foo"));
}

#[test]
fn glob_match_middle_wildcard() {
    assert!(glob_match("test_*_spec", "test_foo_spec"));
    assert!(!glob_match("test_*_spec", "test_foo"));
}

#[test]
fn glob_match_star_only() {
    assert!(glob_match("*", "anything"));
    assert!(glob_match("*", ""));
}

// --- DeadCodeDetector tests ---

#[test]
fn detect_returns_empty_for_empty_db() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    assert!(result.is_empty(), "empty DB should yield no dead code");
}

#[test]
fn detect_finds_dead_function() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // `foo` has no incoming CALLS edges; `main` also has no incoming
    // edges but is excluded as an entry point.
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
    create_function(
        &kit,
        "f_main",
        "demo",
        "main",
        "demo.main",
        "/src/main.rs",
        1,
    );
    create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
    create_file(&kit, "file2", "demo", "/src/main.rs", "rust");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"foo"), "foo should be dead: {:?}", names);
    assert!(
        !names.contains(&"main"),
        "main should be excluded: {:?}",
        names
    );
}

// T182-B: Function nodes whose signature carries an entry-point
// attribute (`#[tool(...)]` / `#[forge(...)]` / `#[tokio::main]` /
// `#[rocket::main]` / `#[actix::main]` / `#[axum::main]` etc.) must
// NOT be reported dead. These attributes register the function as an
// external entry point via macro expansion (e.g. rmcp `#[tool]` /
// CodeNexus `#[forge]` register MCP tools; `#[tokio::main]` synthesizes
// a synchronous `main` that calls the async fn). tree-sitter does not
// expand macros, so the synthesised CALLS edge is invisible to the
// graph — dead_code must treat the attribute itself as the entry-point
// signal (B4.5 deferred task, T045/T046 spec).
#[test]
fn b_tool_attribute_marked_functions_treated_as_live() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // `#[tool(name = "query")]` registers `query_mcp` as an MCP tool —
    // macro expansion synthesises a dispatch table that calls it.
    create_function_with_flags(
        &kit,
        "f_tool",
        "demo",
        "query_mcp",
        "demo.query_mcp",
        "/src/service/query.rs",
        75,
        false,
        "#[tool(name = \"query\")]\nasync fn query_mcp() {}",
    );
    // `#[forge(name = "architecture", cli = true)]` is the CodeNexus
    // equivalent — `#[forge]` registers both an MCP tool and a CLI
    // subcommand.
    create_function_with_flags(
        &kit,
        "f_forge",
        "demo",
        "architecture",
        "demo.architecture",
        "/src/service/architecture.rs",
        63,
        false,
        "#[forge(name = \"architecture\", cli = true)]\nasync fn architecture() {}",
    );
    // Control: plain private function with no attribute, no incoming
    // CALLS edges → IS dead.
    create_function_with_flags(
        &kit,
        "f_plain",
        "demo",
        "unused_helper",
        "demo.unused_helper",
        "/src/lib.rs",
        100,
        false,
        "fn unused_helper() {}",
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"query_mcp"),
        "T182-B: #[tool]-marked query_mcp must NOT be dead: {:?}",
        names
    );
    assert!(
        !names.contains(&"architecture"),
        "T182-B: #[forge]-marked architecture must NOT be dead: {:?}",
        names
    );
    assert!(
        names.contains(&"unused_helper"),
        "T182-B: plain unused_helper IS dead: {:?}",
        names
    );
}

// T182-B: Verifies that the common async-runtime / web-framework entry
// attributes (`#[tokio::main]`, `#[rocket::main]`, `#[actix::main]`,
// `#[axum::main]`) are also recognised as entry-point seeds. These
// macros synthesise a synchronous `main` that calls the decorated async
// fn, so the decorated fn has no static CALLS edge in the graph.
#[test]
fn b_async_runtime_entry_attributes_treated_as_live() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_tokio",
        "demo",
        "run",
        "demo.run",
        "/src/main.rs",
        10,
        false,
        "#[tokio::main]\nasync fn run() {}",
    );
    create_function_with_flags(
        &kit,
        "f_rocket",
        "demo",
        "rocket_main",
        "demo.rocket_main",
        "/src/main.rs",
        20,
        false,
        "#[rocket::main]\nasync fn rocket_main() {}",
    );
    create_function_with_flags(
        &kit,
        "f_actix",
        "demo",
        "actix_main",
        "demo.actix_main",
        "/src/main.rs",
        30,
        false,
        "#[actix::main]\nasync fn actix_main() {}",
    );
    create_function_with_flags(
        &kit,
        "f_axum",
        "demo",
        "axum_main",
        "demo.axum_main",
        "/src/main.rs",
        40,
        false,
        "#[axum::main]\nasync fn axum_main() {}",
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    for expected in ["run", "rocket_main", "actix_main", "axum_main"] {
        assert!(
            !names.contains(&expected),
            "T182-B: async-runtime entry attribute should keep {expected} live: {:?}",
            names
        );
    }
}

// T182-B: Verifies the attribute seed check is a substring match (not
// exact match), so both `#[tool]` (bare) and `#[tool(...)]` (with
// arguments) are recognised. Also verifies that `#[cfg(...)]` /
// `#[derive(...)]` (non-entry-point attributes) do NOT falsely mark a
// function as live.
#[test]
fn b_attribute_seed_uses_substring_match_and_ignores_non_entry_attributes() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // Bare `#[tool]` without arguments — substring `#[tool` matches.
    create_function_with_flags(
        &kit,
        "f_bare",
        "demo",
        "bare_tool",
        "demo.bare_tool",
        "/src/m.rs",
        1,
        false,
        "#[tool]\nfn bare_tool() {}",
    );
    // `#[cfg(test)]` + `#[derive(Debug)]` are NOT entry-point attributes
    // — this function should still be reported dead.
    create_function_with_flags(
        &kit,
        "f_cfg",
        "demo",
        "cfg_only",
        "demo.cfg_only",
        "/src/m.rs",
        10,
        false,
        "#[cfg(test)]\n#[derive(Debug)]\nfn cfg_only() {}",
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"bare_tool"),
        "T182-B: bare #[tool] should mark bare_tool as live: {:?}",
        names
    );
    assert!(
        names.contains(&"cfg_only"),
        "T182-B: #[cfg]/#[derive] should NOT mark cfg_only as live: {:?}",
        names
    );
}

// B7: a Function with no incoming CALLS edges but targeted by a
// REEXPORTS edge (File→Function, created by `resolve/imports.rs`
// for `pub use` / `export ... from`) must NOT be reported dead —
// the symbol is reachable from outside the current crate/module.
#[test]
fn detect_excludes_reexport_targets() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // `bar` is re-exported: REEXPORTS edge from a File node to `bar`.
    create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 1);
    create_function(&kit, "f_qux", "demo", "qux", "demo.qux", "/src/lib.rs", 50);
    create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
    create_file(&kit, "file2", "demo", "/src/main.rs", "rust");
    // REEXPORTS edge: file2 re-exports bar from file1.
    create_edge(&kit, "e_reexport", "file2", "f_bar", "demo", "REEXPORTS");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"bar"),
        "bar is re-exported, must NOT be dead: {:?}",
        names
    );
    assert!(
        names.contains(&"qux"),
        "qux has no incoming edges and is not re-exported, must be dead: {:?}",
        names
    );
}

// B7: BatchPrefetch correctly loads REEXPORTS edge targets into
// `reexport_target_ids` and `is_reexport_target` returns true for them.
#[test]
fn batch_prefetch_loads_reexport_targets() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 1);
    create_function(&kit, "f_baz", "demo", "baz", "demo.baz", "/src/lib.rs", 50);
    create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
    create_file(&kit, "file2", "demo", "/src/main.rs", "rust");
    create_edge(&kit, "e1", "file2", "f_bar", "demo", "REEXPORTS");

    let storage = storage(&kit);
    let functions = load_all_functions(&*storage, "demo").expect("load_all_functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &DeadCodeConfig::default(), &functions)
        .expect("BatchPrefetch::load");
    assert!(
        prefetch.is_reexport_target("f_bar"),
        "f_bar is a REEXPORTS target"
    );
    assert!(
        !prefetch.is_reexport_target("f_baz"),
        "f_baz is NOT a REEXPORTS target"
    );
    assert_eq!(
        prefetch.reexport_target_ids_len(),
        1,
        "exactly 1 reexport target"
    );
}

#[test]
fn detect_excludes_entry_points() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_main",
        "demo",
        "main",
        "demo.main",
        "/src/main.rs",
        1,
    );
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
    create_file(&kit, "file1", "demo", "/src/main.rs", "rust");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"main"), "main is an entry point");
    assert!(names.contains(&"foo"), "foo is not an entry point");
}

#[test]
fn detect_excludes_test_functions() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // `test_foo` matches the default `test_*` pattern and is excluded
    // even though it has zero incoming CALLS edges.
    create_function(
        &kit,
        "f_test_foo",
        "demo",
        "test_foo",
        "demo.test_foo",
        "/src/lib.rs",
        1,
    );
    create_function(
        &kit,
        "f_foo_test",
        "demo",
        "foo_test",
        "demo.foo_test",
        "/src/lib.rs",
        10,
    );
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 20);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"test_foo"), "test_foo matches test_*");
    assert!(!names.contains(&"foo_test"), "foo_test matches *_test");
    assert!(names.contains(&"foo"), "foo is dead");
}

#[test]
fn detect_handles_method_nodes() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // A Method node with zero incoming CALLS edges is dead code.
    create_method(
        &kit,
        "m_1",
        "demo",
        "helper",
        "demo.Class.helper",
        "/src/lib.rs",
        5,
    );
    create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"helper"), "Method helper should be dead");
}

#[test]
fn detect_excludes_functions_with_incoming_calls() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // `main` calls `foo`; `foo` has an incoming CALLS edge and is NOT dead.
    create_function(
        &kit,
        "f_main",
        "demo",
        "main",
        "demo.main",
        "/src/main.rs",
        1,
    );
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
    create_calls_edge(&kit, "e1", "f_main", "f_foo", "demo");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"foo"), "foo is called by main, not dead");
    assert!(!names.contains(&"main"), "main is an entry point");
}

#[test]
fn detect_resolves_language_from_file_table() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.py", 1);
    create_file(&kit, "file1", "demo", "/src/lib.py", "python");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let entry = result
        .iter()
        .find(|e| e.name == "foo")
        .expect("foo should be dead");
    assert_eq!(entry.language, "python");
}

#[test]
fn detect_includes_reason_field() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let entry = result
        .iter()
        .find(|e| e.name == "foo")
        .expect("foo should be dead");
    assert_eq!(entry.reason, "zero incoming CALLS edges");
}

#[test]
fn detect_all_dead_when_no_edges_and_no_entries() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
    create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/b.rs", 1);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    // No entry patterns, no CALLS edges → everything is dead.
    let result = detector.detect("demo", &[]).expect("detect");
    assert_eq!(result.len(), 2, "both functions should be dead");
}

#[test]
fn detect_filters_by_project() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
    create_function(&kit, "f_b", "other", "b", "other.b", "/src/b.rs", 1);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a"), "a is in demo project");
    assert!(!names.contains(&"b"), "b is in other project");
}

// --- T002: DeadCodeConfig / Confidence tests ---

#[test]
fn dead_code_config_default_values() {
    let cfg = DeadCodeConfig::default();
    // Entry patterns default to the 6 multi-language entry points.
    assert_eq!(
        cfg.entry_patterns,
        vec![
            "main".to_string(),
            "Main".to_string(),
            "__main__".to_string(),
            "wmain".to_string(),
            "WinMain".to_string(),
            "DLLMain".to_string(),
        ]
    );
    // Test patterns mirror DEFAULT_TEST_PATTERNS (B2: expanded to 8).
    assert_eq!(cfg.test_patterns.len(), 8);
    assert!(cfg.test_patterns.contains(&"test_*".to_string()));
    assert!(cfg.test_patterns.contains(&"*_test".to_string()));
    assert!(cfg.test_patterns.contains(&"*_spec".to_string()));
    assert!(cfg.test_patterns.contains(&"it_*".to_string()));
    assert!(cfg.test_patterns.contains(&"sec_*".to_string()));
    assert!(cfg.test_patterns.contains(&"snap_*".to_string()));
    assert!(cfg.test_patterns.contains(&"perf_*".to_string()));
    assert!(cfg.test_patterns.contains(&"bench_*".to_string()));
    // Exported / FFI checks are on by default.
    assert!(cfg.check_exported, "check_exported should default to true");
    assert!(cfg.check_ffi, "check_ffi should default to true");
    // B3.5: Dynamic-dispatch (trait impl recognition) is ON by default,
    // aligning with rustc's dead_code lint which treats trait impls as
    // reachable via vtable.
    assert!(
        cfg.check_dynamic_dispatch,
        "check_dynamic_dispatch should default to true (B3.5)"
    );
    assert!(!cfg.check_reflection);
    // Edge types must include all variants used for "used" detection
    // per R-dead_code-001.
    assert!(cfg.edge_types.contains(&EdgeType::Calls));
    assert!(cfg.edge_types.contains(&EdgeType::FfiCalls));
    assert!(cfg.edge_types.contains(&EdgeType::Implements));
    assert!(cfg.edge_types.contains(&EdgeType::HandlesRoute));
    assert!(cfg.edge_types.contains(&EdgeType::Usage));
    assert!(cfg.edge_types.contains(&EdgeType::Tests));
    assert!(cfg.edge_types.contains(&EdgeType::UsesType));
    assert!(cfg.edge_types.contains(&EdgeType::HttpCalls));
    assert!(cfg.edge_types.contains(&EdgeType::AsyncCalls));
}

#[test]
fn confidence_serializes_high_medium_low() {
    // Variant name is the JSON representation (serde default).
    assert_eq!(
        serde_json::to_string(&Confidence::High).unwrap(),
        "\"High\""
    );
    assert_eq!(
        serde_json::to_string(&Confidence::Medium).unwrap(),
        "\"Medium\""
    );
    assert_eq!(serde_json::to_string(&Confidence::Low).unwrap(), "\"Low\"");
    // Roundtrip every variant.
    for c in [Confidence::High, Confidence::Medium, Confidence::Low] {
        let json = serde_json::to_string(&c).unwrap();
        let parsed: Confidence = serde_json::from_str(&json).unwrap();
        assert_eq!(c, parsed, "roundtrip failed for {json}");
    }
}

#[test]
fn confidence_rejects_invalid_variant() {
    assert!(serde_json::from_str::<Confidence>("\"Critical\"").is_err());
    assert!(serde_json::from_str::<Confidence>("\"high\"").is_err());
}

#[test]
fn detect_sets_confidence_high_for_zero_incoming() {
    // Until T007 refines scoring, zero-incoming entries are High.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let entry = result
        .iter()
        .find(|e| e.name == "foo")
        .expect("foo should be dead");
    assert_eq!(entry.confidence, Confidence::High);
}

#[test]
fn with_config_accepts_custom_config() {
    // with_config must not panic and must produce a working detector.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
    let storage = storage(&kit);
    let cfg = DeadCodeConfig {
        entry_patterns: vec![],
        test_patterns: vec![],
        check_exported: false,
        check_dynamic_dispatch: false,
        check_reflection: false,
        check_ffi: false,
        attribute_entries: vec![],
        edge_types: vec![EdgeType::Calls],
    };
    let detector = DeadCodeDetector::with_config(&*storage, cfg);
    let result = detector.detect("demo", &[]).expect("detect");
    assert_eq!(result.len(), 1, "a should be dead with empty patterns");
}

// --- T003: multi-edge-type reference detection tests ---

#[test]
fn detect_usage_edge_prevents_dead_code() {
    // B5: a USAGE edge propagates reachability from a seed source to its
    // target. `bar` is configured as an entry-pattern seed; `foo` is
    // reachable from `bar` via USAGE, so neither is dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
    create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
    // bar uses foo → foo is reachable from seed bar.
    create_edge(&kit, "e1", "f_bar", "f_foo", "demo", "USAGE");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["bar"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"foo"),
        "foo reachable from seed bar via USAGE"
    );
    assert!(!names.contains(&"bar"), "bar is a seed (entry pattern)");
}

#[test]
fn detect_handles_route_edge_prevents_dead_code() {
    // B5: a HANDLES_ROUTE edge propagates reachability from a seed source
    // to its target. `reg` is configured as an entry-pattern seed;
    // `handler` is reachable from `reg` via HANDLES_ROUTE.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_handler",
        "demo",
        "handler",
        "demo.handler",
        "/src/lib.rs",
        1,
    );
    create_function(&kit, "f_reg", "demo", "reg", "demo.reg", "/src/lib.rs", 5);
    // reg -> handler (HANDLES_ROUTE) → handler reachable from seed reg.
    create_edge(&kit, "e1", "f_reg", "f_handler", "demo", "HANDLES_ROUTE");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["reg"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"handler"),
        "handler reachable from seed reg via HANDLES_ROUTE"
    );
    assert!(!names.contains(&"reg"), "reg is a seed (entry pattern)");
}

#[test]
fn detect_tests_edge_prevents_dead_code() {
    // B5: a TESTS edge propagates reachability from a seed source to its
    // target. `ttest` is configured as an entry-pattern seed; `target` is
    // reachable from `ttest` via TESTS.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_target",
        "demo",
        "target",
        "demo.target",
        "/src/lib.rs",
        1,
    );
    create_function(
        &kit,
        "f_ttest",
        "demo",
        "ttest",
        "demo.ttest",
        "/src/lib.rs",
        5,
    );
    // ttest tests target → target reachable from seed ttest.
    create_edge(&kit, "e1", "f_ttest", "f_target", "demo", "TESTS");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["ttest"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"target"),
        "target reachable from seed ttest via TESTS"
    );
    assert!(!names.contains(&"ttest"), "ttest is a seed (entry pattern)");
}

#[test]
fn detect_all_edge_types_exhaustive_no_incoming_is_dead() {
    // R-dead_code-001: a function with no incoming edges of ANY type is dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_lone",
        "demo",
        "lone",
        "demo.lone",
        "/src/lib.rs",
        1,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"lone"),
        "lone has zero incoming edges → dead"
    );
}

#[test]
fn has_incoming_edge_returns_true_for_existing_edge() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
    create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
    create_edge(&kit, "e1", "f_a", "f_b", "demo", "USAGE");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    assert!(
        detector
            .has_incoming_edge("f_b", EdgeType::Usage)
            .expect("has_incoming_edge"),
        "f_b should have USAGE incoming edge"
    );
}

#[test]
fn has_incoming_edge_returns_false_for_missing_edge() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    assert!(
        !detector
            .has_incoming_edge("f_a", EdgeType::Calls)
            .expect("has_incoming_edge"),
        "f_a has no CALLS incoming edge"
    );
}

#[test]
fn has_incoming_edge_distinguishes_edge_types() {
    // A function with a USAGE edge but no CALLS edge: USAGE=true, CALLS=false.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
    create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
    create_edge(&kit, "e1", "f_a", "f_b", "demo", "USAGE");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    assert!(
        detector
            .has_incoming_edge("f_b", EdgeType::Usage)
            .expect("has_incoming_edge"),
        "f_b has USAGE edge"
    );
    assert!(
        !detector
            .has_incoming_edge("f_b", EdgeType::Calls)
            .expect("has_incoming_edge"),
        "f_b has no CALLS edge"
    );
}

#[test]
fn load_referenced_ids_collects_targets_across_multiple_edge_types() {
    // Verifies the single IN-clause query captures targets across all
    // configured edge types (CALLS, USAGE, TESTS) in one pass.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_tgt_calls",
        "demo",
        "tgt_calls",
        "demo.tgt_calls",
        "/src/lib.rs",
        1,
    );
    create_function(
        &kit,
        "f_tgt_usage",
        "demo",
        "tgt_usage",
        "demo.tgt_usage",
        "/src/lib.rs",
        5,
    );
    create_function(
        &kit,
        "f_tgt_tests",
        "demo",
        "tgt_tests",
        "demo.tgt_tests",
        "/src/lib.rs",
        10,
    );
    create_function(
        &kit,
        "f_src_a",
        "demo",
        "src_a",
        "demo.src_a",
        "/src/lib.rs",
        20,
    );
    create_function(
        &kit,
        "f_src_b",
        "demo",
        "src_b",
        "demo.src_b",
        "/src/lib.rs",
        25,
    );
    create_function(
        &kit,
        "f_src_c",
        "demo",
        "src_c",
        "demo.src_c",
        "/src/lib.rs",
        30,
    );
    create_edge(&kit, "e1", "f_src_a", "f_tgt_calls", "demo", "CALLS");
    create_edge(&kit, "e2", "f_src_b", "f_tgt_usage", "demo", "USAGE");
    create_edge(&kit, "e3", "f_src_c", "f_tgt_tests", "demo", "TESTS");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let referenced = detector
        .load_referenced_ids("demo")
        .expect("load_referenced_ids");
    assert!(
        referenced.contains("f_tgt_calls"),
        "CALLS target should be referenced"
    );
    assert!(
        referenced.contains("f_tgt_usage"),
        "USAGE target should be referenced"
    );
    assert!(
        referenced.contains("f_tgt_tests"),
        "TESTS target should be referenced"
    );
    assert_eq!(
        referenced.len(),
        3,
        "exactly 3 targets should be referenced"
    );
}

// --- T004: exported function detection tests ---

#[test]
fn detect_excludes_exported_functions() {
    // R-dead_code-002: isExported=true with no incoming edges → NOT dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_pub",
        "demo",
        "pub_fn",
        "demo.pub_fn",
        "/src/lib.rs",
        1,
        true,
        "",
    );
    create_function(
        &kit,
        "f_priv",
        "demo",
        "priv_fn",
        "demo.priv_fn",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"pub_fn"),
        "exported pub_fn should NOT be dead"
    );
    assert!(names.contains(&"priv_fn"), "private priv_fn should be dead");
}

#[test]
fn detect_includes_exported_when_check_exported_false() {
    // When check_exported=false, exported functions ARE dead code.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_pub",
        "demo",
        "pub_fn",
        "demo.pub_fn",
        "/src/lib.rs",
        1,
        true,
        "",
    );

    let storage = storage(&kit);
    let cfg = DeadCodeConfig {
        check_exported: false,
        ..DeadCodeConfig::default()
    };
    let detector = DeadCodeDetector::with_config(&*storage, cfg);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"pub_fn"),
        "with check_exported=false, pub_fn IS dead"
    );
}

#[test]
fn batch_prefetch_exported_ids_distinguishes_pub_from_priv() {
    // Replaces `is_exported_returns_correct_value` (arch-1): the detector
    // no longer exposes a per-function `is_exported` method. `BatchPrefetch`
    // bulk-loads all exported ids in two Cypher round-trips; callers
    // check liveness via `HashSet::contains`.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_pub",
        "demo",
        "pub_fn",
        "demo.pub_fn",
        "/src/lib.rs",
        1,
        true,
        "",
    );
    create_function(
        &kit,
        "f_priv",
        "demo",
        "priv_fn",
        "demo.priv_fn",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(prefetch.is_exported("f_pub"), "f_pub should be exported");
    assert!(
        !prefetch.is_exported("f_priv"),
        "f_priv should NOT be exported"
    );
}

// --- T005: FFI entry point detection tests ---

#[test]
fn detect_excludes_ffi_entry_extern_c() {
    // R-dead_code-003: signature with `extern "C"` → NOT dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_ffi",
        "demo",
        "ffi_fn",
        "demo.ffi_fn",
        "/src/lib.rs",
        1,
        false,
        r#"pub extern "C" fn ffi_fn(x: i32) -> i32"#,
    );
    create_function(
        &kit,
        "f_plain",
        "demo",
        "plain",
        "demo.plain",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"ffi_fn"),
        "ffi_fn is an FFI entry → not dead"
    );
    assert!(names.contains(&"plain"), "plain has no FFI markers → dead");
}

#[test]
fn detect_excludes_ffi_entry_no_mangle() {
    // R-dead_code-003: signature with `#[no_mangle]` → NOT dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_nm",
        "demo",
        "native_fn",
        "demo.native_fn",
        "/src/lib.rs",
        1,
        false,
        "#[no_mangle]\npub fn native_fn() -> u32",
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"native_fn"),
        "native_fn has #[no_mangle] → not dead"
    );
}

#[test]
fn detect_includes_ffi_when_check_ffi_false() {
    // When check_ffi=false, FFI functions ARE dead code.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_ffi",
        "demo",
        "ffi_fn",
        "demo.ffi_fn",
        "/src/lib.rs",
        1,
        false,
        r#"extern "C" fn ffi_fn()"#,
    );

    let storage = storage(&kit);
    let cfg = DeadCodeConfig {
        check_ffi: false,
        ..DeadCodeConfig::default()
    };
    let detector = DeadCodeDetector::with_config(&*storage, cfg);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"ffi_fn"),
        "with check_ffi=false, ffi_fn IS dead"
    );
}

#[test]
fn batch_prefetch_distinguishes_ffi_from_plain() {
    // Replaces the old `is_ffi_entry_distinguishes_ffi_from_plain` test
    // (arch-1: `is_ffi_entry` was removed; FFI detection now goes
    // through `BatchPrefetch::load`).
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function_with_flags(
        &kit,
        "f_ffi",
        "demo",
        "ffi_fn",
        "demo.ffi_fn",
        "/src/lib.rs",
        1,
        false,
        r#"extern "C" fn ffi_fn()"#,
    );
    create_function(
        &kit,
        "f_plain",
        "demo",
        "plain",
        "demo.plain",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(
        prefetch.ffi_entry_ids.contains("f_ffi"),
        "f_ffi should be FFI entry"
    );
    assert!(
        !prefetch.ffi_entry_ids.contains("f_plain"),
        "f_plain should NOT be FFI entry"
    );
}

// --- T006: expanded entry point pattern tests ---

#[test]
fn detect_excludes_all_default_entry_patterns() {
    // R-dead_code-004: all 6 default entry patterns must be excluded.
    for entry_name in ["main", "Main", "__main__", "wmain", "WinMain", "DLLMain"] {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_entry",
            "demo",
            entry_name,
            &format!("demo.{entry_name}"),
            "/src/lib.rs",
            1,
        );
        // Also create a control function that IS dead.
        create_function(
            &kit,
            "f_dead",
            "demo",
            "dead_fn",
            "demo.dead_fn",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        // Pass empty entry_patterns — config defaults should still apply.
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&entry_name),
            "{entry_name} should be excluded by default config patterns"
        );
        assert!(names.contains(&"dead_fn"), "dead_fn should still be dead");
    }
}

#[test]
fn detect_excludes_custom_entry_patterns_parameter() {
    // R-dead_code-004: custom entry_patterns parameter still works.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_h",
        "demo",
        "handler",
        "demo.handler",
        "/src/lib.rs",
        1,
    );
    create_function(
        &kit,
        "f_d",
        "demo",
        "dead_fn",
        "demo.dead_fn",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["handler"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"handler"),
        "handler matches custom pattern"
    );
    assert!(names.contains(&"dead_fn"), "dead_fn is still dead");
}

#[test]
fn detect_merges_parameter_and_config_entry_patterns() {
    // Both the parameter and config patterns are checked.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // "main" is in config defaults; "custom_entry" is passed via parameter.
    create_function(&kit, "f_m", "demo", "main", "demo.main", "/src/lib.rs", 1);
    create_function(
        &kit,
        "f_c",
        "demo",
        "custom_entry",
        "demo.custom_entry",
        "/src/lib.rs",
        5,
    );
    create_function(
        &kit,
        "f_d",
        "demo",
        "dead_fn",
        "demo.dead_fn",
        "/src/lib.rs",
        10,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["custom_entry"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"main"), "main excluded by config");
    assert!(
        !names.contains(&"custom_entry"),
        "custom_entry excluded by parameter"
    );
    assert!(names.contains(&"dead_fn"), "dead_fn is dead");
}

// --- T007: confidence scoring tests ---

#[test]
fn detect_confidence_high_for_zero_incoming_edges() {
    // R-dead_code-005: no incoming edges of ANY type → High.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let entry = result
        .iter()
        .find(|e| e.name == "foo")
        .expect("foo should be dead");
    assert_eq!(
        entry.confidence,
        Confidence::High,
        "zero incoming edges → High"
    );
}

#[test]
fn detect_confidence_medium_for_non_calls_edge_only() {
    // R-dead_code-005: has USAGE but no CALLS → Medium.
    // Config with edge_types=[Calls] only: USAGE doesn't count as "used".
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
    create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
    // bar uses foo → foo has a USAGE incoming edge.
    create_edge(&kit, "e1", "f_bar", "f_foo", "demo", "USAGE");

    let storage = storage(&kit);
    let cfg = DeadCodeConfig {
        edge_types: vec![EdgeType::Calls],
        ..DeadCodeConfig::default()
    };
    let detector = DeadCodeDetector::with_config(&*storage, cfg);
    let result = detector.detect("demo", &[]).expect("detect");
    // foo is dead because USAGE is not in config.edge_types.
    let foo_entry = result
        .iter()
        .find(|e| e.name == "foo")
        .expect("foo should be dead (USAGE not in config.edge_types)");
    assert_eq!(
        foo_entry.confidence,
        Confidence::Medium,
        "USAGE but no CALLS → Medium"
    );
}

#[test]
fn detect_confidence_low_for_calls_edge_with_empty_config() {
    // R-dead_code-005: has CALLS but config doesn't check CALLS → Low.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
    create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
    // bar calls foo → foo has a CALLS incoming edge.
    create_calls_edge(&kit, "e1", "f_bar", "f_foo", "demo");

    let storage = storage(&kit);
    let cfg = DeadCodeConfig {
        edge_types: vec![], // empty: nothing counts as "used"
        ..DeadCodeConfig::default()
    };
    let detector = DeadCodeDetector::with_config(&*storage, cfg);
    let result = detector.detect("demo", &[]).expect("detect");
    let foo_entry = result
        .iter()
        .find(|e| e.name == "foo")
        .expect("foo should be dead (empty edge_types)");
    assert_eq!(
        foo_entry.confidence,
        Confidence::Low,
        "has CALLS incoming edge → Low"
    );
}

#[test]
fn detect_confidence_serializes_in_dead_code_entry() {
    // Confidence field must appear in serialized JSON.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let entry = result
        .iter()
        .find(|e| e.name == "foo")
        .expect("foo should be dead");
    let json = serde_json::to_string(entry).expect("serialize");
    assert!(
        json.contains("\"confidence\":\"High\""),
        "JSON should contain confidence field: {json}"
    );
}

// --- Additional coverage tests (targeting uncovered lines) ---

#[test]
fn batch_prefetch_exported_ids_empty_for_nonexistent_id() {
    // Replaces `is_exported_returns_false_for_nonexistent_id` (arch-1).
    // `BatchPrefetch::load` only returns ids that actually exist in the
    // Function/Method tables — nonexistent ids are simply absent from
    // the returned HashSet.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);

    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(
        !prefetch.is_exported("nonexistent_id"),
        "nonexistent id should not be in exported_ids"
    );
    assert!(
        prefetch.exported_ids_len() == 0,
        "empty db → empty exported_ids"
    );
}

#[test]
fn batch_prefetch_ffi_entry_ids_empty_for_nonexistent_id() {
    // Replaces `is_ffi_entry_returns_false_for_nonexistent_id` (arch-1).
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);

    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(
        !prefetch.ffi_entry_ids.contains("nonexistent_id"),
        "nonexistent id should not be in ffi_entry_ids"
    );
    assert!(
        prefetch.ffi_entry_ids_len() == 0,
        "empty db → empty ffi_entry_ids"
    );
}

// --- Additional coverage: glob_helper edge cases ---

#[test]
fn glob_match_returns_false_when_pattern_char_but_empty_text() {
    // Line 469: `(Some(_), None) => false` — non-`*` pattern char with
    // empty text cannot match.
    assert!(!glob_match("a", ""));
    assert!(!glob_match("abc", ""));
}

#[test]
fn glob_match_returns_false_when_first_char_mismatches() {
    // Line 470: `*pc == *tc` evaluates false → short-circuits to false.
    assert!(!glob_match("a", "b"));
    assert!(!glob_match("xa", "yb"));
}

// --- Additional coverage: load_referenced_ids early return ---

#[test]
fn load_referenced_ids_returns_empty_when_edge_types_empty() {
    // Line 285-287: `if self.config.edge_types.is_empty() { return Ok(HashSet::new()) }`
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
    create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
    create_calls_edge(&kit, "e1", "f_a", "f_b", "demo");

    let storage = storage(&kit);
    let cfg = DeadCodeConfig {
        edge_types: vec![],
        ..DeadCodeConfig::default()
    };
    let detector = DeadCodeDetector::with_config(&*storage, cfg);
    let referenced = detector
        .load_referenced_ids("demo")
        .expect("load_referenced_ids");
    assert!(
        referenced.is_empty(),
        "empty edge_types → empty referenced set"
    );
}

// --- Additional coverage: Method label iteration in is_exported / is_ffi_entry ---

/// Creates a Method node with `isExported` and `signature` flags.
#[allow(clippy::too_many_arguments)]
fn create_method_with_flags(
    kit: &AsyncKit<AsyncReady>,
    id: &str,
    project: &str,
    name: &str,
    qn: &str,
    file: &str,
    line: u32,
    is_exported: bool,
    signature: &str,
) {
    let storage = storage(kit);
    let end_line = line + 10;
    let cypher = format!(
        "CREATE (:Method {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '{}', returnType: '', \
             isExported: {}, docstring: '', content: '', parameterCount: 0, parentQn: ''}});",
        escape_cypher_string(id),
        escape_cypher_string(project),
        escape_cypher_string(name),
        escape_cypher_string(qn),
        escape_cypher_string(file),
        line,
        end_line,
        escape_cypher_string(signature),
        is_exported,
    );
    storage.execute(&cypher).expect("create method with flags");
}

#[test]
fn batch_prefetch_exported_ids_includes_method_label() {
    // Replaces `is_exported_checks_method_label` (arch-1). Verifies
    // `BatchPrefetch::load` picks up exported Method nodes, not just
    // Function nodes.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_method_with_flags(
        &kit,
        "m_pub",
        "demo",
        "method_pub",
        "demo.Class.method_pub",
        "/src/lib.rs",
        1,
        true,
        "",
    );
    create_method(
        &kit,
        "m_priv",
        "demo",
        "method_priv",
        "demo.Class.method_priv",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(
        prefetch.is_exported("m_pub"),
        "m_pub should be exported (Method label)"
    );
    assert!(
        !prefetch.is_exported("m_priv"),
        "m_priv should NOT be exported (Method label)"
    );
}

#[test]
fn batch_prefetch_ffi_entry_ids_includes_method_label() {
    // Replaces `is_ffi_entry_checks_method_label` (arch-1).
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_method_with_flags(
        &kit,
        "m_ffi",
        "demo",
        "method_ffi",
        "demo.Class.method_ffi",
        "/src/lib.rs",
        1,
        false,
        r#"extern "C" fn method_ffi()"#,
    );
    create_method(
        &kit,
        "m_plain",
        "demo",
        "method_plain",
        "demo.Class.method_plain",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(
        prefetch.ffi_entry_ids.contains("m_ffi"),
        "m_ffi should be FFI entry (Method label)"
    );
    assert!(
        !prefetch.ffi_entry_ids.contains("m_plain"),
        "m_plain should NOT be FFI entry (Method label)"
    );
}

#[test]
fn detect_excludes_exported_method_nodes() {
    // Integration: an exported Method with zero incoming edges is NOT dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_method_with_flags(
        &kit,
        "m_pub",
        "demo",
        "method_pub",
        "demo.Class.method_pub",
        "/src/lib.rs",
        1,
        true,
        "",
    );
    create_method(
        &kit,
        "m_priv",
        "demo",
        "method_priv",
        "demo.Class.method_priv",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"method_pub"),
        "exported Method should NOT be dead"
    );
    assert!(
        names.contains(&"method_priv"),
        "non-exported Method should be dead"
    );
}

#[test]
fn detect_excludes_ffi_method_nodes() {
    // Integration: a Method with FFI signature and zero incoming edges is NOT dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_method_with_flags(
        &kit,
        "m_ffi",
        "demo",
        "method_ffi",
        "demo.Class.method_ffi",
        "/src/lib.rs",
        1,
        false,
        r#"extern "C" fn method_ffi()"#,
    );
    create_method(
        &kit,
        "m_plain",
        "demo",
        "method_plain",
        "demo.Class.method_plain",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"method_ffi"),
        "FFI Method should NOT be dead"
    );
    assert!(
        names.contains(&"method_plain"),
        "non-FFI Method should be dead"
    );
}

// --- B0: #tests disambiguator recognition (CalNexus 66% false positives) ---

#[test]
fn detect_excludes_functions_inside_mod_tests_block() {
    // B0 fix: In Rust, `mod tests { fn foo() {} }` produces a QN with
    // `#tests` disambiguator (e.g. `demo.src.lib.rs.foo#tests`). These are
    // test-module-scoped functions and should NOT be flagged as dead.
    // This was the largest false-positive source on CalNexus (239/360 = 66%).
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_tests_foo",
        "demo",
        "foo",
        "demo.src.lib.rs.foo#tests",
        "/src/lib.rs",
        1,
    );
    create_function(
        &kit,
        "f_tests_bar",
        "demo",
        "bar",
        "demo.src.lib.rs.bar#tests_ConfigurableMockDomain",
        "/src/lib.rs",
        10,
    );
    // Control: a non-test function with no incoming edges IS dead.
    create_function(
        &kit,
        "f_plain",
        "demo",
        "plain",
        "demo.plain",
        "/src/lib.rs",
        20,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"foo"),
        "foo inside mod tests (#tests disambiguator) should NOT be dead"
    );
    assert!(
        !names.contains(&"bar"),
        "bar inside mod tests (#tests_ConfigurableMockDomain) should NOT be dead"
    );
    assert!(names.contains(&"plain"), "plain (no disambiguator) is dead");
}

// --- B2: expanded test patterns (it_*/sec_*/snap_*/perf_*/bench_*) ---

#[test]
fn detect_excludes_expanded_test_prefix_patterns() {
    // B2 fix: CalNexus uses it_*/sec_*/snap_*/perf_*/bench_* prefixes
    // for integration/security/snapshot/performance/benchmark tests.
    // DEFAULT_TEST_PATTERNS must cover these.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_it",
        "demo",
        "it_cli_001",
        "demo.it_cli_001",
        "/tests/cli.rs",
        1,
    );
    create_function(
        &kit,
        "f_sec",
        "demo",
        "sec_001_injection",
        "demo.sec_001_injection",
        "/tests/sec.rs",
        10,
    );
    create_function(
        &kit,
        "f_snap",
        "demo",
        "snap_001_diff",
        "demo.snap_001_diff",
        "/tests/snap.rs",
        20,
    );
    create_function(
        &kit,
        "f_perf",
        "demo",
        "perf_001_baseline",
        "demo.perf_001_baseline",
        "/tests/perf.rs",
        30,
    );
    create_function(
        &kit,
        "f_bench",
        "demo",
        "bench_decode_small",
        "demo.bench_decode_small",
        "/benches/decode.rs",
        40,
    );
    // Control: a non-test function with no incoming edges IS dead.
    create_function(
        &kit,
        "f_plain",
        "demo",
        "plain",
        "demo.plain",
        "/src/lib.rs",
        50,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"it_cli_001"), "it_* should NOT be dead");
    assert!(
        !names.contains(&"sec_001_injection"),
        "sec_* should NOT be dead"
    );
    assert!(
        !names.contains(&"snap_001_diff"),
        "snap_* should NOT be dead"
    );
    assert!(
        !names.contains(&"perf_001_baseline"),
        "perf_* should NOT be dead"
    );
    assert!(
        !names.contains(&"bench_decode_small"),
        "bench_* should NOT be dead"
    );
    assert!(names.contains(&"plain"), "plain (no test prefix) is dead");
}

// --- B3: trait impl method recognition ---

#[test]
fn detect_excludes_trait_impl_methods_when_dynamic_dispatch_enabled() {
    // B3 fix: Trait impl methods (e.g. `impl Display for X { fn fmt() {} }`)
    // produce Method nodes with disambiguator `#Display`, `#ReplHelper`, etc.
    // These are called via dynamic dispatch and should NOT be flagged as dead
    // when check_dynamic_dispatch=true.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_method(
        &kit,
        "m_fmt",
        "demo",
        "fmt",
        "demo.src.lib.rs.fmt#Display",
        "/src/lib.rs",
        5,
    );
    create_method(
        &kit,
        "m_complete",
        "demo",
        "complete",
        "demo.src.repl.rs.complete#ReplHelper",
        "/src/repl.rs",
        15,
    );
    // Control: a non-trait-impl method with no incoming edges IS dead.
    create_method(
        &kit,
        "m_plain",
        "demo",
        "plain_method",
        "demo.src.lib.rs.plain_method",
        "/src/lib.rs",
        25,
    );

    let storage = storage(&kit);
    let config = DeadCodeConfig {
        check_dynamic_dispatch: true,
        ..DeadCodeConfig::default()
    };
    let detector = DeadCodeDetector::with_config(&*storage, config);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"fmt"),
        "trait impl fmt#Display should NOT be dead when check_dynamic_dispatch=true"
    );
    assert!(
        !names.contains(&"complete"),
        "trait impl complete#ReplHelper should NOT be dead when check_dynamic_dispatch=true"
    );
    assert!(
        names.contains(&"plain_method"),
        "non-trait-impl method is dead"
    );
}

#[test]
fn detect_flags_trait_impl_methods_when_dynamic_dispatch_disabled() {
    // B3: when check_dynamic_dispatch=false (opt-out), trait impl methods
    // ARE flagged as dead. Default is `true` since B3.5, so we must
    // explicitly disable it here.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_method(
        &kit,
        "m_fmt",
        "demo",
        "fmt",
        "demo.src.lib.rs.fmt#Display",
        "/src/lib.rs",
        5,
    );

    let storage = storage(&kit);
    let config = DeadCodeConfig {
        check_dynamic_dispatch: false,
        ..DeadCodeConfig::default()
    };
    let detector = DeadCodeDetector::with_config(&*storage, config);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"fmt"),
        "trait impl fmt#Display IS dead when check_dynamic_dispatch=false (opt-out)"
    );
}

// --- B4: integration test file recognition ---

#[test]
fn is_integration_test_file_recognizes_rust_tests_dir() {
    // Rust top-level integration tests live in `tests/` directory.
    assert!(is_integration_test_file("tests/numerical_linalg_test.rs"));
    assert!(is_integration_test_file("tests/repl_integration.rs"));
    assert!(is_integration_test_file("tests/helpers/mod.rs"));
}

#[test]
fn is_integration_test_file_recognizes_python_tests_dir() {
    // Python tests live in `tests/` or `test/` directory.
    assert!(is_integration_test_file("tests/test_foo.py"));
    assert!(is_integration_test_file("test/test_bar.py"));
    assert!(is_integration_test_file("src/tests/test_baz.py"));
}

#[test]
fn is_integration_test_file_recognizes_jvm_src_test_dir() {
    // Java/Kotlin/Scala tests live in `src/test/`.
    assert!(is_integration_test_file(
        "src/test/java/com/example/FooTest.java"
    ));
    assert!(is_integration_test_file("src/test/kotlin/FooTest.kt"));
}

#[test]
fn is_integration_test_file_rejects_production_source() {
    // Production source files must NOT be flagged as integration tests.
    assert!(!is_integration_test_file("src/lib.rs"));
    assert!(!is_integration_test_file("src/main.rs"));
    assert!(!is_integration_test_file("src/cli.rs"));
    assert!(!is_integration_test_file("src/domains/numerical.rs"));
}

#[test]
fn is_integration_test_file_handles_windows_paths() {
    // Windows-style paths should be normalized and recognized.
    assert!(is_integration_test_file("tests\\foo.rs"));
    assert!(is_integration_test_file("src\\tests\\bar.py"));
}

#[test]
fn detect_excludes_integration_test_functions_in_tests_dir() {
    // B4 fix: Functions in `tests/` directory are integration tests
    // discovered by `cargo test` / `pytest` / `go test`. They have no
    // static CALLS edge and should NOT be flagged as dead.
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // Integration test with descriptive name (no test_*/it_* prefix).
    create_function(
        &kit,
        "f_it",
        "demo",
        "eig_end_to_end_returns_json_with_values_and_vectors",
        "demo.tests.numerical_linalg_test.rs.eig_end_to_end_returns_json_with_values_and_vectors",
        "tests/numerical_linalg_test.rs",
        58,
    );
    create_function(
        &kit,
        "f_it2",
        "demo",
        "repl_infrastructure_present",
        "demo.tests.repl_integration.rs.repl_infrastructure_present",
        "tests/repl_integration.rs",
        157,
    );
    // Control: a production function with no incoming edges IS dead.
    create_function(
        &kit,
        "f_prod",
        "demo",
        "unused_helper",
        "demo.src.lib.rs.unused_helper",
        "src/lib.rs",
        100,
    );

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"eig_end_to_end_returns_json_with_values_and_vectors"),
        "integration test in tests/ should NOT be dead (B4)"
    );
    assert!(
        !names.contains(&"repl_infrastructure_present"),
        "integration test in tests/ should NOT be dead (B4)"
    );
    assert!(
        names.contains(&"unused_helper"),
        "production function with no incoming edges IS dead"
    );
}

// ===== B5: worklist reachability propagation =====

/// B5 core: verifies that worklist propagation marks indirectly
/// reachable functions as live, while unreachable functions (even if
/// they have incoming edges from dead functions) are correctly flagged
/// as dead.
///
/// Graph:
/// ```text
/// entry -> a -> b   (reachable chain from entry seed)
/// c -> d            (unreachable: c is not a seed, d has incoming
///                    edge from c but c itself is dead)
/// ```
///
/// Without B5 (single-layer `referenced_ids` check), `d` would be
/// incorrectly marked live because `c -> d` exists. With B5, `d` is
/// dead because `c` is unreachable from any seed.
#[test]
fn test_reachability_propagation_basic() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // entry -> a -> b (reachable chain)
    create_function(
        &kit,
        "f_entry",
        "demo",
        "entry",
        "demo.entry",
        "/src/main.rs",
        1,
    );
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
    create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 10);
    // c -> d (unreachable chain)
    create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/lib.rs", 20);
    create_function(&kit, "f_d", "demo", "d", "demo.d", "/src/lib.rs", 30);
    create_file(&kit, "file1", "demo", "/src/main.rs", "rust");
    create_file(&kit, "file2", "demo", "/src/lib.rs", "rust");
    create_calls_edge(&kit, "e1", "f_entry", "f_a", "demo");
    create_calls_edge(&kit, "e2", "f_a", "f_b", "demo");
    create_calls_edge(&kit, "e3", "f_c", "f_d", "demo");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["entry"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    // Reachable from entry: entry (seed), a (entry->a), b (a->b).
    assert!(!names.contains(&"entry"), "entry is seed: {:?}", names);
    assert!(!names.contains(&"a"), "a reachable from entry: {:?}", names);
    assert!(!names.contains(&"b"), "b reachable via a: {:?}", names);
    // Unreachable: c (no incoming edges, not a seed), d (only reachable
    // via c which is itself dead).
    assert!(names.contains(&"c"), "c not reachable: {:?}", names);
    assert!(
        names.contains(&"d"),
        "B5: d is dead even though c->d exists (c is unreachable): {:?}",
        names
    );
}

/// B5: trait impl methods are seeds when `check_dynamic_dispatch=true`.
/// Verifies the trait impl method is in `live_set` and any function it
/// calls is also reachable.
#[test]
fn test_reachability_with_trait_impl() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // Trait impl method (B3 seed) calls a free function.
    // qualified_name has `#Display` disambiguator.
    create_method(
        &kit,
        "m_fmt",
        "demo",
        "fmt",
        "demo.src.lib.rs.fmt#Display",
        "/src/lib.rs",
        1,
    );
    create_function(
        &kit,
        "f_helper",
        "demo",
        "helper",
        "demo.helper",
        "/src/lib.rs",
        10,
    );
    create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
    create_calls_edge(&kit, "e1", "m_fmt", "f_helper", "demo");

    let storage = storage(&kit);
    // Default config has check_dynamic_dispatch=true (B3.5).
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    // Trait impl method is a seed (B3).
    assert!(
        !names.contains(&"fmt"),
        "B5: trait impl method fmt#Display is seed: {:?}",
        names
    );
    // helper is reachable from fmt (seed) via CALLS edge.
    assert!(
        !names.contains(&"helper"),
        "B5: helper reachable from trait impl seed: {:?}",
        names
    );
}

/// B5: private unused functions (no incoming edges, not a seed) are
/// correctly flagged as dead. This is the most basic case — verifies
/// the analyzer does not over-approximate the live set.
#[test]
fn test_reachability_excludes_private_unused() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // Private unused function (no incoming edges, not exported, not FFI,
    // not a test, not an entry, not a trait impl).
    create_function(
        &kit,
        "f_unused",
        "demo",
        "unused",
        "demo.unused",
        "/src/lib.rs",
        1,
    );
    // main is the entry seed.
    create_function(
        &kit,
        "f_main",
        "demo",
        "main",
        "demo.main",
        "/src/main.rs",
        1,
    );
    create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
    create_file(&kit, "file2", "demo", "/src/main.rs", "rust");

    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["main"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"unused"),
        "B5: private unused function IS dead: {:?}",
        names
    );
    assert!(
        !names.contains(&"main"),
        "B5: main is entry seed, NOT dead: {:?}",
        names
    );
}

// ===== perf-1 + arch-1: BatchPrefetch + DRY consolidation =====

/// perf-1: `BatchPrefetch::load` returns ALL exported function ids in
/// a single Cypher query (vs the previous 2N pattern where
/// `is_exported` was called per function with 2 label queries each).
#[test]
fn batch_prefetch_loads_all_exported_ids_in_one_query() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // Create 5 functions, 3 of them marked isExported=true.
    create_function_with_flags(
        &kit,
        "f1",
        "demo",
        "f1",
        "demo.f1",
        "/src/lib.rs",
        1,
        true,
        "",
    );
    create_function_with_flags(
        &kit,
        "f2",
        "demo",
        "f2",
        "demo.f2",
        "/src/lib.rs",
        5,
        true,
        "",
    );
    create_function_with_flags(
        &kit,
        "f3",
        "demo",
        "f3",
        "demo.f3",
        "/src/lib.rs",
        10,
        true,
        "",
    );
    create_function_with_flags(
        &kit,
        "f4",
        "demo",
        "f4",
        "demo.f4",
        "/src/lib.rs",
        15,
        false,
        "",
    );
    create_function_with_flags(
        &kit,
        "f5",
        "demo",
        "f5",
        "demo.f5",
        "/src/lib.rs",
        20,
        false,
        "",
    );
    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(prefetch.is_exported("f1"));
    assert!(prefetch.is_exported("f2"));
    assert!(prefetch.is_exported("f3"));
    assert!(!prefetch.is_exported("f4"), "f4 is not exported");
    assert!(!prefetch.is_exported("f5"), "f5 is not exported");
    assert_eq!(prefetch.exported_ids.len(), 3);
}

/// perf-1: `BatchPrefetch::load` returns ALL FFI entry ids in a single
/// Cypher query (vs the previous 2N pattern).
#[test]
fn batch_prefetch_loads_all_ffi_entries_in_one_query() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // 3 FFI entries with different FFI markers.
    create_function_with_flags(
        &kit,
        "f1",
        "demo",
        "f1",
        "demo.f1",
        "/src/lib.rs",
        1,
        false,
        r#"pub extern "C" fn f1(x: i32) -> i32"#,
    );
    create_function_with_flags(
        &kit,
        "f2",
        "demo",
        "f2",
        "demo.f2",
        "/src/lib.rs",
        5,
        false,
        "#[no_mangle]\npub fn f2() -> i32",
    );
    create_function_with_flags(
        &kit,
        "f3",
        "demo",
        "f3",
        "demo.f3",
        "/src/lib.rs",
        10,
        false,
        r#"extern "C" { fn f3(); }"#,
    );
    // 1 non-FFI function as control.
    create_function_with_flags(
        &kit,
        "f4",
        "demo",
        "f4",
        "demo.f4",
        "/src/lib.rs",
        15,
        false,
        "pub fn f4() {}",
    );
    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    assert!(prefetch.ffi_entry_ids.contains("f1"), "f1 has extern \"C\"");
    assert!(prefetch.ffi_entry_ids.contains("f2"), "f2 has #[no_mangle]");
    assert!(prefetch.ffi_entry_ids.contains("f3"), "f3 has extern \"C\"");
    assert!(!prefetch.ffi_entry_ids.contains("f4"), "f4 is not FFI");
    assert_eq!(prefetch.ffi_entry_ids.len(), 3);
}

/// perf-1: `BatchPrefetch::load` returns ALL outgoing edges grouped by
/// source id, so `propagate()` can do an O(1) HashMap lookup per pop
/// instead of an O(1) Cypher round-trip per pop.
#[test]
fn batch_prefetch_loads_outgoing_edges_grouped_by_source() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
    create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
    create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/lib.rs", 10);
    // f_a calls f_b and f_c (2 outgoing edges from f_a).
    create_calls_edge(&kit, "e1", "f_a", "f_b", "demo");
    create_calls_edge(&kit, "e2", "f_a", "f_c", "demo");
    // f_b calls f_c (1 outgoing edge from f_b).
    create_calls_edge(&kit, "e3", "f_b", "f_c", "demo");
    let storage = storage(&kit);
    let config = DeadCodeConfig::default();
    let functions = load_all_functions(&*storage, "demo").expect("functions");
    let prefetch = BatchPrefetch::load(&*storage, "demo", &config, &functions).expect("prefetch");
    let a_targets = prefetch
        .outgoing_edges("f_a")
        .expect("f_a has outgoing edges");
    assert_eq!(a_targets.len(), 2, "f_a calls f_b and f_c");
    assert!(a_targets.contains(&"f_b".to_string()));
    assert!(a_targets.contains(&"f_c".to_string()));
    let b_targets = prefetch
        .outgoing_edges("f_b")
        .expect("f_b has outgoing edges");
    assert_eq!(b_targets.len(), 1, "f_b calls f_c");
    assert!(b_targets.contains(&"f_c".to_string()));
    assert!(
        prefetch.outgoing_edges("f_c").is_none(),
        "f_c has no outgoing edges"
    );
}

/// perf-1 + arch-1 regression: `detect()` returns correct results after
/// batch prefetch refactor. Re-verifies B5 reachability propagation
/// with batch-prefetched edges (3-query pattern instead of 4N+V).
///
/// Graph:
/// ```text
/// entry -> a -> b -> c   (reachable chain from entry seed)
/// d                       (unreachable: no incoming edges, not a seed)
/// ```
#[test]
fn detect_uses_batch_prefetch_for_reachability_propagation() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    create_function(
        &kit,
        "f_entry",
        "demo",
        "entry",
        "demo.entry",
        "/src/main.rs",
        1,
    );
    create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
    create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 10);
    create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/lib.rs", 20);
    // d is unreachable (no incoming edges, not a seed).
    create_function(&kit, "f_d", "demo", "d", "demo.d", "/src/lib.rs", 30);
    create_file(&kit, "file1", "demo", "/src/main.rs", "rust");
    create_file(&kit, "file2", "demo", "/src/lib.rs", "rust");
    create_calls_edge(&kit, "e1", "f_entry", "f_a", "demo");
    create_calls_edge(&kit, "e2", "f_a", "f_b", "demo");
    create_calls_edge(&kit, "e3", "f_b", "f_c", "demo");
    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &["entry"]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"entry"), "entry is seed");
    assert!(!names.contains(&"a"), "a reachable from entry");
    assert!(!names.contains(&"b"), "b reachable via a");
    assert!(!names.contains(&"c"), "c reachable via b");
    assert!(names.contains(&"d"), "d is unreachable (dead)");
}

/// perf-1 + arch-1 regression: `detect()` correctly identifies
/// exported functions as live when they have no incoming edges
/// (using batch-prefetched `exported_ids`).
#[test]
fn detect_uses_batch_prefetch_for_exported_function_liveness() {
    let db = fresh_db_path();
    let kit = build_kit_for_db(&db);
    // `pub_exported` is marked isExported=true and has no incoming
    // CALLS edges. With `check_exported=true` (default), it should be
    // treated as a seed and not flagged as dead.
    create_function_with_flags(
        &kit,
        "f_pub",
        "demo",
        "pub_exported",
        "demo.pub_exported",
        "/src/lib.rs",
        1,
        true,
        "",
    );
    // `unused_private` is not exported, has no incoming edges, is not
    // a test/entry/trait-impl — IS dead.
    create_function(
        &kit,
        "f_priv",
        "demo",
        "unused_private",
        "demo.unused_private",
        "/src/lib.rs",
        10,
    );
    create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
    let storage = storage(&kit);
    let detector = DeadCodeDetector::new(&*storage);
    let result = detector.detect("demo", &[]).expect("detect");
    let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&"pub_exported"),
        "exported function is a seed (live)"
    );
    assert!(
        names.contains(&"unused_private"),
        "non-exported function with no incoming edges IS dead"
    );
}
