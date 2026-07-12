// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Context command: show a 360-degree view of a symbol.

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, TraceModule};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
use crate::trace::context::{
    collect_incoming, collect_outgoing, collect_processes, resolve_start_id,
};
use crate::trace::types::{ContextOutput, SymbolNodeOutput};

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// Runs context against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_context(
    kit: &AsyncKit<AsyncReady>,
    symbol: &str,
    depth: u32,
) -> Result<ContextOutput, CodeNexusError> {
    let trace_engine = kit.require::<TraceModule>()?;
    let graph = trace_engine.load_graph(symbol, depth as usize)?;
    let start_id = resolve_start_id(&graph, symbol)
        .ok_or_else(|| CodeNexusError::InvalidInput(format!("symbol not found: {symbol}")))?;
    let symbol_node = graph.get_node(&start_id).ok_or_else(|| {
        CodeNexusError::Internal(format!("symbol node resolved but not in graph: {symbol}"))
    })?;
    let incoming = collect_incoming(&graph, &start_id);
    let outgoing = collect_outgoing(&graph, &start_id);
    let processes = collect_processes(&graph, &start_id);
    Ok(ContextOutput {
        symbol: symbol.to_string(),
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
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result =
        run_context(&kit, &symbol, depth).map_err(|e| to_api_error(e, "context_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "context_error"))?;
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
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run_context(&kit, &symbol, depth).map_err(|e| to_api_error(e, "context_error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_context_testdb");
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
    fn run_context_returns_invalid_input_for_unknown_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "nonexistent.symbol", 3)
            .expect_err("unknown symbol should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("symbol not found"),
            "error should mention 'symbol not found': {msg}"
        );
        assert!(
            msg.contains("nonexistent.symbol"),
            "error should mention the missing symbol: {msg}"
        );
    }

    #[test]
    fn run_context_returns_invalid_input_for_empty_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "", 3).expect_err("empty symbol should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn run_context_returns_invalid_input_at_depth_zero() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "missing.symbol", 0).expect_err("missing symbol should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn run_context_error_message_format() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_context(&kit, "foo.bar", 2).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.starts_with("invalid input: symbol not found: foo.bar"),
            "unexpected message: {msg}"
        );
    }
}
