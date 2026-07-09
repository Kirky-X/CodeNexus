// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! MCP server module (v0.3.0, T009) — sdforge-based MCP protocol exposure.
//!
//! Replaces the hand-written JSON-RPC in `src/cli/mcp_cmd.rs` with sdforge's
//! declarative `#[service_api]` macro + rmcp stdio transport.
//!
//! # Architecture
//!
//! 1. [`init_kit()`] stores the Kit in a global `OnceLock<Arc<Kit>>`
//! 2. Tool handlers (defined via `#[service_api]`) access the Kit via [`kit()`]
//! 3. [`serve()`] builds the sdforge MCP server from registered tools and
//!    serves it over stdio using a tokio runtime
//! 4. [`run()`] is the entry point that calls `init_kit` then `serve`
//!
//! # Why OnceLock instead of passing Kit to handlers
//!
//! sdforge's `#[service_api]` macro generates standalone async functions with
//! no mechanism to inject runtime state. A process-global `OnceLock` is the
//! simplest way to make the Kit available to these functions without wrapping
//! every handler in a closure or struct.

use std::sync::{Arc, OnceLock};

use codenexus::cli::args::McpArgs;
use codenexus::cli::error::{CliError, Result as CliResult};
use codenexus::kit::Kit;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// MCP-feature-gated imports — only needed when building sdforge MCP tools.
#[cfg(feature = "mcp")]
use codenexus::kit::QueryKey;
#[cfg(feature = "mcp")]
use codenexus::kit::TraceKey;
#[cfg(feature = "mcp")]
use codenexus::query::QueryResult;
#[cfg(feature = "mcp")]
use codenexus::trace::{TraceEdge, TraceNode, TraceResult, TraceType};
#[cfg(feature = "mcp")]
use sdforge::prelude::ApiError;
#[cfg(feature = "mcp")]
use sdforge::service_api;
#[cfg(feature = "mcp")]
use serde_json::json;

/// Global Kit instance injected into MCP tool handlers.
///
/// Set once by [`init_kit()`], accessed by tool handlers via [`kit()`].
static KIT: OnceLock<Arc<Kit>> = OnceLock::new();

/// Returns the Kit instance if initialized, or `None` if [`run`] hasn't been
/// called.
///
/// Tool handlers use this to access the query/trace/storage capabilities:
///
/// ```no_run
/// let kit = codenexus::mcp::kit().expect("Kit not initialized");
/// let query = kit.require::<codenexus::kit::QueryKey>()?;
/// ```
#[must_use]
pub fn kit() -> Option<&'static Arc<Kit>> {
    KIT.get()
}

/// Stores the Kit in the global `OnceLock` so tool handlers can access it.
///
/// This is separated from [`serve()`] so it can be tested independently
/// (the serve loop blocks on stdin, which is not testable in unit tests).
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] if the Kit has already been initialized
/// (the `OnceLock` is set-once).
pub fn init_kit(kit: Kit) -> CliResult<()> {
    KIT.set(Arc::new(kit)).map_err(|_| {
        CliError::InvalidInput("MCP server already initialized".to_string())
    })
}

/// Starts the sdforge MCP server over stdio.
///
/// Builds the MCP server from registered tools (via `inventory`) and serves
/// it over stdio using rmcp's `ServiceExt::serve` with a tokio runtime.
///
/// # Errors
///
/// Returns [`CliError::Io`] if the tokio runtime fails to create, or
/// [`CliError::InvalidInput`] if the MCP server fails to start or serve.
fn serve() -> CliResult<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        use rmcp::ServiceExt;
        let server = sdforge::mcp::build();
        let transport = rmcp::transport::stdio();
        let service = server
            .serve(transport)
            .await
            .map_err(|e| CliError::InvalidInput(format!("MCP serve error: {e}")))?;
        service
            .waiting()
            .await
            .map_err(|e| CliError::InvalidInput(format!("MCP service error: {e}")))?;
        Ok(())
    })
}

