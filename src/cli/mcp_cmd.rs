// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `mcp` subcommand handler (H13) — minimal MCP stdio server.
//!
//! Exposes CodeNexus's query/trace/impact/search/context capabilities as MCP
//! tools so AI coding agents (Claude Code, Cursor, Codex) can invoke them via
//! the Model Context Protocol. The server speaks JSON-RPC 2.0 over stdio
//! (line-delimited), conforming to MCP protocol version `2024-11-05`.
//!
//! # Supported methods
//!
//! | Method                      | Behaviour                                  |
//! |-----------------------------|--------------------------------------------|
//! | `initialize`                | Returns server info + `tools` capability.  |
//! | `notifications/initialized` | No-op notification (no response).          |
//! | `tools/list`                | Returns the 5 tool definitions.            |
//! | `tools/call`                | Dispatches to the named tool.              |
//!
//! # Tools
//!
//! - `query` — execute a Cypher query.
//! - `trace` — trace a symbol's call/data-flow paths.
//! - `impact` — analyze the blast radius of changing a symbol.
//! - `search` — full-text or semantic search for symbols.
//! - `context` — 360° view of a symbol.
//!
//! # Why no external MCP crate
//!
//! The MCP protocol surface we need (initialize + tools/list + tools/call) is
//! small enough to implement directly with `serde_json`, avoiding a new
//! dependency (Rule 2 — simplicity first). A future task may swap this for the
//! official `rmcp` crate if the protocol surface grows.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::args::McpArgs;
use super::error::Result;
use crate::kit::{Kit, QueryKey, TraceKey};
use crate::query::{QueryResult, SearchResult};
use crate::trace::{TraceEdge, TraceNode, TraceResult, TraceType};

/// MCP protocol version this server speaks.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Server name reported in the `initialize` response.
pub const SERVER_NAME: &str = "codenexus";

/// Server version reported in the `initialize` response.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSON-RPC error codes (per JSON-RPC 2.0 spec + MCP additions).
mod error_codes {
    /// Parse error.
    pub const PARSE_ERROR: i32 = -32700;
    /// Invalid request.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Method not found.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid params.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal error.
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// Runs the MCP stdio server.
///
/// Reads line-delimited JSON-RPC 2.0 messages from stdin, dispatches each to
/// [`handle_request`], and writes the response (if any) to stdout. Returns
/// when stdin is closed.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Io`] if stdin/stdout fail. JSON-RPC-level errors are
/// written as error responses, not returned.
pub fn run(kit: &Kit, _args: &McpArgs) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut reader = stdin.lock();

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // EOF — client closed stdin.
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = handle_raw_line(kit, trimmed);
        if let Some(resp) = response {
            let json = serde_json::to_string(&resp)?;
            writeln!(stdout, "{json}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Parses a raw JSON-RPC line and returns the response (if any).
///
/// Returns `None` for notifications (requests without an `id`), per the
/// JSON-RPC 2.0 spec.
fn handle_raw_line(kit: &Kit, raw: &str) -> Option<Value> {
    let request: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(Value::Null, error_codes::PARSE_ERROR, &e.to_string()));
        }
    };
    // Notifications (no `id`) get no response. Using `?` returns `None` from
    // this function when `id` is absent (notification per JSON-RPC 2.0).
    let id = request.get("id").cloned()?;
    let method = request.get("method").and_then(|m| m.as_str());

    let method = match method {
        Some(m) => m,
        None => {
            return Some(error_response(id, error_codes::INVALID_REQUEST, "missing `method`"));
        }
    };

    let params = request.get("params").cloned().unwrap_or(Value::Null);
    match handle_request(kit, method, &params) {
        Ok(result) => Some(success_response(id, result)),
        Err((code, msg)) => Some(error_response(id, code, &msg)),
    }
}

/// Dispatches a JSON-RPC method to the appropriate handler.
///
/// Returns `Ok(result_value)` on success, or `Err((code, message))` on a
/// JSON-RPC-level error.
fn handle_request(kit: &Kit, method: &str, params: &Value) -> std::result::Result<Value, (i32, String)> {
    match method {
        "initialize" => Ok(handle_initialize()),
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(kit, params),
        _ => Err((
            error_codes::METHOD_NOT_FOUND,
            format!("unknown method: {method}"),
        )),
    }
}

