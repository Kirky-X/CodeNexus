// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Query command: execute Cypher queries against the knowledge graph.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kit::QueryKey;
use crate::query::{validate_cypher_subset, QueryResult};
use crate::service::error::{CliError, to_api_error};
use crate::service::runtime::kit;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable query result.
///
/// Mirrors [`QueryResult`] but with `Serialize`/`Deserialize` for transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub duration_ms: u64,
}

#[cfg(any(feature = "cli", feature = "mcp"))]
fn query_output(r: QueryResult) -> QueryOutput {
    QueryOutput {
        columns: r.columns,
        rows: r.rows,
        duration_ms: r.duration_ms,
    }
}

/// Core query logic — shared by CLI and MCP wrappers.
#[cfg(any(feature = "cli", feature = "mcp"))]
async fn query_core(cypher: String) -> Result<QueryOutput, CliError> {
    let kit = kit().ok_or_else(CliError::kit_not_initialized)?;
    let q = kit.require::<QueryKey>()?;
    validate_cypher_subset(&cypher)?;
    let result = q.cypher(&cypher)?;
    Ok(query_output(result))
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "query",
    version = "0.3.2",
    description = "Execute a Cypher query against the CodeNexus knowledge graph.",
    cli = true
)]
async fn query(cypher: String) -> Result<(), ApiError> {
    let result = query_core(cypher)
        .await
        .map_err(|e| to_api_error(e, "query_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CliError::from(e), "query_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "query",
    version = "0.3.2",
    tool_name = "query",
    description = "Execute a Cypher query against the CodeNexus knowledge graph."
)]
async fn query_mcp(cypher: String) -> Result<QueryOutput, ApiError> {
    query_core(cypher)
        .await
        .map_err(|e| to_api_error(e, "query_error"))
}