/// Entry point for the `mcp` CLI subcommand.
///
/// Stores the Kit in the global `OnceLock` via [`init_kit()`], then starts
/// the sdforge MCP server over stdio via [`serve()`].
///
/// # Arguments
///
/// * `kit` - The fully-wired Kit with all capabilities registered.
/// * `_args` - MCP subcommand arguments (currently unused — the server
///   speaks stdio only).
///
/// # Errors
///
/// See [`init_kit()`] and [`serve()`] for error conditions.
pub fn run(kit: Kit, _args: &McpArgs) -> CliResult<()> {
    init_kit(kit)?;
    serve()
}

// ---------------------------------------------------------------------------
// Query tool (T010)
// ---------------------------------------------------------------------------

/// JSON-serializable query result (migrated from `cli::mcp_cmd::QueryOutput`).
///
/// Mirrors [`QueryResult`] but with `Serialize`/`Deserialize` for MCP transport.
///
/// [`QueryResult`]: codenexus::query::QueryResult
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryOutput {
    /// Column names returned by the Cypher query.
    pub columns: Vec<String>,
    /// Row values, one inner `Vec` per row, each element a JSON value.
    pub rows: Vec<Vec<Value>>,
    /// Wall-clock execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Converts a [`QueryResult`] into a JSON-serializable [`QueryOutput`].
#[cfg(feature = "mcp")]
fn query_output(r: QueryResult) -> QueryOutput {
    QueryOutput {
        columns: r.columns,
        rows: r.rows,
        duration_ms: r.duration_ms,
    }
}

/// Wraps an error as an `ApiError::Internal` with a timestamp-based `error_id`.
///
/// Used by MCP tool handlers to convert subsystem errors (`KitError`,
/// `QueryError`, etc.) into the sdforge error type expected by the
/// `#[service_api]` macro.
#[cfg(feature = "mcp")]
fn mcp_error<E: std::error::Error + Send + Sync + 'static>(
    message: impl Into<String>,
    source: E,
) -> ApiError {
    use std::time::{SystemTime, UNIX_EPOCH};
    let error_id = format!(
        "{:016x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    ApiError::internal_with_source(message, error_id, source)
}

/// MCP tool: Execute a Cypher query against the CodeNexus knowledge graph.
///
/// Registered automatically via sdforge's `inventory::submit!` when the `mcp`
/// feature is enabled. The `#[service_api]` macro generates a `SdForgeTool`
/// struct that deserializes `{"cypher": "..."}` from the MCP input and
/// serializes the [`QueryOutput`] into the MCP response.
#[cfg(feature = "mcp")]
#[service_api(
    name = "query",
    version = "v1",
    tool_name = "query",
    description = "Execute a Cypher query against the CodeNexus knowledge graph."
)]
async fn query(cypher: String) -> Result<QueryOutput, ApiError> {
    let kit = kit().ok_or_else(|| {
        ApiError::internal_error("MCP server not initialized", "mcp_kit_not_initialized")
    })?;
    let q = kit
        .require::<QueryKey>()
        .map_err(|e| mcp_error("Failed to resolve query capability", e))?;
    let result = q
        .cypher(&cypher)
        .map_err(|e| mcp_error("Query execution failed", e))?;
    Ok(query_output(result))
}

// ---------------------------------------------------------------------------
// Trace tool (T011)
// ---------------------------------------------------------------------------

/// JSON-serializable trace result (migrated from `cli::mcp_cmd::TraceOutput`).
///
/// Mirrors [`TraceResult`] but with `Serialize`/`Deserialize` for MCP transport.
/// [`TraceNode`] and [`TraceEdge`] don't derive `Serialize`, so they are
/// converted to `serde_json::Value` via [`trace_node_to_json`] and
/// [`trace_edge_to_json`].
///
/// [`TraceResult`]: codenexus::trace::TraceResult
/// [`TraceNode`]: codenexus::trace::TraceNode
/// [`TraceEdge`]: codenexus::trace::TraceEdge
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceOutput {
    /// The symbol that was traced.
    pub symbol: String,
    /// The trace paths discovered.
    pub paths: Vec<TracePathOutput>,
}

/// A single trace path — a sequence of nodes and edges at a given depth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TracePathOutput {
    /// Nodes along this path (JSON objects with name/label/filePath/startLine).
    pub nodes: Vec<Value>,
    /// Edges along this path (JSON objects with edgeType/reason/confidence).
    pub edges: Vec<Value>,
    /// The depth at which this path was discovered.
    pub depth: usize,
}