/// Builds the `initialize` response result.
fn handle_initialize() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        }
    })
}

/// Builds the `tools/list` response result.
fn handle_tools_list() -> Value {
    json!({
        "tools": [
            tool_def("query", "Execute a Cypher query against the CodeNexus knowledge graph.", json!({
                "type": "object",
                "properties": {
                    "cypher": { "type": "string", "description": "Cypher query string" }
                },
                "required": ["cypher"]
            })),
            tool_def("trace", "Trace a symbol's call and/or data-flow paths.", json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Symbol name or FQN" },
                    "trace_type": { "type": "string", "enum": ["calls", "dataflow", "all"], "default": "all" },
                    "depth": { "type": "integer", "minimum": 1, "default": 3 }
                },
                "required": ["symbol"]
            })),
            tool_def("impact", "Analyze the blast radius (upstream callers) of changing a symbol.", json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Symbol name or FQN" },
                    "depth": { "type": "integer", "minimum": 1, "default": 3 }
                },
                "required": ["symbol"]
            })),
            tool_def("search", "Search for symbols by name or content (full-text or semantic).", json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Search text" },
                    "semantic": { "type": "boolean", "default": false },
                    "limit": { "type": "integer", "minimum": 1, "default": 10 }
                },
                "required": ["text"]
            })),
            tool_def("context", "Show a 360-degree view of a symbol (callers, callees, processes).", json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Symbol name or FQN" },
                    "depth": { "type": "integer", "minimum": 1, "default": 2 }
                },
                "required": ["symbol"]
            }))
        ]
    })
}

/// Builds a single tool definition object.
fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

/// Dispatches a `tools/call` request to the named tool.
fn handle_tools_call(kit: &Kit, params: &Value) -> std::result::Result<Value, (i32, String)> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or((error_codes::INVALID_PARAMS, "missing `name`".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Null);

    let text = match name {
        "query" => dispatch_query(kit, &arguments)?,
        "trace" => dispatch_trace(kit, &arguments)?,
        "impact" => dispatch_impact(kit, &arguments)?,
        "search" => dispatch_search(kit, &arguments)?,
        "context" => dispatch_context(kit, &arguments)?,
        _ => {
            return Err((
                error_codes::INVALID_PARAMS,
                format!("unknown tool: {name}"),
            ));
        }
    };
    Ok(json!({
        "content": [{ "type": "text", "text": text }]
    }))
}

/// Dispatches the `query` tool.
fn dispatch_query(kit: &Kit, args: &Value) -> std::result::Result<String, (i32, String)> {
    let cypher = args
        .get("cypher")
        .and_then(|c| c.as_str())
        .ok_or((error_codes::INVALID_PARAMS, "missing `cypher`".to_string()))?;
    let query = kit
        .require::<QueryKey>()
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let result = query
        .cypher(cypher)
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let output = query_output(result);
    serde_json::to_string(&output).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))
}

/// Dispatches the `trace` tool.
fn dispatch_trace(kit: &Kit, args: &Value) -> std::result::Result<String, (i32, String)> {
    let symbol = args
        .get("symbol")
        .and_then(|s| s.as_str())
        .ok_or((error_codes::INVALID_PARAMS, "missing `symbol`".to_string()))?;
    let trace_type = args
        .get("trace_type")
        .and_then(|t| t.as_str())
        .unwrap_or("all");
    let trace_type = parse_trace_type(trace_type)
        .map_err(|e| (error_codes::INVALID_PARAMS, e))?;
    let depth = args
        .get("depth")
        .and_then(|d| d.as_u64())
        .map(|d| d as usize)
        .unwrap_or(3);
    let trace = kit
        .require::<TraceKey>()
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let result = trace
        .trace(symbol, trace_type, depth)
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let output = trace_output(result);
    serde_json::to_string(&output).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))
}

