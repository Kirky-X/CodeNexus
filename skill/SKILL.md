---
name: codenexus
description: "Code knowledge graph indexer and query tool. Use when indexing code, tracing calls, analyzing impact, or querying the graph. Triggers: index codebase, trace calls, impact analysis, codenexus. when_to_use: Triggers include 'index a codebase', 'query the graph', 'trace calls from X', 'what's the impact of changing X', 'search for symbol', 'start the daemon', 'list projects', 'clean project', 'export/import graph', 'show context of X', 'what changed', 'rename X', 'set up MCP', or any reference to CodeNexus / codenexus CLI."
---

# CodeNexus CLI Skill

> **Verified against `codenexus 0.4.2`.** The CLI is **strictly flag-based**: there are **no positional arguments**, and almost every function parameter is a **mandatory flag** (plain `String`/`u32`/`bool` types in the source map to required flags — only `--db <DB_PATH>` and `--debounce-ms <MS>` are global options with defaults). Booleans are pass-by-value (e.g. `--force true`, `--apply true`, `--cross_service false`).

## Description

CodeNexus is a code knowledge graph indexing tool. It parses source code (C, Rust, Fortran, Python, TypeScript, Go, Java, C++) using tree-sitter, builds a queryable graph in LadybugDB, and supports call-chain tracing, data-flow analysis, cross-language FFI tracking, semantic search, change-impact analysis, refactoring proposals, LSP integration, and MCP server integration.

Use this Skill when you need to index a codebase, query its structure, trace function calls or data flow, analyze the impact of changes, search for symbols, watch files for incremental updates, manage projects, export/import graph artifacts, inspect a symbol's 360° context, detect symbols affected by git changes, propose renames, query an LSP server, or set up MCP integration with AI agents.

## Conventions (apply to every subcommand)

- **No positional arguments.** Everything is a named flag. `codenexus query "MATCH ..."` fails; use `codenexus query --cypher "MATCH ..."`.
- **Booleans take a value**: `--force true`, `--apply true`, `--cross_service false`, `--embed false`, `--ram_first false`, `--enhanced false`.
- **Project filter accepts BOTH name and id.** All commands that take `--project <VALUE>` resolve the value via `resolve_project_id` (in `src/service/project.rs`): if the value matches a stored project `name`, the canonical project `id` is used; otherwise the value is treated as a raw project id. Pass `--project ""` (empty string) only where explicitly documented to disable the filter.
- **Plain-typed params are required.** Function parameters with plain `String`/`u32`/`bool` types (no `Option<T>`) map to **mandatory** CLI flags with no built-in defaults. Empty strings are allowed where the source explicitly handles them (e.g. `--edge_types ""`, `--path_filter ""`, `--protocol ""`).
- **Global options** on every command: `--db <DB_PATH>` and `--debounce-ms <MS>` (default `2000`, daemon-only relevance — safe to ignore otherwise).
- **Default DB path** (when `--db` is omitted): `.codenexus/<project>.lbug`, where `<project>` is sanitized from the subcommand's `--name` arg (preferred), or the dirname of its `--path` arg, or the fallback `codenexus`. The `.codenexus/` directory is auto-created. Example: `codenexus index --path /home/me/myrepo --name myrepo ...` resolves to `.codenexus/myrepo.lbug`. To override, pass `--db /custom/path.lbug`.
- **Stderr noise**: every connection prints warnings like `inklog ... Failed to set log crate logger` and `storage::connection - skipping unsupported DDL statement`. These are benign; filter with `2>/dev/null` to see clean JSON, or `2>&1 | grep -v "WARN\|skipping unsupported"`.
- **JSON output**: every command that returns a result prints a single JSON object/array to stdout (commands like `daemon`, `hook`, `mcp` are streaming/long-running and do not emit a final JSON blob).

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

CodeNexus has **28 subcommands** grouped into eight functional areas.

