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
use codenexus::query::QueryResult;
#[cfg(feature = "mcp")]
use sdforge::prelude::ApiError;
#[cfg(feature = "mcp")]
use sdforge::service_api;

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
}
