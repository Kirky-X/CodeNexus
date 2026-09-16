# 📖 CodeNexus 用户指南

**CodeNexus** 将源代码仓库索引为可查询的知识图谱：tree-sitter 多语言解析、LadybugDB 图存储，支持 Cypher 查询、调用链追踪、影响分析、语义搜索与多智能体 MCP 集成。本指南从安装入门一路走到全部 29 个子命令（外加 `mcp` 服务模式）的用法、复杂度分析与死代码检测详解，以及故障排查。

## 📋 目录

- [简介](#-简介)
- [安装与准备](#-安装与准备)
- [五分钟上手](#-五分钟上手)
- [核心约定](#-核心约定)
- [命令详解](#-命令详解)
  - [索引与项目管理](#-索引与项目管理)
  - [查询与搜索](#-查询与搜索)
  - [追踪与影响分析](#-追踪与影响分析)
  - [代码演化管理](#-代码演化管理)
- [复杂度分析](#-复杂度分析)
- [死代码检测](#-死代码检测)
- [API 审查工具包](#-api-审查工具包)
- [架构图与语义 Delta](#-架构图与语义-delta)
- [多智能体集成](#-多智能体集成)
- [环境变量](#-环境变量)
- [配置文件](#️-配置文件)
- [故障排查](#-故障排查)
- [延伸阅读](#-延伸阅读)

---

## 🎯 简介

本指南将带您掌握：

| 内容 | 说明 |
|:-----|:-----|
| **快速上手** | 5 分钟完成安装并索引第一个仓库 |
| **全部命令** | 29 个子命令（+ `mcp` 服务模式）的参数与示例 |
| **深度分析** | 复杂度分析、死代码检测的指标与阈值 |
| **智能体集成** | MCP 服务、setup 自动接入与 hook |

> 💡 **提示**：CodeNexus 是 CLI 工具，不需要写代码即可使用；Rust 开发者也可以将其作为库 crate（`codenexus`）嵌入自己的程序，见 [📘 API 参考](API_REFERENCE.md)。

---

## 📦 安装与准备

```bash
# 从 crates.io 安装（默认 full 预设，含全部 21 语言 + 所有功能）
cargo install codenexus

# 或从源码构建
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo install --path .
```

| 前置条件 | 说明 |
|:---------|:-----|
| Rust 工具链 | MSRV 1.97.1（`Cargo.toml` `rust-version`；仅源码构建需要） |
| `libstdc++` | GCC ≤ 12 的系统需 `LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus`（另需 `cmake`） |
| 磁盘空间 | 数据库默认写入 `<项目>/.codenexus/<项目名>.lbug`，随仓库规模增长 |
| git | `detect_changes`、`diagram --repo_root` 源码证据等命令依赖 |

构建预设：`minimal`（仅 Rust）/ `core`（C + Rust + Python）/ `full`（默认，21 语言 + 全部功能）。完整 Feature 清单见 [README · 构建预设与 Feature 开关](../README.md#-构建预设与-feature-开关)。

---

## 🚀 五分钟上手

```bash
# 1. 索引一个仓库（--name 决定数据库名与项目名）
codenexus index --path /path/to/project --name myproject

# 2. 看索引状态
codenexus status
codenexus list

# 3. Cypher 查询
codenexus query --cypher "MATCH (f:Function) RETURN f.name LIMIT 10"

# 4. 追踪调用链（追踪 main 的调用关系，深度 5）
codenexus trace --symbol main --trace_type calls --depth 5 --path_filter "" --detect_cycles false --cross_service false

# 5. 影响分析（改一个函数会波及谁）
codenexus impact --symbol parse_function --depth 3 --edge_types "" --max_depth 0 --include_tests false

# 6. 搜索符号
codenexus search --text "parse" --limit 20 --mode exact --fulltext false --project ""
codenexus search --text "get.*user" --mode regex --fulltext false --project ""
codenexus search --text "authentication logic" --fulltext true --project ""

# 7. 看某个符号的 360° 上下文
codenexus context --symbol main --depth 1 --project "" --enhanced false
codenexus context --symbol main --project myproject --enhanced true
```

每条命令输出单个 JSON 对象/数组到 stdout（`daemon`、`hook`、`mcp` 为流式/常驻命令除外）。

---

## 🧭 核心约定

以下约定适用于所有子命令（与 [`skill/SKILL.md`](../skill/SKILL.md) 对 v0.3.12 的校验一致）：

- **无位置参数**。一切都是命名 flag：`codenexus query "MATCH ..."` 会失败，必须写 `codenexus query --cypher "MATCH ..."`。
- **布尔参数显式传值**：`--force true`、`--fresh true`（仅 `index`）、`--apply true`、`--cross_service false`（仅 `trace`）、`--embed false`、`--ram_first false`、`--enhanced false`、`--include_tests false`（`impact`）。
- **`--project` 接受名称或 id**：所有带 `--project <VALUE>` 的命令经 `resolve_project_id`（`src/service/project.rs`）解析：匹配已存项目 `name` 则用其规范 `id`，否则按原始 project id 处理。仅在有文档明确说明处才可传 `--project ""` 关闭过滤。
- **可选参数自带默认值**：高频命令的"可选"参数已内置默认值，无需传占位 flag——`trace`：`--depth 5`、`--path_filter ""`、`--detect_cycles false`、`--cross_service false`；`search`：`--limit 50`、`--mode ""`、`--fulltext false`、`--project ""`；`impact`：`--depth 3`、`--edge_types ""`、`--max_depth 0`、`--include_tests false`；`context`：`--depth 1`、`--project ""`、`--enhanced false`；`index` 的 `--force`/`--lsp`/`--embed`/`--ram_first` 默认 `false`；`detect_changes --mode` 默认 `unstaged`；其余（complexity 阈值等）见各命令 `--help`。默认值可用 `.codenexus/config.json` 按项目覆盖（见 [配置文件](#️-配置文件)）。
- **全局选项**：`--db <DB_PATH>` 与 `--debounce-ms <MS>`（默认 `2000`，仅 daemon 相关）适用于每个命令。
- **默认数据库路径**（省略 `--db` 时）：`.codenexus/<project>.lbug`，`<project>` 取 `index`/`daemon` 的 `--name`（优先）或 `--path` 目录名，兜底 `codenexus`。v0.3.7 起，无 `--name`/`--path` 可用且 `.codenexus/` 中恰好只有一个 `.lbug` 文件时自动选中该文件；存在多个时回退到 `codenexus` 并要求显式 `--db`。配置文件 `general.db` 可为非标准路径兜底。
- **stderr 噪音**：所有日志（含 info 级与索引逐阶段进度行 `[codenexus] [i/n] <phase> ok`）都输出到 stderr，stdout 永远只有命令本身的 JSON，重定向无需过滤；日志文件在 `.codenexus/logs/codenexus.log`。stderr 中 `inklog ... Failed to set log crate logger` 等警告属良性输出。
- **退出码契约**（`src/service/error.rs`）：`0` 成功；`1` 内部错误 / I/O / JSON / Kit 错误；`2` 无效输入 / 项目不存在 / 查询、追踪、存储、解析错误；`4` NotFound / 数据库损坏；`--fresh` 删除失败另有 5/6 退出码。

---

## 🛠️ 命令详解

### 📥 索引与项目管理

| 命令 | 关键参数 | 说明 |
|------|----------|------|
| `index` | `--path` `--name` `--force` `--fresh` `--ram_first` | 索引仓库到知识图谱。增量模式经 SHA-256 哈希比对只解析变更文件；`--force true` 全量重建；`--fresh true` 索引前删除旧 DB 文件回收 DuckDB 死空间；`--ram_first true` 启用 LZ4 内存压缩批量入库 |
| `daemon` | `--path` `--name` | 文件监视守护进程，检测到变更自动增量索引；SIGTERM/SIGINT 优雅退出；`--debounce-ms` 全局选项控制去抖（默认 2000ms） |
| `status` | — | 查看当前索引状态 |
| `list` | — | 列出所有已索引项目 |
| `clean` | `--project` | 删除项目及其索引 |
| `export` | `--output` `--project` | 导出 LadybugDB 转储为 zstd 压缩制品（`.graph.zst`），数据库由全局 `--db` 指定 |
| `import` | `--input` `--reindex` `--path` `--name` | 导入制品；`--reindex true` 时配合 `--path`/`--name` 增量补齐本地差异 |

### 🔎 查询与搜索

| 命令 | 关键参数 | 说明 |
|------|----------|------|
| `query` | `--cypher` | 对图执行 Cypher 子集查询。注意：该子集不支持 `UNION` / `UNION ALL`，也不支持多标签 `WHERE (n:A OR n:B)` 表达式 |
| `search` | `--text` `--mode` `--limit` `--fulltext` `--project` | 5 种结构化模式 `exact` / `regex` / `fuzzy` / `graph` / `multi`；`--fulltext true` 走 BM25 全文检索（`matchReason` 为 `bm25 fts` / `bm25f weighted`） |
| `search_by_type` 场景 | `--text` 传类型名 | `search` 按类型过滤返回对应 label 的符号 |
| `context` | `--symbol` `--depth` `--project` `--enhanced` | 360° 符号视图：入度调用/导入、出度调用、所属流程；`--enhanced true` 追加多维 SymbolContext（LSP 增强信息等） |

### 🕸️ 追踪与影响分析

| 命令 | 关键参数 | 说明 |
|------|----------|------|
| `trace` | `--symbol` `--trace_type` `--depth` `--path_filter` `--detect_cycles` `--cross_service` | `--trace_type` 取 `calls` / `data`（调用链 / 数据流）；`--path_filter` 支持 glob（如 `/src/api/**`）；`--detect_cycles true` 检测环；`--cross_service true` 关联跨服务调用 |
| `impact` | `--symbol` `--depth` `--edge_types` `--max_depth` `--include_tests` | 影响半径分析（上游调用者子图）+ `risk_assessment` 风险评估；`--edge_types "CALLS,IMPLEMENTS,USES_TYPE"` 按边类型过滤；子图节点上限 5000（`MAX_NODES_LIMIT`） |
| `detect_changes` | `--path` `--mode` | git diff → 受影响符号 + `risk_level`，提交前评估改动影响面 |
| `rename` | `--from` `--to` `--path` `--apply` | 高置信度图编辑 + 文本搜索编辑；`--apply false` 为 dry-run（默认推荐先跑 dry-run 审查） |

### 🧪 代码演化管理

| 命令 | 关键参数 | 说明 |
|------|----------|------|
| `dead_code` | `--project` `--entry` `--check_exported` `--check_ffi` `--edge_types` `--check_dynamic_dispatch` | 死代码检测，详见 [死代码检测](#-死代码检测) |
| `architecture` | `--project` | 架构概览：模块边界 + 依赖方向 + 分层 + 跨服务依赖（`ArchitectureOverview` JSON），详见 [架构文档](ARCHITECTURE.md) |
| `complexity` | `--project` + 阈值参数 | AST 复杂度分析，详见 [复杂度分析](#-复杂度分析) |
| `community` | `--project` `--resolution` | 社区检测（Leiden 模块度优化）；`--resolution` 省略时默认 0.5 |
| `cross_service` | `--project` `--protocol` | 跨服务调用链检测：HTTP REST / gRPC / GraphQL / 消息队列 / 事件总线；`--protocol` 省略时检测所有协议 |

---

## 📊 复杂度分析

`complexity` 子命令对项目内所有函数计算 AST 复杂度指标，输出 JSON（含 `complexity` 数组与 `summary` 统计）。

### 指标

| 指标            | 字段                    | 说明                                                                    |
| --------------- | ----------------------- | ----------------------------------------------------------------------- |
| 圈复杂度        | `cyclomatic`            | McCabe 1976，含分支节点 + 显式出口（return/break/continue）+ 逻辑运算符 |
| 认知复杂度      | `cognitive`             | 按嵌套层级加权的 SonarQube 风格复杂度                                   |
| 嵌套深度        | `nesting_depth`         | 分支节点最大嵌套层数                                                    |
| 函数长度        | `function_length`       | 起止行差 +1                                                             |
| Halstead 复杂度 | `halstead`              | Halstead 1977：`n1/n2/N1/N2/volume/difficulty/effort/delivered_bugs`    |
| 可维护性指数    | `maintainability_index` | Microsoft 2007 修订公式，0-100（越高越好）                              |
| 时间复杂度      | `time_complexity`       | AST 模式估算：O(1)/O(log n)/O(n)/O(n log n)/O(n^2)/O(n^3)/O(2^n)        |
| 空间复杂度      | `space_complexity`      | 分配模式识别：O(1)/O(n)/O(n^2)                                          |

每项指标按阈值分为 Green / Yellow / Red / Critical 四级，`overall_severity` 取最高级别。

### 阈值 CLI 参数

| 参数                                                                                                        | 说明                                |
| ----------------------------------------------------------------------------------------------------------- | ----------------------------------- |
| `--cyclomatic_green <N>` / `--cyclomatic_yellow <N>` / `--cyclomatic_red <N>`                               | 圈复杂度阈值                        |
| `--cognitive_green <N>` / `--cognitive_yellow <N>` / `--cognitive_red <N>`                                  | 认知复杂度阈值                      |
| `--nesting_green <N>` / `--nesting_yellow <N>` / `--nesting_red <N>`                                        | 嵌套深度阈值                        |
| `--func_length_green <N>` / `--func_length_yellow <N>` / `--func_length_red <N>`                            | 函数长度阈值                        |
| `--halstead_volume_green <N>` / `--halstead_volume_yellow <N>` / `--halstead_volume_red <N>`                | Halstead volume 阈值                |
| `--maintainability_green <N>` / `--maintainability_yellow <N>` / `--maintainability_red <N>`                | 可维护性指数阈值（越高越好）        |
| `--time_complexity_green <O(...)>` / `--time_complexity_yellow <O(...)>` / `--time_complexity_red <O(...)>` | 时间复杂度阈值                      |
| `--space_complexity_yellow <O(...)>` / `--space_complexity_red <O(...)>`                                    | 空间复杂度阈值（3 级，无 Critical） |

`<O(...)>` 取值：时间 `O(1)` / `O(log n)` / `O(n)` / `O(n log n)` / `O(n^2)` / `O(n^3)` / `O(2^n)`，空间 `O(1)` / `O(n)` / `O(n^2)`。所有阈值与标志参数（含 `--red_only`/`--sort_by_severity`）均可省略；省略时 `u32` 阈值与 `O(...)` 字符串阈值走默认值（见下表），`bool` 标志默认 `false`。

### 默认阈值

| 指标             | Green    | Yellow | Red    |
| ---------------- | -------- | ------ | ------ |
| cyclomatic       | 10       | 20     | 25     |
| cognitive        | 10       | 15     | 20     |
| nesting          | 3        | 5      | 6      |
| func_length      | 30       | 100    | 200    |
| halstead_volume  | 100      | 1000   | 8000   |
| maintainability  | 85       | 65     | 25     |
| time_complexity  | O(log n) | O(n)   | O(n^2) |
| space_complexity | —        | O(1)   | O(n)   |

> `maintainability` 阈值含义反转：MI 越高越好，`value >= green → Green`，`value >= yellow → Yellow`，`value >= red → Red`，否则 `Critical`。`space_complexity` 只有 3 级（Green/Yellow/Red），无 Critical。

### 示例

```bash
# 默认阈值分析
codenexus complexity --project myproject

# 自定义圈复杂度阈值（green=5, yellow=10, red=15）
codenexus complexity --project myproject --cyclomatic_green 5 --cyclomatic_yellow 10 --cyclomatic_red 15

# 仅显示 Red 和 Critical 级函数并按严重度排序
codenexus complexity --project myproject --red_only true --sort_by_severity true

# 自定义时间复杂度阈值（green=O(1), yellow=O(n log n), red=O(n^2)）
codenexus complexity --project myproject --time_complexity_green "O(1)" --time_complexity_yellow "O(n log n)" --time_complexity_red "O(n^2)"
```

---

## 💀 死代码检测

`dead_code` 子命令基于工作列表（worklist）可达性传播算法识别死代码：从种子集合（入口函数 / 导出函数 / FFI 入口 / 测试函数 / trait impl 方法 / `pub use` 重导出目标 / 属性标记入口）出发 BFS 传播 liveness 到不动点，未被传播覆盖的 Function/Method 节点判定为死代码。每条死代码记录携带 High/Medium/Low 置信度分层，输出含 `indexed_commit` / `current_head` / `is_stale` 三字段标识索引新鲜度。

### 配置项

通过 `DeadCodeConfig`（service 层 CLI 参数透传）控制检测行为，关键字段：

| 字段                     | 默认值                                                                                             | 说明                                                                                                                                                                                            |
| ------------------------ | -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `entry_patterns`         | `["main", "Main", "__main__", "wmain", "WinMain", "DLLMain"]`                                      | 入口函数名 glob 模式，匹配的 Function 视为 live 种子                                                                                                                                            |
| `test_patterns`          | 8 项：`test_*` / `*_test` / `*_spec` / `it_*` / `sec_*` / `snap_*` / `perf_*` / `bench_*`          | 测试函数名 glob 模式，匹配的 Function 视为 live 种子                                                                                                                                            |
| `attribute_entries`      | 6 项：`#[tool` / `#[forge` / `#[tokio::main` / `#[rocket::main` / `#[actix::main` / `#[axum::main` | 函数 `signature` 字段子串匹配，命中则视为宏展开合成的入口（tree-sitter 不展开宏，CALLS 边不可见）。`#[test]` / `#[bench]` 由 `test_patterns` 名字 glob 覆盖，故不在 `attribute_entries` 内      |
| `check_dynamic_dispatch` | `true`                                                                                             | 检测 qualified_name 中的 `#<TypeName>` disambiguator（如 `fmt#Display`），命中则视为 trait impl 方法（vtable 动态分发，无静态 CALLS 边），跳过死代码判定。设为 `false` 时按保守模式判定为死代码 |
| `check_exported`         | `true`                                                                                             | `isExported=true` 的 Function/Method 视为外部可见，跳过死代码判定                                                                                                                               |
| `check_ffi`              | `true`                                                                                             | signature 含 `extern "C"` / `#[no_mangle]` 的 Function 视为 FFI 入口，跳过死代码判定                                                                                                            |
| `edge_types`             | `CALLS` / `FfiCalls` / `Implements` / `Usage` / `Tests` / `UsesType` / `HttpCalls` / `AsyncCalls`  | 参与可达性传播的边类型白名单                                                                                                                                                                    |

### 示例

```bash
# 默认配置检测
codenexus dead_code --project myproject --entry "" --check_exported true --check_ffi true --edge_types ""

# 自定义 edge_types（仅 CALLS + IMPLEMENTS）
codenexus dead_code --project myproject --edge_types "CALLS,IMPLEMENTS"

# 保守模式：禁用 trait impl 识别
codenexus dead_code --project myproject --check_dynamic_dispatch false
```

> **限制**：tree-sitter 不展开 procedural macro（`#[derive(Serialize)]` 等），derive 生成的 impl 方法会被判定为死代码（已知误报）。const fn 在 const 上下文的调用也不被 tree-sitter 识别为 CALLS 边。Bash 语言的 `trap cleanup EXIT`（信号触发）与经字符串拼接 / 变量展开的间接调用对 tree-sitter 不可见，会以 Medium 置信度上报，删除前需人工确认。

---

## 🧭 API 审查工具包

`api-review` feature 提供 4 个命令（axum 项目支持 `Router::new().route("/x", get(h))` 编程式路由提取）：

| 命令 | 说明 |
|------|------|
| `route_map` | HTTP 路由映射，输出 API 端点清单（Route 节点 + HANDLES_ROUTE 边） |
| `shape_check` | API 形状检查，验证请求/响应结构一致性 |
| `api_impact` | API 变更影响分析；`--endpoint` 省略时分析所有端点 |
| `tool_map` | 工具映射，输出 MCP 工具清单 |

> 已知限制：`extract_axum_routes` 仅识别裸标识符 handler（`get(handler)`）；路径限定（`get(crate::module::handler)`）与闭包（`get(|| { ... })`）handler 会被跳过，需人工核查。`cross_service` 对纯宏注册路由（路由串是宏参数而非独立字符串字面量）的 axum 项目可能返回空 `[]`。

---

## 🖼️ 架构图与语义 Delta

`diagram` feature 提供两个命令（输出语义详见 [架构文档](ARCHITECTURE.md)）：

```bash
# 架构图导出：自包含交互式 HTML + BLAKE3 交付回执
codenexus diagram --project myproject --output ./arch.html

# showcase 档位：任何告警升级为错误并拒绝交付
codenexus diagram --project myproject --output ./arch.html --quality showcase

# 挂 Git 源码证据徽标（校验 origin URL 一致性）
codenexus diagram --project myproject --output ./arch.html --repo_root /path/to/project --repo_url https://github.com/you/yourrepo

# 架构语义 Delta：两个已索引项目对比
codenexus arch_diff --base_project v1 --head_project v2 --output ./delta.html
```

`diagram` 参数：`--quality`（`standard` 默认 / `showcase`）、`--repo_root`、`--repo_url`、`--title`、`--locale`（`en` / `zh-CN`）。组件类型（interface / service / storage / model）来自真实分层事实，不基于命名猜测。

`arch_diff` 输出 Before / Delta / After 三节 HTML 与机器回执 JSON（`<path>.receipt.json`，两文件原子成对写入）；每条变更携带 `kind`（`added`/`removed`/`changed`）、`changed_fields`（JSON Pointer）、`classification`（`topology` / `semantic`）。符号级行变化请使用 `detect_changes`。

---

## 🤖 多智能体集成

```bash
# 自动检测 Claude Code / Cursor / Codex 并写入 MCP 配置
codenexus setup

# 强制覆盖已有配置（跳过确认提示）
codenexus setup --force

# 输出 PreToolUse/PostToolUse JSON（exit 0，永不阻塞，适合作为智能体钩子）
codenexus hook

# 启动 stdio MCP 服务（10 个工具：query/trace/impact/search/context/architecture/diagram/arch_diff/dead_code/detect_changes）
codenexus mcp [--db <DB_PATH>]
```

`setup` 检测 `~/.claude/`、`~/.cursor/`、`~/.codex/` 三个目录判断已安装的智能体。MCP 工具与 CLI 命令共用同一套 `#[forge]` 定义（`src/service/`），每个工具的 description 内置参数语义与默认值说明；服务器以只读方式打开数据库，可与写入进程并存。

LSP 增强命令（`lsp` feature）：

| 命令 | 说明 |
|------|------|
| `lsp_goto_def` | LSP 定义跳转（rust-analyzer 等服务端集成） |
| `lsp_hover` | LSP 悬停信息（类型签名等语义增强） |

> LSP 服务不可用的环境中（语言服务进程启动即退出），`lsp_goto_def`/`lsp_hover` 可能以 exit 2（`LSP communication error: server connection closed`）失败——这是环境/配置问题，不是工具逻辑缺陷。LSP 服务按需启动：纯 Rust 仓库只启动 rust-analyzer。

---

## 🗂️ 配置文件

CodeNexus 支持 per-project 配置文件 `.codenexus/config.json`（与索引库同目录）。所有配置项的优先级为：**显式 CLI flag > 配置文件 > 内置默认值**；文件缺失时完全无感，格式错误只打 `[warn]` 并回退内置默认值，不会中断命令。

```json
{
  "general": {
    "db": ".codenexus/team.lbug",
    "debounce_ms": 2000,
    "verbose": false
  },
  "command_defaults": {
    "index": { "ram_first": true, "force": false },
    "complexity": { "cyclomatic_red": 25 },
    "trace": { "depth": 5 }
  }
}
```

| 字段 | 说明 |
|------|------|
| `general.db` | 省略 `--db` 且无法从 `--name`/`--path` 推导、也未发现唯一 `.lbug` 文件时使用的兜底数据库路径（典型用途：数据库放在非标准位置，免去每条读命令都带 `--db`） |
| `general.debounce_ms` | `--debounce-ms` 的兜底值（daemon 去抖毫秒数） |
| `general.verbose` | `--verbose` 的兜底值（daemon 每批文件事件的调试诊断） |
| `command_defaults.<命令>.<参数>` | 该子命令对应 flag 的默认值（参数必须注册在该命令上，值为字符串/数字/布尔）；显式传入的 flag 永远优先 |

> 💡 建议把 `.codenexus/` 加入项目 `.gitignore`：索引库（`.lbug`）、日志（`logs/codenexus.log`）与本配置文件都在这个目录下，其中前两者不应入库。

---

## ⚙️ 环境变量

CodeNexus 是 CLI 工具，自身不读取 `.env` 文件；模板见 [`.env.example`](../.env.example)，供 shell / systemd / supervisord / docker / direnv 消费：

| 变量 | 默认 | 说明 |
|------|------|------|
| `RUST_LOG` | `info` | 日志级别，支持单级别与按模块过滤（如 `codenexus=debug,sqlx=warn`） |
| `CODENEXUS_DB_PATH` | （注释态） | 可选的默认数据库路径，省略 `--db` 时生效 |
| `LBUG_BUILD_FROM_SOURCE` | 未设置 | 设为 `1` 时从源码编译 LadybugDB（老 `libstdc++` 系统需要） |

---

## 🔧 故障排查

| 症状 | 原因与处理 |
|------|-----------|
| 链接报 `undefined symbol: std::to_chars(..., _Float128, ...)` | 预编译 LadybugDB 二进制依赖较新 `libstdc++`（GCC ≤ 12 系统）。执行 `LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus`（需 `cmake`） |
| 使用 mold 链接器报 `undefined symbol: __cpu_model` | v0.3.7 已修复：`build.rs` 自动静态链接 `libgcc.a`；旧版本请升级 |
| `query`/`list` 未传 `--db` 失败 | v0.3.8 起自动扫描 `.codenexus/`：仅一个 `.lbug` 时自动选中，多个时需显式 `--db` |
| 死代码大量误报在 `#[test]` 函数上 | v0.3.8 起以 `TEST_ATTRIBUTE_MARKERS` 识别 `#[test]`/`#[tokio::test]`/`#[rstest]` 标记函数为入口；如仍出现请确认索引不是 stale（看 `is_stale` 字段） |
| `route_map`/`shape_check`/`cross_service`/`tool_map` 在 axum 项目返回空 | v0.3.8 起支持编程式 `Router::new().route(...)` 路由；宏注册路由仍不可见（见上文限制） |
| `lsp_goto_def`/`lsp_hover` exit 2 | 语言服务进程启动即退出，属环境/配置问题；确认对应 LSP server 已安装且可在终端手动启动 |
| 符号歧义报错 `ambiguous symbol 'xxx': N candidates` | 歧义消解策略：无法唯一确定时显式报错（exit 2）而非猜测；用更精确的符号名或 `--project` 收窄 |
| DQ-002 Duplicate FQN / DQ-004 Orphan edge 告警 | 数据质量回执（稳定规则码 + 证据 + 修复话术）；DQ-002/DQ-004 两类缺陷已在 v0.3.9 修复，出现新告警请按回执提示处理 |
| stderr 出现 `skipping unsupported DDL statement` | 良性日志；`2>/dev/null` 过滤获得干净 JSON |

---

## 📚 延伸阅读

| 文档 | 内容 |
|------|------|
| [📘 API 参考](API_REFERENCE.md) | 库 crate 公开 API 与 CLI/MCP 接口契约 |
| [🏗️ 架构文档](ARCHITECTURE.md) | 三层结构、索引管线、图模型与架构图命令语义 |
| [⚡ 性能指南](PERFORMANCE.md) | 基准数据、内存优化与调优建议 |
| [❓ FAQ](FAQ.md) | 常见问题解答 |
| [🔒 安全文档](SECURITY.md) | 漏洞报告流程与安全最佳实践 |
| [📐 架构设计文档（ADD）](ADD.md) | 架构决策记录 |
| [🗄️ 数据库设计文档（DDD）](DDD.md) | 图存储 Schema 设计 |
