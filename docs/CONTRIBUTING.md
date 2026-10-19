# 🤝 贡献 CodeNexus

感谢你对 CodeNexus 感兴趣。本文档介绍开发环境搭建方法，以及每个 Pull Request 必须遵守的约定。

## 📋 目录

- [开发环境](#️-开发环境)
- [提交约定 — Conventional Commits](#-提交约定--conventional-commits)
- [Pull Request 流程](#-pull-request-流程)
- [测试与 Lint 要求](#✅-测试与-lint-要求)
- [代码风格](#-代码风格)
- [添加新语言解析器](#-添加新语言解析器)
- [提交 Issue](#-提交-issue)
- [行为准则](#-行为准则)
- [问题](#-问题)

---

## 🧰 开发环境

### 前置依赖

| 工具 | 版本 | 说明 |
|------|------|------|
| Rust 工具链（stable） | 1.95+ | 最低支持版本，在 CI 和 `Cargo.toml` 中固定（`rust-version = "1.95"`）。 |
| Rust nightly 工具链 | 最新 | `cargo fmt` 使用 nightly 专有选项 `imports_granularity` 和 `group_imports`（见 `rustfmt.toml`）。 |
| C/C++ 编译器 | 系统默认 | 构建 `tree-sitter` 语法 crate 所需。 |
| `zstd` CLI | 任意近期版本 | `export`/`import` 用于 `.graph.zst` 制品。 |
| Git | 2.20+ | Hooks 和 `detect-changes` 依赖现代 `git` 行为。 |

使用 [rustup](https://rustup.rs/) 安装两个 Rust 工具链：

```bash
rustup toolchain install 1.95
rustup toolchain install nightly
rustup component add clippy --toolchain 1.95
rustup component add rustfmt --toolchain nightly
```

### 获取源码

```bash
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo build                    # 冒烟构建
cargo test                     # 冒烟测试
```

### 构建变体

默认特性集为 `full`（全部 21 种语言 + daemon + analysis + complexity + api-review + community + cross-service + lsp + mcp + cli + cache + embed）。为加快开发迭代，可以构建更精简的二进制：

```bash
# 最小：仅 Rust，无 daemon（编译最快）
cargo build --no-default-features --features minimal

# 核心：C + Rust + Python，无 daemon
cargo build --no-default-features --features core

# 全部，包括向量嵌入（编译较慢）
cargo build --features embeddings
```

## 📝 提交约定 — Conventional Commits

CodeNexus 遵循 [Conventional Commits](https://conventionalcommits.org/)。每条提交信息必须匹配：

```
<type>(<scope>): <subject>

[可选 body]

[可选 footer]
```

### 允许的 `type` 值

| 类型 | 用途 |
|------|---------|
| `feat` | 面向用户的新功能 |
| `fix` | Bug 修复 |
| `perf` | 不改变行为的性能优化 |
| `refactor` | 既不修复 bug 也不添加功能的代码变更 |
| `docs` | 仅文档变更（README、CHANGELOG、设计文档） |
| `test` | 添加或修正测试 |
| `chore` | 构建、CI、依赖、工具、仓库杂务 |
| `revert` | 回退之前的提交 |

### `scope`

可选但推荐使用。使用模块名（`parse`、`resolve`、`storage`、`query`、`trace`、`cli`、`daemon`、`embed`、`mcp`、`model`、`kit`）。跨模块变更可省略 scope。

### 示例

```
feat(parse): 添加 Go 语言提取器
fix(storage): 防止解析器端点 ID 失同步时的孤儿边
perf(parse): dedupe_qn O(N²) → O(1) HashSet
docs(readme): 添加 Roadmap 和 Acknowledgments 章节
chore(ci): workflow 中固定 Rust 1.95
```

footer 中的 `BREAKING CHANGE:` 或 `feat!:` / `fix!:`（带 `!`）标记破坏性变更，必须包含迁移说明。

## 🔀 Pull Request 流程

1. **Fork** 仓库并 clone 你的 fork。
2. 从 `main` **创建分支**：
   ```bash
   git checkout -b feat/short-description
   ```
   使用与提交 `type` 匹配的前缀：`feat/`、`fix/`、`docs/`、`refactor/`、`perf/`、`test/`、`chore/`。
3. 使用 Conventional Commits 信息**提交**。保持提交聚焦；小提交比一个大提交更容易审查。
4. **推送**到你的 fork：
   ```bash
   git push -u origin feat/short-description
   ```
5. 对 `main` **发起 Pull Request**。填写 PR 模板：
   - 变更内容和原因
   - 关联的 Issue（如 `Closes #123`）
   - 测试方式
   - 破坏性变更（如有）及迁移说明
6. 通过推送新提交来**处理审查反馈**（审查期间不要 force-push，除非被要求）。
7. 由 Kirky.X **squash-merge**。最终提交信息遵循 Conventional Commits。

## ✅ 测试与 Lint 要求

CI 在每个 PR 上运行以下门禁；推送前必须在本地通过：

```bash
# 1. 格式检查（nightly rustfmt）
cargo +nightly fmt --all -- --check

# 2. Lint — 警告视为错误
cargo clippy --all-targets -- -D warnings

# 3. 测试
cargo test --all-features
```

如果变更涉及特性门控路径，还需运行：

```bash
cargo test --no-default-features --features minimal
cargo test --no-default-features --features core
cargo test --features embeddings
```

### 编写测试

- 单元测试位于各模块内的 `#[cfg(test)] mod tests` 块中。
- 集成测试位于 `tests/`，端到端地测试 CLI / 索引 → 查询管线。
- 基准测试位于 `benches/`（`criterion` 框架）。不要以基准数据作为正确性门禁——只以 `cargo test` 为准。
- 测试必须断言有意义的行为（返回值、图结构、副作用），而非仅断言"没有 panic"。

## 🎨 代码风格

- **格式**：`cargo +nightly fmt` 是最终标准。配置位于 [`rustfmt.toml`](rustfmt.toml)：4 空格缩进、100 列宽、Unix 换行、`imports_granularity = Preserve`。
- **编辑器配置**：见 [`.editorconfig`](.editorconfig)（Rust 4 空格、Markdown 2 空格、YAML 2 空格、UTF-8、LF、去除行尾空白）。
- **命名**：遵循标准 Rust 风格（函数/变量/模块用 `snake_case`，类型/trait/枚举用 `PascalCase`）。与代码库现有约定保持一致——有疑问时参考相邻代码。
- **错误处理**：库错误枚举使用 `thiserror`，CLI/`main` 边界使用 `anyhow`。不要静默吞掉错误；将它们暴露出来（参见项目规则"Fail Loud"）。
- **禁止 emoji**：源码、注释和提交信息中不使用 emoji。文档保持纯文本。

## 🌐 添加新语言解析器

如果要添加新语言（如 Go），高层步骤如下：

1. 在 `Cargo.toml` 中添加 `lang-go` 特性，引入 `tree-sitter-go`。
2. 按需将其接入分层预设（`minimal` / `core` / `full`）。
3. 在 `src/parse/` 下实现提取器，遵循现有提取器模式（访问 tree-sitter 节点 → 输出 `Node`/`Edge` 记录）。
4. 在统一 Kit 注册表（`trait-kit`）中注册提取器。
5. 如该语言需要新标签，在 `src/model/` 中添加节点/边类型。
6. 在 `tests/` 中添加测试，索引小型样本并断言提取的图。
7. 更新 `README.md` 的"支持语言"和"特性标志"表。

开始大规模工作前先开 Issue 讨论范围。

## 🐛 提交 Issue

- 使用 [GitHub Issue Tracker](https://github.com/Kirky-X/codenexus/issues)。
- **安全漏洞**不要公开开 Issue——见 [SECURITY.md](SECURITY.md)。
- 包含：CodeNexus 版本（`codenexus --version`）、Rust 版本、操作系统、失败的确切命令、完整错误输出，以及最小复现（如可能，提供小型仓库或文件）。

## 📜 行为准则

参与本项目受 [行为准则](CODE_OF_CONDUCT.md) 约束。贡献即表示同意遵守。

## ❓ 问题

开 [GitHub Discussion](https://github.com/Kirky-X/codenexus/discussions) 或标记为 `question` 的 Issue。请耐心等待——本项目由 Kirky.X 维护。
