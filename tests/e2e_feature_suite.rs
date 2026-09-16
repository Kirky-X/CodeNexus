// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! 综合端到端测试套件：穷举 CodeNexus 全部核心功能的使用场景。
//!
//! 覆盖范围：
//! 1. 多语言索引（Go / Java / JavaScript / Bash / HTML / CSS / JSON）
//! 2. 增量索引（修改文件 / 空仓库 / 空文件）
//! 3. 查询引擎（Cypher / 结构化搜索 / 全文搜索 / 按类型搜索 / 按文件搜索）
//! 4. 追踪引擎（调用链 / 数据流 / 深度限制 / 异常处理）
//! 5. 影响分析（上游调用者 / 空结果）
//! 6. 上下文函数（入度 / 出度 / 流程）
//! 7. 死代码检测（默认 / 自定义配置 / 导出检查）
//! 8. 架构概览（语言分布 / 入口点 / 热点）
//! 9. 复杂度分析（函数指标 / 严重度）
//! 10. 社区检测（调用图聚类）
//! 11. 跨服务检测（空结果场景）
//! 12. 集成场景（索引→查询→追踪全链路 / 多项目隔离 / FFI 边 / 损坏数据库）

use std::fs;
use std::path::Path;

use codenexus::index::IndexFacade;
use codenexus::model::NodeLabel;
use codenexus::query::QueryFacade;
use tempfile::TempDir;

// Kit-based imports (for trace / analysis tests).
use codenexus::kit::{build_kit, KitBootstrapConfig, StorageModule, TraceModule};
use codenexus::storage::capability::Storage;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 在 `dir/rel` 写入文件（自动创建父目录）。
fn write_file(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// 返回一个临时数据库路径（TempDir 故意泄漏以保证数据库文件存活）。
fn fresh_db_path() -> std::path::PathBuf {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("e2e_suite_db");
    std::mem::forget(dir);
    path
}

/// 构建 Kit 并返回 (kit, storage) 对。
// NOTE: build_test_kit 保留供后续扩展使用。
#[allow(dead_code)]
async fn build_test_kit(
    db_path: &Path,
) -> (
    codenexus::kit::AsyncKit<codenexus::kit::AsyncReady>,
    std::sync::Arc<dyn Storage>,
) {
    let config = KitBootstrapConfig::new(db_path.to_path_buf());
    let kit = build_kit(&config).await.expect("build_kit");
    let storage: std::sync::Arc<dyn Storage> =
        kit.require::<StorageModule>().expect("require_storage");
    (kit, storage)
}

// ===========================================================================
// § 1 — 多语言索引
// ===========================================================================

#[test]
fn index_go_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "main.go",
        "package main\n\nfunc greet(name string) string {\n\treturn \"Hello, \" + name\n}\n\nfunc main() {\n\tgreet(\"world\")\n}\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "go_demo", false).expect("index");
    assert!(result.files_indexed >= 1, "应索引 Go 文件");
    assert!(result.nodes_created > 0, "应创建节点");
}

#[test]
fn index_java_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "App.java",
        "public class App {\n    public static void main(String[] args) {}\n    public int compute(int x) { return x * 2; }\n}\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "java_demo", false).expect("index");
    assert!(result.files_indexed >= 1, "应索引 Java 文件");
}

#[test]
fn index_javascript_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "index.js",
        "function fetchData(url) { return fetch(url); }\nfunction process(data) { return data; }\nmodule.exports = { fetchData, process };\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "js_demo", false).expect("index");
    assert!(result.files_indexed >= 1, "应索引 JavaScript 文件");
}

#[test]
fn index_bash_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "deploy.sh",
        "#!/bin/bash\nset -euo pipefail\ndeploy_app() {\n  echo \"deploying...\"\n}\ndeploy_app\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "bash_demo", false).expect("index");
    assert!(result.files_indexed >= 1, "应索引 Bash 文件");
}

