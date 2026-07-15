---
name: codenexus
description: "Code knowledge graph indexer and query tool. Use when indexing code, tracing calls, analyzing impact, or querying the graph. Triggers: index codebase, trace calls, impact analysis, codenexus. when_to_use: Triggers include 、'index a codebase', 'query the graph', 'trace calls from X', 'what's the impact of changing X', 'search for symbol', 'start the daemon', 'list projects', 'clean project', 'export/import graph', 'show context of X', 'what changed', 'rename X', 'set up MCP', or any reference to CodeNexus / codenexus CLI."
---

# CodeNexus CLI Skill

> **Verified against `codenexus 0.4.2`.** The CLI in this version is **strictly flag-based**: there are **no positional arguments**, and several flags that were optional in older docs are now **mandatory**. Every command below shows the exact flags the current binary requires. Booleans are pass-by-value (e.g. `--force true`, `--apply true`, `--cross_service false`).

## Description

CodeNexus is a code knowledge graph indexing tool. It parses source code (C, Rust, Fortran, Python, TypeScript, Go, Java, C++) using tree-sitter, builds a queryable graph in LadybugDB, and supports call-chain tracing, data-flow analysis, cross-language FFI tracking, semantic search, change-impact analysis, refactoring proposals, and MCP server integration.

Use this Skill when you need to index a codebase, query its structure, trace function calls or data flow, analyze the impact of changes, search for symbols, watch files for incremental updates, manage projects, export/import graph artifacts, inspect a symbol's 360° context, detect symbols affected by git changes, propose renames, or set up MCP integration with AI agents.

## Conventions (apply to every subcommand)

- **No positional arguments.** Everything is a named flag. `codenexus query "MATCH ..."` fails; use `codenexus query --cypher "MATCH ..."`.
- **Booleans take a value**: `--force true`, `--apply true`, `--cross_service false`, `--embed false`, `--ram_first false`, `--lsp false`.
- **Project filter**: most read commands accept `--project <NAME>`. Pass `--project ""` (empty string) to disable the project filter when you want all indexed projects in the DB.
- **Global options** on every command: `--db <DB_PATH>` (default `./codenexus.lbug`) and `--debounce-ms <MS>` (default `2000`, daemon-only relevance — safe to ignore otherwise).
- **Stderr noise**: every connection prints warnings like `inklog ... Failed to set log crate logger` and `storage::connection - skipping unsupported DDL statement`. These are benign; filter with `2>/dev/null` to see clean JSON, or `2>&1 | grep -v "WARN\|skipping unsupported"`.
- **JSON output**: every command prints a single JSON object/array to stdout.

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
codenexus index --path <PATH> --name <PROJECT_NAME> --force <BOOL> --lsp <BOOL> --embed <BOOL> --ram_first <BOOL> [OPTIONS]
```

All of `--path`, `--name`, `--force`, `--lsp`, `--embed`, `--ram_first` are **required** (booleans value-style).

**Options:**
- `--path <PATH>` — Codebase root to index (required)
- `--name <NAME>` — Project name (required)
- `--force <BOOL>` — Re-parse every file, ignoring cached hashes (required; e.g. `--force false`)
- `--lsp <BOOL>` — Reserved for future use (currently unimplemented; required; e.g. `--lsp false`)
- `--embed <BOOL>` — Generate embeddings for semantic search (requires `embed` feature; required; e.g. `--embed false`)
- `--ram_first <BOOL>` — RAM-first indexing (H15): LZ4-compress sources into memory, parse from memory, single `COPY FROM` dump. Recommended for repos < 1 GB source. Default is streaming. (required; e.g. `--ram_first false`)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `project_id`, `files_indexed`, `files_skipped`, `nodes_created`, `edges_created`, `duration_ms`

#### query — Execute a Cypher query

```bash
codenexus query --cypher "<CYPHER>" [--db <DB_PATH>]
```

**Options:**
- `--cypher <CYPHER>` — The Cypher query string (**required**, named flag — NOT positional)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `columns`, `rows`, `duration_ms`

> ADR-021: A Cypher subset validator (`validate_cypher_subset`) in `src/query/cypher_subset.rs` is **wired into `query_cmd.rs`** and rejects destructive clauses (`CREATE`/`DELETE`/`SET`/`MERGE`/`REMOVE`/`CALL`/`LOAD CSV`/`FOREACH`) at the CLI boundary. Safe to use with LLM-generated queries.

**Examples:**
```bash
# Count nodes / functions
codenexus query --cypher "MATCH (n) RETURN count(n) AS c" --db codenexus.lbug
codenexus query --cypher "MATCH (f:Function) RETURN count(f) AS func_count" --db codenexus.lbug

