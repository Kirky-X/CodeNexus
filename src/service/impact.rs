// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Impact command: analyze the blast radius of changing a symbol.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, TraceModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable impact analysis result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactOutput {
    pub symbol: String,
    pub depth: u32,
    pub node_count: usize,
    pub edge_count: usize,
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
}

#[cfg(any(feature = "cli", feature = "mcp", test))]
fn impact_output(symbol: String, depth: u32, graph: crate::model::Graph) -> ImpactOutput {
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
    }
}

/// Runs impact analysis against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_impact(
    kit: &AsyncKit<AsyncReady>,
    symbol: &str,
    depth: u32,
) -> Result<ImpactOutput, CodeNexusError> {
    let trace_engine = kit.require::<TraceModule>()?;
    let graph = trace_engine.load_graph(symbol, depth as usize)?;
    Ok(impact_output(symbol.to_string(), depth, graph))
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "impact",
    version = "0.3.2",
    description = "Analyze the blast radius (upstream callers) of changing a symbol.",
    cli = true
)]
async fn impact(symbol: String, depth: u32) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result = run_impact(&kit, &symbol, depth).map_err(|e| to_api_error(e, "impact_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "impact_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "impact",
    version = "0.3.2",
    tool_name = "impact",
    description = "Analyze the blast radius (upstream callers) of changing a symbol."
)]
async fn impact_mcp(symbol: String, depth: u32) -> Result<ImpactOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_impact(&kit, &symbol, depth).map_err(|e| to_api_error(e, "impact_error"))
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
    fn run_impact_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_impact(&kit, "demo.foo", 3).expect("run should succeed");
        assert_eq!(output.symbol, "demo.foo");
        assert_eq!(output.depth, 3);
        assert_eq!(output.node_count, 0);
        assert_eq!(output.edge_count, 0);
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
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"symbol\":\"demo.foo\""));
        assert!(json.contains("\"depth\":3"));
        assert!(json.contains("\"node_count\":0"));
        assert!(json.contains("\"edge_count\":0"));
    }

    #[test]
    fn run_impact_returns_non_empty_graph_for_known_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<crate::kit::StorageModule>().expect("storage");
        storage.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'root', qualifiedName: 'demo.root', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create root");
        storage.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'leaf', qualifiedName: 'demo.leaf', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create leaf");
        storage.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: 'direct call', startLine: 2, project: 'demo'});").expect("create edge");

        let output = run_impact(&kit, "demo.root", 3).expect("impact should succeed");
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

        let output = run_impact(&kit, "demo.solo", 0).expect("impact depth 0 should succeed");
        assert_eq!(output.symbol, "demo.solo");
        assert_eq!(output.depth, 0);
        // At depth 0, only the start node is loaded (no BFS expansion).
        assert_eq!(output.node_count, 1, "only start node at depth 0");
        assert_eq!(output.edge_count, 0, "no edges at depth 0");
    }
}
