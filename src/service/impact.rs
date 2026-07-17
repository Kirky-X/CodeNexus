// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Impact command: analyze the blast radius of changing a symbol.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, TraceModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::model::EdgeType;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::trace::find_start_node_id;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::trace::{ImpactAnalyzer, ImpactConfig, ImpactNode, RiskAssessment, MAX_SUBGRAPH_NODES};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// JSON-serializable impact analysis result.
#[cfg(any(feature = "cli", feature = "mcp", test))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactOutput {
    pub symbol: String,
    pub depth: u32,
    pub node_count: usize,
    pub edge_count: usize,
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_assessment: Option<RiskAssessment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub affected: Vec<ImpactNode>,
    /// True when `load_graph` hit the `MAX_SUBGRAPH_NODES` cap and returned a
    /// truncated subgraph. Always serialized (no skip) so incompleteness is
    /// explicit — rule 12: never hide a degraded result behind a default.
    pub truncated: bool,
}

#[cfg(any(feature = "cli", feature = "mcp", test))]
fn impact_output(
    symbol: String,
    depth: u32,
    graph: crate::model::Graph,
    truncated: bool,
) -> ImpactOutput {
    let nodes: Vec<Value> = graph
        .nodes
        .values()
        .map(|n| serde_json::to_value(n).unwrap_or(Value::Null))
        .collect();
    let edges: Vec<Value> = graph
        .edges
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
        .collect();
    ImpactOutput {
        node_count: nodes.len(),
        edge_count: edges.len(),
        nodes,
        edges,
        symbol,
        depth,
        risk_assessment: None,
        affected: vec![],
        truncated,
    }
}

/// Builds an [`ImpactConfig`] from CLI parameters.
///
/// `edge_types` is a comma-separated list of UPPERCASE DDL edge type strings
/// (e.g. `"CALLS,IMPLEMENTS,USES_TYPE"`). An empty string means "use the
/// default edge types" from [`ImpactConfig::default`].
///
/// `max_depth` of `0` means "use the default" (5). Non-zero values are used
/// as-is (clamped internally by `trace_upstream` to the spec maximum of 10).
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn build_impact_config(edge_types: &str, max_depth: u32, include_tests: bool) -> ImpactConfig {
    let default = ImpactConfig::default();
    let final_edge_types = if edge_types.trim().is_empty() {
        default.edge_types.clone()
    } else {
        let parsed: Vec<EdgeType> = edge_types
            .split(',')
            .filter_map(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    trimmed.parse::<EdgeType>().ok()
                }
            })
            .collect();
        if parsed.is_empty() {
            default.edge_types.clone()
        } else {
            parsed
        }
    };
    let final_max_depth = if max_depth == 0 {
        default.max_depth
    } else {
        max_depth
    };
    ImpactConfig {
        max_depth: final_max_depth,
        edge_types: final_edge_types,
        include_tests,
    }
}

