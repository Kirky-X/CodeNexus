// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Search command: search for symbols by name or content.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::kit::QueryModule;
use crate::query::SearchResult;
use crate::service::error::{CodeNexusError, to_api_error};
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
async fn search_core(text: String, fulltext: bool, limit: u32) -> Result<SearchOutput, CodeNexusError> {
    let kit = kit().ok_or_else(CodeNexusError::kit_not_initialized)?;
    let q = kit.require::<QueryModule>()?;
    let results = if fulltext {
        q.fulltext_search(&text, None, limit as usize)
    } else {
        q.search(&text, None, limit as usize)
    }?;
    let results: Vec<Value> = results.iter().map(search_result_to_json).collect();
    Ok(SearchOutput {
        count: results.len(),
        results,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "search",
    version = "0.3.2",
    description = "Search for symbols by name (structured) or content (BM25 full-text).",
    cli = true
)]
async fn search(text: String, fulltext: bool, limit: u32) -> Result<(), ApiError> {
    let result = search_core(text, fulltext, limit)
        .await
        .map_err(|e| to_api_error(e, "search_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "search_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[service_api(
    name = "search",
    version = "0.3.2",
    tool_name = "search",
    description = "Search for symbols by name (structured) or content (BM25 full-text)."
)]
async fn search_mcp(text: String, fulltext: bool, limit: u32) -> Result<SearchOutput, ApiError> {
    search_core(text, fulltext, limit)
        .await
        .map_err(|e| to_api_error(e, "search_error"))
}
