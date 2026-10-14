// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! `codenexus webui` 二进制端到端测试：起临时索引库、spawn 服务、按
//! token/Host 矩阵请求 HTTP 端点后收尾。
#![cfg(all(feature = "cli", feature = "webui"))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use codenexus::model::{Edge, EdgeType, Language, Node, NodeLabel};
use codenexus::storage::Repository;

/// 起一个含 2 个 Function + 1 条 CALLS 边的临时项目目录（`.codenexus/demo.lbug`）。
fn fixture_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_dir = dir.path().join(".codenexus");
    std::fs::create_dir_all(&db_dir).expect("mkdir .codenexus");
    let db_path = db_dir.join("demo.lbug");
    let repo = Repository::open(&db_path).expect("open");
    let nodes: Vec<Node> = ["alpha", "beta"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            Node::builder(NodeLabel::Function, *name, format!("demo.{name}"))
                .id(&format!("f{i}"))
                .project("demo")
                .file_path("/src/main.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .signature("fn main()")
                .build()
        })
        .collect();
    repo.save_nodes_stream(&nodes, NodeLabel::Function)
        .expect("save_nodes");
    let edge = Edge::new("demo.alpha", "demo.beta", EdgeType::Calls, "demo");
    repo.save_edges_stream(&[edge]).expect("save_edges");
    drop(repo);
    (dir, db_path)
}

/// 取一个空闲回环端口（bind 后立即释放；与 --port 0=默认端口的约定区分）。
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
    listener.local_addr().expect("addr").port()
}

/// 保证测试 panic 时也 kill + reap 子进程（对齐 tests/mcp_integration.rs 惯例）。
struct WebuiChild(Child);

impl Drop for WebuiChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// spawn `codenexus webui --port <p>`，等待 token 行并返回（子进程守卫, token）。
fn spawn_webui(dir: &std::path::Path, port: u16) -> (WebuiChild, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codenexus"))
        .args(["webui", "--port", &port.to_string()])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn codenexus webui");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let token_line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("token line on stdout");
    let token = token_line
        .strip_prefix("graph-server auth token: ")
        .expect("token line prefix")
        .trim()
        .to_string();
    (WebuiChild(child), token)
}

/// 原始 HTTP/1.1 GET（Connection: close），返回（状态码, 完整响应文本）。
fn http_get(port: u16, path: &str, host: &str, token: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect webui");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(t) = token {
        req.push_str(&format!("x-graph-token: {t}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).expect("write request");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read response");
    let status = resp
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, resp)
}

#[test]
fn webui_serves_static_api_and_enforces_auth() {
    let (dir, db_path) = fixture_dir();
    let port = free_port();
    let (_child, token) = spawn_webui(dir.path(), port);

    // 等待端口就绪（token 行先于监听打印，连接重试最多 ~5s）
    let mut ready = false;
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "webui 未在 5s 内监听 127.0.0.1:{port}");

    // 静态资源：无需 token，200 + HTML
    let (status, body) = http_get(port, "/", "127.0.0.1", None);
    assert_eq!(status, 200, "GET / 应为 200，响应：{body}");
    assert!(
        body.to_lowercase().contains("<html") || body.contains("CodeNexus"),
        "GET / 应返回 HTML 页面"
    );

    // 鉴权矩阵
    let (status, _) = http_get(port, "/api/projects", "127.0.0.1", None);
    assert_eq!(status, 401, "无 token → 401");
    let (status, _) = http_get(port, "/api/projects", "127.0.0.1", Some("wrong"));
    assert_eq!(status, 401, "错误 token → 401");
    let (status, _) = http_get(port, "/api/projects", "evil.example.com", Some(&token));
    assert_eq!(status, 403, "非回环 Host → 403");

    // 合法请求：project 列出 fixture 库
    let (status, body) = http_get(port, "/api/projects", "127.0.0.1", Some(&token));
    assert_eq!(status, 200, "GET /api/projects 应为 200，响应：{body}");
    assert!(body.contains("\"demo\""), "应列出 demo 项目：{body}");
    assert!(body.contains("\"node_count\""), "字段名与旧 server 一致");

    // schema：计数与 fixture 相符（2 Function + SchemaMeta 单行表 / 1 条边）
    let (status, body) = http_get(port, "/api/schema?project=demo", "127.0.0.1", Some(&token));
    assert_eq!(status, 200, "GET /api/schema 应为 200，响应：{body}");
    assert!(body.contains("\"total_edges\":1"), "schema 边计数：{body}");
    assert!(
        body.contains("\"label\":\"Function\""),
        "schema 标签计数：{body}"
    );

    // trace：逐跳展开命中 beta
    let (status, body) = http_get(
        port,
        "/api/trace?project=demo&node_id=alpha&mode=call&direction=downstream&max_depth=3",
        "127.0.0.1",
        Some(&token),
    );
    assert_eq!(status, 200, "GET /api/trace 应为 200，响应：{body}");
    assert!(body.contains("\"origin\""), "trace 响应形状：{body}");
    assert!(body.contains("\"beta\""), "trace 应命中 beta：{body}");

    // webui 不构建 Kit：不得产生 general.lbug 之类空库（仅 demo.lbug + logs）
    let entries: Vec<String> = std::fs::read_dir(dir.path().join(".codenexus"))
        .expect("read .codenexus")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(entries.contains(&"demo.lbug".to_string()), "fixture 库存在");
    assert!(
        !entries
            .iter()
            .any(|n| n.ends_with(".lbug") && n != "demo.lbug"),
        "webui 不应创建额外空库：{entries:?}"
    );
    assert!(db_path.exists());
}
