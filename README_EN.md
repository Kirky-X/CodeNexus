<div align="center">

<img src="docs/assets/CodeNexus.png" alt="CodeNexus Logo" width="200">

[![Build](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml/badge.svg)](https://github.com/Kirky-X/codenexus/actions/workflows/ci.yml) [![Crates.io](https://img.shields.io/crates/v/codenexus.svg)](https://crates.io/crates/codenexus) [![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Rust Version](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org)

[中文](README.md) | **English**

**A multi-language code knowledge graph tool built on LadybugDB and tree-sitter**

[✨ Key Features](#-key-features) • [🚀 Quick Start](#-quick-start) • [📚 Documentation](#-documentation) • [💻 Examples](#-examples) • [🤝 Contributing](#-contributing)

</div>

---

<div align="center">

### 🎯 Index once, query the whole repo

Run `codenexus index` once and symbol relationships land in the graph — every question after that goes to the graph:

<table style="width:100%; border-collapse: collapse">
<tr>
<td align="center" width="25%">⚡<br><b>Incremental pipeline</b><br><span style="color:#64748B">hash diffing · reparse only changes</span></td>
<td align="center" width="25%">🕸️<br><b>Property graph</b><br><span style="color:#64748B">44 node types · 30 edge types · Cypher</span></td>
<td align="center" width="25%">🧭<br><b>Multi-hop tracing</b><br><span style="color:#64748B">call chains · data flow · taint paths</span></td>
<td align="center" width="25%">🔌<br><b>Dual entry</b><br><span style="color:#64748B">29 CLI commands + serve mode · 10 MCP tools</span></td>
</tr>
</table>

</div>

---

## 📋 Table of Contents

- [✨ Key Features](#-key-features)
- [🚀 Quick Start](#-quick-start)
- [🛠️ CLI Commands](#️-cli-commands)
- [🔌 MCP Integration](#-mcp-integration)
- [📚 Documentation](#-documentation)
- [💻 Examples](#-examples)
- [🏗️ Architecture](#️-architecture)
- [🧪 Testing](#-testing)
- [📊 Performance](#-performance)
- [🔒 Security](#-security)
- [🗺️ Roadmap](#️-roadmap)
- [🤝 Contributing](#-contributing)
- [📋 Changelog](#-changelog)
- [📄 License](#-license)
- [🙏 Acknowledgments](#-acknowledgments)
- [📞 Contact & Support](#-contact--support)
- [⭐ Star History](#-star-history)

---

## ✨ Key Features

<table style="width:100%; border-collapse: collapse">
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🌐 <b>Multi-language parsing</b><br><span style="color:#64748B">The default <code>full</code> preset supports 21 languages (C, Rust, Fortran, Python, TypeScript, Go, Java, C++, JavaScript, Ruby, Haskell, OCaml, Scala, PHP, C#, Bash, HTML, CSS, JSON, Regex, Verilog); trim it down with <code>lang-*</code> features</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🕸️ <b>Graph database</b><br><span style="color:#64748B">LadybugDB graph storage with 44 node types + 30 edge types, queryable via a Cypher subset</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🔄 <b>Incremental indexing</b><br><span style="color:#64748B">SHA-256 file hash comparison re-parses only changed files; Rayon parallelism + thread-local parser pool</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">💾 <b>RAM-first indexing</b><br><span style="color:#64748B">LZ4-compressed in-memory sources with a single <code>COPY FROM</code> bulk load (<code>--ram_first</code>)</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🔍 <b>Symbol tracing</b><br><span style="color:#64748B">Bidirectional call-chain (Calls) and data-flow (DataFlows) tracing; cross-language multi-hop taint path tracing (<code>TaintPathTracer</code>)</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🎯 <b>Impact analysis</b><br><span style="color:#64748B">Change blast-radius analysis, layered by depth, with multi-edge-type dimensions + risk assessment</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🧩 <b>Ambiguity resolution & confidence tiers</b><br><span style="color:#64748B">Multi-match symbols are resolved by ranked confidence; every edge carries a tier (SameFile / ImportScoped / Global) + 0.0-1.0 score</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">📐 <b>Architecture diagrams</b><br><span style="color:#64748B"><code>diagram</code> compiles the architecture into a self-contained interactive HTML (deterministic layout / orthogonal routing / dark & light themes / source-evidence badges)</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🆚 <b>Architecture semantic delta</b><br><span style="color:#64748B"><code>arch_diff</code> compares two indexed projects, emitting Before/Delta/After HTML + a machine receipt (added/removed/changed + JSON Pointer fields)</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🧾 <b>Diagnostic receipts</b><br><span style="color:#64748B">Errors and warnings emit structured receipts (stable rule codes + evidence + actionable fixes); symbol ambiguity / stale index / truncated results all ship repair hints</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🔗 <b>Cross-language FFI</b><br><span style="color:#64748B">C-Fortran bind(C), Rust extern, and other cross-language call resolution</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">📦 <b>Team artifacts</b><br><span style="color:#64748B"><code>export</code> / <code>import</code> compressed <code>.graph.zst</code> artifacts for sharing indexes</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🤖 <b>Multi-agent MCP</b><br><span style="color:#64748B"><code>setup</code> auto-detects Claude Code / Cursor / Codex; <code>hook</code> emits PreToolUse/PostToolUse JSON; <code>mcp</code> stdio server exposes 10 tools with parameter semantics in each tool description</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">👁️ <b>File watching</b><br><span style="color:#64748B">Daemon mode with automatic incremental indexing (<code>daemon</code> feature, graceful SIGTERM/SIGINT shutdown)</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🧮 <b>Analysis toolkit</b><br><span style="color:#64748B">Dead-code detection (worklist reachability + confidence), architecture overview, complexity analysis (8 metrics), community detection (Leiden), cross-service call chains</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🧠 <b>Vector embeddings</b><br><span style="color:#64748B">Semantic search enabled by default (<code>embed</code> feature, local ONNX inference + BM25 full-text)</span></td>
</tr>
<tr>
<td width="50%" style="vertical-align:top; padding: 12px">🌍 <b>Internationalization</b><br><span style="color:#64748B">Unicode case folding + NFC normalization (ICU4X, <code>i18n</code> feature, included in the <code>full</code> preset)</span></td>
<td width="50%" style="vertical-align:top; padding: 12px">🧰 <b>LSP enrichment</b><br><span style="color:#64748B">7 LSP clients (rust-analyzer, pyright, clangd, gopls, ts-lang-server, fortls, jdtls) provide type-accurate resolution beyond tree-sitter (<code>lsp</code> feature)</span></td>
</tr>
</table>

Beyond the core capabilities above, CodeNexus also ships `context` assembly, `detect_changes` change detection, `rename` rename-impact pre-checks, oxcache-backed query-result caching, and structured logging via inklog; the grouped list of all 29 subcommands (plus the `mcp` serve mode) lives in the [🛠️ CLI Commands](#️-cli-commands) section, and per-command flag semantics with runnable examples are in the [📖 User Guide · Command reference](docs/USER_GUIDE.md#️-命令详解).

---

## 🚀 Quick Start

### 📦 Installation

```bash
# Install from crates.io (default full preset, all 21 languages + all features)
cargo install codenexus

# Build from source
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo install --path .

# Or compile directly
cargo build --release
```

> **Link failure troubleshooting (openEuler / CentOS and other GCC ≤ 12 systems)**: the default install downloads a prebuilt LadybugDB binary that requires a recent `libstdc++`. If linking fails with `undefined symbol: std::to_chars(..., _Float128, ...)`, force a source build via an environment variable (requires `cmake`):
>
> ```bash
> LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus
> ```

Requires Rust 1.97.1 or later (MSRV, matching `rust-version` in `Cargo.toml` and `msrv` in `clippy.toml`; the CI toolchain is currently pinned to 1.95, see `.github/workflows/ci.yml`).

#### 🔧 Build presets and feature flags

**Presets**: `default = ["full"]`

| Feature           | Default | Description |
| ----------------- | ------- | ----------- |
| `minimal`         | —       | Minimal preset: `lang-rust` only |
| `core`            | —       | Core preset: `lang-c` + `lang-rust` + `lang-python` |
| `full`            | Enabled | Full preset: `core` + Fortran/TypeScript/Go/Java/C++/JavaScript/Ruby/Haskell/OCaml/Scala/PHP/C#/Bash/HTML/CSS/JSON/Regex/Verilog + daemon/analysis/complexity/api-review/community/cross-service/diagram/lsp/cli/mcp/cache/embed/i18n |
| `lang-c`          | —       | C parser (tree-sitter-c) |
| `lang-rust`       | Enabled | Rust parser (tree-sitter-rust) |
| `lang-fortran`    | —       | Fortran parser (tree-sitter-fortran) |
| `lang-python`     | —       | Python parser (tree-sitter-python) |
| `lang-typescript` | —       | TypeScript parser (tree-sitter-typescript) |
| `lang-go`         | —       | Go parser (tree-sitter-go) |
| `lang-java`       | —       | Java parser (tree-sitter-java) |
| `lang-cpp`        | —       | C++ parser (tree-sitter-cpp) |
| `lang-javascript` | —       | JavaScript parser (tree-sitter-javascript) |
| `lang-ruby`       | —       | Ruby parser (tree-sitter-ruby) |
| `lang-haskell`    | —       | Haskell parser (tree-sitter-haskell) |
| `lang-ocaml`      | —       | OCaml parser (tree-sitter-ocaml) |
| `lang-scala`      | —       | Scala parser (tree-sitter-scala) |
| `lang-php`        | —       | PHP parser (tree-sitter-php) |
| `lang-csharp`     | —       | C# parser (tree-sitter-c-sharp) |
| `lang-bash`       | —       | Bash parser (tree-sitter-bash) |
| `lang-html`       | —       | HTML parser (tree-sitter-html) |
| `lang-css`        | —       | CSS parser (tree-sitter-css) |
| `lang-json`       | —       | JSON parser (tree-sitter-json) |
| `lang-regex`      | —       | Regex parser (tree-sitter-regex) |
| `lang-verilog`    | —       | Verilog parser (tree-sitter-verilog) |
| `daemon`          | Enabled | File-watching daemon (notify + notify-debouncer-full) |
| `embed`           | Enabled | Vector embedding semantic search (reqwest HTTP + local ONNX inference) |
| `lsp`             | Enabled | LSP-enriched parsing (7 LSP clients: rust-analyzer, pyright, clangd, gopls, ts-lang-server, fortls, jdtls) |
| `analysis`        | Enabled | Dead-code detection + architecture overview (pure Cypher aggregation) |
| `complexity`      | Enabled | AST complexity analysis (8 metrics, depends on `analysis`) |
| `api-review`      | Enabled | API review toolkit (route_map/shape_check/api_impact/tool_map) |
| `community`       | Enabled | Community detection (Leiden modularity optimization, depends on petgraph) |
| `cross-service`   | Enabled | Cross-service call-chain detection (HTTP route pattern matching) |
| `diagram`         | Enabled | Architecture diagram pipeline: `diagram`/`arch_diff` commands (depends on `analysis`) |
| `mcp`             | Enabled | MCP server (sdforge `mcp` stdio transport) |
| `cli`             | Enabled | CLI binary (sdforge `cli` transport, required for the binary) |
| `cache`           | Enabled | Query result cache (oxcache) |
| `i18n`            | Enabled | Unicode case folding + NFC normalization (ICU4X) |

> **Logging**: inklog is the only log backend (console + file rotation + daily rolling + LZ4 compression); the optional tracing-subscriber backend has been removed.

```bash
# Minimal build (Rust only, no daemon/analysis)
cargo build --release --no-default-features --features minimal

# Core build (C + Rust + Python)
cargo build --release --no-default-features --features core

# Single-language lean build (e.g., C only)
cargo build --release --no-default-features --features lang-c

# Full build (default, all languages + all features)
cargo build --release

# Build with vector embeddings
cargo build --release --features embed
```

### 💡 Minimal example

The following commands are adapted from [`examples/src/bin/basic_indexing.rs`](examples/src/bin/basic_indexing.rs) and other examples plus the [📖 User Guide](docs/USER_GUIDE.md), and run out of the box:

```bash
# 1. Index a repository (the database defaults to .codenexus/<project>.lbug)
codenexus index --path /path/to/project --name myproject

# 1b. RAM-first indexing (LZ4 in-memory compression, faster for small-to-medium repos)
codenexus index --path /path/to/project --name myproject --ram_first true

# 2. Query functions (Cypher subset)
codenexus query --cypher "MATCH (f:Function) RETURN f.name LIMIT 10"

# 3. Trace call chains (optional flags have built-in defaults: depth=5, no path filter)
codenexus trace --symbol main --trace_type calls

# 4. Search symbols (exact / regex / fuzzy + BM25 full-text; limit defaults to 50)
codenexus search --text "parse" --mode exact
codenexus search --text "authentication logic" --fulltext true
```

> 💡 **Tip**: add `.codenexus/` to your project's `.gitignore` (the index database and logs both live there). You can also create `.codenexus/config.json` to pin per-project defaults (e.g. `ram_first`, complexity thresholds) — see the [📖 User Guide · Configuration file](docs/USER_GUIDE.md#️-配置文件).

### 🧭 Core concepts

- **Knowledge graph model**: source code is parsed into a property graph of 44 node types and 30 edge types, stored in LadybugDB and queryable via a Cypher subset.
- **Strict flag-based CLI**: no positional arguments; parameters are snake_case long options (e.g. `--symbol`, `--trace_type`) and booleans take explicit values (`true`/`false`). Optional parameters carry built-in defaults (e.g. `trace --depth 5`, `search --limit 50` — full list via each command's `--help`) and can be pinned per project via `.codenexus/config.json`.
- **Global `--db` option**: the database path defaults to `.codenexus/<project>.lbug` and must precede the subcommand; when exactly one index exists it is auto-discovered.
- **Exit-code contract**: 0 success, 1 internal error, 2 invalid input / project not found / query error, 4 not found / database corrupt (see `src/service/error.rs`).

> The full set of core conventions (incremental indexing, confidence tiers, etc.) is in the [📖 User Guide · Core conventions](docs/USER_GUIDE.md#-核心约定).

---

## 🛠️ CLI Commands

CodeNexus ships **29 subcommands** (plus the `codenexus mcp` serve mode), grouped by function:

- **Indexing & project management**: `index` / `daemon` / `status` / `list` / `clean` / `export` / `import`
- **Query & search**: `query` / `search` / `context`
- **Tracing & impact analysis**: `trace` / `impact` / `detect_changes` / `rename`
- **Analysis toolkit**: `dead_code` / `architecture` / `complexity` / `community` / `cross_service`
- **API review & diagrams**: `route_map` / `shape_check` / `api_impact` / `tool_map` / `diagram` / `arch_diff`
- **Multi-agent & LSP**: `setup` / `hook` / `mcp` / `lsp_goto_def` / `lsp_hover`

Full flag semantics and runnable examples for every command are in the [📖 User Guide · Command reference](docs/USER_GUIDE.md#️-命令详解); complexity metrics and thresholds in the [📖 User Guide · Complexity Analysis](docs/USER_GUIDE.md#-复杂度分析), dead-code configuration in the [📖 User Guide · Dead Code Detection](docs/USER_GUIDE.md#-死代码检测).

---

## 🔌 MCP Integration

CodeNexus uses [sdforge](https://crates.io/crates/sdforge) to provide an MCP (Model Context Protocol) server exposing **10 tools** (`query` / `trace` / `impact` / `search` / `context` / `architecture` / `diagram` / `arch_diff` / `dead_code` / `detect_changes`) over the sdforge `mcp` stdio transport. The tools share the same `#[forge]` definitions as the identically named CLI commands; each tool description documents its parameter semantics and defaults, and the server opens the database read-only so it can run alongside a writer.

```bash
# Start the MCP server (stdio)
codenexus mcp [--db <DB_PATH>]

# Auto-detect installed Claude Code / Cursor / Codex and write MCP configs (--force skips the prompt)
codenexus setup

# Emit PreToolUse/PostToolUse JSON (exit 0, never blocks; meant as an agent hook)
codenexus hook
```

What each tool does is described in the [📖 User Guide · Multi-agent integration](docs/USER_GUIDE.md#-多智能体集成).

---

## 📚 Documentation

| Document | Description |
| -------- | ----------- |
| [📖 User Guide](docs/USER_GUIDE.md) | Complete tutorial from installation to advanced usage (incl. complexity analysis and dead-code detection) |
| [📘 API Reference](docs/API_REFERENCE.md) | Library crate public API, facades, and CLI/MCP external interfaces |
| [🏗️ Architecture](docs/ARCHITECTURE.md) | Layered structure, indexing pipeline, graph model, and diagram command semantics |
| [⚡ Performance Guide](docs/PERFORMANCE.md) | Benchmark suite, measured baselines, SLOs, and the L1–L7 memory defenses |
| [🔒 Security](docs/SECURITY.md) | Security policy, vulnerability reporting, and best practices |
| [❓ FAQ](docs/FAQ.md) | Frequently asked questions |
| [🧪 Test Scenario Matrix](docs/TEST_SCENARIOS.md) | Scenario matrix exhaustively derived from the real test suite |
| [📋 Changelog](docs/CHANGELOG.md) | Release-by-release changes (Keep a Changelog format) |
| [🤝 Contributing](docs/CONTRIBUTING.md) | How to contribute to the project |
| [📜 Code of Conduct](docs/CODE_OF_CONDUCT.md) | Community code of conduct |
| [📐 Architecture Design Doc (ADD)](docs/ADD.md) | Architecture decisions and design details (Chinese) |
| [🎯 Product Requirements (PRD)](docs/PRD.md) | Product requirements and SLO metrics (Chinese) |
| [🧾 Technical Requirements (TRD)](docs/TRD.md) | Technical requirement breakdown (Chinese) |
| [🗄️ Database Design Doc (DDD)](docs/DDD.md) | Graph storage schema design (Chinese) |
| [🗜️ Database Compression](docs/database-compression.md) | gzip / zstd / lz4 compression ratios and timing measurements (Chinese) |
| [🔬 Research Notes](docs/research/) | Paper notes: TaintRadar, cascaded vulnerability chains |
| [🛡️ Security Audits](docs/security/) | Strix audit triage with a ReDoS false-positive proof |
| [🤖 CLI Skill](skill/SKILL.md) | Agent-facing CLI usage knowledge pack (verified against v0.3.12) |
| [📈 Benchmark Notes](benches/README.md) | Criterion benchmark suite and SLO threshold table |
| [📦 crates.io](https://crates.io/crates/codenexus) | Publication page |

---

## 💻 Examples

All 14 runnable examples live in [`examples/`](examples/), each registered as a `cargo run --bin` target in `examples/Cargo.toml`:

| Example | File | Description |
| ------- | ---- | ----------- |
| basic_indexing | `examples/src/bin/basic_indexing.rs` | Index Rust source into the knowledge graph, list functions via Cypher |
| cypher_query | `examples/src/bin/cypher_query.rs` | Run a variety of Cypher queries (by type, by name) |
| symbol_search | `examples/src/bin/symbol_search.rs` | Search symbols by name and type, handle empty results |
| call_tracing | `examples/src/bin/call_tracing.rs` | Trace function call paths forward, build a call graph |
| impact_analysis | `examples/src/bin/impact_analysis.rs` | Analyze the blast radius of changing a symbol (reverse BFS) |
| symbol_context | `examples/src/bin/symbol_context.rs` | 360° symbol view (callers / callees / execution flows), subgraph loading and symbol disambiguation |
| export_import | `examples/src/bin/export_import.rs` | Graph database export/import verification |
| project_lifecycle | `examples/src/bin/project_lifecycle.rs` | Project lifecycle: index multiple projects → list → resolve by name → remove |
| code_analysis | `examples/src/bin/code_analysis.rs` | Code-quality trio: complexity / dead code / community detection |
| api_surface | `examples/src/bin/api_surface.rs` | API surface analysis: route map / schema check / API impact / cross-service calls / MCP tool map |
| architecture_diagram | `examples/src/bin/architecture_diagram.rs` | Architecture overview + self-contained interactive HTML diagram + two-project architecture diff |
| daemon_watch | `examples/src/bin/daemon_watch.rs` | File-watching daemon: `notify` debounce → incremental indexing (Observer pattern) → graceful stop |
| git_integration | `examples/src/bin/git_integration.rs` | Git integration: map `git diff` changed lines to affected symbols with risk grading |
| setup_mcp | `examples/src/bin/setup_mcp.rs` | MCP onboarding: auto-detect installed AI coding agents and write MCP server config |

```bash
# Run a single example
cargo run --manifest-path examples/Cargo.toml --bin basic_indexing

# Run all examples
for bin in basic_indexing cypher_query symbol_search call_tracing impact_analysis symbol_context export_import project_lifecycle code_analysis api_surface architecture_diagram daemon_watch git_integration setup_mcp; do
  cargo run --manifest-path examples/Cargo.toml --bin $bin
done
```

Examples use `IndexFacade` to index source, `QueryFacade` to query, and `TraceFacade` to trace calls; temp directories are cleaned up on exit. Full library API details in the [📘 API Reference](docs/API_REFERENCE.md).

---

## 🏗️ Architecture

CodeNexus is a dual-target "library + binary" crate: `src/lib.rs` exposes the public API (model / parse / storage / index / query / trace / service modules) and `src/main.rs` is the sdforge-driven CLI binary; since v0.3.2 the CLI and MCP interfaces are unified through the sdforge `#[forge]` macro inside `src/service/`, where each command defines a core function + CLI wrapper + MCP wrapper. Indexing flows through "file discovery → incremental hashing → parallel parsing → symbol resolution → bulk load".

The three-layer source structure, pipeline diagram, graph model (44 node / 30 edge types with confidence tiers), per-language extraction table, and the output semantics of `architecture` / `diagram` / `arch_diff` live in the [🏗️ Architecture doc](docs/ARCHITECTURE.md).

---

## 🧪 Testing

### 🎯 Test strategy

Layered strategy: inline `#[cfg(test)]` unit tests in `src/` → integration tests in `tests/` (CLI child-process E2E, full-feature suite, MCP/diagram integration, non-ASCII paths) → acceptance tests in `tests/acceptance/` over 8 real open-source projects (cross-validated against gitnexus) → 7 Criterion benchmark groups in `benches/` for regression guarding. The layer overview and the exhaustive scenario matrix live in the [🧪 Test Scenario Matrix](docs/TEST_SCENARIOS.md).

### ▶️ Commands (identical to CI)

```bash
# Format check (nightly rustfmt; rustfmt.toml uses nightly-only options)
cargo +nightly fmt --all -- --check

# Clippy gate (CI runs both the full and minimal tiers; warnings are errors)
cargo +1.95 clippy -- -D warnings
cargo +1.95 clippy --lib --no-default-features --features minimal -- -D warnings

# Tests (CI matrix runs minimal / core / full / core,daemon,analysis,complexity / core,lsp,cache / full,embed)
cargo test --lib --verbose
cargo test --lib --no-default-features --features "core" --verbose

# Coverage gate: at least 95% line coverage (CI coverage job + pre-push hook)
cargo llvm-cov --lib --fail-under-lines 95 --lcov --output-path lcov.info

# Benchmarks (--quick stops once statistical significance is reached)
cargo bench -- --quick
cargo bench --bench daemon_bench --features daemon -- --quick

# Security audits (CI security job: RustSec advisories + license/banned-deps)
cargo audit
cargo deny check
```

> CI also runs CodeQL static analysis on every push/PR (`.github/workflows/codeql.yml`), and the Release workflow fires on `v*` tags (GitHub Release + crates.io publish).

### 📊 Test scale

As of v0.3.12 (grep count of `#[test]` / `#[tokio::test]` functions): ~4600+ unit tests (147 source files contain `#[cfg(test)]` modules) + 118 integration tests (8 files) + 8 acceptance projects + 7 Criterion benchmark groups; the coverage gate is ≥95% line coverage, enforced by both the CI coverage job and the pre-push hook. The per-file breakdown and full statistics live in [🧪 Test Scenario Matrix · Statistics](docs/TEST_SCENARIOS.md#-统计汇总).

---

## 📊 Performance

The benchmark suite is 7 Criterion groups in `benches/` (SLO thresholds from `docs/PRD.md` §5.1): measured cold-start indexing of 1000 files is ~3929 files/s (SLO ≥ 100), single-file incremental ~4987 files/s (SLO ≥ 500), and daemon debounce response ~2.76 s (SLO ≤ 3 s); `incremental_500_of_1000` is a known gap. The L1–L7 memory defenses shipped in v0.3.10–v0.3.12 (`MemoryBudget` three-level memory pressure, streaming CSV, pipeline streaming, buffer_pool cap, etc.) reduced index peak memory on a 70 GB host from ~60 GB to ~4 GB. The full measured baseline, SLO table, and tuning recipes are in the [⚡ Performance Guide](docs/PERFORMANCE.md); the SLO threshold table in [`benches/README.md`](benches/README.md).

---

## 🔒 Security

### 🛡️ Security design

The attack surface is concentrated in index files (LadybugDB databases, `.graph.zst` import artifacts, tree-sitter parse inputs) and the user input fed into Cypher-subset queries by `query` / `trace` / `impact` / `search`; on the code side this is paired with Cypher/identifier escaping, dry-run-by-default graph edits, and fail-loud diagnostic receipts. Design details and scope are in the [🔒 Security doc](docs/SECURITY.md).

### ⛓️ Supply chain and gates

CI enforces four gates: `cargo-audit` (RustSec advisory scanning), `cargo-deny` (license / banned-dependency checks), CodeQL static analysis, and pre-commit secret scanning. The full list and the ignored-advisory notes are in the [🔒 Security doc](docs/SECURITY.md#️-supply-chain-and-gates).

### 🚨 Reporting a vulnerability

Do not report security vulnerabilities through public issues. Email **security@kirky-x.dev** instead, including a description of the vulnerability and its impact, reproduction steps (a minimal codebase or `codenexus` command sequence), version info (`codenexus --version`, Rust toolchain, OS), and any known mitigations. The project commits to acknowledging within 48 hours and giving an initial assessment within 5 business days. The full policy (supported versions, disclosure process, scope) is in the [🔒 Security doc](docs/SECURITY.md).

---

## 🗺️ Roadmap

<table style="width:100%; border-collapse: collapse">
<tr><th style="text-align:center">Status</th><th style="text-align:left">Area</th><th style="text-align:left">Items</th></tr>
<tr><td align="center">✅</td><td>Core indexing & graph model</td><td>v0.1.0 — Multi-language indexing (C/Rust/Fortran/Python/TypeScript), graph schema (44 node types + 30 edge types), <code>query</code>/<code>trace</code>/<code>impact</code>/<code>context</code>/<code>search</code>, incremental indexing, RAM-first mode, MCP server, team <code>export</code>/<code>import</code>, daemon mode, confidence tiers, ambiguity resolution</td></tr>
<tr><td align="center">✅</td><td>Stability & performance hardening</td><td>v0.1.x — Incremental re-index coverage, large-repo memory tuning, more language-specific edge extraction</td></tr>
<tr><td align="center">✅</td><td>LSP enrichment</td><td>v0.2.0 — <code>lsp</code> feature: LSP-enriched extraction with type-accurate resolution beyond tree-sitter (rust-analyzer integration)</td></tr>
<tr><td align="center">✅</td><td>Expanded language coverage</td><td>v0.2.0 — Expanded language coverage (Go, Java, C++, plus JavaScript/Ruby/Haskell/OCaml/Scala/PHP/C#/Bash/HTML/CSS/JSON/Regex/Verilog) controlled by new <code>lang-*</code> features</td></tr>
<tr><td align="center">✅</td><td>Analysis toolkit</td><td>v0.2.0 — Dead-code detection, architecture overview, API review (route_map/shape_check/api_impact/tool_map), community detection, cross-service link detection</td></tr>
<tr><td align="center">✅</td><td>Complexity analysis</td><td>v0.2.1 — AST complexity analysis: cyclomatic/cognitive complexity, nesting depth, function length, green/yellow/red/critical alerts</td></tr>
<tr><td align="center">✅</td><td>MCP server</td><td>v0.3.0 — sdforge-based MCP server: <code>#[forge]</code> macro + sdforge <code>mcp</code> stdio transport replacing hand-written JSON-RPC; 6 tools (query/trace/impact/search/context/architecture)</td></tr>
<tr><td align="center">✅</td><td>Cross-language taint tracing</td><td>v0.3.2 — Cross-language data-flow end-to-end tracing: <code>TaintPathTracer</code> BFS over DataFlows/Reads/Writes/FfiCalls edges</td></tr>
<tr><td align="center">✅</td><td>Semantic search</td><td>v0.3.2 — Vector embeddings on by default for semantic search (<code>embed</code> feature included in the <code>full</code> preset)</td></tr>
<tr><td align="center">✅</td><td>Internationalization</td><td>v0.3.3 — Internationalization module (<code>i18n</code> feature): ICU4X Unicode case folding + NFC normalization + CJK boundary detection</td></tr>
<tr><td align="center">✅</td><td>Harness modernization</td><td>v0.3.3 — CI upgrade to Rust 1.91 + 6-feature matrix + dependabot + codeql + crates.io publishing</td></tr>
<tr><td align="center">✅</td><td>Large-repo memory defenses</td><td>v0.3.11 — Large-repo indexing OOM fix (L1–L7 seven-layer defense): <code>MemoryBudget</code> three-level memory pressure + <code>Graph::nodes_view/edges_view</code> iterators + streaming CSV + mpsc channel parallel parsing + L5 adaptive degradation + L6 pipeline streaming (<code>ctx.remove</code> replaces <code>Graph::clone</code>) + L7 LadybugDB buffer_pool cap (4 GB) + on-demand LSP startup + RAM-first 8× amplification budget. Peak memory on a 70 GB host dropped from 60 GB to ~4 GB</td></tr>
<tr><td align="center">🚧</td><td>In-house base library upgrades</td><td>RC upgrade of in-house base libraries (trait-kit / sdforge / oxcache 0.5.0-rc.2, inklog 0.3.0-rc.2); MSRV 1.95 → 1.97.1</td></tr>
<tr><td align="center">📋</td><td>Web UI & graph visualization</td><td>Web UI / graph visualization on top of the query facade (<code>diagram</code>/<code>arch_diff</code> already ship architecture HTML and semantic deltas; 3D graph-viewer integration and more chart types are still planned)</td></tr>
</table>

---

## 🤝 Contributing

For the detailed contribution process and code standards, see the [🤝 Contributing Guide](docs/CONTRIBUTING.md).

### 🛠️ Development environment

The toolchain is Rust stable 1.95+ (CI pins 1.95; `Cargo.toml` MSRV is 1.97.1) plus nightly (`cargo fmt` uses nightly-only options); system dependencies are a C/C++ compiler (for tree-sitter grammar builds), `libssl-dev`, `pkg-config`, and `protobuf-compiler`; run `cargo +nightly fmt --all -- --check` and `cargo clippy -- -D warnings` before committing; the [pre-commit](https://pre-commit.com/) Git hooks run file checks, private-key/secret scanning, fmt, and clippy on pre-commit, and `cargo test --lib`, the coverage gate (≥95%), `cargo audit`, and `cargo deny check` on pre-push; commit messages follow Conventional Commits (`feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`, `revert`). For the full environment setup, see the [🤝 Contributing Guide · Development Environment](docs/CONTRIBUTING.md#-development-environment).

### 💖 Ways to contribute

<table style="width:100%; border-collapse: collapse">
<tr>
<td width="33%" align="center" style="padding: 16px">

### 🐛 Report a Bug

Found a problem?<br>
<a href="https://github.com/Kirky-X/codenexus/issues/new">Open an Issue</a>

</td>
<td width="33%" align="center" style="padding: 16px">

### 💡 Suggest a Feature

Have an idea?<br>
<a href="https://github.com/Kirky-X/codenexus/issues">Propose a Feature</a>

</td>
<td width="33%" align="center" style="padding: 16px">

### 🔧 Submit a PR

Want to contribute code?<br>
<a href="https://github.com/Kirky-X/codenexus/pulls">Fork & open a PR</a>

</td>
</tr>
</table>

When filing an issue, please include: CodeNexus version (`codenexus --version`), Rust version, OS, the exact command, the full error output, and a minimal reproduction. Security vulnerabilities must not be filed publicly — see the [🔒 Security doc](docs/SECURITY.md).

---

## 📋 Changelog

The full release history lives in the [📋 Changelog](docs/CHANGELOG.md) (following the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format, semantic versioning).

| Version | Date | Highlights |
| ------- | ---- | ---------- |
| Unreleased | — | RC upgrade of in-house base libraries (trait-kit / sdforge / oxcache 0.5.0-rc.2, inklog 0.3.0-rc.2); MSRV 1.95 → 1.97.1 |
| 0.3.12 | 2026-07-30 | Dynamic `max_db_size` + `--fresh` flag fix DB bloat; read-only 4 TiB cap fixes crashes on >16 GiB databases; LSP hover batched UNWIND updates and the rest of the P-series fixes |
| 0.3.11 | 2026-07-26 | L6+L7 memory work: pipeline streaming + iterator APIs + buffer_pool cap, cutting peak memory on a 70 GB host from 60 GB to ~4 GB |
| 0.3.10 | 2026-07-25 | L1–L5 five-layer defense against OOM on large-repo indexing: memory budget, graph view iterators, streaming CSV, mpsc concurrency cap, adaptive degradation |

---

## 📄 License

This project is licensed under the [MIT](LICENSE) license.

---

## 🙏 Acknowledgments

### 🌟 Core dependencies

CodeNexus stands on the shoulders of these excellent open-source projects:

| Dependency | Purpose |
| ---------- | ------- |
| [lbug](https://github.com/ladybugdb/ladybugdb) (LadybugDB) | Graph database storage |
| [tree-sitter](https://tree-sitter.github.io/) + 21 grammar crates | Multi-language AST parsing |
| [rayon](https://github.com/rayon-rs/rayon) | Data parallelism |
| [notify](https://github.com/notify-rs/notify) / notify-debouncer-full | File watching and debouncing |
| [sdforge](https://crates.io/crates/sdforge) | CLI + MCP dual-transport framework (`#[forge]` macro) |
| [trait-kit](https://crates.io/crates/trait-kit) | Capability registry |
| [oxcache](https://crates.io/crates/oxcache) | Query result cache |
| [inklog](https://crates.io/crates/inklog) | Log backend (console + rotation + LZ4 compression) |
| [ort](https://github.com/pykeio/ort) / tokenizers | Local ONNX embedding inference |
| [ICU4X](https://github.com/unicode-org/icu4x) (icu_normalizer / icu_casemap) | Unicode normalization and case mapping |
| [petgraph](https://github.com/petgraph/petgraph) | Community-detection graph algorithms |
| [criterion](https://github.com/bheisler/criterion.rs) | Benchmarking |

### 💝 Special thanks

Thanks to the Rust community and all [contributors](https://github.com/Kirky-X/codenexus/graphs/contributors).

---

## 📞 Contact & Support

<table style="width:100%; max-width: 600px">
<tr>
<td align="center" width="33%">
<a href="https://github.com/Kirky-X/codenexus/issues"><b style="color:#991B1B">Issues</b></a><br>
<span style="color:#64748B">Report problems and bugs</span>
</td>
<td align="center" width="33%">
<a href="docs/FAQ.md"><b style="color:#1E40AF">Docs / FAQ</b></a><br>
<span style="color:#64748B">Please check here before asking</span>
</td>
<td align="center" width="33%">
<a href="https://github.com/Kirky-X/codenexus"><b style="color:#1E293B">GitHub</b></a><br>
<span style="color:#64748B">Browse the source</span>
</td>
</tr>
</table>

---

## ⭐ Star History

[![Star History Chart](https://api.star-history.com/svg?repos=Kirky-X/codenexus&type=Date)](https://star-history.com/#Kirky-X/codenexus&Date)

If this project helps you, please consider giving it a ⭐️!

**Built by Kirky.X**

---

<sub>© 2026 Kirky.X. All rights reserved.</sub>