# List functions with their file + line
codenexus query --cypher "MATCH (f:Function) RETURN f.name, f.filePath, f.startLine ORDER BY f.name LIMIT 50" --db codenexus.lbug

# Find a single function by exact name
codenexus query --cypher "MATCH (f:Function) WHERE f.name = 'index_cmd' RETURN f.name, f.filePath LIMIT 5" --db codenexus.lbug

# Call graph (note: aggregate with WITH ... RETURN, not inline aggregates)
codenexus query --cypher "MATCH (f:Function)-[:CALLS]->(g) WITH f, count(*) AS c RETURN f.name AS caller, c ORDER BY c DESC LIMIT 10" --db codenexus.lbug

# Cross-language FFI
codenexus query --cypher "MATCH (a:Function)-[:FFI_CALLS]->(b:Function) RETURN a.name, b.name, a.filePath, b.filePath" --db codenexus.lbug
```

#### search — Search for symbols

> ⚠️ **Known caveat (verified on `codenexus.lbug` built for 0.4.2):** `search` currently **returns `{"count":0,"results":[]}` for all queries** against an existing index — both `exact`/`regex`/`fuzzy`/`graph` name modes and `--fulltext true` BM25. This appears to be an index/feature mismatch (the name-search tables are not populated by the default `index` run; semantic search additionally requires the `embed` feature). Until this is fixed, **use `query`** (e.g. `MATCH (f:Function) WHERE f.name CONTAINS 'parse' RETURN ...`) as the reliable symbol lookup path.

```bash
codenexus search --text <TEXT> --fulltext <BOOL> --mode <MODE> --project <NAME> [--limit <N>] [--db <DB_PATH>]
```

All of `--text`, `--fulltext`, `--mode`, `--project` are **required**.

**Options:**
- `--text <TEXT>` — Search text (required)
- `--fulltext <BOOL>` — `true` for BM25 full-text over `content`/`docstring`; `false` for structured name search (required)
- `--mode <MODE>` — `exact` (case-insensitive substring), `regex` (Rust regex over name/qualifiedName), `fuzzy` (Levenshtein), `graph` (name + degree/label filter), or `multi` (multi-signal scoring) (required)
- `--project <NAME>` — Project filter; empty string `""` = no filter (required)
- `--limit <N>` — Maximum results (default: 10)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `{"count":N,"results":[{name, label, file_path, start_line, qualified_name, score, match_reason, degree}]}`

### Tracing & impact

#### trace — Trace a symbol's paths

```bash
codenexus trace --symbol <SYMBOL> --trace_type <TYPE> --depth <N> --path_filter <GLOB> --detect_cycles <BOOL> --cross_service <BOOL> [--db <DB_PATH>]
```

All of `--symbol`, `--trace_type`, `--path_filter`, `--detect_cycles`, `--cross_service` are **required**.

**Options:**
- `--symbol <SYMBOL>` — Symbol name to trace (required)
- `--trace_type <TYPE>` — `calls`, `dataflow`, or `all` (required)
- `--depth <N>` — Maximum traversal depth (required; e.g. `--depth 3`)
- `--path_filter <GLOB>` — Glob to restrict path nodes' `filePath` (e.g. `"/src/api/**"`); empty string `""` = no filter (required)
- `--detect_cycles <BOOL>` — `true` appends a `cycles` array of detected call-graph cycles (DFS white/gray/black) (required)
- `--cross_service <BOOL>` — `true` traverses `HTTP_CALLS` and reverse `HANDLES_ROUTE` edges for cross-service chains (required)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `symbol`, `paths[].nodes`, `paths[].edges`, `paths[].depth`, `cycles[]` (when `--detect_cycles true`)

#### impact — Analyze impact radius

Performs reverse traversal to find all symbols that depend on the target.

> ⚠️ **Performance:** On large graphs (the bundled `codenexus.lbug` has ~928k nodes), `impact` with broad `--edge_types`/`--max_depth` and a high-degree target **exceeds the 120s tool timeout** (e.g. `run_index --depth 3 --edge_types "" --max_depth 5` timed out). Keep it tractable: narrow `--edge_types` to a few types, lower `--max_depth`, and pick a small/leaf target. A scoped run (`--edge_types "CALLS" --max_depth 3`) on a leaf function completes in seconds and still returns thousands of dependent nodes.

```bash
codenexus impact --symbol <SYMBOL> --depth <N> --edge_types <LIST> --max_depth <N> --include_tests <BOOL> [--db <DB_PATH>]
```

All of `--symbol`, `--edge_types`, `--max_depth`, `--include_tests` are **required**.

**Options:**
- `--symbol <SYMBOL>` — Target symbol name (required)
- `--depth <N>` — Maximum reverse-traversal depth (required; e.g. `--depth 3`)
- `--edge_types <LIST>` — Comma-separated UPPERCASE edge types to traverse (e.g. `"CALLS,IMPLEMENTS,USES_TYPE"`). Empty string `""` uses defaults (`CALLS` + `IMPLEMENTS` + `USES_TYPE`) (required)
- `--max_depth <N>` — Max BFS depth for enhanced analysis (default: 5, clamped to 10). When non-zero, enables risk assessment (required)
- `--include_tests <BOOL>` — `true` includes `TESTS` edges in the reverse BFS; default `false` (required)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `symbol`, `depth`, `node_count`, `edge_count`, `nodes[]`, `edges[]`, `risk_assessment` (when enhanced), `affected[]` (when enhanced, each `ImpactNode` has `id`, `name`, `qualified_name`, `edge_type`, `depth`, `path`)

#### context — Show a 360° view of a symbol (H8)

Shows the resolved node, incoming edges (callers/importers/readers/writers), outgoing edges (callees/imports/uses), and processes/routes/endpoints the symbol participates in.

```bash
codenexus context --symbol <SYMBOL> [--depth <N>] [--project <NAME>] [--enhanced <BOOL>] [--db <DB_PATH>]
```

**Options:**
- `--symbol <SYMBOL>` — Symbol name (required)
- `--depth <N>` — BFS expansion depth for the surrounding subgraph (default: 2)
- `--project <NAME>` — Project name (required for `--enhanced true`)
- `--enhanced <BOOL>` — `true` returns the multi-dimensional `SymbolContext` (symbol definition + type context + module context + test context); `false` (default) returns the legacy caller/callee/processes view
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `{symbol, node, incoming[], outgoing[], processes[], routes[], endpoints[]}` (legacy) or the enhanced `SymbolContext`.

### Analysis

#### dead_code — Detect unreferenced functions

Identifies `Function`/`Method` nodes with zero incoming edges (across 9 configured edge types) that are not entry points (`main`/`Main`/`__main__`/`wmain`/`WinMain`/`DLLMain`) or test functions (`test_*`/`*_test`/`*_spec`). Each finding includes a confidence level: `High` (zero incoming of any type), `Medium` (non-CALLS edges only), `Low` (CALLS exists but excluded by config).

```bash
codenexus dead_code --project <NAME> --entry <PATTERNS> --check_exported <BOOL> --check_ffi <BOOL> --edge_types <LIST> [--db <DB_PATH>]
```

All of `--project`, `--entry`, `--check_exported`, `--check_ffi`, `--edge_types` are **required**.

**Options:**
- `--project <NAME>` — Project name (required)
- `--entry <PATTERNS>` — Comma-separated glob patterns for entry-point function names (required; default `main,Main,__main__,wmain,WinMain,DLLMain`)
- `--check_exported <BOOL>` — `true` (default) excludes `isExported=true` nodes (required)
- `--check_ffi <BOOL>` — `true` (default) excludes `extern "C"` / `#[no_mangle]` signatures as FFI entry points (required)
- `--edge_types <LIST>` — Comma-separated UPPERCASE edge types whose incoming edges mark a function as "used" (required; default `CALLS,FFI_CALLS,IMPLEMENTS,HANDLES_ROUTE,USAGE,TESTS,USES_TYPE,HTTP_CALLS,ASYNC_CALLS`)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `project`, `dead_code[]` (each entry: `name`, `qualified_name`, `file_path`, `start_line`, `language`, `reason`, `confidence`)

