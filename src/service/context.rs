// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Context command: show a 360-degree view of a symbol.

use serde_json::Value;

use crate::kit::TraceKey;
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::runtime::kit;
use crate::trace::context::{
    collect_incoming, collect_outgoing, collect_processes, resolve_start_id,
};
use crate::trace::types::{ContextOutput, SymbolNodeOutput};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// Core context logic — shared by CLI and MCP wrappers.
#[cfg(any(feature = "cli", feature = "mcp"))]
async fn context_core(symbol: String, depth: u32) -> Result<ContextOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let trace_engine = kit
        .require::<TraceKey>()
        .map_err(|e| wrap_error("Failed to resolve trace capability", e))?;
    let graph = trace_engine
        .load_graph(&symbol, depth as usize)
        .map_err(|e| wrap_error("Context graph load failed", e))?;
    let start_id = resolve_start_id(&graph, &symbol).ok_or_else(|| ApiError::InvalidInput {
        message: format!("symbol not found: {symbol}"),
        field: Some("symbol".to_string()),
        value: Some(Value::String(symbol.clone())),
    })?;
    let symbol_node = graph.get_node(&start_id).ok_or_else(|| {
        ApiError::internal_error(
            format!("symbol node resolved but not in graph: {symbol}"),
            "context_node_missing",
        )
    })?;
    let incoming = collect_incoming(&graph, &start_id);
    let outgoing = collect_outgoing(&graph, &start_id);
    let processes = collect_processes(&graph, &start_id);
    Ok(ContextOutput {
        symbol,
        node: SymbolNodeOutput::from(symbol_node),
        incoming,
        outgoing,
        processes,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "context",
    version = "0.3.2",
    description = "Show a 360-degree view of a symbol (callers, callees, processes).",
    cli = true
)]
async fn context(symbol: String, depth: u32) -> Result<(), ApiError> {
    let result = context_core(symbol, depth).await?;
    let json =
        serde_json::to_string(&result).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "context",
    version = "0.3.2",
    tool_name = "context",
    description = "Show a 360-degree view of a symbol (callers, callees, processes)."
)]
async fn context_mcp(symbol: String, depth: u32) -> Result<ContextOutput, ApiError> {
    context_core(symbol, depth).await
}
