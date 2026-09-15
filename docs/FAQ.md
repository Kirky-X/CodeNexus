# ❓ CodeNexus FAQ

本页汇总 CodeNexus 的常见问题与解答，按主题分组。没有找到答案？欢迎前往 [GitHub Issues](https://github.com/Kirky-X/codenexus/issues) 提问。

## 📋 目录

- [通用问题](#-通用问题)
- [安装与构建](#-安装与构建)
- [使用与命令](#️-使用与命令)
- [解析与语言支持](#-解析与语言支持)
- [性能与内存](#-性能与内存)
- [故障排查](#-故障排查)

---

## 🧭 通用问题

### ❓ 什么是 CodeNexus？

**CodeNexus** 是一个多语言代码知识图谱工具。它把源代码仓库索引为属性图：

| 能力 | 说明 |
|:-----|:-----|
| **多语言解析** | tree-sitter，默认 `full` 预设 21 种语言 |
| **图存储** | LadybugDB，44 种节点类型 + 30 种边类型 |
| **查询分析** | Cypher 子集查询、调用链/数据流追踪、影响分析、语义搜索 |
| **智能体集成** | MCP 服务器（8 个工具）+ setup 自动接入 Claude Code / Cursor / Codex |

**了解更多**：[用户指南](USER_GUIDE.md)。

### ❓ CodeNexus 适合什么场景？

| 场景 | 推荐命令 |
|------|----------|
| 接手陌生代码库，想快速看清结构 | `index` → `architecture` → `context --enhanced true` |
| 评估一次改动的影响面 | `detect_changes`、`impact` |
| 清理无人调用的函数 | `dead_code` |
| API 契约审查 | `route_map`、`shape_check`、`api_impact` |
| 让 AI 智能体理解代码库 | `setup` + `mcp` |
| 团队共享索引 | `export` / `import` |

### ❓ CodeNexus 是库还是工具？

两者都是。`src/main.rs` 是 CLI 二进制（`codenexus`），`src/lib.rs` 暴露公共 API（`IndexFacade` / `QueryFacade` / `TraceFacade` 等），可作为 Rust crate 嵌入你自己的程序——见 [📘 API 参考](API_REFERENCE.md)。

---

## 📦 安装与构建

### ❓ 支持哪些 Rust 版本？

MSRV 为 **1.97.1**（`Cargo.toml` `rust-version`，`clippy.toml` `msrv` 同步）。CI 工具链当前锁定 1.95（见 `.github/workflows/ci.yml`）。格式化需要 nightly 工具链（`rustfmt.toml` 使用 nightly-only 选项）。

### ❓ 只想要部分语言怎么办？

用 feature 预设或单语言裁剪：

```bash
cargo build --release --no-default-features --features minimal   # 仅 Rust
cargo build --release --no-default-features --features core      # C + Rust + Python
cargo build --release --no-default-features --features lang-c    # 仅 C
```

完整 Feature 清单见 [README · Feature 开关](../README.md#-构建预设与-feature-开关)。

### ❓ `cargo install codenexus` 链接失败怎么办？

两条已知环境问题：

1. **`undefined symbol: std::to_chars(..., _Float128, ...)`**（openEuler / CentOS 等 GCC ≤ 12 系统）：预编译 LadybugDB 依赖较新 `libstdc++`。执行 `LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus` 强制源码编译（需 `cmake`）。
2. **mold 链接器报 `undefined symbol: __cpu_model`**：v0.3.7 起已修复（`build.rs` 静态链接 `libgcc.a`），旧版本请升级。

### ❓ 数据库文件存在哪里？

默认 `.codenexus/<项目名>.lbug`（`<项目名>` 取 `--name`，缺失时取 `--path` 目录名），可用全局 `--db` 覆盖。v0.3.12 起支持 `--fresh true` 删除旧 DB 回收死空间（多次 `--force` 会让 DuckDB 死空间累积，曾有 6.6 MB 源码膨胀到 35 GB DB 的案例）。

---

## 🛠️ 使用与命令

### ❓ 为什么所有参数都必须写成长选项？

CLI 是**严格 flag 风格**：没有位置参数，源码中 `String`/`u32`/`bool` 类型（非 `Option<T>`）的参数映射为无默认值的必填 flag，布尔必须显式传值。例如：

```bash
# ✗ 错误：位置参数
codenexus query "MATCH (f:Function) RETURN f.name LIMIT 10"

# ✓ 正确
codenexus query --cypher "MATCH (f:Function) RETURN f.name LIMIT 10"

# ✓ 布尔显式传值
codenexus index --path ./myrepo --name myrepo --ram_first true
```

完整约定见 [用户指南 · 核心约定](USER_GUIDE.md#-核心约定)。

### ❓ `--project` 传项目名还是项目 id？

都可以。所有带 `--project` 的命令经 `resolve_project_id` 解析：匹配已存项目 `name` 则使用其规范 `id`，否则按原始 project id 处理。仅在有文档说明处才可传 `--project ""` 关闭过滤。

### ❓ 命令输出的 stderr 里有警告正常吗？

正常。每次连接会打印 `inklog ... Failed to set log crate logger`、`storage::connection - skipping unsupported DDL statement` 等良性警告。需要干净 JSON 时：`codenexus query ... 2>/dev/null`。

### ❓ 怎么知道命令失败的原因和退出码？

所有错误经 `CodeNexusError` 收敛并映射稳定退出码：`0` 成功、`1` 内部错误、`2` 无效输入 / 项目不存在 / 查询错误、`4` NotFound / 数据库损坏。错误输出为结构化诊断回执（稳定规则码 + 证据 + 修复话术）。详见 [API 参考 · 错误类型](API_REFERENCE.md#-错误类型与退出码)。

### ❓ MCP 模式下有哪些工具？

8 个：`query`、`trace`、`impact`、`search`、`context`、`architecture`、`diagram`、`arch_diff`。与 CLI 命令共用同一套 `#[forge]` 定义，参数语义一致。用 `codenexus setup` 自动写入 Claude Code / Cursor / Codex 的 MCP 配置。

---

## 🔤 解析与语言支持

### ❓ 支持哪些语言？

默认 `full` 预设 21 种：C、Rust、Fortran、Python、TypeScript、Go、Java、C++、JavaScript、Ruby、Haskell、OCaml、Scala、PHP、C#、Bash、HTML、CSS、JSON、Regex、Verilog。各语言提取的节点/边类型见 [README · 支持语言相关章节](../README.md#️-架构) 与 [架构文档](ARCHITECTURE.md)。

### ❓ 为什么死代码检测会误报？

tree-sitter 不展开 procedural macro，三类已知盲区：

| 盲区 | 表现 | 缓解 |
|------|------|------|
| `#[derive(...)]` 生成的 impl 方法 | 被判为死代码 | 已知误报，人工确认 |
| 宏入口（`#[tokio::main]`、axum `#[route]` 等） | CALLS 边不可见 | `attribute_entries` 配置将 6 类属性标记视为入口 |
| `#[test]` / `#[rstest]` 测试函数 | 宏调用不可见 | `test_patterns` 名字 glob + `TEST_ATTRIBUTE_MARKERS` 属性识别 |

另：const fn 在 const 上下文的调用不产生 CALLS 边；Bash 的 `trap cleanup EXIT` 与字符串拼接间接调用以 Medium 置信度上报，删除前需人工确认。

### ❓ axum 项目的路由为什么扫不全？

`extract_axum_routes`（v0.3.8 起）支持编程式 `Router::new().route("/x", get(h))`，但仅识别裸标识符 handler；路径限定（`get(crate::module::handler)`）与闭包 handler 会被跳过，需人工核查。纯宏注册的路由串（宏参数而非独立字符串字面量）对 `cross_service` 不可见。

### ❓ 非 ASCII 路径/项目名支持吗？

支持。中文/日文/韩文目录与文件名、Unicode 项目名、非 ASCII DB 路径均有专门测试覆盖（`tests/non_ascii_path_test.rs`）；`i18n` feature（ICU4X case folding + NFC 规范化）含于 `full` 预设。

---

## ⚡ 性能与内存

### ❓ 大仓库索引会不会 OOM？

v0.3.10–v0.3.12 落地了 L1–L7 七层内存防线（内存预算、图迭代器视图、流式 CSV、mpsc 并发上限、自适应降级、管线流式化、buffer_pool 封顶），70 GB 主机上的索引峰值内存从约 60 GB 降至约 4 GB。详见 [⚡ 性能指南](PERFORMANCE.md)。

### ❓ 什么时候用 `--ram_first`？

RAM 优先模式把源码 LZ4 压缩进内存再单次批量入库，减少 LadybugDB 写放大。**建议源码 ≥ 1 GB 的仓库使用**（ADR-024）；1000 文件小仓库的对比实测中 ram-first 反而更耗内存（568 MB 对 450 MB），属预期行为。

### ❓ 查询/追踪性能有保证吗？

PRD SLO：Cypher 查询 P99 ≤ 200 ms、追踪 P99 ≤ 500 ms。Criterion 基准套件持续监控（`query_bench`、`trace_bench`），实测基线与复现方法见 [⚡ 性能指南](PERFORMANCE.md)。

### ❓ 索引越来越慢 / DB 文件越来越大怎么办？

重复 `--force` 索引会让 DuckDB 死空间累积。v0.3.12 的 `--fresh true` 在索引前删除旧 DB 文件彻底回收；read-only 连接的 `max_db_size` 已提升至 4 TiB cap，>16 GiB 的大库查询不再崩溃。

---

## 🔧 故障排查

### ❓ `lsp_goto_def` / `lsp_hover` 报 exit 2？

`LSP communication error: server connection closed` 通常是对应语言服务进程启动即退出的环境/配置问题，不是工具缺陷。确认 LSP server（rust-analyzer、pyright、clangd、gopls、ts-lang-server、fortls、jdtls）已安装并可手动启动。LSP 按需启动：纯 Rust 仓库只启动 rust-analyzer。

### ❓ `query` 不传 `--db` 就失败？

v0.3.8 起：`.codenexus/` 中恰好一个 `.lbug` 文件时自动选中；存在多个时必须显式 `--db`。

### ❓ 符号查询报 `ambiguous symbol`？

歧义消解策略是「自动选择唯一匹配；无法唯一确定时报错」（exit 2）。用更精确的符号名（如限定名）、`--project` 过滤或 `context` 命令收窄范围。

### ❓ Cypher 查询报错或语法不支持？

查询走 LadybugDB 的 Cypher **子集**：不支持 `UNION` / `UNION ALL`，不支持多标签 `WHERE (n:A OR n:B)` 表达式。需要多标签时写成两条查询在应用层合并。支持的语法以 `query --cypher` 实测与 [📘 API 参考](API_REFERENCE.md#queryfacade查询与搜索) 为准。

### ❓ 如何报告 Bug？

1. 收集信息：`codenexus --version`、Rust 版本、操作系统、完整命令、完整错误输出、最小复现仓库。
2. 前往 [GitHub Issues](https://github.com/Kirky-X/codenexus/issues/new) 提交（Conventional Commits 风格的标题有助于快速分诊）。
3. 安全漏洞**不要**公开提交——发邮件至 **security@kirky-x.dev**，见 [🔒 安全文档](SECURITY.md)。
