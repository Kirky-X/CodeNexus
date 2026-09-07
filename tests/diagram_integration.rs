// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Integration tests for the `diagram` command (absorb-archify T022).
//!
//! Exercises `run_diagram` end-to-end against a seeded LadybugDB: module
//! typing from real layer facts, cycle/cross-service edge variants, and the
//! showcase evidence gate. `arch_diff` coverage lives in
//! `service::arch_diff` unit tests (same core pipeline).

use codenexus::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
use codenexus::service::diagram::run_diagram;
use codenexus::service::error::CodeNexusError;
use codenexus::storage::capability::Storage;

fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
    let config = KitBootstrapConfig::new(db.to_path_buf());
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(build_kit(&config))
        .expect("build_kit")
}

/// Seeds: src/api (controller handling a route), src/svc (called via fetch
/// → cross-service), src/db — with an api→db→api CALLS cycle.
fn seed_rich_project(storage: &dyn Storage) {
    storage.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/demo', language: 'rust', fileCount: 3, indexedAt: 1000, lastCommit: 'abc'});").expect("project");
    storage.execute("CREATE (:Function {id: 'f_ctrl', project: 'demo', name: 'list_users', qualifiedName: 'demo.list_users', filePath: '/src/api/h.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("ctrl");
    storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("route");
    storage.execute("CREATE (:CodeRelation {id: 'e_hr', source: 'f_ctrl', target: 'r1', type: 'HANDLES_ROUTE', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("handles");
    storage.execute("CREATE (:Function {id: 'f_svc', project: 'demo', name: 'call_service', qualifiedName: 'demo.call_service', filePath: '/src/svc/call.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("svc");
    storage.execute("CREATE (:Function {id: 'f_db', project: 'demo', name: 'query_users', qualifiedName: 'demo.query_users', filePath: '/src/db/q.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("db");
    // cycle: api → db and db → api
    storage.execute("CREATE (:CodeRelation {id: 'e_ad', source: 'f_ctrl', target: 'f_db', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("api->db");
    storage.execute("CREATE (:CodeRelation {id: 'e_da', source: 'f_db', target: 'f_ctrl', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("db->api");
}

#[test]
fn diagram_int_renders_variants_and_types_from_graph_facts() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("it.db");
    let kit = build_kit_for_db(&db);
    {
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_rich_project(&*storage);
    }
    let target = dir.path().join("arch.html");

    let out = run_diagram(
        &kit,
        "demo",
        target.to_str().unwrap(),
        "standard",
        "",
        "",
        "",
        "en",
    )
    .expect("run_diagram should succeed");
    assert!(target.exists());
    assert!(out.receipt.validation.check_count > 0);

    let html = String::from_utf8(std::fs::read(&target).unwrap()).unwrap();
    // Interface typing comes from the HANDLES_ROUTE layer fact.
    assert!(
        html.contains("node-interface\" data-node-id=\"src-api\""),
        "controller module must be typed interface"
    );
    // The api→db→api cycle must surface as an Error-variant edge.
    assert!(
        html.contains("data-variant=\"error\""),
        "cycle edge visible"
    );
    // The fetch-to-route fact must surface as an Async edge.
    assert!(
        html.contains("data-variant=\"async\""),
        "cross-service edge visible"
    );
    // Truth boundary is always disclosed.
    assert!(html.contains("static index facts"));
}

#[test]
fn diagram_int_showcase_blocks_on_invalid_evidence() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("it.db");
    let kit = build_kit_for_db(&db);
    {
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_rich_project(&*storage);
    }
    let target = dir.path().join("arch.html");

    // The seeded lastCommit "abc" is not a real revision in any repo, so
    // evidence verification fails; showcase must refuse to ship.
    let repo_root = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let err = run_diagram(
        &kit,
        "demo",
        target.to_str().unwrap(),
        "showcase",
        &repo_root,
        "",
        "",
        "en",
    )
    .expect_err("showcase must block on invalid evidence");
    assert!(
        matches!(err, CodeNexusError::InvalidInput(ref msg) if msg.contains("evidence")),
        "{err:?}"
    );
    assert!(
        !target.exists(),
        "blocked render must not write the artifact"
    );
}
