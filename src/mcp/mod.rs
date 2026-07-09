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
use codenexus::cli::context_cmd::{
    collect_incoming, collect_outgoing, collect_processes, resolve_start_id, ContextOutput,
};
#[cfg(feature = "mcp")]
use codenexus::kit::QueryKey;
#[cfg(feature = "mcp")]
use codenexus::kit::TraceKey;
#[cfg(feature = "mcp")]
use codenexus::query::{QueryResult, SearchResult};
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

// ---------------------------------------------------------------------------
// Impact tool (T012)
// ---------------------------------------------------------------------------

/// JSON-serializable impact analysis result.
///
/// Contains the subgraph (nodes + edges) reachable from the target symbol
/// within `depth` hops, plus summary counts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactOutput {
    /// The symbol that was analyzed.
    pub symbol: String,
    /// The traversal depth used.
    pub depth: u32,
    /// Number of nodes in the subgraph.
    pub node_count: usize,
    /// Number of edges in the subgraph.
    pub edge_count: usize,
    /// Nodes in the subgraph (serialized via `serde_json::to_value`).
    pub nodes: Vec<Value>,
    /// Edges in the subgraph (serialized via `serde_json::to_value`).
    pub edges: Vec<Value>,
}

/// Converts a [`Graph`] into a JSON-serializable [`ImpactOutput`].
#[cfg(feature = "mcp")]
fn impact_output(symbol: String, depth: u32, graph: codenexus::model::Graph) -> ImpactOutput {
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

/// MCP tool: Analyze the blast radius (upstream callers) of changing a symbol.
///
/// Returns the subgraph reachable from `symbol` within `depth` hops, including
/// node/edge counts for quick assessment.
#[cfg(feature = "mcp")]
#[service_api(
    name = "impact",
    version = "v1",
    tool_name = "impact",
    description = "Analyze the blast radius (upstream callers) of changing a symbol."
)]
async fn impact(symbol: String, depth: u32) -> Result<ImpactOutput, ApiError> {
    let kit = kit().ok_or_else(|| {
        ApiError::internal_error("MCP server not initialized", "mcp_kit_not_initialized")
    })?;
    let trace_engine = kit
        .require::<TraceKey>()
        .map_err(|e| mcp_error("Failed to resolve trace capability", e))?;
    let graph = trace_engine
        .load_graph(&symbol, depth as usize)
        .map_err(|e| mcp_error("Impact graph load failed", e))?;
    Ok(impact_output(symbol, depth, graph))
}

// ---------------------------------------------------------------------------
// Search tool (T013)
// ---------------------------------------------------------------------------

/// JSON-serializable search result.
///
/// Contains the count and the search results as JSON objects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchOutput {
    /// Number of results returned.
    pub count: usize,
    /// Search results (JSON objects with name/label/filePath/startLine/
    /// qualifiedName/score).
    pub results: Vec<Value>,
}

/// Converts a [`SearchResult`] to a JSON object.
///
/// [`SearchResult`] doesn't derive `Serialize`, so we manually map its fields
/// to a JSON object matching the MCP output schema.
#[cfg(feature = "mcp")]
fn search_result_to_json(r: &SearchResult) -> Value {
    json!({
        "name": r.name,
        "label": r.label,
        "filePath": r.file_path,
        "startLine": r.start_line,
        "qualifiedName": r.qualified_name,
        "score": r.score,
    })
}

/// MCP tool: Search for symbols by name or content (full-text or semantic).
///
/// When `semantic` is true, uses BM25 full-text search; otherwise uses
/// structured name search (CONTAINS).
#[cfg(feature = "mcp")]
#[service_api(
    name = "search",
    version = "v1",
    tool_name = "search",
    description = "Search for symbols by name or content (full-text or semantic)."
)]
async fn search(text: String, semantic: bool, limit: u32) -> Result<SearchOutput, ApiError> {
    let kit = kit().ok_or_else(|| {
        ApiError::internal_error("MCP server not initialized", "mcp_kit_not_initialized")
    })?;
    let q = kit
        .require::<QueryKey>()
        .map_err(|e| mcp_error("Failed to resolve query capability", e))?;
    let results = if semantic {
        q.fulltext_search(&text, None, limit as usize)
    } else {
        q.search(&text, None, limit as usize)
    }
    .map_err(|e| mcp_error("Search execution failed", e))?;
    let results: Vec<Value> = results.iter().map(search_result_to_json).collect();
    Ok(SearchOutput {
        count: results.len(),
        results,
    })
}

// ---------------------------------------------------------------------------
// Context tool (T014)
// ---------------------------------------------------------------------------

