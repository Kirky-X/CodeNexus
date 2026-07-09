---
name: codenexus
description: Code knowledge graph indexer and query tool. Use when indexing code, tracing calls, analyzing impact, or querying the graph. Triggers: index codebase, trace calls, impact analysis, codenexus.
when_to_use: Triggers include "index a codebase", "query the graph", "trace calls from X", "what's the impact of changing X", "search for symbol", "start the daemon", "list projects", "clean project", "export/import graph", "show context of X", "what changed", "rename X", "set up MCP", or any reference to CodeNexus / codenexus CLI.
---

# CodeNexus CLI Skill

## Description

CodeNexus is a code knowledge graph indexing tool. It parses source code (C, Rust, Fortran, Python, TypeScript, Go, Java, C++) using tree-sitter, builds a queryable graph in LadybugDB, and supports call-chain tracing, data-flow analysis, cross-language FFI tracking, semantic search, change-impact analysis, refactoring proposals, and MCP server integration.

Use this Skill when you need to index a codebase, query its structure, trace function calls or data flow, analyze the impact of changes, search for symbols, watch files for incremental updates, manage projects, export/import graph artifacts, inspect a symbol's 360° context, detect symbols affected by git changes, propose renames, or set up MCP integration with AI agents.

## Prerequisites

Install from crates.io (recommended):

```bash
cargo install codenexus
```

Or build from source:

```bash
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo build --release
```

The binary is at `target/release/codenexus` (or `~/.cargo/bin/codenexus` if installed via `cargo install`). For semantic search (optional), install with:

```bash
cargo install codenexus --features embed
```

Feature presets: `minimal` (Rust only), `core` (C+Rust+Python), `full` (all 8 languages + daemon + analysis + complexity + api-review + community + cross-service + lsp + mcp). The `mcp` feature enables the sdforge-based MCP server (`codenexus mcp`). At least one `lang-*` feature is required — the crate fails to compile otherwise.

## Commands

CodeNexus has 17 subcommands grouped into five functional areas.

### Indexing & querying

#### index — Index a codebase

```bash
codenexus index <PATH> --name <PROJECT_NAME> [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--force` — Re-parse every file, ignoring cached hashes
- `--lsp` — Reserved for future use (currently unimplemented)
- `--embed` — Generate embeddings for semantic search (requires `embed` feature)
- `--ram-first` — RAM-first indexing (H15): LZ4-compress sources into memory, parse from memory, single `COPY FROM` dump. Recommended for repos < 1 GB source. Default is streaming.

**Output (JSON):** `project_id`, `files_indexed`, `files_skipped`, `nodes_created`, `edges_created`, `duration_ms`

#### query — Execute a Cypher query

```bash
codenexus query "<CYPHER>" [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--project <NAME>` — Optional project filter (informational)

**Output (JSON):** `columns`, `rows`, `duration_ms`

> ADR-021: A Cypher subset validator (`validate_cypher_subset`) in `src/query/cypher_subset.rs` is **wired into `query_cmd.rs`** and rejects destructive clauses (`CREATE`/`DELETE`/`SET`/`MERGE`/`REMOVE`/`CALL`/`LOAD CSV`/`FOREACH`) at the CLI boundary. Safe to use with LLM-generated queries.

#### search — Search for symbols

```bash
codenexus search <TEXT> [OPTIONS]
```

**Options:**
- `--semantic` — Use vector similarity search (requires `embed` feature)
- `--limit <N>` — Maximum results (default: 10)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--uid <UID>` — Narrow by node UID — direct lookup (H14)
- `--file <PATH>` — Narrow by file path (H14)
- `--kind <LABEL>` — Narrow by node label, e.g. `"Function"` (H14)

**Output (JSON):** Array of `{name, label, file_path, start_line, qualified_name, score}`

### Tracing & impact

#### trace — Trace a symbol's paths

```bash
codenexus trace <SYMBOL> [OPTIONS]
```

**Options:**
- `--type <TYPE>` — Trace type: `calls`, `dataflow`, or `all` (default: `all`)
- `--depth <N>` — Maximum traversal depth (default: 3)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--min-confidence <0.0-1.0>` — Drop edges with lower confidence. `0.85` keeps only SameFile + ImportScoped edges (design.md D4).
- `--uid <UID>` — Disambiguate by node UID (H14)
- `--file <PATH>` — Disambiguate by file path (H14)
- `--kind <LABEL>` — Disambiguate by node label (H14)

**Output (JSON):** `paths[].nodes`, `paths[].edges`, `paths[].depth`

#### impact — Analyze impact radius

Performs reverse traversal to find all symbols that depend on the target.

```bash
codenexus impact <SYMBOL> [OPTIONS]
```

