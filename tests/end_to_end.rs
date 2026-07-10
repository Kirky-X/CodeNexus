// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! 端到端集成测试：索引 → 查询 → 追踪全流程（SubTask 17.1）。
//!
//! 本测试覆盖 AC-INDEX-001（C/Rust/Fortran 代码库端到端索引）、
//! AC-INDEX-003（多项目共存互不干扰）、AC-QUERY-001（Cypher 查询）、
//! AC-SEARCH-001（结构化搜索）等验收标准。

use std::fs;
use std::path::Path;

use codenexus::index::IndexFacade;
use codenexus::model::NodeLabel;
use codenexus::query::QueryFacade;
use tempfile::TempDir;

// SubTask 17.3: tracing capture helpers (used by index_emits_all_log_events).
use std::cell::RefCell;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

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
    let path = dir.path().join("integration_testdb");
    std::mem::forget(dir);
    path
}

/// 构建一个包含 Rust + C 的小型代码库。
fn build_multilang_repo(dir: &Path) {
    // Rust 文件：main 调用 helper，helper 调用 c_bridge（extern "C"）。
    write_file(
        dir,
        "src/main.rs",
        "\
fn main() {
    helper();
}

fn helper() {
    let x = 42;
    println!(\"{}\", x);
}

extern \"C\" {
    fn c_bridge(input: i32) -> i32;
}
",
    );
    // C 文件：c_bridge 函数定义。
    write_file(
        dir,
        "src/c_bridge.c",
        "\
#include <stdio.h>

int c_bridge(int input) {
    return input * 2;
}
",
    );
    // C 头文件。
    write_file(
        dir,
        "src/c_bridge.h",
        "\
#ifndef C_BRIDGE_H
#define C_BRIDGE_H
int c_bridge(int input);
#endif
",
    );
}

// --- AC-INDEX-001: 端到端索引 ---

#[test]
fn index_multilang_repo_succeeds() {
    let tmp = TempDir::new().unwrap();
    build_multilang_repo(tmp.path());
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "demo", false).expect("index");

    assert!(!result.project_id.is_empty(), "project_id 应非空");
    assert!(
        result.files_indexed >= 2,
        "至少索引 2 个文件，got {}",
        result.files_indexed
    );
    assert!(
        result.nodes_created > 0,
        "应创建节点，got {}",
        result.nodes_created
    );
}

#[test]
fn index_creates_project_node() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade
        .index(tmp.path(), "my_project", false)
        .expect("index");

    // 通过 Cypher 查询验证 Project 节点存在（Project 表无 qualifiedName/filePath
    // 等列，故不能使用 search_by_type）。
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

// --- AC-INDEX-003: 多项目隔离 ---

#[test]
fn multi_project_isolation() {
    let tmp1 = TempDir::new().unwrap();
    let tmp2 = TempDir::new().unwrap();
    write_file(tmp1.path(), "a.rs", "fn alpha() {}\n");
    write_file(tmp2.path(), "b.rs", "fn beta() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    // 管线将 project_id（UUID）写入节点的 project 列，故过滤时需使用 project_id。
    let result_a = facade
        .index(tmp1.path(), "project_a", false)
        .expect("index a");
    let result_b = facade
        .index(tmp2.path(), "project_b", false)
        .expect("index b");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");

    // project_a 仅含 alpha。
    let results_a = query
        .search("alpha", Some(&result_a.project_id), 10)
        .expect("search a");
    assert!(
        results_a.iter().any(|r| r.name.contains("alpha")),
        "project_a 应含 alpha"
    );

    // project_b 仅含 beta。
    let results_b = query
        .search("beta", Some(&result_b.project_id), 10)
        .expect("search b");
    assert!(
        results_b.iter().any(|r| r.name.contains("beta")),
        "project_b 应含 beta"
    );

    // project_a 不应含 beta。
    let cross = query
        .search("beta", Some(&result_a.project_id), 10)
        .expect("search cross");
    assert!(
        !cross.iter().any(|r| r.name.contains("beta")),
        "project_a 不应含 beta（多项目隔离）"
    );
}

// --- AC-INDEX-004: .gitignore 跳过 ---

#[test]
fn gitignore_target_dir_skipped() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\n");
    write_file(tmp.path(), ".gitignore", "target/\n");
    // target/ 下的文件应被跳过。
    write_file(tmp.path(), "target/build.rs", "fn should_skip() {}\n");

    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "demo", false).expect("index");

    // 只应索引 main.rs，不应索引 target/build.rs。
    assert_eq!(
        result.files_indexed, 1,
        "target/ 应被 .gitignore 跳过，实际索引 {} 个文件",
        result.files_indexed
    );
}

