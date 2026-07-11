// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Trace command: trace a symbol's call and/or data-flow paths.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::kit::TraceModule;
use crate::service::error::{CliError, to_api_error};
use crate::service::runtime::kit;
use crate::trace::{TraceEdge, TraceNode, TraceResult, TraceType};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable trace result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceOutput {
    pub symbol: String,
    pub paths: Vec<TracePathOutput>,
}

/// A single trace path — a sequence of nodes and edges at a given depth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TracePathOutput {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    pub depth: usize,
}

#[cfg(any(feature = "cli", feature = "mcp"))]
fn trace_output(r: TraceResult) -> TraceOutput {
    let paths = r
        .paths
        .into_iter()
        .map(|p| TracePathOutput {
            nodes: p.nodes.iter().map(trace_node_to_json).collect(),
            edges: p.edges.iter().map(trace_edge_to_json).collect(),
            depth: p.depth,
        })
        .collect();
    TraceOutput {
        symbol: r.symbol,
        paths,
    }
}

#[cfg(any(feature = "cli", feature = "mcp"))]
fn trace_node_to_json(n: &TraceNode) -> Value {
    json!({
        "name": n.name,
        "label": n.label,
        "filePath": n.file_path,
        "startLine": n.start_line,
    })
}

#[cfg(any(feature = "cli", feature = "mcp"))]
fn trace_edge_to_json(e: &TraceEdge) -> Value {
    json!({
        "edgeType": e.edge_type,
        "reason": e.reason,
        "confidence": e.confidence,
    })
}

/// Core trace logic — shared by CLI and MCP wrappers.
#[cfg(any(feature = "cli", feature = "mcp"))]
async fn trace_core(
    symbol: String,
    trace_type: String,
    depth: u32,
) -> Result<TraceOutput, CliError> {
    let kit = kit().ok_or_else(CliError::kit_not_initialized)?;
    let tt = TraceType::from_cli_str(&trace_type).ok_or_else(|| {
        CliError::InvalidInput(format!(
            "invalid trace_type: {trace_type} (expected calls|dataflow|all)"
        ))
    })?;
    let trace_engine = kit.require::<TraceModule>()?;
    let result = trace_engine.trace(&symbol, tt, depth as usize)?;
    Ok(trace_output(result))
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "trace",
    version = "0.3.2",
    description = "Trace a symbol's call and/or data-flow paths.",
    cli = true
)]
async fn trace(symbol: String, trace_type: String, depth: u32) -> Result<(), ApiError> {
    let result = trace_core(symbol, trace_type, depth)
        .await
        .map_err(|e| to_api_error(e, "trace_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CliError::from(e), "trace_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "trace",
    version = "0.3.2",
    tool_name = "trace",
    description = "Trace a symbol's call and/or data-flow paths."
)]
async fn trace_mcp(
    symbol: String,
    trace_type: String,
    depth: u32,
) -> Result<TraceOutput, ApiError> {
    trace_core(symbol, trace_type, depth)
        .await
        .map_err(|e| to_api_error(e, "trace_error"))
}
