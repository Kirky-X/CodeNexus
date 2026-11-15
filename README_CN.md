[English](README.md) | 中文

<div align="center">

<img src="docs/assets/CodeNexus.png" alt="CodeNexus Logo" width="240">

[![Rust 2021](https://img.shields.io/badge/Rust-2021-edded?logo=rust&style=for-the-badge)](https://www.rust-lang.org/) [![Rustc 1.97.1+](https://img.shields.io/badge/Rustc-1.97.1+-orange.svg?style=for-the-badge)](https://www.rust-lang.org/) [![crates.io](https://img.shields.io/crates/v/codenexus?style=for-the-badge)](https://crates.io/crates/codenexus) [![downloads](https://img.shields.io/crates/d/codenexus?style=for-the-badge)](https://crates.io/crates/codenexus) [![CI](https://img.shields.io/github/actions/workflow/status/Kirky-X/codenexus/ci.yml?branch=main&label=CI&style=for-the-badge)](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml) [![license](https://img.shields.io/crates/l/codenexus?style=for-the-badge)](LICENSE)

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
<td align="center" width="25%">🕸️<br><b>属性图模型</b><br><span style="color:#64748B">45类节点 · 32类边 · Cypher</span></td>
<td align="center" width="25%">🧭<br><b>多跳追踪</b><br><span style="color:#64748B">调用链 · 数据流 · 污点路径</span></td>
<td align="center" width="25%">🔌<br><b>双入口</b><br><span style="color:#64748B">39 命令 + mcp · 11 工具 · 同语义</span></td>
</tr>
</table>

</div>

---

## 📋 目录

<details open>
<summary>📑 目录</summary>

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

</details>

---

## ✨ 功能特性

<table style="width:100%; border-collapse: collapse">
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🌐 <b>多语言解析</b><br><span style="color:#64748B">默认 <code>full</code> 预设支持 22 种语言（C、Rust、Fortran、Python、TypeScript、Go、Java、C++、JavaScript、Ruby、Haskell、OCaml、Scala、PHP、C#、Bash、HTML、CSS、JSON、Regex、Verilog、Swift），可用 <code>lang-*</code> feature 按需裁剪</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🕸️ <b>图数据库</b><br><span style="color:#64748B">LadybugDB 图存储，45 种节点类型 + 32 种边类型，Cypher 子集查询</span></td>
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
<td width="50%" style="vertical-align:top; padding: 12px">📦 <b>团队制品</b><br><span style="color:#64748B"><code>export</code> / <code>import</code> 压缩 <code>.cnxp</code> 制品，共享索引</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🤖 <b>多智能体 MCP</b><br><span style="color:#64748B"><code>setup</code> 自动检测 Claude Code / Cursor / Codex；<code>skill</code> 一键同步技能文档到各 Agent 全局技能目录；<code>hook</code> 输出 PreToolUse/PostToolUse JSON；<code>mcp</code> stdio 服务暴露 11 个工具（参数语义写入工具描述）；<code>ask</code> 自然语言入口路由既有命令</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">👁️ <b>文件监视</b><br><span style="color:#64748B">守护进程模式，自动增量索引（<code>daemon</code> feature，SIGTERM/SIGINT 优雅退出）</span></td>
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
<td width="50%" style="vertical-align:top; padding: 12px">🧠 <b>向量嵌入</b><br><span style="color:#64748B">默认启用的语义搜索（<code>embeddings</code> feature，本地 Candle 推理 + BM25 全文）</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🌍 <b>国际化</b><br><span style="color:#64748B">Unicode case folding + NFC 规范化（ICU4X，<code>i18n</code> feature，含于 <code>full</code> 预设）</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🧰 <b>LSP 增强</b><br><span style="color:#64748B">7 个 LSP 客户端（rust-analyzer、pyright、clangd、gopls、ts-lang-server、fortls、jdtls）提供超越 tree-sitter 的类型精确解析（<code>lsp</code> feature）</span></td>
</tr>
</table>

除上述核心能力外，CodeNexus 还提供 `context` 上下文组装（`--budget <N>` 控制 token 预算，省略时不裁剪）、`detect_changes` 变更检测、`rename` 重命名影响预检、`ask` 自然语言入口、`ci`/`lint` 架构门禁、`taint` 污点审计、`supply` 供应链视图、`evolve` 演化回放、`hub` 制品客户端、`skill` 技能同步、基于 oxcache 的查询结果缓存与 inklog 结构化日志等能力；全部 39 个子命令（外加 `mcp` 服务模式）的分组清单见 [🛠️ CLI 命令](#️-cli-命令) 一节，逐命令参数语义与可运行示例见 [📖 用户指南 · 命令详解](docs/cn/USER_GUIDE.md#️-命令详解)。

---

## 🚀 快速开始

### 📦 安装

```bash
# 从 crates.io 安装（默认 full 预设，含全部 22 语言 + 所有功能）
cargo install codenexus
```

要求 Rust 1.97.1 及以上（MSRV，`Cargo.toml` `rust-version` 与 `clippy.toml` `msrv` 一致；CI 工具链当前锁定 1.97.1，见 `.github/workflows/ci.yml`）。

> **链接失败排查**（openEuler / CentOS 等 GCC ≤ 12 系统）：见 [❓ FAQ · 安装与构建](docs/cn/FAQ.md#-安装与构建)。
>
> **源码构建、按语言裁剪与完整 Feature 清单**：见 [📖 用户指南 · 构建预设与 Feature 开关](docs/cn/USER_GUIDE.md#-构建预设与-feature-开关)。

### 💡 最小示例

以下命令改编自 [`examples/src/bin/basic_indexing.rs`](examples/src/bin/basic_indexing.rs) 等示例与 [📖 用户指南](docs/cn/USER_GUIDE.md)，均可直接运行：

```bash
# 1. 索引一个代码仓库（数据库默认写入 .codenexus/<项目名>.lbug）
codenexus index --path /path/to/project --name myproject

# 1b. RAM 优先索引（LZ4 内存压缩 + 单次批量入库；建议源码 ≥ 1 GB 的仓库使用，ADR-024，
#     小仓库实测更耗内存，超出内存预算时自动回退流式磁盘读取）
codenexus index --path /path/to/project --name myproject --ram_first true

# 2. 查询函数（Cypher 子集）
codenexus query --cypher "MATCH (f:Function) RETURN f.name LIMIT 10"

# 3. 追踪调用链（可选参数已有内置默认值：depth=5、无路径过滤）
codenexus trace --symbol main --trace_type calls

# 4. 搜索符号（exact / regex / fuzzy + BM25 全文；limit 默认 50）
codenexus search --text "parse" --mode exact
codenexus search --text "authentication logic" --fulltext true
```

> 💡 **建议**：把 `.codenexus/` 加入项目的 `.gitignore`（索引库与日志都在这个目录里，不应入库）。也可以创建 `.codenexus/config.json` 固化每项目的常用参数（如 `ram_first`、复杂度阈值），详见 [📖 用户指南 · 配置文件](docs/cn/USER_GUIDE.md#️-配置文件)。

---

## 🛠️ CLI 命令

CodeNexus 提供 **39 个子命令**（外加 `codenexus mcp` 服务模式），按功能分组：

- **索引与项目管理**：`index` / `daemon` / `status` / `list` / `clean` / `export` / `import`
- **查询与搜索**：`query` / `search` / `source` / `context`
- **追踪与影响分析**：`trace` / `impact` / `detect_changes` / `rename`
- **分析工具包**：`dead_code` / `architecture` / `complexity` / `community` / `cross_service` / `verify`
- **API 审查与架构图**：`route_map` / `shape_check` / `api_impact` / `tool_map` / `diagram` / `arch_diff`
- **安全与门禁**：`taint` / `supply` / `ci` / `lint`
- **制品与智能体生态**：`hub` / `skill` / `ask` / `evolve`
- **多智能体与 LSP**：`setup` / `hook` / `lsp_goto_def` / `lsp_hover`

启用 `webui` feature 后追加 **`webui`** 子命令（一条命令启动内嵌 graph-viewer Web UI，见 [📖 用户指南 · 内嵌 Web UI](docs/cn/USER_GUIDE.md#️-内嵌-web-uigraph-viewer)）。所有命令同时提供等形态别名 **`cn`**（与 `codenexus` 同一 CLI 面，如 `cn webui`、`cn index ...`）。

每个命令的全部参数语义与可运行示例见 [📖 用户指南 · 命令详解](docs/cn/USER_GUIDE.md#️-命令详解)；复杂度指标与阈值表见 [📖 用户指南 · 复杂度分析](docs/cn/USER_GUIDE.md#-复杂度分析)，死代码检测配置见 [📖 用户指南 · 死代码检测](docs/cn/USER_GUIDE.md#-死代码检测)。

---

## 🔌 MCP 集成

CodeNexus 使用 [sdforge](https://crates.io/crates/sdforge) 提供 MCP（Model Context Protocol）服务器，经 sdforge `mcp` stdio 传输暴露 **11 个工具**（`query` / `trace` / `impact` / `search` / `source` / `context` / `architecture` / `diagram` / `arch_diff` / `dead_code` / `detect_changes`），与同名 CLI 命令共用同一套 `#[forge]` 定义；每个工具的描述包含参数语义与默认值，服务器以只读方式打开数据库，可与写入进程并存。启动、接入与钩子命令见 [📖 用户指南 · 多智能体集成](docs/cn/USER_GUIDE.md#-多智能体集成)。

---

## 📚 文档

<details>
<summary>📋 表格</summary>

| 文档 | 说明 |
|------|------|
| [📖 用户指南](docs/cn/USER_GUIDE.md) | 从安装到进阶的完整使用教程（含复杂度分析与死代码检测详解） |
| [📘 API 参考](docs/cn/API_REFERENCE.md) | 库 crate 公开 API、Facade 接口与 CLI/MCP 对外接口 |
| [⚡ 性能指南](docs/cn/PERFORMANCE.md) | 基准套件、实测基线、SLO 与内存优化（L1–L7 防线） |
| [🔒 安全文档](docs/cn/SECURITY.md) | 安全策略、漏洞报告流程与最佳实践 |
| [❓ FAQ](docs/cn/FAQ.md) | 常见问题解答 |
| [🧪 测试场景矩阵](docs/cn/TEST_SCENARIOS.md) | 基于真实测试套件的场景穷举矩阵 |
| [📋 更新日志](docs/cn/CHANGELOG.md) | 每个版本的变更记录（Keep a Changelog 格式） |
| [🚀 发布对账清单](docs/cn/RELEASING.md) | 发布前文档数字对账清单 |
| [🤝 贡献指南](docs/cn/CONTRIBUTING.md) | 如何参与项目开发 |
| [📜 行为准则](docs/cn/CODE_OF_CONDUCT.md) | 社区行为准则 |
| [🔬 研究笔记](docs/research/) | TaintRadar、级联漏洞链等论文笔记 |
| [🛡️ 安全审计](docs/security/) | Strix 审计 triage 与 ReDoS 误报复核实证 |
| [🤖 CLI 技能](skill/SKILL.md) | 面向 AI 智能体的 CLI 用法知识包（针对 v0.4.0-rc.2 校验） |
| [📈 基准测试说明](benches/README.md) | Criterion 基准套件与 SLO 阈值表 |
| [📦 crates.io](https://crates.io/crates/codenexus) | 发布页面 |

</details>

---

## 💻 示例

全部 14 个可运行示例位于 [`examples/`](examples/) 目录（清单表格与运行命令见 [📖 用户指南 · 示例](docs/cn/USER_GUIDE.md#-示例)）。示例通过 `IndexFacade` 索引源码、`QueryFacade` 执行查询、`TraceFacade` 追踪调用；库 API 的完整说明见 [📘 API 参考](docs/cn/API_REFERENCE.md)。

---

## 🏗️ 架构

CodeNexus 采用「库 + 二进制」双目标 crate：`src/lib.rs` 暴露公共 API（模型 / 解析 / 存储 / 索引 / 查询 / 追踪 / service 模块），`src/main.rs` 是 sdforge 驱动的 CLI 二进制；v0.3.2 起 CLI 与 MCP 接口经 sdforge `#[forge]` 宏统一封装在 `src/service/`，每个命令定义 core 函数 + CLI wrapper + MCP wrapper。索引方向为「文件发现 → 增量哈希 → 并行解析 → 符号解析 → 批量入库」。

三层源码结构、索引管线流程图、图模型（45 种节点 / 32 种边与置信度分层）、核心语言提取表与 `architecture` / `diagram` / `arch_diff` 命令输出语义，详见 [📖 用户指南](docs/cn/USER_GUIDE.md)。

---

## 🧪 测试

分层测试策略：`src/` 内联 `#[cfg(test)]` 单元测试 → `tests/` 集成测试 → `tests/acceptance/` 8 语言真实开源项目验收（与 gitnexus 交叉验证）→ `benches/` 7 组 Criterion 基准回归。测试分层总览、与 CI 一致的执行命令、逐条场景矩阵与统计汇总见 [🧪 测试场景矩阵](docs/cn/TEST_SCENARIOS.md)。

---

## 📊 性能

基准套件为 `benches/` 下 7 组 Criterion 基准；L1–L7 内存防线将 70 GB 主机上的索引峰值内存从约 60 GB 降至约 4 GB。完整实测基线、SLO 表与调优方法见 [⚡ 性能指南](docs/cn/PERFORMANCE.md)，SLO 阈值表见 [`benches/README.md`](benches/README.md)。

---

## 🔒 安全

安全设计、供应链门禁与漏洞报告政策见 [🔒 安全文档](docs/cn/SECURITY.md)。请勿通过公开 issue 报告安全漏洞，请优先使用 GitHub [Security Advisories](https://github.com/Kirky-X/codenexus/security/policy) 私密披露通道提交报告。

---

## 🗺️ 开发路线图

已完成方向与后续增强（截至 v0.4.0）见 [📋 更新日志 · 开发路线图](docs/cn/CHANGELOG.md#️-开发路线图)。

---

## 🤝 参与贡献

详细的贡献流程与代码规范请参阅 [🤝 贡献指南](docs/cn/CONTRIBUTING.md)。

- 🐛 [报告 Bug](https://github.com/Kirky-X/codenexus/issues/new)
- 💡 [功能建议](https://github.com/Kirky-X/codenexus/issues)
- 🔧 [提交 PR](https://github.com/Kirky-X/codenexus/pulls)

---

## 📋 更新日志

完整版本历史见 [📋 更新日志](docs/cn/CHANGELOG.md)（遵循 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) 格式，语义化版本）。

---

## 📄 许可证

本项目采用 [MIT](LICENSE) 许可证。

---

## 🙏 致谢

### 🌟 核心依赖

CodeNexus 站在以下优秀开源项目的肩膀上：

<details>
<summary>📋 表格</summary>

| 依赖 | 用途 |
|------|------|
| [lbug](https://github.com/ladybugdb/ladybugdb)（LadybugDB） | 图数据库存储 |
| [tree-sitter](https://tree-sitter.github.io/) + 22 个语言 grammar crate | 多语言 AST 解析 |
| [rayon](https://github.com/rayon-rs/rayon) | 数据并行 |
| [notify](https://github.com/notify-rs/notify) / notify-debouncer-full | 文件监听与去抖 |
| [sdforge](https://crates.io/crates/sdforge) | CLI + MCP 双传输框架（`#[forge]` 宏） |
| [trait-kit](https://crates.io/crates/trait-kit) | 能力注册表 |
| [oxcache](https://crates.io/crates/oxcache) | 查询结果缓存 |
| [inklog](https://crates.io/crates/inklog) | 日志后端（console + 日轮转文件 sink；inklog 以 `default-features = false` 引入，`full` 构建经 vecboost 的 `inklog/compression` 特性合并编译 zstd 压缩后端，轮转文件以 zstd 压缩） |
| [vecboost](https://crates.io/crates/vecboost) | 本地 Candle 向量嵌入推理 |
| [ICU4X](https://github.com/unicode-org/icu4x)（icu_normalizer / icu_casemap） | Unicode 规范化与大小写折叠 |
| [petgraph](https://github.com/petgraph/petgraph) | 社区检测图算法 |
| [criterion](https://github.com/bheisler/criterion.rs) | 基准测试 |

</details>

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
<a href="docs/cn/FAQ.md"><b style="color:#1E40AF">文档 / FAQ</b></a><br>
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
