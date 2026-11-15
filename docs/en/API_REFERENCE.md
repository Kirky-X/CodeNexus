# 📘 CodeNexus API Reference

This reference covers the complete public API of the CodeNexus library crate (`codenexus`, entry `src/lib.rs`): core facades, capability traits, data models, error types, and the external CLI/MCP interface contract. The document assumes the `full` preset is enabled; with other feature combinations some modules may be unavailable (per-module feature gating in [Feature gating](#️-feature-gating)).

## 📋 Table of Contents

- [Overview](#-overview)
- [Core Facade API](#-core-facade-api)
  - [IndexFacade (indexing)](#indexfacade-indexing)
  - [QueryFacade (query & search)](#queryfacade-query--search)
  - [TraceFacade (trace & impact analysis)](#tracefacade-trace--impact-analysis)
- [Module reference](#-module-reference)
- [Kit capability registry](#-kit-capability-registry)
- [CLI / MCP external interface](#️-cli--mcp-external-interface)
- [Error types & exit codes](#-error-types--exit-codes)
- [Feature gating](#️-feature-gating)
- [Usage example](#-usage-example)
- [Related docs](#-related-docs)

---

## 🎯 Overview

`codenexus` is a dual-target "library + binary" crate: `src/lib.rs` exposes the public API for the CLI binary and downstream embedders, and `src/main.rs` is the sdforge-driven CLI binary.

| Principle | Notes |
|:----------|:------|
| **Facade layering** | Applications face only the three facades `IndexFacade` / `QueryFacade` / `TraceFacade`; pipeline, parsing, and storage details stay behind the facades |
| **Capability registry** | Cross-module collaboration is assembled via the `trait-kit` Kit registry (`kit` module); features decide which capabilities resolve |
| **Feature gating** | Languages and optional capabilities are independent features; unenabled code does not compile. At least one `lang-*` or `lsp` feature is required, otherwise a compile-time `compile_error!` fires |
| **Fail loud** | Error types funnel into `CodeNexusError` and map to stable process exit codes |

> The full feature list and preset explanation are in the [User Guide · Build presets and feature flags](USER_GUIDE.md#-build-presets-and-feature-flags).

---

## 🧱 Core Facade API

### IndexFacade (indexing)

Defined in `src/index/pipeline.rs`. Owns the full pipeline: file discovery → incremental hashing → parallel parsing → symbol resolution → bulk load.

| Method | Signature essentials | Notes |
|--------|----------------------|-------|
| `new` | `(&self, db_path: &Path) -> Result<Self>` | Opens (or creates) the target LadybugDB database |
| `empty` | `(project_id: impl Into<String>) -> Self` | Builds an empty facade (testing / special purposes) |
| `with_cache` | `(self, cache: Arc<dyn CacheStore>) -> Self` | Attaches a query cache (`cache` feature) |
| `with_budget` | `(self, budget: MemoryBudget) -> Self` | Sets the memory budget (large-repo three-level memory-pressure defense) |
| `index` | `(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult>` | Runs the standard indexing pipeline; `force = true` rebuilds everything |
| `index_incremental` | `(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult>` | Incremental pipeline (parses only changed files; currently delegates to `index`) |
| `index_ram_first` | `(&self, path: &Path, project_name: &str, force: bool) -> Result<IndexResult>` | RAM-first pipeline: reads source into memory, LZ4-compresses, then bulk loads. Recommended for repos with ≥ 1 GB of source (ADR-024); measured to use more memory on small repos; falls back to the default streaming disk-read path automatically when the memory budget is insufficient |

`IndexResult` fields:

| Field | Type | Notes |
|-------|------|-------|
| `project_id` | `String` | Project id assigned by this index run (UUIDv7) |
| `files_indexed` | `usize` | Files actually parsed (changed + new) |
| `files_skipped` | `usize` | Files skipped because hashes matched |
| `nodes_created` | `usize` | Nodes created |
| `edges_created` | `usize` | Edges created |
| `duration_ms` | `u64` | Indexing duration (milliseconds) |

### QueryFacade (query & search)

Defined in `src/query/facade.rs`. Wraps Cypher-subset queries plus structured / full-text search.

| Method | Signature essentials | Notes |
|--------|----------------------|-------|
| `new` | `(db_path: &Path) -> Result<Self>` | Opens a database connection |
| `new_read_only` | `(db_path: &Path) -> Result<Self>` | Read-only connection (concurrent multi-process reads) |
| `with_connection` | `(conn: StorageConnection) -> Self` | Reuses an existing connection |
| `connection` | `(&self) -> &StorageConnection` | Exposes the underlying connection |
| `cypher` | `(&self, query: &str) -> Result<QueryResult>` | Executes a Cypher-subset query; `QueryResult.rows` is a vector of JSON values (`row[0].as_str()` etc. to read columns). The subset supports `UNION` / `UNION ALL`; it does not support label-disjunction expressions `WHERE (n:A OR n:B)` or write operations (`CREATE`/`DELETE`/`SET`/`MERGE`/`REMOVE`); validation in `src/query/cypher_subset.pest`. Reserved-word labels/relationship types (e.g. `Macro`) must be backtick-escaped `(n:\`Macro\`)`; bare reserved words are rejected by the validator with a targeted fix example |
| `search` | `(&self, text: &str, project: Option<&str>, limit: usize) -> Result<Vec<SearchResult>>` | Structured search by name (CONTAINS), sorted by relevance |
| `search_by_type` | `(&self, label: NodeLabel, project: Option<&str>, limit: usize) -> Result<Vec<SearchResult>>` | Lists symbols by node type |
| `search_by_file` | `(&self, file_path: &str, project: Option<&str>) -> Result<Vec<SearchResult>>` | Lists all symbols in a file |
| `fulltext_search` | `(&self, text: &str, project: Option<&str>, limit: usize) -> Result<Vec<SearchResult>>` | BM25 full-text search (uses FTS when the extension is available, falls back to CONTAINS otherwise) |

`SearchResult` fields: `name`, `label`, `file_path: Option<String>`, `start_line: Option<u32>`, `qualified_name: Option<String>`, `score: f64` (relevance 0.0-1.0), `match_reason: String` (e.g. `"exact name match"`, `"bm25 fts"`), `degree: u32` (in-degree used for graph-enhanced scoring; 0 in non-graph search modes).

### TraceEngine (trace & impact analysis engine)

Defined in `src/trace/facade.rs`. A tracing engine built on a `Storage` trait object, wrapping call-chain / data-flow tracing and impact analysis.

| Method | Signature essentials | Notes |
|--------|----------------------|-------|
| `new` | `(storage: &'a dyn Storage) -> Self` | Builds from a Storage trait object (default `TraceConfig`) |
| `with_config` | `(storage: &'a dyn Storage, config: TraceConfig) -> Self` | Builds with a `TraceConfig` |
| `config` / `storage` | Accessors | Read the config and the storage handle |
| `apply_path_filter` | Path filtering | Filters trace paths with glob semantics (library-layer implementation of the CLI `--path_filter`) |

### TraceFacade (graph-view tracing)

Defined in `src/trace/facade.rs`. A graph-view tracing facade that holds an immutable borrow of a `Graph`, for callers that already own a graph structure.

| Method | Signature essentials | Notes |
|--------|----------------------|-------|
| `new` | `(graph: &'a Graph) -> Self` | Builds from a graph view (no Storage connection) |
| `trace` | `(&self, symbol: &str, trace_type: TraceType, depth: usize) -> Result<TraceResult>` | Runs call/data-flow tracing against the graph |
| `trace_by_id` | Trace by node id | Starts from an already-resolved node id |

`TraceConfig` (defined in `src/trace/module.rs`) fields and defaults:

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `db_path` | `PathBuf` | `":memory:"` | Database path; `:memory:` is an in-memory DB (for tests) |
| `max_depth` | `u32` | `5` | Maximum trace depth (cap `MAX_DEPTH_LIMIT = 10`) |
| `edge_types` | `Vec<EdgeType>` | `[Calls]` | Edge types participating in tracing |
| `path_filter` | `Option<PathFilter>` | `None` | Optional path filter |
| `detect_cycles` | `bool` | `false` | Whether to detect cycles |
| `cross_service` | `bool` | `false` | Whether to traverse `HttpCalls` edges for cross-service tracing |
| `read_only` | `bool` | `false` | Open read-only (multi-process shared reads; skips schema initialization) |

`TraceResult` fields: `symbol`, `paths: Vec<TracePath>` (`nodes` / `edges` / `depth`), `cycles: Vec<TraceCycle>`.

---

## 🧩 Module reference

Public modules exported by `src/lib.rs` (feature gating in [Feature gating](#️-feature-gating)):

| Module | Responsibility | Key types |
|--------|----------------|-----------|
| `model` | Graph data model | `Node` / `Edge` / `Graph`, `NodeLabel` (45 kinds), `EdgeType` (32 kinds), language and i18n support |
| `discover` | File discovery | `.gitignore`-respecting directory traversal (`ignore` crate) |
| `parse` | tree-sitter parsing | Per-language extractors that turn ASTs into `Node`/`Edge` records |
| `ir` | Intermediate representation | `ExtractResult` and other pipeline intermediates |
| `resolve` | Symbol resolution | FQN generation, call resolution (calls), dataflow, FFI, imports, type resolution, scopes |
| `index` | Indexing pipeline | `IndexFacade`, `Pipeline` (DAG phases), `MemoryBudget`, incremental hashing |
| `query` | Query engine | `QueryFacade`, Cypher subset, structured search, BM25F full-text (`bm25f.rs`), tokenization |
| `trace` | Tracing engine | `TraceFacade`, BFS tracing, call graph, taint tracing (`TaintPathTracer`), impact analysis (`impact.rs`), 360° context (`context.rs`) |
| `storage` | Graph storage | `Storage` trait, `StorageConnection`, `Repository`, schema management, quality checks (`quality.rs`) |
| `diagnostics` | Diagnostic receipts | Stable rule codes (DQ-002 Duplicate FQN, DQ-004 Orphan edge, etc.) + evidence + fix wording |
| `service` | CLI/MCP service layer | Per-`#[forge]`-command core + CLI wrapper + MCP wrapper; `error::CodeNexusError` (re-exported at the crate root) |
| `kit` | Capability registry | `build_kit` / `KitBootstrapConfig` (next section) |
| `analysis` (`analysis`) | Analysis toolkit | Dead-code detection, architecture overview, complexity, community detection, cross-service detection |
| `diagram` (`diagram`) | Architecture diagram pipeline | Deterministic layout, orthogonal routing, HTML output, quality checks, semantic Delta |
| `embed` (`embeddings`) | Vector embeddings | Semantic-search embeddings (local Candle inference) |
| `lsp` (`lsp`) | LSP enrichment | 7 LSP clients, on-demand startup, batched hover updates |
| `cache` (`cache`) | Query cache | `CacheStore` capability (oxcache) |
| `daemon` (`daemon`) | File-watching daemon | Event loop, debouncing (`DEFAULT_DEBOUNCE_MS = 2000`), graceful shutdown |

The crate root additionally exports:

| Item | Notes |
|------|-------|
| `version() -> &'static str` | Returns the crate version (used by the CLI `--version`) |
| `CodeNexusError` | Root re-export of `service::error::CodeNexusError` (see [Error types](#-error-types--exit-codes)) |
| `sdforge` (`cli`) | Re-export of sdforge so the binary target can use `sdforge::clap` without depending on clap directly |

### Storage trait

Defined in `src/storage/capability.rs`; the unified port of the storage layer (object-safe, `Send + Sync`), with each method matching an existing `StorageConnection` / `Repository` implementation:

| Method | Notes |
|--------|-------|
| `init_schema` | Initializes the full schema (idempotent), returns `SchemaInitReport` |
| `execute` | Executes a Cypher statement that returns no rows (DDL/DML) |
| `query` | Executes a Cypher query, returns `Vec<Vec<serde_json::Value>>` |
| `save_project` / `save_nodes` / `save_edges` | Saves projects / bulk-saves nodes by label (CSV `COPY FROM`, ADR-014) / bulk-saves edges |
| `get_project` / `list_projects` | Project queries |
| `query_functions` | Lists all functions in a project (sorted by qualified name) |
| `get_file_hash` / `get_all_file_hashes` | File-hash access used by incremental indexing |
| `delete_project` / `delete_file_nodes` | Project- and file-level deletes (including orphan-edge cleanup) |

---

## 🧰 Kit capability registry

`src/kit/` (based on trait-kit) assembles capabilities in-process; the CLI's `init_kit` goes through the same path:

| API | Notes |
|-----|-------|
| `KitBootstrapConfig::new(db_path: PathBuf)` | Builds the bootstrap config from a database path |
| `with_debounce_ms(u64)` | daemon debounce milliseconds (default 2000) |
| `with_read_only(bool)` | Read-only mode (trace/query commands) |
| `with_embedding_config(EmbeddingConfig)` | Embedding config (`embeddings` feature) |
| `build_kit(config)` | Builds the Kit instance; `KitError` reports missing capabilities / build failures |

---

## 🖥️ CLI / MCP external interface

CLI and MCP share the same command set defined by the `#[forge]` macro in `src/service/` (core function + CLI wrapper + MCP wrapper). The 38 CLI subcommands (plus the `mcp` serve mode), the 11 MCP tools (query / trace / impact / search / source / context / architecture / diagram / arch_diff / dead_code / detect_changes), and the semantics of every flag are covered in the [📖 User Guide](USER_GUIDE.md).

### service::source (symbol source-reading library API)

Defined in `src/service/source.rs`. Core library interface — external Rust programs can wrap it directly without the CLI/MCP (always compiled, no feature gating):

```rust
use codenexus::service::source::{
    run_read_symbol_source, LineWindow, SourceOptions, SymbolKey,
};

// 按 qualifiedName 读完整函数源码（推荐从 search/context 输出复制标识）
let out = run_read_symbol_source(&kit, SymbolKey::QualifiedName("myproj.src.main.rs.build_cli_command".into()), &SourceOptions::default())?;
println!("{}", out.source); // 完整函数体；out.start_line/end_line 为实际切片行号

// 超大符号分页：绝对行号窗口（span ∩ window）
let opts = SourceOptions {
    window: Some(LineWindow { start: 500, end: None }),
    ..SourceOptions::default()
};

// 其余定位键：SymbolKey::NodeId(id)、SymbolKey::FilePathAndName { file_path, name }
```

- Returns `SymbolSource` (serde camelCase: `nodeId` / `label` / `qualifiedName` / `filePath` / `startLine` / `endLine` / `totalLines` / `source`)
- Error `SourceError` (bridged to `CodeNexusError` via `From` for unified exit codes): `SymbolNotFound` / `AmbiguousSymbol` (with candidates) / `StaleIndex` (FileMissing / HashMismatch / SpanBeyondFile, hints `codenexus index`) / `WindowOutsideSpan` / `SourceTooLarge` (hints windowed pagination) / `SourceUnavailable`
- **Security contract**: the graph is the single source of truth for paths — read paths come only from DB node rows, and slicing goes through canonicalize + prefix validation to prevent traversal; the `filePath` argument is only a query key

Key contracts:

- **Flag style**: no positional arguments; snake_case long options; booleans take explicit values; optional flags carry built-in defaults (e.g. `trace --depth 5`, `search --limit 50`) and can be pinned per project via `.codenexus/config.json` (precedence: CLI flag > config file > built-in default).
- **Global options**: `--db <DB_PATH>`, `--debounce-ms <MS>`.
- **Exit codes**: next section; MCP tools return receipts/error objects and do not use process exit codes.
- **Output**: result commands emit a single JSON object/array to stdout (all logs go to stderr, redirect-safe); `daemon`, `hook`, and `mcp` are streaming/long-running.

---

## ❌ Error types & exit codes

`CodeNexusError` (`src/service/error.rs`, re-exported at the crate root) unifies library and service-layer errors; the `exit_code()` method maps to stable process exit codes:

| Exit code | Variants | Scenarios |
|-----------|----------|-----------|
| 0 | — | Success |
| 1 | `Internal` / `Io` / `Json` / `Discover` / `Daemon` / `Cache` / `Embed` / `Lsp` / `Index` (some) | Internal errors, I/O, JSON serialization, capability bootstrap failures, etc. |
| 2 | `InvalidInput` / `ProjectNotFound` / `Query` / `Trace` / `Storage` / `Resolve` / `Phase` | Invalid input, project not found, query / trace / storage / resolution errors |
| 4 | `NotFound` | Not found, database corrupt (`Index`'s `DatabaseCorrupt` also maps to 4) |
| 5 | `--fresh` deletion failure | Dedicated code for failing to delete the old DB file (internal error; `--fresh` is only supported by the `index` subcommand) |
| 6 | `--fresh` invalid `--db` | Safety rejection when `--db` points at a non-`.lbug` file (`InvalidInput`) |

`KitError` maps independently via `kit_exit_code`. Typical `IndexError` variants include `PathNotFound` (path does not exist) and others.

---

## 🎛️ Feature gating

| Module / API | Required feature | Notes |
|--------------|------------------|-------|
| `parse` (per language) | `lang-*` | 22 language features; at least one must be enabled |
| `analysis` | `analysis` | Dead code / architecture overview / complexity / community / cross-service |
| `diagram` | `diagram` (includes `analysis`) | Architecture diagrams and semantic Delta |
| `embed` | `embeddings` | Vector-embedding semantic search |
| `lsp` | `lsp` | LSP-enriched parsing |
| `cache` | `cache` | Query cache |
| `daemon` | `daemon` | File-watching daemon |
| `service` (CLI) | `cli` | Required by the CLI binary |
| `service` (MCP) | `mcp` | MCP server |

Compile-time guarantee: the `compile_error!` assertion at the top of `src/lib.rs` requires at least one `lang-*` or `lsp` feature, otherwise compilation is rejected with a clear message. Under non-`full` combinations some code paths compile but do not run (`allow(dead_code, ...)`); strict linting runs only under `full`.

---

## 💡 Usage example

Adapted from [`examples/src/bin/basic_indexing.rs`](../../examples/src/bin/basic_indexing.rs):

```rust
use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;

fn main() {
    let db_path = std::path::Path::new(".codenexus/demo.lbug");

    // 1. 索引：写内嵌示例源码到临时目录后索引
    let facade = IndexFacade::new(db_path).expect("IndexFacade::new");
    let result = facade
        .index(std::path::Path::new("src"), "demo-project", true)
        .expect("index");

    println!("Files indexed: {}", result.files_indexed);
    println!("Nodes created: {}", result.nodes_created);
    println!("Edges created: {}", result.edges_created);
    println!("Project ID:    {}", result.project_id);

    // 2. 查询：Cypher 列出函数
    let query = QueryFacade::new(db_path).expect("QueryFacade::new");
    let qr = query
        .cypher("MATCH (f:Function) RETURN f.name, f.qualifiedName LIMIT 10")
        .expect("cypher failed");
    for row in &qr.rows {
        let name = row[0].as_str().unwrap_or("?");
        let qn = row[1].as_str().unwrap_or("?");
        println!("  {name} @ {qn}");
    }
}
```

More runnable examples live in the `examples/` directory (`cypher_query`, `symbol_search`, `call_tracing`, `impact_analysis`, `export_import`); how to run them is in the [User Guide · Examples](USER_GUIDE.md#-examples).

---

## 📚 Related docs

| Document | Content |
|----------|---------|
| [📖 User Guide](USER_GUIDE.md) | Tutorial for every CLI subcommand |
| [⚡ Performance Guide](PERFORMANCE.md) | Benchmark data and tuning advice |
| [🔒 Security doc](SECURITY.md) | Security policy and vulnerability reporting |
