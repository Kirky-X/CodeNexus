# CodeNexus

<div align="center">

**基于 LadybugDB 与 tree-sitter 的多语言代码知识图谱工具**

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust Version](https://img.shields.io/badge/rust-1.81%2B-orange.svg)](https://www.rust-lang.org)
[![Build](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml/badge.svg)](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml)

[English](README.md) | 简体中文

</div>

## 简介

CodeNexus 将源代码仓库索引为可查询的知识图谱。它使用 [tree-sitter](https://tree-sitter.github.io/) 进行多语言语法解析，[LadybugDB](https://github.com/ladybugdb/ladybugdb) 进行图存储，支持符号追踪、影响分析和数据流分析。

支持 **5 种语言**：C、Rust、Fortran、Python、TypeScript。

## 核心特性

| 特性 | 说明 |
|------|------|
| 多语言解析 | C / Rust / Fortran / Python / TypeScript，基于 tree-sitter |
| 图数据库 | LadybugDB 图存储，21 种节点类型 + 14 种边类型 |
| 增量索引 | SHA-256 文件哈希比对，仅重新解析变更文件 |
| 并行解析 | Rayon 并行 + 线程局部 parser 池 |
| 符号追踪 | 调用链 (Calls) 与数据流 (DataFlows) 双向追踪 |
| 影响分析 | 变更影响半径分析，按深度分层 |
| 跨语言 FFI | C-Fortran bind(C)、Rust extern 等跨语言调用解析 |
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

| Feature | 默认 | 说明 |
|---------|------|------|
| `daemon` | 启用 | 文件监视守护进程（notify + notify-debouncer-full） |
| `embed` | 关闭 | 向量嵌入语义搜索（reqwest HTTP 客户端） |
| `lsp` | 关闭 | LSP 增强解析（预留，当前未实现） |

```bash
# 精简构建（不含 daemon，减小二进制体积）
cargo build --release --no-default-features

# 完整构建（含嵌入）
cargo build --release --features embed
```

## 快速开始

```bash
# 1. 索引一个代码仓库
codenexus index /path/to/project --name myproject

# 2. 查询函数
codenexus query "MATCH (f:Function) RETURN f.name LIMIT 10"

# 3. 追踪调用链
codenexus trace main --type calls --depth 5

# 4. 分析变更影响
codenexus impact parse_function --depth 3

# 5. 搜索符号
codenexus search "parse" --limit 20

# 6. 查看索引状态
codenexus status

# 7. 启动文件监视守护进程
codenexus daemon /path/to/project --name myproject

# 8. 列出所有项目
codenexus list

# 9. 删除项目
codenexus clean myproject
```

## CLI 命令

| 命令 | 说明 |
|------|------|
| `index` | 索引代码仓库到知识图谱 |
| `query` | 执行 Cypher 查询 |
| `trace` | 追踪符号的调用/数据流路径 |
| `impact` | 分析符号变更的影响半径 |
| `search` | 按名称或内容搜索符号 |
| `daemon` | 启动文件监视守护进程 |
| `status` | 查看索引状态 |
| `list` | 列出所有已索引项目 |
| `clean` | 删除项目及其索引 |

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

- **21 种节点类型**：Project, Folder, File, Module, Class, Struct, Enum, Trait, Impl, Function, Method, Variable, GlobalVar, Parameter, Const, Static, Macro, TypeAlias, Typedef, Namespace, Interface
- **14 种边类型**：Contains, Defines, MemberOf, Calls, FfiCalls, DataFlows, Reads, Writes, Implements, Extends, UsesType, References, Imports, Includes
- 每条边携带置信度分数 (0.0-1.0)

## 支持语言

| 语言 | 节点类型 | 边类型 |
|------|----------|--------|
| C | Function, GlobalVar, Struct, Enum, Typedef, Macro | Calls, Imports, Reads, Writes, Includes |
| Rust | Function, Struct, Enum, Trait, Impl, Const, Static, Macro, Module, TypeAlias | Calls, Imports, Reads, Writes |
| Fortran | Module, Function | Calls, Imports, FfiCalls |
| Python | Function, Method, Class | Calls, Imports, Extends |
| TypeScript | Function, Class, Method, Interface, Enum, TypeAlias, Const | Calls, Imports |

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

## 许可证

[MIT](LICENSE)