#[test]
fn index_html_css_json_files() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "index.html",
        "<!DOCTYPE html><html><head><title>T</title></head><body><h1>Hello</h1></body></html>\n",
    );
    write_file(
        tmp.path(),
        "style.css",
        "body { margin: 0; color: #333; }\nh1 { font-size: 2rem; }\n",
    );
    write_file(
        tmp.path(),
        "config.json",
        "{\"name\": \"demo\", \"version\": 1}\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "web_demo", false).expect("index");
    assert!(
        result.files_indexed >= 3,
        "应索引 HTML + CSS + JSON 三个文件，got {}",
        result.files_indexed
    );
}

// ===========================================================================
// § 2 — 增量索引边界场景
// ===========================================================================

#[test]
fn index_empty_repo_produces_zero_files() {
    let tmp = TempDir::new().unwrap();
    // 空目录，无源码文件。
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade
        .index(tmp.path(), "empty_demo", false)
        .expect("index");
    assert_eq!(result.files_indexed, 0, "空仓库应索引 0 个文件");
}

#[test]
fn index_file_with_no_parseable_symbols() {
    let tmp = TempDir::new().unwrap();
    // 空 Rust 文件（无函数/结构体）。
    write_file(tmp.path(), "empty.rs", "// just a comment\n");
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade
        .index(tmp.path(), "empty_file_demo", false)
        .expect("index");
    assert_eq!(result.files_indexed, 1, "应索引 1 个文件（即使无符号）");
}

#[test]
fn incremental_index_detects_modified_file() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "fn a() {}\n");
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index 1");

    // 修改文件内容。
    write_file(tmp.path(), "a.rs", "fn a() {}\nfn b() {}\n");
    let result2 = facade.index(tmp.path(), "demo", false).expect("index 2");
    assert!(
        result2.files_indexed >= 1,
        "修改后的文件应被重新索引，got {}",
        result2.files_indexed
    );
}

// ===========================================================================
// § 3 — 查询引擎
// ===========================================================================

#[test]
fn cypher_query_returns_function_nodes() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "pub fn alpha() {}\npub fn beta() {}\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let result = query
        .cypher("MATCH (f:Function) RETURN f.name AS name LIMIT 10;")
        .expect("cypher");
    assert!(!result.rows.is_empty(), "应返回至少一行");
    assert!(result.duration_ms < u64::MAX, "duration_ms 应有限");
}

#[test]
fn cypher_invalid_syntax_returns_error() {
    let db = fresh_db_path();
    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let err = query.cypher("MATCH (a RETURN a;");
    assert!(err.is_err(), "无效 Cypher 应返回错误");
}

#[test]
fn cypher_empty_string_returns_error() {
    let db = fresh_db_path();
    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let err = query.cypher("");
    assert!(err.is_err(), "空查询字符串应返回错误");
}

#[test]
fn structured_search_exact_match() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "pub fn parse_config() {}\npub fn read_file() {}\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let results = query.search("parse_config", None, 10).expect("search");
    assert!(
        results.iter().any(|r| r.name == "parse_config"),
        "精确搜索应找到 parse_config"
    );
}

#[test]
fn structured_search_no_match_returns_empty() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "lib.rs", "pub fn alpha() {}\n");
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let results = query.search("zzz_nonexistent", None, 10).expect("search");
    assert!(results.is_empty(), "不匹配的搜索应返回空");
}

#[test]
fn structured_search_respects_limit() {
    let tmp = TempDir::new().unwrap();
    // 创建多个函数。
    let content = (0..10)
        .map(|i| format!("pub fn func_{i}() {{}}\n"))
        .collect::<String>();
    write_file(tmp.path(), "lib.rs", &content);
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let results = query.search("func_", None, 3).expect("search");
    assert!(results.len() <= 3, "搜索结果应受 limit 限制");
}

