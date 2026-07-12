// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `architecture` service: high-level project structure overview.

use serde::Serialize;

#[cfg(feature = "analysis")]
use crate::analysis::architecture::{ArchitectureAnalyzer, ArchitectureOverview};
#[cfg(feature = "analysis")]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "analysis", any(feature = "cli", feature = "mcp")))]
use crate::service::error::to_api_error;
#[cfg(feature = "analysis")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "analysis", any(feature = "cli", feature = "mcp")))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "analysis", any(feature = "cli", feature = "mcp")))]
use crate::service::runtime::kit;

#[cfg(all(feature = "analysis", any(feature = "cli", feature = "mcp")))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "analysis", any(feature = "cli", feature = "mcp")))]
use sdforge::service_api;

/// JSON-serializable architecture output.
#[cfg(feature = "analysis")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchitectureOutput {
    pub project: String,
    pub overview: ArchitectureOverview,
}

/// Runs architecture overview against an injected Kit (testable core).
///
/// Returns the full [`ArchitectureOverview`] including the enhanced fields:
/// `module_boundaries`, `dependency_directions`, `layers`, and
/// `cross_service_deps`.
#[cfg(feature = "analysis")]
pub fn run_architecture(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
) -> Result<ArchitectureOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let analyzer = ArchitectureAnalyzer::new(&*storage);
    let overview: ArchitectureOverview = analyzer.overview(project)?;
    Ok(ArchitectureOutput {
        project: project.to_string(),
        overview,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "analysis"))]
#[service_api(
    name = "architecture",
    version = "0.3.2",
    description = "Show high-level architecture overview of a project.",
    cli = true
)]
async fn architecture(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_architecture(&kit, &project)
        .map_err(|e| to_api_error(e, "architecture_error"))?;
    let json = serde_json::to_string(&output)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "architecture_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(all(feature = "mcp", feature = "analysis"))]
#[service_api(
    name = "architecture",
    version = "0.3.2",
    tool_name = "architecture",
    description = "Show high-level architecture overview of a project."
)]
async fn architecture_mcp(project: String) -> Result<ArchitectureOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_architecture(&kit, &project).map_err(|e| to_api_error(e, "architecture_error"))
}

