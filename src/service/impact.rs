// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Impact command: analyze the blast radius of changing a symbol.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kit::TraceKey;
use crate::service::error::{kit_not_initialized, wrap_error};
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

#[cfg(any(feature = "cli", feature = "mcp"))]
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

/// Core impact logic — shared by CLI and MCP wrappers.
#[cfg(any(feature = "cli", feature = "mcp"))]
async fn impact_core(symbol: String, depth: u32) -> Result<ImpactOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let trace_engine = kit
        .require::<TraceKey>()
        .map_err(|e| wrap_error("Failed to resolve trace capability", e))?;
    let graph = trace_engine
        .load_graph(&symbol, depth as usize)
        .map_err(|e| wrap_error("Impact graph load failed", e))?;
    Ok(impact_output(symbol, depth, graph))
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
    let result = impact_core(symbol, depth).await?;
    let json =
        serde_json::to_string(&result).map_err(|e| wrap_error("JSON serialization failed", e))?;
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
    impact_core(symbol, depth).await
}