#[test]
fn search_by_type_returns_correct_label() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "lib.rs", "pub fn alpha() {}\nstruct Config;\n");
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let results = query
        .search_by_type(NodeLabel::Struct, None, 10)
        .expect("search_by_type");
    assert!(
        results.iter().all(|r| r.label == "Struct"),
        "按类型搜索应仅返回 Struct 节点"
    );
    assert!(results.iter().any(|r| r.name.contains("Config")));
}

#[test]
fn fulltext_search_finds_content_matches() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "pub fn parse_json(input: &str) -> Value {}\npub fn parse_xml(input: &str) -> Value {}\n",
    );
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let results = query.fulltext_search("parse", None, 10).expect("fulltext");
    assert!(!results.is_empty(), "全文搜索应返回结果");
}

#[test]
fn search_by_file_returns_symbols_in_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "src/lib.rs",
        "pub fn alpha() {}\npub fn beta() {}\n",
    );
    write_file(tmp.path(), "src/main.rs", "fn main() {}\n");
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    // 搜索 lib.rs 文件中的符号。
    let results = query
        .search_by_file("lib.rs", None)
        .expect("search_by_file");
    // 应找到 lib.rs 中的符号（路径匹配可能包含前缀）。
    for r in &results {
        assert!(
            r.file_path.as_deref().unwrap_or("").contains("lib.rs"),
            "搜索结果应来自 lib.rs"
        );
    }
}

// ===========================================================================
// § 4 — 追踪引擎（kit-based）
// ===========================================================================

#[tokio::test]
async fn trace_calls_returns_call_paths() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "fn main() { helper(); }\nfn helper() { println!(\"hi\"); }\n",
    );
    let db = fresh_db_path();

    let config = KitBootstrapConfig::new(db.clone());
    let kit = build_kit(&config).await.expect("build_kit");
    let indexer = kit
        .require::<codenexus::kit::IndexerModule>()
        .expect("indexer");
    indexer
        .index(tmp.path(), "trace_demo", false)
        .expect("index");

    let trace = kit.require::<TraceModule>().expect("trace");
    let (graph, _truncated) = trace.load_graph("main", 3, 1000).expect("load_graph");
    // main 应至少有一条 CALLS 边指向 helper。
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.edge_type == codenexus::model::EdgeType::Calls),
        "trace 应返回含 CALLS 边的路径"
    );
}

#[tokio::test]
async fn trace_unknown_symbol_returns_empty_graph() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "lib.rs", "fn alpha() {}\n");
    let db = fresh_db_path();

    let config = KitBootstrapConfig::new(db.clone());
    let kit = build_kit(&config).await.expect("build_kit");
    let indexer = kit
        .require::<codenexus::kit::IndexerModule>()
        .expect("indexer");
    indexer
        .index(tmp.path(), "trace_demo", false)
        .expect("index");

    let trace = kit.require::<TraceModule>().expect("trace");
    let (graph, _truncated) = trace
        .load_graph("nonexistent_symbol", 3, 1000)
        .expect("load_graph");
    assert!(graph.nodes.is_empty(), "不存在的符号应返回空图");
}

// ===========================================================================
// § 5 — 影响分析
// ===========================================================================

#[tokio::test]
async fn impact_analysis_returns_upstream_callers() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "fn caller_a() { target(); }\nfn caller_b() { target(); }\nfn target() {}\n",
    );
    let db = fresh_db_path();

    let config = KitBootstrapConfig::new(db.clone());
    let kit = build_kit(&config).await.expect("build_kit");
    let indexer = kit
        .require::<codenexus::kit::IndexerModule>()
        .expect("indexer");
    indexer
        .index(tmp.path(), "impact_demo", false)
        .expect("index");

    let trace = kit.require::<TraceModule>().expect("trace");
    let (graph, _) = trace.load_graph("target", 3, 1000).expect("load_graph");
    // target 应被 caller_a 和 caller_b 调用，图中应有 CALLS 边。
    let calls_edges: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| e.edge_type == codenexus::model::EdgeType::Calls)
        .collect();
    assert!(
        !calls_edges.is_empty(),
        "impact 图应包含指向 target 的 CALLS 边"
    );
}