/// MCP tool: Show a 360-degree view of a symbol (callers, callees, processes).
///
/// Loads the BFS subgraph around `symbol` via `TraceKey::load_graph`, resolves
/// the symbol to a node, then partitions the edges into incoming (callers),
/// outgoing (callees), and processes (structural participation).
///
/// Reuses `context_cmd`'s `collect_incoming`/`collect_outgoing`/`collect_processes`
/// to avoid duplicating the partitioning logic (Rule 8 — don't create a second
/// version of existing code).
#[cfg(feature = "mcp")]
#[service_api(
    name = "context",
    version = "v1",
    tool_name = "context",
    description = "Show a 360-degree view of a symbol (callers, callees, processes)."
)]
async fn context(symbol: String, depth: u32) -> Result<ContextOutput, ApiError> {
    let kit = kit().ok_or_else(|| {
        ApiError::internal_error("MCP server not initialized", "mcp_kit_not_initialized")
    })?;
    let trace_engine = kit
        .require::<TraceKey>()
        .map_err(|e| mcp_error("Failed to resolve trace capability", e))?;
    let graph = trace_engine
        .load_graph(&symbol, depth as usize)
        .map_err(|e| mcp_error("Context graph load failed", e))?;
    let start_id = resolve_start_id(&graph, &symbol).ok_or_else(|| {
        ApiError::InvalidInput {
            message: format!("symbol not found: {symbol}"),
            field: Some("symbol".to_string()),
            value: Some(Value::String(symbol.clone())),
        }
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
        node: codenexus::cli::context_cmd::SymbolNodeOutput::from(symbol_node),
        incoming,
        outgoing,
        processes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenexus::kit::{build_kit, KitBootstrapConfig};
    #[cfg(feature = "mcp")]
    use codenexus::kit::QueryKey;
    use std::sync::Mutex;

    /// Serializes MCP tests that share the global KIT's database.
    ///
    /// `storage.save_edges()` writes a CSV temp file with a fixed name
    /// ("coderelation.csv"); concurrent calls from multiple tests race on
    /// that file. This mutex ensures only one MCP test accesses the
    /// database at a time.
    static MCP_TEST_LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = MCP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = MCP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Verifies the `impact` MCP tool handler loads a subgraph and returns
    /// non-zero `node_count` for a seeded call graph.
    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn impact_tool_returns_subgraph() {
        let _guard = MCP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ensure kit is initialized (may already be set by another test).
        if kit().is_none() {
            let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
            let config = KitBootstrapConfig::new(tmp.path().to_path_buf());
            let built = build_kit(&config).expect("build_kit should succeed");
            std::mem::forget(tmp);
            let _ = init_kit(built);
        }

        // Seed two Function nodes and a CALLS edge.
        let k = kit().expect("kit should be initialized");
        let storage = k
            .require::<codenexus::kit::StorageKey>()
            .expect("require_storage");
        let node_a = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "impact_caller",
            "demo.impact_caller",
        )
        .id("f_impact_caller")
        .project("demo")
        .file_path("/src/impact_caller.rs")
        .start_line(1)
        .end_line(5)
        .language(codenexus::model::Language::Rust)
        .build();
        let node_b = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "impact_callee",
            "demo.impact_callee",
        )
        .id("f_impact_callee")
        .project("demo")
        .file_path("/src/impact_callee.rs")
        .start_line(1)
        .end_line(5)
        .language(codenexus::model::Language::Rust)
        .build();
        storage
            .save_nodes(&[node_a, node_b], codenexus::model::NodeLabel::Function)
            .expect("save_nodes");
        let edge = codenexus::model::Edge::new(
            "f_impact_caller",
            "f_impact_callee",
            codenexus::model::EdgeType::Calls,
            "demo",
        );
        storage.save_edges(&[edge]).expect("save_edges");

        // Call the impact handler.
        let result = impact("impact_caller".to_string(), 1)
            .await
            .expect("impact should succeed");

        // Assert the symbol is echoed back and the subgraph is non-empty.
        assert_eq!(result.symbol, "impact_caller");
        assert!(
            result.node_count > 0,
            "node_count should be > 0 for a seeded symbol"
        );
    }

    /// Verifies the `search` MCP tool handler searches for symbols by name and
    /// returns a non-empty `SearchOutput`.
    ///
    /// Seeds a Function node via the Query module's own connection (matching
    /// the `query_tool_executes_cypher` pattern — search uses QueryKey's
    /// connection, so seed via the same connection), then calls the `search`
    /// handler and asserts the seeded node is returned.
    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn search_tool_returns_results() {
        let _guard = MCP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ensure kit is initialized (may already be set by another test).
        if kit().is_none() {
            let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
            let config = KitBootstrapConfig::new(tmp.path().to_path_buf());
            let built = build_kit(&config).expect("build_kit should succeed");
            std::mem::forget(tmp);
            let _ = init_kit(built);
        }

        // Seed a Function node via the Query module's own connection — search
        // uses QueryKey's connection, so seed via the same connection.
        let k = kit().expect("kit should be initialized");
        let query_engine = k.require::<QueryKey>().expect("require_query");
        query_engine
            .cypher("CREATE (:Function {id: 'search_tool_test', name: 'searchable_func', qualifiedName: 'demo.searchable_func', filePath: '/src/search.rs', startLine: 1, project: 'demo'});")
            .expect("seed function");

        // Call the search handler — search for "searchable" (non-semantic).
        let result = search("searchable".to_string(), false, 10)
            .await
            .expect("search should succeed");

        // Assert non-empty results containing the seeded symbol.
        assert!(
            result.count > 0,
            "count should be > 0 for a seeded symbol matching 'searchable'"
        );
        let names: Vec<&str> = result
            .results
            .iter()
            .filter_map(|v| v.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(
            names.iter().any(|n| n.contains("searchable")),
            "results should contain a symbol with 'searchable' in its name, got: {names:?}"
        );
    }

    /// Verifies the `context` MCP tool handler loads a subgraph and returns a
    /// 360° view with incoming (callers) and outgoing (callees) edges.
    ///
    /// Seeds three Function nodes and two CALLS edges (A→B, A→C) via the
    /// Storage capability, then calls `context("context_caller", 1)` and
    /// asserts the result has non-empty `outgoing` (callees) and the symbol
    /// node is resolved.
    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn context_tool_returns_360_view() {
        let _guard = MCP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if kit().is_none() {
            let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
            let config = KitBootstrapConfig::new(tmp.path().to_path_buf());
            let built = build_kit(&config).expect("build_kit should succeed");
            std::mem::forget(tmp);
            let _ = init_kit(built);
        }

        // Seed three Function nodes: context_caller calls context_callee_a
        // and context_callee_b.
        let k = kit().expect("kit should be initialized");
        let storage = k
            .require::<codenexus::kit::StorageKey>()
            .expect("require_storage");
        let node_a = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "context_caller",
            "demo.context_caller",
        )
        .id("f_context_caller")
        .project("demo")
        .file_path("/src/context_caller.rs")
        .start_line(1)
        .end_line(5)
        .language(codenexus::model::Language::Rust)
        .build();
        let node_b = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "context_callee_a",
            "demo.context_callee_a",
        )
        .id("f_context_callee_a")
        .project("demo")
        .file_path("/src/context_callee_a.rs")
        .start_line(1)
        .end_line(5)
        .language(codenexus::model::Language::Rust)
        .build();
        let node_c = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "context_callee_b",
            "demo.context_callee_b",
        )
        .id("f_context_callee_b")
        .project("demo")
        .file_path("/src/context_callee_b.rs")
        .start_line(1)
        .end_line(5)
        .language(codenexus::model::Language::Rust)
        .build();
        storage
            .save_nodes(&[node_a, node_b, node_c], codenexus::model::NodeLabel::Function)
            .expect("save_nodes");
        let edge_ab = codenexus::model::Edge::new(
            "f_context_caller",
            "f_context_callee_a",
            codenexus::model::EdgeType::Calls,
            "demo",
        );
        let edge_ac = codenexus::model::Edge::new(
            "f_context_caller",
            "f_context_callee_b",
            codenexus::model::EdgeType::Calls,
            "demo",
        );
        storage.save_edges(&[edge_ab, edge_ac]).expect("save_edges");

        // Call the context handler.
        let result = context("context_caller".to_string(), 1)
            .await
            .expect("context should succeed");

        // Assert the symbol is echoed back and the node is resolved.
        assert_eq!(result.symbol, "context_caller");
        assert_eq!(result.node.name, "context_caller");
        // Outgoing = callees (context_caller calls callee_a and callee_b).
        assert!(
            !result.outgoing.is_empty(),
            "outgoing (callees) should be non-empty for a seeded call graph"
        );
        let callee_names: Vec<&str> = result.outgoing.iter().map(|r| r.name.as_str()).collect();
        assert!(
            callee_names.contains(&"context_callee_a"),
            "outgoing should contain context_callee_a, got: {callee_names:?}"
        );
        assert!(
            callee_names.contains(&"context_callee_b"),
            "outgoing should contain context_callee_b, got: {callee_names:?}"
        );
    }
}