### Indexing & project management

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
- `--embed <BOOL>` — Deprecated: embedding is now controlled by the `embed` cargo feature. A `--embed true` only emits a warning to stderr. Required; pass `--embed false`.
- `--ram_first <BOOL>` — RAM-first indexing (H15): LZ4-compress sources into memory, parse from memory, single `COPY FROM` dump. Recommended for repos < 1 GB source. Default is streaming. (required; e.g. `--ram_first false`)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project_id`, `files_indexed`, `files_skipped`, `nodes_created`, `edges_created`, `duration_ms`

> Note: inklog may emit `indexing started` INFO messages that can appear on stdout/stderr interleaved with the result JSON. Pipe through `2>/dev/null` or parse the last JSON line on stdout to extract the result.

#### list — List indexed projects

```bash
codenexus list [--db <DB_PATH>]
```

**Output (JSON):** Array of project entries (`id`, `name`, `root_path`, `file_count`, `indexed_at`, `last_commit`).

#### status — Show indexing status with staleness check

```bash
codenexus status [--db <DB_PATH>]
```

**Output (JSON):** List of projects with their indexing metadata and a `stale` flag indicating whether the working tree has moved past `last_commit`.

#### clean — Remove a project's index

```bash
codenexus clean --project <PROJECT_NAME_OR_ID> [--db <DB_PATH>]
```

- `--project <NAME_OR_ID>` — Project name or id to remove (required, named flag — NOT positional). Resolved via `resolve_project_id`.
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

Removes a project and all its associated nodes and edges from the database.

#### daemon — Start file-watching daemon

```bash
codenexus daemon --path <PATH> --name <PROJECT_NAME> [--debounce-ms <MS>] [--db <DB_PATH>]
```

**Options:**
- `--path <PATH>` — Directory to watch (required)
- `--name <NAME>` — Project name (required)
- `--debounce-ms <MS>` — Debounce window in milliseconds (global option, default: 2000)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Behavior:** Watches the directory recursively; only `.c`/`.h`/`.rs`/`.f90`/`.f`/`.f95`/`.py`/`.ts`/`.tsx` trigger indexing; debounces consecutive changes; pauses event processing during indexing; runs until interrupted (Ctrl+C). Requires the `daemon` feature. Does not emit a final JSON blob (long-running).

### Querying & search

#### query — Execute a Cypher query

```bash
codenexus query --cypher "<CYPHER>" [--db <DB_PATH>]
```

**Options:**
- `--cypher <CYPHER>` — The Cypher query string (**required**, named flag — NOT positional)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

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

# Edges live in the CodeRelation node table (LadybugDB design — see Storage Model below)
codenexus query --cypher "MATCH (r:CodeRelation) WHERE r.type='CALLS' RETURN r.source, r.target LIMIT 20" --db codenexus.lbug
```

#### search — Search for symbols

> ⚠️ **Known caveat (verified on `codenexus.lbug` built for 0.4.2):** `search` currently **returns `{"count":0,"results":[]}` for all queries** against an index built by the default `index` run — both `exact`/`regex`/`fuzzy`/`graph` name modes and `--fulltext true` BM25. The name-search tables are not populated by the default indexing flow; semantic search additionally requires the `embed` feature. Until this is fixed, **use `query`** (e.g. `MATCH (f:Function) WHERE f.name CONTAINS 'parse' RETURN ...`) as the reliable symbol lookup path.

```bash
codenexus search --text <TEXT> --fulltext <BOOL> --limit <N> --mode <MODE> --project <NAME_OR_ID> [--db <DB_PATH>]
```

All of `--text`, `--fulltext`, `--limit`, `--mode`, `--project` are **required**.