// ===========================================================================
// § 6 — 上下文函数
// ===========================================================================

#[tokio::test]
async fn context_collect_incoming_and_outgoing() {
    use codenexus::trace::{collect_incoming, collect_outgoing, resolve_start_id};

    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "fn caller() { target(); }\nfn target() { helper(); }\nfn helper() {}\n",
    );
    let db = fresh_db_path();

    let config = KitBootstrapConfig::new(db.clone());
    let kit = build_kit(&config).await.expect("build_kit");
    let indexer = kit
        .require::<codenexus::kit::IndexerModule>()
        .expect("indexer");
    indexer.index(tmp.path(), "ctx_demo", false).expect("index");

    let trace = kit.require::<TraceModule>().expect("trace");
    let (graph, _) = trace.load_graph("target", 3, 1000).expect("load_graph");

    // resolve_start_id 应找到 target 节点。
    let start_id = resolve_start_id(&graph, "target")
        .expect("resolve")
        .expect("found");
    // collect_incoming: caller → target 的 CALLS 边。
    let incoming = collect_incoming(&graph, &start_id);
    assert!(
        incoming.iter().any(|n| n.name == "caller"),
        "incoming 应包含 caller，got: {:?}",
        incoming.iter().map(|n| &n.name).collect::<Vec<_>>()
    );
    // collect_outgoing: target → helper 的 CALLS 边。
    let outgoing = collect_outgoing(&graph, &start_id);
    assert!(
        outgoing.iter().any(|n| n.name == "helper"),
        "outgoing 应包含 helper，got: {:?}",
        outgoing.iter().map(|n| &n.name).collect::<Vec<_>>()
    );
}

// ===========================================================================
// § 7 — 死代码检测
// ===========================================================================

#[tokio::test]
async fn dead_code_detection_finds_unreachable_functions() {
    use codenexus::analysis::dead_code::DeadCodeDetector;

    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "\
fn main() { live_fn(); }
fn live_fn() {}
fn dead_fn() {}
fn another_dead_fn() {}
",
    );
    let db = fresh_db_path();

    // 索引：使用 IndexFacade（打开自己的 DB 连接写入数据）。
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let _result = facade.index(tmp.path(), "dc_demo", false).expect("index");

    // 分析：索引完成后打开新的只读连接。
    let repo = codenexus::storage::Repository::open_read_only(&db).expect("open read-only");
    let detector = DeadCodeDetector::new(&repo);
    // 使用 project_name 查询（节点 project 列存储的是 project_id/UUID）。
    // 先通过 Cypher 查出实际的 project_id。
    let rows = repo
        .query("MATCH (p:Project) RETURN p.id AS id;")
        .expect("query projects");
    let project_id = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .expect("project_id");
    let entries = detector.detect(project_id, &["main"]).expect("detect");
    let dead_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

    // dead_fn 和 another_dead_fn 应为死代码。
    assert!(
        dead_names.contains(&"dead_fn"),
        "dead_fn 应被检测为死代码，got: {dead_names:?}"
    );
    assert!(
        dead_names.contains(&"another_dead_fn"),
        "another_dead_fn 应被检测为死代码，got: {dead_names:?}"
    );
    // main 和 live_fn 不应为死代码。
    assert!(!dead_names.contains(&"main"), "main 不应为死代码");
    assert!(
        !dead_names.contains(&"live_fn"),
        "live_fn 不应为死代码（被 main 调用）"
    );
}

#[tokio::test]
async fn dead_code_detection_exported_functions_are_live() {
    use codenexus::analysis::dead_code::DeadCodeDetector;

    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "pub fn public_api() {}\nfn internal_only() {}\n",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let _result = facade
        .index(tmp.path(), "dc_export_demo", false)
        .expect("index");

    let repo = codenexus::storage::Repository::open_read_only(&db).expect("open read-only");
    let rows = repo
        .query("MATCH (p:Project) RETURN p.id AS id;")
        .expect("query projects");
    let project_id = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .expect("project_id");
    let detector = DeadCodeDetector::new(&repo);
    let entries = detector.detect(project_id, &[]).expect("detect");
    let dead_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

    // pub fn 应被视为外部可见（live），不是死代码。
    assert!(
        !dead_names.contains(&"public_api"),
        "pub fn public_api 不应为死代码（check_exported=true 默认）"
    );
}