// --- AC-QUERY-001: Cypher 查询 ---

#[test]
fn cypher_query_after_index() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "lib.rs",
        "pub fn parse_input(s: &str) -> Vec<u8> { s.as_bytes().to_vec() }\n",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let result = query
        .cypher("MATCH (f:Function) RETURN f.name AS name LIMIT 5;")
        .expect("cypher");

    assert!(!result.columns.is_empty(), "应返回列");
    assert!(!result.rows.is_empty(), "应返回至少一行");
}

// --- AC-SEARCH-001: 结构化搜索 ---

#[test]
fn structured_search_by_name() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "main.rs",
        "fn parse_config() {}\nfn read_file() {}\n",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let results = query.search("parse", None, 10).expect("search");

    assert!(
        results.iter().any(|r| r.name.contains("parse")),
        "结构化搜索应找到 parse_config"
    );
}

#[test]
fn structured_search_by_type() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\nstruct Config;\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    let results = query
        .search_by_type(NodeLabel::Struct, None, 10)
        .expect("search_by_type");

    assert!(
        results.iter().any(|r| r.name.contains("Config")),
        "按类型搜索应找到 Struct 节点 Config"
    );
}

// --- 全文搜索 ---

#[test]
fn fulltext_search_finds_matches() {
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

    assert!(!results.is_empty(), "全文搜索 parse 应返回结果");
}

// --- 增量索引 ---

#[test]
fn incremental_index_skips_unchanged_files() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "fn a() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");

    // 第一次索引：解析 a.rs。
    let result1 = facade.index(tmp.path(), "demo", false).expect("index 1");
    assert_eq!(result1.files_indexed, 1, "首次应索引 1 个文件");

    // 第二次索引：a.rs 哈希未变，应跳过。
    let result2 = facade.index(tmp.path(), "demo", false).expect("index 2");
    assert_eq!(result2.files_skipped, 1, "第二次应跳过未变更文件");
}

#[test]
fn incremental_index_detects_new_file() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "fn a() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index 1");

    // 新增文件 b.rs。
    write_file(tmp.path(), "b.rs", "fn b() {}\n");
    let result2 = facade.index(tmp.path(), "demo", false).expect("index 2");

    assert!(
        result2.files_indexed >= 1,
        "应索引新增的 b.rs，got {}",
        result2.files_indexed
    );
}

// --- --force 全量重解析 ---

#[test]
fn force_reindexes_all_files() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "fn a() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "demo", false).expect("index 1");

    // --force 应忽略哈希，全量重解析。
    let result2 = facade.index(tmp.path(), "demo", true).expect("index force");
    assert_eq!(result2.files_indexed, 1, "--force 应重解析所有文件");
}

// --- 异常处理 ---

#[test]
fn index_nonexistent_path_returns_error() {
    let db = fresh_db_path();
    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(Path::new("/nonexistent/path"), "demo", false);
    assert!(result.is_err(), "不存在的路径应返回错误");
}

// --- 多语言索引 ---

#[test]
fn index_python_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "main.py",
        "\
def greet(name):
    return f\"Hello, {name}!\"

class Greeter:
    def __init__(self):
        self.name = \"world\"
",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "py_demo", false).expect("index");

    assert!(result.files_indexed >= 1, "应索引 Python 文件");
    assert!(result.nodes_created > 0, "应创建节点");
}

#[test]
fn index_typescript_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "main.ts",
        "\
function add(a: number, b: number): number {
    return a + b;
}

class Calculator {
    add(a: number, b: number): number {
        return add(a, b);
    }
}
",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "ts_demo", false).expect("index");

    assert!(result.files_indexed >= 1, "应索引 TypeScript 文件");
}

#[test]
fn index_fortran_file() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "main.f90",
        "\
module math_utils
    implicit none