/// Converts a [`TraceResult`] into a JSON-serializable [`TraceOutput`].
#[cfg(feature = "mcp")]
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

/// Converts a [`TraceNode`] to a JSON object.
///
/// [`TraceNode`] doesn't derive `Serialize`, so we manually map its fields
/// to a JSON object matching the MCP output schema.
#[cfg(feature = "mcp")]
fn trace_node_to_json(n: &TraceNode) -> Value {
    json!({
        "name": n.name,
        "label": n.label,
        "filePath": n.file_path,
        "startLine": n.start_line,
    })
}

/// Converts a [`TraceEdge`] to a JSON object.
///
/// [`TraceEdge`] doesn't derive `Serialize`, so we manually map its fields
/// to a JSON object matching the MCP output schema.
#[cfg(feature = "mcp")]
fn trace_edge_to_json(e: &TraceEdge) -> Value {
    json!({
        "edgeType": e.edge_type,
        "reason": e.reason,
        "confidence": e.confidence,
    })
}

/// MCP tool: Trace a symbol's call and/or data-flow paths.
///
/// Registered automatically via sdforge's `inventory::submit!` when the `mcp`
/// feature is enabled. The `#[service_api]` macro generates a `SdForgeTool`
/// struct that deserializes `{"symbol": "...", "trace_type": "...", "depth": N}`
/// from the MCP input and serializes the [`TraceOutput`] into the MCP response.
#[cfg(feature = "mcp")]
#[service_api(
    name = "trace",
    version = "v1",
    tool_name = "trace",
    description = "Trace a symbol's call and/or data-flow paths."
)]
async fn trace(symbol: String, trace_type: String, depth: u32) -> Result<TraceOutput, ApiError> {
    let kit = kit().ok_or_else(|| {
        ApiError::internal_error("MCP server not initialized", "mcp_kit_not_initialized")
    })?;
    let tt = TraceType::from_cli_str(&trace_type).ok_or_else(|| ApiError::InvalidInput {
        message: format!("invalid trace_type: {trace_type} (expected calls|dataflow|all)"),
        field: Some("trace_type".to_string()),
        value: Some(Value::String(trace_type)),
    })?;
    let trace_engine = kit
        .require::<TraceKey>()
        .map_err(|e| mcp_error("Failed to resolve trace capability", e))?;
    let result = trace_engine
        .trace(&symbol, tt, depth as usize)
        .map_err(|e| mcp_error("Trace execution failed", e))?;
    Ok(trace_output(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenexus::kit::{build_kit, KitBootstrapConfig};
    #[cfg(feature = "mcp")]
    use codenexus::kit::QueryKey;

    /// Verifies that `init_kit` stores the Kit in the global `OnceLock`.
    ///
    /// Note: This test sets the process-global `KIT` OnceLock. Since `OnceLock`
    /// is set-once and cargo runs tests in the same process using threads,
    /// another test (e.g. `query_tool_executes_cypher`) may have already
    /// initialized the Kit. We tolerate that race by ignoring `init_kit`'s
    /// "already initialized" error and just verifying `kit()` returns `Some`.
    #[test]
    fn mcp_run_initializes_kit() {
        // Build a minimal Kit with an in-memory database.
        // std::mem::forget keeps the temp file alive for the process lifetime
        // (matching the pattern in `cli::mcp_cmd::tests::fresh_kit`) so other
        // tests using the global KIT can still access the database.
        let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
        let config = KitBootstrapConfig::new(tmp.path().to_path_buf());
        let built = build_kit(&config).expect("build_kit should succeed");
        std::mem::forget(tmp);

        // init_kit may fail if another test already set the OnceLock (set-once
        // semantics). That's expected — we just need kit() to be Some after.
        let _ = init_kit(built);

        // After init (or if already initialized by another test), kit() must
        // return Some.
        assert!(kit().is_some(), "kit() should return Some after init_kit");
    }

    /// Verifies the `query` MCP tool handler executes a Cypher query and
    /// returns a non-empty `QueryOutput`.
    ///
    /// This test requires the `mcp` feature (for the `query` function and
    /// sdforge `ApiError` type). It seeds a Project node via the Query
    /// module's own connection (LadybugDB cross-handle isolation means
    /// Storage module writes are not visible to Query), then calls the
    /// `query` handler and asserts the seeded node is returned.
    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn query_tool_executes_cypher() {
        // Ensure kit is initialized (may already be set by another test in
        // the same process — OnceLock is set-once). We check-then-init, but
        // tolerate the race where another test sets KIT between our check
        // and our init_kit call.
        if kit().is_none() {
            let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
            let config = KitBootstrapConfig::new(tmp.path().to_path_buf());
            let built = build_kit(&config).expect("build_kit should succeed");
            std::mem::forget(tmp);
            let _ = init_kit(built); // may fail if another test raced us
        }

        // Seed via the Query module's own connection — the Storage module
        // opens a separate handle whose writes are not visible to Query.
        let k = kit().expect("kit should be initialized");
        let query_engine = k.require::<QueryKey>().expect("require_query");
        query_engine
            .cypher("CREATE (:Project {id: 'query_tool_test', name: 'query_tool_test', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0, lastCommit: ''});")
            .expect("seed project");

        // Call the query handler.
        let result = query(
            "MATCH (p:Project {name: 'query_tool_test'}) RETURN p.name AS name".to_string(),
        )
        .await
        .expect("query should succeed");

        // Assert non-empty result with expected structure.
        assert!(!result.columns.is_empty(), "columns should be non-empty");
        assert_eq!(result.columns, vec!["name".to_string()]);
        assert!(!result.rows.is_empty(), "rows should be non-empty");
        assert_eq!(result.rows[0][0], "query_tool_test");
    }

    /// Verifies the `trace` MCP tool handler traces a seeded call graph and
    /// returns a non-empty `TraceOutput` with paths.
    ///
    /// Seeds two Function nodes and a CALLS edge via the Storage capability
    /// (the Trace capability opens its own DB connection — Storage writes are
    /// visible to Trace after flushing, matching the pattern in
    /// `cli::mcp_cmd::tests`).
    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn trace_tool_returns_paths() {
        // Ensure kit is initialized (may already be set by another test).
        if kit().is_none() {
            let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
            let config = KitBootstrapConfig::new(tmp.path().to_path_buf());
            let built = build_kit(&config).expect("build_kit should succeed");
            std::mem::forget(tmp);
            let _ = init_kit(built);
        }

        // Seed two Function nodes and a CALLS edge via the Storage capability.
        // The Trace capability opens its own connection and can see
        // Storage-written data after flush.
        let k = kit().expect("kit should be initialized");
        let storage = k
            .require::<codenexus::kit::StorageKey>()
            .expect("require_storage");
        let node_a = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "trace_caller",
            "demo.trace_caller",
        )
        .id("f_trace_caller")
        .project("demo")
        .file_path("/src/caller.rs")
        .start_line(1)
        .end_line(5)
        .language(codenexus::model::Language::Rust)
        .build();
        let node_b = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "trace_callee",
            "demo.trace_callee",
        )
        .id("f_trace_callee")
        .project("demo")
        .file_path("/src/callee.rs")
        .start_line(1)
        .end_line(5)
        .language(codenexus::model::Language::Rust)
        .build();
        storage
            .save_nodes(&[node_a, node_b], codenexus::model::NodeLabel::Function)
            .expect("save_nodes");
        let edge = codenexus::model::Edge::new(
            "f_trace_caller",
            "f_trace_callee",
            codenexus::model::EdgeType::Calls,
            "demo",
        );
        storage.save_edges(&[edge]).expect("save_edges");

        // Call the trace handler — trace "trace_caller" with depth 1.
        let result = trace("trace_caller".to_string(), "all".to_string(), 1)
            .await
            .expect("trace should succeed");

        // Assert the symbol is echoed back and at least one path exists.
        assert_eq!(result.symbol, "trace_caller");
        assert!(
            !result.paths.is_empty(),
            "paths should be non-empty for a seeded call graph"
        );
    }
}