**Options:**
- `--depth <N>` — Maximum reverse-traversal depth (default: 3)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--min-confidence <0.0-1.0>` — Drop edges with lower confidence
- `--uid <UID>` / `--file <PATH>` / `--kind <LABEL>` — Disambiguation (H14)

**Output (JSON):** List of affected symbols with their paths.

#### context — Show a 360° view of a symbol (H8)

Shows the resolved node, incoming edges (callers/importers/readers/writers), outgoing edges (callees/imports/uses), and processes/routes/endpoints the symbol participates in.

```bash
codenexus context <SYMBOL> [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--depth <N>` — BFS expansion depth for the surrounding subgraph (default: 2)

### Project management

#### status — Show indexing status

```bash
codenexus status [--db <DB_PATH>]
```

**Output (JSON):** List of projects with their indexing metadata.

#### list — List indexed projects

```bash
codenexus list [--db <DB_PATH>]
```

**Output (JSON):** Array of project names and metadata.

#### clean — Remove a project's index

```bash
codenexus clean <PROJECT_NAME> [--db <DB_PATH>]
```

Removes a project and all its associated nodes and edges from the database.

### Daemon & team artifacts

#### daemon — Start file-watching daemon

```bash
codenexus daemon <PATH> --name <PROJECT_NAME> [OPTIONS]
```

**Options:**
- `--debounce-ms <MS>` — Debounce window in milliseconds (default: 2000)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Behavior:** Watches the directory recursively; only `.c`/`.h`/`.rs`/`.f90`/`.f`/`.f95`/`.py`/`.ts`/`.tsx` trigger indexing; debounces consecutive changes; pauses event processing during indexing; runs until interrupted (Ctrl+C). Requires the `daemon` feature.

#### export — Export graph to a team artifact (H7)

```bash
codenexus export [OPTIONS]
```

Dumps the LadybugDB database to a zstd-compressed artifact (`codenexus.graph.zst`) with a JSON manifest (version, timestamp, source DB path).

**Options:**
- `--output <PATH>` — Output artifact path (default: `./codenexus.graph.zst`)
- `--db <DB_PATH>` — Database path to export (default: `./codenexus.lbug`)
- `--project <NAME>` — Project name to include in the manifest

#### import — Import a team artifact (H7)

```bash
codenexus import [OPTIONS]
```

Decompresses a team artifact and loads it into a LadybugDB database. Optionally triggers an incremental reindex of the local diff.

**Options:**
- `--input <PATH>` — Input artifact path (default: `./codenexus.graph.zst`)
- `--db <DB_PATH>` — Database path to import into (default: `./codenexus.lbug`)
- `--reindex` — Trigger incremental reindex after import (requires `--path` and `--name`)
- `--path <PATH>` — Codebase root path for reindex
- `--name <NAME>` — Project name for reindex

### Refactoring & MCP integration

#### detect-changes — Detect symbols affected by git changes (H8)

Runs `git diff` in `--path` and maps each touched file/line range to indexed symbols, then classifies each affected symbol's risk level by incoming edge count.

```bash
codenexus detect-changes <PATH> [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--mode <MODE>` — Git diff mode: `unstaged` (default), `staged`, or `head` (vs HEAD)

#### rename — Propose graph + text edits for renaming a symbol (H8)

Proposes graph-edits for high-confidence edges and text-search edits for review. Always runs in dry-run mode by default; `--apply` writes text edits to disk (graph edits are applied via a subsequent `index` run).

```bash
codenexus rename <FROM> <TO> [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--path <PATH>` — Codebase root path (required for text-search edits)
- `--apply` — Apply text edits to disk (default: dry-run, only print the plan)

#### setup — Auto-detect AI agents and write MCP config (H13)

Auto-detects installed AI coding agents (Claude Code, Cursor, Codex) under `$HOME` and writes the MCP server config for `codenexus mcp` into each agent's config file. Existing entries pointing to a different binary prompt for confirmation unless `--force` is given.

```bash
codenexus setup [--force]
```

#### hook — Emit PreToolUse/PostToolUse hook JSON (H13)

Reads a PreToolUse/PostToolUse JSON payload from stdin and emits a no-op acknowledgment. The hook always exits 0, never blocks a tool call, and never intercepts `Read` tool invocations.

```bash
codenexus hook [--db <DB_PATH>]
```

#### mcp — Serve MCP tools over stdio (H13)

Starts an sdforge-based MCP server over stdio, exposing 5 tools: `query` (execute Cypher), `trace` (trace symbol paths), `impact` (blast radius analysis), `search` (symbol search), `context` (360° symbol view). Launched by AI agents via the config written by `codenexus setup`. This replaces the previous hand-written JSON-RPC implementation.

```bash
codenexus mcp [--db <DB_PATH>]
```

## Typical Workflows

### Workflow 1: Index and explore a new codebase

```bash
codenexus index /path/to/repo --name myproject
codenexus query "MATCH (f:Function) RETURN f.name, f.filePath, f.startLine ORDER BY f.name LIMIT 50"
codenexus search "parse"
codenexus trace main --type calls --depth 5
codenexus impact critical_function --depth 5
```

### Workflow 2: Continuous indexing with daemon

```bash
codenexus index /path/to/repo --name myproject
codenexus daemon /path/to/repo --name myproject --debounce-ms 1000
# In another terminal:
codenexus query "MATCH (f:Function)-[:CALLS]->(g:Function) RETURN f.name, g.name LIMIT 20"
```

### Workflow 3: Multi-project management

```bash
codenexus index /path/to/project-a --name projectA --db /shared/graph.lbug
codenexus index /path/to/project-b --name projectB --db /shared/graph.lbug
codenexus list --db /shared/graph.lbug
codenexus clean projectA --db /shared/graph.lbug
```

### Workflow 4: Cross-language FFI tracing

```bash
codenexus index /path/to/mixed-repo --name ffiproject
codenexus trace rust_entry_point --type calls --depth 10
codenexus query "MATCH (a:Function)-[:FFI_CALLS]->(b:Function) RETURN a.name, b.name, a.filePath, b.filePath"
```

### Workflow 5: Refactoring with confidence filtering

```bash
# Only trust same-file + import-scoped edges (design.md D4)
codenexus impact critical_function --depth 5 --min-confidence 0.85
codenexus trace data_var --type dataflow --min-confidence 0.85
# Detect what a git change touches, then propose a rename
codenexus detect-changes /repo --mode unstaged
codenexus rename old_name new_name --path /repo --db /repo/codenexus.lbug
codenexus rename old_name new_name --path /repo --apply
```

### Workflow 6: Team artifact sharing

```bash
# On machine A: export the indexed graph
codenexus export --db /work/graph.lbug --project myproject --output myproject.graph.zst
# On machine B: import and reindex local diff
codenexus import --input myproject.graph.zst --db /work/graph.lbug --reindex --path /repo --name myproject
```

### Workflow 7: MCP integration with AI agents

```bash
# One-time: write MCP config into detected agents
codenexus setup
# Agents then launch `codenexus mcp` automatically; or run manually for testing:
codenexus mcp --db /work/graph.lbug
```

## Supported Languages

| Language | Extensions | Key Extractions |
|----------|-----------|-----------------|
| C | `.c`, `.h` | Functions, calls, `#include`, typedef, globals |
| Rust | `.rs` | `fn`, `struct`, `enum`, `trait`, `impl`, `extern "C"`, `use` |
| Fortran | `.f90`, `.f`, `.f95` | `subroutine`, `function`, `module`, `ISO_C_BINDING`, `call` |
| Python | `.py` | `def`, `class`, `import`, `__init__.py` |
| TypeScript | `.ts`, `.tsx` | `function`, `class`, `import`, `export` |
| Go | `.go` | `func`, `struct`, `interface`, `import` |
| Java | `.java` | `class`, `method`, `import`, `package` |
| C++ | `.cpp`, `.cc`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++` | `class`, `function`, `#include`, `namespace`, `template` |

## Node Types (44)

**Structural (4):** Project, Folder, File, Module
**Type definitions (5):** Class, Struct, Enum, Trait, Impl
**Callables (2):** Function, Method
**Variables (5):** Variable, GlobalVar, Parameter, Const, Static
**Meta (5):** Macro, TypeAlias, Typedef, Namespace, Interface
**H1 Type definitions (5):** Constructor, Property, Record, Delegate, Annotation
**H1 Templates (1):** Template
**H1 Union/Variant/Field (3):** Union, Variant, Field
**H1 Runtime/architecture (7):** Event, Handler, Middleware, Service, Endpoint, Route, Process
**H1 Data/infra (2):** Database, Config
**H1 Quality/docs (2):** Test, Section
**H1 Community/extension (3):** Community, Tool, Embedding

## Edge Types (24)

**Original (14):** CONTAINS, DEFINES, MEMBER_OF, CALLS, FFI_CALLS, DATAFLOWS, READS, WRITES, IMPLEMENTS, EXTENDS, USES_TYPE, REFERENCES, IMPORTS, INCLUDES
**H1 T9 extension (10):** HAS_METHOD, HAS_PROPERTY, ACCESSES, METHOD_OVERRIDES, METHOD_IMPLEMENTS, STEP_IN_PROCESS, HANDLES_ROUTE, FETCHES, HANDLES_TOOL, ENTRY_POINT_OF

Each edge carries a `confidence` score in `[0.0, 1.0]` and a `confidenceTier` (`SameFile` / `ImportScoped` / `Global`) populated during resolution. Use `--min-confidence` on `trace`/`impact` to filter by tier/score (design.md D4).

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Invalid input (path not found, bad arguments) |
| 2 | Database locked (retry) |
| 3 | System error (out of memory) |
| 4 | Database corrupt |

## Examples

Runnable example programs live in `examples/src/bin/`:

| Example | Covers |
|---------|--------|
| `basic_indexing` | `index` + Cypher query |
| `cypher_query` | `query` with multiple Cypher patterns |
| `symbol_search` | `search` with empty-result handling |
| `call_tracing` | `trace --type calls` + graph construction |
| `impact_analysis` | `impact` reverse BFS |
| `export_import` | DB copy (note: production uses `codenexus export`/`import` with zstd + manifest) |

Run with: `cargo run --manifest-path examples/Cargo.toml --bin <name>`

> The examples use `IndexFacade`/`QueryFacade`/`TraceFacade` directly rather than the Kit registry, because Kit creates per-subsystem connections that can cause file-DB visibility issues. Direct Facade use is the recommended programmatic API.