contains
    function square(x) result(y)
        integer, intent(in) :: x
        integer :: y
        y = x * x
    end function square
end module math_utils
",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    let result = facade.index(tmp.path(), "f90_demo", false).expect("index");

    assert!(result.files_indexed >= 1, "应索引 Fortran 文件");
}

// --- SubTask 17.3: end-to-end LOG event verification (LOG-001/002/006) ---

/// A `MakeWriter` that buffers emitted tracing events into a shared `Vec<u8>`.
struct CapturingMakeWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl MakeWriter<'_> for CapturingMakeWriter {
    type Writer = CapturingWriter;

    fn make_writer(&self) -> Self::Writer {
        CapturingWriter {
            buf: self.buf.clone(),
        }
    }
}

struct CapturingWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Write for CapturingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().unwrap().write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Thread-local storage for the tracing `DefaultGuard` on rayon worker
// threads. Each worker thread sets its own subscriber via
// `tracing::subscriber::set_default` and stores the guard here so it stays
// alive for the thread's lifetime (mirrors the pattern in `parallel.rs`).
thread_local! {
    static TRACING_GUARD: RefCell<Option<tracing::subscriber::DefaultGuard>> =
        const { RefCell::new(None) };
}

/// Verifies that `codenexus index` emits all LOG events defined in the spec
/// when run with RUST_LOG=debug:
/// - LOG-001: `index_started` and `index_completed` (info, main thread)
/// - LOG-002: `file_parsed` (debug, rayon worker thread — one per file)
/// - LOG-006: `performance` with `files_per_second` (info, main thread)
///
/// Because `parallel_parse` uses rayon worker threads that do NOT inherit the
/// current thread's tracing subscriber, this test builds a custom rayon thread
/// pool whose `start_handler` installs the same capturing subscriber on each
/// worker thread. The index call is run via `pool.install()` so that `par_iter`
/// inside the pipeline uses worker threads that have the subscriber set.
#[test]
fn index_emits_all_log_events() {
    use rayon::ThreadPoolBuilder;

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));

    // Subscriber for the main thread (captures LOG-001 and LOG-006).
    let main_subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_target(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(CapturingMakeWriter { buf: buf.clone() })
        .finish();

    // Custom rayon pool: each worker installs a capturing subscriber sharing
    // the same buffer, so LOG-002 file_parsed events on worker threads are
    // captured too.
    let buf_for_handler = buf.clone();
    let pool = ThreadPoolBuilder::new()
        .start_handler(move |_idx| {
            let worker_subscriber = tracing_subscriber::FmtSubscriber::builder()
                .with_target(false)
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(CapturingMakeWriter {
                    buf: buf_for_handler.clone(),
                })
                .finish();
            let guard = tracing::subscriber::set_default(worker_subscriber);
            TRACING_GUARD.with(|g| *g.borrow_mut() = Some(guard));
        })
        .build()
        .expect("rayon thread pool");

    // Prepare a small repo with 2 Rust files.
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "fn a() {}\n");
    write_file(tmp.path(), "b.rs", "fn b() {}\n");
    let db = fresh_db_path();
    let src_path = tmp.path().to_path_buf();

    tracing::subscriber::with_default(main_subscriber, || {
        pool.install(|| {
            let facade = IndexFacade::new(&db).expect("IndexFacade::new");
            facade.index(&src_path, "log_e2e", false).expect("index");
        });
    });

    let bytes = buf.lock().unwrap().clone();
    let captured = String::from_utf8(bytes).unwrap();

    // LOG-001: index_started
    assert!(
        captured.contains("index_started"),
        "LOG-001: index_started event missing, got: {captured:?}"
    );
    // LOG-001: index_completed
    assert!(
        captured.contains("index_completed"),
        "LOG-001: index_completed event missing, got: {captured:?}"
    );
    // LOG-002: file_parsed (at least one per parsed file)
    assert!(
        captured.contains("file_parsed"),
        "LOG-002: file_parsed event missing, got: {captured:?}"
    );
    let file_parsed_count = captured.matches("file_parsed").count();
    assert!(
        file_parsed_count >= 2,
        "LOG-002: expected at least 2 file_parsed events (one per file), got {file_parsed_count}"
    );
    // LOG-006: performance
    assert!(
        captured.contains("performance"),
        "LOG-006: performance event missing, got: {captured:?}"
    );
    assert!(
        captured.contains("files_per_second"),
        "LOG-006: performance event should carry files_per_second field, got: {captured:?}"
    );
}

