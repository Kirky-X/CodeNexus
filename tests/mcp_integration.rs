// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! MCP integration test — verifies the sdforge-based MCP server boots,
//! responds to `initialize`, and exposes all 5 tools via `tools/list`.
//!
//! This test spawns `codenexus mcp` as a subprocess, communicates via
//! stdin/stdout (line-delimited JSON-RPC 2.0 per MCP stdio transport),
//! and validates the protocol handshake + tool discovery.

#![cfg(feature = "mcp")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

/// MCP subprocess client — owns the child process and a reader thread that
/// pumps stdout lines into a channel for timeout-safe receive.
struct McpClient {
    child: std::process::Child,
    receiver: mpsc::Receiver<Value>,
}

impl McpClient {
    /// Spawns `codenexus mcp --db <db_path>` and starts a background reader
    /// thread that parses each stdout line as JSON and sends it through a
    /// channel.
    fn spawn(db_path: &str) -> Self {
        let binary = env!("CARGO_BIN_EXE_codenexus");
        let mut child = Command::new(binary)
            .arg("mcp")
            .arg("--db")
            .arg(db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn codenexus mcp: {e}"));

        // Take stdout once — the reader thread owns it for the lifetime of
        // the client. This avoids the bug where `child.stdout.take()` on each
        // call removes stdout after the first request.
        let stdout = child.stdout.take().expect("stdout pipe");
        let (tx, rx) = mpsc::channel::<Value>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break, // EOF — server closed stdout
                    Ok(_) => {
                        // Only accept JSON objects — JSON-RPC messages are
                        // always objects. LadybugDB may print schema DDL as
                        // bare JSON strings to stdout during init, which would
                        // parse as Value::String and pollute the channel.
                        if let Ok(parsed) = serde_json::from_str::<Value>(&line) {
                            if parsed.is_object() {
                                if tx.send(parsed).is_err() {
                                    break; // receiver dropped — test is done
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            child,
            receiver: rx,
        }
    }

    /// Sends a JSON-RPC request and reads one response with a 5s timeout.
    fn send_rpc(&mut self, request: &Value) -> Value {
        let json_str = serde_json::to_string(request).expect("serialize request");
        {
            let stdin = self.child.stdin.as_mut().expect("stdin pipe");
            stdin
                .write_all(format!("{json_str}\n").as_bytes())
                .expect("write to stdin");
            stdin.flush().expect("flush stdin");
        }
        match self.receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(response) => response,
            Err(_) => panic!("MCP server did not respond within 5s"),
        }
    }

    /// Sends a JSON-RPC notification (no `id` → no response expected).
    fn send_notification(&mut self, method: &str) {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": {}
        });
        let json_str = serde_json::to_string(&notification).expect("serialize notification");
        let stdin = self.child.stdin.as_mut().expect("stdin pipe");
        stdin
            .write_all(format!("{json_str}\n").as_bytes())
            .expect("write notification");
        stdin.flush().expect("flush stdin");
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_server_initializes_and_lists_tools() {
    let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
    let db_path = tmp.path().to_str().expect("db path to str");

    let mut client = McpClient::spawn(db_path);

    // 1. Send initialize request.
    let init_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "integration-test",
                "version": "0.1.0"
            }
        }
    });
    let init_response = client.send_rpc(&init_request);

    // Assert the server responded with a non-empty serverInfo.name.
    //
    // sdforge derives serverInfo.name from the first registered tool's
    // tool_name (via McpToolRegistration.name), NOT from the #[service_api(name =
    // "...")] parameter. Since inventory iteration order is non-deterministic,
    // the server name will be one of: "query", "trace", "impact", "search",
    // "context" — whichever tool happens to be first in the inventory. We can't
    // control this without forking sdforge, so we just assert non-empty.
    let server_name = init_response
        .get("result")
        .and_then(|r| r.get("serverInfo"))
        .and_then(|si| si.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or_else(|| {
            panic!(
                "initialize response missing serverInfo.name: {init_response}"
            )
        });
    assert!(
        !server_name.is_empty(),
        "serverInfo.name should be non-empty, got: '{server_name}'"
    );

    // 2. Send notifications/initialized (required by MCP protocol before
    //    calling other methods).
    client.send_notification("notifications/initialized");

    // 3. Send tools/list request.
    let tools_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let tools_response = client.send_rpc(&tools_request);

    // Assert 5 tools are returned.
    let tools = tools_response
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .unwrap_or_else(|| {
            panic!(
                "tools/list response missing result.tools array: {tools_response}"
            )
        });

    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();

    let expected = ["query", "trace", "impact", "search", "context"];
    for name in &expected {
        assert!(
            tool_names.contains(name),
            "tools/list should contain '{name}', got: {tool_names:?}"
        );
    }
    assert_eq!(
        tool_names.len(),
        expected.len(),
        "should have exactly {} tools, got {}: {:?}",
        expected.len(),
        tool_names.len(),
        tool_names
    );

    client.kill();
}
