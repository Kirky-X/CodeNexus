// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Search command: search for symbols by name or content.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::kit::QueryKey;
use crate::query::SearchResult;
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::runtime::kit;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchOutput {
    pub count: usize,
    pub results: Vec<Value>,
}

#[cfg(any(feature = "cli", feature = "mcp"))]
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

/// Core search logic — shared by CLI and MCP wrappers.
#[cfg(any(feature = "cli", feature = "mcp"))]
async fn search_core(
    text: String,
    fulltext: bool,
    limit: u32,
) -> Result<SearchOutput, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let q = kit
        .require::<QueryKey>()
        .map_err(|e| wrap_error("Failed to resolve query capability", e))?;
    let results = if fulltext {
        q.fulltext_search(&text, None, limit as usize)
    } else {
        q.search(&text, None, limit as usize)
    }
    .map_err(|e| wrap_error("Search execution failed", e))?;
    let results: Vec<Value> = results.iter().map(search_result_to_json).collect();
    Ok(SearchOutput {
        count: results.len(),
        results,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "codenexus",
    version = "0.3.2",
    tool_name = "search",
    description = "Search for symbols by name (structured) or content (BM25 full-text).",
    cli = true,
)]
async fn search(text: String, fulltext: bool, limit: u32) -> Result<(), ApiError> {
    let result = search_core(text, fulltext, limit).await?;
    let json = serde_json::to_string(&result)
        .map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "codenexus",
    version = "0.3.2",
    tool_name = "search",
    description = "Search for symbols by name (structured) or content (BM25 full-text).",
)]
async fn search_mcp(text: String, fulltext: bool, limit: u32) -> Result<SearchOutput, ApiError> {
    search_core(text, fulltext, limit).await
}