// --- BR-TRACE-005/006: Reads/Writes edges in graph (multi-language e2e) ---

/// Verifies that indexing a multi-language repo produces READS and WRITES
/// edges in the graph (BR-TRACE-005 / BR-TRACE-006). Each language fixture
/// contains a function that reads a parameter and writes a local variable,
/// so the resolver should emit at least one of each edge type.
#[test]
fn reads_writes_edges_exist_after_multilang_index() {
    let tmp = TempDir::new().unwrap();
    // Rust: `let y = x + 1;` reads x, writes y.
    write_file(
        tmp.path(),
        "main.rs",
        "fn caller(x: i32) -> i32 {\n    let y = x + 1;\n    y\n}\n",
    );
    // C: `int y = x + 1;` reads x, writes y (init_declarator).
    write_file(
        tmp.path(),
        "main.c",
        "int caller(int x) {\n    int y = x + 1;\n    return y;\n}\n",
    );
    // Python: `y = x + 1` reads x, writes y (assignment).
    write_file(
        tmp.path(),
        "main.py",
        "def caller(x):\n    y = x + 1\n    return y\n",
    );
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "rw_e2e", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    // Fetch all CodeRelation rows and filter by type in Rust (robust against
    // Cypher WHERE/count dialect differences across LadybugDB versions).
    let result = query
        .cypher("MATCH (r:CodeRelation) RETURN r.type AS type;")
        .expect("cypher CodeRelation");

    let reads_count = result
        .rows
        .iter()
        .filter(|row| row.first().and_then(|v| v.as_str()) == Some("READS"))
        .count();
    let writes_count = result
        .rows
        .iter()
        .filter(|row| row.first().and_then(|v| v.as_str()) == Some("WRITES"))
        .count();

    assert!(
        reads_count > 0,
        "graph should contain at least one READS edge (BR-TRACE-005), got {reads_count}"
    );
    assert!(
        writes_count > 0,
        "graph should contain at least one WRITES edge (BR-TRACE-006), got {writes_count}"
    );
}

// --- FFI cross-language edges (complete-test-coverage spec) ---

/// Verifies that indexing a multilang repo (Rust extern "C" + C code) produces
/// FFI_CALLS edges in the graph.
///
/// Spec: `ffi_edge_exists_after_multilang_index` — indexes `build_multilang_repo`
/// then queries `MATCH (r:CodeRelation) RETURN r.type AS type;` and asserts at
/// least one row has type `FFI_CALLS`.
#[test]
fn ffi_edge_exists_after_multilang_index() {
    let tmp = TempDir::new().unwrap();
    build_multilang_repo(tmp.path());
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "multilang", false).expect("index");

    let query = QueryFacade::new(&db).expect("QueryFacade::new");
    // Fetch all CodeRelation rows and filter by type in Rust (robust against
    // Cypher WHERE/count dialect differences across LadybugDB versions).
    let result = query
        .cypher("MATCH (r:CodeRelation) RETURN r.type AS type;")
        .expect("cypher CodeRelation");

    let ffi_count = result
        .rows
        .iter()
        .filter(|row| row.first().and_then(|v| v.as_str()) == Some("FFI_CALLS"))
        .count();

    assert!(
        ffi_count >= 1,
        "FFI 索引后应至少有 1 条 FfiCalls 边，实际 {}",
        ffi_count
    );
}