**Options:**
- `--text <TEXT>` — Search text (required)
- `--fulltext <BOOL>` — `true` for BM25 full-text over `content`/`docstring`; `false` for structured name search (required)
- `--limit <N>` — Maximum results (required; e.g. `--limit 10`)
- `--mode <MODE>` — `exact` (case-insensitive substring), `regex` (Rust regex over name/qualifiedName), `fuzzy` (Levenshtein), `graph` (name + degree/label filter), or `multi` (multi-signal scoring) (required)
- `--project <NAME_OR_ID>` — Project filter (name or id); empty string `""` = no filter (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `{"count":N,"results":[{name, label, file_path, start_line, qualified_name, score, match_reason, degree}]}`

### Tracing & impact

#### trace — Trace a symbol's paths

```bash
codenexus trace --symbol <SYMBOL> --trace_type <TYPE> --depth <N> --path_filter <GLOB> --detect_cycles <BOOL> --cross_service <BOOL> [--db <DB_PATH>]
```

All of `--symbol`, `--trace_type`, `--depth`, `--path_filter`, `--detect_cycles`, `--cross_service` are **required**.

**Options:**
- `--symbol <SYMBOL>` — Symbol name to trace (required)
- `--trace_type <TYPE>` — `calls`, `dataflow`, or `all` (required)
- `--depth <N>` — Maximum traversal depth (required; e.g. `--depth 3`)
- `--path_filter <GLOB>` — Glob to restrict path nodes' `filePath` (e.g. `"/src/api/**"`); empty string `""` = no filter (required)
- `--detect_cycles <BOOL>` — `true` appends a `cycles` array of detected call-graph cycles (DFS white/gray/black) (required)
- `--cross_service <BOOL>` — `true` traverses `HTTP_CALLS` and reverse `HANDLES_ROUTE` edges for cross-service chains (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `symbol`, `paths[].nodes`, `paths[].edges`, `paths[].depth`, `cycles[]` (when `--detect_cycles true`)

#### impact — Analyze impact radius

Performs reverse traversal to find all symbols that depend on the target.

> ⚠️ **Performance:** On large graphs (the bundled `codenexus.lbug` has ~928k nodes), `impact` with broad `--edge_types`/`--max_depth` and a high-degree target **exceeds the 120s tool timeout** (e.g. `run_index --depth 3 --edge_types "" --max_depth 5` timed out). Keep it tractable: narrow `--edge_types` to a few types, lower `--max_depth`, and pick a small/leaf target. A scoped run (`--edge_types "CALLS" --max_depth 3`) on a leaf function completes in seconds and still returns thousands of dependent nodes.

```bash
codenexus impact --symbol <SYMBOL> --depth <N> --edge_types <LIST> --max_depth <N> --include_tests <BOOL> [--db <DB_PATH>]
```

All of `--symbol`, `--depth`, `--edge_types`, `--max_depth`, `--include_tests` are **required**.

**Options:**
- `--symbol <SYMBOL>` — Target symbol name (required)
- `--depth <N>` — Maximum reverse-traversal depth (required; e.g. `--depth 3`)
- `--edge_types <LIST>` — Comma-separated UPPERCASE edge types to traverse (e.g. `"CALLS,IMPLEMENTS,USES_TYPE"`). Empty string `""` uses defaults (`CALLS` + `IMPLEMENTS` + `USES_TYPE`) (required)
- `--max_depth <N>` — Max BFS depth for enhanced analysis (clamped to 10). When non-zero, enables risk assessment (required; e.g. `--max_depth 5`)
- `--include_tests <BOOL>` — `true` includes `TESTS` edges in the reverse BFS; default `false` (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `symbol`, `depth`, `node_count`, `edge_count`, `nodes[]`, `edges[]`, `risk_assessment` (when enhanced), `affected[]` (when enhanced, each `ImpactNode` has `id`, `name`, `qualified_name`, `edge_type`, `depth`, `path`)

#### context — Show a 360° view of a symbol (H8)

Shows the resolved node, incoming edges (callers/importers/readers/writers), outgoing edges (callees/imports/uses), and processes/routes/endpoints the symbol participates in.

```bash
codenexus context --symbol <SYMBOL> --depth <N> --project <NAME_OR_ID> --enhanced <BOOL> [--db <DB_PATH>]
```

All of `--symbol`, `--depth`, `--project`, `--enhanced` are **required**.

**Options:**
- `--symbol <SYMBOL>` — Symbol name (required)
- `--depth <N>` — BFS expansion depth for the surrounding subgraph (required; e.g. `--depth 2`)
- `--project <NAME_OR_ID>` — Project name or id (required; used by `--enhanced true`)
- `--enhanced <BOOL>` — `true` returns the multi-dimensional `SymbolContext` (symbol definition + type context + module context + test context); `false` returns the legacy caller/callee/processes view (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `{symbol, node, incoming[], outgoing[], processes[], routes[], endpoints[]}` (legacy) or the enhanced `SymbolContext`.

> ⚠️ **Known issue (Problem F):** `context --enhanced true` currently raises `CodeNexus error occurred` against some projects. The enhanced mode is unstable in 0.4.2; prefer `--enhanced false` until fixed.

### Code analysis

#### dead_code — Detect unreferenced functions

Identifies `Function`/`Method` nodes with zero incoming edges (across 9 configured edge types) that are not entry points (`main`/`Main`/`__main__`/`wmain`/`WinMain`/`DLLMain`) or test functions (`test_*`/`*_test`/`*_spec`). Each finding includes a confidence level: `High` (zero incoming of any type), `Medium` (non-CALLS edges only), `Low` (CALLS exists but excluded by config).

```bash
codenexus dead_code --project <NAME_OR_ID> --entry <PATTERNS> --check_exported <BOOL> --check_ffi <BOOL> --edge_types <LIST> [--db <DB_PATH>]
```

All of `--project`, `--entry`, `--check_exported`, `--check_ffi`, `--edge_types` are **required**.

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--entry <PATTERNS>` — Comma-separated glob patterns for entry-point function names (required; e.g. `"main,Main,__main__,wmain,WinMain,DLLMain"`)
- `--check_exported <BOOL>` — `true` excludes `isExported=true` nodes (required)
- `--check_ffi <BOOL>` — `true` excludes `extern "C"` / `#[no_mangle]` signatures as FFI entry points (required)
- `--edge_types <LIST>` — Comma-separated UPPERCASE edge types whose incoming edges mark a function as "used" (required; e.g. `"CALLS,FFI_CALLS,IMPLEMENTS,HANDLES_ROUTE,USAGE,TESTS,USES_TYPE,HTTP_CALLS,ASYNC_CALLS"`)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `dead_code[]` (each entry: `name`, `qualified_name`, `file_path`, `start_line`, `language`, `reason`, `confidence`)

> Note: For Rust projects, tree-sitter's static analysis does not capture trait-object `dyn` dispatch or all cross-module calls, so `dead_code` may produce false positives (e.g. `parse`, `route`, `new`). Treat results as a triage list, not ground truth.

#### architecture — Show architecture overview

Returns a high-level architecture overview including module boundaries (directory groups with cohesion scores), dependency directions (circular dependency detection), layers (Controller/Service/Repository/Model classification), entry points, and hotspots.

```bash
codenexus architecture --project <NAME_OR_ID> [--db <DB_PATH>]
```

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required, named flag — NOT positional)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `overview` (containing `languages[]`, `packages[]`, `layers[]`, `module_boundaries[]`, `dependency_directions[]`, `cross_service_deps[]`, `entry_points[]`, `hotspots[]`)

#### complexity — AST-based complexity metrics

Analyzes per-function complexity: cyclomatic, cognitive, nesting depth, function length, Halstead volume, maintainability index, time complexity, and space complexity. Each metric is classified into `Green`/`Yellow`/`Red`/`Critical` (or 3-level for space complexity). When the stored `Function.content` is empty, the analyzer falls back to reading the source range from disk using the Project's `rootPath` + `filePath` + `startLine`/`endLine`.

```bash
codenexus complexity \
  --project <NAME_OR_ID> \
  --red_only <BOOL> \
  --sort_by_severity <BOOL> \
  --cyclomatic_green <N> --cyclomatic_yellow <N> --cyclomatic_red <N> \
  --cognitive_green <N> --cognitive_yellow <N> --cognitive_red <N> \
  --nesting_green <N> --nesting_yellow <N> --nesting_red <N> \
  --func_length_green <N> --func_length_yellow <N> --func_length_red <N> \
  --halstead_volume_green <N> --halstead_volume_yellow <N> --halstead_volume_red <N> \
  --maintainability_green <N> --maintainability_yellow <N> --maintainability_red <N> \
  --time_complexity_green "<O(..)>" --time_complexity_yellow "<O(..)>" --time_complexity_red "<O(..)>" \
  --space_complexity_yellow "<O(..)>" --space_complexity_red "<O(..)>" \
  [--db <DB_PATH>]
```

All 26 parameters are **required** (no defaults). Pass `0` for numeric thresholds you want to leave at the in-memory defaults, and `""` for time/space complexity thresholds you want to leave unset.

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--red_only <BOOL>` — `true` keeps only Red/Critical findings (required)
- `--sort_by_severity <BOOL>` — `true` sorts results by severity descending (required)
- `--cyclomatic_green <N>` / `--cyclomatic_yellow <N>` / `--cyclomatic_red <N>` — cyclomatic thresholds (required; in-memory defaults 10/20/25 — pass `0` to keep them)
- `--cognitive_green <N>` / `--cognitive_yellow <N>` / `--cognitive_red <N>` — cognitive thresholds (required; in-memory defaults 10/15/20)
- `--nesting_green <N>` / `--nesting_yellow <N>` / `--nesting_red <N>` — nesting depth thresholds (required; in-memory defaults 3/5/6)
- `--func_length_green <N>` / `--func_length_yellow <N>` / `--func_length_red <N>` — function length thresholds (required; in-memory defaults 30/100/200)
- `--halstead_volume_green <N>` / `--halstead_volume_yellow <N>` / `--halstead_volume_red <N>` — Halstead volume thresholds (required; in-memory defaults 100/1000/8000)
- `--maintainability_green <N>` / `--maintainability_yellow <N>` / `--maintainability_red <N>` — maintainability index thresholds (required; in-memory defaults 85/65/25)
- `--time_complexity_green "<O(..)>"` / `--time_complexity_yellow "<O(..)>"` / `--time_complexity_red "<O(..)>"` — time complexity class thresholds; valid values: `O(1)`, `O(log n)`, `O(n)`, `O(n log n)`, `O(n^2)`, `O(n^3)`, `O(2^n)` (required; in-memory defaults `O(log n)`/`O(n)`/`O(n^2)` — pass `""` to leave unset)
- `--space_complexity_yellow "<O(..)>"` / `--space_complexity_red "<O(..)>"` — space complexity class thresholds; valid values: `O(1)`, `O(n)`, `O(n^2)` (required; in-memory defaults `O(1)`/`O(n)` — pass `""` to leave unset)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `complexity[]` (each entry has function identity + per-metric severity + raw values), `summary` (totals by severity)

> Note: With all threshold flags set to `0`/`""`, the analyzer uses its in-memory defaults. The flags are required by the CLI even when you want defaults — this is a UX trade-off in 0.4.2. Requires the `complexity` feature.

#### community — Louvain community detection

Detects communities (clusters) in the call graph using Louvain modularity optimization.

```bash
codenexus community --project <NAME_OR_ID> --resolution <RESOLUTION> [--db <DB_PATH>]
```

Both `--project` and `--resolution` are **required**.

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--resolution <RESOLUTION>` — Louvain resolution parameter (required; pass empty string `""` for default, or a float like `"1.5"`)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `resolution`, `communities[]` (each entry has community id + member nodes)

#### cross_service — Detect cross-service call links

Matches HTTP route definitions (`Route` nodes) against caller-side string literals in `Function` bodies. Supports multi-protocol detection: HTTP REST, gRPC, GraphQL, message queue (Kafka/RabbitMQ), and event bus (Socket.IO/EventEmitter).

```bash
codenexus cross_service --project <NAME_OR_ID> --protocol <PROTO> [--db <DB_PATH>]
```

Both `--project` and `--protocol` are **required**.

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--protocol <PROTO>` — Filter by protocol: `http_rest`, `grpc`, `graphql`, `message_queue`, `event_bus`, or empty string `""` for all protocols (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `links[]` (HTTP REST matches), `matches[]` (multi-protocol matches with `protocol`, `match_type`, `confidence`)

#### route_map — List API routes and handlers

Lists HTTP API routes discovered in the source and their handler functions.

```bash
codenexus route_map --project <NAME_OR_ID> [--db <DB_PATH>]
```

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `route_map[]` (each entry has route path/method + handler symbol + file/line)

#### tool_map — List MCP tools and handlers

Lists MCP tool definitions discovered in the source (e.g. `#[tool(...)]` annotations) and their handler functions.

```bash
codenexus tool_map --project <NAME_OR_ID> [--db <DB_PATH>]
```

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `tool_map[]` (each entry has tool name + handler symbol + file/line)

#### shape_check — Validate API endpoint shape consistency

Validates that API endpoints follow consistent request/response shape conventions (e.g. pagination envelope, error shape).

```bash
codenexus shape_check --project <NAME_OR_ID> [--db <DB_PATH>]
```

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `violations[]` (each entry has endpoint + violation description + severity)

#### api_impact — Trace callers affected by an API change

Traces which callers would be affected if a given API endpoint changes.

```bash
codenexus api_impact --project <NAME_OR_ID> --endpoint <ENDPOINT> [--db <DB_PATH>]
```

Both `--project` and `--endpoint` are **required**.

**Options:**
- `--project <NAME_OR_ID>` — Project name or id (required)
- `--endpoint <ENDPOINT>` — Endpoint path or identifier to analyze (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `project`, `endpoint`, `impact[]` (each entry has caller symbol + edge type + path)

### Refactoring

#### detect_changes — Detect symbols affected by git changes (H8)

Runs `git diff` in `--path` and maps each touched file/line range to indexed symbols, then classifies each affected symbol's risk level by incoming edge count.

```bash
codenexus detect_changes --path <PATH> --mode <MODE> [--db <DB_PATH>]
```

Both `--path` and `--mode` are **required**.

**Options:**
- `--path <PATH>` — Repository path to diff (required, named flag — NOT positional)
- `--mode <MODE>` — Git diff mode: `unstaged`, `staged`, or `head` (vs HEAD) (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `{path, mode, files_changed, affected[]}`

#### rename — Propose graph + text edits for renaming a symbol (H8)

> **Flag change:** the old positional form `codenexus rename <FROM> <TO>` is **gone**. Use `--from` / `--to`. `--apply` is now a value-style boolean (`--apply true`).

Proposes graph-edits for high-confidence edges and text-search edits for review. Always runs in dry-run mode by default; `--apply true` writes text edits to disk (graph edits are applied via a subsequent `index` run).

```bash
codenexus rename --from <OLD> --to <NEW> --path <PATH> --apply <BOOL> [--db <DB_PATH>]
```

All of `--from`, `--to`, `--path`, `--apply` are **required**.

**Options:**
- `--from <OLD>` — Current symbol name (required)
- `--to <NEW>` — New symbol name (required; must match `[A-Za-z_][A-Za-z0-9_]*`)
- `--path <PATH>` — Codebase root path; empty string `""` skips text-search edits (required)
- `--apply <BOOL>` — `true` applies text edits to disk; `false` is dry-run (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `graph_edit` (with `new_qualified_name`, `edges[]`), `text_edits[]`

> ⚠️ **Known issue (Problem C):** In 0.4.2 the dry-run `graph_edit.new_qualified_name` may still show the **old** qualified name (suffix not updated). When the project has multiple same-named symbols (e.g. `parser::parse` vs `CalcError::parse`), `rename` only matches one of them with no ambiguity prompt. Verify the proposed edits carefully before `--apply true`.

### LSP integration

#### lsp_goto_def — Query LSP Go-to-Definition

Auto-detects the language server from the file extension (Rust → rust-analyzer, TypeScript → tsserver, etc.) and queries the definition location at the given position.

```bash
codenexus lsp_goto_def --file <FILE> --line <N> --col <N> --workspace <PATH> [--db <DB_PATH>]
```

All of `--file`, `--line`, `--col`, `--workspace` are **required**.

**Options:**
- `--file <FILE>` — Source file path (required)
- `--line <N>` — 1-based line number (required)
- `--col <N>` — 1-based column number (required)
- `--workspace <PATH>` — Workspace root path (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `{found, location}` where `location` is `{file, line, col}` when found. Requires the `lsp` feature and a running language server for the file's language.

> Note: When no language server is available, the command prints an Error to stderr but may still exit 0 (see Problem E in `temp/problem.md`). Scripts cannot rely on the exit code alone to detect LSP failures — inspect the JSON `found` field.

#### lsp_hover — Query LSP Hover info

Auto-detects the language server and queries hover documentation at the given position.

```bash
codenexus lsp_hover --file <FILE> --line <N> --col <N> --workspace <PATH> [--db <DB_PATH>]
```

All of `--file`, `--line`, `--col`, `--workspace` are **required**.

**Options:**
- `--file <FILE>` — Source file path (required)
- `--line <N>` — 1-based line number (required)
- `--col <N>` — 1-based column number (required)
- `--workspace <PATH>` — Workspace root path (required)
- `--db <DB_PATH>` — Database path (default: `.codenexus/<project>.lbug`; see Conventions)

**Output (JSON):** `{found, contents, range}` where `contents` is markdown text and `range` is `{start_line, start_col, end_line, end_col}`. Requires the `lsp` feature and a running language server.

### Team artifacts

#### export — Export graph to a team artifact (H7)

```bash
codenexus export --output <PATH> --project <NAME_OR_ID> [--db <DB_PATH>]
```

Both `--output` and `--project` are **required**.

Dumps the LadybugDB database to a zstd-compressed artifact with a JSON manifest (version, timestamp, source DB path).

**Options:**
- `--output <PATH>` — Output artifact path (required; e.g. `./codenexus.graph.zst`)
- `--project <NAME_OR_ID>` — Project name or id to include in the manifest (required)
- `--db <DB_PATH>` — Database path to export (default: `.codenexus/<project>.lbug`; see Conventions)

> ⚠️ **Known issue (Problem D):** In 0.4.2 the export manifest writes `"codenexus_version":"0.3.4"` regardless of the actual binary version. Imported artifacts carry this forward. This is cosmetic (does not affect graph data) but will be fixed in a future release.

#### import — Import a team artifact (H7)

```bash
codenexus import --input <PATH> --reindex <BOOL> --path <PATH> --name <NAME> [--db <DB_PATH>]
```

All of `--input`, `--reindex`, `--path`, `--name` are **required** (in 0.4.2 the CLI marks them mandatory even when `--reindex false`).

Decompresses a team artifact and loads it into a LadybugDB database. Optionally triggers an incremental reindex of the local diff when `--reindex true`.

**Options:**
- `--input <PATH>` — Input artifact path (required; e.g. `./codenexus.graph.zst`)
- `--reindex <BOOL>` — `true` triggers incremental reindex after import (required)
- `--path <PATH>` — Codebase root path for reindex (required; pass `""` when `--reindex false`)
- `--name <NAME>` — Project name for reindex (required; pass `""` when `--reindex false`)
- `--db <DB_PATH>` — Database path to import into (default: `.codenexus/<project>.lbug`; see Conventions)

### MCP & agent integration

#### setup — Auto-detect AI agents and write MCP config (H13)

Auto-detects installed AI coding agents (Claude Code, Cursor, Codex) under `$HOME` and writes the MCP server config for `codenexus mcp` into each agent's config file. Existing entries pointing to a different binary prompt for confirmation unless `--force true` is given.

```bash
codenexus setup --force <BOOL>
```

- `--force <BOOL>` — `true` overwrites existing config entries without prompting; `false` prompts (required)

**Output (JSON):** `{configured, skipped}` (lists of agent names that were configured vs. skipped)

#### hook — Emit PreToolUse/PostToolUse hook JSON (H13)

Reads a PreToolUse/PostToolUse JSON payload from stdin and emits a no-op acknowledgment. The hook always exits 0, never blocks a tool call, and never intercepts `Read` tool invocations.

```bash
codenexus hook [--db <DB_PATH>]
```

**Output (JSON):** `{decision, summary}` where `decision` is typically `"pass"`. Long-running (reads stdin).

#### mcp — Serve MCP tools over stdio (H13)

Starts an sdforge-based MCP server over stdio, exposing tools such as `query` (execute Cypher), `trace` (trace symbol paths), `impact` (blast radius analysis), `search` (symbol search), `context` (360° symbol view). Launched by AI agents via the config written by `codenexus setup`. This replaces the previous hand-written JSON-RPC implementation.

```bash
codenexus mcp [--db <DB_PATH>]
```

Long-running (serves stdio until interrupted). Requires the `mcp` feature.

## Storage Model — CodeRelation Node Table

> Important for `query` users: CodeNexus stores edges as **nodes** in a `CodeRelation` NODE TABLE, not as LadybugDB REL relationships. This is by design (see `src/storage/schema.rs:80-97`): LadybugDB's `REL TABLE` requires concrete node-table names for FROM/TO and cannot express a general edge across 44 node types.

**Implication for Cypher:**
- `MATCH ()-[r]->()` and `MATCH ()-[r:CALLS]->()` **return 0** — there are no REL relationships.
- To traverse edges, query the `CodeRelation` node table:
  ```cypher
  -- Count edges by type
  MATCH (r:CodeRelation) WHERE r.type='CALLS' RETURN count(r)

  -- Find callers of a symbol (reverse traversal)
  MATCH (r:CodeRelation) WHERE r.type='CALLS' AND r.target='my_namespace::my_func'
  RETURN r.source, r.filePath

  -- Find callees of a symbol (forward traversal)
  MATCH (r:CodeRelation) WHERE r.type='CALLS' AND r.source='my_namespace::my_func'
  RETURN r.target, r.filePath
  ```
- `CodeRelation` columns: `id`, `source`, `target`, `type`, `confidence`, `confidenceTier`, `reason`, `startLine`, `project`. `source`/`target` hold the symbol qualifiedName (equal to `Function.id`).
- High-level commands (`trace`, `impact`, `dead_code`, `architecture`, `community`, etc.) abstract over this layout — they read `CodeRelation` internally so you don't have to write the join yourself.

## Typical Workflows

> Every command below uses the **verified 0.4.2 flag syntax** (no positional args, mandatory flags supplied, booleans value-styled). `--project` accepts either name or id.

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
# In another terminal, query the live graph:
codenexus query --cypher "MATCH (r:CodeRelation) WHERE r.type='CALLS' RETURN r.source, r.target LIMIT 20"
```

### Workflow 3: Multi-project management

```bash
codenexus index --path /path/to/project-a --name projectA --force false --lsp false --embed false --ram_first false --db /shared/graph.lbug
codenexus index --path /path/to/project-b --name projectB --force false --lsp false --embed false --ram_first false --db /shared/graph.lbug
codenexus list --db /shared/graph.lbug
# clean accepts either name or id:
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
codenexus rename --from old_name --to new_name --path /repo --apply false --db /repo/codenexus.lbug
codenexus rename --from old_name --to new_name --path /repo --apply true
```

### Workflow 6: Team artifact sharing

```bash
# On machine A: export the indexed graph
codenexus export --output myproject.graph.zst --project myproject --db /work/graph.lbug
# On machine B: import and reindex local diff
codenexus import --input myproject.graph.zst --reindex true --path /repo --name myproject --db /work/graph.lbug
```

### Workflow 7: MCP integration with AI agents

```bash
# One-time: write MCP config into detected agents
codenexus setup --force false
# Agents then launch `codenexus mcp` automatically; or run manually for testing:
codenexus mcp --db /work/graph.lbug
```

### Workflow 8: Complexity audit

```bash
# All 26 threshold flags are required. Pass 0/"" to keep in-memory defaults.
codenexus complexity \
  --project myproject \
  --red_only false --sort_by_severity true \
  --cyclomatic_green 0 --cyclomatic_yellow 0 --cyclomatic_red 0 \
  --cognitive_green 0 --cognitive_yellow 0 --cognitive_red 0 \
  --nesting_green 0 --nesting_yellow 0 --nesting_red 0 \
  --func_length_green 0 --func_length_yellow 0 --func_length_red 0 \
  --halstead_volume_green 0 --halstead_volume_yellow 0 --halstead_volume_red 0 \
  --maintainability_green 0 --maintainability_yellow 0 --maintainability_red 0 \
  --time_complexity_green "" --time_complexity_yellow "" --time_complexity_red "" \
  --space_complexity_yellow "" --space_complexity_red ""
```

### Workflow 9: API surface analysis

```bash
codenexus route_map --project myproject          # list HTTP routes + handlers
codenexus tool_map --project myproject           # list MCP tools + handlers
codenexus shape_check --project myproject        # check endpoint shape consistency
codenexus api_impact --project myproject --endpoint "/api/v1/users"
codenexus cross_service --project myproject --protocol ""
```

## Supported Languages

| Language | Extensions | Key extractions |
|----------|-----------|-----------------|
| C | `.c`, `.h` | Functions, calls, `#include`, typedef, globals |
| Rust | `.rs` | `fn`, `struct`, `enum`, `trait`, `impl`, `extern "C"`, `use` |
| Fortran | `.f90`, `.f`, `.f95` | `subroutine`, `function`, `module`, `ISO_C_BINDING`, `call` |
| Python | `.py` | `def`, `class`, `import`, `__init__.py` |
| TypeScript | `.ts`, `.tsx` | `function`, `class`, `import`, `export` |
| Go | `.go` | `func`, `struct`, `interface`, `import` |
| Java | `.java` | `class`, `method`, `import`, `package` |
| C++ | `.cpp`, `.cc`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++` | `class`, `function`, `#include`, `namespace`, `template` |

> Static analysis limitation: For Rust in particular, trait-object `dyn` dispatch and many cross-module calls are not captured by tree-sitter. The resulting call graph is a lower bound. The reserved `--lsp true` flag and the `lsp_goto_def`/`lsp_hover` commands point at the future LSP-augmented path.

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

## Edge Types (24, stored as `CodeRelation` nodes)

**Original (14):** CONTAINS, DEFINES, MEMBER_OF, CALLS, FFI_CALLS, DATAFLOWS, READS, WRITES, IMPLEMENTS, EXTENDS, USES_TYPE, REFERENCES, IMPORTS, INCLUDES
**H1 T9 extension (10):** HAS_METHOD, HAS_PROPERTY, ACCESSES, METHOD_OVERRIDES, METHOD_IMPLEMENTS, STEP_IN_PROCESS, HANDLES_ROUTE, FETCHES, HANDLES_TOOL, ENTRY_POINT_OF

Each `CodeRelation` row carries a `confidence` score in `[0.0, 1.0]` and a `confidenceTier` (`SameFile` / `ImportScoped` / `Global`) populated during resolution. Use `--edge_types` on `impact` and `--path_filter` on `trace` to scope results by edge type or file path (design.md D4).

## Exit Codes

Exit codes are produced by `CodeNexusError::exit_code()` in `src/service/error.rs` (with `IndexError::exit_code()` for index-flow errors). The mapping is:

| Code | Meaning | Source variants |
|------|---------|-----------------|
| 0 | Success (also: non-fatal parse errors that do not abort indexing) | `IndexError::Parse(_)` |
| 1 | Internal / system error (IO, JSON serialization, Kit, daemon, cache, embed, LSP, discover) | `Internal`, `Io`, `Json`, `Discover`, `Daemon`, `Cache`, `Embed`, `Lsp`, `IndexError::PathNotFound`, `IndexError::Io`, `IndexError::Discover` |
| 2 | Client / database error (invalid input, project not found, query/trace/storage errors, DB locked, resolve/phase errors) | `InvalidInput`, `ProjectNotFound`, `Query`, `Trace`, `Storage`, `Resolve`, `Phase`, `IndexError::DatabaseLocked`, `IndexError::Storage` |
| 3 | (reserved, currently unused) | — |
| 4 | Not found / database corrupt | `NotFound`, `IndexError::DatabaseCorrupt` |

> ⚠️ **Known issue (Problem E):** Some failure paths still exit 0 in 0.4.2 (e.g. `lsp_hover`/`lsp_goto_def` when no language server is running, and `list --db <missing path>` which returns `[]`). Scripts in CI must inspect the JSON payload, not rely solely on the exit code, for these commands.

## Known Issues in 0.4.2

Tracked in `temp/problem.md`:

| ID | Severity | Command(s) affected | Summary |
|----|----------|---------------------|---------|
| A | P0 (fixed) | All analysis commands | `--project` now resolves name → id via `resolve_project_id` (`src/service/project.rs`). |
| B | P1 (fixed) | `complexity` | `Function.content` is empty after `index`; `complexity` falls back to reading source from disk via Project `rootPath` + `filePath` + line range. |
| C | P2 | `rename` | Dry-run `graph_edit.new_qualified_name` may still show the old name; ambiguous same-named symbols are silently resolved to one match. |
| D | P2 | `export`/`import` | Export manifest hardcodes `"codenexus_version":"0.3.4"` regardless of binary version. |
| E | P3 | `lsp_*`, `list`, `complexity` | Some failure paths still exit 0. CI scripts must inspect JSON. |
| F | P3 | `context --enhanced true` | Raises a generic `CodeNexus error occurred` against some projects. Use `--enhanced false` until fixed. |
| Search | P2 | `search` | Returns `{"count":0,"results":[]}` against default indexes. Use `query` for symbol lookup. |
| Rust call graph | P1 | `dead_code`, `trace`, `impact` (Rust) | Trait `dyn` dispatch and cross-module calls are not captured; treat results as a lower bound. |

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
