# 📘 CodeNexus API 参考

本参考文档完整收录 CodeNexus 库 crate（`codenexus`，入口 `src/lib.rs`）的公开 API：核心 Facade、能力 trait、数据模型、错误类型，以及 CLI/MCP 的对外接口契约。文档假设启用了 `full` 预设；使用其他特性组合时，部分模块可能不可用（各模块的 feature 门控见 [特性门控](#-特性门控)）。

## 📋 目录

- [概述](#-概述)
- [核心 Facade API](#-核心-facade-api)
  - [IndexFacade（索引）](#indexfacade索引)
  - [QueryFacade（查询与搜索）](#queryfacade查询与搜索)
  - [TraceFacade（追踪与影响分析）](#tracefacade追踪与影响分析)
- [模块参考](#-模块参考)
- [Kit 能力注册表](#-kit-能力注册表)
- [CLI / MCP 对外接口](#-cli--mcp-对外接口)
- [错误类型与退出码](#-错误类型与退出码)
- [特性门控](#-特性门控)
- [使用示例](#-使用示例)
- [相关文档](#-相关文档)

---

## 🎯 概述

`codenexus` 是一个库 + 二进制双目标 crate：`src/lib.rs` 暴露公共 API 供 CLI 二进制与下游嵌入方使用，`src/main.rs` 是 sdforge 驱动的 CLI 二进制。

| 原则 | 说明 |
|:-----|:-----|
| **Facade 分层** | 应用层只面向 `IndexFacade` / `QueryFacade` / `TraceFacade` 三个门面；管线、解析、存储细节在门面之后 |
| **能力注册表** | 跨模块协作经 `trait-kit` Kit 注册表（`kit` 模块）组装，feature 决定哪些能力可解析 |
| **Feature 门控** | 语言与可选能力均为独立 feature，未启用的代码不参与编译；至少需要一个 `lang-*` 或 `lsp` feature，否则编译期 `compile_error!` 报错 |
| **失败显性化** | 错误类型统一收敛到 `CodeNexusError`，并映射到稳定的进程退出码 |

> 完整特性清单与预设说明见 [README · 构建预设与 Feature 开关](../README.md#-构建预设与-feature-开关)。

---

## 🧱 核心 Facade API

### IndexFacade（索引）

定义于 `src/index/pipeline.rs`。负责文件发现 → 增量哈希 → 并行解析 → 符号解析 → 批量入库的完整管线。

| 方法 | 签名要点 | 说明 |
|------|----------|------|
| `new` | `(&self, db_path: &Path) -> Result<Self>` | 打开（或创建）目标 LadybugDB 数据库 |
| `empty` | `(project_id: impl Into<String>) -> Self` | 构造空 Facade（测试/特殊用途） |
| `with_cache` | `(self, cache: Arc<dyn CacheStore>) -> Self` | 挂接查询缓存（`cache` feature） |
| `with_budget` | `(self, budget: MemoryBudget) -> Self` | 设置内存预算（大仓库三级内存压力防线） |
| `index` | `(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult>` | 运行标准索引管线；`force = true` 全量重建 |
| `index_incremental` | `(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult>` | 增量管线（仅解析变更文件；当前委托到 `index`） |
| `index_ram_first` | `(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult>` | RAM 优先管线：源码读入内存并 LZ4 压缩后批量入库。适合 < 1 GB 源码的中小仓库；大仓库请用默认 `index` 流式路径避免 OOM |

`IndexResult` 字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `project_id` | `String` | 本次索引分配的项目 id（UUIDv7） |
| `files_indexed` | `usize` | 实际解析的文件数（变更 + 新增） |
| `files_skipped` | `usize` | 哈希匹配而跳过的文件数 |
| `nodes_created` | `usize` | 创建的节点数 |
| `edges_created` | `usize` | 创建的边数 |
| `duration_ms` | `u64` | 索引耗时（毫秒） |

### QueryFacade（查询与搜索）

定义于 `src/query/facade.rs`。封装 Cypher 子集查询与结构化 / 全文搜索。

| 方法 | 签名要点 | 说明 |
|------|----------|------|
| `new` | `(db_path: &Path) -> Result<Self>` | 打开数据库连接 |
| `new_read_only` | `(db_path: &Path) -> Result<Self>` | 只读连接（多进程并发读） |
| `with_connection` | `(conn: StorageConnection) -> Self` | 复用已有连接 |
| `connection` | `(&self) -> &StorageConnection` | 暴露底层连接 |
| `cypher` | `(&self, query: &str) -> Result<QueryResult>` | 执行 Cypher 子集查询；`QueryResult.rows` 为 JSON 值向量（`row[0].as_str()` 等按列取值）。子集不支持 `UNION` / 多标签 `OR` |
| `search` | `(&self, text: &str, project: Option<&str>, limit: usize) -> Result<Vec<SearchResult>>` | 按名称结构化搜索（CONTAINS），按相关度排序 |
| `search_by_type` | `(&self, label: NodeLabel, project: Option<&str>, limit: usize) -> Result<Vec<SearchResult>>` | 按节点类型列出符号 |
| `search_by_file` | `(&self, file_path: &str, project: Option<&str>) -> Result<Vec<SearchResult>>` | 列出某文件内的全部符号 |
| `fulltext_search` | `(&self, text: &str, project: Option<&str>, limit: usize) -> Result<Vec<SearchResult>>` | BM25 全文搜索（有 FTS 扩展用 FTS，否则回退 CONTAINS） |

`SearchResult` 字段：`name`、`label`、`file_path: Option<String>`、`start_line: Option<u32>`、`qualified_name: Option<String>`、`score: f64`（0.0-1.0 相关度）、`match_reason: String`（如 `"exact name match"`、`"bm25 fts"`）。

### TraceFacade（追踪与影响分析）

定义于 `src/trace/facade.rs`。封装调用链 / 数据流追踪与影响分析。

| 方法 | 签名要点 | 说明 |
|------|----------|------|
| `new` | `(storage: &'a dyn Storage) -> Self` | 基于 Storage trait 对象构造 |
| `with_config` | `(storage: &'a dyn Storage, config: TraceConfig) -> Self` | 携带 `TraceConfig` 构造 |
| `config` / `storage` | 访问器 | 读取配置与存储句柄 |
| `apply_path_filter` | 路径过滤 | glob 语义过滤追踪路径（CLI `--path_filter` 的库层实现） |
| `trace` | `(&self, symbol: &str, trace_type: TraceType, depth: usize) -> Result<TraceResult>`（图视图形态） | 对图执行调用/数据流追踪 |
| `trace_by_id` | 按节点 id 追踪 | 从已解析的节点 id 出发 |

`TraceConfig`（定义于 `src/trace/module.rs`）字段与默认值：

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `db_path` | `PathBuf` | `":memory:"` | 数据库路径；`:memory:` 为内存库（测试用） |
| `max_depth` | `u32` | `5` | 最大追踪深度（上限 `MAX_DEPTH_LIMIT = 10`） |
| `edge_types` | `Vec<EdgeType>` | `[Calls]` | 参与追踪的边类型 |
| `path_filter` | `Option<PathFilter>` | `None` | 可选路径过滤 |
| `detect_cycles` | `bool` | `false` | 是否检测环 |
| `cross_service` | `bool` | `false` | 是否遍历 `HttpCalls` 边做跨服务追踪 |
| `read_only` | `bool` | `false` | 只读打开（多进程共享读；跳过 schema 初始化） |

`TraceResult` 字段：`symbol`、`paths: Vec<TracePath>`（`nodes` / `edges` / `depth`）、`cycles: Vec<TraceCycle>`。

---

## 🧩 模块参考

`src/lib.rs` 导出的公开模块一览（feature 门控见 [特性门控](#-特性门控)）：

| 模块 | 职责 | 关键类型 |
|------|------|----------|
| `model` | 图数据模型 | `Node` / `Edge` / `Graph`、`NodeLabel`（44 种）、`EdgeType`（30 种）、语言与 i18n 支持 |
| `discover` | 文件发现 | 遵守 `.gitignore` 的目录遍历（`ignore` crate） |
| `parse` | tree-sitter 解析 | 各语言 extractor，把 AST 转为 `Node`/`Edge` 记录 |
| `ir` | 中间表示 | `ExtractResult` 等管线中间产物 |
| `resolve` | 符号解析 | FQN 生成、调用解析（calls）、数据流（dataflow）、FFI、导入、类型解析、作用域 |
| `index` | 索引管线 | `IndexFacade`、`Pipeline`（DAG 相位）、`MemoryBudget`、增量哈希 |
| `query` | 查询引擎 | `QueryFacade`、Cypher 子集、结构化搜索、BM25F 全文（`bm25f.rs`）、分词 |
| `trace` | 追踪引擎 | `TraceFacade`、BFS 追踪、调用图、污点追踪（`TaintPathTracer`）、影响分析（`impact.rs`）、360° 上下文（`context.rs`） |
| `storage` | 图存储 | `Storage` trait、`StorageConnection`、`Repository`、schema 管理、质量检查（`quality.rs`） |
| `diagnostics` | 诊断回执 | 稳定规则码（DQ-002 Duplicate FQN、DQ-004 Orphan edge 等）+ 证据 + 修复话术 |
| `service` | CLI/MCP 服务层 | 每个 `#[forge]` 命令的 core + CLI wrapper + MCP wrapper；`error::CodeNexusError`（在 crate 根重导出） |
| `kit` | 能力注册表 | `build_kit` / `KitBootstrapConfig`（见下节） |
| `analysis`（`analysis`） | 分析工具包 | 死代码检测、架构概览、复杂度、社区检测、跨服务检测 |
| `diagram`（`diagram`） | 架构图管线 | 确定性布局、正交路由、HTML 写出、质检、语义 Delta |
| `embed`（`embed`） | 向量嵌入 | 语义搜索嵌入（本地 ONNX 推理） |
| `lsp`（`lsp`） | LSP 增强 | 7 个 LSP 客户端、按需启动、hover 批量更新 |
| `cache`（`cache`） | 查询缓存 | `CacheStore` 能力（oxcache） |
| `daemon`（`daemon`） | 文件监视守护进程 | 事件循环、去抖（`DEFAULT_DEBOUNCE_MS = 2000`）、优雅退出 |

crate 根另外导出：

| 项 | 说明 |
|----|------|
| `version() -> &'static str` | 返回 crate 版本（CLI `--version` 使用） |
| `CodeNexusError` | `service::error::CodeNexusError` 的根重导出（见 [错误类型](#-错误类型与退出码)） |
| `sdforge`（`cli`） | sdforge 的重导出，供二进制目标复用 `sdforge::clap` 而无需直接依赖 clap |

### Storage trait

定义于 `src/storage/capability.rs`，是存储层的统一端口（object-safe、`Send + Sync`），每个方法对应 `StorageConnection` / `Repository` 的既有实现：

| 方法 | 说明 |
|------|------|
| `init_schema` | 初始化完整 schema（幂等），返回 `SchemaInitReport` |
| `execute` | 执行不返回行的 Cypher 语句（DDL/DML） |
| `query` | 执行 Cypher 查询，返回 `Vec<Vec<serde_json::Value>>` |
| `save_project` / `save_nodes` / `save_edges` | 保存项目节点 / 按标签批量保存节点（CSV `COPY FROM`，ADR-014）/ 批量保存边 |
| `get_project` / `list_projects` | 项目查询 |
| `query_functions` | 列出项目内全部函数（按限定名排序） |
| `get_file_hash` / `get_all_file_hashes` | 增量索引用的文件哈希存取 |
| `delete_project` / `delete_file_nodes` | 项目与文件级删除（含孤儿边清理） |

---

## 🧰 Kit 能力注册表

`src/kit/`（基于 trait-kit）在进程内组装各能力，CLI 的 `init_kit` 也走这条路径：

| API | 说明 |
|-----|------|
| `KitBootstrapConfig::new(db_path: PathBuf)` | 以数据库路径构造引导配置 |
| `with_debounce_ms(u64)` | daemon 去抖毫秒数（默认 2000） |
| `with_read_only(bool)` | 只读模式（追踪/查询命令） |
| `with_embedding_config(EmbeddingConfig)` | 嵌入配置（`embed` feature） |
| `build_kit(config)` | 构建 Kit 实例；`KitError` 报告能力缺失 / 构建失败 |

---

## 🖥️ CLI / MCP 对外接口

CLI 与 MCP 共用 `src/service/` 中 `#[forge]` 宏定义的同一套命令（core 函数 + CLI wrapper + MCP wrapper）。37 个 CLI 子命令（外加 `mcp` 服务模式）、10 个 MCP 工具（query / trace / impact / search / context / architecture / diagram / arch_diff / dead_code / detect_changes）与全部参数的语义见 [📖 用户指南](USER_GUIDE.md)。

关键契约：

- **参数风格**：无位置参数，snake_case 长选项，布尔显式传值；可选参数带内置默认值（如 `trace --depth 5`、`search --limit 50`），并可用 `.codenexus/config.json` 按项目固化（优先级：CLI flag > 配置文件 > 内置默认）。
- **全局选项**：`--db <DB_PATH>`、`--debounce-ms <MS>`。
- **退出码**：见下节；MCP 工具以回执/错误对象返回，不使用进程退出码。
- **输出**：结果命令向 stdout 输出单个 JSON 对象/数组（所有日志走 stderr，重定向安全）；`daemon`、`hook`、`mcp` 为流式/常驻。

---

## ❌ 错误类型与退出码

`CodeNexusError`（`src/service/error.rs`，crate 根重导出）统一库与服务层错误，`exit_code()` 方法映射到稳定的进程退出码：

| 退出码 | 变体 | 场景 |
|--------|------|------|
| 0 | — | 成功 |
| 1 | `Internal` / `Io` / `Json` / `Discover` / `Daemon` / `Cache` / `Embed` / `Lsp` / `Index`（部分） | 内部错误、I/O、JSON 序列化、能力引导失败等 |
| 2 | `InvalidInput` / `ProjectNotFound` / `Query` / `Trace` / `Storage` / `Resolve` / `Phase` | 无效输入、项目不存在、查询 / 追踪 / 存储 / 解析错误 |
| 4 | `NotFound` | NotFound、数据库损坏（`Index` 的 `DatabaseCorrupt` 同样映射 4） |
| 5/6 | `--fresh` 删除失败 | 旧 DB 文件删除失败的专用码 |

`KitError` 经 `kit_exit_code` 独立映射。`IndexError` 的典型变体包括 `PathNotFound`（路径不存在）等。

---

## 🎛️ 特性门控

| 模块 / API | 所需 feature | 说明 |
|------------|--------------|------|
| `parse`（对应语言） | `lang-c` … `lang-verilog` | 21 个语言 feature，至少启用其一 |
| `analysis` | `analysis` | 死代码 / 架构概览 / 复杂度 / 社区 / 跨服务 |
| `diagram` | `diagram`（含 `analysis`） | 架构图与语义 Delta |
| `embed` | `embed` | 向量嵌入语义搜索 |
| `lsp` | `lsp` | LSP 增强解析 |
| `cache` | `cache` | 查询缓存 |
| `daemon` | `daemon` | 文件监视守护进程 |
| `service`（CLI） | `cli` | CLI 二进制必需 |
| `service`（MCP） | `mcp` | MCP 服务器 |

编译期保证：`src/lib.rs` 顶部的 `compile_error!` 断言至少启用一个 `lang-*` 或 `lsp` feature，否则以明确信息拒绝编译。非 `full` 组合下部分代码路径编译但不执行（`allow(dead_code, ...)`），严格 lint 只在 `full` 下执行。

---

## 💡 使用示例

改编自 [`examples/src/bin/basic_indexing.rs`](../examples/src/bin/basic_indexing.rs)：

```rust
use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;

fn main() {
    let db_path = std::path::Path::new(".codenexus/demo.lbug");

    // 1. 索引：写内嵌示例源码到临时目录后索引
    let facade = IndexFacade::new(db_path).expect("IndexFacade::new");
    let result = facade
        .index(std::path::Path::new("src"), "demo-project", true)
        .expect("index");

    println!("Files indexed: {}", result.files_indexed);
    println!("Nodes created: {}", result.nodes_created);
    println!("Edges created: {}", result.edges_created);
    println!("Project ID:    {}", result.project_id);

    // 2. 查询：Cypher 列出函数
    let query = QueryFacade::new(db_path).expect("QueryFacade::new");
    let qr = query
        .cypher("MATCH (f:Function) RETURN f.name, f.qualifiedName LIMIT 10")
        .expect("cypher failed");
    for row in &qr.rows {
        let name = row[0].as_str().unwrap_or("?");
        let qn = row[1].as_str().unwrap_or("?");
        println!("  {name} @ {qn}");
    }
}
```

更多可运行示例见 `examples/` 目录（`cypher_query`、`symbol_search`、`call_tracing`、`impact_analysis`、`export_import`），运行方式见 [README · 示例](../README.md#-示例)。

---

## 📚 相关文档

| 文档 | 内容 |
|------|------|
| [📖 用户指南](USER_GUIDE.md) | CLI 全部子命令的使用教程 |
| [🏗️ 架构文档](ARCHITECTURE.md) | 模块划分、索引管线与图模型 |
| [⚡ 性能指南](PERFORMANCE.md) | 基准数据与调优建议 |
| [🔒 安全文档](SECURITY.md) | 安全策略与漏洞报告 |
