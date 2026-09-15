# CodeNexus Examples

示例程序集合，演示 CodeNexus 的核心功能。每个示例都是独立可运行的二进制文件（临时目录内自包含，运行结束自动清理）。

## 运行方式

```bash
# 运行单个示例
cargo run -p codenexus-examples --bin basic_indexing

# 运行所有示例
for bin in basic_indexing cypher_query symbol_search call_tracing impact_analysis export_import project_lifecycle code_analysis architecture_diagram api_surface symbol_context git_integration daemon_watch setup_mcp; do
  cargo run -p codenexus-examples --bin $bin
done
```

## 示例列表与 CLI 覆盖对照

| 示例 | 功能 | 覆盖的 CLI 命令 |
|------|------|----------------|
| `basic_indexing` | 基础索引 | `index` |
| `cypher_query` | Cypher 查询 | `query` |
| `symbol_search` | 符号搜索 | `search` |
| `call_tracing` | 调用链追踪 | `trace` |
| `impact_analysis` | 影响分析 | `impact` |
| `export_import` | 导出/导入 | `export` / `import` |
| `project_lifecycle` | 项目生命周期 | `list` / `status` / `clean` |
| `code_analysis` | 代码质量分析 | `complexity` / `dead_code` / `community` |
| `architecture_diagram` | 架构总览与图渲染 | `architecture` / `diagram` / `arch_diff` |
| `api_surface` | API/Web 服务面 | `route_map` / `shape_check` / `api_impact` / `cross_service` / `tool_map` |
| `symbol_context` | 符号 360° 视图 | `context` |
| `git_integration` | Git 集成 | `detect_changes` / `hook` |
| `daemon_watch` | 文件监视守护 | `daemon` |
| `setup_mcp` | MCP 接入配置 | `setup` |

**CLI-only，无示例**（需要外部环境或为长驻服务，直接用二进制体验）：

- `mcp` — stdio 长驻 MCP 服务（`codenexus mcp`，由 AI agent 客户端拉起）
- `lsp_hover` / `lsp_goto_def` — 需要外部 language server（rust-analyzer / pyright / clangd 等）
- `rename` — 重命名建议逻辑目前为 service 私有，无公开库入口
- `hook` 的完整 stdin 协议 — 摘要统计已在 `git_integration` 中复现

## 前提条件

- Rust 1.97+
- CodeNexus 默认 feature（`full`：全部语言 + daemon + analysis + cache + lsp + mcp）
- `git_integration` 需要 PATH 中有 `git`

## 每个示例的工作原理

1. 创建临时目录作为工作区
2. 将内嵌的 Rust 源码写入临时文件
3. 通过 `IndexFacade` 索引源码到 LadybugDB（或 `setup` 示例中的临时家目录）
4. 通过 `QueryFacade` / `Repository` / 各分析器执行查询与分析
5. 退出时临时目录自动清理

## 作为库使用

这些示例展示了如何以编程方式使用 `codenexus` 库：

```rust
use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;
use codenexus::storage::Repository;

// 索引源码
let indexer = IndexFacade::new(db_path)?;
let result = indexer.index(&source_dir, "my-project", true)?;

// 查询图谱
let query = QueryFacade::new(db_path)?;
let functions = query.cypher("MATCH (f:Function) RETURN f.name")?;

// 搜索符号
let results = query.search("parse", Some(&result.project_id), 10)?;

// 打开存储，交给分析器使用
let repo = Repository::open(db_path)?;
let entries = codenexus::analysis::complexity::ComplexityAnalyzer::new(&repo)
    .analyze(&result.project_id)?;
```

## 注意事项

示例使用 `IndexFacade` / `Repository` / 分析器直接操作数据库，而非通过 Kit 注册表。
这是因为 Kit 在启动时为每个子系统创建独立的数据库连接，可能导致文件数据库的数据可见性问题。
直接使用 Facade 是推荐的编程方式。

已知现状：当前 Rust 提取器不把函数体写入 Function.content，
因此 `cross_service` 的字面量扫描对全新索引返回 0 条匹配（`api_surface` 示例内已注释说明）。