#[cfg(all(test, feature = "cli", feature = "analysis"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_architecture_testdb");
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
    fn run_architecture_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert!(output.overview.languages.is_empty());
        assert!(output.overview.packages.is_empty());
        assert!(output.overview.entry_points.is_empty());
        assert!(output.overview.routes.is_empty());
        assert!(output.overview.hotspots.is_empty());
    }

    #[test]
    fn run_architecture_returns_languages() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage
            .execute("CREATE (:File {id: 'f1', project: 'demo', name: 'main.rs', filePath: '/src/main.rs', language: 'rust', hash: '', lineCount: 0});")
            .expect("create file");
        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert!(!output.overview.languages.is_empty());
    }

    #[test]
    fn run_architecture_with_routes() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage
            .execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});")
            .expect("create route");
        storage
            .execute("CREATE (:Handler {id: 'h1', project: 'demo', name: 'list_users', qualifiedName: 'list_users', filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create handler");
        storage
            .execute("CREATE (:CodeRelation {id: 'e1', source: 'h1', target: 'r1', type: 'HANDLES', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});")
            .expect("create edge");
        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert!(!output.overview.routes.is_empty());
    }

    #[test]
    fn output_serializes_to_json() {
        let out = ArchitectureOutput {
            project: "demo".into(),
            overview: ArchitectureOverview {
                languages: vec![],
                packages: vec![],
                entry_points: vec![],
                routes: vec![],
                hotspots: vec![],
                module_boundaries: vec![],
                dependency_directions: vec![],
                layers: vec![],
                cross_service_deps: vec![],
            },
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"overview\""));
    }

    // ===== T041: Enhanced architecture fields tests =====

    #[test]
    fn run_architecture_includes_module_boundaries() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        // Create functions in different modules
        storage.execute("CREATE (:Function {id: 'f_a1', project: 'demo', name: 'a1', qualifiedName: 'demo.a1', filePath: '/src/a/a1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a1");
        storage.execute("CREATE (:Function {id: 'f_b1', project: 'demo', name: 'b1', qualifiedName: 'demo.b1', filePath: '/src/b/b1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b1");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a1', target: 'f_b1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create cross-module edge");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert!(
            !output.overview.module_boundaries.is_empty(),
            "should detect module boundaries"
        );
        let module_names: Vec<&str> = output
            .overview
            .module_boundaries
            .iter()
            .map(|m| m.module_name.as_str())
            .collect();
        assert!(
            module_names.iter().any(|n| n.ends_with("src/a")),
            "should have module src/a, got: {module_names:?}"
        );
        assert!(
            module_names.iter().any(|n| n.ends_with("src/b")),
            "should have module src/b, got: {module_names:?}"
        );
    }

    #[test]
    fn run_architecture_includes_dependency_directions() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a1', project: 'demo', name: 'a1', qualifiedName: 'demo.a1', filePath: '/src/a/a1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a1");
        storage.execute("CREATE (:Function {id: 'f_b1', project: 'demo', name: 'b1', qualifiedName: 'demo.b1', filePath: '/src/b/b1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b1");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a1', target: 'f_b1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create a->b edge");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert!(
            !output.overview.dependency_directions.is_empty(),
            "should detect dependency directions"
        );
        let dir = &output.overview.dependency_directions[0];
        assert!(
            dir.from_module.ends_with("src/a") || dir.to_module.ends_with("src/b"),
            "should have direction from src/a to src/b"
        );
    }

    #[test]
    fn run_architecture_includes_layers() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        // Controller: function that HANDLES_ROUTE
        storage.execute("CREATE (:Function {id: 'f_ctrl', project: 'demo', name: 'list_users', qualifiedName: 'demo.list_users', filePath: '/src/api/handler.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create controller");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_ctrl', target: 'r1', type: 'HANDLES_ROUTE', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create HANDLES_ROUTE edge");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert!(
            !output.overview.layers.is_empty(),
            "should detect layers"
        );
        let controller = output
            .overview
            .layers
            .iter()
            .find(|l| l.layer == "Controller")
            .expect("should have Controller layer");
        assert!(
            controller.members.contains(&"demo.list_users".to_string()),
            "Controller layer should contain demo.list_users"
        );
    }

    #[test]
    fn run_architecture_serializes_enhanced_fields() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_architecture(&kit, "demo").expect("run should succeed");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"module_boundaries\""), "json should contain module_boundaries");
        assert!(json.contains("\"dependency_directions\""), "json should contain dependency_directions");
        assert!(json.contains("\"layers\""), "json should contain layers");
        assert!(json.contains("\"cross_service_deps\""), "json should contain cross_service_deps");
    }

    #[test]
    fn run_architecture_detects_circular_dependency() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a1', project: 'demo', name: 'a1', qualifiedName: 'demo.a1', filePath: '/src/a/a1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a1");
        storage.execute("CREATE (:Function {id: 'f_b1', project: 'demo', name: 'b1', qualifiedName: 'demo.b1', filePath: '/src/b/b1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b1");
        // A→B and B→A (circular)
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a1', target: 'f_b1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create a->b");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b1', target: 'f_a1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create b->a");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        let has_circular = output
            .overview
            .dependency_directions
            .iter()
            .any(|d| d.is_circular);
        assert!(has_circular, "should detect circular dependency");
    }

    #[test]
    fn run_architecture_module_boundary_cohesion() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        // Module A with internal calls only → cohesion = 1.0
        storage.execute("CREATE (:Function {id: 'f_a1', project: 'demo', name: 'a1', qualifiedName: 'demo.a1', filePath: '/src/a/a1.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a1");
        storage.execute("CREATE (:Function {id: 'f_a2', project: 'demo', name: 'a2', qualifiedName: 'demo.a2', filePath: '/src/a/a2.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a2");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a1', target: 'f_a2', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create internal edge");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        let module_a = output
            .overview
            .module_boundaries
            .iter()
            .find(|m| m.module_name.ends_with("src/a"))
            .expect("should find module src/a");
        assert_eq!(module_a.incoming_deps, 0);
        assert_eq!(module_a.outgoing_deps, 0);
        assert!((module_a.cohesion - 1.0).abs() < 0.001, "cohesion should be 1.0 for isolated module");
    }

    // ===== run_architecture: cross_service_deps via Route + fetch content =====

    #[test]
    fn run_architecture_includes_cross_service_deps() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/a/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("create caller");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert!(
            !output.overview.cross_service_deps.is_empty(),
            "should detect cross-service deps via Route + fetch content"
        );
    }

    #[test]
    fn run_architecture_with_packages() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage.execute("CREATE (:File {id: 'f1', project: 'demo', name: 'main.rs', filePath: '/src/main.rs', language: 'rust', hash: '', lineCount: 100});").expect("create file");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/main.rs', startLine: 1, endLine: 50, signature: '', returnType: '', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create function");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert_eq!(output.project, "demo");
    }

    #[test]
    fn run_architecture_with_entry_points() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_main', project: 'demo', name: 'main', qualifiedName: 'demo.main', filePath: '/src/main.rs', startLine: 1, endLine: 50, signature: 'fn main()', returnType: '', isExported: true, docstring: '', content: '', parentQn: ''});").expect("create main");
        storage.execute("CREATE (:CodeRelation {id: 'e_ep', source: 'f_main', target: 'f_main', type: 'ENTRY_POINT', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});").expect("create entry point edge");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert_eq!(output.project, "demo");
    }

    #[test]
    fn run_architecture_unknown_project_returns_empty_overview() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage.execute("CREATE (:File {id: 'f1', project: 'other', name: 'main.rs', filePath: '/src/main.rs', language: 'rust', hash: '', lineCount: 0});").expect("create file in other project");

        let output = run_architecture(&kit, "demo").expect("run should succeed");
        assert!(output.overview.languages.is_empty(), "no languages for absent project");
        assert!(output.overview.packages.is_empty(), "no packages for absent project");
    }
}