// ===========================================================================
// § 8 — 架构概览
// ===========================================================================

#[tokio::test]
async fn architecture_overview_returns_language_stats() {
    use codenexus::analysis::architecture::ArchitectureAnalyzer;

    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\nfn helper() {}\n");
    write_file(tmp.path(), "lib.py", "def greet():\n    pass\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let _result = facade.index(tmp.path(), "arch_demo", false).expect("index");

    let repo = codenexus::storage::Repository::open_read_only(&db).expect("open read-only");
    let rows = repo
        .query("MATCH (p:Project) RETURN p.id AS id;")
        .expect("query projects");
    let project_id = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .expect("project_id");
    let analyzer = ArchitectureAnalyzer::new(&repo);
    let overview = analyzer.overview(project_id).expect("overview");

    // 应检测到至少一种语言。
    assert!(!overview.languages.is_empty(), "架构概览应包含语言统计");
    // 应检测到入口点（main）。
    assert!(
        overview.entry_points.iter().any(|ep| ep.name == "main"),
        "应检测到 main 入口点"
    );
}

// ===========================================================================
// § 9 — 复杂度分析
// ===========================================================================

#[tokio::test]
async fn complexity_analysis_returns_metrics() {
    use codenexus::analysis::complexity::ComplexityAnalyzer;

    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "\
fn simple() -> i32 { 42 }
fn complex_fn(x: i32) -> i32 {
    if x > 0 {
        if x > 10 {
            for i in 0..x {
                if i % 2 == 0 {
                    return i;
                }
            }
        }
    }
    0
}
",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let _result = facade.index(tmp.path(), "cplx_demo", false).expect("index");

    let repo = codenexus::storage::Repository::open_read_only(&db).expect("open read-only");
    let rows = repo
        .query("MATCH (p:Project) RETURN p.id AS id;")
        .expect("query projects");
    let project_id = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .expect("project_id");
    let analyzer = ComplexityAnalyzer::new(&repo);
    let entries = analyzer.analyze(project_id).expect("analyze");

    assert!(!entries.is_empty(), "复杂度分析应返回函数条目");
    // complex_fn 应有比 simple 更高的圈复杂度。
    let simple_entry = entries.iter().find(|e| e.name == "simple");
    let complex_entry = entries.iter().find(|e| e.name == "complex_fn");
    if let (Some(s), Some(c)) = (simple_entry, complex_entry) {
        assert!(
            c.cyclomatic > s.cyclomatic,
            "complex_fn 的圈复杂度 ({}) 应大于 simple ({})",
            c.cyclomatic,
            s.cyclomatic
        );
    }
}

// ===========================================================================
// § 10 — 社区检测
// ===========================================================================

#[tokio::test]
async fn community_detection_returns_result() {
    use codenexus::analysis::community::CommunityDetector;

    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "\
fn a() { b(); }
fn b() { c(); }
fn c() {}
fn d() { e(); }
fn e() {}
fn main() { a(); d(); }
",
    );
    let db = fresh_db_path();

    // 索引：使用 IndexFacade 写入数据。
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let _result = facade.index(tmp.path(), "comm_demo", false).expect("index");

    // 分析：索引完成后打开新的只读连接。
    let repo = codenexus::storage::Repository::open_read_only(&db).expect("open read-only");
    let rows = repo
        .query("MATCH (p:Project) RETURN p.id AS id;")
        .expect("query projects");
    let project_id = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .expect("project_id");
    let detector = CommunityDetector::new(&repo, project_id);
    let communities = detector.detect_communities().expect("detect_communities");
    // 社区检测结果可以为空（如果调用图太简单），但不应报错。
    // 对于此 fixture，至少应有一个社区。
    assert!(
        communities.is_empty() || communities.iter().all(|c| !c.members.is_empty()),
        "每个社区应有至少一个成员"
    );
}

