# Commands Reference

> Detailed flags, options, and output schemas for all 28 CodeNexus subcommands. Part of the CodeNexus skill — see [SKILL.md](../SKILL.md) for the overview and command quick-reference table.

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
