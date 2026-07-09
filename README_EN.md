# CodeNexus

<div align="center">

**A multi-language code knowledge graph tool built on LadybugDB and tree-sitter**

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Rust Version](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org) [![Build](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml/badge.svg)](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml)

English | [简体中文](README.md)

</div>

## Overview

CodeNexus indexes source code repositories into a queryable knowledge graph. It uses [tree-sitter](https://tree-sitter.github.io/) for multi-language parsing and [LadybugDB](https://github.com/ladybugdb/ladybugdb) for graph storage, supporting symbol tracing, impact analysis, and data-flow analysis.

CodeNexus turns a codebase into a structured graph of symbols and their relationships (calls, data flows, imports, FFI bindings, ...). Once indexed, you can query the graph with a Cypher subset, trace how a symbol is reached, measure the blast radius of a change, and feed the graph to AI agents through a Model Context Protocol (MCP) server.

Supports **8 languages**: C, Rust, Fortran, Python, TypeScript, Go, Java, C++.

### Typical Use Cases

- **Impact analysis before refactoring** — find every caller of a function across files and languages before editing it.
- **Onboarding a new codebase** — index a repo, then `query`/`context`/`trace` to navigate symbols and their relationships instead of grepping.
- **AI agent grounding** — run `codenexus mcp` so Claude Code / Cursor / Codex can call `query`, `context`, `impact`, and `detect-changes` tools with real call-graph data.
- **Team knowledge sharing** — `export` an index as a `.graph.zst` artifact and `import` it on a teammate's machine.

## Key Features

| Feature | Description |
|---------|-------------|
| Multi-language parsing | C / Rust / Fortran / Python / TypeScript / Go / Java / C++ via tree-sitter |
| Graph database | LadybugDB storage with 44 node types + 24 edge types |
| Incremental indexing | SHA-256 file hash diffing, re-parses only changed files |
| Parallel parsing | Rayon parallelism + thread-local parser pool |
| RAM-first indexing | LZ4-compress source into memory, single `COPY FROM` dump (`--ram-first`) |
| Symbol tracing | Bidirectional call (Calls) and data-flow (DataFlows) tracing |
| Impact analysis | Change impact radius analysis, layered by depth |
| Disambiguation | Ranked multi-match symbol resolution with `--uid`/`--file`/`--kind` narrowing |
| Confidence tiers | Each edge carries a tier (SameFile / ImportScoped / Global) + 0.0-1.0 score |
| Cross-language FFI | C-Fortran `bind(C)`, Rust `extern`, and other FFI call resolution |
| Team artifacts | `export`/`import` compressed `.graph.zst` artifacts for sharing indexes |
| Multi-agent MCP | `setup` auto-detects Claude Code/Cursor/Codex; `hook` emits PreToolUse/PostToolUse JSON; `mcp` stdio server |
| File watching | Daemon mode with auto-incremental indexing (`daemon` feature) |
| Vector embedding | Optional semantic search (`embed` feature) |

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

### Supported Languages

| Language | Node Types | Edge Types |
|----------|------------|------------|
| C | Function, GlobalVar, Struct, Enum, Typedef, Macro | Calls, Imports, Reads, Writes, Includes |
| Rust | Function, Struct, Enum, Trait, Impl, Const, Static, Macro, Module, TypeAlias | Calls, Imports, Reads, Writes |
| Fortran | Module, Function | Calls, Imports, FfiCalls |
| Python | Function, Method, Class | Calls, Imports, Extends |
| TypeScript | Function, Class, Method, Interface, Enum, TypeAlias, Const | Calls, Imports |
| Go | Function, Method, Struct, Interface, TypeAlias | Defines, Calls, Imports |
| Java | Class, Interface, Enum, Method | Defines, Calls, Imports |
| C++ | Function, Method, Class, Struct, Namespace, Enum, Template | Defines, Calls, Imports |

## Quick Start

### Prerequisites

| Dependency | Version | Notes |
|------------|---------|-------|
| Rust toolchain | 1.85+ (stable) | Required for `cargo build`. CI pins 1.85. |
| nightly rustfmt | latest | `cargo fmt` uses nightly-only options (`imports_granularity`, `group_imports`). |
| C/C++ compiler | system default | Required to build tree-sitter grammar crates. |
| `zstd` CLI | any recent version | Used by `export`/`import` for `.graph.zst` artifacts. |

### Installation

```bash
# Build from source
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo install --path .

# Or compile directly (binary at target/release/codenexus)
cargo build --release
```

### Feature Flags

**Preset**: `default = ["full"]`

| Feature | Default | Description |
|---------|---------|-------------|
| `minimal` | — | Minimal preset: `lang-rust` only |
| `core` | — | Core preset: `lang-c` + `lang-rust` + `lang-python` |
| `full` | enabled | Full preset: `core` + Fortran/TypeScript/Go/Java/C++ + daemon/analysis/complexity/api-review/community/cross-service/lsp |
| `lang-c` | — | C language parser (tree-sitter-c) |
| `lang-rust` | enabled | Rust language parser (tree-sitter-rust) |
| `lang-fortran` | — | Fortran language parser (tree-sitter-fortran) |
| `lang-python` | — | Python language parser (tree-sitter-python) |
| `lang-typescript` | — | TypeScript language parser (tree-sitter-typescript) |
| `lang-go` | — | Go language parser (tree-sitter-go) |
| `lang-java` | — | Java language parser (tree-sitter-java) |
| `lang-cpp` | — | C++ language parser (tree-sitter-cpp) |
| `daemon` | enabled | File-watching daemon (notify + notify-debouncer-full) |
| `embed` | disabled | Vector embedding semantic search (reqwest HTTP + local ONNX inference) |
| `lsp` | disabled | LSP-enhanced extraction (rust-analyzer integration, semantic type augmentation) |
| `analysis` | enabled | Dead code detection + architecture overview (pure Cypher aggregation) |
| `complexity` | enabled | AST complexity analysis (cyclomatic/cognitive/nesting/length/Halstead/maintainability/time/space, depends on `analysis`) |
| `api-review` | enabled | API review toolkit (route-map/shape-check/api-impact/tool-map) |
| `community` | enabled | Community detection (Louvain modularity optimization, depends on petgraph) |
| `cross-service` | enabled | Cross-service call chain detection (HTTP route pattern matching) |

```bash
# Minimal build (Rust only, no daemon/analysis)
cargo build --release --no-default-features --features minimal

# Core build (C + Rust + Python)
cargo build --release --no-default-features --features core

# Single-language lean build (e.g., C only)
cargo build --release --no-default-features --features lang-c

# Full build (default, all languages + all features)
cargo build --release

# Build with vector embedding
cargo build --release --features embed
```

### First Index

```bash
# 1. Index a codebase into the knowledge graph
codenexus index /path/to/project --name myproject

# 1b. RAM-first indexing (LZ4 in-memory, faster for small-medium repos)
codenexus index /path/to/project --name myproject --ram-first

# 2. Verify the index
codenexus status
codenexus list

# 3. Start exploring
codenexus query "MATCH (f:Function) RETURN f.name LIMIT 10"
codenexus context main
```

### Common Workflows

```bash
# Trace call paths (with disambiguation narrowing)
codenexus trace main --type calls --depth 5
codenexus trace main --uid "proj.fn.main.1" --depth 5

# Analyze change impact (filter by confidence)
codenexus impact parse_function --depth 3
codenexus impact parse_function --depth 3 --min-confidence 0.7

# Search symbols
codenexus search "parse" --limit 20

# 360° symbol context: incoming calls/imports, outgoing calls, processes
codenexus context main

# Detect git-diff affected symbols before committing
codenexus detect-changes /path/to/project

# Rename a symbol (graph-edits + text-search, --dry-run supported)
codenexus rename old_name new_name --dry-run

# Export / import team artifacts
codenexus export --db ./my.lbug --output team.graph.zst
codenexus import --input team.graph.zst --db ./shared.lbug

# Start file-watching daemon for auto-incremental indexing
codenexus daemon /path/to/project --name myproject

# Remove a project and its index
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
| `dead-code` | Dead code detection (uncalled functions, `analysis` feature) |
| `architecture` | Architecture overview (module dependency graph, `analysis` feature) |
| `complexity` | AST complexity analysis (8 metrics + configurable thresholds, `complexity` feature) |
| `api-route-map` | HTTP route mapping (API endpoint inventory, `api-review` feature) |
| `api-shape-check` | API shape check (request/response structure validation, `api-review` feature) |
| `api-impact` | API change impact analysis (`api-review` feature) |
| `api-tool-map` | Tool mapping (MCP tool inventory, `api-review` feature) |
| `community` | Community detection (Louvain modularity optimization, `community` feature) |
| `cross-service` | Cross-service call chain detection (HTTP route pattern matching, `cross-service` feature) |

## Complexity Analysis

The `complexity` subcommand computes AST complexity metrics for every function in a project, emitting JSON with a `complexity` array and a `summary` aggregate.

### Metrics

| Metric | Field | Description |
|--------|-------|-------------|
| Cyclomatic | `cyclomatic` | McCabe 1976 — branch nodes + explicit exits (return/break/continue) + logical operators |
| Cognitive | `cognitive` | Nesting-weighted SonarQube-style complexity |
| Nesting depth | `nesting_depth` | Maximum branch-node nesting depth |
| Function length | `function_length` | End line − start line + 1 |
| Halstead | `halstead` | Halstead 1977: `n1/n2/N1/N2/volume/difficulty/effort/delivered_bugs` |
| Maintainability Index | `maintainability_index` | Microsoft 2007 revision, 0-100 (higher = better) |
| Time complexity | `time_complexity` | AST-pattern estimate: O(1)/O(log n)/O(n)/O(n log n)/O(n^2)/O(n^3)/O(2^n) |
| Space complexity | `space_complexity` | Allocation-pattern recognition: O(1)/O(n)/O(n^2) |

Each metric is classified Green / Yellow / Red against thresholds; `overall_severity` is the maximum.

### Threshold CLI flags

| Flag | Description |
|------|-------------|
| `--cyclomatic-yellow <N>` / `--cyclomatic-red <N>` | Cyclomatic thresholds |
| `--cognitive-yellow <N>` / `--cognitive-red <N>` | Cognitive thresholds |
| `--nesting-yellow <N>` / `--nesting-red <N>` | Nesting depth thresholds |
| `--func-length-yellow <N>` / `--func-length-red <N>` | Function length thresholds |
| `--halstead-volume-yellow <N>` / `--halstead-volume-red <N>` | Halstead volume thresholds |
| `--maintainability-yellow <N>` / `--maintainability-red <N>` | Maintainability Index thresholds (higher = better) |
| `--time-complexity-yellow <O(...)>` / `--time-complexity-red <O(...)>` | Time complexity thresholds |
| `--space-complexity-yellow <O(...)>` / `--space-complexity-red <O(...)>` | Space complexity thresholds |

`<O(...)>` values: time `O(1)` / `O(log n)` / `O(n)` / `O(n log n)` / `O(n^2)` / `O(n^3)` / `O(2^n)`, space `O(1)` / `O(n)` / `O(n^2)`. Unset flags fall back to defaults.

### Default thresholds

| Metric | Yellow | Red |
|--------|--------|-----|
| cyclomatic | 20 | 25 |
| cognitive | 15 | 20 |
| nesting | 5 | 6 |
| func_length | 100 | 200 |
| halstead_volume | 1000 | 8000 |
| maintainability | 65 | 85 |
| time_complexity | O(n) | O(n^2) |
| space_complexity | O(1) | O(n) |

> `maintainability` is inverted: MI higher = better, so `value >= red → Green`, `value >= yellow → Yellow`, else `Red`.

### Examples

```bash
# Analyse with default thresholds
codenexus complexity myproject

# Custom cyclomatic thresholds (yellow=10, red=15)
codenexus complexity myproject --cyclomatic-yellow 10 --cyclomatic-red 15

# Show only Red functions, sorted by severity
codenexus complexity myproject --red-only --sort-by-severity

# Custom time complexity thresholds (yellow=O(n log n), red=O(n^2))
codenexus complexity myproject --time-complexity-yellow "O(n log n)" --time-complexity-red "O(n^2)"
```

## Configuration

CodeNexus is a CLI tool and is configured primarily through command-line flags. A small number of environment variables are honored:

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | `tracing` log level (`error`/`warn`/`info`/`debug`/`trace`), supports `codenexus=debug` style filtering. |
| `CODENEXUS_DB_PATH` | `./codenexus.lbug` | Default LadybugDB database path used when `--db` is not passed to `index`/`query`/`status`/etc. |

See [`.env.example`](.env.example) for a copy-paste template. CodeNexus does not read a `.env` file itself; that file is for shells or process managers.

### Agent Integration

Run `codenexus setup` to auto-detect installed AI agents (Claude Code, Cursor, Codex) and write the MCP configuration into the right location for each. After setup, the agent can call CodeNexus tools (`query`, `context`, `impact`, `detect-changes`, `rename`, ...) over the MCP stdio server started by `codenexus mcp`.

For Git hooks, `codenexus hook` emits `PreToolUse`/`PostToolUse` JSON events and always exits 0, so it can be wired into a hook without blocking agent actions.

## API Documentation

CodeNexus exposes two programmatic interfaces:

### MCP Server (`codenexus mcp`)

A stdio JSON-RPC 2.0 server implementing [Model Context Protocol](https://modelcontextprotocol.io/) (version 2024-11-05). AI agents call it to query the knowledge graph. Run `codenexus setup` once to register the server with your agent; the agent then starts `codenexus mcp` automatically.

### Library Crate

CodeNexus is published as a Rust library (`codenexus` lib crate, see `Cargo.toml` `[lib]`). Embed the indexing pipeline, query facade, or trace engine in another Rust project by depending on the crate. Runnable usage examples live under [`examples/`](examples/).

### In-Repo Design Docs

Detailed design material is kept in `docs/` (note: some of these files are git-ignored as they are internal working documents):

- `docs/PRD.md` — Product Requirements Document
- `docs/TRD.md` — Technical Requirements Document
- `docs/DDD.md` — Detailed Design Document
- `docs/ADD.md` — Architecture Design Document (ADRs)

## Development

```bash
# Run tests
cargo test

# Lint (CI gate)
cargo clippy -- -D warnings

# Format (requires nightly rustfmt for imports_granularity/group_imports)
cargo +nightly fmt

# Benchmarks
cargo bench
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development workflow, and [`.editorconfig`](.editorconfig) / [`rustfmt.toml`](rustfmt.toml) for style rules.

## Contributing

Issues and Pull Requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) for:

- Development environment setup
- Conventional Commits conventions
- Pull Request workflow
- Test and lint requirements (`cargo test` and `cargo clippy -- -D warnings` must pass)
- Code style (`cargo +nightly fmt`)

By participating, you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Roadmap

CodeNexus is at v0.2.1. Planned work, ordered by current priority:

- [x] v0.1.0 — Multi-language indexing (C/Rust/Fortran/Python/TypeScript), graph schema (44 node types + 24 edge types), `query`/`trace`/`impact`/`context`/`search`, incremental indexing, RAM-first mode, MCP server, team `export`/`import`, daemon mode, confidence tiers, disambiguation
- [x] v0.1.x — Stability and performance hardening: incremental reindex coverage, larger-repo memory tuning, more language-specific edge extraction
- [x] v0.2.0 — `lsp` feature: LSP-enhanced extraction for type-accurate resolution beyond tree-sitter (rust-analyzer integration)
- [x] v0.2.0 — Expand language coverage (Go, Java, C++) behind new `lang-*` features
- [x] v0.2.0 — Analysis toolkit: dead-code detection, architecture overview, API review (route-map/shape-check/api-impact/tool-map), community detection, cross-service link detection
- [x] v0.2.1 — AST complexity analysis: cyclomatic/cognitive complexity, nesting depth, function length with green/yellow/red severity alerting
- [ ] v0.3.0 — Cross-language data-flow tracing end-to-end (currently edges are recorded; multi-hop taint paths need a dedicated query path)
- [ ] v0.3.0 — Vector embedding default-on semantic search once ONNX model size and startup cost are acceptable
- [ ] Future — Web UI / graph visualization on top of the query facade

## License

[MIT](LICENSE)

## Acknowledgments

CodeNexus would not be possible without these projects:

- [tree-sitter](https://tree-sitter.github.io/) — incremental parsing framework that powers all language extractors
- [LadybugDB](https://github.com/ladybugdb/ladybugdb) — graph database backing the knowledge graph
- [Rayon](https://github.com/rayon-rs/rayon) — data-parallel parsing
- [ignore](https://docs.rs/ignore) — `.gitignore`-aware file discovery
- [clap](https://docs.rs/clap) — CLI framework
- [Model Context Protocol](https://modelcontextprotocol.io/) — spec for the `mcp` server
- Every tree-sitter grammar maintainer — the per-language grammar crates do the hard parsing work

Project author: **Kirky.X** — [github.com/Kirky-X](https://github.com/Kirky-X)
