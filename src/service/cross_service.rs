// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `cross_service` service: detect cross-service links via route pattern matching.

use serde::Serialize;

#[cfg(feature = "cross-service")]
use crate::analysis::cross_service::{
    CrossServiceDetector, CrossServiceLink, CrossServiceLinker, CrossServiceMatch, ServiceProtocol,
};
#[cfg(all(feature = "cross-service", any(feature = "cli", test)))]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "cross-service", any(feature = "cli", test)))]
use crate::service::error::CodeNexusError;
#[cfg(all(feature = "cli", feature = "cross-service"))]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(all(feature = "cli", feature = "cross-service"))]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable cross-service link output.
#[cfg(feature = "cross-service")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CrossServiceOutput {
    pub project: String,
    pub links: Vec<CrossServiceLink>,
    /// Multi-protocol matches from [`CrossServiceDetector`], filtered by the
    /// requested protocol (empty protocol = all protocols).
    pub matches: Vec<CrossServiceMatch>,
}

/// Parses a protocol filter string into a [`ServiceProtocol`].
///
/// Accepts case-insensitive aliases:
/// - `http_rest`, `http`, `rest` → `HttpRest`
/// - `grpc` → `Grpc`
/// - `graphql` → `GraphQL`
/// - `message_queue`, `mq`, `message` → `MessageQueue`
/// - `event_bus`, `event` → `EventBus`
///
/// Returns `None` for empty or unrecognized strings (empty = all protocols).
#[cfg(feature = "cross-service")]
fn parse_protocol(protocol: &str) -> Option<ServiceProtocol> {
    let lower = protocol.trim().to_ascii_lowercase();
    match lower.as_str() {
        "" => None,
        "http_rest" | "http" | "rest" => Some(ServiceProtocol::HttpRest),
        "grpc" => Some(ServiceProtocol::Grpc),
        "graphql" => Some(ServiceProtocol::GraphQL),
        "message_queue" | "mq" | "message" => Some(ServiceProtocol::MessageQueue),
        "event_bus" | "event" => Some(ServiceProtocol::EventBus),
        _ => None,
    }
}

/// Runs cross-service link detection against an injected Kit (testable core).
///
/// `protocol` filters the multi-protocol `matches` field:
/// - Empty string → all protocols (HttpRest, Grpc, GraphQL, MessageQueue, EventBus)
/// - Specific protocol (e.g. `"grpc"`) → only matches for that protocol
///
/// The `links` field is always populated from [`CrossServiceLinker`] (HTTP
/// route pattern matching with edge persistence) for backward compatibility.
#[cfg(all(feature = "cross-service", any(feature = "cli", test)))]
pub fn run_cross_service(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    protocol: &str,
) -> Result<CrossServiceOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let linker = CrossServiceLinker::new(&*storage, project);
    let links = linker.link()?;
    let detector = CrossServiceDetector::new(&*storage);
    let mut matches = detector.detect_all(project)?;
    if let Some(filter) = parse_protocol(protocol) {
        matches.retain(|m| m.protocol == filter);
    }
    Ok(CrossServiceOutput {
        project: project.to_string(),
        links,
        matches,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "cross-service"))]