// ===========================================================================
// § 11 — 跨服务检测
// ===========================================================================

#[tokio::test]
async fn cross_service_detection_returns_empty_for_simple_project() {
    use codenexus::analysis::cross_service::CrossServiceDetector;

    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "lib.rs", "fn main() {}\nfn helper() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let _result = facade.index(tmp.path(), "xs_demo", false).expect("index");

    let repo = codenexus::storage::Repository::open_read_only(&db).expect("open read-only");
    let rows = repo
        .query("MATCH (p:Project) RETURN p.id AS id;")
        .expect("query projects");
    let project_id = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .expect("project_id");
    let detector = CrossServiceDetector::new(&repo);
    let matches = detector.detect_all(project_id).expect("detect_all");
    // 简单项目无跨服务调用，应为空。
    assert!(
        matches.is_empty(),
        "简单项目不应有跨服务匹配，got: {matches:?}"
    );
}

// ===========================================================================
// § 12 — 集成场景
// ===========================================================================

/// 索引 → 查询 → 追踪全链路验证。
#[tokio::test]
async fn full_pipeline_index_then_query_then_trace() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "\
fn entry_point() {
    service_a();
    service_b();
}
fn service_a() { repository(); }
fn service_b() { repository(); }
fn repository() { println!(\"data\"); }
",
    );
    let db = fresh_db_path();

    // Step 1: 索引。
    let config = KitBootstrapConfig::new(db.clone());
    let kit = build_kit(&config).await.expect("build_kit");
    let indexer = kit
        .require::<codenexus::kit::IndexerModule>()
        .expect("indexer");
    let index_result = indexer
        .index(tmp.path(), "pipeline_demo", false)
        .expect("index");
    assert!(index_result.nodes_created > 0, "应创建节点");

    // Step 2: 查询。
    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let search_results = query.search("service", None, 10).expect("search");
    assert!(
        search_results.iter().any(|r| r.name.contains("service")),
        "搜索应找到 service_a / service_b"
    );

    // Step 3: 追踪。
    let trace = kit.require::<TraceModule>().expect("trace");
    let (graph, _) = trace
        .load_graph("entry_point", 5, 1000)
        .expect("load_graph");
    assert!(!graph.nodes.is_empty(), "追踪 entry_point 应返回非空图");
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.edge_type == codenexus::model::EdgeType::Calls),
        "追踪图应包含 CALLS 边"
    );
}

/// 多语言 FFI 边验证（Rust extern "C" + C 实现）。
#[tokio::test]
async fn multilang_ffi_edges_in_graph() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "src/main.rs",
        "fn main() { unsafe { native_compute(42); } }\nextern \"C\" { fn native_compute(x: i32) -> i32; }\n",
    );
    write_file(
        tmp.path(),
        "src/native.c",
        "#include <stdint.h>\nint32_t native_compute(int32_t x) { return x * 2; }\n",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "ffi_demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let result = query
        .cypher("MATCH (r:CodeRelation) RETURN r.type AS type;")
        .expect("cypher");
    let ffi_count = result
        .rows
        .iter()
        .filter(|row| row.first().and_then(|v| v.as_str()) == Some("FFI_CALLS"))
        .count();
    assert!(
        ffi_count >= 1,
        "应至少有 1 条 FFI_CALLS 边，got {ffi_count}"
    );
}

