# CodeNexus

<div align="center">

**基于 LadybugDB 与 tree-sitter 的多语言代码知识图谱工具**

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Rust Version](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org) [![Build](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml/badge.svg)](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml)

[English](README_EN.md) | 简体中文

</div>

## 简介

CodeNexus 将源代码仓库索引为可查询的知识图谱。它使用 [tree-sitter](https://tree-sitter.github.io/) 进行多语言语法解析，[LadybugDB](https://github.com/ladybugdb/ladybugdb) 进行图存储，支持符号追踪、影响分析和数据流分析。

支持 **8 种语言**：C、Rust、Fortran、Python、TypeScript、Go、Java、C++。

## 核心特性

| 特性 | 说明 |
|------|------|
| 多语言解析 | C / Rust / Fortran / Python / TypeScript / Go / Java / C++，基于 tree-sitter |
| 图数据库 | LadybugDB 图存储，44 种节点类型 + 24 种边类型 |
| 增量索引 | SHA-256 文件哈希比对，仅重新解析变更文件 |
| 并行解析 | Rayon 并行 + 线程局部 parser 池 |
| RAM 优先索引 | LZ4 压缩源码到内存，单次 `COPY FROM` 批量入库（`--ram-first`） |
| 符号追踪 | 调用链 (Calls) 与数据流 (DataFlows) 双向追踪 |
| 影响分析 | 变更影响半径分析，按深度分层 |
| 歧义消解 | 多匹配符号排序消解，支持 `--uid`/`--file`/`--kind` 收窄 |
| 置信度分层 | 每条边携带分层（SameFile / ImportScoped / Global）+ 0.0-1.0 分数 |
| 跨语言 FFI | C-Fortran bind(C)、Rust extern 等跨语言调用解析 |
| 团队制品 | `export`/`import` 压缩 `.graph.zst` 制品，共享索引 |
| 多智能体 MCP | `setup` 自动检测 Claude Code/Cursor/Codex；`hook` 输出 PreToolUse/PostToolUse JSON；`mcp` stdio 服务 |
| 文件监视 | 守护进程模式，自动增量索引（`daemon` feature） |
| 向量嵌入 | 可选的语义搜索（`embed` feature） |

## 安装

```bash
# 从源码构建
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo install --path .

# 或直接编译
cargo build --release
```

### Feature 开关

**预设**：`default = ["full"]`

| Feature | 默认 | 说明 |
|---------|------|------|
| `minimal` | — | 最小预设：仅 `lang-rust` |
| `core` | — | 核心预设：`lang-c` + `lang-rust` + `lang-python` |
| `full` | 启用 | 完整预设：`core` + Fortran/TypeScript/Go/Java/C++ + daemon/analysis/complexity/api-review/community/cross-service/lsp |
| `lang-c` | — | C 语言解析器（tree-sitter-c） |
| `lang-rust` | 启用 | Rust 语言解析器（tree-sitter-rust） |
| `lang-fortran` | — | Fortran 语言解析器（tree-sitter-fortran） |
| `lang-python` | — | Python 语言解析器（tree-sitter-python） |
| `lang-typescript` | — | TypeScript 语言解析器（tree-sitter-typescript） |
| `lang-go` | — | Go 语言解析器（tree-sitter-go） |
| `lang-java` | — | Java 语言解析器（tree-sitter-java） |
| `lang-cpp` | — | C++ 语言解析器（tree-sitter-cpp） |
| `daemon` | 启用 | 文件监视守护进程（notify + notify-debouncer-full） |
| `embed` | 关闭 | 向量嵌入语义搜索（reqwest HTTP + 本地 ONNX 推理） |
| `lsp` | 关闭 | LSP 增强解析（rust-analyzer 集成，语义类型增强） |
| `analysis` | 启用 | 死代码检测 + 架构概览（纯 Cypher 聚合） |
| `complexity` | 启用 | AST 复杂度分析（圈/认知/嵌套/长度/Halstead/可维护性/时间/空间复杂度，依赖 `analysis`） |
| `api-review` | 启用 | API 审查工具包（route-map/shape-check/api-impact/tool-map） |
| `community` | 启用 | 社区检测（Louvain 模块度优化，依赖 petgraph） |
| `cross-service` | 启用 | 跨服务调用链检测（HTTP 路由模式匹配） |

```bash
# 最小构建（仅 Rust，不含 daemon/analysis）
cargo build --release --no-default-features --features minimal

# 核心构建（C + Rust + Python）
cargo build --release --no-default-features --features core

# 单语言精简构建（例如仅 C）
cargo build --release --no-default-features --features lang-c

# 完整构建（默认，含所有语言 + 全部功能）
cargo build --release

# 含向量嵌入的构建
cargo build --release --features embed
```

## 快速开始

```bash
# 1. 索引一个代码仓库
codenexus index /path/to/project --name myproject

# 1b. RAM 优先索引（LZ4 内存压缩，适合中小仓库，更快）
codenexus index /path/to/project --name myproject --ram-first

# 2. 查询函数
codenexus query "MATCH (f:Function) RETURN f.name LIMIT 10"

# 3. 追踪调用链（支持歧义消解收窄）
codenexus trace main --type calls --depth 5
codenexus trace main --uid "proj.fn.main.1" --depth 5

# 4. 分析变更影响（按置信度过滤）
codenexus impact parse_function --depth 3
codenexus impact parse_function --depth 3 --min-confidence 0.7

# 5. 搜索符号
codenexus search "parse" --limit 20

# 6. 360° 符号上下文
codenexus context main

# 7. 检测 git diff 影响的符号
codenexus detect-changes /path/to/project

# 8. 重命名符号（图编辑 + 文本搜索，支持 --dry-run）
codenexus rename old_name new_name --dry-run

# 9. 导出 / 导入团队制品
codenexus export --db ./my.lbug --output team.graph.zst
codenexus import --input team.graph.zst --db ./shared.lbug

# 10. 多智能体 MCP 集成
codenexus setup                    # 自动检测智能体，写入 MCP 配置
codenexus hook                     # 输出 PreToolUse/PostToolUse JSON
codenexus mcp                      # stdio MCP 服务（JSON-RPC 2.0）

# 11. 查看索引状态
codenexus status

# 12. 启动文件监视守护进程
codenexus daemon /path/to/project --name myproject

# 13. 列出所有项目
codenexus list

# 14. 删除项目
codenexus clean myproject
```

## CLI 命令

| 命令 | 说明 |
|------|------|
| `index` | 索引代码仓库到知识图谱（`--ram-first` 启用 LZ4 内存模式） |
| `query` | 执行 Cypher 查询 |
| `trace` | 追踪符号的调用/数据流路径（`--uid`/`--file`/`--kind` 收窄） |
| `impact` | 分析符号变更的影响半径（`--min-confidence` 过滤） |
| `search` | 按名称或内容搜索符号（`--uid`/`--file`/`--kind` 收窄） |
| `context` | 360° 符号视图：入度调用/导入、出度调用、所属流程 |
| `detect-changes` | git diff → 受影响符号 + risk_level |
| `rename` | 高置信度图编辑 + 文本搜索编辑（`--dry-run`） |
| `export` | 导出 LadybugDB 转储 → zstd `codenexus.graph.zst` 制品 |
| `import` | 导入制品 → LadybugDB（可选 `--reindex` 增量补齐本地差异） |
| `setup` | 自动检测已安装的智能体（Claude Code/Cursor/Codex）并写入 MCP 配置 |
| `hook` | 输出 PreToolUse/PostToolUse JSON（exit 0，永不阻塞） |
| `mcp` | stdio MCP 服务（JSON-RPC 2.0，协议 2024-11-05） |
| `daemon` | 启动文件监视守护进程 |
| `status` | 查看索引状态 |
| `list` | 列出所有已索引项目 |
| `clean` | 删除项目及其索引 |
| `dead-code` | 死代码检测（未被调用的函数，`analysis` feature） |
| `architecture` | 架构概览（模块依赖图，`analysis` feature） |
| `complexity` | AST 复杂度分析（8 项指标 + 可配置阈值，`complexity` feature） |
| `api-route-map` | HTTP 路由映射（API 端点清单，`api-review` feature） |
| `api-shape-check` | API 形状检查（请求/响应结构验证，`api-review` feature） |
| `api-impact` | API 变更影响分析（`api-review` feature） |
| `api-tool-map` | 工具映射（MCP 工具清单，`api-review` feature） |
| `community` | 社区检测（Louvain 模块度优化，`community` feature） |
| `cross-service` | 跨服务调用链检测（HTTP 路由模式匹配，`cross-service` feature） |

## 复杂度分析（complexity）

`complexity` 子命令对项目内所有函数计算 AST 复杂度指标，输出 JSON（含 `complexity` 数组与 `summary` 统计）。

### 指标

| 指标 | 字段 | 说明 |
|------|------|------|
| 圈复杂度 | `cyclomatic` | McCabe 1976，含分支节点 + 显式出口（return/break/continue）+ 逻辑运算符 |
| 认知复杂度 | `cognitive` | 按嵌套层级加权的 SonarQube 风格复杂度 |
| 嵌套深度 | `nesting_depth` | 分支节点最大嵌套层数 |
| 函数长度 | `function_length` | 起止行差 +1 |
| Halstead 复杂度 | `halstead` | Halstead 1977：`n1/n2/N1/N2/volume/difficulty/effort/delivered_bugs` |
| 可维护性指数 | `maintainability_index` | Microsoft 2007 修订公式，0-100（越高越好） |
| 时间复杂度 | `time_complexity` | AST 模式估算：O(1)/O(log n)/O(n)/O(n log n)/O(n^2)/O(n^3)/O(2^n) |
| 空间复杂度 | `space_complexity` | 分配模式识别：O(1)/O(n)/O(n^2) |

每项指标按阈值分为 Green / Yellow / Red 三级，`overall_severity` 取最高级别。

### 阈值 CLI 参数

| 参数 | 说明 |
|------|------|
| `--cyclomatic-yellow <N>` / `--cyclomatic-red <N>` | 圈复杂度阈值 |
| `--cognitive-yellow <N>` / `--cognitive-red <N>` | 认知复杂度阈值 |
| `--nesting-yellow <N>` / `--nesting-red <N>` | 嵌套深度阈值 |
| `--func-length-yellow <N>` / `--func-length-red <N>` | 函数长度阈值 |
| `--halstead-volume-yellow <N>` / `--halstead-volume-red <N>` | Halstead volume 阈值 |
| `--maintainability-yellow <N>` / `--maintainability-red <N>` | 可维护性指数阈值（越高越好） |
| `--time-complexity-yellow <O(...)>` / `--time-complexity-red <O(...)>` | 时间复杂度阈值 |
| `--space-complexity-yellow <O(...)>` / `--space-complexity-red <O(...)>` | 空间复杂度阈值 |

`<O(...)>` 取值：时间 `O(1)` / `O(log n)` / `O(n)` / `O(n log n)` / `O(n^2)` / `O(n^3)` / `O(2^n)`，空间 `O(1)` / `O(n)` / `O(n^2)`。未设置的参数走默认值。

### 默认阈值

| 指标 | Yellow | Red |
|------|--------|-----|
| cyclomatic | 20 | 25 |
| cognitive | 15 | 20 |
| nesting | 5 | 6 |
| func_length | 100 | 200 |
| halstead_volume | 1000 | 8000 |
| maintainability | 65 | 85 |
| time_complexity | O(n) | O(n^2) |
| space_complexity | O(1) | O(n) |

> `maintainability` 阈值含义反转：MI 越高越好，`value >= red → Green`，`value >= yellow → Yellow`，否则 `Red`。

### 示例

```bash
# 默认阈值分析
codenexus complexity myproject

# 自定义圈复杂度阈值（yellow=10, red=15）
codenexus complexity myproject --cyclomatic-yellow 10 --cyclomatic-red 15

# 仅显示 Red 级函数并按严重度排序
codenexus complexity myproject --red-only --sort-by-severity

# 自定义时间复杂度阈值（yellow=O(n log n), red=O(n^2)）
codenexus complexity myproject --time-complexity-yellow "O(n log n)" --time-complexity-red "O(n^2)"
```

## 架构

```
┌─────────────────────────────────────────────┐
│                   CLI (clap)                 │
├─────────────────────────────────────────────┤
│  Index Pipeline  │  Query  │  Trace │ Daemon │
├──────────────────┴─────────┴────────┴────────┤
│           Resolve (符号解析 + 数据流)          │
├──────────────────────────────────────────────┤
│        Parse (tree-sitter 多语言提取)          │
├──────────────────────────────────────────────┤
│     Discover (ignore)  │  Storage (LadybugDB) │
└──────────────────────────────────────────────┘
```

### 索引流程

1. **文件发现** — `ignore` crate 遵守 `.gitignore` 规则
2. **增量哈希** — SHA-256 比对，跳过未变更文件
3. **并行解析** — Rayon 并行 + tree-sitter 提取节点/边
4. **符号解析** — FQN 生成、调用解析、数据流分析、跨语言 FFI
5. **批量入库** — CSV 生成 + `COPY FROM` 批量加载

### 图模型

- **44 种节点类型**：Project, Folder, File, Module, Class, Struct, Enum, Trait, Impl, Function, Method, Variable, GlobalVar, Parameter, Const, Static, Macro, TypeAlias, Typedef, Namespace, Interface, Constructor, Property, Record, Delegate, Annotation, Template, Union, Variant, Field, Event, Handler, Middleware, Service, Endpoint, Route, Process, Database, Config, Test, Section, Community, Tool, Embedding
- **24 种边类型**：Contains, Defines, MemberOf, Calls, FfiCalls, DataFlows, Reads, Writes, Implements, Extends, UsesType, References, Imports, Includes, HasMethod, HasProperty, Accesses, MethodOverrides, MethodImplements, StepInProcess, HandlesRoute, Fetches, HandlesTool, EntryPointOf
- 每条边携带置信度分数 (0.0-1.0) 和置信度分层（`SameFile` / `ImportScoped` / `Global`）

## 支持语言

| 语言 | 节点类型 | 边类型 |
|------|----------|--------|
| C | Function, GlobalVar, Struct, Enum, Typedef, Macro | Calls, Imports, Reads, Writes, Includes |
| Rust | Function, Struct, Enum, Trait, Impl, Const, Static, Macro, Module, TypeAlias | Calls, Imports, Reads, Writes |
| Fortran | Module, Function | Calls, Imports, FfiCalls |
| Python | Function, Method, Class | Calls, Imports, Extends |
| TypeScript | Function, Class, Method, Interface, Enum, TypeAlias, Const | Calls, Imports |
| Go | Function, Method, Struct, Interface, TypeAlias | Defines, Calls, Imports |
| Java | Class, Interface, Enum, Method | Defines, Calls, Imports |
| C++ | Function, Method, Class, Struct, Namespace, Enum, Template | Defines, Calls, Imports |

## 开发

```bash
# 运行测试
cargo test

# 代码检查
cargo clippy -- -D warnings

# 格式化
cargo fmt

# 基准测试
cargo bench
```

## 贡献

欢迎提交 Issue 和 Pull Request。请确保通过 `cargo test` 和 `cargo clippy -- -D warnings`。

## 路线图

CodeNexus 当前版本 v0.2.1。按当前优先级排序的规划工作：

- [x] v0.1.0 — 多语言索引（C/Rust/Fortran/Python/TypeScript）、图模式（44 种节点类型 + 24 种边类型）、`query`/`trace`/`impact`/`context`/`search`、增量索引、RAM 优先模式、MCP 服务、团队 `export`/`import`、守护进程模式、置信度分层、歧义消解
- [x] v0.1.x — 稳定性与性能加固：增量重索引覆盖、大仓库内存调优、更多语言专属边提取
- [x] v0.2.0 — `lsp` feature：LSP 增强提取，超越 tree-sitter 的类型精确解析（rust-analyzer 集成）
- [x] v0.2.0 — 扩展语言覆盖（Go、Java、C++），由新的 `lang-*` feature 控制
- [x] v0.2.0 — 分析工具包：死代码检测、架构概览、API 审查（route-map/shape-check/api-impact/tool-map）、社区检测、跨服务链接检测
- [x] v0.2.1 — AST 复杂度分析：圈/认知复杂度、嵌套深度、函数长度，绿/黄/红三级告警
- [ ] v0.3.0 — 跨语言数据流端到端追踪（当前已记录边；多跳污点路径需专用查询路径）
- [ ] v0.3.0 — 向量嵌入默认开启语义搜索（待 ONNX 模型大小与启动开销可接受后）
- [ ] 未来 — 基于查询门面的 Web UI / 图可视化

## 许可证

[MIT](LICENSE)
