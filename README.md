<div align="center">

<img src="docs/assets/CodeNexus.png" alt="CodeNexus Logo" width="200">

[![Build](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml/badge.svg)](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml) [![Crates.io](https://img.shields.io/crates/v/codenexus.svg)](https://crates.io/crates/codenexus) [![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Rust Version](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org)

**中文** | [English](README_EN.md)

**基于 LadybugDB 与 tree-sitter 的多语言代码知识图谱工具**

[✨ 功能特性](#-功能特性) • [🚀 快速开始](#-快速开始) • [📚 文档](#-文档) • [💻 示例](#-示例) • [🤝 参与贡献](#-参与贡献)

</div>

---

<div align="center">

### 🎯 索引一次，问遍全仓

跑一遍 `codenexus index`，符号关系即入图，剩下的追问交给图完成：

<table style="width:100%; border-collapse: collapse">
<tr>
<td align="center" width="25%">⚡<br><b>增量管线</b><br><span style="color:#64748B">哈希比对 · 只解析变更</span></td>
<td align="center" width="25%">🕸️<br><b>属性图模型</b><br><span style="color:#64748B">44类节点 · 30类边 · Cypher</span></td>
<td align="center" width="25%">🧭<br><b>多跳追踪</b><br><span style="color:#64748B">调用链 · 数据流 · 污点路径</span></td>
<td align="center" width="25%">🔌<br><b>双入口</b><br><span style="color:#64748B">38 命令 · 8 工具 · 同语义</span></td>
</tr>
</table>

</div>

---

## 📋 目录

- [✨ 功能特性](#-功能特性)
- [🚀 快速开始](#-快速开始)
- [🛠️ CLI 命令](#️-cli-命令)
- [🔌 MCP 集成](#-mcp-集成)
- [📚 文档](#-文档)
- [💻 示例](#-示例)
- [🏗️ 架构](#️-架构)
- [🧪 测试](#-测试)
- [📊 性能](#-性能)
- [🔒 安全](#-安全)
- [🗺️ 开发路线图](#️-开发路线图)
- [🤝 参与贡献](#-参与贡献)
- [📋 更新日志](#-更新日志)
- [📄 许可证](#-许可证)
- [🙏 致谢](#-致谢)
- [📞 联系与支持](#-联系与支持)
- [⭐ Star 历史](#-star-历史)

---

## ✨ 功能特性

<table style="width:100%; border-collapse: collapse">
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🌐 <b>多语言解析</b><br><span style="color:#64748B">默认 <code>full</code> 预设支持 24 种语言（C、Rust、Fortran、Python、TypeScript、Go、Java、C++、JavaScript、Ruby、Haskell、OCaml、Scala、PHP、C#、Bash、HTML、CSS、JSON、Regex、Verilog、Kotlin、Swift、Solidity），可用 <code>lang-*</code> feature 按需裁剪</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🕸️ <b>图数据库</b><br><span style="color:#64748B">LadybugDB 图存储，44 种节点类型 + 30 种边类型，Cypher 子集查询</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🔄 <b>增量索引</b><br><span style="color:#64748B">SHA-256 文件哈希比对，仅重新解析变更文件；Rayon 并行 + 线程局部 parser 池</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">💾 <b>RAM 优先索引</b><br><span style="color:#64748B">LZ4 压缩源码到内存，单次 <code>COPY FROM</code> 批量入库（<code>--ram_first</code>）</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🔍 <b>符号追踪</b><br><span style="color:#64748B">调用链（Calls）与数据流（DataFlows）双向追踪；跨语言多跳污点路径追踪（<code>TaintPathTracer</code>）</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🎯 <b>影响分析</b><br><span style="color:#64748B">变更影响半径分析，按深度分层，多维边类型 + 风险评估</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🧩 <b>歧义消解与置信度分层</b><br><span style="color:#64748B">多匹配符号按置信度排序消解；每条边携带分层（SameFile / ImportScoped / Global）+ 0.0-1.0 分数</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">📐 <b>架构图导出</b><br><span style="color:#64748B"><code>diagram</code> 将架构编译为自包含交互式 HTML（确定性布局 / 正交路由 / 暗亮主题 / 源码证据徽标）</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🆚 <b>架构语义 Delta</b><br><span style="color:#64748B"><code>arch_diff</code> 对比两个已索引项目，输出 Before/Delta/After HTML + 机器回执（added/removed/changed + JSON Pointer 字段）</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🧾 <b>诊断回执</b><br><span style="color:#64748B">错误与告警输出结构化回执（稳定规则码 + 证据 + 可执行修复话术），符号歧义 / 索引过期 / 结果截断均附带修复建议</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🔗 <b>跨语言 FFI</b><br><span style="color:#64748B">C-Fortran bind(C)、Rust extern 等跨语言调用解析</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">📦 <b>团队制品</b><br><span style="color:#64748B"><code>export</code> / <code>import</code> 压缩 <code>.graph.zst</code> 制品，共享索引</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🤖 <b>多智能体 MCP</b><br><span style="color:#64748B"><code>setup</code> 自动检测 Claude Code / Cursor / Codex；<code>skill</code> 一键同步技能文档到各 Agent 全局技能目录；<code>hook</code> 输出 PreToolUse/PostToolUse JSON；<code>mcp</code> stdio 服务暴露 10 个工具（参数语义写入工具描述）；<code>ask</code> 自然语言入口路由既有命令</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">👁️ <b>文件监视</b><br><span style="color:#64748B">守护进程模式，自动增量索引（<code>daemon</code> feature，SIGTERM/SIGINT 优雅退出；<code>--notify-impact</code> 变更影响告警）</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🚦 <b>CI 与架构门禁</b><br><span style="color:#64748B"><code>ci</code> 风险门禁（<code>--fail_on</code> 退出码 + PR Markdown，附官方 GitHub Action）；<code>lint</code> 自定义 Cypher 规则包（<code>.codenexus/rules.json</code>）</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🛡️ <b>安全与供应链</b><br><span style="color:#64748B"><code>taint</code> 内置五语言 source/sink 规则库自动审计；<code>supply</code> 外部依赖入图（ExternalPackage/DEPENDS_ON）供应链视图</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">📈 <b>架构演化</b><br><span style="color:#64748B"><code>evolve</code> 回放最近 N 个提交：worktree 快照逐个索引，产出指标时间线 JSON + 内联 SVG sparkline HTML</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">📦 <b>制品生态</b><br><span style="color:#64748B"><code>hub</code> push/pull/list（协议 v1，token 或 <code>CODENEXUS_HUB_TOKEN</code>）；<code>skill</code> 一键同步技能文档到 Agent 全局目录；<code>ask</code> 自然语言入口</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🧮 <b>分析工具包</b><br><span style="color:#64748B">死代码检测（worklist 可达性 + 置信度）、架构概览、复杂度分析（8 项指标）、社区检测（Leiden）、跨服务调用链</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🧠 <b>向量嵌入</b><br><span style="color:#64748B">默认启用的语义搜索（<code>embed</code> feature，本地 ONNX 推理 + BM25 全文）</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🌍 <b>国际化</b><br><span style="color:#64748B">Unicode case folding + NFC 规范化（ICU4X，<code>i18n</code> feature，含于 <code>full</code> 预设）</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🧰 <b>LSP 增强</b><br><span style="color:#64748B">7 个 LSP 客户端（rust-analyzer、pyright、clangd、gopls、ts-lang-server、fortls、jdtls）提供超越 tree-sitter 的类型精确解析（<code>lsp</code> feature）</span></td>
</tr>
</table>

除上述核心能力外，CodeNexus 还提供 `context` 上下文组装（`--budget` token 预算器）、`detect_changes` 变更检测、`rename` 重命名影响预检、`ask` 自然语言入口、`ci`/`lint` 架构门禁、`taint` 污点审计、`supply` 供应链视图、`evolve` 演化回放、`hub` 制品客户端、`skill` 技能同步、基于 oxcache 的查询结果缓存与 inklog 结构化日志等能力；全部 37 个子命令（外加 `mcp` 服务模式）的分组清单见 [🛠️ CLI 命令](#️-cli-命令) 一节，逐命令参数语义与可运行示例见 [📖 用户指南 · 命令详解](docs/USER_GUIDE.md#️-命令详解)。

---

## 🚀 快速开始

### 📦 安装

```bash
# 从 crates.io 安装（默认 full 预设，含全部 24 语言 + 所有功能）
cargo install codenexus

# 从源码构建
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo install --path .

# 或直接编译
cargo build --release
```

> **链接失败排查（openEuler / CentOS 等 GCC ≤ 12 系统）**：默认安装会下载预编译的 LadybugDB 二进制，它依赖较新的 `libstdc++`。若链接报 `undefined symbol: std::to_chars(..., _Float128, ...)`，用环境变量强制从源码编译即可（需安装 `cmake`）：
>
> ```bash
> LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus
> ```

要求 Rust 1.97.1 及以上（MSRV，`Cargo.toml` `rust-version` 与 `clippy.toml` `msrv` 一致；CI 工具链当前锁定 1.95，见 `.github/workflows/ci.yml`）。

#### 🔧 构建预设与 Feature 开关

**预设**：`default = ["full"]`

| Feature           | 默认 | 说明 |
| ----------------- | ---- | ---- |
| `minimal`         | —    | 最小预设：仅 `lang-rust` |
| `core`            | —    | 核心预设：`lang-c` + `lang-rust` + `lang-python` |
| `full`            | 启用 | 完整预设：`core` + Fortran/TypeScript/Go/Java/C++/JavaScript/Ruby/Haskell/OCaml/Scala/PHP/C#/Bash/HTML/CSS/JSON/Regex/Verilog + daemon/analysis/complexity/api-review/community/cross-service/diagram/lsp/cli/mcp/cache/embed/i18n |
| `lang-c`          | —    | C 语言解析器（tree-sitter-c） |
| `lang-rust`       | 启用 | Rust 语言解析器（tree-sitter-rust） |
| `lang-fortran`    | —    | Fortran 语言解析器（tree-sitter-fortran） |
| `lang-python`     | —    | Python 语言解析器（tree-sitter-python） |
| `lang-typescript` | —    | TypeScript 语言解析器（tree-sitter-typescript） |
| `lang-go`         | —    | Go 语言解析器（tree-sitter-go） |
| `lang-java`       | —    | Java 语言解析器（tree-sitter-java） |
| `lang-cpp`        | —    | C++ 语言解析器（tree-sitter-cpp） |
| `lang-javascript` | —    | JavaScript 语言解析器（tree-sitter-javascript） |
| `lang-ruby`       | —    | Ruby 语言解析器（tree-sitter-ruby） |
| `lang-haskell`    | —    | Haskell 语言解析器（tree-sitter-haskell） |
| `lang-ocaml`      | —    | OCaml 语言解析器（tree-sitter-ocaml） |
| `lang-scala`      | —    | Scala 语言解析器（tree-sitter-scala） |
| `lang-php`        | —    | PHP 语言解析器（tree-sitter-php） |
| `lang-csharp`     | —    | C# 语言解析器（tree-sitter-c-sharp） |
| `lang-bash`       | —    | Bash 语言解析器（tree-sitter-bash） |
| `lang-html`       | —    | HTML 语言解析器（tree-sitter-html） |
| `lang-css`        | —    | CSS 语言解析器（tree-sitter-css） |
| `lang-json`       | —    | JSON 语言解析器（tree-sitter-json） |
| `lang-regex`      | —    | 正则语言解析器（tree-sitter-regex） |
| `lang-verilog`    | —    | Verilog 语言解析器（tree-sitter-verilog） |
| `daemon`          | 启用 | 文件监视守护进程（notify + notify-debouncer-full） |
| `embed`           | 启用 | 向量嵌入语义搜索（reqwest HTTP + 本地 ONNX 推理） |
| `lsp`             | 启用 | LSP 增强解析（7 个 LSP 客户端） |
| `analysis`        | 启用 | 死代码检测 + 架构概览（纯 Cypher 聚合） |
| `complexity`      | 启用 | AST 复杂度分析（8 项指标，依赖 `analysis`） |
| `api-review`      | 启用 | API 审查工具包（route_map/shape_check/api_impact/tool_map） |
| `community`       | 启用 | 社区检测（Leiden 模块度优化，依赖 petgraph） |
| `cross-service`   | 启用 | 跨服务调用链检测（HTTP 路由模式匹配） |
| `diagram`         | 启用 | 架构图管线：`diagram`/`arch_diff` 命令（依赖 `analysis`） |
| `mcp`             | 启用 | MCP 服务器（sdforge `mcp` stdio 传输） |
| `cli`             | 启用 | CLI 二进制（sdforge `cli` 传输，二进制必需） |
| `cache`           | 启用 | 查询结果缓存（oxcache） |
| `i18n`            | 启用 | Unicode case folding + NFC 规范化（ICU4X） |

> **日志系统**：inklog 是唯一日志后端（console + file rotation + daily 滚动 + LZ4 压缩），不再提供 tracing-subscriber 可选后端。

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

### 💡 最小示例

以下命令改编自 [`examples/src/bin/basic_indexing.rs`](examples/src/bin/basic_indexing.rs) 等示例与 [📖 用户指南](docs/USER_GUIDE.md)，均可直接运行：

```bash
# 1. 索引一个代码仓库（数据库默认写入 .codenexus/<项目名>.lbug）
codenexus index --path /path/to/project --name myproject

# 1b. RAM 优先索引（LZ4 内存压缩，适合中小仓库，更快）
codenexus index --path /path/to/project --name myproject --ram_first true

# 2. 查询函数（Cypher 子集）
codenexus query --cypher "MATCH (f:Function) RETURN f.name LIMIT 10"

# 3. 追踪调用链（可选参数已有内置默认值：depth=5、无路径过滤）
codenexus trace --symbol main --trace_type calls

# 4. 搜索符号（exact / regex / fuzzy + BM25 全文；limit 默认 50）
codenexus search --text "parse" --mode exact
codenexus search --text "authentication logic" --fulltext true
```

> 💡 **建议**：把 `.codenexus/` 加入项目的 `.gitignore`（索引库与日志都在这个目录里，不应入库）。也可以创建 `.codenexus/config.json` 固化每项目的常用参数（如 `ram_first`、复杂度阈值），详见 [📖 用户指南 · 配置文件](docs/USER_GUIDE.md#️-配置文件)。

### 🧭 核心概念

- **知识图谱模型**：源码被解析为 44 种节点与 30 种边构成的属性图，存入 LadybugDB，可用 Cypher 子集查询。
- **严格 flag 风格 CLI**：无位置参数，参数为 snake_case 长选项（如 `--symbol`、`--trace_type`），布尔选项显式传值（`true`/`false`）。可选参数自带内置默认值（如 `trace --depth 5`、`search --limit 50`，完整清单见各命令 `--help`），并可用 `.codenexus/config.json` 按项目固化。
- **全局 `--db` 选项**：数据库路径默认 `.codenexus/<项目名称>.lbug`，需置于子命令之前；仅有一个索引时可自动发现。
- **退出码契约**：0 成功、1 内部错误、2 无效输入 / 项目不存在 / 查询错误、4 NotFound / 数据库损坏（见 `src/service/error.rs`）。

> 增量索引、置信度分层等完整核心约定见 [📖 用户指南 · 核心约定](docs/USER_GUIDE.md#-核心约定)。

---

## 🛠️ CLI 命令

CodeNexus 提供 **37 个子命令**（外加 `codenexus mcp` 服务模式），按功能分组：

- **索引与项目管理**：`index` / `daemon` / `status` / `list` / `clean` / `export` / `import`
- **查询与搜索**：`query` / `search` / `context`
- **追踪与影响分析**：`trace` / `impact` / `detect_changes` / `rename`
- **分析工具包**：`dead_code` / `architecture` / `complexity` / `community` / `cross_service`
- **API 审查与架构图**：`route_map` / `shape_check` / `api_impact` / `tool_map` / `diagram` / `arch_diff`
- **多智能体与 LSP**：`setup` / `hook` / `mcp` / `lsp_goto_def` / `lsp_hover`

每个命令的全部参数语义与可运行示例见 [📖 用户指南 · 命令详解](docs/USER_GUIDE.md#️-命令详解)；复杂度指标与阈值表见 [📖 用户指南 · 复杂度分析](docs/USER_GUIDE.md#-复杂度分析)，死代码检测配置见 [📖 用户指南 · 死代码检测](docs/USER_GUIDE.md#-死代码检测)。

---

## 🔌 MCP 集成

CodeNexus 使用 [sdforge](https://crates.io/crates/sdforge) 提供 MCP（Model Context Protocol）服务器，经 sdforge `mcp` stdio 传输暴露 **10 个工具**（`query` / `trace` / `impact` / `search` / `context` / `architecture` / `diagram` / `arch_diff` / `dead_code` / `detect_changes`），与同名 CLI 命令共用同一套 `#[forge]` 定义；每个工具的描述包含参数语义与默认值，服务器以只读方式打开数据库，可与写入进程并存。

```bash
# 启动 MCP 服务（stdio）
codenexus mcp [--db <DB_PATH>]

# 自动检测已安装的 Claude Code / Cursor / Codex 并写入 MCP 配置（--force 跳过确认）
codenexus setup

# 输出 PreToolUse/PostToolUse JSON（exit 0，永不阻塞，适合作为智能体钩子）
codenexus hook
```

各工具的能力说明见 [📖 用户指南 · 多智能体集成](docs/USER_GUIDE.md#-多智能体集成)。

---

## 📚 文档

| 文档 | 说明 |
|------|------|
| [📖 用户指南](docs/USER_GUIDE.md) | 从安装到进阶的完整使用教程（含复杂度分析与死代码检测详解） |
| [📘 API 参考](docs/API_REFERENCE.md) | 库 crate 公开 API、Facade 接口与 CLI/MCP 对外接口 |
| [🏗️ 架构文档](docs/ARCHITECTURE.md) | 分层结构、索引管线、图模型与架构图命令语义 |
| [⚡ 性能指南](docs/PERFORMANCE.md) | 基准套件、实测基线、SLO 与内存优化（L1–L7 防线） |
| [🔒 安全文档](docs/SECURITY.md) | 安全策略、漏洞报告流程与最佳实践 |
| [❓ FAQ](docs/FAQ.md) | 常见问题解答 |
| [🧪 测试场景矩阵](docs/TEST_SCENARIOS.md) | 基于真实测试套件的场景穷举矩阵 |
| [📋 更新日志](docs/CHANGELOG.md) | 每个版本的变更记录（Keep a Changelog 格式） |
| [🤝 贡献指南](docs/CONTRIBUTING.md) | 如何参与项目开发 |
| [📜 行为准则](docs/CODE_OF_CONDUCT.md) | 社区行为准则 |
| [📐 架构设计文档（ADD）](docs/ADD.md) | 架构决策与设计细节 |
| [🎯 产品需求文档（PRD）](docs/PRD.md) | 产品需求与 SLO 指标 |
| [🧾 技术需求文档（TRD）](docs/TRD.md) | 技术需求分解 |
| [🗄️ 数据库设计文档（DDD）](docs/DDD.md) | 图存储 Schema 设计 |
| [🗜️ 数据库压缩实测](docs/database-compression.md) | gzip / zstd / lz4 压缩率与耗时实测 |
| [🔬 研究笔记](docs/research/) | TaintRadar、级联漏洞链等论文笔记 |
| [🛡️ 安全审计](docs/security/) | Strix 审计 triage 与 ReDoS 误报复核实证 |
| [🤖 CLI 技能](skill/SKILL.md) | 面向 AI 智能体的 CLI 用法知识包（针对 v0.3.12 校验） |
| [📈 基准测试说明](benches/README.md) | Criterion 基准套件与 SLO 阈值表 |
| [📦 crates.io](https://crates.io/crates/codenexus) | 发布页面 |

---

## 💻 示例

全部 14 个可运行示例位于 [`examples/`](examples/) 目录，每个示例对应一个 `cargo run --bin` 目标（经 `examples/Cargo.toml` 注册）：

| 示例 | 文件 | 描述 |
|------|------|------|
| basic_indexing | `examples/src/bin/basic_indexing.rs` | 索引 Rust 源码到知识图谱，Cypher 查询函数列表 |
| cypher_query | `examples/src/bin/cypher_query.rs` | 对图谱执行多种 Cypher 查询（按类型、按名称） |
| symbol_search | `examples/src/bin/symbol_search.rs` | 按名称、类型搜索符号，处理空结果 |
| call_tracing | `examples/src/bin/call_tracing.rs` | 正向追踪函数调用路径，构建调用图 |
| impact_analysis | `examples/src/bin/impact_analysis.rs` | 分析修改某符号的影响半径（反向 BFS） |
| symbol_context | `examples/src/bin/symbol_context.rs` | 符号 360° 视图：调用方 / 被调方 / 执行流，以及子图加载与符号消歧 |
| export_import | `examples/src/bin/export_import.rs` | 图谱数据库的导出与导入验证 |
| project_lifecycle | `examples/src/bin/project_lifecycle.rs` | 项目生命周期：索引多个项目 → 列出 → 按名解析 → 删除 |
| code_analysis | `examples/src/bin/code_analysis.rs` | 代码质量分析三件套：复杂度 / 死代码 / 社区检测 |
| api_surface | `examples/src/bin/api_surface.rs` | API/Web 服务面分析：路由表 / schema 校验 / API 影响 / 跨服务调用 / MCP 工具表 |
| architecture_diagram | `examples/src/bin/architecture_diagram.rs` | 架构总览 + 自包含交互式架构图 HTML + 双项目架构 diff |
| daemon_watch | `examples/src/bin/daemon_watch.rs` | 文件监视守护：`notify` 防抖 → 增量索引（Observer 模式）→ 优雅停止 |
| git_integration | `examples/src/bin/git_integration.rs` | Git 集成：把 `git diff` 的变更行映射到受影响符号并做风险分级 |
| setup_mcp | `examples/src/bin/setup_mcp.rs` | MCP 接入配置：自动探测已安装的 AI coding agent 并写入 MCP server 配置 |

```bash
# 运行单个示例
cargo run --manifest-path examples/Cargo.toml --bin basic_indexing

# 运行所有示例
for bin in basic_indexing cypher_query symbol_search call_tracing impact_analysis symbol_context export_import project_lifecycle code_analysis api_surface architecture_diagram daemon_watch git_integration setup_mcp; do
  cargo run --manifest-path examples/Cargo.toml --bin $bin
done
```

示例通过 `IndexFacade` 索引源码、`QueryFacade` 执行查询、`TraceFacade` 追踪调用，退出时临时目录自动清理。库 API 的完整说明见 [📘 API 参考](docs/API_REFERENCE.md)。

---

## 🏗️ 架构

CodeNexus 采用「库 + 二进制」双目标 crate：`src/lib.rs` 暴露公共 API（模型 / 解析 / 存储 / 索引 / 查询 / 追踪 / service 模块），`src/main.rs` 是 sdforge 驱动的 CLI 二进制；v0.3.2 起 CLI 与 MCP 接口经 sdforge `#[forge]` 宏统一封装在 `src/service/`，每个命令定义 core 函数 + CLI wrapper + MCP wrapper。索引方向为「文件发现 → 增量哈希 → 并行解析 → 符号解析 → 批量入库」。

三层源码结构、索引管线流程图、图模型（44 种节点 / 30 种边与置信度分层）、核心语言提取表与 `architecture` / `diagram` / `arch_diff` 命令输出语义，详见 [🏗️ 架构文档](docs/ARCHITECTURE.md)。

---

## 🧪 测试

### 🎯 测试策略

分层测试策略：`src/` 内联 `#[cfg(test)]` 单元测试 → `tests/` 集成测试（CLI 子进程 E2E、全功能套件、MCP/图集成、非 ASCII 路径）→ `tests/acceptance/` 8 语言真实开源项目验收（与 gitnexus 交叉验证）→ `benches/` 7 组 Criterion 基准回归。测试分层总览与逐条场景矩阵见 [🧪 测试场景矩阵](docs/TEST_SCENARIOS.md)。

### ▶️ 运行命令（与 CI 一致）

```bash
# 格式检查（nightly rustfmt，rustfmt.toml 使用 nightly-only 选项）
cargo +nightly fmt --all -- --check

# Clippy 门禁（CI 按 full 与 minimal 双档执行，警告即错误）
cargo +1.95 clippy -- -D warnings
cargo +1.95 clippy --lib --no-default-features --features minimal -- -D warnings

# 测试（CI 矩阵按 minimal / core / full / core,daemon,analysis,complexity / core,lsp,cache / full,embed 六档运行）
cargo test --lib --verbose
cargo test --lib --no-default-features --features "core" --verbose

# 覆盖率门禁：行覆盖率不低于 95%（CI coverage job 与 pre-push 钩子执行）
cargo llvm-cov --lib --fail-under-lines 95 --lcov --output-path lcov.info

# 基准测试（--quick 达到统计显著性即停止）
cargo bench -- --quick
cargo bench --bench daemon_bench --features daemon -- --quick

# 安全审计（CI security job：RustSec 公告 + 许可证/禁用依赖）
cargo audit
cargo deny check
```

> CI 还会在每次 push/PR 上运行 CodeQL 静态分析（`.github/workflows/codeql.yml`），并在 `v*` tag 推送时触发 Release 工作流（GitHub Release + crates.io 发布）。

### 📊 测试规模

截至 v0.3.12（`#[test]` / `#[tokio::test]` 函数 grep 统计）：约 4600+ 条单元测试（147 个源文件含 `#[cfg(test)]`）+ 118 条集成测试（8 个文件）+ 8 个验收项目 + 7 组 Criterion 基准；覆盖率门禁为行覆盖 ≥ 95%（CI coverage job 与 pre-push 钩子双重执行）。逐文件分解与完整统计见 [🧪 测试场景矩阵 · 统计汇总](docs/TEST_SCENARIOS.md#-统计汇总)。

---

## 📊 性能

基准套件为 `benches/` 下 7 组 Criterion 基准（SLO 阈值来自 `docs/PRD.md` §5.1）：实测 1000 文件冷启动索引约 3929 files/s（SLO ≥ 100）、单文件增量约 4987 files/s（SLO ≥ 500）、daemon 去抖响应约 2.76 s（SLO ≤ 3 s）；`incremental_500_of_1000` 为已知未达标项。v0.3.10–v0.3.12 落地的 L1–L7 内存防线（`MemoryBudget` 三级内存压力、流式 CSV、管线流式化、buffer_pool 封顶等）将 70 GB 主机上的索引峰值内存从约 60 GB 降至约 4 GB。完整实测基线、SLO 表与调优方法见 [⚡ 性能指南](docs/PERFORMANCE.md)，SLO 阈值表见 [`benches/README.md`](benches/README.md)。

---

## 🔒 安全

### 🛡️ 安全设计

攻击面集中在索引文件（LadybugDB 数据库、`.graph.zst` 导入制品、tree-sitter 解析输入）与进入 Cypher 子集查询的 `query` / `trace` / `impact` / `search` 用户输入；代码层面配套 Cypher / 标识符转义、图编辑 dry-run 默认与诊断回执的失败显性化。设计细节与范围界定见 [🔒 安全文档](docs/SECURITY.md)。

### ⛓️ 供应链与门禁

CI 内置四道门禁：`cargo-audit`（RustSec 公告扫描）、`cargo-deny`（许可证 / 禁用依赖校验）、CodeQL 静态分析与 pre-commit 密钥扫描。完整清单与忽略项说明见 [🔒 安全文档](docs/SECURITY.md#️-supply-chain-and-gates)。

### 🚨 报告安全漏洞

请勿通过公开 issue 报告安全漏洞。请发送邮件至 **security@kirky-x.dev**，附漏洞描述与影响、复现步骤（最小代码库或 `codenexus` 命令序列）、版本信息（`codenexus --version`、Rust 工具链、操作系统）与已知缓解措施。项目承诺 48 小时内确认、5 个工作日内给出初步评估。完整政策（支持版本、披露流程、范围界定）见 [🔒 安全文档](docs/SECURITY.md)。

---

## 🗺️ 开发路线图

<table style="width:100%; border-collapse: collapse">
<tr><th style="text-align:center">状态</th><th style="text-align:left">方向</th><th style="text-align:left">条目</th></tr>
<tr><td align="center">✅</td><td>核心索引与图模型</td><td>v0.1.0 — 多语言索引（C/Rust/Fortran/Python/TypeScript）、图模式（44 种节点类型 + 30 种边类型）、<code>query</code>/<code>trace</code>/<code>impact</code>/<code>context</code>/<code>search</code>、增量索引、RAM 优先模式、MCP 服务、团队 <code>export</code>/<code>import</code>、守护进程模式、置信度分层、歧义消解</td></tr>
<tr><td align="center">✅</td><td>稳定性与性能加固</td><td>v0.1.x — 增量重索引覆盖、大仓库内存调优、更多语言专属边提取</td></tr>
<tr><td align="center">✅</td><td>LSP 增强</td><td>v0.2.0 — <code>lsp</code> feature：LSP 增强提取，超越 tree-sitter 的类型精确解析（rust-analyzer 集成）</td></tr>
<tr><td align="center">✅</td><td>语言覆盖扩展</td><td>v0.2.0 — 扩展语言覆盖（Go、Java、C++，以及 JavaScript/Ruby/Haskell/OCaml/Scala/PHP/C#/Bash/HTML/CSS/JSON/Regex/Verilog），由新的 <code>lang-*</code> feature 控制</td></tr>
<tr><td align="center">✅</td><td>分析工具包</td><td>v0.2.0 — 死代码检测、架构概览、API 审查（route_map/shape_check/api_impact/tool_map）、社区检测、跨服务链接检测</td></tr>
<tr><td align="center">✅</td><td>复杂度分析</td><td>v0.2.1 — AST 复杂度分析：圈/认知复杂度、嵌套深度、函数长度，绿/黄/红/致命四级告警</td></tr>
<tr><td align="center">✅</td><td>MCP 服务器</td><td>v0.3.0 — sdforge-based MCP 服务器：<code>#[forge]</code> 宏 + sdforge <code>mcp</code> stdio 传输，替代手写 JSON-RPC；6 个工具（query/trace/impact/search/context/architecture）</td></tr>
<tr><td align="center">✅</td><td>跨语言污点追踪</td><td>v0.3.2 — 跨语言数据流端到端追踪：<code>TaintPathTracer</code> BFS 遍历 DataFlows/Reads/Writes/FfiCalls 边</td></tr>
<tr><td align="center">✅</td><td>语义搜索</td><td>v0.3.2 — 向量嵌入默认开启语义搜索（<code>embed</code> feature 已包含在 <code>full</code> 预设中）</td></tr>
<tr><td align="center">✅</td><td>国际化</td><td>v0.3.3 — 国际化模块（<code>i18n</code> feature）：ICU4X Unicode case folding + NFC 规范化 + CJK 边界检测</td></tr>
<tr><td align="center">✅</td><td>Harness 现代化</td><td>v0.3.3 — CI 升级 Rust 1.91 + 6 特性矩阵 + dependabot + codeql + crates.io 发布</td></tr>
<tr><td align="center">✅</td><td>大仓库内存防线</td><td>v0.3.11 — 大型仓库索引 OOM 修复（L1–L7 七层防线）：<code>MemoryBudget</code> 三级内存压力 + <code>Graph::nodes_view/edges_view</code> 迭代器 + 流式 CSV + mpsc channel 并行解析 + L5 自适应降级 + L6 管线流式化（<code>ctx.remove</code> 取代 <code>Graph::clone</code>）+ L7 LadybugDB buffer_pool 封顶（4 GB）+ LSP 按需启动 + RAM-first 8× 放大因子预算。70 GB 主机峰值内存从 60 GB 降至 ~4 GB</td></tr>
<tr><td align="center">🚧</td><td>基础库升级</td><td>自研基础库升级至 RC（trait-kit / sdforge / oxcache 0.5.0-rc.2、inklog 0.3.0-rc.2）；MSRV 1.95 → 1.97.1</td></tr>
<tr><td align="center">✅</td><td>功能拓展波次（feature-expansion-wave）</td><td>2026-09 — RICE 排序的 12 项能力：<code>context --budget</code> token 预算器、<code>ci</code> 架构门禁（含官方 GitHub Action）、<code>lint</code> 自定义架构规则包、<code>skill</code> 技能同步、<code>ask</code> 自然语言入口、graph-viewer 快照合流（<code>diagram</code>/<code>arch_diff</code> <code>--viewer_url</code>）、daemon <code>--notify-impact</code> 影响告警、Kotlin/Swift/Solidity 语言支持、<code>taint</code> 安全审计（内置 source/sink 规则库）、<code>supply</code> 供应链视图（ExternalPackage/DEPENDS_ON）、<code>evolve</code> 架构演化回放、<code>hub</code> 制品客户端（协议 v1）</td></tr>
<tr><td align="center">📋</td><td>Web UI 与图可视化</td><td>基于查询门面的 Web UI / 图可视化（<code>diagram</code>/<code>arch_diff</code> 已交付架构图 HTML 与语义 Delta 及 graph-viewer 快照合流；3D graph-viewer 深度集成仍在规划中）</td></tr>
</table>

---

## 🤝 参与贡献

详细的贡献流程与代码规范请参阅 [🤝 贡献指南](docs/CONTRIBUTING.md)。

### 🛠️ 开发环境

工具链为 Rust stable 1.95+（CI 锁定 1.95；`Cargo.toml` MSRV 1.97.1）+ nightly（`cargo fmt` 使用 nightly-only 选项），系统依赖包括 C/C++ 编译器（tree-sitter grammar 构建）、`libssl-dev`、`pkg-config` 与 `protobuf-compiler`；提交前运行 `cargo +nightly fmt --all -- --check` 与 `cargo clippy -- -D warnings`；[pre-commit](https://pre-commit.com/) Git 钩子在 pre-commit 执行文件检查、私钥/密钥扫描、fmt 与 clippy，pre-push 执行 `cargo test --lib`、覆盖率门禁（≥95%）、`cargo audit` 与 `cargo deny check`；提交信息遵循 Conventional Commits（`feat`、`fix`、`perf`、`refactor`、`docs`、`test`、`chore`、`revert`）。完整环境搭建步骤见 [🤝 贡献指南 · 开发环境](docs/CONTRIBUTING.md#-development-environment)。

### 💖 贡献方式

<table style="width:100%; border-collapse: collapse">
<tr>
<td width="33%" align="center" style="padding: 16px">

### 🐛 报告 Bug

发现问题？<br>
<a href="https://github.com/Kirky-X/codenexus/issues/new">创建 Issue</a>

</td>
<td width="33%" align="center" style="padding: 16px">

### 💡 功能建议

有好想法？<br>
<a href="https://github.com/Kirky-X/codenexus/issues">提交功能建议</a>

</td>
<td width="33%" align="center" style="padding: 16px">

### 🔧 提交 PR

想贡献代码？<br>
<a href="https://github.com/Kirky-X/codenexus/pulls">Fork 并提交 PR</a>

</td>
</tr>
</table>

报告 Issue 时请附上：CodeNexus 版本（`codenexus --version`）、Rust 版本、操作系统、完整命令与错误输出、最小复现。安全漏洞请勿公开提交，见 [🔒 安全文档](docs/SECURITY.md)。

---

## 📋 更新日志

完整版本历史见 [📋 更新日志](docs/CHANGELOG.md)（遵循 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) 格式，语义化版本）。

| 版本 | 日期 | 要点 |
|------|------|------|
| Unreleased | — | 自研基础库升级至 RC（trait-kit / sdforge / oxcache 0.5.0-rc.2、inklog 0.3.0-rc.2）；MSRV 1.95 → 1.97.1 |
| 0.3.12 | 2026-07-30 | 动态 `max_db_size` + `--fresh` 标志解决 DB 膨胀；read-only 连接 4 TiB cap 修复 >16 GiB 数据库查询崩溃；LSP hover 批量 UNWIND 更新等 P 系列修复 |
| 0.3.11 | 2026-07-26 | L6+L7 内存优化：管线流式化 + 迭代器 API + buffer_pool 封顶，70 GB 主机峰值内存 60 GB → ~4 GB |
| 0.3.10 | 2026-07-25 | 大仓库索引 OOM 的 L1–L5 五层防线：内存预算、图视图迭代器、流式 CSV、mpsc 并发上限、自适应降级 |

---

## 📄 许可证

本项目采用 [MIT](LICENSE) 许可证。

---

## 🙏 致谢

### 🌟 核心依赖

CodeNexus 站在以下优秀开源项目的肩膀上：

| 依赖 | 用途 |
|------|------|
| [lbug](https://github.com/ladybugdb/ladybugdb)（LadybugDB） | 图数据库存储 |
| [tree-sitter](https://tree-sitter.github.io/) + 21 个语言 grammar crate | 多语言 AST 解析 |
| [rayon](https://github.com/rayon-rs/rayon) | 数据并行 |
| [notify](https://github.com/notify-rs/notify) / notify-debouncer-full | 文件监听与去抖 |
| [sdforge](https://crates.io/crates/sdforge) | CLI + MCP 双传输框架（`#[forge]` 宏） |
| [trait-kit](https://crates.io/crates/trait-kit) | 能力注册表 |
| [oxcache](https://crates.io/crates/oxcache) | 查询结果缓存 |
| [inklog](https://crates.io/crates/inklog) | 日志后端（console + 轮转 + LZ4 压缩） |
| [ort](https://github.com/pykeio/ort) / tokenizers | 本地 ONNX 向量嵌入推理 |
| [ICU4X](https://github.com/unicode-org/icu4x)（icu_normalizer / icu_casemap） | Unicode 规范化与大小写折叠 |
| [petgraph](https://github.com/petgraph/petgraph) | 社区检测图算法 |
| [criterion](https://github.com/bheisler/criterion.rs) | 基准测试 |

### 💝 特别感谢

感谢 Rust 社区与所有[贡献者](https://github.com/Kirky-X/codenexus/graphs/contributors)。

---

## 📞 联系与支持

<table style="width:100%; max-width: 600px">
<tr>
<td align="center" width="33%">
<a href="https://github.com/Kirky-X/codenexus/issues"><b style="color:#991B1B">Issues</b></a><br>
<span style="color:#64748B">报告问题和 Bug</span>
</td>
<td align="center" width="33%">
<a href="docs/FAQ.md"><b style="color:#1E40AF">文档 / FAQ</b></a><br>
<span style="color:#64748B">提问前请先查阅</span>
</td>
<td align="center" width="33%">
<a href="https://github.com/Kirky-X/codenexus"><b style="color:#1E293B">GitHub</b></a><br>
<span style="color:#64748B">查看源代码</span>
</td>
</tr>
</table>

---

## ⭐ Star 历史

[![Star History Chart](https://api.star-history.com/svg?repos=Kirky-X/codenexus&type=Date)](https://star-history.com/#Kirky-X/codenexus&Date)

如果这个项目对您有帮助，请考虑给它一个 ⭐️！

**由 Kirky.X 构建**

---

<sub>© 2026 Kirky.X. 保留所有权利。</sub>