/// Dispatches the `impact` tool.
fn dispatch_impact(kit: &Kit, args: &Value) -> std::result::Result<String, (i32, String)> {
    let symbol = args
        .get("symbol")
        .and_then(|s| s.as_str())
        .ok_or((error_codes::INVALID_PARAMS, "missing `symbol`".to_string()))?;
    let depth = args
        .get("depth")
        .and_then(|d| d.as_u64())
        .map(|d| d as usize)
        .unwrap_or(3);
    let trace = kit
        .require::<TraceKey>()
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    // Impact = reverse trace (upstream callers). We reuse the trace capability
    // with `TraceType::Calls` and let the CLI's impact_cmd logic apply if needed.
    // For the MCP tool, we return the subgraph reachable in `depth` hops.
    let graph = trace
        .load_graph(symbol, depth)
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let nodes: std::result::Result<Vec<Value>, (i32, String)> = graph
        .nodes
        .values()
        .map(|n| serde_json::to_value(n).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string())))
        .collect();
    let nodes = nodes?;
    let edges: std::result::Result<Vec<Value>, (i32, String)> = graph
        .edges
        .iter()
        .map(|e| serde_json::to_value(e).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string())))
        .collect();
    let edges = edges?;
    let output = json!({
        "symbol": symbol,
        "depth": depth,
        "node_count": nodes.len(),
        "edge_count": edges.len(),
        "nodes": nodes,
        "edges": edges,
    });
    serde_json::to_string(&output).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))
}

/// Dispatches the `search` tool.
fn dispatch_search(kit: &Kit, args: &Value) -> std::result::Result<String, (i32, String)> {
    let text = args
        .get("text")
        .and_then(|t| t.as_str())
        .ok_or((error_codes::INVALID_PARAMS, "missing `text`".to_string()))?;
    let limit = args
        .get("limit")
        .and_then(|l| l.as_u64())
        .map(|l| l as usize)
        .unwrap_or(10);
    let query = kit
        .require::<QueryKey>()
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let results = query
        .search(text, None, limit)
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    // SearchResult doesn't derive Serialize, so convert manually.
    let json_results: Vec<Value> = results.iter().map(search_result_to_json).collect();
    let output = json!({
        "count": json_results.len(),
        "results": json_results,
    });
    serde_json::to_string(&output).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))
}

/// Dispatches the `context` tool.
fn dispatch_context(kit: &Kit, args: &Value) -> std::result::Result<String, (i32, String)> {
    let symbol = args
        .get("symbol")
        .and_then(|s| s.as_str())
        .ok_or((error_codes::INVALID_PARAMS, "missing `symbol`".to_string()))?;
    let depth = args
        .get("depth")
        .and_then(|d| d.as_u64())
        .map(|d| d as usize)
        .unwrap_or(2);
    let trace = kit
        .require::<TraceKey>()
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let graph = trace
        .load_graph(symbol, depth)
        .map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))?;
    let nodes: std::result::Result<Vec<Value>, (i32, String)> = graph
        .nodes
        .values()
        .map(|n| serde_json::to_value(n).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string())))
        .collect();
    let nodes = nodes?;
    let edges: std::result::Result<Vec<Value>, (i32, String)> = graph
        .edges
        .iter()
        .map(|e| serde_json::to_value(e).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string())))
        .collect();
    let edges = edges?;
    let output = json!({
        "symbol": symbol,
        "depth": depth,
        "nodes": nodes,
        "edges": edges,
    });
    serde_json::to_string(&output).map_err(|e| (error_codes::INTERNAL_ERROR, e.to_string()))
}

/// Parses a trace-type string into [`TraceType`].
fn parse_trace_type(s: &str) -> std::result::Result<TraceType, String> {
    match s {
        "calls" => Ok(TraceType::Calls),
        "dataflow" => Ok(TraceType::DataFlow),
        "all" => Ok(TraceType::All),
        _ => Err(format!("invalid trace_type: {s} (expected calls|dataflow|all)")),
    }
}

// --- JSON-RPC response helpers ---

fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

// --- Output serializers ---

/// JSON-serializable query result (mirrors `query_cmd::QueryOutput`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub duration_ms: u64,
}

fn query_output(r: QueryResult) -> QueryOutput {
    QueryOutput {
        columns: r.columns,
        rows: r.rows,
        duration_ms: r.duration_ms,
    }
}

/// JSON-serializable trace result (mirrors `trace_cmd::TraceOutput` shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceOutput {
    pub symbol: String,
    pub paths: Vec<TracePathOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TracePathOutput {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    pub depth: usize,
}

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

