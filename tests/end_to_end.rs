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
    assert!(result.files_indexed >= 2, "至少索引 2 个文件，got {}", result.files_indexed);
    assert!(result.nodes_created > 0, "应创建节点，got {}", result.nodes_created);
}

#[test]
fn index_creates_project_node() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\n");
    let db = fresh_db_path();

    let facade = IndexFacade::new(&db).expect("IndexFacade::new");
    facade.index(tmp.path(), "my_project", false).expect("index");

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
    let result_a = facade.index(tmp1.path(), "project_a", false).expect("index a");
    let result_b = facade.index(tmp2.path(), "project_b", false).expect("index b");

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

    assert!(
        !results.is_empty(),
        "全文搜索 parse 应返回结果"
    );
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
    assert_eq!(
        result2.files_skipped, 1,
        "第二次应跳过未变更文件"
    );
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
    assert_eq!(
        result2.files_indexed, 1,
        "--force 应重解析所有文件"
    );
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