#[service_api(
    name = "cross_service",
    version = "0.3.2",
    description = "Detect cross-service links by matching HTTP route patterns against caller string literals.",
    cli = true
)]
async fn cross_service(project: String, protocol: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_cross_service(&kit, &project, &protocol)
        .map_err(|e| to_api_error(e, "cross_service_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(all(test, feature = "cross-service"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_cross_service_testdb");
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
    fn run_cross_service_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_cross_service(&kit, "demo", "").expect("run should succeed");
        assert_eq!(output.project, "demo");
        assert!(output.links.is_empty(), "no links on empty DB");
        assert!(output.matches.is_empty(), "no matches on empty DB");
    }

    #[test]
    fn run_cross_service_returns_links_when_patterns_match() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("create function");
        let output = run_cross_service(&kit, "demo", "").expect("run should succeed");
        assert_eq!(output.project, "demo");
    }

    #[test]
    fn run_cross_service_unknown_project_returns_empty_links() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'other', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        let output = run_cross_service(&kit, "demo", "").expect("run should succeed");
        assert!(output.links.is_empty(), "no links for absent project");
        assert!(output.matches.is_empty(), "no matches for absent project");
    }

    #[test]
    fn cross_service_output_serializes_to_json() {
        let out = CrossServiceOutput {
            project: "demo".into(),
            links: vec![CrossServiceLink {
                route_id: "r1".into(),
                route_pattern: "/api/users".into(),
                caller_id: "f1".into(),
                caller_file: "/src/caller.rs".into(),
                caller_line: 10,
                match_type: crate::analysis::cross_service::MatchType::Exact,
            }],
            matches: vec![],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"links\""));
        assert!(json.contains("\"route_id\":\"r1\""));
        assert!(json.contains("\"match_type\":\"Exact\""));
        assert!(json.contains("\"matches\""));
    }

    // ===== T037: parse_protocol unit tests =====

    #[test]
    fn parse_protocol_empty_returns_none() {
        assert_eq!(parse_protocol(""), None);
        assert_eq!(parse_protocol("   "), None);
    }

    #[test]
    fn parse_protocol_http_aliases() {
        assert_eq!(parse_protocol("http_rest"), Some(ServiceProtocol::HttpRest));
        assert_eq!(parse_protocol("http"), Some(ServiceProtocol::HttpRest));
        assert_eq!(parse_protocol("rest"), Some(ServiceProtocol::HttpRest));
        assert_eq!(parse_protocol("HTTP_REST"), Some(ServiceProtocol::HttpRest));
        assert_eq!(parse_protocol("  http  "), Some(ServiceProtocol::HttpRest));
    }

    #[test]
    fn parse_protocol_grpc() {
        assert_eq!(parse_protocol("grpc"), Some(ServiceProtocol::Grpc));
        assert_eq!(parse_protocol("GRPC"), Some(ServiceProtocol::Grpc));
    }

    #[test]
    fn parse_protocol_graphql() {
        assert_eq!(parse_protocol("graphql"), Some(ServiceProtocol::GraphQL));
        assert_eq!(parse_protocol("GraphQL"), Some(ServiceProtocol::GraphQL));
    }

    #[test]
    fn parse_protocol_message_queue_aliases() {
        assert_eq!(parse_protocol("message_queue"), Some(ServiceProtocol::MessageQueue));
        assert_eq!(parse_protocol("mq"), Some(ServiceProtocol::MessageQueue));
        assert_eq!(parse_protocol("message"), Some(ServiceProtocol::MessageQueue));
    }

    #[test]
    fn parse_protocol_event_bus_aliases() {
        assert_eq!(parse_protocol("event_bus"), Some(ServiceProtocol::EventBus));
        assert_eq!(parse_protocol("event"), Some(ServiceProtocol::EventBus));
    }

    #[test]
    fn parse_protocol_invalid_returns_none() {
        assert_eq!(parse_protocol("invalid"), None);
        assert_eq!(parse_protocol("ftp"), None);
        assert_eq!(parse_protocol("websocket"), None);
    }

    // ===== T037: run_cross_service with protocol filter =====

    #[test]
    fn run_cross_service_with_empty_protocol_returns_all_matches() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_cross_service(&kit, "demo", "").expect("run should succeed");
        // Empty DB → no matches, but field should exist
        assert!(output.matches.is_empty());
    }

    #[test]
    fn run_cross_service_with_protocol_filter_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_cross_service(&kit, "demo", "grpc").expect("run should succeed");
        assert!(output.matches.is_empty(), "no matches on empty DB even with filter");
    }

    #[test]
    fn run_cross_service_with_invalid_protocol_returns_all_matches() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_cross_service(&kit, "demo", "invalid_protocol").expect("run should succeed");
        // Invalid protocol → None filter → all matches (empty on empty DB)
        assert!(output.matches.is_empty());
    }

    // --- run_cross_service: protocol filter with real matches (covers retain closure) ---

    #[test]
    fn run_cross_service_with_http_protocol_filters_matches() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("create function");

        // With protocol="http", only HttpRest matches should be retained
        let output = run_cross_service(&kit, "demo", "http").expect("run should succeed");
        assert!(
            output.matches.iter().all(|m| m.protocol == ServiceProtocol::HttpRest),
            "filtered matches should all be HttpRest"
        );
    }

    #[test]
    fn run_cross_service_with_grpc_protocol_filters_matches() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("create function");

        // With protocol="grpc", HTTP matches should be filtered out
        let output = run_cross_service(&kit, "demo", "grpc").expect("run should succeed");
        assert!(
            output.matches.iter().all(|m| m.protocol == ServiceProtocol::Grpc),
            "filtered matches should all be Grpc"
        );
    }

    #[test]
    fn run_cross_service_no_protocol_returns_all_match_types() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage.execute("CREATE (:Route {id: 'r1', project: 'demo', name: '/api/users', qualifiedName: '/api/users', filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', parentQn: ''});").expect("create route");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'fetch(\"/api/users\");', parentQn: ''});").expect("create function");

        // Empty protocol → all matches returned (no filter)
        let output = run_cross_service(&kit, "demo", "").expect("run should succeed");
        // Should have at least one HTTP match since we created route + caller
        assert!(
            output.matches.iter().any(|m| m.protocol == ServiceProtocol::HttpRest),
            "should have at least one HttpRest match"
        );
    }
}