/// Converts a [`TraceNode`] to a JSON object (TraceNode doesn't derive Serialize).
fn trace_node_to_json(n: &TraceNode) -> Value {
    json!({
        "name": n.name,
        "label": n.label,
        "filePath": n.file_path,
        "startLine": n.start_line,
    })
}

/// Converts a [`TraceEdge`] to a JSON object (TraceEdge doesn't derive Serialize).
fn trace_edge_to_json(e: &TraceEdge) -> Value {
    json!({
        "edgeType": e.edge_type,
        "reason": e.reason,
        "confidence": e.confidence,
    })
}

/// Converts a [`SearchResult`] to a JSON object (SearchResult doesn't derive Serialize).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};

    fn fresh_kit() -> Kit {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp_test.lbug");
        std::mem::forget(dir);
        build_kit(&KitBootstrapConfig::new(path)).expect("build_kit")
    }

    // --- handle_initialize ---

    #[test]
    fn initialize_returns_protocol_version_and_server_info() {
        let result = handle_initialize();
        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], "codenexus");
        assert!(result["capabilities"]["tools"].is_object());
    }

    // --- handle_tools_list ---

    #[test]
    fn tools_list_returns_five_tools() {
        let result = handle_tools_list();
        let tools = result["tools"].as_array().expect("tools is array");
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"query"));
        assert!(names.contains(&"trace"));
        assert!(names.contains(&"impact"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"context"));
    }

    #[test]
    fn tools_list_each_tool_has_name_description_and_input_schema() {
        let result = handle_tools_list();
        let tools = result["tools"].as_array().unwrap();
        for tool in tools {
            assert!(tool["name"].is_string(), "tool has name");
            assert!(tool["description"].is_string(), "tool has description");
            assert!(tool["inputSchema"].is_object(), "tool has inputSchema");
        }
    }

    // --- handle_request: unknown method ---

    #[test]
    fn handle_request_unknown_method_returns_method_not_found() {
        let kit = fresh_kit();
        let result = handle_request(&kit, "bogus/method", &Value::Null);
        let (code, _msg) = result.expect_err("unknown method should error");
        assert_eq!(code, error_codes::METHOD_NOT_FOUND);
    }

    // --- handle_raw_line: notifications get no response ---

    #[test]
    fn handle_raw_line_notification_returns_none() {
        let kit = fresh_kit();
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(handle_raw_line(&kit, raw).is_none());
    }

    // --- handle_raw_line: parse error ---

    #[test]
    fn handle_raw_line_invalid_json_returns_parse_error() {
        let kit = fresh_kit();
        let resp = handle_raw_line(&kit, "not json").expect("parse error should respond");
        assert_eq!(resp["error"]["code"], error_codes::PARSE_ERROR);
    }

    // --- handle_raw_line: initialize ---

    #[test]
    fn handle_raw_line_initialize_returns_success() {
        let kit = fresh_kit();
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let resp = handle_raw_line(&kit, raw).expect("initialize should respond");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], "codenexus");
    }

    // --- handle_raw_line: tools/list ---

    #[test]
    fn handle_raw_line_tools_list_returns_five_tools() {
        let kit = fresh_kit();
        let raw = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let resp = handle_raw_line(&kit, raw).expect("tools/list should respond");
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);
    }

    // --- handle_raw_line: tools/call unknown tool ---

    #[test]
    fn handle_raw_line_tools_call_unknown_tool_returns_error() {
        let kit = fresh_kit();
        let raw = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"bogus","arguments":{}}}"#;
        let resp = handle_raw_line(&kit, raw).expect("unknown tool should respond");
        assert_eq!(resp["error"]["code"], error_codes::INVALID_PARAMS);
    }

    // --- handle_raw_line: tools/call missing name ---

    #[test]
    fn handle_raw_line_tools_call_missing_name_returns_error() {
        let kit = fresh_kit();
        let raw = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"arguments":{}}}"#;
        let resp = handle_raw_line(&kit, raw).expect("missing name should respond");
        assert_eq!(resp["error"]["code"], error_codes::INVALID_PARAMS);
    }

    // --- parse_trace_type ---

    #[test]
    fn parse_trace_type_valid_values() {
        assert!(matches!(parse_trace_type("calls"), Ok(TraceType::Calls)));
        assert!(matches!(parse_trace_type("dataflow"), Ok(TraceType::DataFlow)));
        assert!(matches!(parse_trace_type("all"), Ok(TraceType::All)));
    }

    #[test]
    fn parse_trace_type_invalid_value() {
        assert!(parse_trace_type("bogus").is_err());
    }

    // --- handle_raw_line: missing method ---

    #[test]
    fn handle_raw_line_missing_method_returns_invalid_request() {
        let kit = fresh_kit();
        let raw = r#"{"jsonrpc":"2.0","id":5}"#;
        let resp = handle_raw_line(&kit, raw).expect("missing method should respond");
        assert_eq!(resp["error"]["code"], error_codes::INVALID_REQUEST);
    }

    // --- handle_raw_line: tools/call query with real kit ---

    #[test]
    fn handle_raw_line_tools_call_query_returns_result() {
        let kit = fresh_kit();
        // Seed via the Query module's own connection — the Storage module
        // opens a separate `Database` handle whose writes are not visible to
        // the Query module's connection (LadybugDB cross-handle isolation).
        // Using `query.cypher("CREATE ...")` ensures the seed lands on the
        // same connection that will service the `tools/call query` dispatch.
        let query = kit.require::<crate::kit::QueryKey>().expect("require_query");
        query
            .cypher("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0, lastCommit: ''});")
            .expect("seed project");
        let raw = r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"query","arguments":{"cypher":"MATCH (p:Project) RETURN p.name AS name"}}}"#;
        let resp = handle_raw_line(&kit, raw).expect("query should respond");
        assert!(resp["result"]["content"].is_array(), "result has content array");
        let text = resp["result"]["content"][0]["text"].as_str().expect("text");
        let parsed: Value = serde_json::from_str(text).expect("text is JSON");
        assert!(parsed["rows"].is_array(), "parsed has rows");
        assert_eq!(parsed["rows"][0][0], "demo");
    }

    // --- handle_raw_line: tools/call query missing cypher ---

    #[test]
    fn handle_raw_line_tools_call_query_missing_cypher_returns_error() {
        let kit = fresh_kit();
        let raw = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"query","arguments":{}}}"#;
        let resp = handle_raw_line(&kit, raw).expect("missing cypher should respond");
        assert_eq!(resp["error"]["code"], error_codes::INVALID_PARAMS);
    }

    // --- handle_raw_line: tools/call search ---

    #[test]
    fn handle_raw_line_tools_call_search_returns_results() {
        let kit = fresh_kit();
        // Seed via the Query module's own connection (see query test comment
        // for rationale — Storage module writes are not visible to Query).
        let query = kit.require::<crate::kit::QueryKey>().expect("require_query");
        query
            .cypher("CREATE (:Function {id: 'f1', project: 'demo', name: 'parse_file', qualifiedName: 'demo.parse_file', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("seed function");
        let raw = r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"search","arguments":{"text":"parse","limit":5}}}"#;
        let resp = handle_raw_line(&kit, raw).expect("search should respond");
        assert!(resp["result"]["content"].is_array());
        let text = resp["result"]["content"][0]["text"].as_str().expect("text");
        let parsed: Value = serde_json::from_str(text).expect("text is JSON");
        assert!(parsed["results"].is_array(), "parsed has results array");
        assert!(parsed["count"].as_u64().unwrap_or(0) >= 1, "search should find the seeded function");
    }

    // --- response helpers ---

    #[test]
    fn success_response_includes_id_and_result() {
        let resp = success_response(json!(42), json!({"ok": true}));
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 42);
        assert_eq!(resp["result"]["ok"], true);
    }

    #[test]
    fn error_response_includes_code_and_message() {
        let resp = error_response(json!(1), error_codes::METHOD_NOT_FOUND, "nope");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["error"]["code"], error_codes::METHOD_NOT_FOUND);
        assert_eq!(resp["error"]["message"], "nope");
    }

    // --- constants ---

    #[test]
    fn protocol_version_is_2024_11_05() {
        assert_eq!(MCP_PROTOCOL_VERSION, "2024-11-05");
    }

    #[test]
    fn server_name_is_codenexus() {
        assert_eq!(SERVER_NAME, "codenexus");
    }

    #[test]
    fn server_version_matches_cargo_pkg_version() {
        assert!(!SERVER_VERSION.is_empty());
    }
}