/// Runs impact analysis against an injected Kit (testable core).
///
/// When `edge_types` is non-empty, `max_depth > 0`, or `include_tests` is
/// `true`, uses [`ImpactAnalyzer::with_config`] for multi-dimensional impact
/// analysis with risk assessment. Otherwise, falls back to the legacy
/// graph-loading path (backward compatible).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_impact(
    kit: &AsyncKit<AsyncReady>,
    symbol: &str,
    depth: u32,
    edge_types: &str,
    max_depth: u32,
    include_tests: bool,
) -> Result<ImpactOutput, CodeNexusError> {
    let trace_engine = kit.require::<TraceModule>()?;
    let enhanced = !edge_types.trim().is_empty() || max_depth > 0 || include_tests;

    if enhanced {
        let config = build_impact_config(edge_types, max_depth, include_tests);
        let load_depth = depth.max(config.max_depth) as usize;
        let (graph, truncated) = trace_engine.load_graph(symbol, load_depth, MAX_SUBGRAPH_NODES)?;

        let effective_depth = config.max_depth;
        // Verify symbol exists to avoid silent empty-result success (rule 12).
        let start_id = find_start_node_id(&graph, symbol)
            .ok_or_else(|| CodeNexusError::NotFound(format!("symbol not found: {symbol}")))?;
        let analyzer = ImpactAnalyzer::with_config(&graph, config);
        let result = analyzer.analyze_impact(&start_id);

        let mut output = impact_output(symbol.to_string(), effective_depth, graph, truncated);
        output.affected = result.affected;
        output.risk_assessment = Some(result.risk_assessment);
        Ok(output)
    } else {
        let (graph, truncated) =
            trace_engine.load_graph(symbol, depth as usize, MAX_SUBGRAPH_NODES)?;
        // Verify symbol exists to avoid silent empty-result success (rule 12).
        if find_start_node_id(&graph, symbol).is_none() {
            return Err(CodeNexusError::NotFound(format!(
                "symbol not found: {symbol}"
            )));
        }
        Ok(impact_output(symbol.to_string(), depth, graph, truncated))
    }
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "impact",
    version = "0.3.5",
    description = "Analyze the blast radius (upstream callers) of changing a symbol.",
    cli = true
)]
async fn impact(
    symbol: String,
    depth: u32,
    edge_types: String,
    max_depth: u32,
    include_tests: bool,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result = run_impact(&kit, &symbol, depth, &edge_types, max_depth, include_tests)
        .map_err(|e| to_api_error(e, "impact_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "impact_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[forge(
    name = "impact",
    version = "0.3.5",
    tool_name = "impact",
    description = "Analyze the blast radius (upstream callers) of changing a symbol."
)]
async fn impact_mcp(
    symbol: String,
    depth: u32,
    edge_types: String,
    max_depth: u32,
    include_tests: bool,
) -> Result<ImpactOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_impact(&kit, &symbol, depth, &edge_types, max_depth, include_tests)
        .map_err(|e| to_api_error(e, "impact_error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_impact_testdb");
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
    fn run_impact_returns_not_found_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_impact(&kit, "demo.foo", 3, "", 0, false)
            .expect_err("unknown symbol on empty DB should error");
        assert!(
            matches!(err, CodeNexusError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("demo.foo"),
            "error should mention symbol: {msg}"
        );
    }

    #[test]
    fn impact_output_serializes_to_json() {
        let output = ImpactOutput {
            symbol: "demo.foo".into(),
            depth: 3,
            node_count: 0,
            edge_count: 0,
            nodes: vec![],
            edges: vec![],
            risk_assessment: None,
            affected: vec![],
            truncated: false,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"symbol\":\"demo.foo\""));
        assert!(json.contains("\"depth\":3"));
        assert!(json.contains("\"node_count\":0"));
        assert!(json.contains("\"edge_count\":0"));
        assert!(!json.contains("risk_assessment"));
        assert!(!json.contains("affected"));
        // truncated is always serialized (no skip), even when false.
        assert!(json.contains("\"truncated\":false"));
    }

    #[test]
    fn run_impact_returns_non_empty_graph_for_known_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'root', qualifiedName: 'demo.root', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create root");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'leaf', qualifiedName: 'demo.leaf', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create leaf");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: 'direct call', startLine: 2, project: 'demo'});").expect("create edge");

        let output = run_impact(&kit, "demo.root", 3, "", 0, false).expect("impact should succeed");
        assert_eq!(output.symbol, "demo.root");
        assert_eq!(output.depth, 3);
        assert!(output.node_count >= 1, "should have at least 1 node");
        assert!(output.edge_count >= 1, "should have at least 1 edge");
        assert_eq!(output.nodes.len(), output.node_count);
        assert_eq!(output.edges.len(), output.edge_count);
    }

    #[test]
    fn run_impact_at_depth_zero_returns_only_start_node() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_solo', project: 'demo', name: 'solo', qualifiedName: 'demo.solo', filePath: '/src/s.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create solo");
        storage.execute("CREATE (:Function {id: 'f_other', project: 'demo', name: 'other', qualifiedName: 'demo.other', filePath: '/src/o.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create other");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_solo', target: 'f_other', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create edge");

        let output =
            run_impact(&kit, "demo.solo", 0, "", 0, false).expect("impact depth 0 should succeed");
        assert_eq!(output.symbol, "demo.solo");
        assert_eq!(output.depth, 0);
        assert_eq!(output.node_count, 1, "only start node at depth 0");
        assert_eq!(output.edge_count, 0, "no edges at depth 0");
    }

    // ===== T040: build_impact_config unit tests =====

    #[test]
    fn build_impact_config_defaults_on_empty_params() {
        let config = build_impact_config("", 0, false);
        let default = ImpactConfig::default();
        assert_eq!(config.max_depth, default.max_depth);
        assert_eq!(config.edge_types, default.edge_types);
        assert_eq!(config.include_tests, default.include_tests);
    }

    #[test]
    fn build_impact_config_parses_edge_types() {
        let config = build_impact_config("CALLS,IMPLEMENTS", 0, false);
        assert_eq!(
            config.edge_types,
            vec![EdgeType::Calls, EdgeType::Implements]
        );
    }

    #[test]
    fn build_impact_config_parses_single_edge_type() {
        let config = build_impact_config("HTTP_CALLS", 0, false);
        assert_eq!(config.edge_types, vec![EdgeType::HttpCalls]);
    }

    #[test]
    fn build_impact_config_uses_default_on_invalid_edge_types() {
        let config = build_impact_config("INVALID,BOGUS", 0, false);
        assert_eq!(config.edge_types, ImpactConfig::default().edge_types);
    }

    #[test]
    fn build_impact_config_sets_max_depth() {
        let config = build_impact_config("", 7, false);
        assert_eq!(config.max_depth, 7);
    }

    #[test]
    fn build_impact_config_sets_include_tests() {
        let config = build_impact_config("", 0, true);
        assert!(config.include_tests);
    }

    #[test]
    fn build_impact_config_trims_edge_type_whitespace() {
        let config = build_impact_config(" CALLS , IMPLEMENTS ", 0, false);
        assert_eq!(
            config.edge_types,
            vec![EdgeType::Calls, EdgeType::Implements]
        );
    }

    // ===== T040: find_start_node_id unit tests =====

    #[test]
    fn find_start_node_id_matches_qualified_name() {
        let mut graph = crate::model::Graph::new();
        let node =
            crate::model::Node::builder(crate::model::NodeLabel::Function, "root", "demo.root")
                .id("f_a")
                .project("demo")
                .build();
        graph.add_node(node);
        let id = find_start_node_id(&graph, "demo.root");
        assert_eq!(id, Some("f_a".to_string()));
    }

    #[test]
    fn find_start_node_id_matches_name() {
        let mut graph = crate::model::Graph::new();
        let node =
            crate::model::Node::builder(crate::model::NodeLabel::Function, "root", "demo.root")
                .id("f_a")
                .project("demo")
                .build();
        graph.add_node(node);
        let id = find_start_node_id(&graph, "root");
        assert_eq!(id, Some("f_a".to_string()));
    }

    #[test]
    fn find_start_node_id_returns_none_for_missing() {
        let graph = crate::model::Graph::new();
        let id = find_start_node_id(&graph, "nonexistent");
        assert_eq!(id, None);
    }

    // ===== T040: run_impact enhanced mode tests =====

    #[test]
    fn run_impact_enhanced_returns_risk_assessment() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // Create a caller chain: caller -> target
        storage.execute("CREATE (:Function {id: 'f_target', project: 'demo', name: 'target', qualifiedName: 'demo.target', filePath: '/src/t.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create target");
        storage.execute("CREATE (:Function {id: 'f_caller', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_caller', target: 'f_target', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: 'direct call', startLine: 2, project: 'demo'});").expect("create edge");

        let output = run_impact(&kit, "demo.target", 3, "CALLS", 5, false)
            .expect("enhanced impact should succeed");
        assert_eq!(output.symbol, "demo.target");
        assert!(
            output.risk_assessment.is_some(),
            "should have risk assessment"
        );
        assert!(!output.affected.is_empty(), "should have affected nodes");
        let caller = output
            .affected
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller in affected");
        assert_eq!(caller.edge_type, EdgeType::Calls);
        assert_eq!(caller.depth, 1);
    }

    #[test]
    fn run_impact_enhanced_with_max_depth_limits_traversal() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // Create chain: a -> b -> c -> target
        storage.execute("CREATE (:Function {id: 'f_target', project: 'demo', name: 'target', qualifiedName: 'demo.target', filePath: '/src/t.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create target");
        storage.execute("CREATE (:Function {id: 'f_c', project: 'demo', name: 'c', qualifiedName: 'demo.c', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create c");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_c', target: 'f_target', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create c->target");
        storage.execute("CREATE (:CodeRelation {id: 'e2', source: 'f_b', target: 'f_c', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create b->c");
        storage.execute("CREATE (:CodeRelation {id: 'e3', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create a->b");

        // max_depth=1: only c (direct caller) should be affected
        let output = run_impact(&kit, "demo.target", 3, "CALLS", 1, false)
            .expect("enhanced impact with max_depth=1 should succeed");
        assert_eq!(
            output.affected.len(),
            1,
            "only direct caller at max_depth=1"
        );
        assert_eq!(output.affected[0].name, "c");
    }

    #[test]
    fn run_impact_enhanced_with_custom_edge_types() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // target is used by struct via USES_TYPE
        storage.execute("CREATE (:Struct {id: 's_target', project: 'demo', name: 'Target', qualifiedName: 'demo.Target', filePath: '/src/t.rs', startLine: 1, endLine: 5, isExported: false, docstring: '', content: '', parentQn: ''});").expect("create target struct");
        storage.execute("CREATE (:Function {id: 'f_user', project: 'demo', name: 'user', qualifiedName: 'demo.user', filePath: '/src/u.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create user");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_user', target: 's_target', type: 'USES_TYPE', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create USES_TYPE edge");

        // Use only USES_TYPE edge type
        let output = run_impact(&kit, "demo.Target", 3, "USES_TYPE", 5, false)
            .expect("enhanced impact with USES_TYPE should succeed");
        assert!(
            !output.affected.is_empty(),
            "should find user via USES_TYPE"
        );
        let user = output
            .affected
            .iter()
            .find(|n| n.name == "user")
            .expect("should find user");
        assert_eq!(user.edge_type, EdgeType::UsesType);
    }

    #[test]
    fn run_impact_enhanced_returns_not_found_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_impact(&kit, "demo.missing", 3, "CALLS", 5, false)
            .expect_err("enhanced impact on missing symbol should error");
        assert!(
            matches!(err, CodeNexusError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("demo.missing"),
            "error should mention symbol: {msg}"
        );
    }

    #[test]
    fn run_impact_enhanced_serializes_with_risk_assessment() {
        let output = ImpactOutput {
            symbol: "demo.target".into(),
            depth: 5,
            node_count: 2,
            edge_count: 1,
            nodes: vec![],
            edges: vec![],
            risk_assessment: Some(RiskAssessment {
                level: crate::trace::RiskLevel::High,
                score: 0.7,
                factors: vec![],
            }),
            affected: vec![ImpactNode {
                name: "caller".into(),
                qualified_name: "demo.caller".into(),
                file_path: "/src/c.rs".into(),
                impact_path: vec!["target".into(), "caller".into()],
                edge_type: EdgeType::Calls,
                depth: 1,
            }],
            truncated: true,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"risk_assessment\""));
        assert!(json.contains("\"affected\""));
        assert!(json.contains("\"level\":\"High\""));
        assert!(json.contains("\"edge_type\":\"Calls\""));
    }

    #[test]
    fn run_impact_legacy_mode_no_risk_assessment() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'root', qualifiedName: 'demo.root', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create root");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'leaf', qualifiedName: 'demo.leaf', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create leaf");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create edge");

        let output =
            run_impact(&kit, "demo.root", 3, "", 0, false).expect("legacy impact should succeed");
        assert!(
            output.risk_assessment.is_none(),
            "legacy mode has no risk assessment"
        );
        assert!(
            output.affected.is_empty(),
            "legacy mode has no affected list"
        );
    }

    // ===== build_impact_config: empty segment filtering =====

    #[test]
    fn build_impact_config_filters_empty_segments_in_edge_types() {
        let config = build_impact_config(",CALLS,,IMPLEMENTS,", 0, false);
        assert_eq!(
            config.edge_types,
            vec![EdgeType::Calls, EdgeType::Implements]
        );
    }

    #[test]
    fn build_impact_config_all_empty_segments_falls_back_to_default() {
        let config = build_impact_config(",,,", 0, false);
        assert_eq!(config.edge_types, ImpactConfig::default().edge_types);
    }

    #[test]
    fn build_impact_config_mixed_valid_invalid_edge_types() {
        let config = build_impact_config("CALLS,BOGUS,IMPLEMENTS", 0, false);
        assert_eq!(
            config.edge_types,
            vec![EdgeType::Calls, EdgeType::Implements]
        );
    }

    // ===== run_impact: include_tests parameter =====

    #[test]
    fn run_impact_with_include_tests_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_target', project: 'demo', name: 'target', qualifiedName: 'demo.target', filePath: '/src/t.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create target");
        storage.execute("CREATE (:Function {id: 'f_caller', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create caller");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_caller', target: 'f_target', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create edge");

        let output = run_impact(&kit, "demo.target", 3, "CALLS", 5, true)
            .expect("impact with include_tests should succeed");
        assert_eq!(output.symbol, "demo.target");
        assert!(
            output.risk_assessment.is_some(),
            "enhanced mode should have risk assessment"
        );
    }

    #[test]
    fn run_impact_with_max_depth_only_uses_enhanced_mode() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");

        // max_depth > 0 triggers enhanced mode even without edge_types
        let output = run_impact(&kit, "demo.a", 3, "", 5, false)
            .expect("max_depth > 0 should trigger enhanced mode");
        assert!(
            output.risk_assessment.is_some(),
            "enhanced mode should have risk assessment"
        );
    }

    #[test]
    fn run_impact_enhanced_returns_not_found_for_missing_start_node() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        // Create a node but query for a symbol that doesn't match
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_a', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").expect("create self-edge");

        // Query for a non-existent symbol in enhanced mode
        let err = run_impact(&kit, "nonexistent.symbol", 3, "CALLS", 5, false)
            .expect_err("enhanced impact on missing symbol should error");
        assert!(
            matches!(err, CodeNexusError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("nonexistent.symbol"),
            "error should mention symbol: {msg}"
        );
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn impact_wrapper_returns_error_for_unknown_symbol_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(impact("demo.foo".to_string(), 3, "".to_string(), 0, false));
        assert!(
            result.is_err(),
            "wrapper should fail for unknown symbol: {:?}",
            result
        );

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[test]
    #[cfg(feature = "cli")]
    fn impact_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(impact("demo.foo".to_string(), 3, "".to_string(), 0, false));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
