// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Context command: show a 360-degree view of a symbol.

use crate::kit::TraceKey;
use crate::service::error::{CliError, to_api_error};
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
async fn context_core(symbol: String, depth: u32) -> Result<ContextOutput, CliError> {
    let kit = kit().ok_or_else(CliError::kit_not_initialized)?;
    let trace_engine = kit.require::<TraceKey>()?;
    let graph = trace_engine.load_graph(&symbol, depth as usize)?;
    let start_id = resolve_start_id(&graph, &symbol)
        .ok_or_else(|| CliError::InvalidInput(format!("symbol not found: {symbol}")))?;
    let symbol_node = graph.get_node(&start_id).ok_or_else(|| {
        CliError::Internal(format!("symbol node resolved but not in graph: {symbol}"))
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
    let result = context_core(symbol, depth)
        .await
        .map_err(|e| to_api_error(e, "context_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CliError::from(e), "context_error"))?;
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
    context_core(symbol, depth)
        .await
        .map_err(|e| to_api_error(e, "context_error"))
}