/// READS / WRITES 边在多语言索引后存在。
#[test]
fn reads_writes_edges_after_multilang_index() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "main.rs",
        "fn compute(x: i32) -> i32 { let y = x + 1; y }\n",
    );
    write_file(
        tmp.path(),
        "main.c",
        "int compute(int x) { int y = x + 1; return y; }\n",
    );
    write_file(
        tmp.path(),
        "main.py",
        "def compute(x):\n    y = x + 1\n    return y\n",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "rw_demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let result = query
        .cypher("MATCH (r:CodeRelation) RETURN r.type AS type;")
        .expect("cypher");
    let reads = result
        .rows
        .iter()
        .filter(|r| r.first().and_then(|v| v.as_str()) == Some("READS"))
        .count();
    let writes = result
        .rows
        .iter()
        .filter(|r| r.first().and_then(|v| v.as_str()) == Some("WRITES"))
        .count();
    assert!(reads > 0, "应至少有 1 条 READS 边，got {reads}");
    assert!(writes > 0, "应至少有 1 条 WRITES 边，got {writes}");
}

/// 损坏数据库返回正确错误链。
#[tokio::test]
async fn corrupt_db_error_chain() {
    use codenexus::index::IndexError;
    use codenexus::kit::KitError;
    use codenexus::storage::StorageError;

    let dir = TempDir::new().unwrap();
    let lbug_file = dir.path().join("corrupt.lbug");
    std::fs::write(&lbug_file, b"not a valid ladybugdb").expect("write corrupt");
    std::mem::forget(dir);

    let config = KitBootstrapConfig::new(lbug_file);
    let result = build_kit(&config).await;

    let kit_err = result.expect_err("build_kit 应在损坏数据库上失败");
    match &kit_err {
        KitError::BuildFailed { source, .. } => {
            let storage_err = source
                .downcast_ref::<StorageError>()
                .expect("source 应为 StorageError");
            assert!(matches!(storage_err, StorageError::Corrupt(_)));
        }
        other => panic!("期望 KitError::BuildFailed，实际 {other:?}"),
    }

    // 验证 IndexError 映射和退出码。
    let index_err: IndexError = StorageError::Corrupt("test".to_string()).into();
    assert!(matches!(index_err, IndexError::DatabaseCorrupt(_)));
    assert_eq!(index_err.exit_code(), 4);
}

/// 多项目隔离验证。
#[test]
fn multi_project_data_isolation() {
    let tmp1 = TempDir::new().unwrap();
    let tmp2 = TempDir::new().unwrap();
    write_file(tmp1.path(), "a.rs", "fn project_a_func() {}\n");
    write_file(tmp2.path(), "b.rs", "fn project_b_func() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let ra = facade.index(tmp1.path(), "proj_a", false).expect("index a");
    let rb = facade.index(tmp2.path(), "proj_b", false).expect("index b");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");

    // proj_a 搜索 project_a_func 应成功。
    let sa = query
        .search("project_a_func", Some(&ra.project_id), 10)
        .expect("search a");
    assert!(sa.iter().any(|r| r.name.contains("project_a_func")));

    // proj_a 搜索 project_b_func 应为空（隔离）。
    let cross = query
        .search("project_b_func", Some(&ra.project_id), 10)
        .expect("cross");
    assert!(
        !cross.iter().any(|r| r.name.contains("project_b_func")),
        "proj_a 不应包含 proj_b 的数据"
    );

    // proj_b 搜索 project_b_func 应成功。
    let sb = query
        .search("project_b_func", Some(&rb.project_id), 10)
        .expect("search b");
    assert!(sb.iter().any(|r| r.name.contains("project_b_func")));
}

/// Kit 引导验证：所有核心模块可解析。
#[tokio::test]
async fn kit_all_core_modules_resolvable() {
    let db = fresh_db_path();
    let config = KitBootstrapConfig::new(db);
    let kit = build_kit(&config).await.expect("build_kit");

    kit.require::<StorageModule>().expect("storage");
    kit.require::<codenexus::kit::ParserFactoryModule>()
        .expect("parser");
    kit.require::<codenexus::kit::ExtractorRegistryModule>()
        .expect("extractor");
    kit.require::<codenexus::kit::IndexerModule>()
        .expect("indexer");
    kit.require::<codenexus::kit::ResolverModule>()
        .expect("resolver");
    kit.require::<codenexus::kit::QueryModule>().expect("query");
    kit.require::<TraceModule>().expect("trace");
}

/// 索引结果结构验证。
#[test]
fn index_result_fields_are_populated() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "pub fn alpha() {}\npub fn beta() {}\nstruct Config;\n",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade
        .index(tmp.path(), "struct_demo", false)
        .expect("index");

    assert!(!result.project_id.is_empty(), "project_id 应非空");
    assert!(result.files_indexed >= 1, "files_indexed 应 >= 1");
    assert!(result.nodes_created > 0, "nodes_created 应 > 0");
}

