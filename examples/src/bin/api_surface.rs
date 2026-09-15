// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! API/Web 服务面分析：路由表 / schema 校验 / API 影响 / 跨服务调用 / MCP 工具表。
//!
//! 覆盖 CLI：`route_map` / `shape_check` / `api_impact` / `cross_service` / `tool_map`。
//!
//! 路由识别规则：axum 风格 `Router::new().route("/path", get(handler))`，
//! handler 必须是裸标识符。跨服务检测：调用方字符串字面量与路由模式匹配。

use codenexus::analysis::api_review::ApiReviewer;
use codenexus::analysis::cross_service::{CrossServiceDetector, CrossServiceLinker};
use codenexus::storage::Repository;
use codenexus_examples::{index_sample_code, setup};

const SAMPLE_CODE: &str = r#"
pub fn list_users() -> Vec<&'static str> {
    vec!["alice", "bob"]
}

pub fn create_user(name: &str) -> bool {
    !name.is_empty()
}

pub fn health() -> &'static str {
    "ok"
}

pub fn build_router() {
    let _routes = Router::new()
        .route("/users", get(list_users))
        .route("/users", post(create_user))
        .route("/health", get(health));
}

// Another service's client hits the /users route — the string literal
// at the call site is what the cross-service detector matches.
pub fn reporting_client() -> String {
    http_get("/users")
}

fn http_get(path: &str) -> String {
    format!("GET {path}")
}
"#;

fn main() {
    println!("=== CodeNexus Example: API Surface (route_map/shape_check/api_impact/cross_service/tool_map) ===\n");

    let ctx = setup(SAMPLE_CODE);
    let result = index_sample_code(&ctx, "web-demo");
    let repo = Repository::open(&ctx.db_path).expect("Repository::open");
    let reviewer = ApiReviewer::new(&repo);

    // --- 1. Route map (CLI: `route_map`) --------------------------------
    let routes = reviewer.route_map(&result.project_id).expect("route_map");
    println!("=== Routes ({} found) ===", routes.len());
    for r in &routes {
        println!(
            "  {:<6} {:<12} handler={}",
            r.method, r.path, r.handler_name
        );
    }
    println!();

    // --- 2. Schema consistency (CLI: `shape_check`) ----------------------
    let violations = reviewer
        .shape_check(&result.project_id)
        .expect("shape_check");
    println!("=== Shape Violations ({} found) ===", violations.len());
    for v in &violations {
        println!(
            "  {:<12} expected={} actual={}",
            v.endpoint, v.expected_schema, v.actual_schema
        );
    }
    println!();

    // --- 3. API impact (CLI: `api_impact`) -------------------------------
    let impact = reviewer
        .api_impact(&result.project_id, "/users")
        .expect("api_impact");
    println!(
        "=== Impact of changing /users ({} callers) ===",
        impact.len()
    );
    for e in &impact {
        println!(
            "  {} @ {}:{}",
            e.affected_caller, e.caller_file, e.caller_line
        );
    }
    println!();

    // --- 4. Cross-service links (CLI: `cross_service`) -------------------
    // `link()` writes CROSS_SERVICE_CALLS edges; `detect_all` only reports.
    let links = CrossServiceLinker::new(&repo, &result.project_id)
        .link()
        .expect("cross_service link");
    println!("=== Cross-Service Links ({} written) ===", links.len());
    for l in &links {
        println!(
            "  {} -> caller {} (line {})",
            l.route_pattern, l.caller_id, l.caller_line
        );
    }

    let matches = CrossServiceDetector::new(&repo)
        .detect_all(&result.project_id)
        .expect("cross_service detect");
    println!("=== Cross-Service Matches ({} found) ===", matches.len());
    for m in &matches {
        println!(
            "  {:?} caller={} callee={} confidence={:?}",
            m.match_type, m.caller, m.callee, m.confidence
        );
    }
    println!();

    // --- 5. MCP tool map (CLI: `tool_map`) -------------------------------
    // Requires indexed Tool/Handler nodes (produced by indexing MCP-style
    // services) — a plain code sample typically yields an empty list.
    let tools = reviewer.tool_map(&result.project_id).expect("tool_map");
    println!("=== MCP Tools ({} found) ===", tools.len());
    for t in &tools {
        println!("  {:<20} handler={}", t.tool_name, t.handler_name);
    }
}

// NOTE on cross-service results: `detect_all` scans Function.content for
// string literals. The current Rust extractor does not persist function
// bodies into `content` yet, so freshly indexed code yields 0 matches here —
// the API surface above is exactly what integrators should call once body
// storage lands.
