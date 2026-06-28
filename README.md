# CodeNexus

<div align="center">

**A multi-language code knowledge graph tool built on LadybugDB and tree-sitter**

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust Version](https://img.shields.io/badge/rust-1.81%2B-orange.svg)](https://www.rust-lang.org)
[![Build](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml/badge.svg)](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml)

English | [简体中文](README_ZH.md)

</div>

## Overview

CodeNexus indexes source code repositories into a queryable knowledge graph. It uses [tree-sitter](https://tree-sitter.github.io/) for multi-language parsing and [LadybugDB](https://github.com/ladybugdb/ladybugdb) for graph storage, supporting symbol tracing, impact analysis, and data-flow analysis.

Supports **5 languages**: C, Rust, Fortran, Python, TypeScript.

## Key Features

| Feature | Description |
|---------|-------------|
| Multi-language parsing | C / Rust / Fortran / Python / TypeScript via tree-sitter |
| Graph database | LadybugDB storage with 44 node types + 24 edge types |
| Incremental indexing | SHA-256 file hash diffing, re-parses only changed files |
| Parallel parsing | Rayon parallelism + thread-local parser pool |
| RAM-first indexing | LZ4-compress source into memory, single `COPY FROM` dump (`--ram-first`) |
| Symbol tracing | Bidirectional call (Calls) and data-flow (DataFlows) tracing |
| Impact analysis | Change impact radius analysis, layered by depth |
| Disambiguation | Ranked multi-match symbol resolution with `--uid`/`--file`/`--kind` narrowing |
| Confidence tiers | Each edge carries a tier (SameFile / ImportScoped / Global) + 0.0-1.0 score |
| Cross-language FFI | C-Fortran bind(C), Rust extern, and other FFI call resolution |
| Team artifacts | `export`/`import` compressed `.graph.zst` artifacts for sharing indexes |
| Multi-agent MCP | `setup` auto-detects Claude Code/Cursor/Codex; `hook` emits PreToolUse/PostToolUse JSON; `mcp` stdio server |
| File watching | Daemon mode with auto-incremental indexing (`daemon` feature) |
| Vector embedding | Optional semantic search (`embed` feature) |

## Installation

```bash
# Build from source
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo install --path .

# Or compile directly
cargo build --release
```

### Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `daemon` | enabled | File-watching daemon (notify + notify-debouncer-full) |
| `embed` | disabled | Vector embedding semantic search (reqwest HTTP client) |
| `lsp` | disabled | LSP-enhanced extraction (reserved, not yet implemented) |
| `lang-rust` | enabled | Rust language parser (minimal single-language build) |

```bash
# Lean build (no daemon, smaller binary)
cargo build --release --no-default-features

# Minimal single-language build (Rust only, no daemon)
cargo build --release --no-default-features --features lang-rust

# Full build (with embedding)
cargo build --release --features embed
```

## Quick Start

```bash
# 1. Index a codebase
codenexus index /path/to/project --name myproject

# 1b. RAM-first indexing (LZ4 in-memory, faster for small-medium repos)
codenexus index /path/to/project --name myproject --ram-first

# 2. Query functions
codenexus query "MATCH (f:Function) RETURN f.name LIMIT 10"

# 3. Trace call paths (with disambiguation narrowing)
codenexus trace main --type calls --depth 5
codenexus trace main --uid "proj.fn.main.1" --depth 5

# 4. Analyze change impact (filter by confidence)
codenexus impact parse_function --depth 3
codenexus impact parse_function --depth 3 --min-confidence 0.7

# 5. Search symbols
codenexus search "parse" --limit 20

# 6. 360° symbol context
codenexus context main

# 7. Detect git-diff affected symbols
codenexus detect-changes /path/to/project

# 8. Rename a symbol (graph-edits + text-search, --dry-run supported)
codenexus rename old_name new_name --dry-run

# 9. Export / import team artifacts
codenexus export --db ./my.lbug --output team.graph.zst
codenexus import --input team.graph.zst --db ./shared.lbug

# 10. Multi-agent MCP integration
codenexus setup                    # auto-detect agents, write MCP config
codenexus hook                     # emit PreToolUse/PostToolUse JSON
codenexus mcp                      # stdio MCP server (JSON-RPC 2.0)

# 11. Show indexing status
codenexus status

# 12. Start file-watching daemon
codenexus daemon /path/to/project --name myproject

# 13. List all projects
codenexus list

# 14. Remove a project
codenexus clean myproject
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `index` | Index a codebase into the knowledge graph (`--ram-first` for LZ4 in-memory) |
| `query` | Execute a Cypher query |
| `trace` | Trace a symbol's call/data-flow paths (`--uid`/`--file`/`--kind` narrowing) |
| `impact` | Analyze the impact radius of changing a symbol (`--min-confidence` filter) |
| `search` | Search symbols by name or content (`--uid`/`--file`/`--kind` narrowing) |
| `context` | 360° symbol view: incoming calls/imports, outgoing calls, processes |
| `detect-changes` | Git diff → affected symbols + risk_level |
| `rename` | Graph-edits for high-confidence + text-search edits (`--dry-run`) |
| `export` | Export LadybugDB dump → zstd `codenexus.graph.zst` artifact |
| `import` | Import artifact → LadybugDB (optional `--reindex` for local diff) |
| `setup` | Auto-detect installed agents (Claude Code/Cursor/Codex) and write MCP config |
| `hook` | Emit PreToolUse/PostToolUse JSON (exit 0, never blocks) |
| `mcp` | stdio MCP server (JSON-RPC 2.0, protocol 2024-11-05) |
| `daemon` | Start the file-watching daemon |
| `status` | Show indexing status |
| `list` | List all indexed projects |
| `clean` | Remove a project and its index |

## Architecture

```
┌─────────────────────────────────────────────┐
│                   CLI (clap)                 │
├─────────────────────────────────────────────┤
│  Index Pipeline  │  Query  │  Trace │ Daemon │
├──────────────────┴─────────┴────────┴────────┤
│           Resolve (symbol + data-flow)        │
├──────────────────────────────────────────────┤
│        Parse (tree-sitter multi-language)     │
├──────────────────────────────────────────────┤
│     Discover (ignore)  │  Storage (LadybugDB) │
└──────────────────────────────────────────────┘
```

### Indexing Pipeline

1. **File discovery** — `ignore` crate honors `.gitignore` rules
2. **Incremental hashing** — SHA-256 diffing, skips unchanged files
3. **Parallel parsing** — Rayon parallelism + tree-sitter node/edge extraction
4. **Symbol resolution** — FQN generation, call resolution, data-flow analysis, cross-language FFI
5. **Bulk loading** — CSV generation + `COPY FROM` batch insert

### Graph Model

- **44 node types**: Project, Folder, File, Module, Class, Struct, Enum, Trait, Impl, Function, Method, Variable, GlobalVar, Parameter, Const, Static, Macro, TypeAlias, Typedef, Namespace, Interface, Constructor, Property, Record, Delegate, Annotation, Template, Union, Variant, Field, Event, Handler, Middleware, Service, Endpoint, Route, Process, Database, Config, Test, Section, Community, Tool, Embedding
- **24 edge types**: Contains, Defines, MemberOf, Calls, FfiCalls, DataFlows, Reads, Writes, Implements, Extends, UsesType, References, Imports, Includes, HasMethod, HasProperty, Accesses, MethodOverrides, MethodImplements, StepInProcess, HandlesRoute, Fetches, HandlesTool, EntryPointOf
- Each edge carries a confidence score (0.0-1.0) and a confidence tier (`SameFile` / `ImportScoped` / `Global`)

## Supported Languages

| Language | Node Types | Edge Types |
|----------|------------|------------|
| C | Function, GlobalVar, Struct, Enum, Typedef, Macro | Calls, Imports, Reads, Writes, Includes |
| Rust | Function, Struct, Enum, Trait, Impl, Const, Static, Macro, Module, TypeAlias | Calls, Imports, Reads, Writes |
| Fortran | Module, Function | Calls, Imports, FfiCalls |
| Python | Function, Method, Class | Calls, Imports, Extends |
| TypeScript | Function, Class, Method, Interface, Enum, TypeAlias, Const | Calls, Imports |

## Development

```bash
# Run tests
cargo test

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt

# Benchmarks
cargo bench
```

## Contributing

Issues and Pull Requests are welcome. Please ensure `cargo test` and `cargo clippy -- -D warnings` pass.

## License

[MIT](LICENSE)