#### cross_service — Detect cross-service call links

Matches HTTP route definitions (`Route` nodes) against caller-side string literals in `Function` bodies. Supports multi-protocol detection: HTTP REST, gRPC, GraphQL, message queue (Kafka/RabbitMQ), and event bus (Socket.IO/EventEmitter).

```bash
codenexus cross_service --project <NAME> [--protocol <PROTO>] [--db <DB_PATH>]
```

**Options:**
- `--project <NAME>` — Project name (required)
- `--protocol <PROTO>` — Filter by protocol: `http_rest` (default), `grpc`, `graphql`, `message_queue`, `event_bus`, or empty string for all protocols
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `project`, `links[]` (HTTP REST matches), `matches[]` (multi-protocol matches with `protocol`, `match_type`, `confidence`)

#### architecture — Show architecture overview

Returns a high-level architecture overview including module boundaries (directory groups with cohesion scores), dependency directions (circular dependency detection), layers (Controller/Service/Repository/Model classification), and cross-service dependencies.

```bash
codenexus architecture --project <NAME> [--db <DB_PATH>]
```

**Options:**
- `--project <NAME>` — Project name (required, named flag — NOT positional)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `project`, `overview` (containing `layers[]`, `module_boundaries[]`, `dependency_directions[]`, `cross_service_deps[]`, `entry_points[]`)