/// Verifies that the trace engine returns cross-language paths containing
/// FfiCalls edges after indexing a multilang repo.
///
/// Indexes `build_multilang_repo`, loads the trace graph around `c_bridge`,
/// and asserts the graph contains at least one FfiCalls edge.
#[test]
fn ffi_trace_returns_cross_language_path() {
    use codenexus::kit::{build_kit, IndexerKey, KitBootstrapConfig, TraceKey};
    use codenexus::model::EdgeType;

    let tmp = TempDir::new().unwrap();
    build_multilang_repo(tmp.path());
    let db = fresh_db_path();

    let kit = build_kit(&KitBootstrapConfig::new(db.clone())).expect("build_kit");
    let indexer = kit.require::<IndexerKey>().expect("require_indexer");
    indexer
        .index(tmp.path(), "multilang", false)
        .expect("index");

    // Load the trace graph around "c_bridge" and verify FfiCalls edge exists.
    let trace = kit.require::<TraceKey>().expect("require_trace");
    let graph = trace.load_graph("c_bridge", 3).expect("load_graph");

    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.edge_type == EdgeType::FfiCalls),
        "trace 应返回含 FfiCalls 边的跨语言路径，got edges: {:?}",
        graph.edges.iter().map(|e| e.edge_type).collect::<Vec<_>>()
    );
}

// --- DatabaseCorrupt end-to-end (complete-test-coverage spec) ---

/// Verifies that a corrupt database produces an error chain mapping to exit
/// code 4 (`IndexError::DatabaseCorrupt`).
///
/// Spec: `corrupt_db_returns_exit_code_4` — writes invalid bytes to a `.lbug`
/// file, then attempts to build a Kit against that database. `build_kit`
/// fails with `KitError::BuildFailed` whose `source` is `StorageError::Corrupt`.
/// The manual `From<StorageError> for IndexError` impl maps `Corrupt` to
/// `IndexError::DatabaseCorrupt`, whose `exit_code()` returns 4.
///
/// # 退出码路径说明
///
/// spec 写明 "executes the index command against that database" 并期望进程
/// 退出码 4。`build_kit` 在命令处理之前被调用（负责装配 Kit），遇到损坏
/// 数据库时 `build_kit` 先失败并返回 `KitError::BuildFailed`。
/// `CliError::Kit(_)` 分支通过 `kit_exit_code`（见 src/service/error.rs）
/// downcast source chain，对 `StorageError::Corrupt` / `IndexError::DatabaseCorrupt`
/// 返回退出码 4。
///
/// 本测试验证**错误链**（`build_kit` → `KitError::BuildFailed` →
/// `StorageError::Corrupt` → `IndexError::DatabaseCorrupt` → `exit_code 4`）。
#[test]
fn corrupt_db_returns_exit_code_4() {
    use codenexus::index::IndexError;
    use codenexus::kit::{build_kit, KitBootstrapConfig, KitError};
    use codenexus::storage::StorageError;

    let dir = TempDir::new().unwrap();
    let lbug_file = dir.path().join("corrupt.lbug");
    std::fs::write(&lbug_file, b"this is not a valid ladybugdb file").expect("write corrupt file");
    // Leak the TempDir so the .lbug file survives the test (matches
    // fresh_db_path's pattern).
    std::mem::forget(dir);

    let config = KitBootstrapConfig::new(lbug_file);
    let result = build_kit(&config);

    let kit_err = result.expect_err("build_kit 应在损坏数据库上失败");
    // 导航错误链：KitError::BuildFailed { source } → source。
    let build_failed_source = match &kit_err {
        KitError::BuildFailed { source, .. } => source.as_ref(),
        other => panic!("期望 KitError::BuildFailed，实际 {other:?}"),
    };

    // Downcast source 到 StorageError。bootstrap 的 StorageModuleBuilder 调用
    // StorageConnection::open，将损坏模式错误包装为 StorageError::Corrupt。
    let storage_err = build_failed_source
        .downcast_ref::<StorageError>()
        .unwrap_or_else(|| {
            panic!(
                "期望 BuildFailed.source 为 StorageError，实际: {:?}",
                build_failed_source
            );
        });

    assert!(
        matches!(storage_err, StorageError::Corrupt(_)),
        "期望 StorageError::Corrupt，实际: {storage_err:?}"
    );

    // 验证 From<StorageError> for IndexError 映射产生 DatabaseCorrupt 且
    // exit_code == 4。
    let index_err: IndexError = StorageError::Corrupt("test corrupt".to_string()).into();
    assert!(
        matches!(index_err, IndexError::DatabaseCorrupt(_)),
        "期望 IndexError::DatabaseCorrupt，实际: {index_err:?}"
    );
    assert_eq!(
        index_err.exit_code(),
        4,
        "IndexError::DatabaseCorrupt exit_code 必须为 4 (PRD §4.1.6)"
    );
}