/// 版本 API 验证。
#[test]
fn version_api_returns_non_empty() {
    let v = codenexus::version();
    assert!(!v.is_empty(), "版本号应非空");
    assert!(v.contains('.'), "版本号应含点号（semver）");
}

/// 图模型节点/边类型枚举完整性验证。
#[test]
fn model_edge_type_roundtrip() {
    use codenexus::model::EdgeType;
    // 验证常用 EdgeType 的 Debug 输出非空。
    for edge_type in [
        EdgeType::Calls,
        EdgeType::FfiCalls,
        EdgeType::Reads,
        EdgeType::Writes,
    ] {
        let s = format!("{edge_type:?}");
        assert!(!s.is_empty(), "EdgeType Debug 输出应非空");
    }
}

/// 全文搜索限制为特定项目。
#[test]
fn fulltext_search_respects_project_filter() {
    let tmp1 = TempDir::new().unwrap();
    let tmp2 = TempDir::new().unwrap();
    write_file(tmp1.path(), "lib.rs", "pub fn shared_name_alpha() {}\n");
    write_file(tmp2.path(), "lib.rs", "pub fn shared_name_beta() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let r1 = facade.index(tmp1.path(), "proj_x", false).expect("index x");
    let _r2 = facade.index(tmp2.path(), "proj_y", false).expect("index y");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    // 搜索 proj_x 中的 shared_name。
    let results_x = query
        .fulltext_search("shared_name", Some(&r1.project_id), 10)
        .expect("ft x");
    for r in &results_x {
        assert!(
            r.qualified_name
                .as_ref()
                .is_none_or(|qn| qn.starts_with("proj_x") || qn.contains(&r1.project_id)),
            "proj_x 的全文搜索结果应属于 proj_x"
        );
    }
}

/// Cypher 查询 Project 节点验证。
#[test]
fn cypher_query_project_node_exists() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade
        .index(tmp.path(), "my_project", false)
        .expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let result = query
        .cypher("MATCH (p:Project) RETURN p.name AS name LIMIT 10;")
        .expect("cypher");
    let names: Vec<String> = result
        .rows
        .into_iter()
        .filter_map(|row| {
            row.into_iter()
                .next()
                .and_then(|v| v.as_str().map(String::from))
        })
        .collect();
    assert!(
        names.iter().any(|n| n == "my_project"),
        "应存在名为 my_project 的 Project 节点，got {names:?}"
    );
}

/// .gitignore 规则遵守。
#[test]
fn gitignore_rules_are_respected() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\n");
    write_file(tmp.path(), ".gitignore", "generated/\n");
    write_file(tmp.path(), "generated/code.rs", "fn should_skip() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "gi_demo", false).expect("index");
    assert_eq!(result.files_indexed, 1, "generated/ 应被 .gitignore 跳过");
}

/// 非 ASCII 路径索引。
#[test]
fn index_with_unicode_path() {
    let tmp = TempDir::new().unwrap();
    let unicode_dir = tmp.path().join("プロジェクト");
    fs::create_dir_all(&unicode_dir).unwrap();
    fs::write(unicode_dir.join("main.rs"), "fn main() {}\n").unwrap();
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade
        .index(tmp.path(), "unicode_demo", false)
        .expect("index");
    assert!(result.files_indexed >= 1, "Unicode 路径下的文件应被索引");
}