> Note: against the bundled `codenexus.lbug` this returned empty `layers`/`module_boundaries`/`entry_points` for `CodeNexus` — same index-format caveat as `search`. Prefer `query` for structural questions until re-indexed.

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

**Output (JSON):** Array of project entries (id, name, root_path, file_count, indexed_at, last_commit).

#### clean — Remove a project's index

```bash
codenexus clean --project <PROJECT_NAME> [--db <DB_PATH>]
```

- `--project <NAME>` — Project name to remove (required, named flag — NOT positional)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

Removes a project and all its associated nodes and edges from the database.

### Daemon & team artifacts

#### daemon — Start file-watching daemon

```bash
codenexus daemon --path <PATH> --name <PROJECT_NAME> [--debounce-ms <MS>] [--db <DB_PATH>]
```

**Options:**
- `--path <PATH>` — Directory to watch (required)
- `--name <NAME>` — Project name (required)
- `--debounce-ms <MS>` — Debounce window in milliseconds (default: 2000)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Behavior:** Watches the directory recursively; only `.c`/`.h`/`.rs`/`.f90`/`.f`/`.f95`/`.py`/`.ts`/`.tsx` trigger indexing; debounces consecutive changes; pauses event processing during indexing; runs until interrupted (Ctrl+C). Requires the `daemon` feature.

#### export — Export graph to a team artifact (H7)

```bash
codenexus export [--output <PATH>] [--project <NAME>] [--db <DB_PATH>]
```

Dumps the LadybugDB database to a zstd-compressed artifact (`codenexus.graph.zst`) with a JSON manifest (version, timestamp, source DB path).

**Options:**
- `--output <PATH>` — Output artifact path (default: `./codenexus.graph.zst`)
- `--project <NAME>` — Project name to include in the manifest
- `--db <DB_PATH>` — Database path to export (default: `./codenexus.lbug`)

#### import — Import a team artifact (H7)

```bash
codenexus import [--input <PATH>] [--reindex <BOOL>] [--path <PATH>] [--name <NAME>] [--db <DB_PATH>]
```

Decompresses a team artifact and loads it into a LadybugDB database. Optionally triggers an incremental reindex of the local diff.

**Options:**
- `--input <PATH>` — Input artifact path (default: `./codenexus.graph.zst`)
- `--reindex <BOOL>` — `true` triggers incremental reindex after import (requires `--path` and `--name`)
- `--path <PATH>` — Codebase root path for reindex
- `--name <NAME>` — Project name for reindex
- `--db <DB_PATH>` — Database path to import into (default: `./codenexus.lbug`)

### Refactoring & MCP integration

#### detect_changes — Detect symbols affected by git changes (H8)

Runs `git diff` in `--path` and maps each touched file/line range to indexed symbols, then classifies each affected symbol's risk level by incoming edge count.

```bash
codenexus detect_changes --path <PATH> [--mode <MODE>] [--db <DB_PATH>]
```

**Options:**
- `--path <PATH>` — Repository path to diff (required, named flag — NOT positional)
- `--mode <MODE>` — Git diff mode: `unstaged` (default), `staged`, or `head` (vs HEAD)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `{path, mode, files_changed, affected[]}`

#### rename — Propose graph + text edits for renaming a symbol (H8)

> **Flag change:** the old positional form `codenexus rename <FROM> <TO>` is **gone**. Use `--from` / `--to`. `--apply` is now a value-style boolean (`--apply true`).

Proposes graph-edits for high-confidence edges and text-search edits for review. Always runs in dry-run mode by default; `--apply true` writes text edits to disk (graph edits are applied via a subsequent `index` run).

