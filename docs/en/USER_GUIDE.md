# 📖 CodeNexus User Guide

**CodeNexus** indexes source repositories into a queryable knowledge graph: tree-sitter multi-language parsing, LadybugDB graph storage, with Cypher queries, call-chain tracing, impact analysis, semantic search, and multi-agent MCP integration. This guide walks from installation through the usage of each high-frequency subcommand (plus the `mcp` serve mode), deep dives into complexity analysis and dead-code detection, and troubleshooting.

## 📋 Table of Contents

- [Introduction](#-introduction)
- [Installation & prerequisites](#-installation--prerequisites)
- [Five-minute start](#-five-minute-start)
- [Core conventions](#-core-conventions)
- [Command reference](#-command-reference)
  - [Indexing & project management](#-indexing--project-management)
  - [Query & search](#-query--search)
  - [Tracing & impact analysis](#-tracing--impact-analysis)
  - [Code evolution management](#-code-evolution-management)
- [Complexity analysis](#-complexity-analysis)
- [Dead-code detection](#-dead-code-detection)
- [API review toolkit](#-api-review-toolkit)
- [Architecture diagrams & semantic Delta](#-architecture-diagrams--semantic-delta)
- [Embedded Web UI (graph-viewer)](#️-embedded-web-ui-graph-viewer)
- [Multi-agent integration](#-multi-agent-integration)
- [Environment variables](#-environment-variables)
- [Configuration file](#️-configuration-file)
- [Troubleshooting](#-troubleshooting)
- [Further reading](#-further-reading)
- [Build presets and feature flags](#-build-presets-and-feature-flags)
- [💻 Examples](#-examples)

---

## 🎯 Introduction

This guide will walk you through:

| Topic | Notes |
|:------|:------|
| **Quick start** | Install and index your first repo in 5 minutes |
| **High-frequency commands** | Flags and examples for common subcommands (the full set of 39 subcommands is authoritative via `codenexus <cmd> --help`) |
| **Deep analysis** | Metrics and thresholds for complexity analysis and dead-code detection |
| **Agent integration** | MCP serve mode, `setup` auto-onboarding, and `hook` |

> 💡 **Tip**: CodeNexus is a CLI tool that requires no coding to use; Rust developers can also embed it in their own programs as the library crate (`codenexus`) — see the [📘 API Reference](API_REFERENCE.md).

---

## 📦 Installation & prerequisites

```bash
# Install from crates.io (default full preset, all 22 languages + all features)
cargo install codenexus

# Or build from source
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo install --path .
```

| Prerequisite | Notes |
|:-------------|:------|
| Rust toolchain | MSRV 1.97.1 (`Cargo.toml` `rust-version`; only needed for source builds) |
| `libstdc++` | On GCC ≤ 12 systems use `LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus` (also requires `cmake`) |
| Disk space | The database defaults to `<project>/.codenexus/<project>.lbug` and grows with repo size |
| git | Required by `detect_changes`, `diagram --repo_root` source evidence, and similar commands |

Build presets: `minimal` (Rust only) / `core` (C + Rust + Python) / `full` (default, 22 languages + all features). The full feature list is in [User Guide · Build presets and feature flags](USER_GUIDE.md#-build-presets-and-feature-flags).

---

## 🚀 Five-minute start

```bash
# 1. Index a repository (--name decides the database name and project name)
codenexus index --path /path/to/project --name myproject

# 2. Check index status
codenexus status
codenexus list

# 3. Cypher query
codenexus query --cypher "MATCH (f:Function) RETURN f.name LIMIT 10"

# 4. Trace call chains (trace main's call relationships, depth 5)
codenexus trace --symbol main --trace_type calls --depth 5 --path_filter "" --detect_cycles false --cross_service false

# 5. Impact analysis (who is affected by changing a function)
codenexus impact --symbol parse_function --depth 3 --edge_types "" --max_depth 0 --include_tests false

# 6. Search symbols
codenexus search --text "parse" --limit 20 --mode exact --fulltext false --project ""
codenexus search --text "get.*user" --mode regex --fulltext false --project ""
codenexus search --text "authentication logic" --fulltext true --project ""

# 7. View a symbol's 360° context
codenexus context --symbol main --depth 1 --project "" --enhanced false
codenexus context --symbol main --project myproject --enhanced true

# 8. Cap the context token budget (omitting --budget disables truncation; 0 is invalid)
codenexus context --symbol main --budget 128

# 9. Start the daemon with impact alerts enabled (off by default)
codenexus daemon --path . --name myproject --notify-impact
```

Every command outputs a single JSON object/array to stdout (except the streaming/long-running `daemon`, `hook`, and `mcp` commands).

---

## 🧭 Core conventions

The following conventions apply to every subcommand (verified against v0.4.0-rc.2, matching [`skill/SKILL.md`](../../skill/SKILL.md)):

- **No positional arguments**. Everything is a named flag: `codenexus query "MATCH ..."` fails; you must write `codenexus query --cypher "MATCH ..."`.
- **Booleans take explicit values**: `--force true`, `--fresh true` (`index` only), `--apply true`, `--cross_service false` (`trace` only), `--embed false`, `--ram_first false`, `--enhanced false`, `--include_tests false` (`impact`). `daemon --notify-impact` is the exception: it can be written bare (equivalent to `true`) or take an explicit `true`/`false`.
- **`--project` accepts a name or an id**: every command with `--project <VALUE>` resolves through `resolve_project_id` (`src/service/project.rs`): if it matches an existing project's `name`, its canonical `id` is used; otherwise it is treated as a raw project id. Only where explicitly documented may `--project ""` be passed to disable filtering.
- **Optional flags carry built-in defaults**: the "optional" flags of high-frequency commands have built-in defaults, so no placeholder flags are needed — `trace`: `--depth 5`, `--path_filter ""`, `--detect_cycles false`, `--cross_service false`; `search`: `--limit 50`, `--mode ""`, `--fulltext false`, `--project ""`; `impact`: `--depth 3`, `--edge_types ""`, `--max_depth 0`, `--include_tests false`; `context`: `--depth 1`, `--project ""`, `--enhanced false`, and omitting `--budget` disables truncation; `index`'s `--force`/`--lsp`/`--embed`/`--ram_first` default to `false`; `detect_changes --mode` defaults to `unstaged`; the rest (complexity thresholds etc.) are in each command's `--help`. Defaults can be overridden per project via `.codenexus/config.json` (see [Configuration file](#️-configuration-file)).
- **Global options**: `--db <DB_PATH>`, `--debounce-ms <MS>` (default `2000`, daemon-related only) and `--format text|json` (default `text`) apply to every command. With `--format json`, all CLI boundary errors (no subcommand / unknown command / missing DB / locked database / `--fresh`·`--rebuild` failures) are written to stdout as UnifiedError JSON, and exit codes match the payload classification (see the exit-code contract below); `text` preserves the historical stderr behavior verbatim.
- **Default database path** (when `--db` is omitted): `.codenexus/<project>.lbug`, where `<project>` comes from the `--name` of `index`/`daemon` (preferred) or the `--path` directory name, falling back to `codenexus`. Since v0.3.7, when no `--name`/`--path` is available and `.codenexus/` contains exactly one `.lbug` file, that file is selected automatically; with multiple files it falls back to `codenexus` and requires an explicit `--db`. The environment variable `CODENEXUS_DB_PATH` can pin the default DB path directly (precedence in "Environment variables"), and the config file's `general.db` provides a fallback for non-standard locations.
- **stderr noise**: all logs (including info level and per-phase index progress lines `[codenexus] [i/n] <phase> ok`) go to stderr, and stdout carries only the command's own JSON, so redirection needs no filtering; log files live in `.codenexus/logs/codenexus.log`. Benign stderr warnings include `inklog ... Failed to set log crate logger`. `query` and `status` additionally emit function-level structured records via sdforge's `#[forge::log]` (into stderr and the log file through the inklog pipeline): success as `fn_exit fn=<command> ok=true duration_ms=…` (info level), failure as `fn_exit … ok=false error=…` (error level); the entry record `fn_enter` (with `args` masked by DataMasker) is debug level and visible only with `RUST_LOG=debug`. One-shot commands drain the log queue before exiting (`logging::flush`), so an error-level `fn_exit` of a failed command still lands in the log file; long-running modes (`mcp`/`webui`) do not drain.
- **Exit-code contract** (`src/service/error.rs`): `0` success; `1` internal error / I/O / JSON / Kit error; `2` invalid input / project not found / query, trace, storage, resolution errors; `4` not found / database corrupt; `--fresh` deletion failures have dedicated exit codes 5/6. Under `--format json`, boundary exit codes match the payload classification (`BAD_REQUEST` → 2, `NOT_FOUND` → 4); `--fresh`/`--rebuild` failures keep 5/6 in both modes (documented contract).

---

## 🛠️ Command reference

### 📥 Indexing & project management

| Command | Key flags | Notes |
|---------|-----------|-------|
| `index` | `--path` `--name` `--force` `--fresh` `--ram_first` | Index a repo into the knowledge graph. `--name` may be omitted (defaults to the `--path` directory name; the project name and the default DB file name match); incremental mode parses only changed files via SHA-256 hash comparison; `--force true` rebuilds everything; `--fresh true` deletes the old DB file before indexing to reclaim dead space; `--ram_first true` enables LZ4 in-memory compression with bulk load |
| `daemon` | `--path` `--name` `--notify-impact` | File-watching daemon that incrementally re-indexes on detected changes; SIGTERM/SIGINT graceful shutdown; the global `--debounce-ms` option controls debouncing (default 2000ms); `--notify-impact` enables alerts for affected symbols after incremental indexing (off by default) |
| `status` | — | Show current index status |
| `list` | — | List all indexed projects |
| `clean` | `--project` | Delete a project and its index |
| `export` | `--output` `--project` | Export a LadybugDB dump as a zstd-compressed artifact (`.cnxp`); the database is chosen by the global `--db` |
| `import` | `--input` `--reindex` `--path` `--name` | Import an artifact; with `--reindex true`, combine with `--path`/`--name` to incrementally fill local gaps |

### 🔎 Query & search

| Command | Key flags | Notes |
|---------|-----------|-------|
| `query` | `--cypher` | Runs a Cypher-subset query against the graph. The subset **supports** `UNION` / `UNION ALL` (measured to return merged rows, exit 0); it does not support label-disjunction expressions `WHERE (n:A OR n:B)` (labels must go in the `MATCH (n:A)` pattern, otherwise `unsupported Cypher construct`, exit 2) or write operations (`CREATE`/`DELETE`/`SET`/`MERGE`/`REMOVE`). Reserved-word labels/relationship types (e.g. `Macro`, `UNION`) must be backtick-escaped: `MATCH (n:\`Macro\`)`; bare reserved words are rejected by the validator with a targeted fix example |
| `search` | `--text` `--mode` `--limit` `--fulltext` `--project` | 5 structured modes `exact` / `regex` / `fuzzy` / `graph` / `multi`; `--fulltext true` uses BM25 full-text retrieval (`matchReason` of `bm25 fts` / `bm25f weighted`). The CLI `search` does not filter by node label; filtering by type (label) exists only in the library API `QueryFacade::search_by_type(label: NodeLabel, …)` (`src/query/facade.rs`) |
| `context` | `--symbol` `--depth` `--project` `--enhanced` `--budget` | 360° symbol view: inbound calls/imports, outbound calls, owning flows; `--enhanced true` adds a multi-dimensional SymbolContext (LSP-enriched info etc.); `--budget <N>` caps output tokens (omit to disable truncation, `0` is invalid), with over-limit output truncated by `service::budget` and a `budget` receipt attached |
| `source` | `--qualified_name` `--id` `--file` `--name` `--start_line` `--end_line` `--max_lines` | Reads a symbol's full source by identifier (functions/methods/classes/structs/traits — any symbol with line-level spans). Three mutually exclusive keys: `--qualified_name` (stable identifier, recommended to copy from `search`/`context` output) / `--id` (node UUID, invalidated by re-indexing) / the (`--file` + `--name`) combination. Optional line window `--start_line`/`--end_line` (absolute lines, for paging huge symbols) and `--max_lines` (cap, default 500, exceeding it errors with a pagination hint). If the source file changed after indexing, a hard error refuses and hints `codenexus index` to re-index; the graph stores only locations — source is read back from disk at query time, never stored |

### 🕸️ Tracing & impact analysis

| Command | Key flags | Notes |
|---------|-----------|-------|
| `trace` | `--symbol` `--trace_type` `--depth` `--path_filter` `--detect_cycles` `--cross_service` | `--trace_type` takes `calls` / `dataflow` / `all` (call chains / data flow / both); `--path_filter` supports globs (e.g. `/src/api/**`); `--detect_cycles true` detects cycles; `--cross_service true` follows `HTTP_CALLS` and reverse `HANDLES_ROUTE` edges for cross-service chains (persist links first with `cross_service`); plain `calls` tracing does not treat cross-service links as call edges |
| `impact` | `--symbol` `--depth` `--edge_types` `--max_depth` `--include_tests` | Blast-radius analysis (upstream caller subgraph) + `risk_assessment` risk grading; `--edge_types "CALLS,IMPLEMENTS,USES_TYPE"` filters by edge type; subgraph node cap 5000 (`MAX_NODES_LIMIT`) |
| `detect_changes` | `--path` `--mode` | git diff → affected symbols + `risk_level`; assess change impact before committing |
| `rename` | `--from` `--to` `--path` `--apply` | High-confidence graph edit + text-search edit; `--apply false` is a dry run (reviewing a dry run first is the recommended default) |

### 🧪 Code evolution management

| Command | Key flags | Notes |
|---------|-----------|-------|
| `dead_code` | `--project` `--entry` `--check_exported` `--check_ffi` `--edge_types` `--check_dynamic_dispatch` | Dead-code detection, detailed in [Dead-code detection](#-dead-code-detection) |
| `architecture` | `--project` | Architecture overview: module boundaries + dependency directions + layering + cross-service dependencies (`ArchitectureOverview` JSON) |
| `complexity` | `--project` + threshold flags | AST complexity analysis, detailed in [Complexity analysis](#-complexity-analysis) |
| `community` | `--project` `--resolution` | Community detection (Leiden modularity optimization); `--resolution` defaults to 1.0 when omitted (`DEFAULT_RESOLUTION`) |
| `cross_service` | `--project` `--protocol` | Cross-service call-chain detection: HTTP REST / gRPC / GraphQL / message queues / event buses; `--protocol` detects all protocols when omitted. Detected HTTP links are persisted as `HTTP_CALLS` edges (caller → route) so `trace --cross_service true` can continue to the peer handler; therefore this command opens the database read-write. Function body content comes from the indexed source on disk (the index does not persist `content`) |

---

## 📊 Complexity analysis

The `complexity` subcommand computes AST complexity metrics for all functions in a project, outputting JSON (with a `complexity` array and `summary` statistics).

### Metrics

| Metric | Field | Notes |
| ------ | ----- | ----- |
| Cyclomatic complexity | `cyclomatic` | McCabe 1976, including branch nodes + explicit exits (return/break/continue) + logical operators |
| Cognitive complexity | `cognitive` | SonarQube-style complexity weighted by nesting level |
| Nesting depth | `nesting_depth` | Maximum nesting levels of branch nodes |
| Function length | `function_length` | Start-end line difference + 1 |
| Halstead complexity | `halstead` | Halstead 1977: `n1/n2/N1/N2/volume/difficulty/effort/delivered_bugs` |
| Maintainability index | `maintainability_index` | Microsoft 2007 revised formula, 0-100 (higher is better) |
| Time complexity | `time_complexity` | AST pattern estimation: O(1)/O(log n)/O(n)/O(n log n)/O(n^2)/O(n^3)/O(2^n) |
| Space complexity | `space_complexity` | Allocation-pattern recognition: O(1)/O(n)/O(n^2) |

Each metric is tiered Green / Yellow / Red / Critical by thresholds, and `overall_severity` takes the highest tier.

### Threshold CLI flags

| Flag | Notes |
| ---- | ----- |
| `--cyclomatic_green <N>` / `--cyclomatic_yellow <N>` / `--cyclomatic_red <N>` | Cyclomatic complexity thresholds |
| `--cognitive_green <N>` / `--cognitive_yellow <N>` / `--cognitive_red <N>` | Cognitive complexity thresholds |
| `--nesting_green <N>` / `--nesting_yellow <N>` / `--nesting_red <N>` | Nesting depth thresholds |
| `--func_length_green <N>` / `--func_length_yellow <N>` / `--func_length_red <N>` | Function length thresholds |
| `--halstead_volume_green <N>` / `--halstead_volume_yellow <N>` / `--halstead_volume_red <N>` | Halstead volume thresholds |
| `--maintainability_green <N>` / `--maintainability_yellow <N>` / `--maintainability_red <N>` | Maintainability index thresholds (higher is better) |
| `--time_complexity_green <O(...)>` / `--time_complexity_yellow <O(...)>` / `--time_complexity_red <O(...)>` | Time complexity thresholds |
| `--space_complexity_yellow <O(...)>` / `--space_complexity_red <O(...)>` | Space complexity thresholds (3 tiers, no Critical) |

`<O(...)>` values: time `O(1)` / `O(log n)` / `O(n)` / `O(n log n)` / `O(n^2)` / `O(n^3)` / `O(2^n)`; space `O(1)` / `O(n)` / `O(n^2)`. All thresholds and flag-style options (including `--red_only`/`--sort_by_severity`) may be omitted; when omitted, `u32` thresholds and `O(...)` string thresholds use the defaults (table below), and `bool` flags default to `false`.

### Default thresholds

| Metric | Green | Yellow | Red |
| ------ | ----- | ------ | --- |
| cyclomatic | 10 | 20 | 25 |
| cognitive | 10 | 15 | 20 |
| nesting | 3 | 5 | 6 |
| func_length | 30 | 100 | 200 |
| halstead_volume | 100 | 1000 | 8000 |
| maintainability | 85 | 65 | 25 |
| time_complexity | O(log n) | O(n) | O(n^2) |
| space_complexity | — | O(1) | O(n) |

> The `maintainability` threshold semantics are inverted: higher MI is better, `value >= green → Green`, `value >= yellow → Yellow`, `value >= red → Red`, otherwise `Critical`. `space_complexity` has only 3 tiers (Green/Yellow/Red), no Critical.

### Examples

```bash
# Default threshold analysis
codenexus complexity --project myproject

# Custom cyclomatic thresholds (green=5, yellow=10, red=15)
codenexus complexity --project myproject --cyclomatic_green 5 --cyclomatic_yellow 10 --cyclomatic_red 15

# Show only Red and Critical functions, sorted by severity
codenexus complexity --project myproject --red_only true --sort_by_severity true

# Custom time complexity thresholds (green=O(1), yellow=O(n log n), red=O(n^2))
codenexus complexity --project myproject --time_complexity_green "O(1)" --time_complexity_yellow "O(n log n)" --time_complexity_red "O(n^2)"
```

---

## 💀 Dead-code detection

The `dead_code` subcommand identifies dead code with a worklist reachability propagation algorithm: starting from a seed set (entry functions / exported functions / FFI entries / test functions / trait impl methods / `pub use` re-export targets / attribute-marked entries), it propagates liveness via BFS to a fixed point; Function/Method nodes not covered by the propagation are deemed dead. Each dead-code record carries a High/Medium/Low confidence tier, and the output includes `indexed_commit` / `current_head` / `is_stale` to indicate index freshness.

### Configuration

Detection behavior is controlled via `DeadCodeConfig` (CLI flags pass through at the service layer); key fields:

| Field | Default | Notes |
| ----- | ------- | ----- |
| `entry_patterns` | `["main", "Main", "__main__", "wmain", "WinMain", "DLLMain"]` | Entry-function name glob patterns; matching Functions are live seeds |
| `test_patterns` | 8 items: `test_*` / `*_test` / `*_spec` / `it_*` / `sec_*` / `snap_*` / `perf_*` / `bench_*` | Test-function name glob patterns; matching Functions are live seeds |
| `attribute_entries` | 6 items: `#[tool` / `#[forge` / `#[tokio::main` / `#[rocket::main` / `#[actix::main` / `#[axum::main` | Substring match against the function's `signature` field; a hit is treated as a macro-expanded synthetic entry (tree-sitter does not expand macros, so CALLS edges are invisible). `#[test]` / `#[bench]` are covered by `test_patterns` name globs and therefore not in `attribute_entries` |
| `check_dynamic_dispatch` | `true` | Detects the `#<TypeName>` disambiguator in qualified_name (e.g. `fmt#Display`); a hit is treated as a trait impl method (vtable dynamic dispatch, no static CALLS edges) and skips the dead-code verdict. When set to `false`, conservative mode judges them dead |
| `check_exported` | `true` | Functions/Methods with `isExported=true` are externally visible and skip the dead-code verdict |
| `check_ffi` | `true` | Functions whose signature contains `extern "C"` / `#[no_mangle]` are FFI entries and skip the dead-code verdict |
| `edge_types` | `CALLS` / `FfiCalls` / `Implements` / `HandlesRoute` / `Usage` / `Tests` / `UsesType` / `HttpCalls` / `AsyncCalls` (9 kinds) | Edge-type whitelist participating in reachability propagation |

### Examples

```bash
# Detection with default config (--entry/--check_exported/--check_ffi/--edge_types are required flags with no defaults; missing any one exits 2)
codenexus dead_code --project myproject --entry "" --check_exported true --check_ffi true --edge_types ""

# Custom edge_types (CALLS + IMPLEMENTS only)
codenexus dead_code --project myproject --entry "" --check_exported true --check_ffi true --edge_types "CALLS,IMPLEMENTS"

# Conservative mode: disable trait impl recognition
codenexus dead_code --project myproject --entry "" --check_exported true --check_ffi true --edge_types "" --check_dynamic_dispatch false
```

> **Limitations**: tree-sitter does not expand procedural macros (`#[derive(Serialize)]` etc.), so derive-generated impl methods are judged dead (known false positive). Calls to const fns in const context are also not recognized as CALLS edges by tree-sitter. Bash's `trap cleanup EXIT` (signal-triggered) and indirect calls via string concatenation / variable expansion are invisible to tree-sitter and are reported at Medium confidence — confirm manually before deletion.

---

## 🧭 API review toolkit

The `api-review` feature provides 4 commands (axum projects support programmatic route extraction via `Router::new().route("/x", get(h))`):

| Command | Notes |
|---------|-------|
| `route_map` | HTTP route mapping, outputs the API endpoint inventory (Route nodes + HANDLES_ROUTE edges) |
| `shape_check` | API shape check, validating request/response structure consistency |
| `api_impact` | API change impact analysis; analyzes all endpoints when `--endpoint` is omitted |
| `tool_map` | Tool mapping, outputs the MCP tool inventory |

> Known limitation: `extract_axum_routes` only recognizes bare-identifier handlers (`get(handler)`); path-qualified (`get(crate::module::handler)`) and closure (`get(|| { ... })`) handlers are skipped and need manual review. For axum projects whose routes are registered purely by macros (route strings as macro arguments rather than standalone string literals), `cross_service` may return an empty `[]`.

---

## 🖼️ Architecture diagrams & semantic Delta

The `diagram` feature provides two commands (output fields per command: `codenexus <cmd> --help` and the examples below):

```bash
# Architecture diagram export: self-contained interactive HTML + BLAKE3 delivery receipt
codenexus diagram --project myproject --output ./arch.html

# showcase quality tier: any alert escalates to an error and blocks delivery
codenexus diagram --project myproject --output ./arch.html --quality showcase

# Attach Git source-evidence badges (validates origin URL consistency)
codenexus diagram --project myproject --output ./arch.html --repo_root /path/to/project --repo_url https://github.com/you/yourrepo

# Architecture semantic Delta: compare two indexed projects
codenexus arch_diff --base_project v1 --head_project v2 --output ./delta.html
```

`diagram` flags: `--quality` (`standard` default / `showcase`), `--repo_root`, `--repo_url`, `--title`, `--locale` (`en` / `zh-CN`). Component types (interface / service / storage / model) come from real layering facts, not name guessing.

`arch_diff` outputs Before / Delta / After HTML sections plus a machine receipt JSON (`<path>.receipt.json`; the two files are written atomically as a pair); each change carries `kind` (`added`/`removed`/`changed`), `changed_fields` (JSON Pointer), and `classification` (`topology` / `semantic`). For symbol-level line changes use `detect_changes`.

---

## 🖥️ Embedded Web UI (graph-viewer)

The `webui` feature compiles graph-viewer's backend and frontend assets into the `codenexus` binary; one command starts it, no manual frontend needed:

```bash
# 1) (Optional but recommended) build the frontend first so it is embedded at compile time
cd graph-viewer && npm ci && npm run build && cd ..

# 2) Build the binary with webui enabled (or cargo install --path . --features webui)
cargo build --release --features webui

# 3) Start (equivalent form: cn webui)
codenexus webui            # default port 9800
codenexus webui --port 9000
```

After startup, stdout prints a line `graph-server auth token: <hex>`; opening `http://127.0.0.1:9800/` in a browser shows graph-viewer (drag in or pick a `.lbug` file, or load a snapshot via `?snapshot=`). The snapshot integration of `diagram` / `arch_diff` can point directly at it:

```bash
codenexus diagram --project myproject --output ./arch.html --viewer_url http://127.0.0.1:9800
```

Conventions and security:

- Binds `127.0.0.1` only; `/api/*` requires the `x-graph-token` header (token in stdout), and the Host is validated to allow loopback names only; data resolves only from `.codenexus/` under the startup directory and the current-directory whitelist.
- Without `graph-viewer/dist` built, the binary still compiles and the page shows a fallback guide page (hinting `npm run build`).
- `webui` needs no Kit/database: it does not create an empty DB and does not contend for locks with a writing `index`/`daemon` process.

---

## 🤖 Multi-agent integration

```bash
# Auto-detect Claude Code / Cursor / Codex and write MCP configs
codenexus setup

# Force-overwrite existing configs (skips the confirmation prompt)
codenexus setup --force

# Emit PreToolUse/PostToolUse JSON (exit 0, never blocks; meant as an agent hook)
codenexus hook

# Start the stdio MCP server (11 tools: query/trace/impact/search/source/context/architecture/diagram/arch_diff/dead_code/detect_changes)
codenexus mcp [--db <DB_PATH>]
```

`setup` probes the `~/.claude/`, `~/.cursor/`, and `~/.codex/` directories to detect installed agents. MCP tools share the same `#[forge]` definitions as the CLI commands (`src/service/`), and each tool description documents its parameter semantics and defaults; the server opens the database read-only and can coexist with writer processes.

LSP-enriched commands (`lsp` feature):

| Command | Notes |
|---------|-------|
| `lsp_goto_def` | LSP go-to-definition (rust-analyzer and other server integrations) |
| `lsp_hover` | LSP hover info (type signatures and other semantic enrichment) |

> In environments where the language server is unavailable (the server process exits immediately on startup), `lsp_goto_def`/`lsp_hover` may fail with exit 2 (`LSP communication error: server connection closed`) — an environment/configuration problem, not a tool logic defect. LSP starts on demand: pure-Rust repos only start rust-analyzer.

---

## 🗂️ Configuration file

CodeNexus supports a per-project config file `.codenexus/config.json` (same directory as the index database). Precedence for all settings: **explicit CLI flag > config file > built-in default**; a missing file is fully invisible, and a malformed file only logs `[warn]` and falls back to built-in defaults without interrupting the command.

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

| Field | Notes |
|-------|-------|
| `general.db` | Fallback database path used when `--db` is omitted, cannot be derived from `--name`/`--path`, and no unique `.lbug` file is found (typical use: the database lives at a non-standard location, sparing every read command a `--db`) |
| `general.debounce_ms` | Fallback for `--debounce-ms` (daemon debounce milliseconds) |
| `general.verbose` | Fallback for `--verbose` (daemon debug diagnostics for each batch of file events) |
| `command_defaults.<command>.<param>` | Default value for the subcommand's flag (the parameter must be registered on that command; values are string/number/boolean); explicitly passed flags always win |

> 💡 Add `.codenexus/` to the project `.gitignore`: the index database (`.lbug`), logs (`logs/codenexus.log`), and this config file all live there — the first two should not be committed.

---

## ⚙️ Environment variables

CodeNexus is a CLI tool that does not read `.env` files itself; a template is at [`.env.example`](../../.env.example) for consumption by shell / systemd / supervisord / docker / direnv:

| Variable | Default | Notes |
|----------|---------|-------|
| `RUST_LOG` | `info` | Log level; supports single levels and per-module filtering (e.g. `codenexus=debug,sqlx=warn`) |
| `CODENEXUS_LOG_TARGETS` | unset | Per-module level presets, `target=level` comma-separated (e.g. `codenexus::storage::connection=error,lbug=warn`); precedence below `RUST_LOG`, above the global level; invalid entries are skipped with a stderr notice |
| `CODENEXUS_LOG_SAMPLE` | unset | Log sampling: `level=N` (keep 1 of N for that level, e.g. `debug=10`) or `prefix=N[@level]` (target-prefix sampling, where `@level` always admits that level and above, e.g. `sqlx=100@warn`); decides which records survive rate limiting; invalid entries are skipped with a stderr notice |
| `CODENEXUS_LOG_RATE_LIMIT` | unset | Per-target-prefix log rate-limit quotas (tokens/second), `prefix=rate` comma-separated, longest prefix wins (e.g. `lbug=50,codenexus::daemon=200`); targets not matching any rule are unlimited |
| `CODENEXUS_EMBED_MODEL_PATH` | unset | Local embedding model directory (default `assets/arctic-embed-xs/`, 90 MB, not committed; download it yourself) |
| `CODENEXUS_EMBED_MODEL_SHA256` | unset | Expected SHA-256 of `model.safetensors` (64 hex chars); when set, weight integrity is verified at load and a mismatch refuses to load; invalid values are ignored with a stderr notice (verification stays off). Compute via `sha256sum <model dir>/model.safetensors` |
| `CODENEXUS_DB_PATH` | unset | Optional default database path, effective when `--db` is not passed (including the `mcp` serve mode; precedence above the config file's `general.db` and the `.codenexus/<project>.lbug` derived from `--name`/`--path`; empty values ignored) |
| `LBUG_BUILD_FROM_SOURCE` | unset | Set to `1` to compile LadybugDB from source (needed on old-`libstdc++` systems) |

---

## 🔧 Troubleshooting

| Symptom | Cause and handling |
|---------|--------------------|
| Linking reports `undefined symbol: std::to_chars(..., _Float128, ...)` | The prebuilt LadybugDB binary requires a recent `libstdc++` (GCC ≤ 12 systems). Run `LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus` (requires `cmake`) |
| mold linker reports `undefined symbol: __cpu_model` | Fixed in v0.3.7: `build.rs` statically links `libgcc.a` automatically; upgrade if older |
| `query`/`list` fail without `--db` | Since v0.3.8 `.codenexus/` is scanned automatically: with exactly one `.lbug` it is selected; with several, pass `--db` explicitly |
| Many dead-code false positives on `#[test]` functions | Since v0.3.8, `TEST_ATTRIBUTE_MARKERS` recognizes `#[test]`/`#[tokio::test]`/`#[rstest]`-marked functions as entries; if it still occurs, confirm the index is not stale (check `is_stale`) |
| `route_map`/`shape_check`/`cross_service`/`tool_map` return empty on axum projects | Programmatic `Router::new().route(...)` routes supported since v0.3.8; macro-registered routes remain invisible (see the limitation above) |
| `lsp_goto_def`/`lsp_hover` exit 2 | The language server process exits immediately on startup — an environment/configuration problem; confirm the LSP server is installed and starts manually in a terminal |
| Symbol ambiguity error `ambiguous symbol 'xxx': N candidates` | Ambiguity policy: error explicitly (exit 2) instead of guessing when it cannot be uniquely determined; use a more precise symbol name or `--project` to narrow |
| DQ-002 Duplicate FQN / DQ-004 Orphan edge warnings | Data-quality receipts (stable rule code + evidence + fix wording); both defect classes were fixed in v0.3.9 — for new warnings follow the receipt's guidance |
| `skipping unsupported DDL statement` on stderr | Benign log; filter with `2>/dev/null` for clean JSON |

---

## 📚 Further reading

| Document | Content |
|----------|---------|
| [📘 API Reference](API_REFERENCE.md) | Library crate public API and the CLI/MCP interface contract |
| [⚡ Performance Guide](PERFORMANCE.md) | Benchmark data, memory optimization, and tuning advice |
| [❓ FAQ](FAQ.md) | Frequently asked questions |
| [🔒 Security doc](SECURITY.md) | Vulnerability reporting process and security best practices |

---

## 🔧 Build presets and feature flags

**Presets**: `default = ["full"]`

<details>
<summary>📋 Table</summary>

| Feature | Default | Description |
| ------- | ------- | ----------- |
| `minimal` | — | Minimal preset: `lang-rust` only |
| `core` | — | Core preset: `lang-c` + `lang-rust` + `lang-python` |
| `full` | Enabled | Full preset: `core` + Fortran/TypeScript/Go/Java/C++/JavaScript/Ruby/Haskell/OCaml/Scala/PHP/C#/Bash/HTML/CSS/JSON/Regex/Verilog/Swift + daemon/analysis/complexity/api-review/community/cross-service/diagram/lsp/cli/mcp/cache/embeddings/i18n/hub |
| `lang-c` | — | C language parser (tree-sitter-c) |
| `lang-rust` | Enabled | Rust language parser (tree-sitter-rust) |
| `lang-fortran` | — | Fortran language parser (tree-sitter-fortran) |
| `lang-python` | — | Python language parser (tree-sitter-python) |
| `lang-typescript` | — | TypeScript language parser (tree-sitter-typescript) |
| `lang-go` | — | Go language parser (tree-sitter-go) |
| `lang-java` | — | Java language parser (tree-sitter-java) |
| `lang-cpp` | — | C++ language parser (tree-sitter-cpp) |
| `lang-javascript` | — | JavaScript language parser (tree-sitter-javascript) |
| `lang-ruby` | — | Ruby language parser (tree-sitter-ruby) |
| `lang-haskell` | — | Haskell language parser (tree-sitter-haskell) |
| `lang-ocaml` | — | OCaml language parser (tree-sitter-ocaml) |
| `lang-scala` | — | Scala language parser (tree-sitter-scala) |
| `lang-php` | — | PHP language parser (tree-sitter-php) |
| `lang-csharp` | — | C# language parser (tree-sitter-c-sharp) |
| `lang-bash` | — | Bash language parser (tree-sitter-bash) |
| `lang-html` | — | HTML language parser (tree-sitter-html) |
| `lang-css` | — | CSS language parser (tree-sitter-css) |
| `lang-json` | — | JSON language parser (tree-sitter-json) |
| `lang-regex` | — | Regex language parser (tree-sitter-regex) |
| `lang-verilog` | — | Verilog language parser (tree-sitter-verilog) |
| `lang-swift` | — | Swift language parser (tree-sitter-swift) |
| `daemon` | Enabled | File-watching daemon (notify + notify-debouncer-full) |
| `embeddings` | Enabled | Vector-embedding semantic search (reqwest HTTP + local Candle inference) |
| `lsp` | Enabled | LSP-enriched parsing (7 LSP clients) |
| `analysis` | Enabled | Dead-code detection + architecture overview (pure Cypher aggregation) |
| `complexity` | Enabled | AST complexity analysis (8 metrics, depends on `analysis`) |
| `api-review` | Enabled | API review toolkit (route_map/shape_check/api_impact/tool_map) |
| `community` | Enabled | Community detection (Leiden modularity optimization, depends on petgraph) |
| `cross-service` | Enabled | Cross-service call-chain detection (HTTP route pattern matching) |
| `diagram` | Enabled | Architecture diagram pipeline: `diagram`/`arch_diff` commands (depends on `analysis`) |
| `mcp` | Enabled | MCP server (sdforge `mcp` stdio transport) |
| `cli` | Enabled | CLI binary (sdforge `cli` transport, required for the binary) |
| `cache` | Enabled | Query result cache (oxcache) |
| `i18n` | Enabled | Unicode case folding + NFC normalization (ICU4X) |
| `webui` | — | Embedded graph-viewer Web UI: launched by `codenexus webui` (frontend `graph-viewer/dist` assets embedded at compile time; opt-in, not part of `full`) |

</details>

> **Logging**: inklog is the only log backend (console + daily-rotating file sink with 30-day retention). inklog is pulled in with `default-features = false`; default (`full`) builds compile the zstd backend through vecboost's `inklog/compression` feature (cargo feature unification), so `compress = true` produces zstd-compressed rotation files. Feature sets without vecboost (e.g. `minimal`) compile no compression backend at all, and compression is skipped (warning, no compressed files). The gzip (flate2) backend is not enabled. The optional tracing-subscriber backend has been removed.

```bash
# Minimal build (Rust only, no daemon/analysis)
cargo build --release --no-default-features --features minimal

# Core build (C + Rust + Python)
cargo build --release --no-default-features --features core

# Single-language lean build (e.g. C only)
cargo build --release --no-default-features --features lang-c

# Full build (default, all languages + all features)
cargo build --release

# Build with vector embeddings
cargo build --release --features embeddings
```

---

## 💻 Examples

All 14 runnable examples live in [`examples/`](../../examples/), each registered as a `cargo run --bin` target in `examples/Cargo.toml`:

<details>
<summary>📋 Table</summary>

| Example | File | Description |
| ------- | ---- | ----------- |
| basic_indexing | `examples/src/bin/basic_indexing.rs` | Index Rust source into the knowledge graph, list functions via Cypher |
| cypher_query | `examples/src/bin/cypher_query.rs` | Run a variety of Cypher queries (by type, by name) |
| symbol_search | `examples/src/bin/symbol_search.rs` | Search symbols by name and type, handle empty results |
| call_tracing | `examples/src/bin/call_tracing.rs` | Trace function call paths forward, build a call graph |
| impact_analysis | `examples/src/bin/impact_analysis.rs` | Analyze the blast radius of changing a symbol (reverse BFS) |
| symbol_context | `examples/src/bin/symbol_context.rs` | 360° symbol view: callers / callees / execution flows, plus subgraph loading and symbol disambiguation |
| export_import | `examples/src/bin/export_import.rs` | Graph database export/import verification |
| project_lifecycle | `examples/src/bin/project_lifecycle.rs` | Project lifecycle: index multiple projects → list → resolve by name → remove |
| code_analysis | `examples/src/bin/code_analysis.rs` | Code-quality trio: complexity / dead code / community detection |
| api_surface | `examples/src/bin/api_surface.rs` | API/Web surface analysis: route table / schema check / API impact / cross-service calls / MCP tool table |
| architecture_diagram | `examples/src/bin/architecture_diagram.rs` | Architecture overview + self-contained interactive HTML diagram + two-project architecture diff |
| daemon_watch | `examples/src/bin/daemon_watch.rs` | File-watching daemon: `notify` debounce → incremental indexing (Observer pattern) → graceful stop |
| git_integration | `examples/src/bin/git_integration.rs` | Git integration: map `git diff` changed lines to affected symbols with risk grading |
| setup_mcp | `examples/src/bin/setup_mcp.rs` | MCP onboarding: auto-detect installed AI coding agents and write MCP server config |

</details>

<details>
<summary>📄 Code · bash</summary>

```bash
# Run a single example
cargo run --manifest-path examples/Cargo.toml --bin basic_indexing

# Run all examples
for bin in basic_indexing cypher_query symbol_search call_tracing impact_analysis symbol_context export_import project_lifecycle code_analysis api_surface architecture_diagram daemon_watch git_integration setup_mcp; do
  cargo run --manifest-path examples/Cargo.toml --bin $bin
done
```

</details>

Examples index source with `IndexFacade`, query with `QueryFacade`, and trace calls with `TraceFacade`; temp directories are cleaned up on exit. The full library API is documented in the [📘 API Reference](API_REFERENCE.md).