```bash
codenexus rename --from <OLD> --to <NEW> [--path <PATH>] [--apply <BOOL>] [--db <DB_PATH>]
```

**Options:**
- `--from <OLD>` — Current symbol name (required)
- `--to <NEW>` — New symbol name (required)
- `--path <PATH>` — Codebase root path (required for text-search edits)
- `--apply <BOOL>` — `true` applies text edits to disk; default `false` (dry-run, prints the plan)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

#### setup — Auto-detect AI agents and write MCP config (H13)

Auto-detects installed AI coding agents (Claude Code, Cursor, Codex) under `$HOME` and writes the MCP server config for `codenexus mcp` into each agent's config file. Existing entries pointing to a different binary prompt for confirmation unless `--force` is given.

```bash
codenexus setup [--force <BOOL>]
```

- `--force <BOOL>` — `true` overwrites existing config entries without prompting

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

> Every command below uses the **verified 0.4.2 flag syntax** (no positional args, mandatory flags supplied, booleans value-styled).

### Workflow 1: Index and explore a new codebase

```bash
codenexus index --path /path/to/repo --name myproject --force false --lsp false --embed false --ram_first false
codenexus query --cypher "MATCH (f:Function) RETURN f.name, f.filePath, f.startLine ORDER BY f.name LIMIT 50"
# search is currently unreliable (see caveat); use query for symbol lookup:
codenexus query --cypher "MATCH (f:Function) WHERE f.name CONTAINS 'parse' RETURN f.name, f.filePath LIMIT 20"
codenexus trace --symbol main --trace_type calls --depth 5 --path_filter "" --detect_cycles false --cross_service false
codenexus impact --symbol critical_function --depth 3 --edge_types "CALLS" --max_depth 3 --include_tests false
```

### Workflow 2: Continuous indexing with daemon

```bash
codenexus index --path /path/to/repo --name myproject --force false --lsp false --embed false --ram_first false
codenexus daemon --path /path/to/repo --name myproject --debounce-ms 1000
# In another terminal:
codenexus query --cypher "MATCH (f:Function)-[:CALLS]->(g:Function) RETURN f.name, g.name LIMIT 20"
```

### Workflow 3: Multi-project management

```bash
codenexus index --path /path/to/project-a --name projectA --force false --lsp false --embed false --ram_first false --db /shared/graph.lbug
codenexus index --path /path/to/project-b --name projectB --force false --lsp false --embed false --ram_first false --db /shared/graph.lbug
codenexus list --db /shared/graph.lbug
codenexus clean --project projectA --db /shared/graph.lbug
```

### Workflow 4: Cross-language FFI tracing

```bash
codenexus index --path /path/to/mixed-repo --name ffiproject --force false --lsp false --embed false --ram_first false
codenexus trace --symbol rust_entry_point --trace_type calls --depth 10 --path_filter "" --detect_cycles false --cross_service false
codenexus query --cypher "MATCH (a:Function)-[:FFI_CALLS]->(b:Function) RETURN a.name, b.name, a.filePath, b.filePath"
```

### Workflow 5: Refactoring with multi-dimensional impact

```bash
# Narrow edge types + max_depth to keep impact tractable on large graphs (see performance note)
codenexus impact --symbol critical_function --depth 3 --edge_types "CALLS,IMPLEMENTS,USES_TYPE" --max_depth 3 --include_tests false
codenexus trace --symbol data_var --trace_type dataflow --depth 3 --path_filter "/src/**" --detect_cycles false --cross_service false
# Detect what a git change touches, then propose a rename
codenexus detect_changes --path /repo --mode unstaged
codenexus rename --from old_name --to new_name --path /repo --db /repo/codenexus.lbug
codenexus rename --from old_name --to new_name --path /repo --apply true
```

### Workflow 6: Team artifact sharing

```bash
# On machine A: export the indexed graph
codenexus export --db /work/graph.lbug --project myproject --output myproject.graph.zst
# On machine B: import and reindex local diff
codenexus import --input myproject.graph.zst --db /work/graph.lbug --reindex true --path /repo --name myproject
```

### Workflow 7: MCP integration with AI agents

```bash
# One-time: write MCP config into detected agents
codenexus setup --force false
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

Each edge carries a `confidence` score in `[0.0, 1.0]` and a `confidenceTier` (`SameFile` / `ImportScoped` / `Global`) populated during resolution. Use `--edge_types` on `impact` and `--path_filter` on `trace` to scope results by edge type or file path (design.md D4).

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
